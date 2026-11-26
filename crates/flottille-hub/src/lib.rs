//! The hub's state and its rules (`docs/remote.md`, "The hub", "Pairing"), as a library its server
//! (`server.rs`) calls: its key, the member list it stores, its pair mode, and what it knows of the
//! members' devices and their presence.
//!
//! The hub adds the devices: in pair mode (`open_pairing`, from `flottille-hub pair` on its host), a
//! device that proves it holds the pair code's secret is paired, its entry signed with the hub's
//! key (`Action::Pair`); a role is changed and a device removed the same way from the hub's host
//! (`flottille-hub role`, `flottille-hub remove`). A member's change (a removal, a name, an icon and
//! a colour, the declaration of its machine) is checked by `flottille-link`'s list check against
//! the list the hub holds before it is stored; a member adds no device.
//!
//! On disk, its state folder (700) holds `hub.key` (600, `flottille-link`'s `HubKey`), `hub.lock`,
//! which one process holds while it uses the folder, `hub.sock`, where `flottille-hub pair` and
//! the other commands of the hub's host reach the hub that runs (`server.rs`), and `hub.json`
//! (600), written whole under a temporary name and renamed over the old one:
//!
//! ```json
//! {"version": 1, "members": [/* the list's changes */], "devices": [/* `Facts` */],
//!  "shared": [/* `Sealed`: the values of all machines, opaque */]}
//! ```
//!
//! A `hub.json` of before pair mode may hold a `setup_code`: it is read, and dropped at the next
//! write. The pair mode, the asks taken and the presence are kept in memory: they last minutes, or
//! are told again when a device connects.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use flottille_link::{
    Action, Change, Facts, HubKey, ListError, MOST_VALUES, Member, MemberList, Members, PairCode,
    PairSecret, Proposed, PublicKey, REQUEST_WINDOW, Role, Sealed,
};

mod qr;
mod relay;
mod server;

pub use qr::qr;
pub use relay::{RELAY_BURST, RELAY_QUEUE, RELAY_RATE};
use serde::{Deserialize, Serialize};
pub use server::{
    KEPT_FOR_MEMBERS, LISTEN, MOST_ARRIVING, PAIR_IDLE, SOCKET_FILE, Served, Waits, serve,
    serve_with, tls_server,
};

/// How long a pair mode lasts unless told otherwise, and the longest it may last, in minutes.
pub const PAIR_MINUTES: u64 = 5;
pub const LONGEST_PAIR_MINUTES: u64 = 15;

/// The most changes of the list one member makes within an hour: a member's device, a viewer's
/// too, cannot fill the list (`flottille_link::MOST_CHANGES`) in an afternoon.
pub const CHANGES_AN_HOUR: usize = 30;
const AN_HOUR: u64 = 60 * 60;

/// The state's file in the hub's state folder.
pub const STATE_FILE: &str = "hub.json";

/// The file whose lock one process holds while it uses the state folder.
pub const LOCK_FILE: &str = "hub.lock";

const VERSION: u32 = 1;

/// The longest text a device says of itself (`Ask::About`).
const LONGEST_ABOUT: usize = 32;

/// The hub's time, in seconds since 1970: the system's, or one a test moves.
pub trait Clock {
    fn now(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or_default()
    }
}

/// Why the hub refuses something, or cannot start, in a sentence for the user.
#[derive(Debug)]
pub struct HubError(String);

impl fmt::Display for HubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HubError {}

fn refused(why: impl Into<String>) -> HubError {
    HubError(why.into())
}

/// Whether a client of a member is active, as it tells it (`docs/remote.md`, "Notifications").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presence {
    pub active: bool,
    /// The time of its last input, in seconds since 1970.
    pub last_input: u64,
}

/// A device paired, as `flottille-hub pair` says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PairedDevice {
    pub key: PublicKey,
    pub name: String,
    pub fingerprint: String,
    pub role: Role,
    /// It was a member already: nothing changed, its card is kept.
    pub again: bool,
}

/// How a pair mode ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairEnd {
    Paired(Box<PairedDevice>),
    /// Its minutes passed, or it was closed: why, in a sentence.
    Over(String),
}

