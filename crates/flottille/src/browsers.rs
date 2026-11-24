//! The profiles of the connections' browsers (`docs/extensions.md`, "Adapters"): their cookies
//! and sign-ins, in a folder of Flottille's state that every command it confines is kept out of
//! (`landlock.rs`), but the server of the connection whose browser it is and its login, and that
//! the gate holds for a login's place (`gate.rs`).

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use flottille_core::paths::{Paths, ensure_private_dir};

/// The folder of Flottille's state that holds them, one folder per connection.
pub const FOLDER: &str = "browsers";

/// Where it is, found at the daemon's start: the place every confined command is kept out of.
static BASE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The folders on the way to it a command could rename, found with it: a run that renamed one
/// would have the next run, whose namespace empties the folder at the path found here, reach the
/// profiles under the new name.
static WAYS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Makes the folder at the daemon's start, before anything runs, and keeps where it is.
pub fn find(paths: &Paths) -> flottille_core::Result<()> {
    let base = paths.state.join(FOLDER);
    ensure_private_dir(&base)?;
    let base = BASE.get_or_init(|| base.canonicalize().ok());
    WAYS.get_or_init(|| {
        base.as_deref()
            .map(crate::pushed::on_the_way)
            .unwrap_or_default()
    });
    Ok(())
}

/// The folder found at the daemon's start, if it was.
pub fn base() -> Option<&'static Path> {
    BASE.get()?.as_deref()
}

/// The folders on the way to [`base`] that every confined command has kept where they are, as the
/// ones on the way to `~/.ssh` (`pushed::pinned`).
pub fn pinned() -> &'static [PathBuf] {
    WAYS.get().map(Vec::as_slice).unwrap_or_default()
}

/// The folder of one connection's browser. Only this module makes one, so that a folder named by
/// anything else (an environment variable, a request) is never given back to a confined command
/// (`landlock::confined_browser` accepts no other).
#[derive(Clone, Debug)]
pub struct Folder(PathBuf);

impl Folder {
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The browser's profile, which agent-browser makes in it.
    pub fn profile(&self) -> PathBuf {
        self.0.join("profile")
    }
}

/// The folder of the browser of `connection`, made if it is not there yet and never made again
/// while it is: a browser that runs was confined with it, which Landlock holds by its inode
/// (#1040). Refused when the daemon did not find the browsers' folder at its start, since no
/// command would then be kept out of it.
pub fn folder(paths: &Paths, connection: &str) -> Result<Folder, String> {
    if connection.is_empty()
        || !connection
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("{connection:?} is no name for a browser's folder"));
    }
    let base = paths.state.join(FOLDER);
    ensure_private_dir(&base).map_err(|error| error.to_string())?;
    if base.canonicalize().ok().as_deref() != self::base() {
        return Err(format!(
            "{} is not the folder of the browsers the daemon keeps out of the runs' reach",
            base.display()
        ));
    }
    let folder = base.join(connection);
    match std::fs::DirBuilder::new().mode(0o700).create(&folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("{}: {error}", folder.display())),
    }
    // A folder, never a link: what it leads to is not what the commands are given.
    let found = std::fs::symlink_metadata(&folder).map_err(|error| error.to_string())?;
    if !found.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    folder
        .canonicalize()
        .map(Folder)
        .map_err(|error| format!("{}: {error}", folder.display()))
}

/// Removes the folder of the browser of `connection`, its profile with it: called once the
/// connection is removed and its browser closed.
pub fn remove(paths: &Paths, connection: &str) {
    if connection.is_empty() || connection.contains(['/', '.']) {
        return;
    }
    let _ = std::fs::remove_dir_all(paths.state.join(FOLDER).join(connection));
}
