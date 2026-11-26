//! The link between two machines (`docs/remote.md`, "Architecture", "The transport", "The
//! version"): an endpoint per machine, links authenticated by the machines' keys, two kinds of
//! traffic kept apart, and the version handshake when a link is made. The transport under it is
//! `transport.rs`'s; nothing here names it.
//!
//! On a link, each message goes on a stream of its own: one JSON frame one way, its answer the
//! other way. The first stream is the handshake: the side that dials sends its hello, the other
//! answers its own, and each chooses the protocol, or refuses, by itself.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::transport::{
    Conn, Failure, Frames, Inbound, Raw, RelaySocket, Reply, Transport, Unanswered,
};
use crate::{Hello, MachineKey, NeedsUpdate, PeerState, PublicKey, choose};

/// The longest frame a link takes. Bigger transfers (a git bundle) will have streams of their
/// own.
pub const FRAME_LIMIT: usize = 1 << 20;
/// How long dialling another device, or a handshake, may take.
const WAIT: Duration = Duration::from_secs(10);
/// How long the TLS of a link that came in may take: on loopback or a LAN it takes milliseconds.
const HANDSHAKE: Duration = Duration::from_secs(5);
/// The most links that came in whose TLS is under way at once: one that stalls holds a place,
/// not the others. A stalled handshake costs the endpoint a connection's state, a few kilobytes,
/// for `HANDSHAKE` at most: places are cheap.
const HANDSHAKES: usize = 256;
/// The most of them from one host (an IPv6 /64; the relay gives each key a /64 and a /48 of its
/// own): a device that stalls its own cannot hold the places of the others.
const HANDSHAKES_FROM_ONE: usize = 2;
/// The most of them from one IPv6 /48.
const HANDSHAKES_FROM_SITE: usize = 8;
/// How long a link let go (`Link::retire`) stays open at the least: what the other side sent on
/// it before it let it go too reaches this side within it, and is served or turned away. A frame
/// still being read when the link closes, or a turn-away not yet sent, is lost: its sender reads
/// a loss after it wrote the frame, and does not send it again.
const GRACE: Duration = Duration::from_secs(1);
/// The codes a side closes a link with.
const CLOSE_REFUSED: u32 = 1;
const CLOSE_NEEDS_UPDATE: u32 = 2;
const CLOSE_DONE: u32 = 3;
const CLOSE_HUB_GONE: u32 = 4;

/// The two kinds of traffic on a link, kept apart by the link's protocol name: the user's
/// requests (`client`), and the closed set of messages one machine sends another (`machine`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Traffic {
    Client,
    Machine,
}

impl Traffic {
    fn alpn(self) -> &'static [u8] {
        match self {
            Traffic::Client => b"flottille-client",
            Traffic::Machine => b"flottille-machine",
        }
    }

    fn of(alpn: &[u8]) -> Option<Traffic> {
        [Traffic::Client, Traffic::Machine]
            .into_iter()
            .find(|traffic| traffic.alpn() == alpn)
    }
}

/// What travels on a link: a machine's message on a `machine` link, a client's request on a
/// `client` link. Each side refuses the other kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Message {
    Machine(MachineMessage),
    /// A request of the user's, signed by their device (`docs/remote.md`, "Requests from another
    /// machine's clients"): carried as it is.
    Client(Value),
}

/// What a machine answers a message of a kind it does not know: a sender tells a machine of before
/// by it (`docs/remote.md`, "The version").
pub const UNKNOWN_MESSAGE: &str = "not a message a machine may send";

/// What a machine may send another (`docs/remote.md`, "What a machine may send"): nothing else is
/// taken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineMessage {
    /// The addresses a machine of the previous version tells on every link, for the direct link
    /// removed on 2026-11-21: taken and ignored, so that such a peer still links while the other
    /// machine is updated. This version never sends it.
    Addresses(Vec<String>),
    /// What a runner offers: its projects with their free slots, the agents signed in on it
    /// (`docs/remote.md`, "Roles"). Sent when a link is made, and again when it changes.
    Capabilities(crate::Capabilities),
    /// A place's code on its way, or a session's new commits on their way back (`docs/remote.md`,
    /// "What travels").
    Git(Git),
    /// A vault entry the user shared with the receiver (`docs/remote.md`, "Vault entries").
    Share(Share),
    /// A connection the user shared with the receiver, and the vault entries it names they chose
    /// to share with it (`docs/remote.md`, "Connections").
    ShareConnection(ConnectionShare),
    /// A session or a child started for an agent of the sender's (`agent_launch` with
    /// `machine`), judged by the receiving machine's gate (`docs/remote.md`, "Starting a session
    /// on another machine"). Answered `{"state": "accepted" | "held"}`, or refused; its code
    /// follows as `git` messages of the same launch.
    Launch(Box<LaunchAsk>),
    /// A delivery for an agent of the receiver, of a session it started on the sender
    /// (`docs/remote.md`, "Deliveries across machines"): a child's report, question or failure, a
    /// led session's end of turn. Kept by the sender until it is answered, sent again in order
    /// and once (its `id` is idempotent). Answered `{}`, `{"later": why}` when the receiver
    /// cannot take it now (the sender keeps it), or refused.
    Deliver(Box<Deliver>),
    /// One of the tree's tools aimed by an agent of the sender at a session of the receiver: one
    /// it started, or any other, judged there as a call of one of its own agents
    /// (`docs/remote.md`, "What a machine may send"). Answered `{"text"}` (with `outcome` and the
    /// `request` that holds it, for a session it did not start), `{"later": why}` when the
    /// receiver cannot take it now, or refused.
    Call(Box<Call>),
    /// The sender's live root sessions, light (`docs/remote.md`, "One list of every machine"):
    /// sent by a runner when a link is made and again when the list changes, kept by the
    /// receiver for its clients. Answered `{}`, or refused.
    Sessions(crate::SessionList),
    /// The sender's projects, each by the remotes of its repositories (`docs/remote.md`, "One
    /// list of every machine"): sent by a runner after its sessions and again when they change,
    /// kept by the receiver beside them. A message of its own so that a machine of before, which
    /// does not know it, refuses it alone and goes on taking the sessions. Answered `{}`, or
    /// refused.
    Projects(crate::ProjectList),
    /// The pilots of another machine of the sender's led sessions, each by its machine's name and
    /// its id there (`docs/remote.md`, "One list of every machine"): sent by a runner after its
    /// sessions and again when they change, kept by the receiver beside them. A message of its own
    /// for the reason of `projects`. Answered `{}`, or refused.
    Pilots(crate::PilotList),
    /// The user's close of a session of the receiver, signed by their device (`agent.stop`) and
    /// kept by the sender while the receiver was off: sent once it links again, kept until it is
    /// answered, and idempotent (a session ended already is answered as closed). Answered `{}`,
    /// `{"later": why}`, or refused.
    Close(Box<crate::Request>),
    /// The ghosts of the receiver's sessions, asked once a link by a machine that shows them
    /// (`docs/remote.md`, "Machines are one"): each from after the last event the sender holds of
    /// it. Answered `{}`, then, on the same stream, a [`crate::GhostNews`] a frame for as long as
    /// the link lasts, or refused. A machine of before refuses it as a kind it does not know: the
    /// sender then holds no ghost of its sessions.
    Ghost(crate::GhostAsk),
}

