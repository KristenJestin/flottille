//! The user's SSH agent, kept out of every run (`docs/vault.md`, "The runtime folder"): no run
//! gets `SSH_AUTH_SOCK` (`runs::defend`), and Landlock keeps a socket outside the runtime folders
//! out of reach by its path (`landlock::confined_but`). A push the gate let through runs outside
//! the sandbox, with the daemon's own environment, and reaches it as the user would. The daemon's
//! own fetch or listing of an SSH remote is lent it, with the `ssh` found at the start
//! (`git::remote`).

use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const VARIABLE: &str = "SSH_AUTH_SOCK";

/// The socket of the user's SSH agent, as the daemon's own environment named it at its start,
/// resolved then.
static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The `ssh` the daemon's own `PATH` led to at its start, its links followed: what the daemon's
/// git runs, whatever a repository's settings name, and whatever a link is later pointed at.
static SSH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Finds the user's SSH agent and `ssh` at the daemon's start, before anything runs.
pub fn find() {
    SSH.get_or_init(|| {
        let (ssh, rooted) = chosen(candidates("ssh"))?;
        if !rooted {
            eprintln!(
                "flottille daemon: no ssh of the PATH is owned by root in a folder owned by \
                 root: the daemon's git runs {}",
                ssh.display()
            );
        }
        Some(ssh)
    });
    FOUND.get_or_init(|| {
        let path = PathBuf::from(std::env::var_os(VARIABLE)?)
            .canonicalize()
            .ok()?;
        let socket = std::fs::symlink_metadata(&path).ok()?;
        socket.file_type().is_socket().then_some(path)
    });
}

/// Every `name` of the daemon's `PATH` (`ssh`, `git`), in its order, each the program its links
/// lead to.
pub(crate) fn candidates(name: &str) -> Vec<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .filter_map(|dir| dir.join(name).canonicalize().ok())
        .filter(|ssh| {
            std::fs::metadata(ssh).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .collect()
}

/// The first of `candidates` that only root may change (its file and its folder are root's),
/// else the first, said not to be (`false`): open by default, as the user's own `PATH` is.
pub(crate) fn chosen(candidates: Vec<PathBuf>) -> Option<(PathBuf, bool)> {
    let root = |path: &Path| std::fs::metadata(path).is_ok_and(|m| m.uid() == 0);
    if let Some(rooted) = candidates
        .iter()
        .find(|ssh| root(ssh) && ssh.parent().is_some_and(root))
    {
        return Some((rooted.clone(), true));
    }
    candidates.into_iter().next().map(|ssh| (ssh, false))
}

/// The socket found at the start, which every run is kept from.
pub fn socket() -> Option<&'static std::path::Path> {
    FOUND.get()?.as_deref()
}

/// The `ssh` found at the start.
pub fn ssh() -> Option<&'static std::path::Path> {
    SSH.get()?.as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ssh_owned_by_root_is_chosen_over_one_earlier_in_the_path() {
        let sh = Path::new("/bin/sh").canonicalize().unwrap();
        // SAFETY: getuid has no precondition.
        if unsafe { libc::getuid() } == 0 || std::fs::metadata(&sh).unwrap().uid() != 0 {
            eprintln!("skipped: the tests run as root, or /bin/sh is not root's");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let users = dir.path().join("ssh");
        std::fs::write(&users, "").unwrap();

        assert_eq!(chosen(vec![users.clone(), sh.clone()]), Some((sh, true)));
        assert_eq!(chosen(vec![users.clone()]), Some((users, false)));
        assert_eq!(chosen(Vec::new()), None);
    }
}