/// The pair mode open: one at a time.
struct PairMode {
    /// Raised at each pair mode, so that a command waits for its own only.
    number: u64,
    secret: PairSecret,
    until: u64,
}

/// What `hub.json` holds.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    /// A hub of before pair mode kept its setup code here while it had no member: read, never
    /// written.
    #[serde(default, skip_serializing)]
    setup_code: Option<String>,
    members: MemberList,
    /// What the hub knows of each member's device, by its key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    devices: Vec<Facts>,
    /// The values of all machines, sealed by the machines for each other: the latest of each
    /// name, which the hub cannot read (`flottille_link::Sealed`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    shared: Vec<Sealed>,
}

pub struct Hub<C: Clock = SystemClock> {
    /// Held for the hub's life: no other process writes its state meanwhile.
    _lock: File,
    file: PathBuf,
    /// Shared with the sessions' handshakes, which sign with it while the hub is not locked.
    key: std::sync::Arc<HubKey>,
    clock: C,
    stored: Stored,
    /// The stored list, checked; `None` while it is empty.
    checked: Option<Members>,
    pairing: Option<PairMode>,
    /// How the last pair mode ended, with its number.
    ended: Option<(u64, PairEnd)>,
    pairings: u64,
    presence: Vec<(PublicKey, Presence)>,
    /// The changes stored within the last hour, by their signer, with their time.
    recent: Vec<(PublicKey, u64)>,
}

impl<C: Clock> Hub<C> {
    /// The hub of the state `folder`, made there the first time: its key, and an empty list. The
    /// folder is locked while the hub lives: another process using it is refused. A stored list
    /// that does not check is refused, and so is the start.
    pub fn open(folder: &Path, clock: C) -> Result<Hub<C>, HubError> {
        let key = HubKey::load_or_make(folder).map_err(|error| HubError(error.to_string()))?;
        let lock = lock(folder)?;
        let file = folder.join(STATE_FILE);
        let stored = read(&file)?.unwrap_or(Stored {
            version: VERSION,
            ..Stored::default()
        });
        let checked = match stored.members.root() {
            Some(root) => Some(stored.members.check(&root, &key.public()).map_err(|error| {
                HubError(format!(
                    "the member list in {} does not check: {}; the hub stores only what it and \
                     the members signed, so the file was changed or damaged: run flottille-hub \
                     reset on this machine to start again",
                    file.display(),
                    why(error)
                ))
            })?),
            None => None,
        };
        Ok(Hub {
            _lock: lock,
            file,
            key: std::sync::Arc::new(key),
            clock,
            stored,
            checked,
            pairing: None,
            ended: None,
            pairings: 0,
            presence: Vec::new(),
            recent: Vec::new(),
        })
    }

    /// The hub's public key, which the devices pin when they pair.
    pub fn key(&self) -> PublicKey {
        self.key.public()
    }

    /// The member list, as the hub and the members signed it.
    pub fn list(&self) -> &MemberList {
        &self.stored.members
    }

    /// The members of the stored list, checked; `None` while it is empty.
    pub fn members(&self) -> Option<&Members> {
        self.checked.as_ref()
    }

    /// Whether a device that proved `key`, and said it pinned the list whose first change's hash
    /// is `pinned`, may keep a session: a member's device or the machine a member declared. A
    /// refusal says why, and whether the list the device pinned was reset; nothing else of the
    /// list.
    pub fn welcome(
        &self,
        key: &PublicKey,
        pinned: Option<flottille_link::ChangeHash>,
    ) -> Result<(), Box<(String, bool)>> {
        let first = self.stored.members.changes().first().map(Change::hash);
        let reset = pinned.is_some_and(|pinned| Some(pinned) != first);
        if self
            .checked
            .as_ref()
            .is_some_and(|members| members.get(key).is_some() || members.by_machine(key).is_some())
        {
            return Ok(());
        }
        let reason = if reset {
            "this hub's member list was reset".to_owned()
        } else {
            format!(
                "{} is not one of this hub's members, nor a member's machine: pair it with \
                 flottille-hub pair on the hub's host",
                key.fingerprint()
            )
        };
        Err(Box::new((reason, reset)))
    }

