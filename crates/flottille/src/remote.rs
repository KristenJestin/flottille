//! Remote (`docs/remote.md`): the daemon's side of it, off until the user turns it on in the
//! daemon's configuration (`daemon.toml`, `[remote] on = true`). Off, nothing of it is made or
//! read.

use std::net::SocketAddr;
use std::path::PathBuf;

use flottille_core::paths::Paths;
use flottille_link::{KeyError, MachineKey};

/// The folder of Flottille's state that holds its keys: a login's place, which no agent reaches
/// (`gate::keys`).
pub const FOLDER: &str = "remote";

/// The link's port when `daemon.toml` names none, and the port of an address given without one.
pub const PORT: u16 = 7447;

/// Whether the user turned remote on: off when `daemon.toml`, its table `remote` or its switch
/// `on` is missing, or is not a boolean.
pub fn on(paths: &Paths) -> bool {
    std::fs::read_to_string(paths.config.join("daemon.toml"))
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|document| document.get("remote")?.get("on")?.as_bool())
        .unwrap_or(false)
}

/// Where the link listens: `[remote] listen` in `daemon.toml`, an IP address and a port; every
/// interface on the link's port when it says none.
pub fn listen(paths: &Paths) -> Result<SocketAddr, String> {
    let Some(text) = std::fs::read_to_string(paths.config.join("daemon.toml"))
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|document| Some(document.get("remote")?.get("listen")?.as_str()?.to_owned()))
    else {
        return Ok(SocketAddr::from(([0, 0, 0, 0], PORT)));
    };
    text.parse().map_err(|_| {
        format!(
            "[remote] listen = \"{text}\" in daemon.toml is not an IP address and a port \
             (0.0.0.0:{PORT}, 100.64.0.7:{PORT})"
        )
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