/// A delivery one machine sends another for one of the receiver's agents: taken only for the
/// agent that launched `launch` on the sender.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deliver {
    /// Its id on the sender: the receiver stores it once, however often it comes.
    pub id: String,
    /// The launch it is of, as the receiver named it.
    pub launch: String,
    /// The receiver's agent it is for: the launch's caller.
    pub agent: String,
    /// The delivery's kind (`child-report`, `session-turn-ended`…).
    pub kind: String,
    pub body: String,
    /// `between-turns` or `urgent`.
    pub urgency: String,
    /// Whether the session it is of has ended with it: the launch counts no more.
    #[serde(default)]
    pub ended: bool,
}

/// A tool of the tree an agent of the sender called on a session it started on the receiver:
/// taken only from that agent, for that session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    /// Its id on the sender: a message is delivered once, however often it comes.
    pub id: String,
    /// The launch that started the session, as the sender named it; empty for an
    /// `approval_answer`, whose request names the child's tree.
    pub launch: String,
    /// The sender's agent that calls: the launch's caller.
    pub caller: String,
    /// The receiver's agent it is aimed at: the one the launch started; empty for an
    /// `approval_answer`.
    pub agent: String,
    /// `agent_send`, `agent_status`, `agent_stop`, `session_send`, `session_read` or any tool of
    /// the tree that addresses a session.
    pub tool: String,
    /// The tool's arguments, as JSON.
    pub arguments: String,
    /// For a session the caller did not start (`launch` empty, `agent` the session): the
    /// caller's project, by its name, and its title, as its machine has them. Left out of a call
    /// of a launch, so that such a call reads as before.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The root of the caller's tree, and that tree's mode, `ask`, `auto` or `full`, which the
    /// call is taken in, as a launch's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tree: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// A switch to Full a device signed, which a machine of before sends with Full and none
    /// reads any more: Full follows the tree (`docs/remote.md`, "Machines are one").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch: Option<String>,
}

/// A launch another machine asks for one of its agents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchAsk {
    /// The sender's name for it: its code travels under it.
    pub launch: String,
    /// The sender's agent that launches: the child's parent, or the session's pilot.
    pub caller: String,
    /// The root of the caller's tree, whose mode the launch takes.
    pub tree: String,
    /// `child`, `led` or `handed`.
    pub kind: String,
    pub project: String,
    pub title: String,
    pub brief: String,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    /// The mode of the caller's tree: `ask`, `auto` or `full`, which the launch takes.
    pub mode: String,
    /// A switch to Full a device signed, which a machine of before sends with Full and none
    /// reads any more: Full follows the tree (`docs/remote.md`, "Machines are one").
    #[serde(default)]
    pub switch: Option<String>,
    /// The caller's branch, which the new workspace takes.
    pub branch: String,
    /// The repositories of the caller's place, by their paths in the project.
    pub repositories: Vec<String>,
    /// Each repository of the place as the receiver recognises it among its own projects,
    /// whatever their names and folders (`docs/remote.md`, "Which project").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identities: Vec<RepositoryIdentity>,
    /// The project's settings that travel, as its file writes them
    /// (`flottille_core::project::TRAVELLING`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
    /// The caller is itself a session another agent started (`docs/tools.md`, "No chain without
    /// the user"): a session it starts comes to the user there too. Left out when false, so that
    /// a launch of any other caller reads as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub launched: bool,
    /// The caller's depth in its tree, which a child takes plus one: the depth limit holds across
    /// machines. Left out at 0, so that a machine of before reads the launch: only a root of such
    /// a machine launches, so a launch without it is a root's.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub depth: u32,
    /// The files the project's setup copies that the caller's machine has, by their paths in the
    /// project: their names only, never what they hold. The receiver asks the user for those it
    /// lacks (`docs/remote.md`, "What travels").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copied: Vec<String>,
}

fn is_zero(depth: &u32) -> bool {
    *depth == 0
}

