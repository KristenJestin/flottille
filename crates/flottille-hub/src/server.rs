//! The hub on the network (`docs/remote.md`, "The hub", "Architecture"): it listens on one address,
//! plain (behind a reverse proxy that holds the certificate) or with TLS on a certificate given,
//! and keeps a control session with each device that proves its key (`flottille_link::arrive`).
//!
//! A session sends the device the member list, the addresses the members' machines told, and, to
//! a runner, the presence of the members' clients; then sends them again whenever they change. The
//! device's asks (the first member's setup, a member's change, an invitation, the refusal of a
//! join request, its addresses, its presence) go through the hub's rules (`Hub`), each checked
//! again against the members as they are when it comes in.
//!
//! A session also carries the links between the user's devices (`relay.rs`): each packet a
//! member's device sends for another member's device goes to that device's session, if it is open;
//! any other is dropped. Members are told which members' sessions are open. Each side says
//! `keepalive` when it has nothing to say, so that a reverse proxy does not close the session.
//!
//! A machine that is no member yet may answer an invitation (`flottille_link::Joining`): its
//! session is welcome while its join request waits, and is sent nothing; once the member that
//! invited added its device, it is sent the list and may declare its machine. The member that
//! invited is sent the requests that wait for it.
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
    Action, Address, Ask, Change, Hello, HubSession, Joining, KEEPALIVE, LinkPacket, MemberList,
    PresenceOf, PublicKey, RELAY_PACKET, Requested, Role, SetupCode, SetupProof, ToDevice, Token,
    Told, Transcript, arrive,
};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use crate::relay::Relay;
use crate::{Clock, Hub, HubError, Presence, Standing, not_the_code};

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

/// The waits of the server: how long a connection may take to arrive, and how long a session
/// opened to set the hub up lasts. The production's are the default; a test serves with shorter
/// ones (`serve_with`).
#[derive(Clone, Copy, Debug)]
pub struct Waits {
    pub arrival: Duration,
    pub setup_idle: Duration,
}

impl Default for Waits {
    fn default() -> Waits {
        Waits {
            arrival: ARRIVAL_WAIT,
            setup_idle: SETUP_IDLE,
        }
    }
}

/// What the sessions share: the hub, and a number raised at each change they must send.
struct Shared<C: Clock> {
    hub: Mutex<Hub<C>>,
    key: Arc<flottille_link::HubKey>,
    changed: watch::Sender<u64>,
    hello: Hello,
    waits: Waits,
    /// The sessions the links between devices go through.
    relay: Relay,
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
    serve_with(hub, listener, tls, hello, Waits::default())
}

