//! What a push the gate let through reads, and so runs, outside the sandbox (`docs/vault.md`, "The
//! runtime folder"): the user's `ssh` configuration and their own git settings. Both
//! are read-only to every confined command, in its private mount namespace (`mounts.rs`): `~/.ssh`
//! whole, the files of the user's git settings that were there at the daemon's start with those
//! they include, and each folder on the way to them that a command could otherwise rename kept
//! where it is (bound onto itself: a mount point is not moved or removed).
//!
//! What that cannot hold is not protected file by file: the user's SSH agent is then lent to no
//! push, and a push over SSH is refused, the reason said by `flottille doctor` and the push. It is
//! judged at each push, from the files as they are then: a configuration that reaches outside
//! `~/.ssh` (an `Include` of another folder, a link, a hard link, a program it runs), git settings
//! that include a file not protected since the start, a file protected at the start that is no
//! longer the one at its path, another mount of the same files, a kernel that gives no namespace.
//! Nothing here protects by reading the configuration: reading it only decides whether to lend.
//!
//! A run in Full is confined in no way (`docs/tools.md`, "In Full, a run is the user's own"): it
//! writes `~/.ssh` and the git settings as the user does, and what it writes outlives it. A
//! later push in Ask or Auto still reads them as here, each time: what `ssh`'s configuration
//! runs is refused, the credential helpers are those found at the start, the hooks, the
//! transports, the `ssh` command and the programs a transport names are the daemon's own, and a
//! program of `~/.ssh` the user's `ssh` command names runs only as it was at the start.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// The variable that plays the account's home (what `ssh` reads `~` as), read by a test build
/// only: the tests' daemon is given its own, never the machine's `~/.ssh`.
const ACCOUNT_HOME: &str = "FLOTTILLE_TEST_ACCOUNT_HOME";

/// The variable that plays a kernel without Landlock, read by a test build only.
const NO_LANDLOCK: &str = "FLOTTILLE_TEST_NO_LANDLOCK";

/// What `ssh` runs a program, or loads one, for.
const RUNNING: [&str; 6] = [
    "proxycommand",
    "localcommand",
    "knownhostscommand",
    "pkcs11provider",
    "securitykeyprovider",
    "xauthlocation",
];

/// `ssh`'s short options that take a value (its `getopt` list).
const SSH_VALUED: &str = "bceilmopBDEFIJLOPQRSWw";

/// Keys of an `-o` that share a connection or forward the agent, besides those that run a
/// program: [`crate::git::SSH_OPTIONS`] sets them first, a later one is refused to say so.
const SHARING: [&str; 5] = [
    "controlmaster",
    "controlpath",
    "controlpersist",
    "forwardagent",
    "include",
];

/// Why a short flag of the user's `ssh` command is refused.
fn ssh_flag(letter: char) -> Result<(), String> {
    match letter {
        'A' => Err("has -A, which forwards the user's agent to the host: leave it out".into()),
        'M' => Err("has -M, which shares the connection with any run: leave it out".into()),
        _ => Ok(()),
    }
}

