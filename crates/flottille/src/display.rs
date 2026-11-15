//! A runtime folder of its own for a program that opens a window on the user's screen: it holds
//! the session's Wayland socket alone (a hard link of it), so the program is given the compositor
//! and nothing else of the user's real `$XDG_RUNTIME_DIR`: not the session bus's address, nor the
//! other sockets. What keeps it from the real folder itself is Landlock's (`landlock.rs`), which
//! leaves this folder alone within its reach. A connection's browser uses it
//! (`docs/extensions.md`), and a run that opens windows will.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use flottille_core::paths::Paths;

/// The folder of one owner, removed when this is dropped: the program that used it has stopped.
#[derive(Debug)]
pub struct Folder(PathBuf);

impl Folder {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What a program that shows a window is given: its folder, kept while it runs, and the
/// variables of its environment (`XDG_RUNTIME_DIR` is the folder, then `WAYLAND_DISPLAY`, or
/// `DISPLAY` when the session has no Wayland).
#[derive(Debug)]
pub struct Shown {
    pub folder: Folder,
    pub env: BTreeMap<OsString, OsString>,
}

/// Removes every display folder, left by a daemon that died. Called by a daemon that holds the
/// lock, when it starts: nothing else makes them, so none is in use.
pub fn clean(paths: &Paths) {
    let _ = std::fs::remove_dir_all(paths.runtime.join("display"));
}

/// The folder `$XDG_RUNTIME_DIR/flottille/display/<owner>` (mode 0700, made again if it was left),
/// with the daemon's own session. `owner` is a connection's name or the like: letters, digits,
/// `-` and `_`. The error says why nothing is shown, and the caller then runs without a display,
/// never with the real folder.
pub fn share(paths: &Paths, owner: &str) -> Result<Shown, String> {
    share_with(
        &paths.runtime.join("display"),
        owner,
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        std::env::var_os("WAYLAND_DISPLAY"),
        std::env::var_os("DISPLAY"),
    )
}

/// [`share`] under `base`, with the session given.
fn share_with(
    base: &Path,
    owner: &str,
    real: Option<PathBuf>,
    wayland: Option<OsString>,
    x11: Option<OsString>,
) -> Result<Shown, String> {
    if owner.is_empty()
        || !owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("{owner:?} is no name for a display folder"));
    }
    let folder = base.join(owner);
    // The folders above are made private and checked; the last one is made here and now, never
    // taken over: one that is there (a link planted in it) fails the creation.
    flottille_core::paths::ensure_private_dir(base).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&folder)
        .map_err(|error| error.to_string())?;
    let folder = Folder(folder);
    let mut env = BTreeMap::new();
    env.insert("XDG_RUNTIME_DIR".into(), folder.0.clone().into_os_string());
    match (wayland, x11) {
        (Some(name), _) => {
            let socket = match Path::new(&name).is_absolute() {
                true => PathBuf::from(&name),
                false => real
                    .ok_or("the session has no runtime folder to find its Wayland socket in")?
                    .join(&name),
            };
            let file = socket
                .file_name()
                .ok_or("WAYLAND_DISPLAY names no socket")?;
            std::fs::hard_link(&socket, folder.0.join(file)).map_err(|error| {
                format!(
                    "the Wayland socket {} cannot be shared with a hard link (the filesystem \
                     refuses it, or the socket is not there): {error}",
                    socket.display()
                )
            })?;
            env.insert("WAYLAND_DISPLAY".into(), file.to_owned());
        }
        (None, Some(display)) => {
            env.insert("DISPLAY".into(), display);
        }
        (None, None) => return Err("the session has no display".into()),
    }
    Ok(Shown { folder, env })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_folder_holds_the_wayland_socket_alone() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let _socket = UnixListener::bind(real.join("wayland-9")).unwrap();
        std::fs::write(real.join("bus"), "").unwrap();
        let shown = share_with(
            &temp.path().join("display"),
            "browser",
            Some(real.clone()),
            Some("wayland-9".into()),
            Some(":0".into()),
        )
        .unwrap();
        let folder = temp.path().join("display/browser");
        assert_eq!(names(&folder), ["wayland-9"]);
        assert_eq!(
            std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            shown.env[&OsString::from("XDG_RUNTIME_DIR")],
            folder.as_os_str()
        );
        assert_eq!(shown.env[&OsString::from("WAYLAND_DISPLAY")], "wayland-9");
        // X11 only when there is no Wayland; no bus.
        assert_eq!(shown.env.len(), 2);
        drop(shown);
        assert!(!folder.exists());
    }

    #[test]
    fn x11_is_given_only_without_wayland() {
        let temp = tempfile::tempdir().unwrap();
        let shown = share_with(
            &temp.path().join("display"),
            "browser",
            None,
            None,
            Some(":3".into()),
        )
        .unwrap();
        assert_eq!(shown.env[&OsString::from("DISPLAY")], ":3");
        assert!(!shown.env.contains_key(&OsString::from("WAYLAND_DISPLAY")));
        assert!(names(&temp.path().join("display/browser")).is_empty());
    }

    #[test]
    fn a_socket_that_cannot_be_linked_is_an_error_and_leaves_no_folder() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("display");
        let error = share_with(
            &base,
            "browser",
            Some(temp.path().to_owned()),
            Some("wayland-none".into()),
            None,
        )
        .unwrap_err();
        assert!(error.contains("hard link"), "{error}");
        assert!(!base.join("browser").exists());
    }

    #[test]
    fn a_link_or_a_folder_planted_at_the_owner_is_never_taken_over() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("display");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&base)
            .unwrap();
        // A link to a folder of someone's choosing: it is replaced, what it leads to untouched.
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("kept"), "").unwrap();
        std::os::unix::fs::symlink(&elsewhere, base.join("browser")).unwrap();
        let shown = share_with(&base, "browser", None, None, Some(":0".into())).unwrap();
        let folder = base.join("browser");
        let kind = std::fs::symlink_metadata(&folder).unwrap();
        assert!(kind.is_dir() && !kind.file_type().is_symlink());
        assert_eq!(kind.permissions().mode() & 0o777, 0o700);
        assert!(names(&folder).is_empty());
        assert_eq!(names(&elsewhere), ["kept"]);
        drop(shown);
        assert_eq!(names(&elsewhere), ["kept"]);
        // A folder left there, open to others and holding a socket: made again, private and empty.
        std::fs::DirBuilder::new()
            .mode(0o777)
            .create(&folder)
            .unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o777)).unwrap();
        let _planted = UnixListener::bind(folder.join("wayland-0")).unwrap();
        let shown = share_with(&base, "browser", None, None, Some(":0".into())).unwrap();
        assert_eq!(
            std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(names(&folder).is_empty());
        drop(shown);
    }

    #[test]
    fn an_owner_is_a_plain_name() {
        let temp = tempfile::tempdir().unwrap();
        for owner in ["", "../x", "a/b"] {
            assert!(share_with(temp.path(), owner, None, None, Some(":0".into())).is_err());
        }
    }
}
