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
use std::sync::atomic::{AtomicBool, Ordering};

/// The machine's `ssh` configuration.
const SYSTEM_CONFIG: &str = "/etc/ssh/ssh_config";

/// The variable that plays the machine's configuration, read by a test build only.
pub const SYSTEM: &str = "FLOTTILLE_TEST_SSH_SYSTEM_CONFIG";

/// The variable naming a file while which the copies fail, read by a test build only: a copy is
/// then asked over a path below that file, which no file system can be mounted on.
const FAILING: &str = "FLOTTILLE_TEST_SSH_COPY_FAILS";

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
    // A command's child cannot log: a failure of one is said here, at a later start, once.
    static SAID: AtomicBool = AtomicBool::new(false);
    if let Some(error) = crate::mounts::copies_failed()
        && !SAID.swap(true, Ordering::Relaxed)
    {
        eprintln!(
            "flottille daemon: a command started without the root-owned files ssh includes, \
             which ssh then refuses ({error})"
        );
    }
    // SAFETY: getuid cannot fail.
    if unsafe { libc::getuid() } == 0 {
        return Vec::new();
    }
    let system = match std::env::var_os(SYSTEM).filter(|_| cfg!(debug_assertions)) {
        Some(system) => PathBuf::from(system),
        None => PathBuf::from(SYSTEM_CONFIG),
    };
    let mut copies = copies_of(&system, crate::pushed::account(), 0);
    if let Some(failing) = std::env::var_os(FAILING).filter(|_| cfg!(debug_assertions))
        && Path::new(&failing).is_file()
    {
        copies.insert(0, (Path::new(&failing).join("ssh_config"), Vec::new()));
    }
    copies
}

/// The files `owner` owns that `system` and the `account`'s `~/.ssh/config` include, with their
/// text: root's at every start, the test's own in a test.
fn copies_of(system: &Path, account: Option<&Path>, owner: u32) -> Vec<(PathBuf, Vec<u8>)> {
    let mut walk = Walk {
        account,
        owner,
        ssh: None,
        seen: 0,
        copies: Vec::new(),
    };
    // `ssh` reads a relative `Include` of the machine's configuration in its folder, of the
    // user's in `~/.ssh`.
    if let Some(folder) = system.parent() {
        walk.visit(system, folder, 0);
    }
    if let Some(account) = account {
        let ssh = account.join(".ssh");
        walk.ssh = ssh.canonicalize().ok().map(|real| (ssh, real));
        if let Some((ssh, _)) = walk.ssh.clone() {
            walk.visit(&ssh.join("config"), &ssh, 0);
        }
    }
    walk.copies
}

/// What the walk of `copies_of` leaves to `ssh`'s refusal, in the user's words.
fn gaps_of(_system: &Path, _account: Option<&Path>, _owner: u32) -> Vec<String> {
    Vec::new()
}

/// A walk of the files `ssh` includes. What a confined run can write never steers it: the
/// machine's configuration is followed only through files `ssh` reads, root's (or the walk's
/// first), and the user's only inside `~/.ssh`, read-only to a confined run, as the lent push
/// reads it (`pushed::includes`). A run in Full writes `~/.ssh` as the user does: what it names
/// there is still given only when root owns it, a file the user reads already. Each file is
/// opened without following a link nor waiting on a FIFO, and judged on what was opened.
struct Walk<'a> {
    account: Option<&'a Path>,
    owner: u32,
    /// `~/.ssh` and its real path, once the user's configuration is walked.
    ssh: Option<(PathBuf, PathBuf)>,
    seen: usize,
    copies: Vec<(PathBuf, Vec<u8>)>,
}

