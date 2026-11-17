//! The daemon's control session to the user's hub (`docs/remote.md`, "The hub", "Architecture"),
//! with remote on and the machine set up: it keeps the member list, checked back to the root
//! pinned at setup, and the addresses the hub tells of the members' machines, and writes from them
//! the machines the link trusts (`remote/peers.json`); it tells the hub this machine's addresses.
//!
//! The hub is not trusted with the members: a list that does not check is refused here, whatever
//! the hub said, and the machines trusted stay those of the last list that checked. While the hub
//! is unreachable, the links and the machines known go on; what needs the hub says so, with since
//! when.
//!
//! What the machine pinned is `remote/hub.json` (`flottille_link::Device`): the hub's URL and
//! key, the list's root, and the list as last checked. `remote.setup` writes it; #812's joining
//! will too.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_link::{
    Action, Ask, Change, ControlError, Device, HubUrl, ListError, MachineKey, Members, PublicKey,
    Pushed, Session, Tls,
};
use serde_json::{Value, json};

use crate::link::Remote;

/// The machine's hub, as it pinned it.
pub const HUB_FILE: &str = "hub.json";
/// The longest a hub that cannot be reached waits before it is tried again.
const LONGEST_WAIT: Duration = Duration::from_secs(30);
/// How often this machine's addresses are compared with those the hub was told.
const ROUND: Duration = Duration::from_secs(1);
/// How long the hub may take to answer an ask before it is asked again.
const ASK_WAIT: Duration = Duration::from_secs(10);

/// How the daemon sees its hub.
struct Seen {
    state: &'static str,
    /// Since when, in milliseconds since the Unix epoch.
    since: u64,
    reason: Option<String>,
}

/// The control session's side of the daemon.
pub struct Control {
    key: MachineKey,
    folder: PathBuf,
    seen: Mutex<Seen>,
    /// Raised when `hub.json` was written by a setup: the session starts again from it.
    again: tokio::sync::Notify,
}

impl Control {
    pub fn new(key: MachineKey, folder: PathBuf) -> Control {
        Control {
            key,
            folder,
            seen: Mutex::new(Seen {
                state: "none",
                since: now(),
                reason: None,
            }),
            again: tokio::sync::Notify::new(),
        }
    }

    fn file(&self) -> PathBuf {
        self.folder.join(HUB_FILE)
    }

