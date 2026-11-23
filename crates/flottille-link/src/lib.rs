//! Flottille's link between the user's devices (`docs/remote.md`): identity, keys, the member
//! list, the pair code, request signing, and the link itself between two machines, over a
//! transport only `transport.rs` names.
//!
//! A device's identity is the public half of an Ed25519 key. The private half never leaves the
//! types of this crate: no method returns it, `Debug` prints the key's fingerprint only, and it is
//! zeroized when dropped. This crate knows no daemon and no store: the caller names the key folder
//! and hands its keyring in.

mod control;
mod hello;
mod link;
mod members;
mod offers;
mod pairing;
mod request;
mod serial;
mod sessions;
mod transport;
mod tunnel;
mod upload;

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use ed25519_dalek::{SECRET_KEY_LENGTH, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub use control::{
    Arriving, Ask, Asked, Asker, ControlError, Device, Facts, HANDSHAKE_LIMIT, HUB_PORT,
    HubSession, HubUrl, KEEPALIVE, LINE_LIMIT, LinkPacket, PairProof, Paired, Pairing, PresenceOf,
    Pushed, RELAY_PACKET, RelayPort, Relays, SILENCE, Session, Stream, Tls, ToDevice, Transcript,
    arrive, relays_in_memory,
};
pub use hello::{Hello, NeedsUpdate, PeerState, choose, choose_hub};
pub use link::{
    Accepting, Answers, Call, ConnectionShare, Deliver, Endpoint, FRAME_LIMIT, Git, GitFetch,
    GitOffer, GitPlace, GitRepository, LaunchAsk, Link, LinkError, MachineMessage, Message,
    Received, RepositoryIdentity, Secret, Share, SharedEntry, Streamed, Traffic, UNKNOWN_MESSAGE,
};
pub use members::{
    Action, Change, ChangeHash, Colour, Icon, ListError, MANAGING_HEADROOM, MOST_CHANGES, Member,
    MemberList, Members, Role, Signature, check_name,
};
pub use offers::{Capabilities, MOST_AGENTS, MOST_PROJECTS, Offered};
pub use pairing::{PairCode, PairCodeError, PairSecret, Proposed};
pub use request::{
    Accepted, CLOSE_LASTS, CLOSES_SERVED_MOST, ClosesServed, FULL_SWITCH_LASTS, NONCES_PER_DEVICE,
    Nonce, REQUEST_WINDOW, Request, RequestError, Requests, Signed, full_switch, kept_close,
    runs_agents,
};
pub use serial::hidden;
pub use sessions::{
    LightProject, LightSession, MOST_SESSIONS, ProjectList, STATES, SessionList, TITLE_LIMIT,
};
pub use transport::TlsServer;
pub use tunnel::TunnelStream;
pub use upload::{Attached, UPLOAD_CHUNK, fits, fits_with, upload_chunks, upload_name};

/// The machine key's file in the key folder: the daemon's alone.
pub const MACHINE_FILE: &str = "machine.key";
/// The hub's key's file in its state folder: the hub's alone.
pub const HUB_FILE: &str = "hub.key";
/// The device key's file in the key folder, on a machine with no keyring: its clients' alone.
pub const DEVICE_FILE: &str = "device.key";
/// The device key's item in the session's keyring (`docs/vault.md`, "Where the secrets are").
pub const DEVICE_LABEL: &str = "Flottille: device";
/// Its attributes: Flottille's, and never `flottille-entry`, which names a vault entry (one may be
/// called `device`).
pub const DEVICE_ATTRIBUTES: [(&str, &str); 2] =
    [("application", "flottille"), ("flottille-key", "device")];
/// The new device key of a setup or a join on its way, beside the kept one, with the same care:
/// its file on a machine with no keyring.
pub const DEVICE_PENDING_FILE: &str = "device.key.pending";
/// Its item in the session's keyring.
pub const DEVICE_PENDING_LABEL: &str = "Flottille: device, pending";
pub const DEVICE_PENDING_ATTRIBUTES: [(&str, &str); 2] = [
    ("application", "flottille"),
    ("flottille-key", "device-pending"),
];

/// Where a device key is kept: its keyring item, or its file with no keyring.
struct Place {
    label: &'static str,
    attributes: &'static [(&'static str, &'static str)],
    file: &'static str,
}

const KEPT: Place = Place {
    label: DEVICE_LABEL,
    attributes: &DEVICE_ATTRIBUTES,
    file: DEVICE_FILE,
};

const PENDING: Place = Place {
    label: DEVICE_PENDING_LABEL,
    attributes: &DEVICE_PENDING_ATTRIBUTES,
    file: DEVICE_PENDING_FILE,
};

/// A device's identity: the public half of its key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// The short form a person compares, on the screens: the first 8 bytes
    /// of the key's SHA-256, in hexadecimal by groups of four (`3f2a-91bc-0d4e-77a1`).
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(self.as_bytes());
        digest[..8]
            .chunks(2)
            .map(|pair| format!("{:02x}{:02x}", pair[0], pair[1]))
            .collect::<Vec<_>>()
            .join("-")
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PublicKey")
            .field(&self.fingerprint())
            .finish()
    }
}

