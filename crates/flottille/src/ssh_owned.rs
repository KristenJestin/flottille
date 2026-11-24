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

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

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

/// A file given to a confined command as its own: its real path, the file it was when read
/// (device and inode), and what it held.
pub struct Copied {
    pub path: PathBuf,
    pub id: (u64, u64),
    pub text: Vec<u8>,
}

/// Each root-owned file the machine's configuration and the user's include, with its text now:
/// its real path (links followed, as `ssh` opens it) and what it holds.
pub fn copies() -> Vec<Copied> {
    // A command's child cannot log: a failure of one is said here, at a later start, once for as
    // long as it lasts.
    static SAID: Mutex<Option<String>> = Mutex::new(None);
    let failed = crate::mounts::copies_failed();
    if let Ok(mut said) = SAID.lock()
        && *said != failed
    {
        if let Some(failed) = &failed {
            eprintln!("flottille daemon: {failed}");
        }
        *said = failed;
    }
    // SAFETY: getuid cannot fail.
    if unsafe { libc::getuid() } == 0 {
        return Vec::new();
    }
    let mut copies = walk(&system(), crate::pushed::account(), 0).copies;
    if let Some(failing) = std::env::var_os(FAILING).filter(|_| cfg!(debug_assertions))
        && Path::new(&failing).is_file()
    {
        copies.insert(
            0,
            Copied {
                path: Path::new(&failing).join("ssh_config"),
                id: (0, 0),
                text: Vec::new(),
            },
        );
    }
    copies
}

/// What a confined command's `ssh` still refuses of the files root owns that its settings
/// include, each in a line `flottille doctor` says.
pub fn gaps() -> Vec<String> {
    // SAFETY: getuid cannot fail.
    if unsafe { libc::getuid() } == 0 {
        return Vec::new();
    }
    gaps_of(&system(), crate::pushed::account(), 0)
}

/// The machine's configuration, or the test's.
fn system() -> PathBuf {
    match std::env::var_os(SYSTEM).filter(|_| cfg!(debug_assertions)) {
        Some(system) => PathBuf::from(system),
        None => PathBuf::from(SYSTEM_CONFIG),
    }
}

/// The files `owner` owns that `system` and the `account`'s `~/.ssh/config` include, with their
/// text: root's at every start, the test's own in a test.
#[cfg(test)]
fn copies_of(system: &Path, account: Option<&Path>, owner: u32) -> Vec<Copied> {
    walk(system, account, owner).copies
}

/// What the walk of the files `owner` owns leaves to `ssh`'s refusal, in the user's words.
fn gaps_of(system: &Path, account: Option<&Path>, owner: u32) -> Vec<String> {
    walk(system, account, owner).gaps
}

fn walk<'a>(system: &Path, account: Option<&'a Path>, owner: u32) -> Walk<'a> {
    let mut walk = Walk {
        account,
        owner,
        ssh: None,
        seen: 0,
        copies: Vec::new(),
        gaps: Vec::new(),
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
    walk
}

/// A walk of the files `ssh` includes. What a confined run can write never steers it: the
/// machine's configuration is followed only through files `ssh` reads, root's (or the walk's
/// first), and the user's only inside `~/.ssh`, read-only to a confined run, as the lent push
/// reads it (`pushed::includes`). A run in Full writes `~/.ssh` as the user does: what it names
/// there is still given only when root owns it, a file the user reads already. Each file is
/// opened without following a link on its way nor waiting on a FIFO, and judged on what was
/// opened. What it cannot give, `ssh` refuses, and it is said (`gaps`).
struct Walk<'a> {
    account: Option<&'a Path>,
    owner: u32,
    /// `~/.ssh` and its real path, once the user's configuration is walked.
    ssh: Option<(PathBuf, PathBuf)>,
    seen: usize,
    copies: Vec<Copied>,
    gaps: Vec<String>,
}

