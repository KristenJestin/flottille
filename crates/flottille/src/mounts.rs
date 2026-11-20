//! The private mount namespace a confined command starts in (`docs/vault.md`, "The runtime
//! folder"): a user namespace of its own, where the user's runtime folders and X11's socket
//! folder are empty, so that no socket in them is reached by its path, whatever Landlock's ABI;
//! and where `~/.ssh` is read-only. Only what the daemon itself gives the command is put back in
//! the emptied folders: its display folder, the SSH agent's socket it is lent. Landlock's rules
//! (`landlock.rs`) apply after it, unchanged: this is a layer more.
//!
//! The mounts are the command's own, made before it runs its program: it loses every capability
//! at that `exec` (its user is not root in the namespace), and a namespace it makes inside gets
//! them locked, so it can neither unmount nor remount them.

use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const OPEN_TREE_CLONE: libc::c_uint = 1;
const AT_RECURSIVE: libc::c_uint = 0x8000;
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 0x4;

/// The variable that plays a kernel refusing user namespaces, read by a test build only: the
/// tests' daemon is given it, never a packaged one.
const REFUSED: &str = "FLOTTILLE_TEST_NO_USER_NAMESPACES";

/// What a kernel that refuses user namespaces leaves open, as the daemon's log and
/// `flottille doctor` say it.
pub const REFUSED_GAP: &str = "the kernel gives no user namespace to the commands Flottille \
     confines: they run without their private mount namespace, and reach the sockets of the \
     user's runtime folder (the session bus) and X11's by their path where Landlock does not \
     deny connecting, and can write ~/.ssh (docs/vault.md, \"The gaps\")";

/// A folder or a file put back in an emptied folder: cloned at its path before the folder is
/// emptied, moved onto the same path after, the folders on the way made first.
struct Kept {
    source: CString,
    ways: Vec<CString>,
    target: CString,
    file: bool,
}

/// What the command's namespace is made of, all of it made before the fork: the child only makes
/// system calls with it.
pub struct Plan {
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
    read_only: Vec<(CString, libc::c_ulong)>,
    emptied: Vec<CString>,
    kept: Vec<Kept>,
    trees: Vec<libc::c_int>,
    cwd: Option<CString>,
}

/// Whether the kernel gives a confined command a private mount namespace: probed once, by a
/// child that makes one. Refused (a kernel built without user namespaces, a distribution that
/// forbids them to unprivileged users), the command runs without it, and the gap is said
/// (`flottille doctor`, the daemon's log).
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let refused = cfg!(debug_assertions) && std::env::var_os(REFUSED).is_some();
        !refused && probe()
    })
}

/// A child that makes the namespace and empties `/` in it, then ends.
fn probe() -> bool {
    let Some(mut plan) = plan(&[PathBuf::from("/")], &[], &[], None) else {
        return false;
    };
    // SAFETY: the child makes system calls alone, on what was allocated before the fork, then
    // ends without running anything of the parent's.
    unsafe {
        match libc::fork() {
            -1 => false,
            0 => libc::_exit(if plan.enter().is_ok() { 0 } else { 1 }),
            child => {
                let mut status = 0;
                while libc::waitpid(child, &mut status, 0) < 0
                    && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
                {
                }
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
            }
        }
    }
}

/// The namespace that empties the `emptied` folders, puts the `kept` paths in them back, makes
/// the `read_only` paths so, and goes back to `cwd` in it. None where a path cannot be a C
/// string.
pub fn plan(
    emptied: &[PathBuf],
    kept: &[PathBuf],
    read_only: &[PathBuf],
    cwd: Option<&Path>,
) -> Option<Plan> {
    let c = |path: &Path| CString::new(path.as_os_str().as_bytes()).ok();
    // A folder inside another one emptied is gone with it.
    let mut outer: Vec<&PathBuf> = emptied.iter().collect();
    outer.sort();
    outer.dedup();
    let mut folders: Vec<&PathBuf> = Vec::new();
    for folder in outer {
        if !folders.iter().any(|done| folder.starts_with(done)) {
            folders.push(folder);
        }
    }
    let mut put_back = Vec::new();
    for path in kept {
        let Some(folder) = folders.iter().find(|folder| path.starts_with(folder)) else {
            continue;
        };
        let mut ways: Vec<CString> = path
            .ancestors()
            .skip(1)
            .take_while(|way| way != folder)
            .map(c)
            .collect::<Option<_>>()?;
        ways.reverse();
        put_back.push(Kept {
            source: c(path)?,
            ways,
            target: c(path)?,
            file: !path.is_dir(),
        });
    }
    let mut kept_read_only = Vec::new();
    for path in read_only {
        let Some(flags) = locked_flags(path) else {
            continue;
        };
        kept_read_only.push((c(path)?, flags));
    }
    // SAFETY: getuid and getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    Some(Plan {
        uid_map: format!("{uid} {uid} 1").into_bytes(),
        gid_map: format!("{gid} {gid} 1").into_bytes(),
        read_only: kept_read_only,
        emptied: folders.into_iter().map(|f| c(f)).collect::<Option<_>>()?,
        trees: vec![-1; put_back.len()],
        kept: put_back,
        cwd: cwd.filter(|cwd| cwd.is_absolute()).and_then(c),
    })
}