    /// Opens pair mode for `minutes` (`PAIR_MINUTES` when `None`, `LONGEST_PAIR_MINUTES` at most):
    /// the code to print, which holds `url`, the hub's key and a new secret, valid once. One pair
    /// mode at a time: another one open is said.
    pub fn open_pairing(
        &mut self,
        url: &str,
        minutes: Option<u64>,
    ) -> Result<(u64, PairCode), HubError> {
        let now = self.sweep();
        if let Some(open) = &self.pairing {
            return Err(refused(format!(
                "pair mode is open already, {} s more: pair a device with its code, or stop the \
                 flottille-hub pair that opened it",
                open.until.saturating_sub(now)
            )));
        }
        let minutes = minutes.unwrap_or(PAIR_MINUTES);
        if !(1..=LONGEST_PAIR_MINUTES).contains(&minutes) {
            return Err(refused(format!(
                "pair mode lasts 1 to {LONGEST_PAIR_MINUTES} minutes"
            )));
        }
        let code = PairCode {
            hub_url: url.to_owned(),
            hub_key: self.key(),
            secret: PairSecret::new().map_err(HubError)?,
        };
        // Refused here when the URL is not one: before anything is open.
        code.code().map_err(|error| refused(error.to_string()))?;
        self.pairings += 1;
        self.pairing = Some(PairMode {
            number: self.pairings,
            secret: code.secret,
            until: now + minutes * 60,
        });
        Ok((self.pairings, code))
    }

    /// Closes the pair mode `number`, if it is still open: its command stopped.
    pub fn close_pairing(&mut self, number: u64) {
        if self
            .pairing
            .as_ref()
            .is_some_and(|open| open.number == number)
        {
            self.end_pairing(PairEnd::Over("pair mode was closed".to_owned()));
        }
    }

    /// How the pair mode `number` ended; `None` while it is open. Its minutes passed end it here.
    pub fn pairing_ended(&mut self, number: u64) -> Option<PairEnd> {
        self.sweep();
        match &self.ended {
            Some((ended, end)) if *ended == number => Some(end.clone()),
            _ if self
                .pairing
                .as_ref()
                .is_some_and(|open| open.number == number) =>
            {
                None
            }
            _ => Some(PairEnd::Over("pair mode was closed".to_owned())),
        }
    }

    /// Seconds left to the pair mode open; `None` when none is.
    pub fn pairing_left(&self) -> Option<u64> {
        let now = self.clock.now();
        self.pairing
            .as_ref()
            .map(|open| open.until.saturating_sub(now))
    }

    /// A device pairs: `proves` says whether its proof is good for a secret, the pair mode's. Once
    /// the proof checks, pair mode ends: the device proposed is paired, its entry signed with the
    /// hub's key, as it proposed it; one that is a member already keeps its entry and its card.
    /// A wrong proof is refused and leaves pair mode as it was: anyone who reaches the hub can
    /// send one with a key of their own, and closing pair mode on them would let a stranger keep
    /// the user from ever pairing. The secret holds 128 bits, so the tries protect nothing; the
    /// session says whose they are (its log names the key) and ends.
    pub fn pair(
        &mut self,
        proposed: &Proposed,
        proves: impl Fn(&PairSecret) -> bool,
        version: &str,
    ) -> Result<PairedDevice, HubError> {
        let now = self.sweep();
        let Some(open) = &self.pairing else {
            return Err(refused(
                "pair mode is not open on this hub, or this code's minutes passed, or it served \
                 already: run flottille-hub pair on the hub's host for a new code",
            ));
        };
        if !proves(&open.secret) {
            return Err(refused(
                "this device did not prove it holds the pair code: copy the code again, whole, \
                 from what flottille-hub pair printed",
            ));
        }
        proposed.check().map_err(refused)?;
        let again = self
            .checked
            .as_ref()
            .and_then(|members| members.get(&proposed.key))
            .cloned();
        let paired = match again {
            Some(member) => PairedDevice {
                key: member.key,
                fingerprint: member.key.fingerprint(),
                name: member.name,
                role: member.role,
                again: true,
            },
            None => {
                let mut list = match self.stored.members.root() {
                    Some(_) => self.stored.members.clone(),
                    None => MemberList::start_hub(&self.key)
                        .map_err(|error| HubError(error.to_string()))?,
                };
                list.sign_as_hub(
                    Action::Pair {
                        key: proposed.key,
                        name: proposed.name.clone(),
                        role: proposed.role,
                        icon: proposed.icon,
                        colour: proposed.colour,
                    },
                    &self.key,
                );
                let pairing_end =
                    |error: HubError| HubError(format!("this device cannot be paired: {error}"));
                self.store_list(list).map_err(pairing_end)?;
                PairedDevice {
                    key: proposed.key,
                    name: proposed.name.clone(),
                    fingerprint: proposed.key.fingerprint(),
                    role: proposed.role,
                    again: false,
                }
            }
        };
        // The device is paired once its entry is stored: pair mode ends before the facts are
        // written, so that a disk that refuses them does not leave the code to pair again.
        self.end_pairing(PairEnd::Paired(Box::new(paired.clone())));
        let facts = self.facts_mut(&paired.key);
        facts.paired_at = Some(now);
        facts.version = Some(version.to_owned());
        facts.last_seen = Some(now);
        self.write_facts()?;
        Ok(paired)
    }

