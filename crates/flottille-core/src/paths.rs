//! Where Flottille keeps its files, from the XDG base directories.

use std::ffi::OsString;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct Paths {
    /// `$XDG_RUNTIME_DIR/flottille`: the socket and the lock.
    pub runtime: PathBuf,
    /// `$XDG_STATE_HOME/flottille`: the store and the daemon's log.
    pub state: PathBuf,
    /// `$XDG_CONFIG_HOME/flottille`: the configuration the user may edit by hand.
    pub config: PathBuf,
    /// `$XDG_DATA_HOME/flottille`: what Flottille makes for the user, such as workspaces.
    pub data: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Paths> {
        Paths::from_vars(|name| std::env::var_os(name))
    }

    fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<Paths> {
        // The specification ignores a relative path in any of these variables.
        let absolute = |name: &str| {
            var(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let runtime = absolute("XDG_RUNTIME_DIR").ok_or_else(|| {
            Error::Refused(
                "XDG_RUNTIME_DIR is not set to an absolute path: Flottille keeps its socket there"
                    .into(),
            )
        })?;
        // The specification's fallback for every other base directory: a folder of the home.
        let beside_home = |name: &str, fallback: &str| match absolute(name) {
            Some(base) => Ok(base),
            None => absolute("HOME")
                .map(|home| home.join(fallback))
                .ok_or_else(|| {
                    Error::Refused(format!(
                        "neither {name} nor HOME is set to an absolute path"
                    ))
                }),
        };
        Ok(Paths {
            runtime: runtime.join("flottille"),
            state: beside_home("XDG_STATE_HOME", ".local/state")?.join("flottille"),
            config: beside_home("XDG_CONFIG_HOME", ".config")?.join("flottille"),
            data: beside_home("XDG_DATA_HOME", ".local/share")?.join("flottille"),
        })
    }

    pub fn socket(&self) -> PathBuf {
        self.runtime.join("daemon.sock")
    }

    pub fn lock(&self) -> PathBuf {
        self.runtime.join("daemon.lock")
    }

    /// Held by the daemon that works on the store, for its whole life.
    pub fn store_lock(&self) -> PathBuf {
        self.state.join("store.lock")
    }

    pub fn database(&self) -> PathBuf {
        self.state.join("flottille.db")
    }

    pub fn log(&self) -> PathBuf {
        self.state.join("daemon.log")
    }

    /// How the user wants agents to work, for every project (`docs/projects.md`, "Rules"); absent
    /// while they keep Flottille's default.
    pub fn rules(&self) -> PathBuf {
        self.config.join("rules.md")
    }

    /// One TOML file per project.
    pub fn projects(&self) -> PathBuf {
        self.config.join("projects")
    }
}

/// Creates `path` (and its missing parents) readable by the user only, then checks that what is
/// there is a real folder, owned by the user and closed to everyone else.
pub fn ensure_private_dir(path: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    let shown = path.display();
    if !metadata.is_dir() {
        return Err(Error::Refused(format!("{shown} is not a folder")));
    }
    // SAFETY: getuid has no precondition and cannot fail.
    let uid = unsafe { libc::getuid() };
    if metadata.uid() != uid {
        return Err(Error::Refused(format!("{shown} is not owned by you")));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(Error::Refused(format!(
            "{shown} is open to other users (mode {mode:o}): make it yours only with `chmod 700 {shown}`"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.into())
    }

    #[test]
    fn the_folders_come_from_xdg() {
        let paths = Paths::from_vars(vars(&[
            ("XDG_RUNTIME_DIR", "/run/u"),
            ("XDG_STATE_HOME", "/s"),
            ("HOME", "/h"),
        ]))
        .unwrap();
        assert_eq!(paths.socket(), Path::new("/run/u/flottille/daemon.sock"));
        assert_eq!(paths.database(), Path::new("/s/flottille/flottille.db"));
    }

    #[test]
    fn the_state_falls_back_to_the_home() {
        let paths = Paths::from_vars(vars(&[
            ("XDG_RUNTIME_DIR", "/run/u"),
            ("XDG_STATE_HOME", "relative"),
            ("HOME", "/h"),
        ]))
        .unwrap();
        assert_eq!(paths.state, Path::new("/h/.local/state/flottille"));
    }

    #[test]
    fn the_configuration_and_the_data_come_from_xdg_or_the_home() {
        let paths = Paths::from_vars(vars(&[
            ("XDG_RUNTIME_DIR", "/run/u"),
            ("XDG_CONFIG_HOME", "/c"),
            ("HOME", "/h"),
        ]))
        .unwrap();
        assert_eq!(paths.projects(), Path::new("/c/flottille/projects"));
        assert_eq!(paths.data, Path::new("/h/.local/share/flottille"));
    }

    #[test]
    fn a_missing_runtime_folder_is_refused() {
        let error = Paths::from_vars(vars(&[("HOME", "/h")])).unwrap_err();
        assert!(error.to_string().contains("XDG_RUNTIME_DIR"));
    }

    #[test]
    fn a_private_folder_is_created_for_the_user_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b");
        ensure_private_dir(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        ensure_private_dir(&path).unwrap();
    }

    #[test]
    fn a_link_in_place_of_the_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let error = ensure_private_dir(&dir.path().join("link")).unwrap_err();
        assert!(error.to_string().contains("not a folder"));
    }
}