/// What a repository is, whatever its folder: its root commits, and its remote's URL when it has
/// one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryIdentity {
    /// Its path in the project.
    pub path: String,
    /// The commits of its branch with no parent.
    pub roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The `git` messages: what the receiver has, a bundle offered, its bytes, the workspace made of
/// it. A bundle never goes in one frame: the side it is offered to reads it on a stream of its
/// own (`Fetch`), and only that side may.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Git {
    /// Which commits the receiver has of a repository of a project: its branches and
    /// `refs/flottille/machines/*`. Answered `{"commits": […]}`, `null` for a repository it does
    /// not have yet.
    Has(GitRepository),
    /// A bundle to read and fetch. Answered once it is fetched, or refused.
    Offer(GitOffer),
    /// The bytes of a bundle offered to the side that asks: answered frame by frame, each
    /// `{"chunk": "<base64>"}`.
    Fetch(GitFetch),
    /// The workspace made of the bundles of a launch, one per repository, on the sender's branch
    /// name or the first free one after it. Answered `{"workspace", "branch", "folder"}`.
    Place(GitPlace),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitRepository {
    pub project: String,
    /// Its path in the project, `.` for the project's folder.
    pub repository: String,
    /// The launch it is asked for: code is taken only for a launch the receiver accepted.
    #[serde(default)]
    pub launch: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitOffer {
    /// What the receiver reads it by (`Fetch`).
    pub transfer: String,
    pub launch: String,
    pub project: String,
    pub repository: String,
    /// The sender's branch.
    pub branch: String,
    /// The bundle's length in bytes, and its SHA-256 in hexadecimal: what is read must be both.
    pub size: u64,
    pub sha256: String,
    /// New commits on their way back to the machine that sent the launch.
    #[serde(default)]
    pub back: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitFetch {
    pub transfer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitPlace {
    pub launch: String,
    pub project: String,
    pub branch: String,
    /// The repositories of the place, by their paths in the project, each received for this
    /// launch.
    pub repositories: Vec<String>,
}

/// A vault entry the user shared: their device's signed `vault.share`, which the receiver checks
/// itself (a machine alone shares nothing), the entry's metadata, and its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Share {
    /// The user's request, as their device signed it for the machine that holds the entry.
    pub request: Value,
    /// The entry's metadata, as the holder's vault lists it.
    pub entry: Value,
    pub secret: Secret,
}

/// A connection the user shared: their device's signed `connection.share`, which the receiver
/// checks itself (a machine alone shares nothing), the connection's declaration, and the vault
/// entries it names that the user chose to share with it, each with its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionShare {
    /// The user's request, as their device signed it for the machine that holds the connection.
    pub request: Value,
    /// The connection's declaration, as the holder's configuration has it.
    pub connection: Value,
    pub entries: Vec<SharedEntry>,
}

/// A vault entry going with a connection: its metadata, as the holder's vault lists it, and its
/// secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedEntry {
    pub entry: Value,
    pub secret: Secret,
}

/// A secret on its way to another machine's keyring: zeroized when dropped, and never in its
/// `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub Zeroizing<String>);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([masked])")
    }
}

impl Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Secret, D::Error> {
        Ok(Secret(Zeroizing::new(String::deserialize(deserializer)?)))
    }
}

/// The answer to one message.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Answered {
    Ok(Value),
    Refused(String),
}

/// Why a link could not be made, or a message went unanswered: each a sentence for the user.
#[derive(Debug)]
pub enum LinkError {
    /// The endpoint cannot be made with the machine's key.
    Endpoint(String),
    /// No session to the hub to go through, which every link takes.
    NoHub(String),
    /// The other device has no session open to the hub, which links go through.
    Offline(String),
    Unreachable {
        peer: String,
        why: String,
    },
    /// The other side refused the link: it does not trust this key, say.
    Refused {
        peer: String,
        reason: String,
    },
    NeedsUpdate(NeedsUpdate),
    /// The other side refused one message; the link goes on.
    Declined {
        peer: String,
        reason: String,
    },
    Closed(String),
    /// The link was gone before the message was written whole: the other side never read it.
    /// `reason` is why the link was closed, by either side, when one was said.
    NotSent {
        said: String,
        reason: Option<String>,
    },
    /// The other side let the link go for another to this machine (`Link::retire`), and turned
    /// the message away unserved.
    TurnedAway(String),
}

impl LinkError {
    /// The other machine's state, as this error leaves it.
    pub fn state(&self) -> PeerState {
        match self {
            LinkError::NeedsUpdate(_) => PeerState::NeedsUpdate,
            _ => PeerState::Offline,
        }
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::Endpoint(why) => f.write_str(why),
            LinkError::NoHub(peer) => {
                write!(
                    f,
                    "{peer} cannot be reached: this device has no session to the hub"
                )
            }
            LinkError::Offline(peer) => {
                write!(f, "{peer} is offline: it is not connected to the hub")
            }
            LinkError::Unreachable { peer, why } => write!(f, "{peer} is unreachable: {why}"),
            LinkError::Refused { peer, reason } => write!(f, "{peer} refused the link: {reason}"),
            LinkError::NeedsUpdate(refusal) => refusal.fmt(f),
            LinkError::Declined { peer, reason } => write!(f, "{peer} refused: {reason}"),
            LinkError::Closed(why) => f.write_str(why),
            LinkError::NotSent { said, .. } => f.write_str(said),
            LinkError::TurnedAway(peer) => {
                write!(
                    f,
                    "{peer} let this link go for another, and did not serve the message"
                )
            }
        }
    }
}

impl std::error::Error for LinkError {}

