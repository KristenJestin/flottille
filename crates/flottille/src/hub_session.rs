//! The daemon's control session to the user's hub (`docs/remote.md`, "The hub", "Architecture"),
//! with remote on and the machine paired: it keeps the member list, checked back to the root and
//! the hub's key pinned when it paired, and what it knows of their devices, and writes from them
//! the machines the link trusts (`remote/peers.json`); it tells the hub what this machine is.
//!
//! The hub adds the devices and changes their roles, signed with its key; a list a change of which
//! neither the hub nor a member that may make it signed is refused here, whatever the hub said, and
//! the machines trusted stay those of the last list that checked. While the hub is unreachable,
//! the links and the machines known go on; what needs the hub says so, with since when.
//!
//! What the machine pinned is `remote/hub.json` (`flottille_link::Device`): the hub's URL and
//! key, the list's root, and the list as last checked. A pairing writes it (`remote.pair` with the
//! code `flottille-hub pair` printed, then `remote.declare`), once the hub paired the device and
//! this machine is declared.
//!
//! The user's changes of the list (a removal, a name, an icon and a colour) are signed by the
//! clients with the device key, which the daemon never holds; the daemon carries them over its
//! session, and says what changed in the list it checked as events (`docs/events.md`, `remote.*`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_link::{
    Action, Ask, Asker, Change, ControlError, Device, Facts, Hello, HubUrl, ListError, MachineKey,
    MemberList, Members, PairCode, Paired, Proposed, PublicKey, Pushed, Role, Session, Tls,
};
use serde_json::{Value, json};

use crate::link::{Emit, Remote};

/// The machine's hub, as it pinned it.
pub const HUB_FILE: &str = "hub.json";
/// The longest a hub that cannot be reached waits before it is tried again.
const LONGEST_WAIT: Duration = Duration::from_secs(30);
/// How often the session looks whether this machine slept.
const ROUND: Duration = Duration::from_secs(1);
/// How long the hub may take to answer an ask before it is asked again.
const ASK_WAIT: Duration = Duration::from_secs(10);
/// How much further the wall clock may move than the monotonic one between two rounds before the
/// machine is taken to have slept: a time service steps the clock by less.
const ASLEEP: Duration = Duration::from_secs(5);

/// How the daemon sees its hub.
struct Seen {
    state: &'static str,
    /// Since when, in milliseconds since the Unix epoch.
    since: u64,
    reason: Option<String>,
    /// The hub answered that it no longer knows this machine: its list was reset, or it dropped
    /// this device. A client offers to pair again.
    forgotten: bool,
}

/// A pairing this machine made, its session to the hub open until its machine is declared.
struct Pairing {
    paired: Paired,
    /// The hub pinned and the list checked, once the hub paired the device.
    device: Device,
}

/// The control session's side of the daemon.
pub struct Control {
    key: MachineKey,
    folder: PathBuf,
    seen: Mutex<Seen>,
    /// Raised when `hub.json` was written by a setup: the session starts again from it.
    again: tokio::sync::Notify,
    /// What asks the hub in the session kept, while there is one.
    asker: tokio::sync::watch::Sender<Option<Asker>>,
    /// The list as last checked, for the changes a client asks to wait for it.
    list: tokio::sync::watch::Sender<MemberList>,
    /// What the hub last told of each member's device.
    facts: Mutex<Vec<Facts>>,
    pairing: tokio::sync::Mutex<Option<Pairing>>,
    emit: Emit,
}