    /// `flottille-hub role <device> <role>`: the hub changes the role of the member `device` (its
    /// name or its key's fingerprint), signed with its key.
    pub fn change_role(&mut self, device: &str, role: Role) -> Result<Member, HubError> {
        let member = self.named(device)?;
        if member.role == role {
            return Err(refused(format!(
                "{} is a {} already",
                member.name,
                role.word()
            )));
        }
        let mut list = self.stored.members.clone();
        list.sign_as_hub(
            Action::Role {
                key: member.key,
                role,
            },
            &self.key,
        );
        self.store_list(list)?;
        Ok(Member { role, ..member })
    }

    /// `flottille-hub remove <device>`: the hub removes the member `device`, signed with its key.
    pub fn remove(&mut self, device: &str) -> Result<Member, HubError> {
        let member = self.named(device)?;
        let mut list = self.stored.members.clone();
        list.sign_as_hub(Action::Remove { key: member.key }, &self.key);
        self.store_list(list)?;
        Ok(member)
    }

    /// The member named `device`, or whose key's fingerprint it is.
    fn named(&self, device: &str) -> Result<Member, HubError> {
        self.checked
            .as_ref()
            .and_then(|members| {
                members
                    .members()
                    .iter()
                    .find(|member| member.name == device || member.key.fingerprint() == device)
            })
            .cloned()
            .ok_or_else(|| {
                refused(format!(
                    "{device} is no member's name nor fingerprint: flottille-hub devices lists them"
                ))
            })
    }

    /// A member's change of the list, stored once it checks after the list the hub holds. A member
    /// adds no device, and makes at most `CHANGES_AN_HOUR` changes within an hour.
    pub fn submit(&mut self, change: Change) -> Result<(), HubError> {
        let now = self.sweep();
        if self.stored.members.root().is_none() {
            return Err(refused(
                "the hub has no member yet: pair a first device with flottille-hub pair",
            ));
        }
        if change.signer == self.key() {
            return Err(refused(
                "the hub refuses this change: it is signed with the hub's own key, which signs on \
                 the hub's host only",
            ));
        }
        if matches!(change.action, Action::Add { .. }) {
            return Err(refused(
                "the hub refuses this change: a device is paired on the hub (flottille-hub pair), \
                 no member adds one",
            ));
        }
        let made = self
            .recent
            .iter()
            .filter(|(signer, _)| *signer == change.signer)
            .count();
        if made >= CHANGES_AN_HOUR {
            return Err(refused(format!(
                "{} made {CHANGES_AN_HOUR} changes of the list within an hour: try again later",
                change.signer.fingerprint()
            )));
        }
        let signer = change.signer;
        let mut changes = self.stored.members.changes().to_vec();
        changes.push(change);
        self.store_list(MemberList::from_changes(changes))?;
        self.recent.push((signer, now));
        Ok(())
    }

