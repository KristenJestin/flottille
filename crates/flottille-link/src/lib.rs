//! Flottille's link between the user's devices (`docs/remote.md`): identity, keys, the member
//! list, the invitation, request signing, and the link itself between two machines, over a
//! transport only `transport.rs` names.
//!
//! A device's identity is the public half of an Ed25519 key. The private half never leaves the
//! types of this crate: no method returns it, `Debug` prints the key's fingerprint only, and it is
//! zeroized when dropped. This crate knows no daemon and no store: the caller names the key folder
//! and hands its keyring in.

mod address;
mod control;
mod hello;
mod invitation;
mod link;
mod members;
mod request;
mod serial;
mod transport;
mod words;

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use ed25519_dalek::{SECRET_KEY_LENGTH, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub use address::Address;
pub use control::{
    Arriving, Ask, Asked, Asker, ControlError, Device, HANDSHAKE_LIMIT, HUB_PORT, HubSession,
    HubUrl, LINE_LIMIT, PresenceOf, Pushed, Session, SetupCode, SetupProof, Stream, Tls, ToDevice,
    Told, Transcript, arrive,
};
pub use hello::{Hello, NeedsUpdate, PeerState, choose};
pub use invitation::{Invitation, InvitationAsk, InvitationError, Token};
pub use link::{
    Accepting, Endpoint, FRAME_LIMIT, Link, LinkError, MachineMessage, Message, Received, Traffic,
};
pub use members::{
    Action, Change, ChangeHash, ListError, MANAGING_HEADROOM, MOST_CHANGES, Member, MemberList,
    Members, Role, Signature,
};
pub use request::{
    Accepted, NONCES_PER_DEVICE, Nonce, REQUEST_WINDOW, Request, RequestError, Requests, Signed,
};
pub use transport::TlsServer;
pub use words::verification_words;

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

/// A device's identity: the public half of its key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// The short form a person compares, on an invitation and on the screens: the first 8 bytes
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
}

/// The session's keyring, as the client that holds a device key reaches it.
pub trait Keyring {
    /// The secret of the item that has these attributes, `None` when there is none.
    fn read(&self, attributes: &[(&str, &str)])
    -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError>;

    /// Keeps `secret` in an item labelled `label`, with these attributes.
    fn store(
        &self,
        label: &str,
        attributes: &[(&str, &str)],
        secret: &[u8],
    ) -> Result<(), KeyringError>;
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
        let failed = |why: String| KeyError(format!("the device key's keyring failed: {why}"));
        match keyring.read(&DEVICE_ATTRIBUTES) {
            Ok(Some(seed)) => from_seed(&seed)
                .map(DeviceKey)
                .ok_or_else(|| KeyError(format!("the keyring's item {DEVICE_LABEL} is not a key"))),
            Ok(None) => {
                let key = generate()?;
                match keyring.store(DEVICE_LABEL, &DEVICE_ATTRIBUTES, key.as_bytes()) {
                    Ok(()) => Ok(DeviceKey(key)),
                    Err(KeyringError::Failed(why)) => Err(failed(why)),
                    Err(KeyringError::Absent) => {
                        Err(failed("it was there, then was not".to_owned()))
                    }
                }
            }
            Err(KeyringError::Absent) => {
                private_folder(folder)?;
                load_or_make_file(&folder.join(DEVICE_FILE)).map(DeviceKey)
            }
            Err(KeyringError::Failed(why)) => Err(failed(why)),
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
    let removed = std::fs::remove_file(&temporary);
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
        Err(error) if error.kind() != ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}
