//! The link between two machines (`docs/remote.md`, "Architecture", "The transport", "The
//! version"): an endpoint per machine, links authenticated by the machines' keys, two kinds of
//! traffic kept apart, and the version handshake when a link is made. The transport under it is
//! `transport.rs`'s; nothing here names it.
//!
//! On a link, each message goes on a stream of its own: one JSON frame one way, its answer the
//! other way. The first stream is the handshake: the side that dials sends its hello, the other
//! answers its own, and each chooses the protocol, or refuses, by itself.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::transport::{Conn, Failure, Reply, Transport};
use crate::{Hello, MachineKey, NeedsUpdate, PeerState, PublicKey, choose};

/// The longest frame a link takes. Bigger transfers (a git bundle) will have streams of their
/// own.
pub const FRAME_LIMIT: usize = 1 << 20;
/// How long dialling one address, or a handshake, may take.
const WAIT: Duration = Duration::from_secs(10);
/// How long the TLS of a link that came in may take: on loopback or a LAN it takes milliseconds.
const HANDSHAKE: Duration = Duration::from_secs(5);
/// The most links that came in whose TLS is under way at once: one that stalls holds a place,
/// not the others.
const HANDSHAKES: usize = 16;
/// The codes a side closes a link with.
const CLOSE_REFUSED: u32 = 1;
const CLOSE_NEEDS_UPDATE: u32 = 2;
const CLOSE_DONE: u32 = 3;

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

/// What a machine may send another (`docs/remote.md`, "What a machine may send"): nothing else is
/// taken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineMessage {
    /// Every address the sender may be reached at now, so that the two can link again without
    /// the hub.
    Addresses(Vec<String>),
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
    /// The endpoint cannot listen where it is told to.
    Listen(String),
    /// No address to dial.
    NoAddress(String),
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
            LinkError::Listen(why) => f.write_str(why),
            LinkError::NoAddress(peer) => write!(f, "{peer} has no address this machine knows"),
            LinkError::Unreachable { peer, why } => write!(f, "{peer} cannot be reached: {why}"),
            LinkError::Refused { peer, reason } => write!(f, "{peer} refused the link: {reason}"),
            LinkError::NeedsUpdate(refusal) => refusal.fmt(f),
            LinkError::Declined { peer, reason } => write!(f, "{peer} refused: {reason}"),
            LinkError::Closed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for LinkError {}

/// A machine's endpoint: it listens for links, and dials them.
#[derive(Clone)]
pub struct Endpoint {
    transport: Arc<Transport>,
    key: PublicKey,
    /// The links that came in, their keys proved, in the order their TLS finished.
    accepted: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Accepting>>>,
}

impl Endpoint {
    /// Listens at `listen` with the machine's key, for both kinds of traffic. Within a Tokio
    /// runtime.
    pub fn bind(key: &MachineKey, listen: SocketAddr) -> Result<Endpoint, LinkError> {
        let seed = Zeroizing::new(key.0.to_bytes());
        let public = key.public();
        let alpns = [Traffic::Machine.alpn(), Traffic::Client.alpn()];
        let transport =
            Transport::bind(&seed, public.as_bytes(), listen, &alpns).map_err(LinkError::Listen)?;
        Ok(Endpoint::over(transport, public))
    }

    /// Listens nowhere, and dials: no socket is open until the first link it dials. Within a
    /// Tokio runtime.
    pub fn dialler(key: &MachineKey) -> Result<Endpoint, LinkError> {
        let seed = Zeroizing::new(key.0.to_bytes());
        let public = key.public();
        let transport = Transport::dialler(&seed, public.as_bytes()).map_err(LinkError::Listen)?;
        Ok(Endpoint::over(transport, public))
    }

    fn over(transport: Transport, key: PublicKey) -> Endpoint {
        let transport = Arc::new(transport);
        let (sender, accepted) = tokio::sync::mpsc::channel(HANDSHAKES);
        tokio::spawn(take_in(transport.clone(), sender));
        Endpoint {
            transport,
            key,
            accepted: Arc::new(tokio::sync::Mutex::new(accepted)),
        }
    }

    pub fn key(&self) -> PublicKey {
        self.key
    }

    /// The socket it listens on.
    pub fn listening(&self) -> Option<SocketAddr> {
        self.transport.local()
    }

    /// The addresses it may be reached at, as far as this machine knows without asking anyone:
    /// the address it listens on, or, listening on every interface, the interfaces' addresses.
    pub fn addresses(&self) -> Vec<SocketAddr> {
        let Some(local) = self.listening() else {
            return Vec::new();
        };
        if !local.ip().is_unspecified() {
            return vec![local];
        }
        interfaces()
            .into_iter()
            .filter(|ip| local.is_ipv6() || ip.is_ipv4())
            .map(|ip| SocketAddr::new(ip, local.port()))
            .collect()
    }

    /// A link to the machine whose key is `peer`, named `name`, at the first of `addresses` that
    /// answers; its protocol chosen from both hellos.
    pub async fn connect(
        &self,
        peer: PublicKey,
        addresses: &[SocketAddr],
        traffic: Traffic,
        ours: &Hello,
        name: &str,
    ) -> Result<Link, LinkError> {
        let mut why = None;
        for address in addresses {
            let dialled = tokio::time::timeout(
                WAIT,
                self.transport
                    .connect(peer.as_bytes(), *address, traffic.alpn()),
            )
            .await;
            match dialled {
                Ok(Ok(conn)) => return dialler_handshake(conn, peer, traffic, ours, name).await,
                Ok(Err(Failure::Closed(reason) | Failure::Lost(reason))) => {
                    why = Some(format!("{address}: {reason}"));
                }
                Err(_) => why = Some(format!("{address} did not answer")),
            }
        }
        match why {
            None => Err(LinkError::NoAddress(name.to_owned())),
            Some(why) => Err(LinkError::Unreachable {
                peer: name.to_owned(),
                why,
            }),
        }
    }