/// The flags of the mount `path` is on that a namespace of the user's cannot clear: a remount
/// read-only keeps them, or is refused. None when the path is not there.
fn locked_flags(path: &Path) -> Option<libc::c_ulong> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero statvfs is a valid value for the call to fill.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a statvfs of ours.
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let mut flags = 0;
    for (st, ms) in [
        (libc::ST_NOSUID, libc::MS_NOSUID),
        (libc::ST_NODEV, libc::MS_NODEV),
        (libc::ST_NOEXEC, libc::MS_NOEXEC),
        (libc::ST_NOATIME, libc::MS_NOATIME),
        (libc::ST_NODIRATIME, libc::MS_NODIRATIME),
        (libc::ST_RELATIME, libc::MS_RELATIME),
    ] {
        if stat.f_flag & st != 0 {
            flags |= ms;
        }
    }
    Some(flags)
}

fn check(result: libc::c_long) -> io::Result<()> {
    match result {
        0.. => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

/// `bytes` written to the file at `path`, as one write.
fn write_file(path: &CStr, bytes: &[u8]) -> io::Result<()> {
    // SAFETY: a valid C string, and a buffer alive for the call.
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        check(fd as libc::c_long)?;
        let written = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
        let error = io::Error::last_os_error();
        libc::close(fd);
        if written != bytes.len() as isize {
            return Err(error);
        }
    }
    Ok(())
}

impl Plan {
    /// In the child, before its program: the namespace made as planned. Any failure fails the
    /// command's start: one that started without its folders emptied would reach them.
    pub fn enter(&mut self) -> io::Result<()> {
        let none = std::ptr::null::<libc::c_char>();
        // SAFETY: plain system calls on C strings and buffers of the plan, alive for each call;
        // nothing allocates.
        unsafe {
            check(libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) as libc::c_long)?;
            write_file(c"/proc/self/setgroups", b"deny")?;
            write_file(c"/proc/self/uid_map", &self.uid_map)?;
            write_file(c"/proc/self/gid_map", &self.gid_map)?;
            // Nothing the user's session mounts later (a runtime folder made again at a new
            // login) reaches in.
            check(libc::mount(
                none,
                c"/".as_ptr(),
                none,
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            ) as libc::c_long)?;
            for (kept, tree) in self.kept.iter().zip(self.trees.iter_mut()) {
                let fd = libc::syscall(
                    libc::SYS_open_tree,
                    libc::AT_FDCWD,
                    kept.source.as_ptr(),
                    OPEN_TREE_CLONE | libc::O_CLOEXEC as libc::c_uint | AT_RECURSIVE,
                );
                check(fd)?;
                *tree = fd as libc::c_int;
            }
            for (path, flags) in &self.read_only {
                check(libc::mount(
                    path.as_ptr(),
                    path.as_ptr(),
                    none,
                    libc::MS_BIND | libc::MS_REC,
                    std::ptr::null(),
                ) as libc::c_long)?;
                check(libc::mount(
                    none,
                    path.as_ptr(),
                    none,
                    libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY | flags,
                    std::ptr::null(),
                ) as libc::c_long)?;
            }
            for folder in &self.emptied {
                check(libc::mount(
                    c"tmpfs".as_ptr(),
                    folder.as_ptr(),
                    c"tmpfs".as_ptr(),
                    libc::MS_NOSUID | libc::MS_NODEV,
                    c"mode=0700".as_ptr().cast(),
                ) as libc::c_long)?;
            }
            for (kept, tree) in self.kept.iter().zip(self.trees.iter()) {
                for way in &kept.ways {
                    if libc::mkdir(way.as_ptr(), 0o700) != 0
                        && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
                if kept.file {
                    let fd = libc::open(
                        kept.target.as_ptr(),
                        libc::O_CREAT | libc::O_WRONLY | libc::O_CLOEXEC,
                        0o600,
                    );
                    check(fd as libc::c_long)?;
                    libc::close(fd);
                } else {
                    check(libc::mkdir(kept.target.as_ptr(), 0o700) as libc::c_long)?;
                }
                check(libc::syscall(
                    libc::SYS_move_mount,
                    *tree,
                    c"".as_ptr(),
                    libc::AT_FDCWD,
                    kept.target.as_ptr(),
                    MOVE_MOUNT_F_EMPTY_PATH,
                ))?;
                libc::close(*tree);
            }
            // The folder it was started in, as the namespace shows it: one it held before an
            // emptied folder hid it is not kept.
            if let Some(cwd) = &self.cwd {
                check(libc::chdir(cwd.as_ptr()) as libc::c_long)?;
            }
        }
        Ok(())
    }
}

/// What a confined command gets read-only: the user's `~/.ssh` (of `HOME` and of the account,
/// when they differ), where it leads when it is a link, every link in it that leads out of it,
/// and every file its `config` includes out of it, through the files they include in turn.
/// `ProxyCommand` and `Match exec` there run with the user's credentials when they push.
pub fn ssh_read_only() -> Vec<PathBuf> {
    let homes: Vec<PathBuf> = std::env::var_os("HOME")
        .map(PathBuf::from)
        .into_iter()
        .chain(account_home())
        .collect();
    let mut folders: Vec<(PathBuf, PathBuf)> = Vec::new();
    for home in &homes {
        if let Ok(folder) = home.join(".ssh").canonicalize()
            && !folders.iter().any(|(_, f)| *f == folder)
        {
            folders.push((home.clone(), folder));
        }
    }
    let inside = |path: &Path, folders: &[(PathBuf, PathBuf)]| {
        folders.iter().any(|(_, folder)| path.starts_with(folder))
    };
    let mut out: Vec<PathBuf> = folders.iter().map(|(_, folder)| folder.clone()).collect();
    let mut linked = Vec::new();
    for (_, folder) in &folders {
        links(folder, 0, &mut linked);
    }
    for target in linked {
        if !inside(&target, &folders) {
            out.push(target);
        }
    }
    let mut seen = Vec::new();
    for (home, folder) in &folders {
        included(&folder.join("config"), home, folder, 0, &mut seen);
    }
    for file in seen {
        if !inside(&file, &folders) {
            out.push(file);
        }
    }
    out.sort();
    out.dedup();
    // One inside another of the list is read-only with it.
    let mut kept: Vec<PathBuf> = Vec::new();
    for path in out {
        if !kept.iter().any(|done| path.starts_with(done)) {
            kept.push(path);
        }
    }
    kept
}

/// The account's own home, as the password database says it; in a test build, the one
/// `FLOTTILLE_TEST_ACCOUNT_HOME` names, so that no test reads the developer's own.
fn account_home() -> Option<PathBuf> {
    if cfg!(debug_assertions)
        && let Some(home) = std::env::var_os("FLOTTILLE_TEST_ACCOUNT_HOME")
    {
        return Some(PathBuf::from(home));
    }
    let mut buffer = vec![0 as libc::c_char; 4096];
    // SAFETY: an all-zero passwd is a valid value for the call to fill.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    // SAFETY: the entry and the buffer are ours, alive for the call and while the strings are
    // read.
    let result = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    if result != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    // SAFETY: the call filled pw_dir with a C string in the buffer.
    let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

/// Where each link under `folder` leads, followed through the folders in it.
fn links(folder: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            if let Ok(target) = entry.path().canonicalize() {
                out.push(target);
            }
        } else if kind.is_dir() && depth < 8 {
            links(&entry.path(), depth + 1, out);
        }
    }
}