/// A machine's endpoint: it dials links and takes them in, through the hub only. It opens no
/// socket of its own.
#[derive(Clone)]
pub struct Endpoint {
    /// The machine's key, which each transport through the hub is made with.
    transport: Arc<Transport>,
    key: PublicKey,
    /// The links that came in, their keys proved, in the order their TLS finished.
    accepted: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Accepting>>>,
    /// This machine's name, as a failure to reach another says where it was tried from.
    name: Arc<std::sync::RwLock<String>>,
    /// What hands the links that came in to `accept`, for each transport through the hub; let go
    /// when the endpoint closes.
    taking: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Sender<Accepting>>>>,
    /// The transport through the hub, while a session to it is open.
    relayed: Arc<std::sync::Mutex<Option<Relayed>>>,
}

/// The links of one session to the hub: its transport, and the members it says are online.
#[derive(Clone)]
struct Relayed {
    transport: Arc<Transport>,
    online: tokio::sync::watch::Receiver<Vec<PublicKey>>,
}

impl Endpoint {
    /// The machine's endpoint, with its key, for both kinds of traffic: it links once a session
    /// to the hub carries it (`Endpoint::through`). Within a Tokio runtime.
    pub fn new(key: &MachineKey) -> Result<Endpoint, LinkError> {
        let seed = Zeroizing::new(key.0.to_bytes());
        let public = key.public();
        let transport = Transport::keyed(&seed, public.as_bytes()).map_err(LinkError::Endpoint)?;
        let (sender, accepted) = tokio::sync::mpsc::channel(HANDSHAKES);
        Ok(Endpoint {
            transport: Arc::new(transport),
            key: public,
            accepted: Arc::new(tokio::sync::Mutex::new(accepted)),
            name: Arc::new(std::sync::RwLock::new(public.fingerprint())),
            taking: Arc::new(std::sync::Mutex::new(Some(sender))),
            relayed: Arc::default(),
        })
    }

    /// Carries links through the hub, over the session whose relay port is `port`
    /// (`Session::relay`), in place of any before: dialled by key, taken in as any other, until
    /// the session ends. Then every link through it ends, said so to this machine's side
    /// (`the hub is unreachable`), and a dial through the hub waits for the next session.
    pub fn through(&self, port: crate::RelayPort) {
        let (out, incoming, mut online) = port.split();
        let socket = Arc::new(RelaySocket::new(out, incoming, online.clone()));
        let alpns = [Traffic::Machine.alpn(), Traffic::Client.alpn()];
        let transport = match self.transport.relayed(socket, &alpns) {
            Ok(transport) => Arc::new(transport),
            Err(why) => {
                eprintln!("flottille: {why}");
                return;
            }
        };
        let taking = self.taking.lock().unwrap().clone();
        if let Some(taking) = taking {
            tokio::spawn(take_in(transport.clone(), taking));
        }
        let before = self.relayed.lock().unwrap().replace(Relayed {
            transport: transport.clone(),
            online: online.clone(),
        });
        if let Some(before) = before {
            tokio::spawn(async move {
                before
                    .transport
                    .close_saying(CLOSE_HUB_GONE, "the session to the hub was opened again")
                    .await;
            });
        }
        let relayed = self.relayed.clone();
        tokio::spawn(async move {
            // The members online change until the session ends.
            while online.changed().await.is_ok() {}
            {
                let mut current = relayed.lock().unwrap();
                if current
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(&current.transport, &transport))
                {
                    *current = None;
                }
            }
            transport
                .close_saying(CLOSE_HUB_GONE, "the hub is unreachable")
                .await;
        });
    }

    /// Whether the links go through the hub now: a session to it is open.
    pub fn through_hub(&self) -> bool {
        self.relayed.lock().unwrap().is_some()
    }

    /// Until the hub says another list of the members online; never without a session to it.
    pub async fn online_changed(&self) {
        let online = self
            .relayed
            .lock()
            .unwrap()
            .as_ref()
            .map(|relayed| relayed.online.clone());
        let Some(mut online) = online else {
            return std::future::pending().await;
        };
        online.borrow_and_update();
        if online.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// The members' keys the hub says are online now; none without a session to it.
    pub fn online(&self) -> Vec<PublicKey> {
        self.relayed
            .lock()
            .unwrap()
            .as_ref()
            .map(|relayed| relayed.online.borrow().clone())
            .unwrap_or_default()
    }

    /// The name this machine has among the user's: a failure to reach another says it
    /// (`tried through the hub from pc`). Its key's fingerprint until it is named.
    pub fn name_as(&self, name: &str) {
        name.clone_into(&mut self.name.write().unwrap());
    }

    pub fn key(&self) -> PublicKey {
        self.key
    }

    /// A link to the machine whose key is `peer`, named `name`, through the hub; its protocol
    /// chosen from both hellos.
    pub async fn connect(
        &self,
        peer: PublicKey,
        traffic: Traffic,
        ours: &Hello,
        name: &str,
    ) -> Result<Link, LinkError> {
        let from = self.name.read().unwrap().clone();
        let Some(relayed) = self.relayed.lock().unwrap().clone() else {
            return Err(LinkError::NoHub(name.to_owned()));
        };
        if !relayed.online.borrow().contains(&peer) {
            return Err(LinkError::Offline(name.to_owned()));
        }
        let Some(address) = relayed.transport.address_for(peer.as_bytes()) else {
            return Err(LinkError::Unreachable {
                peer: name.to_owned(),
                why: format!("tried through the hub from {from}: too many devices"),
            });
        };
        let dialled = tokio::time::timeout(
            WAIT,
            relayed
                .transport
                .connect(peer.as_bytes(), address, traffic.alpn()),
        )
        .await;
        let why = match dialled {
            Ok(Ok(conn)) => return dialler_handshake(conn, peer, traffic, ours, name).await,
            Ok(Err(Failure::Closed(reason) | Failure::Lost(reason))) => {
                format!("tried through the hub from {from}: {reason}")
            }
            Err(_) => format!(
                "tried through the hub from {from}, no answer in {} s",
                WAIT.as_secs()
            ),
        };
        Err(LinkError::Unreachable {
            peer: name.to_owned(),
            why,
        })
    }

    /// The next machine that dials this one, its key proved; `None` once the endpoint is closed.
    pub async fn accept(&self) -> Option<Accepting> {
        self.accepted.lock().await.recv().await
    }

    /// Closes every link, telling the other sides.
    pub async fn close(&self) {
        self.taking.lock().unwrap().take();
        let relayed = self.relayed.lock().unwrap().take();
        if let Some(relayed) = relayed {
            relayed.transport.close().await;
        }
        self.transport.close().await;
    }
}

