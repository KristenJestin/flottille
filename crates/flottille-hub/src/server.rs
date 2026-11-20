//! The hub on the network (`docs/remote.md`, "The hub", "Architecture"): it listens on one address,
//! plain (behind a reverse proxy that holds the certificate) or with TLS on a certificate given,
//! and keeps a control session with each device that proves its key (`flottille_link::arrive`).
//!
//! A session sends the device the member list, the addresses the members' machines told, what the
//! hub knows of each member's device, and, to a runner, the presence of the members' clients; then
//! sends them again whenever they change. The device's asks (a member's change, what it says of
//! itself, its addresses, its presence) go through the hub's rules (`Hub`), each checked again
//! against the members as they are when it comes in. The hub carries no link's bytes: a session
//! holds no message and no queue.
//!
//! A device that is no member yet may pair (`flottille_link::Pairing`) while pair mode is open:
//! its proof is checked against the pair mode's secret, the device is paired, and its session is
//! sent the list and may declare its machine, `PAIR_IDLE` at most.
//!
//! The commands of the hub's host (`flottille-hub pair`, `role`, `remove`, `devices`) reach the hub
//! that runs on its socket, `hub.sock` in its state folder (`serve_local`): the folder is the
//! user's only, and a connection from another user is refused. One JSON object a line:
//!
//! ```text
//! {"pair": {"url": U | null, "minutes": n | null}} → {"code": C, "url": U, "left": s}, then
//!                                                    {"paired": {…}} or {"over": "…"}
//! {"role": {"device": D, "role": R}}               → {"ok": {…}} or {"refused": "…"}
//! {"remove": {"device": D}}                        → {"ok": {…}} or {"refused": "…"}
//! {"devices": {}}                                  → {"ok": [{…}]}
//! ```
//!
//! A pair mode lasts while its command does: a command that stops closes it.
//!
//! What someone with no key may hold of the hub is bounded: at most `MOST_ARRIVING` connections
//! that have not proved a member's key at once (a pairing's session among them), then
//! `KEPT_FOR_MEMBERS` more that are let go unless they prove one, a few KiB a handshake line,
//! `ARRIVAL_WAIT` for the TLS handshake and the hub's, and `PAIR_IDLE` from its start a pairing's
//! session. When the process runs out of file descriptors, it waits before it accepts again.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_link::{
    Address, Ask, Facts, Hello, HubSession, MemberList, PresenceOf, PublicKey, Role, ToDevice,
    Told, arrive,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use crate::{Clock, Hub, HubError, PairEnd, Presence};

/// Where the hub listens unless told otherwise: loopback, behind a reverse proxy on the same
/// host. Another address takes TLS, or `--plain` for a proxy elsewhere.
pub const LISTEN: &str = "127.0.0.1:7448";

/// The most connections that have not proved a member's key the hub holds at once: one more is
/// let go as soon as it is accepted.
pub const MOST_ARRIVING: usize = 64;
/// The places kept, past `MOST_ARRIVING`, for connections that prove a member's key: one that
/// does not is let go at the end of its handshake.
pub const KEPT_FOR_MEMBERS: usize = 8;
/// How long a pairing's session lasts, however much it asks, before it is closed, unless the
/// machine it declared made it a member's: time to take the list and declare its machine.
pub const PAIR_IDLE: Duration = Duration::from_secs(30);
/// The socket of the hub's host's commands, in its state folder.
pub const SOCKET_FILE: &str = "hub.sock";
/// The longest line a command of the hub's host says.
const COMMAND_LIMIT: usize = 4 << 10;
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

/// The waits of the server: how long a connection may take to arrive, and how long a pairing's
/// session lasts. The production's are the default; a test serves with shorter ones
/// (`serve_with`).
#[derive(Clone, Copy, Debug)]
pub struct Waits {
    pub arrival: Duration,
    pub pair_idle: Duration,
}

