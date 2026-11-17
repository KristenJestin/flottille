//! The hub on the network (`docs/remote.md`, "The hub", "Architecture"): it listens on one address,
//! plain (behind a reverse proxy that holds the certificate) or with TLS on a certificate given,
//! and keeps a control session with each device that proves its key (`flottille_link::arrive`).
//!
//! A session sends the device the member list, the addresses the members' machines told, and, to
//! a runner, the presence of the members' clients; then sends them again whenever they change. The
//! device's asks (the first member's setup, a member's change, its addresses, its presence) go
//! through the hub's rules (`Hub`), each checked again against the members as they are when it
//! comes in. The hub carries no link's bytes: a session holds no message and no queue, only the
//! list, the addresses and the presence.
//!
//! What someone with no key may hold of the hub is bounded: at most `MOST_ARRIVING` connections
//! that have not proved a member's key at once (a setup's session among them), then
//! `KEPT_FOR_MEMBERS` more that are let go unless they prove one, a few KiB a handshake line,
//! `ARRIVAL_WAIT` for the TLS handshake and the hub's, and `SETUP_IDLE` from its start a session
//! opened to set the hub up. When the process runs out of file descriptors, it waits before it
//! accepts again.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_link::{
    Action, Address, Ask, Change, Hello, HubSession, MemberList, PresenceOf, Role, SetupCode,
    SetupProof, ToDevice, Told, Transcript, arrive,
};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use crate::{Clock, Hub, HubError, Presence, not_the_code};

/// Where the hub listens unless told otherwise: loopback, behind a reverse proxy on the same
/// host. Another address takes TLS, or `--plain` for a proxy elsewhere.
pub const LISTEN: &str = "127.0.0.1:7448";

/// The most connections that have not proved a member's key the hub holds at once: one more is
/// let go as soon as it is accepted.
pub const MOST_ARRIVING: usize = 64;
/// The places kept, past `MOST_ARRIVING`, for connections that prove a member's key: one that
/// does not is let go at the end of its handshake.
pub const KEPT_FOR_MEMBERS: usize = 8;
/// How long a session opened to set the hub up lasts, however much it asks, before it is closed.
pub const SETUP_IDLE: Duration = Duration::from_secs(10);
/// How long the hub waits after a failed accept (no file descriptor left, mostly), at first and
/// at most: twice longer each time it fails again.
const ACCEPT_WAIT: Duration = Duration::from_millis(50);
const LONGEST_ACCEPT_WAIT: Duration = Duration::from_secs(1);

/// How long a connection that came in may take for its TLS handshake and the hub's together: a
/// device needs a few round trips, a stranger that says nothing holds its place no longer.
const ARRIVAL_WAIT: Duration = Duration::from_secs(5);
/// The most addresses one machine may tell, and the longest one.
const MOST_ADDRESSES: usize = 64;
const LONGEST_ADDRESS: usize = 300;

/// What the sessions share: the hub, and a number raised at each change they must send.
struct Shared<C: Clock> {
    hub: Mutex<Hub<C>>,
    key: Arc<flottille_link::HubKey>,
    changed: watch::Sender<u64>,
    hello: Hello,
}

/// A hub serving on the network, until `stop`.
pub struct Served<C: Clock + Send + 'static> {
    address: SocketAddr,
    shared: Arc<Shared<C>>,
    stop: watch::Sender<bool>,
    accepting: tokio::task::JoinHandle<()>,
}

/// Serves `hub` on `listener`, with TLS when `tls` is given. Within a Tokio runtime.
pub fn serve<C: Clock + Send + 'static>(
    hub: Hub<C>,
    listener: TcpListener,
    tls: Option<flottille_link::TlsServer>,
    hello: Hello,
) -> std::io::Result<Served<C>> {
    let address = listener.local_addr()?;
    let shared = Arc::new(Shared {
        key: hub.key.clone(),
        hub: Mutex::new(hub),
        changed: watch::channel(0).0,
        hello,
    });
    let (stop, stopping) = watch::channel(false);
    let accepting = tokio::spawn(accept(listener, tls, shared.clone(), stopping));
    Ok(Served {
        address,
        shared,
        stop,
        accepting,
    })
}

