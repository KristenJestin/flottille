//! The hub's state and its rules (`docs/remote.md`, "The hub", "The first member", "Pairing"), as
//! a library the server part will call: its key, the setup code, the member list it stores, the
//! invitations and their join requests, and the presence and addresses the members tell it.
//!
//! The hub signs nothing and cannot add, remove or rename a member: every change it stores is a
//! member's, checked by `flottille-link`'s list check against the list it holds, before it is
//! stored. An addition answers a join request and is signed by the member that invited.
//!
//! On disk, its state folder (700) holds `hub.key` (600, `flottille-link`'s `HubKey`), `hub.lock`,
//! which one process holds while it uses the folder, and `hub.json` (600), written whole under a
//! temporary name and renamed over the old one:
//!
//! ```json
//! {"version": 1, "setup_code": "7KQ2-M4XP-9TRW-H3CD", "members": [/* the list's changes */]}
//! ```
//!
//! `setup_code` is there only while the list is empty. The invitations, the join requests, the
//! asks taken, the presence and the addresses are kept in memory: they last minutes, or are told
//! again when a device connects.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use flottille_link::{
    Action, Change, HubKey, InvitationAsk, ListError, MemberList, Members, PublicKey, Role, Token,
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

/// How long an invitation lasts, and how far an ask's time may be from the hub's.
pub const FIVE_MINUTES: u64 = 5 * 60;

/// The state's file in the hub's state folder.
pub const STATE_FILE: &str = "hub.json";

/// The file whose lock one process holds while it uses the state folder.
pub const LOCK_FILE: &str = "hub.lock";

const VERSION: u32 = 1;
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

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

/// A device that answered an invitation and waits for the member that invited to accept it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinRequest {
    pub token: Token,
    pub key: PublicKey,
    pub inviter: PublicKey,
}

/// An invitation given and not yet over.
struct Open {
    token: Token,
    inviter: PublicKey,
    made: u64,
    /// The new device's key, once one answered.
    joining: Option<PublicKey>,
}

/// What `hub.json` holds.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    setup_code: Option<String>,
    members: MemberList,
}

pub struct Hub<C: Clock = SystemClock> {
    /// Held for the hub's life: no other process writes its state meanwhile.
    _lock: File,
    file: PathBuf,
    key: HubKey,
    clock: C,
    stored: Stored,
    /// The stored list, checked; `None` while it is empty.
    checked: Option<Members>,
    /// The line to print in the log, when this start made the setup code.
    announcement: Option<String>,
    open: Vec<Open>,
    /// The asks taken, by their member and nonce, with their time: kept while they could be taken.
    asks: Vec<(PublicKey, [u8; 16], u64)>,
    presence: Vec<(PublicKey, Presence)>,
    addresses: Vec<(PublicKey, Vec<String>)>,
}

impl<C: Clock> Hub<C> {
    /// The hub of the state `folder`, made there the first time: its key, and with an empty member
    /// list a setup code, announced once (`take_announcement`). The folder is locked while the hub
    /// lives: another process using it is refused. A stored list that does not check is refused,
    /// and so is the start.
    pub fn open(folder: &Path, clock: C) -> Result<Hub<C>, HubError> {
        let key = HubKey::load_or_make(folder).map_err(|error| HubError(error.to_string()))?;
        let lock = lock(folder)?;
        let file = folder.join(STATE_FILE);
        let mut stored = read(&file)?.unwrap_or(Stored {
            version: VERSION,
            ..Stored::default()
        });
        let checked = match stored.members.root() {
            Some(root) => Some(stored.members.check(&root).map_err(|error| {
                HubError(format!(
                    "the member list in {} does not check: {}; the hub stores only what members \
                     signed, so the file was changed or damaged: run flottille-hub setup-code \
                     --reset on this machine to start again",
                    file.display(),
                    why(error)
                ))
            })?),
            None => None,
        };
        let mut announcement = None;
        if checked.is_none() && stored.setup_code.is_none() {
            let code = new_code()?;
            stored.setup_code = Some(code.clone());
            write(&file, &stored)?;
            announcement = Some(format!(
                "setup code: {code}, valid until a first member joins"
            ));
        }
        Ok(Hub {
            _lock: lock,
            file,
            key,
            clock,
            stored,
            checked,
            announcement,
            open: Vec::new(),
            asks: Vec::new(),
            presence: Vec::new(),
            addresses: Vec::new(),
        })
    }

    /// The hub's public key, which the devices pin when they join.
    pub fn key(&self) -> PublicKey {
        self.key.public()
    }

    /// The line the log prints at the start that made the setup code; `None` after, and at any
    /// other start.
    pub fn take_announcement(&mut self) -> Option<String> {
        self.announcement.take()
    }

    /// The setup code, while no member exists.
    pub fn setup_code(&self) -> Result<&str, HubError> {
        match (&self.checked, &self.stored.setup_code) {
            (None, Some(code)) => Ok(code),
            _ => Err(has_members()),
        }
    }

