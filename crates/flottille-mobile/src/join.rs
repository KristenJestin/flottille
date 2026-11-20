//! Joining the user's hub: its pair mode (#1146, `docs/remote.md`, "Pairing").
//!
//! The QR is the code `flottille-hub pair` prints: the hub's address, its key and a one-time
//! secret. The phone pins the hub's key from it (a hub that answers with another is refused before
//! anything is proved), proposes its own card (its name and role, as the user confirmed them; the
//! phone's icon; the colour of its key), and proves it holds the secret, which never crosses the
//! network. The hub signs its entry; the phone checks the list back to the hub's key and its root,
//! declares its endpoint as its machine, and keeps its device key, pending (sealed) until then.

use std::path::Path;

use flottille_link::{
    Action, Change, Colour, Device, DeviceKey, Icon, MachineKey, PairCode, Proposed, Role, Tls,
};

use crate::MobileError;
use crate::keys::Sealed;

/// Pairs with the hub of the QR's `code` as `name` with `role` (`controller` or `viewer`): the
/// hub as pinned, with the list checked, and the device key kept.
pub(crate) async fn pair(
    folder: &Path,
    sealed: &Sealed,
    endpoint: &MachineKey,
    code: &str,
    name: &str,
    role: &str,
) -> Result<(Device, DeviceKey), MobileError> {
    let code =
        PairCode::from_code(code.trim()).map_err(|error| MobileError::new(error.to_string()))?;
    let role = match Role::from_word(role) {
        Some(role @ (Role::Controller | Role::Viewer)) => role,
        _ => {
            return Err(MobileError::new(format!(
                "a phone is a controller or a viewer, not {}",
                role.escape_debug()
            )));
        }
    };
    let failed = |error: flottille_link::KeyError| MobileError::new(error.to_string());
    let key = match DeviceKey::load_pending(sealed, folder).map_err(failed)? {
        Some(key) => key,
        None => DeviceKey::make()
            .and_then(|key| key.keep_pending(sealed, folder))
            .map_err(failed)?,
    };
    let proposed = Proposed {
        key: key.public(),
        name: name.trim().to_owned(),
        role,
        icon: Icon::Phone,
        colour: Colour::of(&key.public()),
    };
    let said = |error: flottille_link::ControlError| MobileError::new(error.to_string());
    let mut paired = Device::pair(&code, proposed, endpoint, &crate::hello(), &Tls::System)
        .await
        .map_err(said)?;
    let (mut device, _) = paired.listed().await.map_err(said)?;
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
        .submit(&mut paired.session, declared)
        .await
        .map_err(said)?;
    let key = key.promote(sealed, folder).map_err(failed)?;
    Ok((device, key))
}