impl Control {
    pub fn new(key: MachineKey, folder: PathBuf, emit: Emit) -> Control {
        Control {
            key,
            folder,
            seen: Mutex::new(Seen {
                state: "none",
                since: now(),
                reason: None,
                forgotten: false,
            }),
            again: tokio::sync::Notify::new(),
            asker: tokio::sync::watch::Sender::new(None),
            list: tokio::sync::watch::Sender::new(MemberList::default()),
            facts: Mutex::new(Vec::new()),
            pairing: tokio::sync::Mutex::new(None),
            emit,
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
        self.set_as(state, reason, false);
    }

    /// [`Control::set`], for a hub that no longer knows this machine: a client offers to pair
    /// again.
    fn forget(&self, state: &'static str, reason: String) {
        self.set_as(state, Some(reason), true);
    }

    fn set_as(&self, state: &'static str, reason: Option<String>, forgotten: bool) {
        let mut seen = self.seen.lock().unwrap();
        let changed = seen.state != state || seen.reason != reason || seen.forgotten != forgotten;
        if changed {
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
        seen.forgotten = forgotten;
        drop(seen);
        // The clients that watch read it again at once.
        if changed {
            (self.emit)("remote.hub", json!({"state": state}));
        }
    }

    /// The hub as `remote.machines` says it; `null` when this machine is not set up.
    pub fn seen(&self) -> Value {
        let Some(Ok(device)) = self.pinned() else {
            return Value::Null;
        };
        let seen = self.seen.lock().unwrap();
        json!({
            "url": HubUrl::shown_text(&device.url),
            "key": device.hub,
            "fingerprint": device.hub.fingerprint(),
            "state": seen.state,
            "since": seen.since,
            "reason": seen.reason,
            "forgotten": seen.forgotten,
        })
    }

    /// The hub this machine pinned, and the members of the list it last checked.
    pub(crate) fn checked(&self) -> Result<(Device, Members), String> {
        let device = match self.pinned() {
            None => return Err(NOT_SET_UP.to_owned()),
            Some(pinned) => pinned?,
        };
        let members = device.members().map_err(|error| error.to_string())?;
        Ok((device, members))
    }

    /// What asks the hub, once a session is up: a session made again a moment ago (after a
    /// pairing, or a failure) is waited for, a few seconds at most.
    async fn asker(&self) -> Result<Asker, String> {
        let mut watching = self.asker.subscribe();
        let waited = tokio::time::timeout(ASK_WAIT, watching.wait_for(Option::is_some)).await;
        if let Ok(Ok(asker)) = waited {
            return Ok(asker.clone().expect("waited for one"));
        }
        let seen = self.seen.lock().unwrap();
        Err(match &seen.reason {
            Some(reason) => format!(
                "The hub cannot be asked now: {}. Nothing was changed; try again once it answers.",
                reason.trim_end_matches('.')
            ),
            None => "The hub cannot be asked now: this machine has no session to it yet. Nothing \
                     was changed; try again in a moment."
                .to_owned(),
        })
    }

    /// `remote.members`: the members of the list this machine last checked.
    pub fn members(&self) -> Result<Value, String> {
        let (device, members) = self.checked()?;
        let facts = self.facts.lock().unwrap().clone();
        Ok(members_json(&device, &members, &self.key.public(), &facts))
    }

    /// `remote.change`: a member's signed change of the list (a removal, a name, an icon and a
    /// colour), carried to the hub; answered once the hub sent back a list that holds it and that
    /// this machine checked.
    pub async fn change(&self, change: Change) -> Result<Value, String> {
        let hash = change.hash();
        let mut watching = self.list.subscribe();
        tokio::time::timeout(ASK_WAIT, self.asker().await?.ask(Ask::Submit { change }))
            .await
            .map_err(|_| {
                format!(
                    "The hub did not answer in {} s. Nothing was changed; try again once it \
                     answers.",
                    ASK_WAIT.as_secs()
                )
            })?
            .map_err(|error| error.to_string())?;
        let holds = |list: &MemberList| list.changes().iter().any(|change| change.hash() == hash);
        tokio::time::timeout(ASK_WAIT, watching.wait_for(holds))
            .await
            .map_err(|_| {
                format!(
                    "The hub did not confirm this change in {} s: it may be busy, or it refused \
                     it. Nothing was changed; try again once the hub answers.",
                    ASK_WAIT.as_secs()
                )
            })?
            .map_err(|_| {
                "The connection to the hub closed before it confirmed this change. Nothing was \
                 changed; try again."
                    .to_owned()
            })?;
        self.members()
    }

    /// `remote.pair`: this machine pairs its device with the hub of `code` (what `flottille-hub
    /// pair` printed), proposing `proposed`: it reaches the hub, pinning the key the code holds,
    /// proves it holds the code's secret, and answers once the hub paired the device, with the
    /// list's head after which the device declares this machine (`remote.declare`). A machine
    /// paired already pairs again only when asked to `replace` the hub it pinned.
    pub async fn pair(
        &self,
        code: &str,
        proposed: Proposed,
        replace: bool,
    ) -> Result<Value, String> {
        if !replace && let Some(pinned) = self.pinned() {
            let url = pinned.map_or_else(
                |_| self.file().display().to_string(),
                |device| HubUrl::shown_text(&device.url),
            );
            return Err(format!(
                "this machine is paired with the hub at {url} already: give --replace to pair it \
                 with another (the machines it trusts through that hub are forgotten)"
            ));
        }
        let code = PairCode::from_code(code).map_err(|error| error.to_string())?;
        let mut pairing = self.pairing.lock().await;
        // A pairing asked again replaces the one before, whose session ends.
        *pairing = None;
        let key = proposed.key;
        let mut paired = Device::pair(
            &code,
            proposed,
            &self.key,
            &Hello::ours(crate::VERSION),
            &Tls::System,
        )
        .await
        .map_err(|error| error.to_string())?;
        let (device, members) = paired.listed().await.map_err(|error| error.to_string())?;
        eprintln!(
            "flottille daemon: paired with the hub at {}, whose key is {}",
            HubUrl::shown_text(&device.url),
            device.hub.fingerprint()
        );
        let answer = paired_json(&device, &members, &key)
            .expect("the list checked holds the device's entry");
        *pairing = Some(Pairing { paired, device });
        Ok(answer)
    }

    /// `remote.declare`: the declaration of this machine by the device that paired, signed by it
    /// after the head `remote.pair` answered; claimed here with the machine's key and carried to
    /// the hub. Once the hub sent it back, the hub is pinned (`hub.json`) and the session kept.
    pub async fn declare(&self, change: Change) -> Result<Value, String> {
        let mut pairing = self.pairing.lock().await;
        let Some(Pairing { paired, device }) = pairing.as_mut() else {
            return Err("this machine has no pairing going: run flottille remote pair".to_owned());
        };
        let own = self.key.public();
        let declares = matches!(&change.action, Action::Machine { machine, .. } if *machine == own);
        if !declares {
            return Err(
                "the change must declare this machine, whose key is remote.machines' this.key"
                    .to_owned(),
            );
        }
        // Declared once: a device paired again whose machine is declared already declares nothing.
        let declared = device
            .members()
            .ok()
            .and_then(|members| members.by_machine(&own).map(|member| member.key))
            == Some(change.signer);
        let members = if declared {
            device.members().map_err(|error| error.to_string())?
        } else {
            let submitted = device
                .submit(&mut paired.session, change.claimed_by(&self.key))
                .await;
            match submitted {
                Ok(members) => members,
                Err(error) => {
                    // A dead session cannot take a declaration: the pairing is let go.
                    if paired.session.has_ended() {
                        *pairing = None;
                    }
                    return Err(error.to_string());
                }
            }
        };
        self.keep(device)
            .map_err(|error| format!("{} cannot be written: {error}", self.file().display()))?;
        let device = device.clone();
        *pairing = None;
        drop(pairing);
        let _ = self.list.send(device.list.clone());
        let this = members.by_machine(&own).cloned();
        if let Some(this) = &this {
            (self.emit)(
                "remote.member_added",
                added_json(&this.name, this.role, &this.key, &members, &this.added_by),
            );
        }
        eprintln!(
            "flottille daemon: this machine is paired with the hub at {}",
            HubUrl::shown_text(&device.url)
        );
        self.again.notify_one();
        Ok(json!({
            "url": HubUrl::shown_text(&device.url),
            "hub": device.hub.fingerprint(),
            "root": device.root.fingerprint(),
            "name": this.as_ref().map(|this| this.name.clone()),
        }))
    }

    /// A list the session took: kept for those who wait for it, and what changed since `before`
    /// said as events.
    fn took(&self, before: Option<&Members>, list: &MemberList) {
        let _ = self.list.send(list.clone());
        let Some(before) = before else {
            return;
        };
        let known = before.head();
        let Some(from) = list
            .changes()
            .iter()
            .position(|change| change.previous == known)
        else {
            return;
        };
        // The names as the list moves on, to say who signed each change.
        let mut names: HashMap<[u8; 32], String> = before
            .members()
            .iter()
            .map(|member| (*member.key.as_bytes(), member.name.clone()))
            .collect();
        names.insert(*before.hub().as_bytes(), HUB.to_owned());
        for change in &list.changes()[from..] {
            let by = names
                .get(change.signer.as_bytes())
                .cloned()
                .unwrap_or_else(|| change.signer.fingerprint());
            match &change.action {
                Action::Add { key, name, role }
                | Action::Pair {
                    key, name, role, ..
                } => {
                    names.insert(*key.as_bytes(), name.clone());
                    (self.emit)(
                        "remote.member_added",
                        json!({
                            "name": name,
                            "fingerprint": key.fingerprint(),
                            "role": role,
                            "added_by": by,
                            "at": now(),
                        }),
                    );
                }
                Action::Remove { key } => {
                    let name = names
                        .remove(key.as_bytes())
                        .unwrap_or_else(|| key.fingerprint());
                    (self.emit)(
                        "remote.member_removed",
                        json!({
                            "name": name,
                            "fingerprint": key.fingerprint(),
                            "removed_by": by,
                            "at": now(),
                        }),
                    );
                }
                Action::Rename { key, name } => {
                    let from = names
                        .insert(*key.as_bytes(), name.clone())
                        .unwrap_or_else(|| key.fingerprint());
                    (self.emit)(
                        "remote.member_renamed",
                        json!({
                            "name": name,
                            "from": from,
                            "fingerprint": key.fingerprint(),
                            "renamed_by": by,
                            "at": now(),
                        }),
                    );
                }
                Action::Role { key, role } => {
                    let name = names
                        .get(key.as_bytes())
                        .cloned()
                        .unwrap_or_else(|| key.fingerprint());
                    (self.emit)(
                        "remote.member_role",
                        json!({
                            "name": name,
                            "fingerprint": key.fingerprint(),
                            "role": role,
                            "at": now(),
                        }),
                    );
                }
                Action::Machine { .. } | Action::Hub { .. } | Action::Look { .. } => {}
            }
        }
    }

    /// What the hub told of the members' devices: kept for `remote.members`.
    fn told_facts(&self, facts: Vec<Facts>) {
        *self.facts.lock().unwrap() = facts;
    }
}

const NOT_SET_UP: &str = "this machine is paired with no hub: flottille remote pair, with the code \
                          flottille-hub pair prints on the hub's host";

/// How a change the hub signed names its signer.
const HUB: &str = "the hub";

/// The name of the key `key`: a member's, the hub's, or its fingerprint.
fn name_of(members: &Members, key: &PublicKey) -> String {
    if *key == members.hub() {
        return HUB.to_owned();
    }
    members
        .get(key)
        .map_or_else(|| key.fingerprint(), |member| member.name.clone())
}

/// What `remote.pair` answers once the device `key` was paired; `None` when the list does not
/// hold it.
fn paired_json(device: &Device, members: &Members, key: &PublicKey) -> Option<Value> {
    let entry = members.get(key)?;
    Some(json!({
        "head": device.list.head(),
        "name": entry.name,
        "role": entry.role,
        "added_by": name_of(members, &entry.added_by),
        "hub": device.hub.fingerprint(),
        "url": HubUrl::shown_text(&device.url),
    }))
}

/// What is said of a member added.
fn added_json(name: &str, role: Role, key: &PublicKey, members: &Members, by: &PublicKey) -> Value {
    let by = name_of(members, by);
    json!({
        "name": name,
        "fingerprint": key.fingerprint(),
        "role": role,
        "added_by": by,
        "at": now(),
    })
}

/// What this machine proposes for itself when it pairs (`docs/remote.md`, "Pairing"): its host's
/// name, an icon by what it is (a battery: a laptop; a screen connected: a desktop; neither: a
/// server), and its role (`role_proposed`). The user changes them before confirming.
pub(crate) fn proposed(daemon: &crate::daemon::Daemon) -> Value {
    let any = |folder: &str, holds: &dyn Fn(&std::path::Path) -> bool| {
        std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| holds(&entry.path()))
    };
    let battery = any("/sys/class/power_supply", &|path| {
        std::fs::read_to_string(path.join("type")).is_ok_and(|kind| kind.trim() == "Battery")
    });
    let screen = any("/sys/class/drm", &|path| {
        std::fs::read_to_string(path.join("status"))
            .is_ok_and(|status| status.trim() == "connected")
    });
    let icon = match (battery, screen) {
        (true, _) => flottille_link::Icon::Laptop,
        (false, true) => flottille_link::Icon::Desktop,
        (false, false) => flottille_link::Icon::Server,
    };
    let role = role_proposed(icon, !crate::link::offered(daemon).agents.is_empty());
    json!({"name": host_name(), "icon": icon, "role": role})
}

/// The role proposed for a machine of this icon, with agents signed in or not: a machine with
/// no screen is where agents run, and they are signed in later, so it is a `runner` either way; a
/// desktop or a laptop is one with agents signed in, a `controller` without.
fn role_proposed(icon: flottille_link::Icon, agents: bool) -> Role {
    if agents || icon == flottille_link::Icon::Server {
        Role::Runner
    } else {
        Role::Controller
    }
}

/// The host's name, as a device's name is proposed; `None` when it has none.
fn host_name() -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is as long as said; gethostname writes within it.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    let name = String::from_utf8_lossy(&buffer[..end]).into_owned();
    (!name.is_empty()).then_some(name)
}