    /// What a member's device says of itself: what it is and its system.
    pub fn about(
        &mut self,
        member: &PublicKey,
        device: &str,
        system: &str,
    ) -> Result<(), HubError> {
        let member = self.member(member)?.key;
        let fine = |text: &str| {
            !text.is_empty()
                && text.len() <= LONGEST_ABOUT
                && text
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        if !fine(device) || !fine(system) {
            return Err(refused(format!(
                "what a device says of itself is a word of at most {LONGEST_ABOUT} letters, digits, \
                 hyphens, dots or underscores"
            )));
        }
        let facts = self.facts_mut(&member);
        if facts.kind.as_deref() == Some(device) && facts.system.as_deref() == Some(system) {
            return Ok(());
        }
        facts.kind = Some(device.to_owned());
        facts.system = Some(system.to_owned());
        self.write_facts()
    }

    /// A member's device connected, or left, saying `version`: when it was last seen.
    pub fn seen(&mut self, member: &PublicKey, version: &str) -> Result<(), HubError> {
        let member = self.member(member)?.key;
        let now = self.clock.now();
        let facts = self.facts_mut(&member);
        facts.last_seen = Some(now);
        facts.version = Some(version.chars().take(LONGEST_ABOUT).collect());
        self.write_facts()
    }

    /// A value of all machines sealed by a member's machine, kept when it is the latest of its
    /// name (a copy of the same time replaces it when it is sealed for a machine more: a machine
    /// sealed it again for a machine added since), of a known name (`SHARED_NAMES`) and no later
    /// than the hub's clock allows. The hub reads its name, its time and its signature, never the
    /// value.
    pub fn share(&mut self, sealed: Sealed) -> Result<bool, HubError> {
        let members = self
            .checked
            .as_ref()
            .ok_or_else(|| refused("the hub has no member yet"))?;
        sealed.check_for_hub(members).map_err(refused)?;
        // A time past the hub's clock would hold the name for ever: a device's clock may be ahead
        // by the window a signed request is taken in, no more.
        let latest = self
            .clock
            .now()
            .saturating_mul(1000)
            .saturating_add(REQUEST_WINDOW.as_millis() as u64);
        if sealed.at > latest {
            return Err(refused(
                "this value is dated later than the hub's clock allows",
            ));
        }
        let shared = &mut self.stored.shared;
        match shared.iter().position(|kept| kept.name == sealed.name) {
            Some(at) if shared[at].at > sealed.at => return Ok(false),
            // A copy of the same time is the same value sealed again: kept only when it is
            // sealed for every machine the kept one is and one more, a machine paired since.
            // While two machines see different member lists, a copy that drops a machine would
            // swing between them before it settles.
            Some(at)
                if shared[at].at == sealed.at
                    && !(shared[at]
                        .recipients()
                        .all(|kept| sealed.recipients().any(|to| to == kept))
                        && sealed
                            .recipients()
                            .any(|to| shared[at].recipients().all(|kept| kept != to))) =>
            {
                return Ok(false);
            }
            Some(at) => shared[at] = sealed,
            None if shared.len() >= MOST_VALUES => {
                return Err(refused(format!(
                    "the hub keeps {MOST_VALUES} values of all machines at most"
                )));
            }
            None => shared.push(sealed),
        }
        self.stored.setup_code = None;
        write(&self.file, &self.stored)?;
        Ok(true)
    }

    /// The copy of `name` sealed at `at` dropped, as `machine` asks: it found it void, the device
    /// that signed the change inside having left the members or become a viewer. The hub, which
    /// cannot open it, takes a member's machine at its word, as it takes the copies it seals; a
    /// viewer's is refused. Whether a copy was dropped: none when the hub keeps another by then.
    pub fn unshare(&mut self, machine: &PublicKey, name: &str, at: u64) -> Result<bool, HubError> {
        let member = self
            .checked
            .as_ref()
            .and_then(|members| members.by_machine(machine))
            .ok_or_else(|| refused("only a member's machine drops a value of all machines"))?;
        if member.role == Role::Viewer {
            return Err(refused(format!("{} may only view", member.name)));
        }
        let shared = &mut self.stored.shared;
        let Some(kept) = shared
            .iter()
            .position(|kept| kept.name == name && kept.at == at)
        else {
            return Ok(false);
        };
        shared.remove(kept);
        self.stored.setup_code = None;
        write(&self.file, &self.stored)?;
        Ok(true)
    }

    /// The values of all machines, as the machines sealed them.
    pub fn shared(&self) -> &[Sealed] {
        &self.stored.shared
    }

    /// What the hub knows of each member's device.
    pub fn facts(&self) -> &[Facts] {
        &self.stored.devices
    }

    /// What a member's client tells of its presence, kept in memory.
    pub fn tell_presence(
        &mut self,
        member: &PublicKey,
        presence: Presence,
    ) -> Result<(), HubError> {
        self.member(member)?;
        keep(&mut self.presence, member, presence);
        Ok(())
    }

    pub fn presence(&self, member: &PublicKey) -> Option<Presence> {
        self.presence
            .iter()
            .find(|(key, _)| key == member)
            .map(|(_, presence)| *presence)
    }

    /// Checks `members` after the list the hub holds, writes it, then holds it: a list that does
    /// not check, or could not be written, is not taken.
    fn store_list(&mut self, members: MemberList) -> Result<(), HubError> {
        let root = members
            .root()
            .ok_or_else(|| refused("the hub refuses an empty list"))?;
        let checked = members.check(&root, &self.key()).map_err(refuse_change)?;
        // A member removed takes its presence and facts with it, and so does a machine
        // no member declares any more.
        let member =
            |key: &PublicKey| checked.get(key).is_some() || checked.by_machine(key).is_some();
        let stored = Stored {
            version: VERSION,
            setup_code: None,
            devices: self
                .stored
                .devices
                .iter()
                .filter(|facts| checked.get(&facts.key).is_some())
                .cloned()
                .collect(),
            shared: self.stored.shared.clone(),
            members,
        };
        write(&self.file, &stored)?;
        self.presence.retain(|(key, _)| member(key));
        self.stored = stored;
        self.checked = Some(checked);
        Ok(())
    }

    fn write_facts(&mut self) -> Result<(), HubError> {
        self.stored.setup_code = None;
        write(&self.file, &self.stored)
    }

    fn facts_mut(&mut self, key: &PublicKey) -> &mut Facts {
        let at = match self
            .stored
            .devices
            .iter()
            .position(|facts| facts.key == *key)
        {
            Some(at) => at,
            None => {
                self.stored.devices.push(Facts {
                    key: *key,
                    kind: None,
                    system: None,
                    version: None,
                    paired_at: None,
                    last_seen: None,
                });
                self.stored.devices.len() - 1
            }
        };
        &mut self.stored.devices[at]
    }

    fn end_pairing(&mut self, end: PairEnd) {
        if let Some(open) = self.pairing.take() {
            self.ended = Some((open.number, end));
        }
    }

    /// The member of `key`, a member's device key or the machine it declared.
    fn member(&self, key: &PublicKey) -> Result<&Member, HubError> {
        self.checked
            .as_ref()
            .and_then(|members| members.get(key).or_else(|| members.by_machine(key)))
            .ok_or_else(|| refused(format!("{} is not a member", key.fingerprint())))
    }

    /// Ends what is over: a pair mode whose minutes passed, the changes counted an hour. Returns
    /// the time.
    fn sweep(&mut self) -> u64 {
        let now = self.clock.now();
        if self.pairing.as_ref().is_some_and(|open| now >= open.until) {
            self.end_pairing(PairEnd::Over(
                "pair mode's minutes passed with no device paired".to_owned(),
            ));
        }
        self.recent.retain(|(_, at)| now < at + AN_HOUR);
        now
    }
}

/// `flottille-hub reset` (`docs/remote.md`, "Recovery"): empties the member list of the state
/// `folder`, and what the hub knew of the devices. It reads nothing of the state, so that a list
/// that does not check, or a damaged file, is no obstacle; the hub's key stays. The folder's lock
/// is taken: refused while a hub uses it, whose next write would undo the reset.
pub fn reset(folder: &Path) -> Result<(), HubError> {
    HubKey::load_or_make(folder).map_err(|error| HubError(error.to_string()))?;
    let _lock = lock(folder)?;
    let stored = Stored {
        version: VERSION,
        ..Stored::default()
    };
    write(&folder.join(STATE_FILE), &stored)
}

/// The lock of the state `folder`, taken without waiting: held until the file is closed.
fn lock(folder: &Path) -> Result<File, HubError> {
    let path = folder.join(LOCK_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|error| HubError(format!("{} cannot be opened: {error}", path.display())))?;
    // SAFETY: flock has no precondition on an open descriptor, which `file` owns.
    if unsafe {
        libc::flock(
            std::os::fd::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        return Err(HubError(
            if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                format!(
                    "another flottille-hub uses {}: stop it first",
                    folder.display()
                )
            } else {
                format!("{} cannot be locked: {error}", path.display())
            },
        ));
    }
    Ok(file)
}

impl<C: Clock> fmt::Debug for Hub<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hub")
            .field("key", &self.key)
            .field("changes", &self.stored.members.changes().len())
            .finish_non_exhaustive()
    }
}

