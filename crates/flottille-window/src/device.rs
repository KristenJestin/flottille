//! The device key of this user session's clients, a member's (`docs/remote.md`, "Identity and
//! keys"): the window signs with it what the user does to the hub's list, and each request it
//! sends to another machine, as `flottille remote` does. It is the clients' key, never the
//! daemon's: in the session's keyring (`Flottille: device`), or `remote/device.key` without one.
//! The keyring is the binary's to choose and give; a test gives none, and the key is the file of
//! its temporary folder. No key is ever printed, logged or shown.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use flottille_link::{DeviceKey, KeyError, Keyring, KeyringError};
use serde_json::{Value, json};
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
                "this device holds no key of a member: set up a hub, or join one".to_owned()
            })
    }

    /// The key a setup is made with: the one kept, or a new one, kept as pending before the hub
    /// sees it and kept only once the hub took it ([`Device::keep_new`]); whether it is new.
    pub fn key_to_join(&self) -> Result<(DeviceKey, bool)> {
        if let Some(key) = DeviceKey::load(&self.keyring, &self.folder).map_err(said)? {
            return Ok((key, false));
        }
        let key = DeviceKey::make().map_err(said)?;
        let public = key.public();
        let pending = key
            .keep_pending(&self.keyring, &self.folder)
            .map_err(said)?;
        if pending.public() != public {
            return Err(ANOTHER_CLIENT.to_owned());
        }
        Ok((pending, true))
    }

    /// Keeps `key`, the pending key a setup succeeded with.
    pub fn keep_new(&self, key: DeviceKey) -> Result<()> {
        let public = key.public();
        let kept = key.promote(&self.keyring, &self.folder).map_err(said)?;
        if kept.public() != public {
            return Err(ANOTHER_CLIENT.to_owned());
        }
        Ok(())
    }

    /// The pending key a setup or a join left when it did not end: kept when the hub's list holds
    /// it as a member, removed otherwise, so that a key the hub saw in a setup that did not
    /// succeed is never sent again (`flottille remote`'s own rule). A join the daemon accepted
    /// for it is left to `flottille remote join`, which declares the machine.
    pub fn settle_pending(&self, ask: &mut impl FnMut(&str, Value) -> Result<Value>) -> Result<()> {
        let Some(pending) = DeviceKey::load_pending(&self.keyring, &self.folder).map_err(said)?
        else {
            return Ok(());
        };
        let key = pending.public();
        let accepted = ask("remote.join", json!({"key": key, "resume": true}))?;
        if !accepted.is_null() {
            return Err(
                "a join of this device waits for its machine to be declared: run \
                        flottille remote join to end it"
                    .to_owned(),
            );
        }
        let listed = !ask("remote.machines", json!({}))?["hub"].is_null()
            && ask("remote.members", json!({}))?["members"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|member| member["key"] == json!(key));
        if listed {
            self.keep_new(pending)
        } else {
            DeviceKey::discard_pending(&self.keyring, &self.folder).map_err(said)
        }
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