/// Why a short option of the user's `ssh` command, with its value, is refused.
fn ssh_option(letter: char, value: &str) -> Result<(), String> {
    match letter {
        'F' if value == "/dev/null" => Ok(()),
        'F' | 'I' => Err(format!(
            "has -{letter}, which makes ssh read a file Flottille does not keep: put what it \
             says in ~/.ssh/config"
        )),
        'S' => Err("has -S, which shares the connection with any run: leave it out".into()),
        'o' => {
            let key = value
                .split('=')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if RUNNING.contains(&key.as_str()) || key == "match" {
                return Err(format!(
                    "runs a program (-o {value}), which a run may have written"
                ));
            }
            match SHARING.contains(&key.as_str()) {
                true => Err(format!(
                    "has -o {value}, which shares the connection or the agent, or reads a \
                     file Flottille does not keep: leave it out"
                )),
                false => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

/// The entries of `~/.ssh` looked at, at most: a folder holding more is not judged.
const ENTRIES_MAX: usize = 10_000;
/// How deep `Include`s go, as `ssh` reads them.
const INCLUDES_MAX: usize = 16;
/// The bytes of a configuration file read, at most.
const CONFIG_MAX: u64 = 1024 * 1024;

/// What the daemon found at its start.
struct Found {
    /// The account's home, which `ssh` reads `~` as (not `HOME`).
    account: PathBuf,
    /// `HOME`, which git and the shell read `~` as.
    home: Option<PathBuf>,
    /// `<account>/.ssh`, when it is a folder.
    ssh: PathBuf,
    /// The files of the user's git settings that were there, in the order git reads them.
    git: Vec<PathBuf>,
    /// Every path made read-only, with its device and inode then.
    read_only: Vec<(PathBuf, (u64, u64))>,
    /// The folders kept where they are, outermost first.
    pinned: Vec<PathBuf>,
    /// Whether the daemon found it, and made `~/.ssh` (not `flottille doctor`, which probes
    /// what a daemon would find: one would make a missing `~/.ssh`).
    made: bool,
    /// When: what a run in Full, which writes `~/.ssh` as the user does, changed is after it.
    started: std::time::SystemTime,
}

static FOUND: OnceLock<Option<Found>> = OnceLock::new();

/// Finds what a push reads, before anything runs, and makes a missing `~/.ssh` where `make` (the
/// daemon's start; `0700`, as `ssh` would): a folder that is not there could not be kept
/// read-only, and a run would make it.
pub fn find(make: bool) {
    FOUND.get_or_init(|| {
        let account = account_home()?;
        let account = account.canonicalize().unwrap_or(account);
        let ssh = account.join(".ssh");
        if make && std::fs::symlink_metadata(&ssh).is_err() {
            use std::os::unix::fs::DirBuilderExt;
            let made = std::fs::DirBuilder::new().mode(0o700).create(&ssh);
            if let Err(error) = made {
                eprintln!("flottille daemon: cannot make {}: {error}", ssh.display());
            }
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute());
        let git: Vec<PathBuf> = git_settings(home.as_deref())
            .into_iter()
            .filter_map(|file| file.canonicalize().ok())
            .filter(|file| std::fs::metadata(file).is_ok_and(|m| m.is_file()))
            .collect();
        let mut paths: Vec<PathBuf> = Vec::new();
        if std::fs::symlink_metadata(&ssh).is_ok_and(|m| m.is_dir()) {
            paths.push(ssh.clone());
        }
        for file in &git {
            paths.push(file.clone());
            let mut seen = Vec::new();
            included_files(file, home.as_deref(), &mut seen);
            paths.extend(seen);
        }
        paths.sort();
        paths.dedup();
        let read_only: Vec<(PathBuf, (u64, u64))> = paths
            .into_iter()
            .filter_map(|path| Some((identity(&path)?, path)))
            .map(|(id, path)| (path, id))
            .collect();
        let mut pinned: Vec<PathBuf> = read_only
            .iter()
            .flat_map(|(path, _)| on_the_way(path))
            .collect();
        pinned.sort_by_key(|folder| folder.components().count());
        pinned.dedup();
        Some(Found {
            account,
            home,
            ssh,
            git,
            read_only,
            pinned,
            made: make,
            started: std::time::SystemTime::now(),
        })
    });
}

/// The paths every confined command has read-only.
pub fn read_only() -> Vec<PathBuf> {
    found()
        .map(|found| {
            found
                .read_only
                .iter()
                .map(|(path, _)| path.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The folders every confined command has kept where they are, outermost first.
pub fn pinned() -> Vec<PathBuf> {
    found()
        .map(|found| found.pinned.clone())
        .unwrap_or_default()
}

/// The account's home found at the daemon's start: what `ssh` reads `~` as.
pub fn account() -> Option<&'static Path> {
    found().map(|found| found.account.as_path())
}

fn found() -> Option<&'static Found> {
    FOUND.get()?.as_ref()
}

/// The account's home, from the user database: what `ssh` reads `~` as.
fn account_home() -> Option<PathBuf> {
    if cfg!(debug_assertions)
        && let Some(home) = std::env::var_os(ACCOUNT_HOME)
    {
        return Some(PathBuf::from(home));
    }
    // SAFETY: getpwuid_r writes into the buffers given, which outlive the call.
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut buffer = vec![0 as libc::c_char; 16 * 1024];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let failed = libc::getpwuid_r(
            libc::getuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        );
        if failed != 0 || result.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        let dir = std::ffi::CStr::from_ptr(entry.pw_dir);
        use std::os::unix::ffi::OsStrExt;
        let dir = PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes()));
        dir.is_absolute().then_some(dir)
    }
}

/// The files git reads as the user's own settings: `GIT_CONFIG_GLOBAL` alone when it is set,
/// else `$XDG_CONFIG_HOME/git/config` (`~/.config/git/config`) and `~/.gitconfig`, in that order.
fn git_settings(home: Option<&Path>) -> Vec<PathBuf> {
    if let Some(global) = std::env::var_os("GIT_CONFIG_GLOBAL").filter(|g| !g.is_empty()) {
        return vec![PathBuf::from(global)];
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|config| config.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")));
    config
        .map(|config| config.join("git/config"))
        .into_iter()
        .chain(home.map(|home| home.join(".gitconfig")))
        .collect()
}

/// A path's device and inode, its links not followed.
fn identity(path: &Path) -> Option<(u64, u64)> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|m| (m.dev(), m.ino()))
}

/// The folders on the way to `path` that a command could rename or remove: those whose own
/// folder is the user's or one they may write. Their parents are not: their own folder is
/// another's.
pub(crate) fn on_the_way(path: &Path) -> Vec<PathBuf> {
    path.ancestors()
        .skip(1)
        .filter(|folder| folder.parent().is_some_and(changeable))
        .map(Path::to_owned)
        .collect()
}

/// Whether the user may change what is in `folder`: theirs (they may make it writable), or one
/// they may write.
fn changeable(folder: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    if std::fs::metadata(folder).is_ok_and(|m| m.uid() == uid) {
        return true;
    }
    let Ok(path) = std::ffi::CString::new(folder.as_os_str().as_bytes()) else {
        return true;
    };
    // SAFETY: a valid C string for the call.
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

/// Why the user's SSH agent is lent to no push, if it is not: the judgement of each push, made
/// from the files as they are now. Not the user's own `ssh` command, judged with the push
/// (`ssh_command`).
pub fn unlent() -> Option<String> {
    ssh_unlent().or_else(|| git_unlent(found()?))
}

/// [`unlent`], of what `ssh` itself reads alone: what the daemon's own fetch, which runs the `ssh`
/// found at its start, is lent the agent by.
pub fn ssh_unlent() -> Option<String> {
    if crate::mounts::namespaces() == crate::mounts::Namespaces::Refused {
        return Some(
            "the kernel gives no user namespace to the commands Flottille confines, so ~/.ssh \
             cannot be kept read-only to them"
                .into(),
        );
    }
    if !landlock_enforced() {
        return Some(
            "the kernel has no Landlock (its second ABI or later) to confine the commands \
             Flottille runs, so they reach ~/.ssh through /proc as a process outside their \
             namespace sees it"
                .into(),
        );
    }
    match found() {
        Some(found) => folder_unlent(found),
        None => Some("the account's home could not be read at the daemon's start".into()),
    }
}

/// Whether the commands Flottille confines are confined by Landlock (`landlock::confined`): what
/// keeps them from the daemon's runtime folder, the mount namespace's empty folder being reached
/// through `/proc` too.
fn landlock_enforced() -> bool {
    if cfg!(debug_assertions) && std::env::var_os(NO_LANDLOCK).is_some() {
        return false;
    }
    crate::landlock::abi() >= 2
}

/// Why no push the gate let through is run outside the sandbox, if none is: the kernel has no
/// Landlock to keep the folder it runs from out of a run's reach (`runs::push_folder`), or no
/// `git` only root may change was found at the daemon's start. Every such push is then refused,
/// over SSH or not.
pub fn unlifted() -> Option<String> {
    if !landlock_enforced() {
        return Some(
            "the kernel has no Landlock (its second ABI or later), which keeps the folder a push \
             runs from, in the daemon's runtime folder, out of a run's reach"
                .into(),
        );
    }
    if crate::git::program().is_none() {
        return Some(crate::git::UNFOUND.into());
    }
    None
}

/// The sentence that says why no push is lifted ([`unlifted`]), as `flottille doctor` and a push
/// say it.
pub fn unlifted_said(why: &str) -> String {
    format!(
        "a push an agent asks for is refused, since it would run outside the sandbox: {why}; \
         push from a terminal of your own (docs/vault.md, \"The runtime folder\")"
    )
}

/// The sentence that says why, as `flottille doctor` and a push say it.
pub fn said(why: &str) -> String {
    format!(
        "the user's SSH agent is lent to no push, and a push over SSH is refused: {why} \
         (docs/vault.md, \"The runtime folder\")"
    )
}

/// What a push the gate let through signs in with: the `ssh` it runs, and why the user's agent
/// is lent to it not, if it is not.
pub struct Judged {
    pub ssh: OsString,
    pub unlent: Option<String>,
}

/// Judges a push from the repository at `folder`, the files as they are now.
pub async fn judged(folder: &Path) -> Judged {
    let unlent = tokio::task::spawn_blocking(unlent)
        .await
        .unwrap_or_else(|_| Some("the judgement of what ssh reads failed".into()));
    let ssh = match unlent {
        Some(_) => Ok(crate::git::ssh_command()),
        None => ssh_command(folder).await,
    };
    match ssh {
        Ok(ssh) => Judged { ssh, unlent },
        Err(why) => Judged {
            ssh: crate::git::ssh_command(),
            unlent: Some(why),
        },
    }
}

/// What `~/.ssh` lends nothing for.
fn folder_unlent(found: &Found) -> Option<String> {
    let ssh = &found.ssh;
    let shown = ssh.display();
    let meta = std::fs::symlink_metadata(ssh);
    if !found.made && meta.is_err() {
        return None;
    }
    if !meta.is_ok_and(|m| m.is_dir()) {
        return Some(format!(
            "{shown} is not a folder (a link, or missing): make it a folder of its own"
        ));
    }
    if let Some(why) = changed(found, ssh) {
        return Some(why);
    }
    if let Some(why) = aliased(ssh) {
        return Some(why);
    }
    let mut read = Read::default();
    let config = ssh.join("config");
    if std::fs::symlink_metadata(&config).is_ok()
        && let Err(why) = configuration(&config, ssh, &found.account, 0, &mut read)
    {
        return Some(why);
    }
    tree(ssh, &read)
}

/// The files of `~/.ssh` the configuration names, which `ssh` reads: its `Include`s, keys,
/// certificates and known hosts. `any` where one is named by a token or a variable, which could
/// be any file.
#[derive(Default)]
struct Read {
    paths: Vec<PathBuf>,
    any: bool,
}

/// The directives whose values are files `ssh` reads.
const READ_FILES: [&str; 5] = [
    "identityfile",
    "certificatefile",
    "userknownhostsfile",
    "globalknownhostsfile",
    "revokedhostkeys",
];

impl Read {
    /// Whether `ssh` may read `path` (a link, or a folder on the way to a file it reads): by
    /// default (`config`, `id_*`, `known_hosts*`), or as the configuration names it.
    fn reads(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        self.any
            || name == "config"
            || name.starts_with("id_")
            || name.starts_with("known_hosts")
            || self.paths.iter().any(|read| read.starts_with(path))
    }
}

/// Whether the link at `path` leads to a file of `ssh_folder` with no other link on the way,
/// which no confined command can change.
fn inside(path: &Path, ssh_folder: &Path) -> bool {
    let Ok(target) = std::fs::read_link(path) else {
        return false;
    };
    let lexical = path.parent().unwrap_or(Path::new("/")).join(target);
    let within = lexical.strip_prefix(ssh_folder).is_ok_and(|rest| {
        rest.components()
            .all(|part| matches!(part, Component::Normal(_)))
    });
    within && lexical.canonicalize().is_ok_and(|real| real == lexical)
}

/// Why `path`, made read-only at the daemon's start, may no longer be what a command sees
/// read-only: gone, or another file at its path since.
fn changed(found: &Found, path: &Path) -> Option<String> {
    let start = found.read_only.iter().find(|(p, _)| p == path)?.1;
    match identity(path) == Some(start) {
        true => None,
        false => Some(format!(
            "{} is not the one the daemon found at its start (removed, or replaced): restart the \
             daemon, or work in Full",
            path.display()
        )),
    }
}

/// Why a link or a hard link in the folder, or its size, leaves something a command writes in
/// reach of `ssh`. A link to a file of the folder is kept with it; a link to a socket that `ssh`
/// does not read (`read`), as a terminal multiplexer keeps the agent's, is never read.
fn tree(root: &Path, read: &Read) -> Option<String> {
    let mut seen = 0;
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) => return Some(format!("{} cannot be read: {error}", folder.display())),
        };
        for entry in entries {
            seen += 1;
            if seen > ENTRIES_MAX {
                return Some(format!(
                    "{} holds more than {ENTRIES_MAX} entries, which are not looked at",
                    folder.display()
                ));
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                use std::os::unix::fs::FileTypeExt;
                let socket =
                    std::fs::metadata(&path).is_ok_and(|target| target.file_type().is_socket());
                if inside(&path, root) || (socket && !read.reads(&path)) {
                    continue;
                }
                return Some(format!(
                    "{} is a link, which ssh follows to a file a run can write: put the file \
                     itself in ~/.ssh",
                    path.display()
                ));
            }
            if meta.is_dir() {
                folders.push(path);
            } else if meta.nlink() > 1 {
                return Some(format!(
                    "{} has a hard link elsewhere, through which a run can write it: copy it in \
                     place of the link",
                    path.display()
                ));
            }
        }
    }
    None
}