    /// The first member: `from`, the device that presents `code`, with its own entry `root`, a
    /// runner signed by itself. The code works once, only while no member exists.
    pub fn setup(&mut self, code: &str, from: &PublicKey, root: Change) -> Result<(), HubError> {
        let stored = self.setup_code()?;
        let same = match (code_bytes(code), code_bytes(stored)) {
            (Some(presented), Some(stored)) => bool::from(presented.ct_eq(&stored)),
            _ => false,
        };
        if !same {
            return Err(refused(
                "this setup code is not the hub's: copy it again from the hub's log, or from \
                 flottille-hub setup-code on the hub's machine",
            ));
        }
        let own = matches!(&root.action, Action::Add { key, .. } if key == from);
        if !own || root.signer != *from {
            return Err(refused(
                "the first member's entry is not the entry of the device that presents the setup \
                 code",
            ));
        }
        let members = MemberList::from_changes(vec![root]);
        let checked = members.check(from).map_err(refuse_change)?;
        self.store(
            Stored {
                version: VERSION,
                setup_code: None,
                members,
            },
            checked,
        )
    }

    /// The member list, as the members signed it.
    pub fn list(&self) -> &MemberList {
        &self.stored.members
    }

    /// The token of a new invitation, for a member's signed ask: valid 5 minutes, once.
    pub fn invite(&mut self, ask: &InvitationAsk) -> Result<Token, HubError> {
        let now = self.sweep();
        if ask.hub != self.key() {
            return Err(refused("this ask is for another hub"));
        }
        if !ask.signed() {
            return Err(refused("this ask does not carry its member's signature"));
        }
        if now.abs_diff(ask.at) >= FIVE_MINUTES {
            return Err(refused(
                "this ask was not signed within 5 minutes of the hub's time: check the device's \
                 clock",
            ));
        }
        if self
            .asks
            .iter()
            .any(|(member, nonce, _)| *member == ask.member && *nonce == ask.nonce)
        {
            return Err(refused("this ask was taken already"));
        }
        let member = self.member(&ask.member)?;
        if member.role == Role::Viewer {
            return Err(refused(format!(
                "{} may only view: a controller or a runner may invite a device",
                member.name
            )));
        }
        let token = Token(random()?);
        self.asks.push((ask.member, ask.nonce, ask.at));
        self.open.push(Open {
            token,
            inviter: ask.member,
            made: now,
            joining: None,
        });
        Ok(token)
    }

    /// A new device answers the invitation of `token` with its key: the request waits for the
    /// member that invited.
    pub fn join(&mut self, token: &Token, key: PublicKey) -> Result<(), HubError> {
        self.sweep();
        let open = self
            .find(token)
            .ok_or_else(|| refused("this invitation is unknown or expired: ask for a new one"))?;
        if open.joining.is_some() {
            return Err(refused(
                "this invitation was answered already: ask for a new one",
            ));
        }
        open.joining = Some(key);
        Ok(())
    }

    /// The join requests that wait for `inviter`.
    pub fn requests(&mut self, inviter: &PublicKey) -> Vec<JoinRequest> {
        self.sweep();
        self.open
            .iter()
            .filter(|open| open.inviter == *inviter)
            .filter_map(|open| {
                Some(JoinRequest {
                    token: open.token,
                    key: open.joining?,
                    inviter: open.inviter,
                })
            })
            .collect()
    }

    /// `by`, the member that invited, refuses the join request of `token`: the invitation ends.
    pub fn refuse(&mut self, token: &Token, by: &PublicKey) -> Result<(), HubError> {
        self.sweep();
        let open = self
            .find(token)
            .filter(|open| open.joining.is_some())
            .ok_or_else(|| refused("no join request holds this invitation"))?;
        if open.inviter != *by {
            return Err(refused(
                "only the member that invited may refuse its join request",
            ));
        }
        let token = open.token;
        self.open.retain(|open| open.token != token);
        Ok(())
    }