    /// The next machine that dials this one, its key proved; `None` once the endpoint is closed,
    /// at once when it listens nowhere.
    pub async fn accept(&self) -> Option<Accepting> {
        self.accepted.lock().await.recv().await
    }

    /// Closes every link, telling the other sides.
    pub async fn close(&self) {
        self.transport.close().await;
    }
}

/// Takes the links that come in, each TLS in a task of its own, so that one that stalls stops
/// no other; until the endpoint is closed, or every copy of it dropped.
async fn take_in(transport: Arc<Transport>, accepted: tokio::sync::mpsc::Sender<Accepting>) {
    let places = Arc::new(tokio::sync::Semaphore::new(HANDSHAKES));
    loop {
        let incoming = tokio::select! {
            incoming = transport.accept() => incoming,
            _ = accepted.closed() => None,
        };
        let Some(incoming) = incoming else {
            return;
        };
        let Ok(place) = places.clone().acquire_owned().await else {
            return;
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

async fn dialler_handshake(
    conn: Conn,
    peer: PublicKey,
    traffic: Traffic,
    ours: &Hello,
    name: &str,
) -> Result<Link, LinkError> {
    let sent = serde_json::to_vec(ours).expect("a hello is JSON");
    let answered = tokio::time::timeout(WAIT, conn.request(&sent, FRAME_LIMIT)).await;
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
        let Ok(Some((bytes, reply))) = tokio::time::timeout(WAIT, conn.next(FRAME_LIMIT)).await
        else {
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
    conn: Conn,
    peer: PublicKey,
    name: String,
    traffic: Traffic,
    protocol: u32,
    theirs: Hello,
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
        self.send_raw(&serde_json::to_vec(message).expect("a message is JSON"))
            .await
    }

    /// Sends a frame as it is. What it holds is the receiver's to judge: a peer is never trusted
    /// to have checked what it sends.
    #[doc(hidden)]
    pub async fn send_raw(&self, frame: &[u8]) -> Result<Value, LinkError> {
        let bytes = self
            .conn
            .request(frame, FRAME_LIMIT)
            .await
            .map_err(|failure| self.lost(failure))?;
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

    /// The next message the other side sends, of this link's kind; `None` once the link is gone.
    /// A message of the other kind, or one this side does not know, is refused and passed over.
    pub async fn next(&self) -> Option<Received> {
        loop {
            let (bytes, reply) = self.conn.next(FRAME_LIMIT).await?;
            match self.judge(&bytes) {
                Ok(message) => return Some(Received { message, reply }),
                Err(reason) => reply.send(&answer(Err(reason))).await,
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
            .map_err(|_| "not a message a machine may send".to_owned())
    }

    fn lost(&self, failure: Failure) -> LinkError {
        LinkError::Closed(match failure {
            Failure::Closed(reason) => format!("{} closed the link: {reason}", self.name),
            Failure::Lost(why) => format!("the link to {} was lost: {why}", self.name),
        })
    }

    /// Closes the link, telling the other side why.
    pub fn close(&self, reason: &str) {
        self.conn.close(CLOSE_DONE, reason);
    }

    /// Waits until the link is gone; why, in a sentence.
    pub async fn closed(&self) -> String {
        self.lost(self.conn.closed().await).to_string()
    }
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

/// A message received, to answer.
pub struct Received {
    message: Message,
    reply: Reply,
}

impl Received {
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// Answers it: what it gave, or why it was refused. Within a Tokio runtime.
    pub fn answer(self, answered: Result<Value, String>) {
        let bytes = answer(answered);
        tokio::spawn(self.reply.send_owned(bytes));
    }
}

fn answer(answered: Result<Value, String>) -> Vec<u8> {
    let answered = match answered {
        Ok(value) => Answered::Ok(value),
        Err(reason) => Answered::Refused(reason),
    };
    serde_json::to_vec(&answered).expect("an answer is JSON")
}

/// The addresses of this machine's interfaces that another may reach: not a loopback, nor an
/// IPv6 link-local one (it needs an interface named with it).
fn interfaces() -> Vec<IpAddr> {
    let mut found = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list` on success; it is freed below, once.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return found;
    }
    let mut at = list;
    while !at.is_null() {
        // SAFETY: `at` is an entry of the list getifaddrs made, not freed yet.
        let entry = unsafe { &*at };
        at = entry.ifa_next;
        if entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: `ifa_addr` is a socket address whose family says its layout.
        let ip = unsafe {
            match i32::from((*entry.ifa_addr).sa_family) {
                libc::AF_INET => {
                    let address = &*(entry.ifa_addr as *const libc::sockaddr_in);
                    IpAddr::from(u32::from_be(address.sin_addr.s_addr).to_be_bytes())
                }
                libc::AF_INET6 => {
                    let address = &*(entry.ifa_addr as *const libc::sockaddr_in6);
                    IpAddr::from(address.sin6_addr.s6_addr)
                }
                _ => continue,
            }
        };
        let link_local = matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80);
        if !ip.is_loopback() && !link_local && !found.contains(&ip) {
            found.push(ip);
        }
    }
    // SAFETY: `list` is what getifaddrs returned, freed once.
    unsafe { libc::freeifaddrs(list) };
    found
}