/// Checks the `ssh` configuration file `file` and what it includes: an `Include` outside
/// `ssh_folder`, or that Flottille does not read, and a program it runs, lend nothing.
fn configuration(
    file: &Path,
    ssh_folder: &Path,
    account: &Path,
    depth: usize,
    read: &mut Read,
) -> Result<(), String> {
    let shown = file.display();
    if depth > INCLUDES_MAX {
        return Err(format!("{shown} is included more than {INCLUDES_MAX} deep"));
    }
    // Through a link, as `ssh` reads it: one out of `~/.ssh` is refused by `tree`.
    let meta = std::fs::metadata(file).map_err(|e| format!("{shown}: {e}"))?;
    if !meta.is_file() {
        return Ok(());
    }
    if meta.len() > CONFIG_MAX {
        return Err(format!("{shown} is larger than Flottille reads"));
    }
    let text = std::fs::read(file).map_err(|e| format!("{shown} cannot be read: {e}"))?;
    let text = String::from_utf8_lossy(&text);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words = ssh_words(line);
        let Some((keyword, args)) = words.split_first() else {
            continue;
        };
        let keyword = keyword.to_ascii_lowercase();
        let rest = args.join(" ");
        if RUNNING.contains(&keyword.as_str()) && !rest.eq_ignore_ascii_case("none") {
            return Err(format!(
                "{shown} runs a program ({}), which a run may have written: Flottille lends \
                 the agent to no push whose ssh runs one",
                words[0]
            ));
        }
        if READ_FILES.contains(&keyword.as_str()) {
            for value in args
                .iter()
                .filter(|value| !value.eq_ignore_ascii_case("none"))
            {
                if value.contains(['%', '$']) {
                    read.any = true;
                }
                match value.strip_prefix("~/") {
                    Some(rest) => read.paths.push(account.join(rest)),
                    None => {
                        read.paths.push(ssh_folder.join(value));
                        read.paths.push(account.join(value));
                    }
                }
            }
        }
        if keyword == "match"
            && args
                .iter()
                .any(|word| word.trim_start_matches('!').eq_ignore_ascii_case("exec"))
        {
            return Err(format!(
                "{shown} runs a program (Match exec), which a run may have written: Flottille \
                 lends the agent to no push whose ssh runs one"
            ));
        }
        if keyword == "include" {
            for pattern in args {
                for included in includes(pattern, ssh_folder, account)
                    .map_err(|why| format!("{shown} includes {pattern}, {why}"))?
                {
                    read.paths.push(included.clone());
                    configuration(&included, ssh_folder, account, depth + 1, read)?;
                }
            }
        }
    }
    Ok(())
}