    /// A member's change of the list, stored once it checks after the list the hub holds. An
    /// addition must answer a join request to its signer, which it ends.
    pub fn submit(&mut self, change: Change) -> Result<(), HubError> {
        self.sweep();
        let Some(root) = self.stored.members.root() else {
            return Err(refused(
                "the hub has no member yet: the first one joins with the setup code",
            ));
        };
        let answered = match &change.action {
            Action::Add { key, .. } => {
                let open = self
                    .open
                    .iter()
                    .find(|open| open.inviter == change.signer && open.joining == Some(*key))
                    .ok_or_else(|| {
                        refused(
                            "the hub refuses this change: it adds a key no join request to its \
                             signer holds (a device joins through an invitation)",
                        )
                    })?;
                Some(open.token)
            }
            Action::Remove { .. } | Action::Rename { .. } => None,
        };
        let mut changes = self.stored.members.changes().to_vec();
        changes.push(change);
        let members = MemberList::from_changes(changes);
        let checked = members.check(&root).map_err(refuse_change)?;
        self.store(
            Stored {
                members,
                ..self.stored.clone()
            },
            checked,
        )?;
        if let Some(token) = answered {
            self.open.retain(|open| open.token != token);
        }
        Ok(())
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

    /// The addresses a member tells, kept in memory for the other members.
    pub fn tell_addresses(
        &mut self,
        member: &PublicKey,
        addresses: Vec<String>,
    ) -> Result<(), HubError> {
        self.member(member)?;
        keep(&mut self.addresses, member, addresses);
        Ok(())
    }

    pub fn addresses(&self, member: &PublicKey) -> Option<&[String]> {
        self.addresses
            .iter()
            .find(|(key, _)| key == member)
            .map(|(_, addresses)| addresses.as_slice())
    }

    /// Writes `stored`, then holds it: a state that could not be written is not taken.
    fn store(&mut self, stored: Stored, checked: Members) -> Result<(), HubError> {
        write(&self.file, &stored)?;
        // A member removed takes its presence and addresses with it.
        let member = |key: &PublicKey| checked.get(key).is_some();
        self.presence.retain(|(key, _)| member(key));
        self.addresses.retain(|(key, _)| member(key));
        self.stored = stored;
        self.checked = Some(checked);
        Ok(())
    }

    fn member(&self, key: &PublicKey) -> Result<&flottille_link::Member, HubError> {
        self.checked
            .as_ref()
            .and_then(|members| members.get(key))
            .ok_or_else(|| refused(format!("{} is not a member", key.fingerprint())))
    }

    /// The invitation of `token`, compared in constant time.
    fn find(&mut self, token: &Token) -> Option<&mut Open> {
        self.open
            .iter_mut()
            .find(|open| bool::from(open.token.0.ct_eq(&token.0)))
    }

    /// Drops what is over: the invitations 5 minutes old, their join requests with them, and the
    /// asks that could not be taken any more. Returns the time.
    fn sweep(&mut self) -> u64 {
        let now = self.clock.now();
        self.open.retain(|open| now < open.made + FIVE_MINUTES);
        self.asks.retain(|(_, _, at)| now < at + FIVE_MINUTES);
        now
    }
}

/// `flottille-hub setup-code --reset` (`docs/remote.md`, "Recovery"): empties the member list of
/// the state `folder` and makes a new setup code, which it returns. It reads nothing of the state,
/// so that a list that does not check, or a damaged file, is no obstacle; the hub's key stays. The
/// folder's lock is taken: refused while a hub uses it, whose next write would undo the reset.
pub fn reset(folder: &Path) -> Result<String, HubError> {
    HubKey::load_or_make(folder).map_err(|error| HubError(error.to_string()))?;
    let _lock = lock(folder)?;
    let code = new_code()?;
    let stored = Stored {
        version: VERSION,
        setup_code: Some(code.clone()),
        members: MemberList::default(),
    };
    write(&folder.join(STATE_FILE), &stored)?;
    Ok(code)
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

fn has_members() -> HubError {
    refused(
        "the hub has a member already: a setup code serves only before the first member joins \
         (flottille-hub setup-code --reset empties the list and makes a new one)",
    )
}

fn why(error: ListError) -> String {
    match error {
        ListError::Refused(why) => why,
        ListError::Reset => "its root is not the first member's".to_owned(),
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

fn random<const N: usize>() -> Result<[u8; N], HubError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|error| HubError(format!("no random bytes: {error}")))?;
    Ok(bytes)
}

/// A new setup code: 80 random bits in Crockford's base32, by groups of four
/// (`7KQ2-M4XP-9TRW-H3CD`).
fn new_code() -> Result<String, HubError> {
    let bytes: [u8; 10] = random()?;
    let mut value = [0u8; 16];
    value[6..].copy_from_slice(&bytes);
    let value = u128::from_be_bytes(value);
    let mut code = String::with_capacity(19);
    for at in 0..16 {
        if at > 0 && at % 4 == 0 {
            code.push('-');
        }
        code.push(ALPHABET[((value >> (75 - 5 * at)) & 31) as usize] as char);
    }
    Ok(code)
}

/// The 80 bits of a setup code as a person may type it: in any case, `O` as `0`, `I` and `L` as
/// `1`, hyphens and spaces skipped. `None` when it is not 16 such symbols.
fn code_bytes(code: &str) -> Option<[u8; 10]> {
    let (mut value, mut symbols) = (0u128, 0);
    for c in code.chars() {
        if c == '-' || c.is_whitespace() {
            continue;
        }
        let symbol = match c.to_ascii_uppercase() {
            'O' => 0,
            'I' | 'L' => 1,
            upper => ALPHABET
                .iter()
                .position(|&letter| letter as char == upper)? as u128,
        };
        symbols += 1;
        if symbols > 16 {
            return None;
        }
        value = (value << 5) | symbol;
    }
    if symbols != 16 {
        return None;
    }
    value.to_be_bytes()[6..].try_into().ok()
}

/// The state in `file`, `None` when there is none. A link, or a file others may read or write, is
/// refused: it holds the setup code.
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
            "{shown} is open to other users (mode {mode:o}): it holds the setup code; make it \
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
