//! The device key of this user session's clients, a member's (`docs/remote.md`, "Identity and
//! keys"): the window signs with it what the user does to the hub's list, and each request it
//! sends to another machine, as `flottille remote` does. It is the clients' key, never the
//! daemon's: in the session's keyring (`Flottille: device`), or `remote/device.key` without one.
//! The keyring is the binary's to choose and give; a test gives none, and the key is the file of
//! its temporary folder. No key is ever printed, logged or shown.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use flottille_link::{DeviceKey, KeyError, Keyring, KeyringError};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, String>;

/// Where this session's clients keep the device key: a keyring, and the key folder of this
/// machine (`$XDG_STATE_HOME/flottille/remote`).
#[derive(Clone)]
pub struct Device {
    keyring: Shared,
    folder: PathBuf,
}

impl Device {
    pub fn new(keyring: Arc<dyn Keyring + Send + Sync>, folder: PathBuf) -> Device {
        Device {
            keyring: Shared(keyring),
            folder,
        }
    }

    /// No keyring: the key is the file in `folder`, as on a machine with no Secret Service.
    pub fn in_folder(folder: PathBuf) -> Device {
        Device::new(Arc::new(NoKeyring), folder)
    }

    /// The key a member's device signs with.
    pub fn key(&self) -> Result<DeviceKey> {
        DeviceKey::load(&self.keyring, &self.folder)
            .map_err(said)?
            .ok_or_else(|| {
                "this device holds no key of a member: connect it to your hub".to_owned()
            })
    }

    /// The key a pairing is made with (`docs/remote.md`, "Identity and keys"): the one kept, made
    /// once; one a setup or a join of before pair mode left pending, kept now; or a new one, kept
    /// at once. A device paired again with its key finds its card on the hub.
    pub fn key_to_pair(&self) -> Result<DeviceKey> {
        if let Some(key) = DeviceKey::load(&self.keyring, &self.folder).map_err(said)? {
            return Ok(key);
        }
        let key = match DeviceKey::load_pending(&self.keyring, &self.folder).map_err(said)? {
            Some(pending) => pending,
            None => {
                let key = DeviceKey::make().map_err(said)?;
                let public = key.public();
                let pending = key
                    .keep_pending(&self.keyring, &self.folder)
                    .map_err(said)?;
                if pending.public() != public {
                    return Err(ANOTHER_CLIENT.to_owned());
                }
                pending
            }
        };
        let public = key.public();
        let kept = key.promote(&self.keyring, &self.folder).map_err(said)?;
        if kept.public() != public {
            return Err(ANOTHER_CLIENT.to_owned());
        }
        Ok(kept)
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

const ANOTHER_CLIENT: &str = "another client kept a device key at the same moment: Settings, \
                              General, Machines lists the members";

fn said(error: KeyError) -> String {
    error.to_string()
}

/// A keyring given by the binary, shared by the window's threads.
#[derive(Clone)]
struct Shared(Arc<dyn Keyring + Send + Sync>);

impl Keyring for Shared {
    fn read(
        &self,
        attributes: &[(&str, &str)],
    ) -> std::result::Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
        self.0.read(attributes)
    }

    fn store(
        &self,
        label: &str,
        attributes: &[(&str, &str)],
        secret: &[u8],
    ) -> std::result::Result<(), KeyringError> {
        self.0.store(label, attributes, secret)
    }

    fn delete(&self, attributes: &[(&str, &str)]) -> std::result::Result<(), KeyringError> {
        self.0.delete(attributes)
    }
}

/// No keyring: the key folder's file.
struct NoKeyring;

impl Keyring for NoKeyring {
    fn read(
        &self,
        _attributes: &[(&str, &str)],
    ) -> std::result::Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
        Err(KeyringError::Absent)
    }

    fn store(
        &self,
        _label: &str,
        _attributes: &[(&str, &str)],
        _secret: &[u8],
    ) -> std::result::Result<(), KeyringError> {
        Err(KeyringError::Absent)
    }

    fn delete(&self, _attributes: &[(&str, &str)]) -> std::result::Result<(), KeyringError> {
        Err(KeyringError::Absent)
    }
}
