//! The user's SSH agent, kept out of every run (`docs/vault.md`, "The runtime folder"): no run
//! gets `SSH_AUTH_SOCK` (`runs::defend`), and Landlock keeps a socket outside the runtime folders
//! out of reach by its path (`landlock::confined_but`). A push the gate let through runs outside
//! the sandbox, with the daemon's own environment, and reaches it as the user would.

use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::sync::OnceLock;

pub const VARIABLE: &str = "SSH_AUTH_SOCK";

/// The socket of the user's SSH agent, as the daemon's own environment named it at its start,
/// resolved then.
static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Finds the user's SSH agent at the daemon's start, before anything runs.
pub fn find() {
    FOUND.get_or_init(|| {
        let path = PathBuf::from(std::env::var_os(VARIABLE)?)
            .canonicalize()
            .ok()?;
        let socket = std::fs::symlink_metadata(&path).ok()?;
        socket.file_type().is_socket().then_some(path)
    });
}

/// The socket found at the start, which every run is kept from.
pub fn socket() -> Option<&'static std::path::Path> {
    FOUND.get()?.as_deref()
}
