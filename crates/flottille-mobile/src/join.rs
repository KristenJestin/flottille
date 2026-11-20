//! Joining the user's hub: the one function that changes when the hub's pair mode lands (#1146).
//!
//! On `dev` today a device joins with an invitation a member made (`docs/remote.md`, "Pairing"):
//! its code (the hub's address, the hub's key, which is pinned, the token and the inviter) is what
//! the QR holds. The phone shows the four words, the member that invited compares them and adds
//! it; the phone then checks the list back to the root and to the inviter
//! (`Invitation::check_list`), declares its endpoint's key, and keeps its device key.
//!
//! With pair mode, the QR is the code `flottille-hub pair` prints (the hub's address and key, a
//! one-time code), the phone proposes its own name and role, and there are no four words: only
//! `start` and `Pairing::accepted` change, never the core around them.

use std::path::Path;
use std::sync::Arc;

use flottille_link::{Action, Change, Device, DeviceKey, Invitation, Joined, MachineKey, Tls};

use crate::MobileError;
use crate::keys::Sealed;
use crate::mobile::Inner;

/// A join on its way: the words to show, and the wait for the member's answer.
#[derive(uniffi::Object)]
pub struct Pairing {
    words: Vec<String>,
    started: tokio::sync::Mutex<Option<Started>>,
    inner: Arc<Inner>,
}

pub(crate) struct Started {
    joined: Joined,
    key: DeviceKey,
}

#[uniffi::export]
impl Pairing {
    /// The four words to compare with those the member that invited sees.
    pub fn words(&self) -> Vec<String> {
        self.words.clone()
    }

    /// Waits until the member accepted (or refused) the join; once accepted, the phone is a
    /// member, its endpoint declared and its device key kept.
    pub async fn accepted(&self) -> Result<(), MobileError> {
        let started = self
            .started
            .lock()
            .await
            .take()
            .ok_or_else(|| MobileError::new("this join was answered already"))?;
        let inner = self.inner.clone();
        self.inner
            .on(async move {
                let (device, key) =
                    finish(started, &inner.folder, &inner.sealed, &inner.endpoint).await?;
                inner.joined(device, key)
            })
            .await
    }
}

impl Pairing {
    pub(crate) fn new(words: Vec<String>, started: Started, inner: Arc<Inner>) -> Pairing {
        Pairing {
            words,
            started: tokio::sync::Mutex::new(Some(started)),
            inner,
        }
    }
}

/// Reads the QR's `code`, and asks the hub to join with a new device key, kept pending (sealed)
/// before the hub sees it: the words to show, and the join.
pub(crate) async fn start(
    folder: &Path,
    sealed: &Sealed,
    endpoint: &MachineKey,
    code: &str,
) -> Result<(Vec<String>, Started), MobileError> {
    let invitation =
        Invitation::from_code(code.trim()).map_err(|error| MobileError::new(error.to_string()))?;
    let failed = |error: flottille_link::KeyError| MobileError::new(error.to_string());
    let key = match DeviceKey::load_pending(sealed, folder).map_err(failed)? {
        Some(key) => key,
        None => DeviceKey::make()
            .and_then(|key| key.keep_pending(sealed, folder))
            .map_err(failed)?,
    };
    let words = invitation
        .words(&key.public())
        .iter()
        .map(|word| (*word).to_owned())
        .collect();
    let joined = Device::join(
        &invitation,
        &key.public(),
        endpoint,
        &crate::hello(),
        &Tls::System,
    )
    .await
    .map_err(|error| MobileError::new(error.to_string()))?;
    Ok((words, Started { joined, key }))
}

/// The member's answer: the list checked, the phone's endpoint declared through the session it
/// joined with, and its device key kept.
async fn finish(
    started: Started,
    folder: &Path,
    sealed: &Sealed,
    endpoint: &MachineKey,
) -> Result<(Device, DeviceKey), MobileError> {
    let Started { mut joined, key } = started;
    let said = |error: flottille_link::ControlError| MobileError::new(error.to_string());
    let (mut device, _) = joined.accepted().await.map_err(said)?;
    let declared = Change::sign(
        device.list.head(),
        Action::Machine {
            key: key.public(),
            machine: endpoint.public(),
        },
        &key,
    )
    .claimed_by(endpoint);
    device
        .submit(&mut joined.session, declared)
        .await
        .map_err(said)?;
    let key = key
        .promote(sealed, folder)
        .map_err(|error| MobileError::new(error.to_string()))?;
    Ok((device, key))
}