/// Why a key could not be made or read, in a sentence that names its file and never its bytes.
#[derive(Debug)]
pub struct KeyError(String);

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KeyError {}

/// What a keyring answers when it cannot.
#[derive(Debug)]
pub enum KeyringError {
    /// There is no keyring: no Secret Service on this machine (a headless server).
    Absent,
    /// There is one, and it failed (locked, refused): said, never worked around.
    Failed(String),
    /// More than one item has the attributes asked for: which one is meant cannot be told.
    Several(usize),
}

/// The session's keyring, as the client that holds a device key reaches it.
pub trait Keyring {
    /// The secret of the item that has these attributes, `None` when there is none; `Several`
    /// when more than one has them, which cannot be told apart.
    fn read(&self, attributes: &[(&str, &str)])
    -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError>;

    /// Keeps `secret` in an item labelled `label`, with these attributes.
    fn store(
        &self,
        label: &str,
        attributes: &[(&str, &str)],
        secret: &[u8],
    ) -> Result<(), KeyringError>;

    /// Removes the items that have these attributes; nothing when there is none.
    fn delete(&self, _attributes: &[(&str, &str)]) -> Result<(), KeyringError> {
        Err(KeyringError::Failed(
            "this keyring removes no item".to_owned(),
        ))
    }
}

/// The key of a machine's daemon (`docs/remote.md`, "Identity and keys"): it signs what a machine
/// may send, never a user's action. Only the daemon makes and reads it.
pub struct MachineKey(SigningKey);

impl MachineKey {
    /// The key in `folder` (`$XDG_STATE_HOME/flottille/remote`), made there the first time. The
    /// folder is made the user's only (700) and the key's file is 600; either one open to others
    /// is refused, and left as it is.
    pub fn load_or_make(folder: &Path) -> Result<MachineKey, KeyError> {
        private_folder(folder)?;
        load_or_make_file(&folder.join(MACHINE_FILE)).map(MachineKey)
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }
}

impl fmt::Debug for MachineKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MachineKey")
            .field(&self.public().fingerprint())
            .finish()
    }
}

/// The key of the user's hub (`docs/remote.md`, "Identity and keys"): the devices pin it when they
/// join. Only the hub makes and reads it.
pub struct HubKey(SigningKey);

impl HubKey {
    /// The key in the hub's state `folder`, made there the first time, with the machine key's care:
    /// the folder 700, the file 600, either one open to others refused and left as it is.
    pub fn load_or_make(folder: &Path) -> Result<HubKey, KeyError> {
        private_folder(folder)?;
        load_or_make_file(&folder.join(HUB_FILE)).map(HubKey)
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }
}

impl fmt::Debug for HubKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("HubKey")
            .field(&self.public().fingerprint())
            .finish()
    }
}

/// The key of a machine's clients, the window and the command line of a user session: it signs
/// the user's actions. A client makes and reads it, never the daemon, which could then forge them.
pub struct DeviceKey(SigningKey);

impl DeviceKey {
    /// The key in the session's keyring, made there the first time; on a machine with no keyring,
    /// in `folder` as the machine key is (`device.key`, 600). A keyring that is there but fails is
    /// said: a file instead would give the same device a second identity.
    pub fn load_or_make(keyring: &impl Keyring, folder: &Path) -> Result<DeviceKey, KeyError> {
        if let Some(key) = DeviceKey::load(keyring, folder)? {
            return Ok(key);
        }
        DeviceKey::make()?.keep(keyring, folder)
    }

    /// The key kept in the session's keyring, or with no keyring in `folder`; `None` when there
    /// is none yet.
    pub fn load(keyring: &impl Keyring, folder: &Path) -> Result<Option<DeviceKey>, KeyError> {
        load_at(keyring, folder, &KEPT)
    }