/// Takes the links that come in, each TLS in a task of its own, so that one that stalls stops
/// no other; until the endpoint is closed, or every copy of it dropped. One that finds no place
/// free, for all or for its host, is refused at once: the links that come in are still taken,
/// and the one refused dials again.
async fn take_in(transport: Arc<Transport>, accepted: tokio::sync::mpsc::Sender<Accepting>) {
    let places = Pool::new(HANDSHAKES);
    loop {
        let incoming = tokio::select! {
            incoming = transport.accept() => incoming,
            _ = accepted.closed() => None,
        };
        let Some(incoming) = incoming else {
            return;
        };
        let Some(place) = places.take(incoming.from().ip()) else {
            incoming.refuse();
            continue;
        };
        let accepted = accepted.clone();
        tokio::spawn(async move {
            let _place = place;
            let Ok(Ok(conn)) = tokio::time::timeout(HANDSHAKE, incoming.finish()).await else {
                return;
            };
            let peer = conn
                .peer()
                .and_then(|bytes| PublicKey::from_bytes(&bytes).ok());
            let traffic = conn.alpn().and_then(|alpn| Traffic::of(&alpn));
            match (peer, traffic) {
                (Some(peer), Some(traffic)) => {
                    let _ = accepted
                        .send(Accepting {
                            conn,
                            peer,
                            traffic,
                        })
                        .await;
                }
                _ => conn.close(
                    CLOSE_REFUSED,
                    "refused: not a key or a traffic of Flottille's",
                ),
            }
        });
    }
}

/// The places of the handshakes under way, and those each host and each IPv6 site holds.
struct Pool {
    free: Arc<tokio::sync::Semaphore>,
    held: Held,
}

/// The places each host or site holds, by its address and its prefix's length: a /64 and the
/// /48 it starts are one address.
type Held = Arc<std::sync::Mutex<HashMap<(IpAddr, u8), usize>>>;

impl Pool {
    fn new(places: usize) -> Pool {
        Pool {
            free: Arc::new(tokio::sync::Semaphore::new(places)),
            held: Held::default(),
        }
    }

    /// A place for `ip`, unless its host or its site holds all of its own, or none is free.
    fn take(&self, ip: IpAddr) -> Option<Place> {
        let mut counted = vec![((host_of(ip), 64), HANDSHAKES_FROM_ONE)];
        counted.extend(site_of(ip).map(|site| ((site, 48), HANDSHAKES_FROM_SITE)));
        let mut held = self.held.lock().unwrap();
        if counted
            .iter()
            .any(|(at, most)| held.get(at).is_some_and(|count| count >= most))
        {
            return None;
        }
        let free = self.free.clone().try_acquire_owned().ok()?;
        for (at, _) in &counted {
            *held.entry(*at).or_default() += 1;
        }
        Some(Place {
            held: self.held.clone(),
            counted: counted.into_iter().map(|(at, _)| at).collect(),
            _free: free,
        })
    }
}

/// A place among the handshakes under way, let go when dropped.
struct Place {
    held: Held,
    /// The host, and the site, it is counted for.
    counted: Vec<(IpAddr, u8)>,
    _free: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut held = self.held.lock().unwrap();
        for at in &self.counted {
            if let Some(count) = held.get_mut(at) {
                *count -= 1;
                if *count == 0 {
                    held.remove(at);
                }
            }
        }
    }
}

/// The host an address is: an IPv4 address, or the /64 of an IPv6 one, which one host is given
/// whole.
fn host_of(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & !u128::from(u64::MAX)).into()),
        v4 => v4,
    }
}

/// The site of an IPv6 address, its /48; none for an IPv4 one.
fn site_of(ip: IpAddr) -> Option<IpAddr> {
    match ip {
        IpAddr::V6(v6) => Some(IpAddr::V6((u128::from(v6) & !(u128::MAX >> 48)).into())),
        IpAddr::V4(_) => None,
    }
}

async fn dialler_handshake(
    conn: Conn,
    peer: PublicKey,
    traffic: Traffic,
    ours: &Hello,
    name: &str,
) -> Result<Link, LinkError> {
    let sent = serde_json::to_vec(ours).expect("a hello is JSON");
    let answered = tokio::time::timeout(WAIT, async {
        conn.request(&sent, FRAME_LIMIT)
            .await
            .map_err(Unanswered::failure)
    })
    .await;
    let theirs = match answered {
        Err(_) => {
            conn.close(CLOSE_DONE, "no hello");
            return Err(LinkError::Unreachable {
                peer: name.to_owned(),
                why: "it said no hello".to_owned(),
            });
        }
        Ok(Err(Failure::Closed(reason))) => {
            return Err(LinkError::Refused {
                peer: name.to_owned(),
                reason,
            });
        }
        Ok(Err(Failure::Lost(why))) => {
            return Err(LinkError::Unreachable {
                peer: name.to_owned(),
                why,
            });
        }
        Ok(Ok(bytes)) => serde_json::from_slice::<Hello>(&bytes).map_err(|_| {
            conn.close(CLOSE_DONE, "not a hello");
            LinkError::Closed(format!("{name} answered what is not a hello"))
        })?,
    };
    match choose(ours, &theirs, name) {
        Ok(protocol) => Ok(Link {
            conn,
            peer,
            name: name.to_owned(),
            traffic,
            protocol,
            theirs,
            ended: Arc::default(),
        }),
        Err(refusal) => {
            conn.close(CLOSE_NEEDS_UPDATE, "refused: no protocol in common");
            Err(LinkError::NeedsUpdate(refusal))
        }
    }
}

