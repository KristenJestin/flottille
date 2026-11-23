//! The root-owned files `ssh` includes, given to every confined command as its own (#1302,
//! `docs/commands.md`, "The environment"). A confined command's user namespace maps the user's
//! id alone (`mounts.rs`): a file root owns shows there as owned by the overflow id, and `ssh`
//! refuses a configuration it includes that is owned by neither root nor the user ("Bad owner or
//! permissions"), which stopped every git command that reaches an SSH remote. An unprivileged
//! process cannot map root into its namespace (that takes `newuidmap`, a setuid helper), so each
//! such file is copied, as it is when the command starts, into a file system the namespace makes,
//! read-only, and put over the file at its own path: the same name and text, owned by the user in
//! the namespace. The text is root's, which no run can write; it is read for that alone, and
//! never logged nor shown.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The machine's `ssh` configuration.
const SYSTEM_CONFIG: &str = "/etc/ssh/ssh_config";

/// The variable that plays the machine's configuration, read by a test build only.
const SYSTEM: &str = "FLOTTILLE_TEST_SSH_SYSTEM_CONFIG";

/// How deep `Include`s go, as `ssh` reads them.
const INCLUDES_MAX: usize = 16;
/// The files looked at, at most.
const FILES_MAX: usize = 256;
/// The files copied, at most.
const COPIES_MAX: usize = 64;
/// The bytes of a file read, at most.
const CONFIG_MAX: u64 = 1024 * 1024;

/// Each root-owned file the machine's configuration and the user's include, with its text now:
/// its real path (links followed, as `ssh` opens it) and what it holds.
pub fn copies() -> Vec<(PathBuf, Vec<u8>)> {
    let system = match std::env::var_os(SYSTEM).filter(|_| cfg!(debug_assertions)) {
        Some(system) => PathBuf::from(system),
        None => PathBuf::from(SYSTEM_CONFIG),
    };
    let account = crate::pushed::account();
    let mut files = Vec::new();
    // `ssh` reads a relative `Include` of the machine's configuration in its folder, of the
    // user's in `~/.ssh`.
    if let Some(folder) = system.parent() {
        included(&system, folder, account, 0, &mut files);
    }
    if let Some(account) = account {
        let ssh = account.join(".ssh");
        included(&ssh.join("config"), &ssh, Some(account), 0, &mut files);
    }
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut copies: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for file in files {
        let Ok(path) = file.canonicalize() else {
            continue;
        };
        if copies.len() >= COPIES_MAX || copies.iter().any(|(done, _)| *done == path) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.uid() != 0 || uid == 0 || meta.len() > CONFIG_MAX {
            continue;
        }
        if let Ok(text) = std::fs::read(&path) {
            copies.push((path, text));
        }
    }
    copies
}

/// Adds to `files` what `file` includes, and what that includes in turn.
fn included(
    file: &Path,
    folder: &Path,
    account: Option<&Path>,
    depth: usize,
    files: &mut Vec<PathBuf>,
) {
    if depth > INCLUDES_MAX || files.len() >= FILES_MAX {
        return;
    }
    let Ok(meta) = std::fs::metadata(file) else {
        return;
    };
    if !meta.is_file() || meta.len() > CONFIG_MAX {
        return;
    }
    let Ok(text) = std::fs::read(file) else {
        return;
    };
    for line in String::from_utf8_lossy(&text).lines() {
        let words = crate::pushed::ssh_words(line.trim());
        let Some((keyword, patterns)) = words.split_first() else {
            continue;
        };
        if !keyword.eq_ignore_ascii_case("include") {
            continue;
        }
        for pattern in patterns {
            for found in expanded(pattern, folder, account) {
                if files.len() >= FILES_MAX {
                    return;
                }
                files.push(found.clone());
                included(&found, folder, account, depth + 1, files);
            }
        }
    }
}

/// The files the `Include` pattern names, as `ssh` reads it: `~` the account's home, a relative
/// path in `folder`. A pattern with a token, a variable or a class of characters names none here.
fn expanded(pattern: &str, folder: &Path, account: Option<&Path>) -> Vec<PathBuf> {
    if pattern.contains(['$', '%', '[', '\\', '{']) {
        return Vec::new();
    }
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => match account {
            Some(account) => account.join(rest),
            None => return Vec::new(),
        },
        None if pattern.starts_with('~') => return Vec::new(),
        None => folder.join(pattern),
    };
    let rest = path.strip_prefix("/").unwrap_or(&path);
    flottille_core::glob::expand(Path::new("/"), &rest.to_string_lossy())
        .map(|found| {
            found
                .into_iter()
                .map(|rest| Path::new("/").join(rest))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_ssh_includes_is_followed_as_it_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().join("etc");
        std::fs::create_dir_all(etc.join("conf.d")).unwrap();
        let account = dir.path().join("home");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(
            etc.join("ssh_config"),
            "Host *\n  include conf.d/*.conf\nInclude=~/mine \"~other\" $X/y\n",
        )
        .unwrap();
        std::fs::write(etc.join("conf.d/10-a.conf"), "Include deeper\n").unwrap();
        std::fs::write(etc.join("conf.d/20-b.txt"), "").unwrap();
        std::fs::write(etc.join("deeper"), "").unwrap();
        std::fs::write(account.join("mine"), "").unwrap();

        let mut files = Vec::new();
        included(&etc.join("ssh_config"), &etc, Some(&account), 0, &mut files);

        assert_eq!(
            files,
            vec![
                etc.join("conf.d/10-a.conf"),
                etc.join("deeper"),
                account.join("mine"),
            ]
        );
    }

    #[test]
    fn an_include_of_itself_ends() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config"), "Include config\n").unwrap();

        let mut files = Vec::new();
        included(&dir.path().join("config"), dir.path(), None, 0, &mut files);

        assert_eq!(files.len(), INCLUDES_MAX + 1);
    }
}