impl Walk<'_> {
    /// Gives `file` when `ssh` would read it and it is the owner's, and follows what it includes.
    fn visit(&mut self, file: &Path, folder: &Path, depth: usize) {
        if depth > INCLUDES_MAX {
            return;
        }
        if self.seen >= FILES_MAX {
            self.gap(format!(
                "the ssh settings include more than {FILES_MAX} files: a run's ssh refuses those \
                 past them that root owns (Bad owner or permissions)"
            ));
            return;
        }
        self.seen += 1;
        let Ok(path) = file.canonicalize() else {
            return;
        };
        if let Some((_, real)) = &self.ssh
            && !path.starts_with(real)
        {
            self.outside(&path);
            return;
        }
        let Some((meta, text)) = read(&path) else {
            if std::fs::symlink_metadata(&path)
                .is_ok_and(|meta| meta.is_file() && meta.len() > CONFIG_MAX && self.owns(&meta))
            {
                self.gap(format!(
                    "{}, which the ssh settings include, is larger than 1 MB: a run's ssh \
                     refuses it (Bad owner or permissions)",
                    path.display()
                ));
            }
            return;
        };
        // As `ssh` judges an included file, and the user's own configuration: owned by root or
        // the user, written by no one else. The machine's own is not judged.
        let read_by_ssh = meta.mode() & 0o022 == 0;
        let judged = depth > 0 || self.ssh.is_some();
        let given = judged && read_by_ssh && self.owns(&meta);
        if given && !self.copies.iter().any(|done| done.path == path) {
            if self.copies.len() < COPIES_MAX {
                self.copies.push(Copied {
                    path: path.clone(),
                    id: (meta.dev(), meta.ino()),
                    text: text.clone(),
                });
            } else {
                self.gap(format!(
                    "the ssh settings include more than {COPIES_MAX} files root owns: a run's \
                     ssh refuses those past them (Bad owner or permissions)"
                ));
            }
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
                    self.visit(&found, folder, depth + 1);
                }
            }
        }
    }

    /// The files the `Include` pattern names, as `ssh` reads it: in the user's configuration,
    /// `~` the account's home and a relative path in `~/.ssh`, inside `~/.ssh` alone (one outside
    /// is said, not followed); in the machine's, a relative path in `folder`, and no `~`, which
    /// `ssh` refuses there with the whole configuration. A pattern with a token, a variable or a
    /// class of characters names none here.
    fn expanded(&mut self, pattern: &str, folder: &Path) -> Vec<PathBuf> {
        if let (Some((ssh, _)), Some(account)) = (self.ssh.clone(), self.account) {
            return match crate::pushed::includes(pattern, &ssh, account) {
                Ok(found) => found,
                Err(_) => {
                    let path = match pattern.strip_prefix("~/") {
                        Some(rest) => account.join(rest),
                        None => ssh.join(pattern),
                    };
                    if !pattern.starts_with('~') || pattern.starts_with("~/") {
                        for found in globbed(&path) {
                            if let Ok(real) = found.canonicalize() {
                                self.outside(&real);
                            }
                        }
                    }
                    Vec::new()
                }
            };
        }
        if pattern.contains(['$', '%', '[', '\\', '{']) || pattern.starts_with('~') {
            return Vec::new();
        }
        globbed(&folder.join(pattern))
    }

    /// Whether the walk's owner owns what `meta` says.
    fn owns(&self, meta: &std::fs::Metadata) -> bool {
        meta.uid() == self.owner
    }

    /// Says a file of the owner's the user's configuration includes from outside `~/.ssh`: not
    /// followed, so that no run steers the walk, and so refused by `ssh`.
    fn outside(&mut self, real: &Path) {
        if std::fs::metadata(real).is_ok_and(|meta| meta.is_file() && self.owns(&meta)) {
            self.gap(format!(
                "{}, which ~/.ssh/config includes from outside ~/.ssh, is root's: a run's ssh \
                 refuses it (Bad owner or permissions). Copy it into ~/.ssh and include the copy",
                real.display()
            ));
        }
    }

    fn gap(&mut self, gap: String) {
        if !self.gaps.contains(&gap) {
            self.gaps.push(gap);
        }
    }
}

/// The paths `pattern`, an absolute path with `*` and `?`, matches, as `glob(3)` finds them for
/// `ssh`: through a link to a folder too, a hidden name matched only by a part that starts with a
/// dot, sorted.
fn globbed(pattern: &Path) -> Vec<PathBuf> {
    let mut found = vec![PathBuf::from("/")];
    for part in pattern.components() {
        let part = match part {
            Component::Normal(part) => part,
            Component::ParentDir => "..".as_ref(),
            _ => continue,
        };
        let mut next = Vec::new();
        for path in found {
            let wild = part.as_bytes().iter().any(|b| matches!(b, b'*' | b'?'));
            if !wild {
                let named = path.join(part);
                if std::fs::symlink_metadata(&named).is_ok() {
                    next.push(named);
                }
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                if let (Some(name), Some(part)) = (name.to_str(), part.to_str())
                    && flottille_core::glob::matches(part, name)
                {
                    next.push(path.join(name));
                }
            }
        }
        found = next;
    }
    found.sort();
    found
}

/// What the regular file at `path`, a real path, holds, with what it was when opened: none for a
/// link on its way put there since, a FIFO, a device, a folder or a file past the size read. A
/// kernel without `openat2` (before Linux 5.6) reads none.
fn read(path: &Path) -> Option<(std::fs::Metadata, Vec<u8>)> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let fd = crate::mounts::open_unfollowed(&path, libc::O_RDONLY | libc::O_NONBLOCK);
    if fd < 0 {
        return None;
    }
    // SAFETY: a descriptor just opened, owned by the file alone.
    let file = unsafe { std::fs::File::from_raw_fd(fd as libc::c_int) };
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

    fn given(copies: &[Copied]) -> Vec<PathBuf> {
        copies.iter().map(|copied| copied.path.clone()).collect()
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