/// The members of a checked list, as `remote.members` says them.
fn members_json(device: &Device, members: &Members, own: &PublicKey, facts: &[Facts]) -> Value {
    json!({
        "hub": HubUrl::shown_text(&device.url),
        "root": name_of(members, &members.root()),
        "head": members.head(),
        "this": members.by_machine(own).map(|member| member.name.clone()),
        "members": members.members().iter().map(|member| {
            let facts = facts.iter().find(|facts| facts.key == member.key);
            json!({
                "name": member.name,
                "key": member.key,
                "fingerprint": member.key.fingerprint(),
                "role": member.role,
                "icon": member.icon,
                "colour": member.colour,
                "added_by": name_of(members, &member.added_by),
                "machine": member.machine.map(|machine| machine.fingerprint()),
                "kind": facts.and_then(|facts| facts.kind.clone()),
                "system": facts.and_then(|facts| facts.system.clone()),
                "version": facts.and_then(|facts| facts.version.clone()),
                "paired_at": facts.and_then(|facts| facts.paired_at),
                "last_seen": facts.and_then(|facts| facts.last_seen),
            })
        }).collect::<Vec<_>>(),
    })
}

/// The wall clock in ms: a test build's, which a test may set ahead (`link::wall_now`).
fn now() -> u64 {
    crate::link::wall_now()
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
    // Whether a session to the hub was had and lost since the daemon started.
    let mut lost = false;
    loop {
        let pinned = match control.pinned() {
            None => None,
            Some(Ok(device)) => Some(device),
            Some(Err(why)) => {
                control.set("refused", Some(why));
                None
            }
        };
        if pinned.is_none() {
            remote.heard_hub();
        }
        if let Some(device) = pinned {
            let authority = HubUrl::parse(&device.url)
                .map(|url| url.host_shown())
                .unwrap_or_else(|_| device.url.clone());
            // This build's own: a test's hello (`FLOTTILLE_TEST_LINK_HELLO`) is its links' only, so
            // that a machine of another protocol still reaches the hub, which its links go through.
            let hello = Hello::ours(crate::VERSION);
            let connected = tokio::select! {
                connected = device.connect(&control.key, &hello, &Tls::System) => connected,
                _ = stopped(&mut stopping) => return,
            };
            let why = match connected {
                Ok(session) => {
                    unreachable_since = None;
                    wait = ROUND;
                    control.set("connected", None);
                    let ended =
                        keep_session(&control, &remote, device, session, &mut stopping).await;
                    // Held until the next session's list, `FIRST_LIST` at most: a machine that
                    // was asleep during a removal does not link by the list it kept meanwhile.
                    remote.lost_hub();
                    lost = true;
                    match ended {
                        Ended::Stopping => return,
                        Ended::Again => continue,
                        Ended::Lost(why) => Some(why),
                    }
                }
                Err(ControlError::Reset) => {
                    control.forget("reset", ListError::Reset.to_string());
                    None
                }
                Err(error @ ControlError::NotMember { .. }) => {
                    control.forget("refused", error.to_string());
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
                // Said of a change, never of a connection.
                Err(error @ ControlError::Unconfirmed) => Some(error.to_string()),
            };
            // No list from the hub at start: the links go on with the one kept. After a session
            // was lost, they wait for the bound.
            if !lost {
                remote.heard_hub();
            }
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

/// Tells a machine that slept: on Linux the monotonic clock does not count a suspend, and the wall
/// clock does. A session to the hub may look alive for minutes after one, with no heartbeat to say
/// otherwise, while the list it kept missed what changed meanwhile.
#[derive(Clone, Copy)]
pub(crate) struct Awake {
    wall: std::time::SystemTime,
    steady: std::time::Instant,
}

impl Awake {
    pub(crate) fn new(wall: std::time::SystemTime, steady: std::time::Instant) -> Awake {
        Awake { wall, steady }
    }

    /// The clocks now.
    pub(crate) fn now() -> Awake {
        Awake::new(crate::link::wall_now(), std::time::Instant::now())
    }

    /// Whether the machine slept between these clocks and `later`.
    pub(crate) fn slept_until(&self, later: &Awake) -> bool {
        // A wall clock set back moved by nothing.
        let walked = later.wall.duration_since(self.wall).unwrap_or_default();
        let ran = later.steady.saturating_duration_since(self.steady);
        walked > ran + ASLEEP
    }

    /// Whether the machine slept since the last round, at `wall` and `steady` now.
    fn slept(&mut self, wall: std::time::SystemTime, steady: std::time::Instant) -> bool {
        let now = Awake { wall, steady };
        let slept = self.slept_until(&now);
        *self = now;
        slept
    }
}

enum Ended {
    Stopping,
    /// A pairing wrote another hub: start from it.
    Again,
    Lost(String),
}

/// One session, until it ends: the lists the hub sends, checked and kept.
async fn keep_session(
    control: &Control,
    remote: &Arc<Remote>,
    mut device: Device,
    mut session: Session,
    stopping: &mut tokio::sync::watch::Receiver<bool>,
) -> Ended {
    let mut members: Option<Members> = device.members().ok();
    let mut round = tokio::time::interval(ROUND);
    let asker = session.asker();
    control.asker.send_replace(Some(asker.clone()));
    let _ = control.list.send(device.list.clone());
    // The asker is let go with the session, however it ends.
    struct Forget<'a>(&'a Control);
    impl Drop for Forget<'_> {
        fn drop(&mut self) {
            self.0.asker.send_replace(None);
        }
    }
    let _forget = Forget(control);
    // The asks on their way: dropped with the session.
    let mut telling = tokio::task::JoinSet::new();
    // The links to the other machines go through this session while it lasts.
    if let Some(port) = session.relay() {
        remote.through(port);
    }
    let mut awake = Awake::now();
    remote.awake_at(awake);
    // What this machine is, told once a session: the hub keeps it on the device's card.
    let about = asker.clone();
    telling.spawn(async move {
        let asked = about.ask(Ask::About {
            device: "computer".to_owned(),
            system: std::env::consts::OS.to_owned(),
        });
        let _ = tokio::time::timeout(ASK_WAIT, asked).await;
    });
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
                            control.took(members.as_ref(), &device.list);
                            members = Some(checked);
                        }
                        Err(refused) => {
                            let state = match refused {
                                ListError::Reset => "reset",
                                ListError::Refused(_) => "refused",
                            };
                            control.set(state, Some(refused.to_string()));
                            remote.heard_hub();
                            continue;
                        }
                    },
                    // Read by the notifications, later (`docs/remote.md`, "Notifications").
                    Pushed::Presence(_) => continue,
                    Pushed::Facts(facts) => {
                        control.told_facts(facts);
                        continue;
                    }
                }
                if let Some(members) = &members {
                    trust(remote, &control.key.public(), members);
                    remote.heard_hub();
                }
            }
            _ = round.tick() => {
                let now = Awake::now();
                remote.awake_at(now);
                if awake.slept(now.wall, now.steady) {
                    // Ended as lost: the links wait for the next session's list.
                    return Ended::Lost("this machine was asleep".to_owned());
                }
            }
            Some(_) = telling.join_next() => {}
            _ = control.again.notified() => return Ended::Again,
            _ = stopped(stopping) => return Ended::Stopping,
        }
    }
}