fn why(error: ListError) -> String {
    match error {
        ListError::Refused(why) => why,
        ListError::Reset => "its root is not the list's first change".to_owned(),
    }
}

fn refuse_change(error: ListError) -> HubError {
    HubError(format!("the hub refuses this change: {}", why(error)))
}

fn keep<T>(entries: &mut Vec<(PublicKey, T)>, key: &PublicKey, value: T) {
    match entries.iter_mut().find(|(entry, _)| entry == key) {
        Some(entry) => entry.1 = value,
        None => entries.push((*key, value)),
    }
}

/// The state in `file`, `None` when there is none. A link, or a file others may read or write, is
/// refused: what is in it is the user's, and the hub trusts nothing else wrote it.
fn read(file: &Path) -> Result<Option<Stored>, HubError> {
    let shown = file.display();
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(file);
    let mut opened = match opened {
        Ok(opened) => opened,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(HubError(format!(
                "{shown} is a link: the hub's state is a file of its own; remove the link"
            )));
        }
        Err(error) => return Err(HubError(format!("{shown} cannot be read: {error}"))),
    };
    let metadata = opened
        .metadata()
        .map_err(|error| HubError(format!("{shown} cannot be read: {error}")))?;
    if !metadata.is_file() {
        return Err(HubError(format!("{shown} is not a file")));
    }
    // SAFETY: getuid has no precondition and cannot fail.
    if metadata.uid() != unsafe { libc::getuid() } {
        return Err(HubError(format!("{shown} is not owned by you")));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(HubError(format!(
            "{shown} is open to other users (mode {mode:o}): it is the hub's state; make it \
             yours only with `chmod 600 {shown}`"
        )));
    }
    let mut text = String::new();
    opened
        .read_to_string(&mut text)
        .map_err(|error| HubError(format!("{shown} cannot be read: {error}")))?;
    let stored: Stored = serde_json::from_str(&text)
        .map_err(|error| HubError(format!("{shown} is not the hub's state: {error}")))?;
    if stored.version != VERSION {
        return Err(HubError(format!(
            "{shown} is the state of another version of flottille-hub (version {})",
            stored.version
        )));
    }
    Ok(Some(stored))
}

/// Writes `stored` to `file` whole: a temporary file made 600, synced, then renamed over the old
/// one, so that a crash leaves the old state or the new one.
fn write(file: &Path, stored: &Stored) -> Result<(), HubError> {
    let failed =
        |error: std::io::Error| HubError(format!("{} cannot be written: {error}", file.display()));
    let folder = file.parent().unwrap_or(Path::new("."));
    let temporary = folder.join(format!(".{STATE_FILE}.{}.new", std::process::id()));
    // One a crash of a process of the same id left.
    let _ = std::fs::remove_file(&temporary);
    let text = serde_json::to_vec_pretty(stored).expect("the state serializes");
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)
        .and_then(|mut out| {
            out.write_all(&text)?;
            out.sync_all()
        })
        .and_then(|()| std::fs::rename(&temporary, file));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(failed(error));
    }
    File::open(folder)
        .and_then(|folder| folder.sync_all())
        .map_err(failed)
}
