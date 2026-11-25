//! Remote (`docs/remote.md`): the daemon's side of it, off until the user turns it on in the
//! daemon's configuration (`daemon.toml`, `[remote] on = true`). Off, nothing of it is made or
//! read.

use std::path::PathBuf;

use flottille_core::paths::Paths;
use flottille_link::{KeyError, MachineKey};

/// The folder of Flottille's state that holds its keys: a login's place, which no agent reaches
/// (`gate::keys`).
pub const FOLDER: &str = "remote";

/// Whether the user turned remote on: off when `daemon.toml`, its table `remote` or its switch
/// `on` is missing, or is not a boolean.
pub fn on(paths: &Paths) -> bool {
    std::fs::read_to_string(paths.config.join("daemon.toml"))
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|document| document.get("remote")?.get("on")?.as_bool())
        .unwrap_or(false)
}

/// Whether the phones are told even while the user is at a client (`[remote] notify =
/// "always"`); by default they are told only while the user is away (`"away"`).
pub fn notify_always(paths: &Paths) -> bool {
    notify(paths) == "always"
}

/// What `daemon.toml` says of the phones' notifications: `always` or `away`, the default.
pub fn notify(paths: &Paths) -> &'static str {
    let said = std::fs::read_to_string(paths.config.join("daemon.toml"))
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|document| Some(document.get("remote")?.get("notify")?.as_str()? == "always"));
    match said {
        Some(true) => "always",
        _ => "away",
    }
}

/// Whether `daemon.toml` still holds `[remote] listen`, which the direct link removed on
/// 2026-11-21 read: the key is left in the user's file, and no longer read.
pub fn says_listen(paths: &Paths) -> bool {
    std::fs::read_to_string(paths.config.join("daemon.toml"))
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .is_some_and(|document| {
            document
                .get("remote")
                .and_then(|remote| remote.get("listen"))
                .is_some()
        })
}

/// The machine's key folder, `$XDG_STATE_HOME/flottille/remote`.
pub fn folder(paths: &Paths) -> PathBuf {
    paths.state.join(FOLDER)
}

/// At the daemon's start, with remote on: the machine key, made at the first start and read back
/// at every other. A key others may read is refused, and so is the start.
pub fn machine_key(paths: &Paths) -> Result<Option<MachineKey>, KeyError> {
    if !on(paths) {
        return Ok(None);
    }
    let key = MachineKey::load_or_make(&folder(paths))?;
    eprintln!(
        "flottille daemon: remote is on; this machine's key is {}",
        key.public().fingerprint()
    );
    Ok(Some(key))
}