/// The machines the link trusts: every member's machine but this one, named as the member.
fn trust(remote: &Remote, own: &PublicKey, members: &Members) {
    if let Some(this) = members.by_machine(own) {
        remote.name_as(&this.name);
    }
    let machines = members
        .members()
        .iter()
        .filter_map(|member| {
            let machine = member.machine?;
            (machine != *own).then(|| (member.name.clone(), machine))
        })
        .collect();
    remote.trust_machines(machines);
}

/// Until `stopping` says so; nothing held of it, so that a `select!` holding it stays `Send`.
async fn stopped(stopping: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = stopping.wait_for(|stop| *stop).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_without_a_screen_is_proposed_as_a_runner_without_agents() {
        // Agents are signed in later: a server is where they run, signed in or not.
        assert_eq!(
            role_proposed(flottille_link::Icon::Server, false),
            Role::Runner
        );
        assert_eq!(
            role_proposed(flottille_link::Icon::Server, true),
            Role::Runner
        );
    }

    #[test]
    fn a_desktop_without_agents_is_proposed_as_a_controller() {
        for icon in [flottille_link::Icon::Desktop, flottille_link::Icon::Laptop] {
            assert_eq!(role_proposed(icon, false), Role::Controller);
            assert_eq!(role_proposed(icon, true), Role::Runner);
        }
    }

    #[test]
    fn a_machine_that_wakes_from_a_suspend_waits_for_the_next_list() {
        let (wall, steady) = (std::time::SystemTime::now(), std::time::Instant::now());
        let at = |seconds: u64, ran: u64| {
            (
                wall + Duration::from_secs(seconds),
                steady + Duration::from_secs(ran),
            )
        };
        let mut awake = Awake::new(wall, steady);
        // Rounds a second apart, or late on a loaded machine: both clocks move alike.
        let (now, ran) = at(1, 1);
        assert!(!awake.slept(now, ran));
        let (now, ran) = at(31, 31);
        assert!(!awake.slept(now, ran));
        // The wall clock set back, or a little ahead, as a time service does.
        let (now, ran) = at(30, 32);
        assert!(!awake.slept(now, ran));
        let (now, ran) = at(35, 33);
        assert!(!awake.slept(now, ran));
        // A suspend: an hour on the wall clock, a second on the one that does not count it. The
        // session is then ended as lost, so that the links wait for the next list.
        let (now, ran) = at(3635, 34);
        assert!(awake.slept(now, ran));
        // And once awake, the rounds go on.
        let (now, ran) = at(3636, 35);
        assert!(!awake.slept(now, ran));
    }
}