/// A link another machine dialled, its key proved, before the handshake.
pub struct Accepting {
    conn: Conn,
    peer: PublicKey,
    traffic: Traffic,
}

impl Accepting {
    pub fn peer(&self) -> PublicKey {
        self.peer
    }

    pub fn traffic(&self) -> Traffic {
        self.traffic
    }

    /// Refuses the link, telling the other side why in `reason`.
    pub fn refuse(self, reason: &str) {
        self.conn.close(CLOSE_REFUSED, reason);
    }

    /// Answers the other side's hello with `ours`: the link, or the refusal both sides say when
    /// they speak no protocol in common. `name` is the other machine's, for the sentence.
    pub async fn handshake(self, ours: &Hello, name: &str) -> Result<Link, LinkError> {
        let conn = self.conn;
        let Ok(Some((bytes, reply))) = tokio::time::timeout(WAIT, conn.next()).await else {
            conn.close(CLOSE_DONE, "no hello");
            return Err(LinkError::Closed(format!("{name} said no hello")));
        };
        let Ok(theirs) = serde_json::from_slice::<Hello>(&bytes) else {
            conn.close(CLOSE_DONE, "not a hello");
            return Err(LinkError::Closed(format!(
                "{name} said what is not a hello"
            )));
        };
        reply
            .send(&serde_json::to_vec(ours).expect("a hello is JSON"))
            .await;
        match choose(ours, &theirs, name) {
            Ok(protocol) => Ok(Link {
                conn,
                peer: self.peer,
                name: name.to_owned(),
                traffic: self.traffic,
                protocol,
                theirs,
                ended: Arc::default(),
            }),
            Err(refusal) => {
                // The other side reads this hello, refuses too, and closes.
                let _ = tokio::time::timeout(WAIT, conn.closed()).await;
                Err(LinkError::NeedsUpdate(refusal))
            }
        }
    }
}

/// A link made: two machines that proved their keys, and a protocol both speak.
#[derive(Clone)]
pub struct Link {
    pub(crate) conn: Conn,
    peer: PublicKey,
    name: String,
    traffic: Traffic,
    protocol: u32,
    theirs: Hello,
    /// How this side ends it, shared by its clones.
    ended: Arc<Ended>,
}

/// How this side ends a link: why, and what is under way on it meanwhile.
#[derive(Default)]
struct Ended {
    /// Why this side closed the link, or lets it go: a close said after the letting go is the
    /// last word.
    why: std::sync::Mutex<Option<String>>,
    /// Let go (`Link::retire`): it sends nothing more, and turns away what comes.
    retiring: AtomicBool,
    /// The messages under way on it, sent and served.
    busy: AtomicUsize,
    idle: tokio::sync::Notify,
}

/// A message under way on a link, while it is held.
struct Busy(Arc<Ended>);