/// The words of a line of `ssh`'s configuration, as `ssh` splits it: at blanks, the first `=`
/// too, quotes and a backslash's escape undone (`"ProxyCommand"` is the keyword).
pub(crate) fn ssh_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut equals = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (None, '"' | '\'') => {
                quote = Some(c);
                word.get_or_insert_default();
            }
            (_, '\\') => word.get_or_insert_default().extend(chars.next()),
            (None, c) if c.is_whitespace() || (c == '=' && !equals) => {
                equals |= c == '=';
                words.extend(word.take());
            }
            (_, c) => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word);
    words
}

/// The files the `Include` pattern names, read as `ssh` reads it: `~` the account's home, a
/// relative path in `~/.ssh`. One outside `~/.ssh`, or that Flottille does not read (a variable, a
/// token, a class of characters), is refused, saying how to fix it.
pub(crate) fn includes(
    pattern: &str,
    ssh_folder: &Path,
    account: &Path,
) -> Result<Vec<PathBuf>, String> {
    if pattern.contains(['$', '%', '[', '\\', '{']) {
        return Err("a pattern Flottille does not read: name the files in ~/.ssh plainly".into());
    }
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => account.join(rest),
        None if pattern.starts_with('~') => {
            return Err("another account's home: move the file into ~/.ssh".into());
        }
        None => ssh_folder.join(pattern),
    };
    let inside = path.strip_prefix(ssh_folder).ok().filter(|rest| {
        rest.components()
            .all(|part| matches!(part, Component::Normal(_)))
    });
    let Some(rest) = inside else {
        return Err(format!(
            "outside ~/.ssh ({}), which a run can write: move it into ~/.ssh",
            path.display()
        ));
    };
    flottille_core::glob::expand(ssh_folder, &rest.to_string_lossy())
        .map(|found| {
            found
                .into_iter()
                .map(|rest| ssh_folder.join(rest))
                .collect()
        })
        .map_err(|error| format!("which cannot be read: {error}"))
}

/// Why `path` is also reached through another mount of its file system, which stays writable:
/// a bind mount of the home elsewhere, the top of a Btrfs volume mounted beside its subvolumes.
/// And why a mount below it is not made read-only with it.
fn aliased(path: &Path) -> Option<String> {
    let mounts = match std::fs::read_to_string("/proc/self/mountinfo") {
        Ok(mounts) => mounts,
        Err(error) => return Some(format!("the mounts cannot be read: {error}")),
    };
    if let Some(other) = alias(path, &mounts) {
        return Some(format!(
            "{} is also reached through {}, another mount of its file system, which stays \
             writable to a run",
            path.display(),
            other.display()
        ));
    }
    below(path, &mounts).map(|point| {
        format!(
            "{} is mounted below {}, and stays writable to a run",
            point.display(),
            path.display()
        )
    })
}

/// A mount point strictly below `path`.
fn below(path: &Path, mountinfo: &str) -> Option<PathBuf> {
    mountinfo
        .lines()
        .filter_map(|line| line.split(' ').nth(4).map(unescape))
        .find(|point| point != path && point.starts_with(path))
}