/// [`serve`], with the waits `waits` says.
pub fn serve_with<C: Clock + Send + 'static>(
    hub: Hub<C>,
    listener: TcpListener,
    tls: Option<flottille_link::TlsServer>,
    hello: Hello,
    waits: Waits,
) -> std::io::Result<Served<C>> {
    let address = listener.local_addr()?;
    let shared = Arc::new(Shared {
        key: hub.key.clone(),
        hub: Mutex::new(hub),
        changed: watch::channel(0).0,
        hello,
        waits,
        relay: Relay::default(),
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
            let arriving = match tokio::time::timeout(shared.waits.arrival, arrival).await {
                Ok(Ok(arriving)) => arriving,
                Ok(Err(why)) => {
                    eprintln!("flottille-hub: a connection from {from} was not taken: {why}");
                    return;
                }
                Err(_) => return,
            };
            let device = arriving.device();
            let (welcome, setting_up) = {
                let mut hub = shared.hub.lock().unwrap();
                let welcome = hub.welcome(&device, arriving.pinned());
                // A machine that is no member answers an invitation: its request waits. In a
                // place kept for members it is not taken, and the invitation stays unanswered.
                let welcome = match (welcome, arriving.joining()) {
                    (Err(_), Some(_)) if members_only => Err(Box::new((
                        "the hub is busy: try again in a moment".to_owned(),
                        false,
                    ))),
                    (Err(_), Some(joining)) => hub
                        .join(&joining.token, joining.key)
                        .map(|()| Some(joining))
                        .map_err(|error| Box::new((error.to_string(), false))),
                    (welcome, _) => welcome.map(|()| None),
                };
                (welcome, hub.members().is_none())
            };
            let joining = match welcome {
                Ok(joining) => joining,
                Err(refusal) => {
                    let (reason, reset) = *refusal;
                    eprintln!(
                        "flottille-hub: refused {} from {from}: {reason}",
                        device.fingerprint()
                    );
                    arriving.refuse(&reason, reset).await;
                    return;
                }
            };
            if joining.is_some() {
                // The member that invited is told the request.
                shared.changed.send_modify(|at| *at += 1);
            }
            // `welcome` takes anyone while no member exists: a setup is no member's.
            if members_only && setting_up {
                arriving
                    .refuse("the hub is busy: try again in a moment", false)
                    .await;
                return;
            }
            // A member's session is not counted; a setup's or a joining machine's is, while it
            // lasts.
            let _permit = (setting_up || joining.is_some()).then_some(permit);
            let Ok(session) = arriving.welcome().await else {
                return;
            };
            eprintln!(
                "flottille-hub: {} connected from {from}{}",
                device.fingerprint(),
                if joining.is_some() { " to join" } else { "" }
            );
            let why = run(session, joining, &shared, stopping).await;
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
    requests: Option<Vec<Requested>>,
    online: Option<Vec<PublicKey>>,
}

/// Whether a session may go on, as the members are now: a member's (or a setup's), or a joining
/// machine's, whose request waits (it is sent nothing and may ask nothing) or was accepted.
enum Welcome {
    Member,
    Waiting { left: u64 },
}

/// Where `device`'s session stands, or the refusal that ends it.
fn standing<C: Clock>(
    hub: &mut Hub<C>,
    device: &PublicKey,
    pinned: Option<flottille_link::ChangeHash>,
    joining: Option<&Joining>,
) -> Result<Welcome, (String, bool)> {
    let refusal = match hub.welcome(device, pinned) {
        Ok(()) => return Ok(Welcome::Member),
        Err(refusal) => *refusal,
    };
    let Some(joining) = joining else {
        return Err(refusal);
    };
    match hub.joined(&joining.token, &joining.key) {
        Standing::Added => Ok(Welcome::Member),
        Standing::Waiting { left } => Ok(Welcome::Waiting { left }),
        Standing::Over => Err((
            "this join request was refused, or its invitation expired: ask for a new one"
                .to_owned(),
            false,
        )),
    }
}

/// One device's session, until it ends; why it ended.
async fn run<C: Clock + Send + 'static>(
    mut session: HubSession,
    joining: Option<Joining>,
    shared: &Shared<C>,
    mut stopping: watch::Receiver<bool>,
) -> String {
    let device = session.device();
    let pinned = session.pinned();
    let mut changed = shared.changed.subscribe();
    let mut sent = Sent::default();
    // Set once: a session that keeps asking is closed as one that asks nothing.
    let setup_ends = tokio::time::Instant::now() + shared.waits.setup_idle;
    let mut relays = session.relays();
    // Every session is told when this one closes, once it is no longer open to the links: this is
    // let go after `opened`.
    struct SayClosed<'a>(&'a watch::Sender<u64>);
    impl Drop for SayClosed<'_> {
        fn drop(&mut self) {
            self.0.send_modify(|at| *at += 1);
        }
    }
    let _say_closed = SayClosed(&shared.changed);
    // Open to the links once the device is a member's.
    let mut opened = None;
    let mut keepalive = tokio::time::interval(KEEPALIVE);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let frames = {
            let mut hub = shared.hub.lock().unwrap();
            standing(&mut hub, &device, pinned, joining.as_ref()).map(|standing| match standing {
                Welcome::Member => {
                    let member = is_member(&hub, &device);
                    if member && opened.is_none() {
                        opened = Some(shared.relay.open(device));
                        shared.changed.send_modify(|at| *at += 1);
                    }
                    let mut frames = news(&hub, &device, joining.as_ref(), &mut sent);
                    if member {
                        let online: Vec<PublicKey> = shared
                            .relay
                            .online()
                            .into_iter()
                            .filter(|key| is_member(&hub, key))
                            .collect();
                        if sent.online.as_ref() != Some(&online) {
                            frames.push(ToDevice::Online(online.clone()));
                            sent.online = Some(online);
                        }
                    }
                    (frames, hub.members().is_none(), None)
                }
                Welcome::Waiting { left } => (Vec::new(), false, Some(left)),
            })
        };
        let (frames, setting_up, waiting) = match frames {
            Ok(frames) => frames,
            Err((reason, reset)) => {
                let _ = session.send(&ToDevice::Refused { reason, reset }).await;
                session.close().await;
                if joining.is_some() {
                    // The member that invited is told the request is over.
                    shared.changed.send_modify(|at| *at += 1);
                    return "its join request is over".to_owned();
                }
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
        // A request that waits ends with its invitation: looked at again then.
        let expiry = async {
            match waiting {
                Some(left) => tokio::time::sleep(Duration::from_secs(left)).await,
                None => std::future::pending::<()>().await,
            }
        };
        let relayed = async {
            match relays.as_mut() {
                Some(relays) => relays.next().await,
                None => std::future::pending().await,
            }
        };
        let forwarded = async {
            match opened.as_mut() {
                Some(opened) => opened.inbox.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            (to, packet) = relayed => {
                // Only from a member's device open to the links, only to a member's: any other
                // packet is dropped, and so is one longer than a link sends.
                let allowed = opened.is_some() && packet.len() <= RELAY_PACKET && {
                    let hub = shared.hub.lock().unwrap();
                    is_member(&hub, &device) && is_member(&hub, &to)
                };
                if allowed {
                    shared.relay.forward(device, &to, packet);
                }
            }
            Some((from, packet)) = forwarded => {
                let frame = ToDevice::Relayed { from, packet: LinkPacket(packet) };
                if let Err(error) = session.send(&frame).await {
                    return error.to_string();
                }
            }
            _ = keepalive.tick() => {
                if let Err(error) = session.send(&ToDevice::Keepalive {}).await {
                    return error.to_string();
                }
            }
            asked = session.ask() => {
                let asked = match asked {
                    Ok(asked) => asked,
                    Err(why) => return why,
                };
                let transcript = *session.transcript();
                let handled = handle(shared, &transcript, pinned, joining.as_ref(), asked.ask);
                let (refused, extra) = match handled {
                    Ok(extra) => (None, extra),
                    Err(error) => (Some(error.to_string()), Extra::default()),
                };
                let answer = ToDevice::Answer {
                    id: asked.id,
                    refused,
                    proof: extra.proof,
                    token: extra.token,
                };
                if session.send(&answer).await.is_err() {
                    return "it closed the connection".to_owned();
                }
            }
            _ = changed.changed() => {}
            _ = expiry => {}
            _ = idle => {
                let reason = format!(
                    "no setup came within {} s of this session",
                    shared.waits.setup_idle.as_secs()
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

/// Whether `key` is a member's device or a member's machine: the only keys links go between.
fn is_member<C: Clock>(hub: &Hub<C>, key: &PublicKey) -> bool {
    hub.members().is_some_and(|members| {
        members
            .get(key)
            .or_else(|| members.by_machine(key))
            .is_some()
    })
}

/// What `device` was not sent yet of the hub's state.
fn news<C: Clock>(
    hub: &Hub<C>,
    device: &PublicKey,
    joining: Option<&Joining>,
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
    let member = members
        .get(device)
        .or_else(|| members.by_machine(device))
        .or_else(|| joining.and_then(|joining| members.get(&joining.key)));
    if let Some(member) = member {
        let requests: Vec<Requested> = hub
            .pending(&member.key)
            .into_iter()
            .map(|request| Requested {
                token: request.token,
                key: request.key,
            })
            .collect();
        if sent.requests.as_ref() != Some(&requests) {
            frames.push(ToDevice::Requests(requests.clone()));
            sent.requests = Some(requests);
        }
    }
    let runner = member.is_some_and(|member| member.role == Role::Runner);
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
    joining: Option<&Joining>,
    ask: Ask,
) -> Result<Extra, HubError> {
    let device = &transcript.device;
    let mut hub = shared.hub.lock().unwrap();
    match standing(&mut hub, device, pinned, joining) {
        Ok(Welcome::Member) => {}
        Ok(Welcome::Waiting { .. }) => {
            return Err(HubError(
                "this device waits for its join request to be accepted: it may ask nothing yet"
                    .to_owned(),
            ));
        }
        Err((reason, _)) => return Err(HubError(reason)),
    }
    // The member this session is: its device's, its machine's, or, for a joining machine, the
    // new device's once added.
    let member = hub.members().and_then(|members| {
        members
            .get(device)
            .or_else(|| members.by_machine(device))
            .or_else(|| joining.and_then(|joining| members.get(&joining.key)))
            .map(|member| member.key)
    });
    let mut extra = Extra::default();
    let done = match ask {
        Ask::Setup {
            proof: theirs,
            changes,
        } => setup(&mut hub, transcript, &theirs, changes).map(|ours| extra.proof = Some(ours)),
        Ask::Invite { ask } => hub.invite(&ask).map(|token| extra.token = Some(token)),
        Ask::Refuse { token } => match member {
            Some(member) => hub.refuse(&token, &member),
            None => Err(HubError("this device is not a member".to_owned())),
        },
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
        Ask::Presence { active, last_input } => match member {
            Some(member) => hub.tell_presence(&member, Presence { active, last_input }),
            None => Err(HubError("this device is not a member".to_owned())),
        },
    };
    drop(hub);
    if done.is_ok() {
        shared.changed.send_modify(|at| *at += 1);
    }
    done.map(|()| extra)
}

/// What an answer carries besides a yes: the hub's proof of the setup code, an invitation's
/// token.
#[derive(Default)]
struct Extra {
    proof: Option<SetupProof>,
    token: Option<Token>,
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
