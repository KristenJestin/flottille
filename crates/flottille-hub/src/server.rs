//! The hub on the network (`docs/remote.md`, "The hub", "Architecture"): it listens on one address,
//! plain (behind a reverse proxy that holds the certificate) or with TLS on a certificate given,
//! and keeps a control session with each device that proves its key (`flottille_link::arrive`).
//!
//! A session sends the device the member list, the addresses the members' machines told, and, to
//! a runner, the presence of the members' clients; then sends them again whenever they change. The
//! device's asks (the first member's setup, a member's change, its addresses, its presence) go
//! through the hub's rules (`Hub`). The hub carries no link's bytes: a session holds no message
//! and no queue, only the list, the addresses and the presence.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_link::{
    Action, Address, Ask, Change, Hello, HubSession, MemberList, PresenceOf, Role, ToDevice, Told,
    arrive,
};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::{Clock, Hub, HubError, Presence};

/// Where the hub listens unless told otherwise: every interface, one port. Whether it should be
/// loopback by default (a hub behind a reverse proxy on the same host) is open; it is this line.
pub const LISTEN: &str = "0.0.0.0:7448";

/// How long a TLS handshake that came in may take.
const TLS_WAIT: Duration = Duration::from_secs(10);
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
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = stopped(&mut stopping) => break,
        };
        // Finished sessions are let go as others come.
        while sessions.try_join_next().is_some() {}
        let Ok((stream, from)) = accepted else {
            continue;
        };
        let (shared, tls) = (shared.clone(), tls.clone());
        let stopping = stopping.clone();
        sessions.spawn(async move {
            let stream: Box<dyn flottille_link::Stream> = match tls {
                Some(tls) => match tokio::time::timeout(TLS_WAIT, tls.accept(stream)).await {
                    Ok(Ok(secured)) => secured,
                    _ => return,
                },
                None => Box::new(stream),
            };
            let arriving = match arrive(stream, &shared.key, &shared.hello).await {
                Ok(arriving) => arriving,
                Err(why) => {
                    eprintln!("flottille-hub: a connection from {from} was not taken: {why}");
                    return;
                }
            };
            let device = arriving.device();
            let welcome = shared.hub.lock().unwrap().welcome(&device);
            if let Err(refusal) = welcome {
                let (reason, root) = *refusal;
                eprintln!(
                    "flottille-hub: refused {} from {from}: {reason}",
                    device.fingerprint()
                );
                arriving.refuse(&reason, root).await;
                return;
            }
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
    let mut changed = shared.changed.subscribe();
    let mut sent = Sent::default();
    loop {
        let frames = {
            let hub = shared.hub.lock().unwrap();
            hub.welcome(&device)
                .map(|()| news(&hub, &device, &mut sent))
        };
        let frames = match frames {
            Ok(frames) => frames,
            Err(refusal) => {
                let (reason, root) = *refusal;
                let _ = session.send(&ToDevice::Refused { reason, root }).await;
                session.close().await;
                return "it is not a member any more".to_owned();
            }
        };
        for frame in frames {
            if let Err(error) = session.send(&frame).await {
                return error.to_string();
            }
        }
        tokio::select! {
            asked = session.ask() => {
                let asked = match asked {
                    Ok(asked) => asked,
                    Err(why) => return why,
                };
                let answer = handle(shared, &device, asked.ask);
                let refused = answer.err().map(|error| error.to_string());
                if session.send(&ToDevice::Answer { id: asked.id, refused }).await.is_err() {
                    return "it closed the connection".to_owned();
                }
            }
            _ = changed.changed() => {}
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

/// A device's ask, through the hub's rules; every session is told what changed.
fn handle<C: Clock>(
    shared: &Shared<C>,
    device: &flottille_link::PublicKey,
    ask: Ask,
) -> Result<(), HubError> {
    let mut hub = shared.hub.lock().unwrap();
    let done = match ask {
        Ask::Setup { code, changes } => setup(&mut hub, device, &code, changes),
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
    done
}

/// The first member: its own entry and the declaration of the machine that presents the code,
/// which is this session's key. Both are checked before the code is taken.
fn setup<C: Clock>(
    hub: &mut Hub<C>,
    device: &flottille_link::PublicKey,
    code: &str,
    changes: Vec<Change>,
) -> Result<(), HubError> {
    let [root, machine] = <[Change; 2]>::try_from(changes).map_err(|_| {
        HubError(
            "a setup is the first member's entry, then the declaration of its machine".to_owned(),
        )
    })?;
    let declares = matches!(&machine.action, Action::Machine { key, machine } if *key == root.signer && machine == device);
    if !declares {
        return Err(HubError(
            "the setup's second change must declare the machine that presents the code".to_owned(),
        ));
    }
    MemberList::from_changes(vec![root.clone(), machine.clone()])
        .check(&root.signer)
        .map_err(|error| HubError(format!("the hub refuses this setup: {error}")))?;
    let signer = root.signer;
    hub.setup(code, &signer, root)?;
    hub.submit(machine)
}

/// Until `stopping` says so; nothing held of it, so that a `select!` holding it stays `Send`.
async fn stopped(stopping: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = stopping.wait_for(|stop| *stop).await;
}