    /// A new key, in memory only until it is kept: a device that joins keeps its key once the
    /// join succeeded, so that a join that did not makes a new one (`docs/remote.md`, "Pairing").
    pub fn make() -> Result<DeviceKey, KeyError> {
        generate().map(DeviceKey)
    }

    /// Keeps this key where `load` reads it, when no key is kept there yet; the key kept then,
    /// which is another one when another client kept its own at the same moment. Two kept at the
    /// same moment in the keyring are refused: which one is the member's cannot be told.
    pub fn keep(self, keyring: &impl Keyring, folder: &Path) -> Result<DeviceKey, KeyError> {
        keep_at(self, keyring, folder, &KEPT)
    }

    /// The new key of a setup or a join that did not say how it ended, kept beside the kept one
    /// (`keep_pending`); `None` when there is none. Two clients that kept theirs at the same
    /// moment leave two, of which the hub may have seen either: both are removed, never sent
    /// again, and `None` said, so that a new one is made.
    pub fn load_pending(
        keyring: &impl Keyring,
        folder: &Path,
    ) -> Result<Option<DeviceKey>, KeyError> {
        if let Err(KeyringError::Several(_)) = keyring.read(PENDING.attributes) {
            DeviceKey::discard_pending(keyring, folder)?;
            return Ok(None);
        }
        load_at(keyring, folder, &PENDING)
    }

    /// Keeps this new key as pending, before the hub sees it: a setup or a join whose command is
    /// gone before it ends leaves it there, for the next one to keep once it is a member, or to
    /// remove. The key pending then, which is another one when another client kept its own.
    pub fn keep_pending(
        self,
        keyring: &impl Keyring,
        folder: &Path,
    ) -> Result<DeviceKey, KeyError> {
        keep_at(self, keyring, folder, &PENDING)
    }

    /// This pending key, a member now, made the kept one; the pending one removed. When another
    /// client kept its own first, that one is said and this one stays pending: it may be the
    /// member's, and only it signs for it.
    pub fn promote(self, keyring: &impl Keyring, folder: &Path) -> Result<DeviceKey, KeyError> {
        let public = self.public();
        let kept = self.keep(keyring, folder)?;
        if kept.public() == public {
            DeviceKey::discard_pending(keyring, folder)?;
        }
        Ok(kept)
    }

    /// Removes the pending key, which is not a member: never sent to a hub again.
    pub fn discard_pending(keyring: &impl Keyring, folder: &Path) -> Result<(), KeyError> {
        match keyring.delete(PENDING.attributes) {
            Ok(()) => Ok(()),
            Err(error @ (KeyringError::Failed(_) | KeyringError::Several(_))) => Err(failed(error)),
            Err(KeyringError::Absent) => {
                let file = folder.join(PENDING.file);
                match std::fs::remove_file(&file) {
                    Err(error) if error.kind() != ErrorKind::NotFound => Err(KeyError(format!(
                        "{} cannot be removed: {error}",
                        file.display()
                    ))),
                    _ => Ok(()),
                }
            }
        }
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DeviceKey")
            .field(&self.public().fingerprint())
            .finish()
    }
}

/// The key at `place`: in the keyring, or with none in `folder`.
fn load_at(
    keyring: &impl Keyring,
    folder: &Path,
    place: &Place,
) -> Result<Option<DeviceKey>, KeyError> {
    match keyring.read(place.attributes) {
        Ok(Some(seed)) => from_seed(&seed)
            .map(DeviceKey)
            .map(Some)
            .ok_or_else(|| not_a_key(place)),
        Ok(None) => Ok(None),
        Err(KeyringError::Absent) => {
            private_folder(folder)?;
            read_file(&folder.join(place.file)).map(|key| key.map(DeviceKey))
        }
        Err(error @ (KeyringError::Failed(_) | KeyringError::Several(_))) => Err(failed(error)),
    }
}