/// The files an SSH configuration includes, and those they include, each once: a relative path is
/// the user's `~/.ssh`'s, as `ssh` reads the user's own configuration.
fn included(config: &Path, home: &Path, folder: &Path, depth: usize, seen: &mut Vec<PathBuf>) {
    let Ok(text) = std::fs::read_to_string(config) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim_start();
        let split = line
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(line.len());
        let (key, rest) = line.split_at(split);
        if !key.eq_ignore_ascii_case("include") {
            continue;
        }
        let rest = rest.trim_start().trim_start_matches('=');
        for word in rest.split_whitespace() {
            let word = word.trim_matches('"');
            let pattern = match word.strip_prefix("~/") {
                Some(under) => home.join(under),
                None => folder.join(word),
            };
            for file in glob(&pattern) {
                let Ok(file) = file.canonicalize() else {
                    continue;
                };
                if seen.contains(&file) {
                    continue;
                }
                seen.push(file.clone());
                if depth < 16 {
                    included(&file, home, folder, depth + 1, seen);
                }
            }
        }
    }
}

/// The paths a shell pattern matches, as `ssh` expands an `Include`.
fn glob(pattern: &Path) -> Vec<PathBuf> {
    let Ok(c) = CString::new(pattern.as_os_str().as_bytes()) else {
        return Vec::new();
    };
    // SAFETY: an all-zero glob_t is the value the call expects to fill.
    let mut found: libc::glob_t = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a glob_t of ours, freed below.
    let result = unsafe { libc::glob(c.as_ptr(), 0, None, &mut found) };
    let mut paths = Vec::new();
    if result == 0 {
        for i in 0..found.gl_pathc {
            // SAFETY: the call filled gl_pathc C strings.
            let path = unsafe { CStr::from_ptr(*found.gl_pathv.add(i)) };
            paths.push(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())));
        }
    }
    // SAFETY: filled by glob, freed once.
    unsafe { libc::globfree(&mut found) };
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_kept_is_put_back_under_the_emptied_one_with_the_folders_on_its_way() {
        let runtime = PathBuf::from("/run/user/1000");
        let display = runtime.join("flottille/display/browser");
        let plan = plan(
            &[
                runtime.clone(),
                runtime.join("inner"),
                PathBuf::from("/x11"),
            ],
            std::slice::from_ref(&display),
            &[],
            None,
        )
        .unwrap();
        let names = |list: &[CString]| -> Vec<String> {
            list.iter()
                .map(|c| c.to_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(names(&plan.emptied), ["/run/user/1000", "/x11"]);
        assert_eq!(
            names(&plan.kept[0].ways),
            [
                "/run/user/1000/flottille",
                "/run/user/1000/flottille/display"
            ]
        );
    }

    #[test]
    fn an_emptied_folder_holds_what_is_kept_alone_and_cannot_be_unmounted() {
        use std::os::unix::process::CommandExt;
        if !available() {
            eprintln!("skipped: this kernel refuses user namespaces to this user");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("runtime")).unwrap();
        let runtime = temp.path().join("runtime").canonicalize().unwrap();
        let shown = runtime.join("flottille/display/browser");
        std::fs::create_dir_all(&shown).unwrap();
        std::fs::write(shown.join("wayland-9"), "").unwrap();
        let _bus = std::os::unix::net::UnixListener::bind(runtime.join("bus")).unwrap();
        let mut plan = plan(
            std::slice::from_ref(&runtime),
            std::slice::from_ref(&shown),
            &[],
            None,
        )
        .unwrap();
        let mut command = std::process::Command::new("python3");
        command
            .args([
                "-c",
                "import ctypes, json, os, sys\n\
                 libc = ctypes.CDLL(None, use_errno=True)\n\
                 out = {'runtime': sorted(os.listdir(sys.argv[1])),\n\
                     'shown': sorted(os.listdir(sys.argv[2])),\n\
                     'umount': libc.umount2(sys.argv[1].encode(), 2)}\n\
                 print(json.dumps(out))",
            ])
            .arg(&runtime)
            .arg(&shown);
        // SAFETY: as in `landlock::confine`.
        unsafe { command.pre_exec(move || plan.enter()) };
        let output = command.output().unwrap();
        let out: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

        assert_eq!(out["runtime"], serde_json::json!(["flottille"]), "{out}");
        assert_eq!(out["shown"], serde_json::json!(["wayland-9"]), "{out}");
        assert_eq!(out["umount"], -1, "{out}");
    }

    #[test]
    fn the_ssh_includes_out_of_the_folder_are_found_through_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::create_dir_all(dir.path().join("dots/conf.d")).unwrap();
        std::fs::write(
            ssh.join("config"),
            format!(
                "Host a\n  User me\nInclude {}/dots/conf.d/*.conf local\n",
                dir.path().display()
            ),
        )
        .unwrap();
        std::fs::write(ssh.join("local"), "include ~/more\n").unwrap();
        std::fs::write(home.join("more"), "").unwrap();
        std::fs::write(dir.path().join("dots/conf.d/one.conf"), "").unwrap();

        let mut seen = Vec::new();
        included(&ssh.join("config"), &home, &ssh, 0, &mut seen);
        seen.sort();

        let canonical = |p: PathBuf| p.canonicalize().unwrap();
        let mut wanted = vec![
            canonical(dir.path().join("dots/conf.d/one.conf")),
            canonical(ssh.join("local")),
            canonical(home.join("more")),
        ];
        wanted.sort();
        assert_eq!(seen, wanted);
    }
}