impl Walk<'_> {
    /// Gives `file` when `ssh` would read it and it is the owner's, and follows what it includes.
    fn visit(&mut self, file: &Path, folder: &Path, depth: usize) {
        if depth > INCLUDES_MAX || self.seen >= FILES_MAX {
            return;
        }
        self.seen += 1;
        let Ok(path) = file.canonicalize() else {
            return;
        };
        if let Some((_, real)) = &self.ssh
            && !path.starts_with(real)
        {
            return;
        }
        let Some((meta, text)) = read(&path) else {
            return;
        };
        // As `ssh` judges an included file: owned by root or the user, written by no one else.
        let read_by_ssh = meta.mode() & 0o022 == 0;
        let given = depth > 0 && read_by_ssh && meta.uid() == self.owner;
        if given
            && self.copies.len() < COPIES_MAX
            && !self.copies.iter().any(|(done, _)| *done == path)
        {
            self.copies.push((path.clone(), text.clone()));
        }
        if self.ssh.is_none() && depth > 0 && !given {
            return;
        }
        for line in String::from_utf8_lossy(&text).lines() {
            let words = crate::pushed::ssh_words(line.trim());
            let Some((keyword, patterns)) = words.split_first() else {
                continue;
            };
            if !keyword.eq_ignore_ascii_case("include") {
                continue;
            }
            for pattern in patterns {
                for found in self.expanded(pattern, folder) {
                    if self.seen >= FILES_MAX {
                        return;
                    }
                    self.visit(&found, folder, depth + 1);
                }
            }
        }
    }

    /// The files the `Include` pattern names, as `ssh` reads it: `~` the account's home, a
    /// relative path in `folder`; in the user's configuration, inside `~/.ssh` alone. A pattern
    /// with a token, a variable or a class of characters names none here.
    fn expanded(&self, pattern: &str, folder: &Path) -> Vec<PathBuf> {
        if let (Some((ssh, _)), Some(account)) = (&self.ssh, self.account) {
            return crate::pushed::includes(pattern, ssh, account).unwrap_or_default();
        }
        if pattern.contains(['$', '%', '[', '\\', '{']) {
            return Vec::new();
        }
        let path = match pattern.strip_prefix("~/") {
            Some(rest) => match self.account {
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
}

/// What the regular file at `path`, a real path, holds, with what it was when opened: none for a
/// link put there since, a FIFO, a device, a folder or a file past the size read.
fn read(path: &Path) -> Option<(std::fs::Metadata, Vec<u8>)> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > CONFIG_MAX {
        return None;
    }
    let mut text = Vec::new();
    file.take(CONFIG_MAX + 1).read_to_end(&mut text).ok()?;
    (text.len() as u64 <= CONFIG_MAX).then_some((meta, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn what_ssh_includes_is_followed_as_it_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().canonicalize().unwrap().join("etc");
        let account = dir.path().canonicalize().unwrap().join("home");
        file(
            &etc.join("ssh_config"),
            "Host *\n  include conf.d/*.conf\nInclude=~/mine \"~other\" $X/y\n",
            0o644,
        );
        file(&etc.join("conf.d/10-a.conf"), "Include deeper\n", 0o644);
        file(&etc.join("conf.d/20-b.txt"), "", 0o644);
        file(&etc.join("deeper"), "", 0o644);
        file(&account.join("mine"), "", 0o644);

        let copies = copies_of(&etc.join("ssh_config"), Some(&account), me());

        // `ssh` refuses a `~` in the machine's configuration (a "bad include path", and the
        // configuration with it): the user's home is never read from there.
        assert_eq!(
            given(&copies),
            vec![etc.join("conf.d/10-a.conf"), etc.join("deeper")]
        );
    }

    #[test]
    fn a_link_on_the_way_to_an_included_file_is_not_followed() {
        // A folder on the real path found, swapped for a link before the file is opened: what
        // is opened is no longer what was judged.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        file(&root.join("real/x.conf"), "", 0o644);
        std::os::unix::fs::symlink(root.join("real"), root.join("linked")).unwrap();

        assert!(read(&root.join("real/x.conf")).is_some());
        assert!(read(&root.join("linked/x.conf")).is_none());
    }

    #[test]
    fn a_wildcard_under_a_linked_folder_is_expanded_as_ssh_expands_it() {
        // `glob(3)`, which `ssh` expands an `Include` with, goes through a link to a folder.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let etc = root.join("etc");
        file(&etc.join("ssh_config"), "Include conf.d/*.conf\n", 0o644);
        file(&root.join("usr/conf.d/10-a.conf"), "", 0o644);
        std::os::unix::fs::symlink(root.join("usr/conf.d"), etc.join("conf.d")).unwrap();

        let copies = copies_of(&etc.join("ssh_config"), None, me());

        assert_eq!(given(&copies), vec![root.join("usr/conf.d/10-a.conf")]);
    }

    #[test]
    fn the_user_s_config_is_given_when_root_owns_it() {
        // `ssh` reads `~/.ssh/config` only when it is the user's or root's, as it does an include.
        let dir = tempfile::tempdir().unwrap();
        let account = dir.path().canonicalize().unwrap();
        file(&account.join(".ssh/config"), "", 0o644);

        let copies = copies_of(&account.join("no-system"), Some(&account), me());

        assert_eq!(given(&copies), vec![account.join(".ssh/config")]);
    }

    #[test]
    fn copies_past_the_limits_are_said() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().canonicalize().unwrap();
        file(&etc.join("ssh_config"), "Include conf.d/*\n", 0o644);
        for n in 0..=COPIES_MAX {
            file(&etc.join(format!("conf.d/{n:03}.conf")), "", 0o644);
        }
        let big = etc.join("conf.d/big.conf");
        file(&big, &"#".repeat(CONFIG_MAX as usize + 1), 0o644);

        let gaps = gaps_of(&etc.join("ssh_config"), None, me());

        assert!(gaps.iter().any(|gap| gap.contains("64 files")), "{gaps:?}");
        assert!(
            gaps.iter()
                .any(|gap| gap.contains(&big.display().to_string()) && gap.contains("1 MB")),
            "{gaps:?}"
        );
    }

    #[test]
    fn a_root_owned_file_included_from_outside_ssh_is_said() {
        // Not followed, so that no run steers the daemon's walk: said instead.
        let dir = tempfile::tempdir().unwrap();
        let account = dir.path().canonicalize().unwrap();
        file(
            &account.join(".ssh/config"),
            "Include ~/.config/ssh/*\n",
            0o600,
        );
        let outside = account.join(".config/ssh/outside.conf");
        file(&outside, "", 0o644);

        let gaps = gaps_of(&account.join("no-system"), Some(&account), me());

        assert!(
            gaps.iter()
                .any(|gap| gap.contains(&outside.display().to_string())),
            "{gaps:?}"
        );
    }

    #[test]
    fn an_include_of_itself_ends() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().canonicalize().unwrap().join("config");
        file(&config, "Include config\n", 0o644);

        let copies = copies_of(&config, None, me());

        assert_eq!(given(&copies), vec![config]);
    }

    /// A file of `text` at `path`, of `mode`.
    fn file(path: &Path, text: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn me() -> u32 {
        // SAFETY: getuid cannot fail.
        unsafe { libc::getuid() }
    }

    fn given(copies: &[(PathBuf, Vec<u8>)]) -> Vec<PathBuf> {
        copies.iter().map(|(path, _)| path.clone()).collect()
    }

    #[test]
    fn a_file_ssh_refuses_for_its_mode_is_not_given() {
        // `ssh` refuses an included file that its group or others may write, root's or not: a copy
        // of the user's own, 0644, would make it one ssh reads.
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().canonicalize().unwrap().join("etc");
        file(
            &etc.join("ssh_config"),
            "Include kept.conf group.conf others.conf\n",
            0o644,
        );
        file(&etc.join("kept.conf"), "", 0o644);
        file(&etc.join("group.conf"), "", 0o664);
        file(&etc.join("others.conf"), "", 0o646);

        let copies = copies_of(&etc.join("ssh_config"), None, me());

        assert_eq!(given(&copies), vec![etc.join("kept.conf")]);
    }

    #[test]
    fn an_include_of_the_user_s_outside_ssh_is_not_followed() {
        // What a run can write steers no walk of the daemon's: outside `~/.ssh`, by a path or a
        // link, the user's configuration includes nothing here.
        let dir = tempfile::tempdir().unwrap();
        let account = dir.path().canonicalize().unwrap();
        let ssh = account.join(".ssh");
        file(
            &ssh.join("config"),
            "Include ~/.config/ssh/* inside.conf linked.conf\n",
            0o644,
        );
        file(&ssh.join("inside.conf"), "", 0o644);
        file(&account.join(".config/ssh/outside.conf"), "", 0o644);
        file(&account.join("dots/linked.conf"), "", 0o644);
        std::os::unix::fs::symlink(account.join("dots/linked.conf"), ssh.join("linked.conf"))
            .unwrap();

        let copies = copies_of(&account.join("no-system"), Some(&account), me());

        assert_eq!(
            given(&copies),
            vec![ssh.join("config"), ssh.join("inside.conf")]
        );
    }

    #[test]
    fn a_fifo_include_does_not_block_the_walk() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().canonicalize().unwrap();
        file(&etc.join("ssh_config"), "Include fifo\n", 0o644);
        let fifo = CString::new(etc.join("fifo").as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a C string alive for the call.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
        let (done, walked) = std::sync::mpsc::channel();
        let system = etc.join("ssh_config");
        std::thread::spawn(move || done.send(copies_of(&system, None, me())));

        let copies = walked
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the walk blocked on a FIFO");

        assert!(copies.is_empty());
    }
}