    /// The hub this machine pinned; `None` when it is not set up.
    fn pinned(&self) -> Option<Result<Device, String>> {
        match std::fs::read_to_string(self.file()) {
            Ok(text) => Some(serde_json::from_str(&text).map_err(|error| {
                format!(
                    "{} is not a hub this machine pinned: {error}",
                    self.file().display()
                )
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(Err(format!(
                "{} cannot be read: {error}",
                self.file().display()
            ))),
        }
    }

    fn keep(&self, device: &Device) -> std::io::Result<()> {
        crate::link::write_json(&self.file(), device)
    }

    /// The hub's state, said once in the log when it changes.
    fn set(&self, state: &'static str, reason: Option<String>) {
        let mut seen = self.seen.lock().unwrap();
        if seen.state != state || seen.reason != reason {
            match &reason {
                Some(reason) => eprintln!("flottille daemon: {reason}"),
                None => eprintln!("flottille daemon: the hub is {state}"),
            }
        }
        if seen.state != state {
            seen.since = now();
        }
        seen.state = state;
        seen.reason = reason;
    }

    /// The hub as `remote.machines` says it; `null` when this machine is not set up.
    pub fn seen(&self) -> Value {
        let Some(Ok(device)) = self.pinned() else {
            return Value::Null;
        };
        let seen = self.seen.lock().unwrap();
        json!({
            "url": device.url,
            "key": device.hub,
            "fingerprint": device.hub.fingerprint(),
            "state": seen.state,
            "since": seen.since,
            "reason": seen.reason,
        })
    }

    /// `remote.setup`: this machine, carrying its device's signed changes, becomes the hub's
    /// first member; the declaration of this machine is claimed here, with the machine's key. The
    /// hub's key is pinned from this session. A machine set up already is set up again only when
    /// asked to `replace` the hub it pinned.
    pub async fn setup(
        &self,
        url: &str,
        code: &str,
        mut changes: Vec<Change>,
        replace: bool,
    ) -> Result<Value, String> {
        if !replace && let Some(pinned) = self.pinned() {
            let url =
                pinned.map_or_else(|_| self.file().display().to_string(), |device| device.url);
            return Err(format!(
                "this machine is set up with the hub at {url} already: give --replace to set it \
                 up with another (the machines it trusts through that hub are forgotten)"
            ));
        }
        let own = self.key.public();
        match changes.get_mut(1) {
            Some(change) if matches!(&change.action, Action::Machine { machine, .. } if *machine == own) =>
            {
                *change = change.clone().claimed_by(&self.key);
            }
            _ => {
                return Err(
                    "the setup's second change must declare this machine, whose key is \
                     remote.machines' this.key"
                        .to_owned(),
                );
            }
        }
        let (device, session) = Device::setup(
            url,
            code,
            changes,
            &self.key,
            &crate::link::hello(),
            &Tls::System,
        )
        .await
        .map_err(|error| error.to_string())?;
        drop(session);
        self.keep(&device)
            .map_err(|error| format!("{} cannot be written: {error}", self.file().display()))?;
        eprintln!(
            "flottille daemon: this machine set up the hub at {}, whose key is {}",
            device.url,
            device.hub.fingerprint()
        );
        self.again.notify_one();
        Ok(json!({
            "url": device.url,
            "hub": device.hub.fingerprint(),
            "root": device.root.fingerprint(),
        }))
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

/// Keeps the session to the hub while the daemon runs: made again after each failure, waiting
/// longer each time, or at once after a setup.
pub async fn run(
    control: Arc<Control>,
    remote: Arc<Remote>,
    mut stopping: tokio::sync::watch::Receiver<bool>,
) {
    let mut wait = ROUND;
    // When the hub was last reached, to say since when it is not.
    let mut unreachable_since: Option<jiff::Zoned> = None;
    loop {
        let pinned = match control.pinned() {
            None => None,
            Some(Ok(device)) => Some(device),
            Some(Err(why)) => {
                control.set("refused", Some(why));
                None
            }
        };
        if let Some(device) = pinned {
            let authority = HubUrl::parse(&device.url)
                .map(|url| url.authority())
                .unwrap_or_else(|_| device.url.clone());
            let hello = crate::link::hello();
            let connected = tokio::select! {
                connected = device.connect(&control.key, &hello, &Tls::System) => connected,
                _ = stopped(&mut stopping) => return,
            };
            let why = match connected {
                Ok(session) => {
                    unreachable_since = None;
                    wait = ROUND;
                    control.set("connected", None);
                    match keep_session(&control, &remote, device, session, &mut stopping).await {
                        Ended::Stopping => return,
                        Ended::Again => continue,
                        Ended::Lost(why) => Some(why),
                    }
                }
                Err(ControlError::Reset) => {
                    control.set("reset", Some(ListError::Reset.to_string()));
                    None
                }
                Err(error @ (ControlError::AnotherKey { .. } | ControlError::Refused { .. })) => {
                    control.set("refused", Some(error.to_string()));
                    None
                }
                Err(ControlError::NeedsUpdate(refusal)) => {
                    control.set("needs an update", Some(refusal.to_string()));
                    None
                }
                Err(ControlError::Unreachable { why, .. } | ControlError::Lost { why, .. }) => {
                    Some(why)
                }
            };
            if let Some(why) = why {
                let since = unreachable_since.get_or_insert_with(jiff::Zoned::now);
                control.set(
                    "unreachable",
                    Some(format!(
                        "the hub at {authority} is unreachable since {}: {why}",
                        since.strftime("%H:%M")
                    )),
                );
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = control.again.notified() => {}
            _ = stopped(&mut stopping) => return,
        }
        wait = (wait * 2).min(LONGEST_WAIT);
    }
}

enum Ended {
    Stopping,
    /// A setup wrote another hub: start from it.
    Again,
    Lost(String),
}

/// One session, until it ends: the lists and addresses the hub sends, checked and kept; this
/// machine's addresses told whenever they change.
async fn keep_session(
    control: &Control,
    remote: &Arc<Remote>,
    mut device: Device,
    mut session: Session,
    stopping: &mut tokio::sync::watch::Receiver<bool>,
) -> Ended {
    let mut told: Option<Vec<String>> = None;
    let mut addresses: HashMap<[u8; 32], Vec<String>> = HashMap::new();
    let mut members: Option<Members> = device.members().ok();
    let mut round = tokio::time::interval(ROUND);
    let asker = session.asker();
    // The ask of this machine's addresses on its way, at most one: dropped with the session.
    let mut telling = tokio::task::JoinSet::new();
    let mut said: Option<String> = None;
    loop {
        tokio::select! {
            pushed = session.next() => {
                let Some(pushed) = pushed else {
                    return Ended::Lost(session.ended());
                };
                match pushed {
                    Pushed::List(list) => match device.take(list) {
                        Ok(checked) => {
                            if let Err(error) = control.keep(&device) {
                                eprintln!(
                                    "flottille daemon: the member list was not kept: {error}"
                                );
                            }
                            control.set("connected", None);
                            members = Some(checked);
                        }
                        Err(refused) => {
                            let state = match refused {
                                ListError::Reset => "reset",
                                ListError::Refused(_) => "refused",
                            };
                            control.set(state, Some(refused.to_string()));
                            continue;
                        }
                    },
                    Pushed::Addresses(given) => {
                        addresses = given
                            .into_iter()
                            .map(|told| (*told.key.as_bytes(), told.addresses))
                            .collect();
                    }
                    // Read by the notifications, later (`docs/remote.md`, "Notifications").
                    Pushed::Presence(_) => continue,
                }
                if let Some(members) = &members {
                    trust(remote, &control.key.public(), members, &addresses);
                }
            }
            _ = round.tick() => {
                let now = remote.told_addresses();
                if telling.is_empty() && told.as_ref() != Some(&now) {
                    // Answered later: the session goes on meanwhile.
                    let asker = asker.clone();
                    telling.spawn(async move {
                        let asked = asker.ask(Ask::Addresses { addresses: now.clone() });
                        let answer = match tokio::time::timeout(ASK_WAIT, asked).await {
                            Ok(answer) => answer.map_err(|error| error.to_string()),
                            Err(_) => Err(format!("it did not answer in {} s", ASK_WAIT.as_secs())),
                        };
                        (now, answer)
                    });
                }
            }
            Some(Ok((sent, answer))) = telling.join_next() => match answer {
                // Told only once the hub took them: a refusal is asked again at the next round.
                Ok(()) => {
                    told = Some(sent);
                    said = None;
                }
                Err(error) => {
                    if said.as_ref() != Some(&error) {
                        eprintln!("flottille daemon: the hub was not told this machine's addresses: {error}");
                    }
                    said = Some(error);
                }
            },
            _ = control.again.notified() => return Ended::Again,
            _ = stopped(stopping) => return Ended::Stopping,
        }
    }
}

/// The machines the link trusts: every member's machine but this one, named as the member, with
/// the addresses the hub told for it (those kept before, when the hub told none in this session).
fn trust(
    remote: &Remote,
    own: &PublicKey,
    members: &Members,
    addresses: &HashMap<[u8; 32], Vec<String>>,
) {
    if let Some(this) = members.by_machine(own) {
        remote.name_as(&this.name);
    }
    let machines = members
        .members()
        .iter()
        .filter_map(|member| {
            let machine = member.machine?;
            (machine != *own).then(|| {
                (
                    member.name.clone(),
                    machine,
                    addresses.get(machine.as_bytes()).cloned(),
                )
            })
        })
        .collect();
    remote.trust_machines(machines);
}

/// Until `stopping` says so; nothing held of it, so that a `select!` holding it stays `Send`.
async fn stopped(stopping: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = stopping.wait_for(|stop| *stop).await;
}