impl<C: Clock + Send + 'static> Served<C> {
    /// The address it listens on.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Does `act` with the hub, then sends every session what changed. For the hub's own uses
    /// that come in another way than a session, and for the tests.
    pub fn with_hub<R>(&self, act: impl FnOnce(&mut Hub<C>) -> R) -> R {
        let result = act(&mut self.shared.hub.lock().unwrap());
        self.shared.changed.send_modify(|at| *at += 1);
        result
    }

    /// Stops serving: every session is closed, and the hub let go, its state folder with it.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.accepting.await;
    }
}

/// The certificate and private key in PEM files, for TLS. The key's bytes are never said.
pub fn tls_server(certificate: &Path, key: &Path) -> Result<flottille_link::TlsServer, HubError> {
    flottille_link::TlsServer::from_pem(certificate, key).map_err(HubError)
}

async fn accept<C: Clock + Send + 'static>(
    listener: TcpListener,
    tls: Option<flottille_link::TlsServer>,
    shared: Arc<Shared<C>>,
    mut stopping: watch::Receiver<bool>,
) {
    let mut sessions = JoinSet::new();
    let places = Arc::new(Semaphore::new(MOST_ARRIVING));
    let kept = Arc::new(Semaphore::new(KEPT_FOR_MEMBERS));
    let mut wait = ACCEPT_WAIT;
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = stopped(&mut stopping) => break,
        };
        // Finished sessions are let go as others come.
        while sessions.try_join_next().is_some() {}
        let (stream, from) = match accepted {
            Ok(accepted) => {
                wait = ACCEPT_WAIT;
                accepted
            }
            Err(error) => {
                eprintln!(
                    "flottille-hub: a connection could not be accepted: {error}; trying again in \
                     {} ms",
                    wait.as_millis()
                );
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = stopped(&mut stopping) => break,
                }
                wait = (wait * 2).min(LONGEST_ACCEPT_WAIT);
                continue;
            }
        };
        let (permit, members_only) = match places.clone().try_acquire_owned() {
            Ok(permit) => (permit, false),
            Err(_) => match kept.clone().try_acquire_owned() {
                Ok(permit) => (permit, true),
                // Dropped: the connection is closed at once.
                Err(_) => continue,
            },
        };
        let (shared, tls) = (shared.clone(), tls.clone());
        let stopping = stopping.clone();
        sessions.spawn(async move {
            let arrival = async {
                let stream: Box<dyn flottille_link::Stream> = match tls {
                    Some(tls) => tls
                        .accept(stream)
                        .await
                        .map_err(|error| error.to_string())?,
                    None => Box::new(stream),
                };
                arrive(stream, &shared.key, &shared.hello).await
            };
            let arriving = match tokio::time::timeout(ARRIVAL_WAIT, arrival).await {
                Ok(Ok(arriving)) => arriving,
                Ok(Err(why)) => {
                    eprintln!("flottille-hub: a connection from {from} was not taken: {why}");
                    return;
                }
                Err(_) => return,
            };
            let device = arriving.device();
            let (welcome, setting_up) = {
                let hub = shared.hub.lock().unwrap();
                (
                    hub.welcome(&device, arriving.pinned()),
                    hub.members().is_none(),
                )
            };
            if let Err(refusal) = welcome {
                let (reason, reset) = *refusal;
                eprintln!(
                    "flottille-hub: refused {} from {from}: {reason}",
                    device.fingerprint()
                );
                arriving.refuse(&reason, reset).await;
                return;
            }
            // `welcome` takes anyone while no member exists: a setup is no member's.
            if members_only && setting_up {
                arriving
                    .refuse("the hub is busy: try again in a moment", false)
                    .await;
                return;
            }
            // A member's session is not counted; a setup's is, while it lasts.
            let _permit = setting_up.then_some(permit);
            let Ok(session) = arriving.welcome().await else {
                return;
            };
            eprintln!(
                "flottille-hub: {} connected from {from}",
                device.fingerprint()
            );
            let why = run(session, &shared, stopping).await;
            eprintln!("flottille-hub: {} left: {why}", device.fingerprint());
        });
    }
    sessions.shutdown().await;
}