impl Busy {
    fn on(ended: &Arc<Ended>) -> Busy {
        ended.busy.fetch_add(1, Ordering::SeqCst);
        Busy(ended.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if self.0.busy.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

impl Link {
    pub fn peer(&self) -> PublicKey {
        self.peer
    }

    pub fn traffic(&self) -> Traffic {
        self.traffic
    }

    pub fn protocol(&self) -> u32 {
        self.protocol
    }

    /// What the other side said in its hello.
    pub fn theirs(&self) -> &Hello {
        &self.theirs
    }

    /// Sends `message`, and waits for its answer.
    pub async fn send(&self, message: &Message) -> Result<Value, LinkError> {
        self.send_raw(&framed(message)).await
    }

    /// Sends a frame as it is. What it holds is the receiver's to judge: a peer is never trusted
    /// to have checked what it sends.
    #[doc(hidden)]
    pub async fn send_raw(&self, frame: &[u8]) -> Result<Value, LinkError> {
        let _busy = Busy::on(&self.ended);
        if let Some(why) = self.retiring() {
            return Err(LinkError::NotSent {
                said: format!("the link to {} was let go: {why}", self.name),
                reason: Some(why),
            });
        }
        let bytes = self
            .conn
            .request(frame, FRAME_LIMIT)
            .await
            .map_err(|unanswered| match unanswered {
                Unanswered::Unsent(failure) => {
                    let reason = match &failure {
                        Failure::Closed(reason) => Some(reason.clone()),
                        Failure::Lost(_) => self.ended.why.lock().unwrap().clone(),
                    };
                    LinkError::NotSent {
                        said: self.lost(failure).to_string(),
                        reason,
                    }
                }
                Unanswered::Sent(failure) => self.lost(failure),
                Unanswered::TurnedAway => LinkError::TurnedAway(self.name.clone()),
            })?;
        match serde_json::from_slice::<Answered>(&bytes) {
            Ok(Answered::Ok(value)) => Ok(value),
            Ok(Answered::Refused(reason)) => Err(LinkError::Declined {
                peer: self.name.clone(),
                reason,
            }),
            Err(_) => Err(LinkError::Closed(format!(
                "{} answered what is not an answer",
                self.name
            ))),
        }
    }

    /// Sends a client's request, and gives what comes back on its stream: its answer, then, for a
    /// subscription, its events, until either side ends it (`docs/remote.md`, "Requests from
    /// another machine's clients").
    pub async fn ask(&self, message: &Message) -> Result<Answers, LinkError> {
        let frame = serde_json::to_vec(message).expect("a message is JSON");
        let frames = self
            .conn
            .open(&frame)
            .await
            .map_err(|failure| self.lost(failure))?;
        Ok(Answers {
            frames,
            name: self.name.clone(),
        })
    }

    /// The next message the other side sends, of this link's kind; `None` once the link is gone.
    /// A message of the other kind, or one this side does not know, is refused and passed over.
    pub async fn next(&self) -> Option<Received> {
        loop {
            match self.conn.next_stream().await? {
                Inbound::Frame(bytes, reply) => {
                    // A frame may carry a vault entry's secret (`share`): wiped once it is read.
                    let bytes = Zeroizing::new(bytes);
                    // Let go: what comes is turned away unserved, which its sender is told.
                    if self.retiring().is_some() {
                        reply.turn_away();
                        continue;
                    }
                    match self.judge(&bytes) {
                        Ok(message) => {
                            return Some(Received {
                                message,
                                reply: Replying::Frame(reply),
                                _busy: Busy::on(&self.ended),
                            });
                        }
                        Err(reason) => reply.send(&answer(Err(reason))).await,
                    }
                }
                // A tunnel's connection (`docs/remote.md`, "Tunnels"), on a client link only.
                Inbound::Raw(raw, head) => match (self.traffic, tunnel_of(&head)) {
                    (Traffic::Client, Some(id)) => {
                        return Some(Received {
                            message: Message::Client(serde_json::json!({"tunnel": id})),
                            reply: Replying::Tunnel(id, raw),
                            _busy: Busy::on(&self.ended),
                        });
                    }
                    _ => raw.refuse(),
                },
            }
        }
    }

    fn judge(&self, bytes: &[u8]) -> Result<Message, String> {
        let Ok(Value::Object(frame)) = serde_json::from_slice::<Value>(bytes) else {
            return Err("not a message of Flottille's link".to_owned());
        };
        let kind = match frame.keys().next().map(String::as_str) {
            Some("machine") if frame.len() == 1 => Traffic::Machine,
            Some("client") if frame.len() == 1 => Traffic::Client,
            _ => return Err("not a message of Flottille's link".to_owned()),
        };
        match (kind, self.traffic) {
            (Traffic::Machine, Traffic::Client) => {
                return Err("a machine's message is not taken on a client link".to_owned());
            }
            (Traffic::Client, Traffic::Machine) => {
                return Err("a client's request is not taken on a machine link".to_owned());
            }
            _ => {}
        }
        serde_json::from_value::<Message>(Value::Object(frame))
            .map_err(|_| UNKNOWN_MESSAGE.to_owned())
    }

    pub(crate) fn lost(&self, failure: Failure) -> LinkError {
        LinkError::Closed(match failure {
            Failure::Closed(reason) => format!("{} closed the link: {reason}", self.name),
            Failure::Lost(why) => format!("the link to {} was lost: {why}", self.name),
        })
    }

    /// Closes the link, telling the other side why.
    pub fn close(&self, reason: &str) {
        *self.ended.why.lock().unwrap() = Some(reason.to_owned());
        self.ended.retiring.store(false, Ordering::SeqCst);
        self.conn.close(CLOSE_DONE, reason);
    }

    /// Lets the link go for another to the same machine: it sends nothing more, and a frame that
    /// comes on it is turned away unserved (`Reply::turn_away`); a request and its one answer
    /// under way either way are finished. It closes with `reason` once none is, `GRACE` at the
    /// soonest and `WAIT` at the latest: an answer given frame by frame (a client's request, a git
    /// fetch) or a tunnel's connection is cut then. Within a Tokio runtime.
    pub fn retire(&self, reason: &str) {
        {
            let mut why = self.ended.why.lock().unwrap();
            if why.is_some() {
                return;
            }
            *why = Some(reason.to_owned());
            self.ended.retiring.store(true, Ordering::SeqCst);
        }
        let (conn, ended, reason) = (self.conn.clone(), self.ended.clone(), reason.to_owned());
        tokio::spawn(async move {
            let settled = async {
                tokio::time::sleep(GRACE).await;
                loop {
                    let idle = ended.idle.notified();
                    tokio::pin!(idle);
                    idle.as_mut().enable();
                    if ended.busy.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                    idle.await;
                }
            };
            tokio::select! {
                _ = tokio::time::timeout(WAIT, settled) => {}
                _ = conn.closed() => {}
            }
            conn.close(CLOSE_DONE, &reason);
        });
    }

    /// Why this link was let go (`retire`), while it is.
    fn retiring(&self) -> Option<String> {
        self.ended
            .retiring
            .load(Ordering::SeqCst)
            .then(|| self.ended.why.lock().unwrap().clone())
            .flatten()
    }

    /// Whether `other` is this very link.
    pub fn same(&self, other: &Link) -> bool {
        Arc::ptr_eq(&self.ended, &other.ended)
    }

    /// Waits until the link is gone; why, in a sentence.
    pub async fn closed(&self) -> String {
        self.lost(self.conn.closed().await).to_string()
    }
}

/// `message` as a frame, wiped when it is dropped: one may carry a vault entry's secret
/// (`share`). Sized first, so that no buffer it outgrew is freed with a copy of it.
fn framed(message: &Message) -> Zeroizing<Vec<u8>> {
    struct Counted(usize);
    impl std::io::Write for Counted {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counted = Counted(0);
    serde_json::to_writer(&mut counted, message).expect("a message is JSON");
    let mut frame = Zeroizing::new(Vec::with_capacity(counted.0));
    serde_json::to_writer(&mut *frame, message).expect("a message is JSON");
    frame
}

impl fmt::Debug for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Link")
            .field("peer", &self.peer)
            .field("name", &self.name)
            .field("traffic", &self.traffic)
            .field("protocol", &self.protocol)
            .finish()
    }
}

/// What comes back for a client's request (`Link::ask`), one frame after another.
pub struct Answers {
    frames: Frames,
    name: String,
}

impl Answers {
    /// The next frame that came back, as JSON; `None` once the other side ended the stream. A
    /// request the other side refused before reading it (one of the other kind) is its refusal.
    pub async fn next(&mut self) -> Option<Result<Value, LinkError>> {
        let frame = match self.frames.next(FRAME_LIMIT).await? {
            Ok(frame) => frame,
            Err(Failure::Closed(reason)) => {
                return Some(Err(LinkError::Closed(format!(
                    "{} closed the link: {reason}",
                    self.name
                ))));
            }
            Err(Failure::Lost(why)) => {
                return Some(Err(LinkError::Closed(format!(
                    "the link to {} was lost: {why}",
                    self.name
                ))));
            }
        };
        Some(match serde_json::from_slice::<Value>(&frame) {
            Ok(Value::Object(object)) if object.len() == 1 && object.contains_key("refused") => {
                Err(LinkError::Declined {
                    peer: self.name.clone(),
                    reason: object["refused"].as_str().unwrap_or_default().to_owned(),
                })
            }
            Ok(value) => Ok(value),
            Err(_) => Err(LinkError::Closed(format!(
                "{} answered what is not an answer",
                self.name
            ))),
        })
    }
}

/// An answer given frame by frame (`Received::stream`).
pub struct Streamed(Reply);

impl Streamed {
    /// Sends one frame, a JSON text with no newline; `false` once the other side stopped reading.
    pub async fn send(&mut self, frame: &[u8]) -> bool {
        self.0.line(frame).await
    }

    /// Ends the answer.
    pub async fn end(self) {
        self.0.end().await
    }

    /// Until the other side stops reading, or the link is gone.
    pub fn gone(&self) -> impl Future<Output = ()> + Send + 'static {
        self.0.gone()
    }
}

/// A message received, to answer.
pub struct Received {
    message: Message,
    reply: Replying,
    /// Held until it is answered: a link let go is not closed under it.
    _busy: Busy,
}

/// Where a message is answered: on the stream it came on, or, for a tunnel's connection, that
/// stream is the connection itself.
enum Replying {
    Frame(Reply),
    Tunnel(String, Raw),
}

/// The id of the tunnel an opening line names: `{"tunnel": "<id>"}`.
fn tunnel_of(head: &[u8]) -> Option<String> {
    match serde_json::from_slice::<Value>(head).ok()? {
        Value::Object(object) if object.len() == 1 => {
            object.get("tunnel")?.as_str().map(str::to_owned)
        }
        _ => None,
    }
}

impl Received {
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// The tunnel connection this is, with the id of the tunnel it names, or the message back
    /// when it is any other.
    pub fn take_tunnel(self) -> Result<(String, crate::TunnelStream), Box<Received>> {
        match self.reply {
            Replying::Tunnel(id, raw) => Ok((id, crate::TunnelStream::new(raw))),
            reply => Err(Box::new(Received {
                message: self.message,
                reply,
                _busy: self._busy,
            })),
        }
    }

