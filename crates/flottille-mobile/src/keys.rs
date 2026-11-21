//! The phone's keys (`docs/remote.md`, "Identity and keys"): its device key, sealed by the app,
//! and the key of its endpoint.
//!
//! The device key signs the user's actions. It is kept as the link's `Keyring` keeps one, in a file
//! of the core's folder whose bytes the app sealed (`Sealer`: an AES key of the Android Keystore,
//! which never leaves it), so that the file alone gives nothing.
//!
//! The endpoint's key is what a daemon's machine key is to a computer: the key the phone's link
//! presents to a runner, declared by the device in the member list. A member's own key may not be
//! its machine's (`flottille_link::MemberList`), so it is a key of its own, in the core's folder
//! with the machine key's care (the folder 700, the file 600), which on Android is the app's
//! private storage.

use std::io::ErrorKind;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flottille_link::{Keyring, KeyringError, MachineKey};
use zeroize::Zeroizing;

use crate::MobileError;

/// What seals the device key for the core: the app's, from a key of the Android Keystore. What
/// `seal` gives, `open` gives back; anything else it refuses.
#[uniffi::export(with_foreign)]
pub trait Sealer: Send + Sync {
    fn seal(&self, plain: Vec<u8>) -> Result<Vec<u8>, MobileError>;
    fn open(&self, sealed: Vec<u8>) -> Result<Vec<u8>, MobileError>;
}

/// The sealed device key's file, kept or pending (a join on its way).
pub(crate) const DEVICE_SEALED: &str = "device.sealed";
pub(crate) const PENDING_SEALED: &str = "device.pending.sealed";
/// The endpoint key's folder in the core's folder, and that of the one a join on its way declares.
pub(crate) const ENDPOINT_FOLDER: &str = "endpoint";
pub(crate) const ENDPOINT_PENDING: &str = "endpoint.pending";

/// The link's keyring over the core's folder: each item a file of sealed bytes.
pub(crate) struct Sealed {
    folder: PathBuf,
    sealer: Arc<dyn Sealer>,
}

impl Sealed {
    pub(crate) fn new(folder: &Path, sealer: Arc<dyn Sealer>) -> Sealed {
        Sealed {
            folder: folder.to_owned(),
            sealer,
        }
    }

    /// The file of the item these attributes name: the kept device key, or the pending one.
    fn file(&self, attributes: &[(&str, &str)]) -> Result<PathBuf, KeyringError> {
        let which = attributes
            .iter()
            .find(|(name, _)| *name == "flottille-key")
            .map(|(_, value)| *value);
        match which {
            Some("device") => Ok(self.folder.join(DEVICE_SEALED)),
            Some("device-pending") => Ok(self.folder.join(PENDING_SEALED)),
            _ => Err(KeyringError::Failed(
                "the phone keeps no other item than its device key".to_owned(),
            )),
        }
    }
}

impl Sealed {
    /// Removes the kept device key: a join replaces it with the one the hub admitted.
    pub(crate) fn forget_kept(&self) -> std::io::Result<()> {
        match std::fs::remove_file(self.folder.join(DEVICE_SEALED)) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

impl Keyring for Sealed {
    fn read(
        &self,
        attributes: &[(&str, &str)],
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
        let file = self.file(attributes)?;
        let sealed = match std::fs::read(&file) {
            Ok(sealed) => sealed,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(KeyringError::Failed(format!(
                    "{} cannot be read: {error}",
                    file.display()
                )));
            }
        };
        let plain = self
            .sealer
            .open(sealed)
            .map_err(|error| KeyringError::Failed(format!("the device key: {error}")))?;
        Ok(Some(Zeroizing::new(plain)))
    }

    fn store(
        &self,
        _label: &str,
        attributes: &[(&str, &str)],
        secret: &[u8],
    ) -> Result<(), KeyringError> {
        let file = self.file(attributes)?;
        let sealed = self
            .sealer
            .seal(secret.to_vec())
            .map_err(|error| KeyringError::Failed(format!("the device key: {error}")))?;
        write_private(&file, &sealed)
            .map_err(|error| KeyringError::Failed(format!("{}: {error}", file.display())))
    }

    fn delete(&self, attributes: &[(&str, &str)]) -> Result<(), KeyringError> {
        let file = self.file(attributes)?;
        match std::fs::remove_file(&file) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(KeyringError::Failed(
                format!("{} cannot be removed: {error}", file.display()),
            )),
            _ => Ok(()),
        }
    }
}

/// Writes `bytes` to `file`, the user's only, whole or not at all.
pub(crate) fn write_private(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let partial = file.with_extension("partial");
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)?;
    out.write_all(bytes)?;
    out.sync_all()?;
    std::fs::rename(&partial, file)
}

/// The key of the phone's endpoint, made the first time.
pub(crate) fn endpoint_key(folder: &Path) -> Result<MachineKey, MobileError> {
    MachineKey::load_or_make(&folder.join(ENDPOINT_FOLDER))
        .map_err(|error| MobileError::new(error.to_string()))
}

/// The endpoint key a join declares: a new one, as its device key is, kept beside the one in use
/// until the join succeeded. The machine of a member it replaces may still be listed (a key the
/// Keystore lost), and a machine is one member's only.
pub(crate) fn pending_endpoint_key(folder: &Path) -> Result<MachineKey, MobileError> {
    MachineKey::load_or_make(&folder.join(ENDPOINT_PENDING))
        .map_err(|error| MobileError::new(error.to_string()))
}

/// The join succeeded: its endpoint key is the one in use.
pub(crate) fn promote_endpoint_key(folder: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(folder.join(ENDPOINT_FOLDER)) {
        Err(error) if error.kind() != ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    std::fs::rename(folder.join(ENDPOINT_PENDING), folder.join(ENDPOINT_FOLDER))
}