/// What a session last sent, so that only what changed is sent again.
#[derive(Default)]
struct Sent {
    list: Option<MemberList>,
    addresses: Option<Vec<Told>>,
    presence: Option<Vec<PresenceOf>>,
}

/// One device's session, until it ends; why it ended.
async fn run<C: Clock + Send + 'static>(
    mut session: HubSession,
    shared: &Shared<C>,
    mut stopping: watch::Receiver<bool>,
) -> String {
    let device = session.device();
    let pinned = session.pinned();
    let mut changed = shared.changed.subscribe();
    let mut sent = Sent::default();
    // Set once: a session that keeps asking is closed as one that asks nothing.
    let setup_ends = tokio::time::Instant::now() + SETUP_IDLE;
    loop {
        let frames = {
            let hub = shared.hub.lock().unwrap();
            hub.welcome(&device, pinned)
                .map(|()| (news(&hub, &device, &mut sent), hub.members().is_none()))
        };
        let (frames, setting_up) = match frames {
            Ok(frames) => frames,
            Err(refusal) => {
                let (reason, reset) = *refusal;
                let _ = session.send(&ToDevice::Refused { reason, reset }).await;
                session.close().await;
                return "it is not a member any more".to_owned();
            }
        };
        for frame in frames {
            if let Err(error) = session.send(&frame).await {
                return error.to_string();
            }
        }
        let idle = async {
            if setting_up {
                tokio::time::sleep_until(setup_ends).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            asked = session.ask() => {
                let asked = match asked {
                    Ok(asked) => asked,
                    Err(why) => return why,
                };
                let transcript = *session.transcript();
                let (refused, proof) = match handle(shared, &transcript, pinned, asked.ask) {
                    Ok(proof) => (None, proof),
                    Err(error) => (Some(error.to_string()), None),
                };
                let answer = ToDevice::Answer { id: asked.id, refused, proof };
                if session.send(&answer).await.is_err() {
                    return "it closed the connection".to_owned();
                }
            }
            _ = changed.changed() => {}
            _ = idle => {
                let reason = format!(
                    "no setup came within {} s of this session",
                    SETUP_IDLE.as_secs()
                );
                let _ = session.send(&ToDevice::Refused { reason: reason.clone(), reset: false }).await;
                session.close().await;
                return reason;
            }
            _ = stopped(&mut stopping) => {
                session.close().await;
                return "the hub stops".to_owned();
            }
        }
    }
}

/// What `device` was not sent yet of the hub's state.
fn news<C: Clock>(
    hub: &Hub<C>,
    device: &flottille_link::PublicKey,
    sent: &mut Sent,
) -> Vec<ToDevice> {
    let mut frames = Vec::new();
    let list = hub.list().clone();
    if sent.list.as_ref() != Some(&list) {
        frames.push(ToDevice::List(list.clone()));
        sent.list = Some(list);
    }
    let Some(members) = hub.members() else {
        return frames;
    };
    let addresses: Vec<Told> = members
        .members()
        .iter()
        .filter_map(|member| member.machine)
        .filter_map(|machine| {
            Some(Told {
                key: machine,
                addresses: hub.addresses(&machine)?.to_vec(),
            })
        })
        .collect();
    if sent.addresses.as_ref() != Some(&addresses) {
        frames.push(ToDevice::Addresses(addresses.clone()));
        sent.addresses = Some(addresses);
    }
    let runner = members
        .get(device)
        .or_else(|| members.by_machine(device))
        .is_some_and(|member| member.role == Role::Runner);
    if runner {
        let presence: Vec<PresenceOf> = members
            .members()
            .iter()
            .filter_map(|member| {
                let Presence { active, last_input } = hub.presence(&member.key)?;
                Some(PresenceOf {
                    key: member.key,
                    active,
                    last_input,
                })
            })
            .collect();
        if sent.presence.as_ref() != Some(&presence) {
            frames.push(ToDevice::Presence(presence.clone()));
            sent.presence = Some(presence);
        }
    }
    frames
}

/// A device's ask, through the hub's rules, the device welcome as the members are now: a session
/// opened before a change that leaves it out asks nothing more. Every session is told what
/// changed. The answer to a setup carries the hub's proof that it holds the code.
fn handle<C: Clock>(
    shared: &Shared<C>,
    transcript: &Transcript,
    pinned: Option<flottille_link::ChangeHash>,
    ask: Ask,
) -> Result<Option<SetupProof>, HubError> {
    let device = &transcript.device;
    let mut hub = shared.hub.lock().unwrap();
    if let Err(refusal) = hub.welcome(device, pinned) {
        let (reason, _) = *refusal;
        return Err(HubError(reason));
    }
    let mut proof = None;
    let done = match ask {
        Ask::Setup {
            proof: theirs,
            changes,
        } => setup(&mut hub, transcript, &theirs, changes).map(|ours| proof = Some(ours)),
        Ask::Submit { change } => {
            if hub.members().is_none() {
                Err(HubError(
                    "the hub has no member yet: the first one joins with the setup code".to_owned(),
                ))
            } else {
                hub.submit(change)
            }
        }
        Ask::Addresses { addresses } => {
            if addresses.len() > MOST_ADDRESSES
                || addresses.iter().any(|address| {
                    address.len() > LONGEST_ADDRESS || Address::parse(address).is_err()
                })
            {
                Err(HubError(format!(
                    "these are not addresses: at most {MOST_ADDRESSES}, each a host and a port"
                )))
            } else {
                hub.tell_addresses(device, addresses)
            }
        }
        Ask::Presence { active, last_input } => {
            let member = hub.members().and_then(|members| {
                members
                    .get(device)
                    .or_else(|| members.by_machine(device))
                    .map(|member| member.key)
            });
            match member {
                Some(member) => hub.tell_presence(&member, Presence { active, last_input }),
                None => Err(HubError("this device is not a member".to_owned())),
            }
        }
    };
    drop(hub);
    if done.is_ok() {
        shared.changed.send_modify(|at| *at += 1);
    }
    done.map(|()| proof)
}

/// The first member: its own entry and the declaration of the machine that presents the code,
/// which is this session's key, with the device's proof that it holds the code, for this session
/// and these changes. Both are checked before the code is taken, and stored in one write; the
/// hub's own proof is the answer.
fn setup<C: Clock>(
    hub: &mut Hub<C>,
    transcript: &Transcript,
    proof: &SetupProof,
    changes: Vec<Change>,
) -> Result<SetupProof, HubError> {
    let code = hub.setup_code()?.to_owned();
    let parsed = SetupCode::parse(&code).ok_or_else(not_the_code)?;
    let head = MemberList::from_changes(changes.clone()).head();
    if !transcript.proves(&parsed, 2, &head, proof) {
        return Err(not_the_code());
    }
    let [root, machine] = <[Change; 2]>::try_from(changes).map_err(|_| {
        HubError(
            "a setup is the first member's entry, then the declaration of its machine".to_owned(),
        )
    })?;
    let declares = matches!(&machine.action, Action::Machine { key, machine } if *key == root.signer && *machine == transcript.device);
    if !declares {
        return Err(HubError(
            "the setup's second change must declare the machine that presents the code".to_owned(),
        ));
    }
    hub.setup(&code, vec![root, machine])?;
    Ok(transcript.setup_proof(&parsed, 1, &head))
}

/// Until `stopping` says so; nothing held of it, so that a `select!` holding it stays `Send`.
async fn stopped(stopping: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = stopping.wait_for(|stop| *stop).await;
}