    /// Answers it frame by frame: a client's request, whose answer may be followed by events.
    pub fn stream(self) -> Streamed {
        match self.reply {
            Replying::Frame(reply) => Streamed(reply),
            Replying::Tunnel(..) => unreachable!("a tunnel's connection is taken with take_tunnel"),
        }
    }

    /// Answers it: what it gave, or why it was refused. Within a Tokio runtime.
    pub fn answer(self, answered: Result<Value, String>) {
        match self.reply {
            Replying::Frame(reply) => {
                let bytes = answer(answered);
                let busy = self._busy;
                tokio::spawn(async move {
                    reply.send_owned(bytes).await;
                    drop(busy);
                });
            }
            // A connection of a tunnel has no answer to give: it is closed.
            Replying::Tunnel(_, raw) => raw.refuse(),
        }
    }
}

fn answer(answered: Result<Value, String>) -> Vec<u8> {
    let answered = match answered {
        Ok(value) => Answered::Ok(value),
        Err(reason) => Answered::Refused(reason),
    };
    serde_json::to_vec(&answered).expect("an answer is JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn an_ipv6_host_holds_two_places_and_its_site_eight() {
        let places = Pool::new(HANDSHAKES);
        let mut held = Vec::new();
        // Two for one /64, refused a third.
        held.push(places.take(ip("2001:db8:1:1::1")).unwrap());
        held.push(places.take(ip("2001:db8:1:1::2")).unwrap());
        assert!(places.take(ip("2001:db8:1:1::3")).is_none());
        // Eight for the /48, whichever of its /64s they come from.
        for subnet in 2..5 {
            for _ in 0..2 {
                held.push(places.take(ip(&format!("2001:db8:1:{subnet}::1"))).unwrap());
            }
        }
        assert!(places.take(ip("2001:db8:1:99::1")).is_none());
        // Another site is not counted with it.
        held.push(places.take(ip("2001:db8:2::1")).unwrap());
        // A place let go is free again.
        held.remove(0);
        assert!(places.take(ip("2001:db8:1:99::1")).is_some());
    }
}