impl Default for Waits {
    fn default() -> Waits {
        Waits {
            arrival: ARRIVAL_WAIT,
            pair_idle: PAIR_IDLE,
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
    /// The URL a pair code holds unless its command gives one: `--public-url`, else the address
    /// the hub listens on; `None` when that one is no address another device dials (`0.0.0.0`, the
    /// loopback): a code that holds it would pair nothing, and says so when it is asked for.
    url: Mutex<Option<String>>,
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
    let scheme = if tls.is_some() { "https" } else { "http" };
    let url = (!address.ip().is_unspecified() && !address.ip().is_loopback())
        .then(|| format!("{scheme}://{address}"));
    let shared = Arc::new(Shared {
        key: hub.key.clone(),
        hub: Mutex::new(hub),
        changed: watch::channel(0).0,
        hello,
        waits,
        url: Mutex::new(url),
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

    /// The URL a pair code holds unless its command gives one, in place of the address the hub
    /// listens on (`--public-url`): where the devices reach it, through a reverse proxy mostly.
    pub fn set_public_url(&self, url: String) {
        *self.shared.url.lock().unwrap() = Some(url);
    }

    /// Serves the commands of the hub's host on the socket `path` (`hub.sock` in the state folder,
    /// whose lock the hub holds: one left by a hub that stopped is replaced), until `stop`.
    pub fn serve_local(&self, path: &Path) -> std::io::Result<()> {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        let shared = self.shared.clone();
        let mut stopping = self.stop.subscribe();
        tokio::spawn(async move {
            let mut commands = JoinSet::new();
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    _ = stopped(&mut stopping) => break,
                };
                while commands.try_join_next().is_some() {}
                let Ok((stream, _)) = accepted else {
                    tokio::time::sleep(ACCEPT_WAIT).await;
                    continue;
                };
                // The folder is the user's only; another user that reached the socket anyway is
                // refused, but root, which reaches the state folder anyway (a systemd service's
                // user is its own).
                // SAFETY: getuid has no precondition and cannot fail.
                let own = unsafe { libc::getuid() };
                let peer = stream.peer_cred().map(|cred| cred.uid()).ok();
                if peer != Some(own) && peer != Some(0) {
                    continue;
                }
                let shared = shared.clone();
                let stopping = stopping.clone();
                commands.spawn(async move { command(stream, &shared, stopping).await });
            }
            commands.shutdown().await;
        });
        Ok(())
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
            let version = arriving.version().to_owned();
            let (welcome, member) = {
                let mut hub = shared.hub.lock().unwrap();
                let welcome = hub.welcome(&device, arriving.pinned());
                // A device that pairs: one that is no member, or whose machine is a member's
                // already (its hub forgotten on it), which spends pair mode all the same. In a place
                // kept for members it is not taken, and pair mode stays open.
                let welcome = match (welcome, arriving.pairing()) {
                    (_, Some(_)) if members_only => Err(Box::new((
                        "the hub is busy: try again in a moment".to_owned(),
                        false,
                    ))),
                    (_, Some(pairing)) => {
                        let transcript = *arriving.transcript();
                        hub.pair(
                            &pairing.proposed,
                            |secret| {
                                transcript.proves_pair(secret, &pairing.proposed, &pairing.proof)
                            },
                            &version,
                        )
                        .map(|paired| Some(paired.key))
                        .map_err(|error| Box::new((error.to_string(), false)))
                    }
                    (welcome, _) => welcome.map(|()| None),
                };
                let member = member_of(&hub, &device, None);
                if let Some(member) = member {
                    let _ = hub.seen(&member, &version);
                }
                (welcome, member)
            };
            // Told at once: a pairing changed the list or ended pair mode; a member was seen.
            shared.changed.send_modify(|at| *at += 1);
            let paired = match welcome {
                Ok(paired) => paired,
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
            // A member's session is not counted; a pairing's is, while it lasts.
            let _permit = paired.is_some().then_some(permit);
            let Ok(session) = arriving.welcome().await else {
                return;
            };
            match &paired {
                Some(key) => eprintln!(
                    "flottille-hub: {} paired {} from {from}",
                    device.fingerprint(),
                    key.fingerprint()
                ),
                None => eprintln!(
                    "flottille-hub: {} connected from {from}",
                    device.fingerprint()
                ),
            }
            let why = run(session, paired, &shared, stopping).await;
            if let Some(member) = member.or(paired) {
                let _ = shared.hub.lock().unwrap().seen(&member, &version);
            }
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
    facts: Option<Vec<Facts>>,
}

/// The member a session is: its device's, its machine's, or, for a pairing's, the device paired.
fn member_of<C: Clock>(
    hub: &Hub<C>,
    device: &PublicKey,
    paired: Option<&PublicKey>,
) -> Option<PublicKey> {
    let members = hub.members()?;
    members
        .get(device)
        .or_else(|| members.by_machine(device))
        .or_else(|| paired.and_then(|paired| members.get(paired)))
        .map(|member| member.key)
}

/// Whether `device`'s session may go on, as the members are now: a member's, or a pairing's whose
/// device is still a member. The refusal that ends it otherwise.
fn standing<C: Clock>(
    hub: &Hub<C>,
    device: &PublicKey,
    pinned: Option<flottille_link::ChangeHash>,
    paired: Option<&PublicKey>,
) -> Result<(), (String, bool)> {
    match hub.welcome(device, pinned) {
        Ok(()) => Ok(()),
        Err(_) if member_of(hub, device, paired).is_some() => Ok(()),
        Err(refusal) => Err(*refusal),
    }
}

/// One device's session, until it ends; why it ended.
async fn run<C: Clock + Send + 'static>(
    mut session: HubSession,
    paired: Option<PublicKey>,
    shared: &Shared<C>,
    mut stopping: watch::Receiver<bool>,
) -> String {
    let device = session.device();
    let pinned = session.pinned();
    let mut changed = shared.changed.subscribe();
    let mut sent = Sent::default();
    // Set once: a pairing's session that keeps asking is closed as one that asks nothing.
    let pair_ends = tokio::time::Instant::now() + shared.waits.pair_idle;
    loop {
        let frames = {
            let hub = shared.hub.lock().unwrap();
            standing(&hub, &device, pinned, paired.as_ref()).map(|()| {
                // Until it declared its machine, or when it is the device itself (a phone).
                let pairing = paired.is_some() && hub.welcome(&device, pinned).is_err();
                (news(&hub, &device, paired.as_ref(), &mut sent), pairing)
            })
        };
        let (frames, pairing) = match frames {
            Ok(frames) => frames,
            Err((reason, reset)) => {
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
            if pairing {
                tokio::time::sleep_until(pair_ends).await;
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
                let refused = handle(shared, &device, pinned, paired.as_ref(), asked.ask)
                    .err()
                    .map(|error| error.to_string());
                let answer = ToDevice::Answer { id: asked.id, refused };
                if session.send(&answer).await.is_err() {
                    return "it closed the connection".to_owned();
                }
            }
            _ = changed.changed() => {}
            _ = idle => {
                let reason = format!(
                    "a pairing's session lasts {} s: this machine declared none in time",
                    shared.waits.pair_idle.as_secs()
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
    device: &PublicKey,
    paired: Option<&PublicKey>,
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
    let facts = hub.facts().to_vec();
    if sent.facts.as_ref() != Some(&facts) {
        frames.push(ToDevice::Facts(facts.clone()));
        sent.facts = Some(facts);
    }
    let member = member_of(hub, device, paired).and_then(|key| members.get(&key));
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
/// changed.
fn handle<C: Clock>(
    shared: &Shared<C>,
    device: &PublicKey,
    pinned: Option<flottille_link::ChangeHash>,
    paired: Option<&PublicKey>,
    ask: Ask,
) -> Result<(), HubError> {
    let mut hub = shared.hub.lock().unwrap();
    standing(&hub, device, pinned, paired).map_err(|(reason, _)| HubError(reason))?;
    let member = member_of(&hub, device, paired);
    let not_member = || HubError("this device is not a member".to_owned());
    let done = match ask {
        Ask::Submit { change } => hub.submit(change),
        Ask::About {
            device: kind,
            system,
        } => match member {
            Some(member) => hub.about(&member, &kind, &system),
            None => Err(not_member()),
        },
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
            None => Err(not_member()),
        },
    };
    drop(hub);
    if done.is_ok() {
        shared.changed.send_modify(|at| *at += 1);
    }
    done
}

/// One command of the hub's host, on its socket, until it is answered; a pair mode until it
/// ends, closed when the command stops first.
async fn command<C: Clock + Send + 'static>(
    stream: UnixStream,
    shared: &Shared<C>,
    mut stopping: watch::Receiver<bool>,
) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read.take(COMMAND_LIMIT as u64)).lines();
    let Ok(Some(line)) = lines.next_line().await else {
        return;
    };
    let say = |value: Value| {
        let mut line = value.to_string();
        line.push('\n');
        line
    };
    let asked: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    let answer = |done: Result<Value, HubError>| match done {
        Ok(value) => json!({"ok": value}),
        Err(error) => json!({"refused": error.to_string()}),
    };
    let done = if let Some(pair) = asked.get("pair") {
        let opened = {
            let mut hub = shared.hub.lock().unwrap();
            let url = pair["url"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| shared.url.lock().unwrap().clone());
            match url {
                None => Err(HubError(
                    "the hub listens on a loopback or a wildcard address, which no other device \
                     dials: give the address the devices reach it at, with --public-url (for \
                     example flottille-hub pair --public-url https://hub.example.net, or on the \
                     hub's own command line, to make it the default)"
                        .to_owned(),
                )),
                Some(url) => hub
                    .open_pairing(&url, pair["minutes"].as_u64())
                    .map(|(number, code)| (number, code, hub.pairing_left().unwrap_or(0))),
            }
        };
        let (number, code, left) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                let _ = write.write_all(say(answer(Err(error))).as_bytes()).await;
                return;
            }
        };
        let text = code.code().expect("checked when it was opened");
        let shown = json!({"code": text, "url": code.hub_url, "left": left});
        if write.write_all(say(shown).as_bytes()).await.is_err() {
            shared.hub.lock().unwrap().close_pairing(number);
            return;
        }
        let mut changed = shared.changed.subscribe();
        let end = loop {
            if let Some(end) = shared.hub.lock().unwrap().pairing_ended(number) {
                break end;
            }
            // Its minutes end it: looked at again every second.
            tokio::select! {
                _ = changed.changed() => {}
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                // The command stopped: nothing more comes on its socket.
                _ = lines.next_line() => {
                    shared.hub.lock().unwrap().close_pairing(number);
                    shared.changed.send_modify(|at| *at += 1);
                    return;
                }
                _ = stopped(&mut stopping) => {
                    break PairEnd::Over("the hub stops".to_owned());
                }
            }
        };
        match end {
            PairEnd::Paired(paired) => json!({"paired": paired}),
            PairEnd::Over(why) => json!({"over": why}),
        }
    } else if let Some(role) = asked.get("role") {
        let device = role["device"].as_str().unwrap_or_default();
        let done = match role["role"].as_str().and_then(Role::from_word) {
            None => Err(HubError(
                "a role is runner, controller or viewer".to_owned(),
            )),
            Some(word) => shared
                .hub
                .lock()
                .unwrap()
                .change_role(device, word)
                .map(|member| json!({"name": member.name, "role": member.role})),
        };
        shared.changed.send_modify(|at| *at += 1);
        answer(done)
    } else if let Some(removed) = asked.get("remove") {
        let device = removed["device"].as_str().unwrap_or_default();
        let done = shared
            .hub
            .lock()
            .unwrap()
            .remove(device)
            .map(|member| json!({"name": member.name}));
        shared.changed.send_modify(|at| *at += 1);
        answer(done)
    } else if asked.get("devices").is_some() {
        answer(Ok(devices(&shared.hub.lock().unwrap())))
    } else {
        answer(Err(HubError(
            "this is no command of flottille-hub's".to_owned(),
        )))
    };
    let _ = write.write_all(say(done).as_bytes()).await;
}

/// The members, as `flottille-hub devices` says them.
fn devices<C: Clock>(hub: &Hub<C>) -> Value {
    let Some(members) = hub.members() else {
        return json!([]);
    };
    let hub_key = hub.key();
    let name_of = |key: &PublicKey| {
        if *key == hub_key {
            return "the hub".to_owned();
        }
        members
            .get(key)
            .map_or_else(|| key.fingerprint(), |member| member.name.clone())
    };
    Value::Array(
        members
            .members()
            .iter()
            .map(|member| {
                let facts = hub.facts().iter().find(|facts| facts.key == member.key);
                json!({
                    "name": member.name,
                    "fingerprint": member.key.fingerprint(),
                    "role": member.role,
                    "added_by": name_of(&member.added_by),
                    "machine": member.machine.map(|machine| machine.fingerprint()),
                    "last_seen": facts.and_then(|facts| facts.last_seen),
                })
            })
            .collect(),
    )
}

/// Until `stopping` says so; nothing held of it, so that a `select!` holding it stays `Send`.
async fn stopped(stopping: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = stopping.wait_for(|stop| *stop).await;
}
