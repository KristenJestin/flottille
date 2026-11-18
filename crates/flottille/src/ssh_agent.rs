//! The user's SSH agent, kept out of every run (`docs/vault.md`, "The runtime folder"): no run
//! gets `SSH_AUTH_SOCK` (`runs::defend`), and Landlock keeps a socket outside the runtime folders
//! out of reach by its path (`landlock::confined_but`). A push the gate let through runs outside
//! the sandbox, with the daemon's own environment, and reaches it as the user would. The daemon's
//! own fetch or listing of an SSH remote is lent it, with the `ssh` found at the start
//! (`git::remote`).

use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::sync::OnceLock;

pub const VARIABLE: &str = "SSH_AUTH_SOCK";

/// The socket of the user's SSH agent, as the daemon's own environment named it at its start,
/// resolved then.
static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The `ssh` the daemon's own `PATH` led to at its start: what the daemon's git runs, whatever a
/// repository's settings name. Not resolved through its links: a multi-call program reads its
/// name.
static SSH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Finds the user's SSH agent and `ssh` at the daemon's start, before anything runs.
pub fn find() {
    SSH.get_or_init(|| crate::providers::find_program("ssh"));
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

/// The `ssh` found at the start.
pub fn ssh() -> Option<&'static std::path::Path> {
    SSH.get()?.as_deref()
}