/// Keeps `key` at `place` when none is there; what is there then, read back.
fn keep_at(
    key: DeviceKey,
    keyring: &impl Keyring,
    folder: &Path,
    place: &Place,
) -> Result<DeviceKey, KeyError> {
    match keyring.read(place.attributes) {
        Ok(Some(seed)) => from_seed(&seed)
            .map(DeviceKey)
            .ok_or_else(|| not_a_key(place)),
        Ok(None) => match keyring.store(place.label, place.attributes, key.0.as_bytes()) {
            // Read back: another client may have stored its own at the same moment.
            Ok(()) => match keyring.read(place.attributes) {
                Ok(Some(seed)) => from_seed(&seed)
                    .map(DeviceKey)
                    .ok_or_else(|| not_a_key(place)),
                Ok(None) => Err(keyring_failed(
                    "the key was stored, then was not there".to_owned(),
                )),
                Err(error @ (KeyringError::Failed(_) | KeyringError::Several(_))) => {
                    Err(failed(error))
                }
                Err(KeyringError::Absent) => {
                    Err(keyring_failed("it was there, then was not".to_owned()))
                }
            },
            Err(error @ (KeyringError::Failed(_) | KeyringError::Several(_))) => Err(failed(error)),
            Err(KeyringError::Absent) => {
                Err(keyring_failed("it was there, then was not".to_owned()))
            }
        },
        Err(error @ (KeyringError::Failed(_) | KeyringError::Several(_))) => Err(failed(error)),
        Err(KeyringError::Absent) => {
            private_folder(folder)?;
            let file = folder.join(place.file);
            remove_left_behind(&file);
            // Linked into place, never over a key that is there.
            write_new(&file, key.0.as_bytes())?;
            read_file(&file)?
                .map(DeviceKey)
                .ok_or_else(|| KeyError(format!("{} was made, then was gone", file.display())))
        }
    }
}

fn not_a_key(place: &Place) -> KeyError {
    KeyError(format!("the keyring's item {} is not a key", place.label))
}

fn keyring_failed(why: String) -> KeyError {
    KeyError(format!("the device key's keyring failed: {why}"))
}

/// What a keyring that is there answered when it could not.
fn failed(error: KeyringError) -> KeyError {
    match error {
        KeyringError::Several(n) => keyring_failed(format!(
            "{n} items of the keyring hold a device key of Flottille's; one at most may: \
             flottille remote members says which key is a member"
        )),
        KeyringError::Failed(why) => keyring_failed(why),
        KeyringError::Absent => keyring_failed("it was there, then was not".to_owned()),
    }
}

/// `N` bytes from the operating system's random generator.
fn random<const N: usize>() -> Result<[u8; N], KeyError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|error| KeyError(format!("no random bytes: {error}")))?;
    Ok(bytes)
}