/// Of `mountinfo`'s mounts, one other than the mount `path` is in that holds it too.
fn alias(path: &Path, mountinfo: &str) -> Option<PathBuf> {
    // (device, root in the file system, mount point)
    let mounts: Vec<(&str, PathBuf, PathBuf)> = mountinfo
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            Some((
                *fields.get(2)?,
                unescape(fields.get(3)?),
                unescape(fields.get(4)?),
            ))
        })
        .collect();
    let (at, (device, root, point)) = mounts
        .iter()
        .enumerate()
        .filter(|(_, (_, _, point))| path.starts_with(point))
        .max_by_key(|(at, (_, _, point))| (point.components().count(), *at))?;
    let inside = root.join(path.strip_prefix(point).ok()?);
    mounts
        .iter()
        .enumerate()
        .find(|(other, (d, r, _))| *other != at && d == device && inside.starts_with(r))
        .map(|(_, (_, r, p))| p.join(inside.strip_prefix(r).unwrap_or(&inside)))
}

/// A path of `mountinfo`, its octal escapes (`\040` for a space) undone.
fn unescape(field: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\'
            && let Some(code) = field
                .get(at + 1..at + 4)
                .and_then(|octal| u8::from_str_radix(octal, 8).ok())
        {
            out.push(code);
            at += 4;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    PathBuf::from(OsString::from_vec(out))
}

/// What git lends nothing for: a file of the settings no longer the one protected, or one that
/// includes a file not protected since the start.
fn git_unlent(found: &Found) -> Option<String> {
    for file in &found.git {
        if let Some(why) = changed(found, file).or_else(|| aliased(file)) {
            return Some(why);
        }
        let mut seen = Vec::new();
        if let Err(why) = included(file, found.home.as_deref(), &mut seen) {
            return Some(why);
        }
        for path in seen {
            let protected = found.read_only.iter().any(|(p, _)| *p == path);
            if !protected {
                return Some(format!(
                    "the git settings include {}, which was not there at the daemon's start: \
                     restart the daemon",
                    path.display()
                ));
            }
            if let Some(why) = changed(found, &path).or_else(|| aliased(&path)) {
                return Some(why);
            }
        }
    }
    None
}

/// The files `file` includes, whatever their condition, and what they include in turn, as far
/// as they can be found.
fn included_files(file: &Path, home: Option<&Path>, seen: &mut Vec<PathBuf>) {
    let _ = included(file, home, seen);
}

/// [`included_files`], with why one of them is not a file that can be protected: under git's
/// own prefix, or a hard link. A missing one is left out, as git leaves it.
fn included(file: &Path, home: Option<&Path>, seen: &mut Vec<PathBuf>) -> Result<(), String> {
    if seen.len() > INCLUDES_MAX {
        return Err(format!(
            "{} includes more than Flottille reads",
            file.display()
        ));
    }
    if std::fs::metadata(file).is_ok_and(|m| m.nlink() > 1) {
        return Err(format!(
            "{} has a hard link elsewhere, through which a run can write it: copy it in place of \
             the link",
            file.display()
        ));
    }
    for value in include_paths(file) {
        let path = match value.strip_prefix("~/") {
            Some(rest) => match home {
                Some(home) => home.join(rest),
                None => return Err(format!("{} includes {value}, with no home", file.display())),
            },
            None if value.starts_with("%(prefix)") || value.starts_with('~') => {
                return Err(format!(
                    "{} includes {value}, which Flottille does not read: name a file of yours",
                    file.display()
                ));
            }
            None => file.parent().unwrap_or(Path::new("/")).join(&value),
        };
        // Missing, git reads it as nothing: one made since the start is not protected, which
        // the judgement of each push finds (`git_unlent`).
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        if seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        included(&path, home, seen)?;
    }
    Ok(())
}

/// The values of `include.path` and `includeIf.<condition>.path` in `file` itself, as git reads
/// them (`git config --file`, not what they include).
fn include_paths(file: &Path) -> Vec<String> {
    let listed = crate::git::start_read(&[
        OsStr::new("config"),
        OsStr::new("--null"),
        OsStr::new("--file"),
        file.as_os_str(),
        OsStr::new("--get-regexp"),
        OsStr::new(r"^include(if\..*)?\.path$"),
    ]);
    listed
        .split('\0')
        .filter_map(|entry| entry.split_once('\n'))
        .map(|(_, value)| value.to_owned())
        .collect()
}

/// The `ssh` a push signs in with, read for the repository at `folder`: the daemon's own
/// `GIT_SSH_COMMAND`, else the `core.sshCommand` of the user's git settings as they apply there
/// (an `includeIf` for its folder or branch), read from the files found at the start alone, never
/// the repository's; else the one found at the start. Refused, with why, when it runs a program
/// a run could change, or reads another configuration than `~/.ssh`'s.
pub async fn ssh_command(folder: &Path) -> Result<OsString, String> {
    let own = match std::env::var_os("GIT_SSH_COMMAND").filter(|c| !c.is_empty()) {
        Some(own) => Some(own.to_string_lossy().into_owned()),
        None => {
            let mut own = None;
            for file in found()
                .map(|found| found.git.as_slice())
                .unwrap_or_default()
            {
                let read = crate::git::run_with(
                    folder,
                    &[
                        "config",
                        "--global",
                        "--includes",
                        "--get",
                        "core.sshCommand",
                    ],
                    crate::git::Class::Read,
                    &[("GIT_CONFIG_GLOBAL", file.as_os_str())],
                )
                .await?;
                let value = read.stdout.trim();
                if read.success && !value.is_empty() {
                    own = Some(value.to_owned());
                }
            }
            own
        }
    };
    match own {
        None => Ok(crate::git::ssh_command()),
        Some(own) => {
            checked(&own, found()).map_err(|why| format!("the user's ssh command ({own}) {why}"))
        }
    }
}

/// The user's own git settings as they apply to the repository at `folder` (an `includeIf` for
/// its folder or its branch), read from the files found at the daemon's start alone, in the
/// order git reads them: what a push the gate let through is given of them, since it runs from a
/// folder of the daemon's (`push_folder`), where git would read them otherwise. A file that is
/// gone is none, as git reads it. **An included file that was not there at the daemon's start
/// (nor the same file) is left out**, with what it says, and so is a `hasconfig` include of such a
/// file: the push is lent the user's sign-in, and a file a run in Full may have made since could
/// say `http.proxy`, `http.sslVerify=false` or `http.sslCAInfo` for it. A top file of the
/// settings is the user's own to change, as `~/.gitconfig` is for a dotfiles update.
pub async fn user_settings(folder: &Path) -> Result<Vec<(String, String)>, String> {
    let mut settings = Vec::new();
    for file in found()
        .map(|found| found.git.as_slice())
        .unwrap_or_default()
    {
        if std::fs::metadata(file).is_err() {
            continue;
        }
        let read = crate::git::run_with(
            folder,
            &[
                "config",
                "--global",
                "--includes",
                "--null",
                "--show-origin",
                "--list",
            ],
            crate::git::Class::Read,
            &[("GIT_CONFIG_GLOBAL", file.as_os_str())],
        )
        .await?;
        if !read.success {
            return Err(format!(
                "the user's git settings in {} cannot be read: {}",
                file.display(),
                read.stderr.trim()
            ));
        }
        let start = found();
        let protected = |path: &Path| {
            start.is_some_and(|found| {
                path.canonicalize().is_ok_and(|real| {
                    found.read_only.iter().any(|(p, _)| *p == real)
                        && changed(found, &real).is_none()
                })
            })
        };
        let top = |path: &Path| {
            start.is_some_and(|found| {
                path.canonicalize()
                    .is_ok_and(|real| found.git.contains(&real))
            }) || protected(path)
        };
        let home = start.and_then(|found| found.home.as_deref());
        settings.extend(given(&read.stdout, &top, &|value| {
            let value = match value.strip_prefix("~/") {
                Some(rest) => home.map(|home| home.join(rest)).unwrap_or_default(),
                None => PathBuf::from(value),
            };
            protected(&value)
        }));
    }
    Ok(settings)
}

/// Of the user's settings as `git config --null --show-origin --list` writes them, with what
/// they include: those a push is given. What they include is listed already, and named again it
/// would be read twice; but what an `includeIf "hasconfig:remote.*.url:…"` includes depends on
/// the URLs of the push's own remotes, which git knows only once it reads them all: it is given
/// as it is, its file named by its full path (a setting given to a command names no file
/// relative to another), for git to include where it applies.
fn given(
    listed: &str,
    from_trusted_file: &dyn Fn(&Path) -> bool,
    include_trusted: &dyn Fn(&str) -> bool,
) -> Vec<(String, String)> {
    let mut parts = listed.split('\0');
    let mut given = Vec::new();
    while let (Some(origin), Some(entry)) = (parts.next(), parts.next()) {
        if origin
            .strip_prefix("file:")
            .is_some_and(|file| !from_trusted_file(Path::new(file)))
        {
            continue;
        }
        let (key, value) = entry.split_once('\n').unwrap_or((entry, "true"));
        if key == "include.path" {
            continue;
        }
        if key.starts_with("includeif.") && key.ends_with(".path") {
            if !key.starts_with("includeif.hasconfig:") {
                continue;
            }
            let path = match (origin.strip_prefix("file:"), value) {
                (_, value) if value.starts_with('/') || value.starts_with("~/") => value.to_owned(),
                (Some(file), value) => Path::new(file)
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .join(value)
                    .to_string_lossy()
                    .into_owned(),
                (None, _) => continue,
            };
            if !include_trusted(&path) {
                continue;
            }
            given.push((key.to_owned(), path));
            continue;
        }
        given.push((key.to_owned(), value.to_owned()));
    }
    given
}

/// The user's own `ssh` command, as a push may run it: `ssh` named alone is the one found at the
/// start; another program must be in `~/.ssh` or only root's to change. No `-F` (another
/// configuration, but `/dev/null`), no `-I` (a library), no option that runs a program, shares a
/// connection (`-M`, `-S`, `ControlPath`…) or forwards the agent (`-A`), and nothing the shell
/// would read as more than words. Given [`crate::git::SSH_OPTIONS`] first, which win over its own.
fn checked(own: &str, found: Option<&Found>) -> Result<OsString, String> {
    let plain = |c: char| c.is_ascii_alphanumeric() || " \t._/~=@:+,-".contains(c);
    if !own.chars().all(plain) {
        return Err(
            "is a shell line Flottille does not read: name a program and its options".into(),
        );
    }
    let mut words = own.split_whitespace();
    let program = words.next().ok_or("is empty")?;
    let words: Vec<&str> = words.collect();
    let mut at = 0;
    while let Some(word) = words.get(at) {
        at += 1;
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.starts_with('-')) else {
            continue;
        };
        for (index, letter) in letters.char_indices() {
            if !SSH_VALUED.contains(letter) {
                ssh_flag(letter)?;
                continue;
            }
            let value = match &letters[index + letter.len_utf8()..] {
                "" => {
                    at += 1;
                    words.get(at - 1).copied().unwrap_or_default()
                }
                rest => rest,
            };
            ssh_option(letter, value)?;
            break;
        }
    }
    let rest = &own.trim_start()[program.len()..];
    if program == "ssh" {
        let mut command = crate::git::ssh_command();
        command.push(rest);
        return Ok(command);
    }
    let path = match program.strip_prefix("~/") {
        Some(rest) => found
            .and_then(|found| found.home.as_ref())
            .map(|home| home.join(rest)),
        None => Some(PathBuf::from(program)).filter(|path| path.is_absolute()),
    };
    let in_ssh = path.as_ref().is_some_and(|path| {
        found.is_some_and(|found| {
            path.strip_prefix(&found.ssh).is_ok_and(|rest| {
                rest.components()
                    .all(|part| matches!(part, Component::Normal(_)))
            })
        })
    });
    let root = |path: &Path| std::fs::metadata(path).is_ok_and(|m| m.uid() == 0);
    let rooted = path
        .as_ref()
        .and_then(|path| path.canonicalize().ok())
        .is_some_and(|real| real.ancestors().all(root));
    // `~/.ssh` is read-only to a confined run, not to a run in Full (`docs/tools.md`): a
    // program there is run only as it was at the daemon's start, its status (`ctime`, which no
    // one sets back) older, through a link or not.
    if in_ssh
        && !rooted
        && let (Some(path), Some(found)) = (&path, found)
    {
        let older = |meta: std::io::Result<std::fs::Metadata>| {
            meta.is_ok_and(|meta| {
                let changed = std::time::UNIX_EPOCH
                    + std::time::Duration::new(meta.ctime() as u64, meta.ctime_nsec() as u32);
                changed <= found.started
            })
        };
        if !older(std::fs::symlink_metadata(path)) || !older(std::fs::metadata(path)) {
            return Err(format!(
                "runs {program}, which was made or changed since the daemon's start (a run in \
                 Full writes ~/.ssh): restart the daemon to have it run"
            ));
        }
    }
    match in_ssh || rooted {
        true => Ok(format!("{program} {}{rest}", crate::git::SSH_OPTIONS).into()),
        false => Err(format!(
            "runs {program}, which a run could change: put it in ~/.ssh, or name ssh itself"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_s_includes_for_a_remote_s_url_are_given_for_git_to_include() {
        let listed = [
            "file:/home/u/.gitconfig\0include.path\nother.gitconfig",
            "file:/home/u/.gitconfig\0includeif.gitdir:~/work/.path\nwork.gitconfig",
            "file:/home/u/.gitconfig\0includeif.hasconfig:remote.*.url:git@forge.invalid:**.path\nforge.gitconfig",
            "file:/home/u/.gitconfig\0includeif.hasconfig:remote.*.url:https://a.invalid/**.path\n~/a.gitconfig",
            "file:/home/u/work.gitconfig\0user.name\nWork",
            "file:/home/u/.gitconfig\0core.askpass",
            "",
        ]
        .join("\0");

        assert_eq!(
            given(&listed, &|_| true, &|_| true),
            [
                (
                    "includeif.hasconfig:remote.*.url:git@forge.invalid:**.path".to_owned(),
                    "/home/u/forge.gitconfig".to_owned()
                ),
                (
                    "includeif.hasconfig:remote.*.url:https://a.invalid/**.path".to_owned(),
                    "~/a.gitconfig".to_owned()
                ),
                ("user.name".to_owned(), "Work".to_owned()),
                ("core.askpass".to_owned(), "true".to_owned()),
            ]
        );
    }

    #[test]
    fn the_settings_of_a_file_made_after_the_start_are_not_given() {
        let listed = [
            "file:/home/u/.gitconfig\0user.name\nKris",
            "file:/home/u/later.gitconfig\0http.proxy\nhttp://127.0.0.1:9",
            "file:/home/u/.gitconfig\0includeif.hasconfig:remote.*.url:https://a.invalid/**.path\nlater.gitconfig",
            "file:/home/u/.gitconfig\0includeif.hasconfig:remote.*.url:https://b.invalid/**.path\nthen.gitconfig",
            "",
        ]
        .join("\0");

        let kept = given(
            &listed,
            &|file| file == Path::new("/home/u/.gitconfig"),
            &|include| include == "/home/u/then.gitconfig",
        );

        assert_eq!(
            kept,
            [
                ("user.name".to_owned(), "Kris".to_owned()),
                (
                    "includeif.hasconfig:remote.*.url:https://b.invalid/**.path".to_owned(),
                    "/home/u/then.gitconfig".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn an_include_outside_ssh_or_not_read_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let account = temp.path();
        let ssh = account.join(".ssh");
        std::fs::create_dir_all(ssh.join("conf.d")).unwrap();
        std::fs::write(ssh.join("conf.d/a"), "").unwrap();
        std::fs::write(ssh.join("conf.d/.hidden"), "").unwrap();

        assert_eq!(
            includes("conf.d/*", &ssh, account).unwrap(),
            [ssh.join("conf.d/a")]
        );
        assert_eq!(
            includes("~/.ssh/conf.d/a", &ssh, account).unwrap(),
            [ssh.join("conf.d/a")]
        );
        for refused in [
            "~/notes",
            "/etc/ssh/extra",
            "../dots/x",
            "conf.d/../../x",
            "~other/x",
            "${HOME}/.ssh/x",
            "%d/.ssh/x",
            "conf.[ad]/*",
        ] {
            assert!(includes(refused, &ssh, account).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_program_ssh_s_configuration_runs_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let config = ssh.join("config");
        for (text, refused) in [
            ("Host x\n  User git\n  ProxyCommand none\n", false),
            ("Host x\n  ProxyJump bastion\n", false),
            ("Host x\n\tProxyCommand nc %h %p\n", true),
            ("host x\n  proxycommand=nc %h %p\n", true),
            ("Match host x exec \"true\"\n", true),
            ("Match host x !exec true\n", true),
            ("PKCS11Provider /usr/lib/p11.so\n", true),
            ("LocalCommand echo\n", true),
            ("# ProxyCommand nc\n", false),
            ("\"ProxyCommand\" nc %h %p\n", true),
            ("Proxy\"Command\"=nc %h %p\n", true),
            ("Match host x \"ex\"ec true\n", true),
        ] {
            std::fs::write(&config, text).unwrap();
            assert_eq!(
                configuration(&config, &ssh, temp.path(), 0, &mut Read::default()).is_err(),
                refused,
                "{text}"
            );
        }
        // Through an include.
        std::fs::write(ssh.join("more"), "ProxyCommand nc %h %p\n").unwrap();
        std::fs::write(&config, "Include more\n").unwrap();
        assert!(configuration(&config, &ssh, temp.path(), 0, &mut Read::default()).is_err());
    }

    #[test]
    fn another_mount_of_the_same_files_is_found() {
        let one = "22 1 0:35 /@home /home rw,relatime shared:1 - btrfs /dev/x rw\n\
                   23 1 0:35 /@ / rw shared:2 - btrfs /dev/x rw\n\
                   24 1 0:22 / /tmp rw shared:3 - tmpfs tmpfs rw\n";
        assert_eq!(alias(Path::new("/home/kris/.ssh"), one), None);
        assert_eq!(below(Path::new("/home/kris/.ssh"), one), None);
        let inside = format!("{one}27 22 0:40 / /home/kris/.ssh/remote rw - fuse.sshfs x rw\n");
        assert_eq!(
            below(Path::new("/home/kris/.ssh"), &inside),
            Some(PathBuf::from("/home/kris/.ssh/remote"))
        );
        let top = format!("{one}25 1 0:35 / /mnt/top\\040volume rw shared:4 - btrfs /dev/x rw\n");
        assert_eq!(
            alias(Path::new("/home/kris/.ssh"), &top),
            Some(PathBuf::from("/mnt/top volume/@home/kris/.ssh"))
        );
        let bound = format!("{one}26 1 0:35 /@home/kris /srv/kris rw - btrfs /dev/x rw\n");
        assert_eq!(
            alias(Path::new("/home/kris/.ssh"), &bound),
            Some(PathBuf::from("/srv/kris/.ssh"))
        );
    }

    /// What a daemon starting now finds in the home `home`.
    fn found_now(home: &Path) -> Found {
        Found {
            account: home.to_owned(),
            home: Some(home.to_owned()),
            ssh: home.join(".ssh"),
            git: Vec::new(),
            read_only: Vec::new(),
            pinned: Vec::new(),
            made: false,
            started: std::time::SystemTime::now(),
        }
    }

    #[test]
    fn a_program_of_ssh_a_run_in_full_may_have_written_is_not_run() {
        let temp = tempfile::tempdir().unwrap();
        let wrapper = temp.path().join(".ssh/wrap");
        std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
        std::fs::write(&wrapper, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let found = found_now(temp.path());
        assert!(checked("~/.ssh/wrap", Some(&found)).is_ok());
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Changed in place since the daemon's start, as a run in Full may (`~/.ssh` is read-only
        // to a confined run alone), or made since.
        std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
        let made = temp.path().join(".ssh/made");
        std::fs::write(&made, "").unwrap();

        for refused in ["~/.ssh/wrap", "~/.ssh/made", "~/.ssh/missing"] {
            let why = checked(refused, Some(&found)).unwrap_err();
            assert!(why.contains("since the daemon's start"), "{refused}: {why}");
        }
    }

    #[test]
    fn a_replaced_settings_file_says_to_restart_the_daemon_or_work_in_full() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join(".gitconfig");
        std::fs::write(&settings, "").unwrap();
        let mut found = found_now(temp.path());
        found
            .read_only
            .push((settings.clone(), identity(&settings).unwrap()));
        assert!(changed(&found, &settings).is_none());

        // As `git config --global` in Full does: a new file renamed over the old.
        let new = temp.path().join(".gitconfig.lock");
        std::fs::write(&new, "[user]\n").unwrap();
        std::fs::rename(&new, &settings).unwrap();

        let why = changed(&found, &settings).unwrap();
        assert!(why.contains("restart the daemon, or work in Full"), "{why}");
    }

    #[test]
    fn the_user_s_ssh_command_runs_nothing_a_run_can_change() {
        let temp = tempfile::tempdir().unwrap();
        let wrapper = temp.path().join(".ssh/wrap");
        std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
        std::fs::write(&wrapper, "").unwrap();
        let found = found_now(temp.path());
        assert!(checked("ssh -i ~/.ssh/work -o IdentitiesOnly=yes", Some(&found)).is_ok());
        // Its first options, which win over any later: no shared connection, no forwarding.
        assert_eq!(
            checked(&format!("{} -v", wrapper.display()), Some(&found)).unwrap(),
            OsString::from(format!(
                "{} -o ControlMaster=no -o ControlPath=none -o ForwardAgent=no -v",
                wrapper.display()
            ))
        );
        let own = checked("ssh -i ~/.ssh/work", Some(&found)).unwrap();
        assert!(
            own.to_string_lossy().ends_with(
                " -o ControlMaster=no -o ControlPath=none -o ForwardAgent=no -i ~/.ssh/work"
            ),
            "{own:?}"
        );
        assert!(checked("~/.ssh/wrap -v", Some(&found)).is_ok());
        assert!(checked("ssh -F /dev/null -i ~/.ssh/work", Some(&found)).is_ok());
        assert!(checked("ssh -vp 2222 -l git", Some(&found)).is_ok());
        for refused in [
            "ssh -vF/tmp/config",
            "ssh -vF /tmp/config",
            "ssh -A",
            "ssh -vA",
            "ssh -S /tmp/master",
            "ssh -M",
            "ssh -oControlPath=/tmp/master",
            "ssh -o ControlMaster=auto",
            "ssh -o ForwardAgent=yes",
            "ssh -o controlpersist=yes",
            "~/bin/wrap",
            "/tmp/wrap",
            "ssh -F ~/dots/ssh_config",
            "ssh -oProxyCommand=nc",
            "ssh -o ProxyCommand=nc",
            "ssh -I /tmp/p11.so",
            "ssh $(cat x)",
            "ssh; true",
            "~/.ssh/../bin/wrap",
        ] {
            assert!(checked(refused, Some(&found)).is_err(), "{refused}");
        }
    }

    #[test]
    fn only_the_folders_a_command_could_move_are_kept_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("dots/git/config");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "").unwrap();
        let way = on_the_way(&file);
        // The temporary folder is the user's, so is all below it: kept. `/` and `/tmp` are not.
        assert!(way.contains(&temp.path().join("dots/git")));
        assert!(way.contains(&temp.path().join("dots")));
        assert!(!way.contains(&PathBuf::from("/")));
        // SAFETY: getuid cannot fail.
        if unsafe { libc::getuid() } != 0 {
            assert!(!way.contains(&PathBuf::from("/tmp")));
        }
    }
}