/// A new key, from the operating system's random generator.
fn generate() -> Result<SigningKey, KeyError> {
    let mut seed = Zeroizing::new([0u8; SECRET_KEY_LENGTH]);
    getrandom::fill(seed.as_mut_slice())
        .map_err(|error| KeyError(format!("no random bytes for a new key: {error}")))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The key whose private half is `seed`, when it is one.
fn from_seed(seed: &[u8]) -> Option<SigningKey> {
    if seed.len() != SECRET_KEY_LENGTH {
        return None;
    }
    // Copied into room made beforehand: a conversion would leave unwiped copies on the stack.
    let mut bytes = Zeroizing::new([0u8; SECRET_KEY_LENGTH]);
    bytes.copy_from_slice(seed);
    Some(SigningKey::from_bytes(&bytes))
}

/// Makes `folder` (and its missing parents) the user's only, then checks that what is there is a
/// real folder, owned by the user and closed to everyone else.
fn private_folder(folder: &Path) -> Result<(), KeyError> {
    let shown = folder.display();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .map_err(|error| KeyError(format!("{shown} cannot be made: {error}")))?;
    let metadata = std::fs::symlink_metadata(folder)
        .map_err(|error| KeyError(format!("{shown} cannot be read: {error}")))?;
    if !metadata.is_dir() {
        return Err(KeyError(format!("{shown} is not a folder")));
    }
    if metadata.uid() != user() {
        return Err(KeyError(format!("{shown} is not owned by you")));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(KeyError(format!(
            "{shown} is open to other users (mode {mode:o}): make it yours only with `chmod 700 \
             {shown}`"
        )));
    }
    Ok(())
}

fn user() -> u32 {
    // SAFETY: getuid has no precondition and cannot fail.
    unsafe { libc::getuid() }
}

/// The key in `file`, made there when there is none.
fn load_or_make_file(file: &Path) -> Result<SigningKey, KeyError> {
    remove_left_behind(file);
    if let Some(key) = read_file(file)? {
        return Ok(key);
    }
    write_new(file, generate()?.as_bytes())?;
    // What is there now: this key, or the one another process made at the same moment.
    read_file(file)?.ok_or_else(|| KeyError(format!("{} was made, then was gone", file.display())))
}

/// Removes the temporary names of `file` (`write_new`) a process that is gone left: a crash
/// between the link and the removal leaves one, a second name of the private key. This process's
/// own may be a key another of its threads is making, and is left.
fn remove_left_behind(file: &Path) {
    let (Some(folder), Some(name)) = (file.parent(), file.file_name()) else {
        return;
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_name = entry.file_name();
        let Some(rest) = entry_name
            .to_str()
            .and_then(|entry_name| entry_name.strip_prefix(&prefix))
            .and_then(|rest| rest.strip_suffix(".new"))
        else {
            continue;
        };
        let Some(pid) = rest
            .split_once('.')
            .filter(|(_, made)| !made.is_empty() && made.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|(pid, _)| pid.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        if pid > 0 && pid as u32 != std::process::id() && gone(pid) {
            // Not there any more, or not removable: the next start tries again.
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Whether no process has this id.
fn gone(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 sends nothing; it only asks whether the process exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    !alive && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// The key in `file`, `None` when there is no file. A link, a file of another user, or one others
/// may read or write is refused, as is a file that is not a key.
fn read_file(file: &Path) -> Result<Option<SigningKey>, KeyError> {
    let shown = file.display();
    // Without waiting: an open of a pipe named so would wait for a writer, and the daemon with it.
    // On a file, `O_NONBLOCK` changes nothing.
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(file);
    let opened = match opened {
        Ok(opened) => opened,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(KeyError(format!(
                "{shown} is a link: a key is a file of its own; remove the link"
            )));
        }
        Err(error) => return Err(KeyError(format!("{shown} cannot be read: {error}"))),
    };
    let metadata = opened
        .metadata()
        .map_err(|error| KeyError(format!("{shown} cannot be read: {error}")))?;
    if !metadata.is_file() {
        return Err(KeyError(format!("{shown} is not a file")));
    }
    if metadata.uid() != user() {
        return Err(KeyError(format!("{shown} is not owned by you")));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        let open = if mode & 0o044 != 0 {
            "readable"
        } else {
            "writable"
        };
        return Err(KeyError(format!(
            "{shown} is {open} by others (mode {mode:o}): a private key is yours only; make it so \
             with `chmod 600 {shown}`"
        )));
    }
    // One byte more than a key, so that a longer file is told; read into room made beforehand,
    // which no growth copies elsewhere before it is zeroized.
    let mut seed = Zeroizing::new(Vec::with_capacity(SECRET_KEY_LENGTH + 1));
    opened
        .take(SECRET_KEY_LENGTH as u64 + 1)
        .read_to_end(&mut seed)
        .map_err(|error| KeyError(format!("{shown} cannot be read: {error}")))?;
    from_seed(&seed)
        .map(Some)
        .ok_or_else(|| KeyError(format!("{shown} is not a key of Flottille's")))
}

/// Writes a new key file: a temporary file made 600 with `O_EXCL`, written and synced, then linked
/// into place, which never replaces a key that is there (a rename would: two clients making the
/// device key at once would each keep their own).
fn write_new(file: &Path, secret: &[u8]) -> Result<(), KeyError> {
    static MADE: AtomicU64 = AtomicU64::new(0);
    let shown = file.display();
    let folder = file.parent().unwrap_or(Path::new("."));
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = folder.join(format!(
        ".{name}.{}.{}.new",
        std::process::id(),
        MADE.fetch_add(1, Ordering::Relaxed)
    ));
    let failed = |error: std::io::Error| KeyError(format!("{shown} cannot be made: {error}"));
    let written = link_new(&temporary, file, secret);
    let removed = match std::fs::remove_file(&temporary) {
        // Renamed as the key (`link_new`, on Android): nothing is left to remove.
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        removed => removed,
    };
    written.map_err(failed)?;
    removed.map_err(failed)?;
    // The folder's entries, durable as the file is.
    File::open(folder)
        .and_then(|folder| folder.sync_all())
        .map_err(failed)
}

/// `secret` in `temporary`, then linked as `file` unless a file is there already.
fn link_new(temporary: &Path, file: &Path, secret: &[u8]) -> std::io::Result<()> {
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(temporary)?;
    out.write_all(secret)?;
    out.sync_all()?;
    match std::fs::hard_link(temporary, file) {
        // Android's SELinux refuses a hard link in an app's own data: the phone's endpoint key
        // (`flottille-mobile`) is made by the app's one process, so a rename, which never leaves a
        // part of a key under its name, does it; a key made there already is kept.
        #[cfg(target_os = "android")]
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            match std::fs::symlink_metadata(file) {
                Ok(_) => Ok(()),
                Err(missing) if missing.kind() == ErrorKind::NotFound => {
                    std::fs::rename(temporary, file)
                }
                Err(other) => Err(other),
            }
        }
        Err(error) if error.kind() != ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}
