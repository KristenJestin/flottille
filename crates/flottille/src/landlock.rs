//! The user's runtime folder kept out of a run's and a connection server's reach
//! (`docs/vault.md`, "The runtime folder"): its session bus leads to the keyring, and the bus's
//! socket is at a path anyone can guess. Landlock grants only what a rule says, so the rules
//! cover every folder but the runtime ones: each entry of `/`, and of each folder on the way to
//! one of them, but that way itself. Applied in the child before it runs its program, never to
//! the daemon.
//!
//! X11's display is kept out by its abstract name, unless the program is given it (a
//! [`crate::display::Folder`] made for X11): an X11 client reads the keys typed in other windows.
//! The sixth ABI's scope keeps out every abstract socket made outside the program. Its sockets by
//! their path under `/tmp/.X11-unix` stay open: a rule on `/tmp` would take from a program what
//! it makes there, and #976 masks the folder instead (`docs/vault.md`, "The gaps").

use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const CREATE_RULESET_VERSION: libc::c_uint = 1;
const RULE_PATH_BENEATH: libc::c_uint = 1;

const EXECUTE: u64 = 1 << 0;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const REFER: u64 = 1 << 13;
const TRUNCATE: u64 = 1 << 14;
const IOCTL_DEV: u64 = 1 << 15;
/// Connecting to a socket by its path: only from the ninth ABI on.
const RESOLVE_UNIX: u64 = 1 << 16;
/// Abstract sockets made outside the program's domain are out of its reach: from the sixth ABI.
const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
/// The rights a rule on a file, not a folder, may hold.
const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV | RESOLVE_UNIX;

#[repr(C, packed)]
struct PathBeneath {
    allowed_access: u64,
    parent_fd: i32,
}

/// The ABI of the running kernel's Landlock, 0 where it has none.
pub fn abi() -> i64 {
    // SAFETY: with a null attribute and the version flag, the call only returns a number.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<u8>(),
            0usize,
            CREATE_RULESET_VERSION,
        )
    };
    abi.max(0)
}

/// Every right the ABI knows. The first ABI is left out: it refuses every move of a file to
/// another folder, which breaks more than it keeps out.
fn rights(abi: i64) -> u64 {
    let mut rights = (1 << 13) - 1;
    if abi >= 2 {
        rights |= REFER;
    }
    if abi >= 3 {
        rights |= TRUNCATE;
    }
    if abi >= 5 {
        rights |= IOCTL_DEV;
    }
    if abi >= 9 {
        rights |= RESOLVE_UNIX;
    }
    rights
}

/// The user's runtime folders: the daemon's `XDG_RUNTIME_DIR` and `/run/user/<uid>`.
fn runtime_folders() -> Vec<PathBuf> {
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut folders: Vec<PathBuf> = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .into_iter()
        .chain([PathBuf::from(format!("/run/user/{uid}"))])
        .filter_map(|folder| folder.canonicalize().ok())
        .filter(|folder| folder != Path::new("/"))
        .collect();
    folders.dedup();
    folders
}

/// X11's socket folder: `/tmp/.X11-unix`, or the tests' own (`FLOTTILLE_X11_SOCKETS`). The one
/// place it is named, for the private mount namespace (#976) to mask.
#[allow(dead_code)]
fn x11_folder() -> Option<PathBuf> {
    std::env::var_os("FLOTTILLE_X11_SOCKETS")
        .map_or_else(|| PathBuf::from("/tmp/.X11-unix"), PathBuf::from)
        .canonicalize()
        .ok()
        .filter(|folder| folder != Path::new("/"))
}

/// Every path under `dir` but the `denied` folders: an entry on the way to one of them is
/// opened up in turn. A symbolic link gets no rule: what it leads to has its own place.
fn allowed(dir: &Path, denied: &[PathBuf], out: &mut Vec<CString>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() || denied.contains(&path) {
            continue;
        }
        if kind.is_dir() && denied.iter().any(|folder| folder.starts_with(&path)) {
            allowed(&path, denied, out);
        } else if let Ok(path) = CString::new(path.as_os_str().as_bytes()) {
            out.push(path);
        }
    }
}

/// The command runs kept out of the user's runtime folders where the kernel has Landlock (its
/// second ABI or later); elsewhere it runs as before (`docs/vault.md` says the gap).
pub fn confined(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    confined_but(command, None)
}

/// [`confined`], `open` (the display folder the daemon made for this command alone) left within
/// its reach, but for the right to link or move a file out of it: the socket in it is not
/// linked elsewhere. A [`crate::display::Folder`] and no path: only the daemon's own folder is
/// ever an exception, never one an environment variable or a request names.
pub fn confined_but<'a>(
    command: &'a mut tokio::process::Command,
    open: Option<&crate::display::Folder>,
) -> &'a mut tokio::process::Command {
    confine(command, open, false)
}

/// [`confined`], lent the user's SSH agent: the right to connect to the socket found at the
/// daemon's start, and nothing else of the folder it is in. Never a path a variable or a
/// request names. For the daemon's own fetch of an SSH remote (`git::remote`).
pub fn confined_lent(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    confine(command, None, true)
}

fn confine<'a>(
    command: &'a mut tokio::process::Command,
    open: Option<&crate::display::Folder>,
    lent: bool,
) -> &'a mut tokio::process::Command {
    let abi = abi();
    let mut denied = runtime_folders();
    let rights = rights(abi);
    let connecting = rights & RESOLVE_UNIX != 0;
    // The user's SSH agent's socket outside the runtime folders: denied by its own path, and each
    // folder on the way to it ruled without the right to connect, so that the command still makes
    // its own files there. One inside them is denied with them. Below the ninth ABI connecting is
    // not governed: only the run's environment, which does not name the agent, keeps it out
    // (`runs::defend`).
    let mut ways = Vec::new();
    let kept = crate::ssh_agent::socket()
        .filter(|_| connecting && !lent)
        .filter(|socket| !denied.iter().any(|folder| socket.starts_with(folder)));
    if let Some(socket) = kept {
        ways = socket
            .ancestors()
            .skip(1)
            .filter(|way| !denied.iter().any(|folder| folder.starts_with(way)))
            .filter_map(|way| CString::new(way.as_os_str().as_bytes()).ok())
            .collect();
        denied.push(socket.to_owned());
    }
    // X11's abstract sockets too, unless given, where the kernel has the scope.
    let given_x11 = open.is_some_and(crate::display::Folder::x11);
    let scoped = match !given_x11 && abi >= 6 {
        true => SCOPE_ABSTRACT_UNIX_SOCKET,
        false => 0,
    };
    if abi < 2 || (denied.is_empty() && scoped == 0) {
        return command;
    }
    let mut paths = Vec::new();
    allowed(Path::new("/"), &denied, &mut paths);
    let open = open.and_then(|folder| CString::new(folder.path().as_os_str().as_bytes()).ok());
    // A rule on the lent socket itself: Landlock checks connecting from the socket's own path up,
    // so the folder it is in stays denied. One outside the runtime folders is not denied at all.
    let lent = crate::ssh_agent::socket()
        .filter(|_| connecting && lent)
        .and_then(|socket| CString::new(socket.as_os_str().as_bytes()).ok());
    // SAFETY: the closure allocates nothing and only makes system calls (open, fstat,
    // landlock_*, prctl, close), all async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            restrict(
                rights,
                &paths,
                open.as_deref(),
                scoped,
                &ways,
                lent.as_deref(),
            )
        })
    }
}

/// In the child: every path a rule with every right, `open` one without `REFER`, the `ways` to
/// the SSH agent's socket without the right to connect, the `lent` socket with that right alone,
/// then the child bound to them and to the `scoped` scopes.
fn restrict(
    rights: u64,
    paths: &[CString],
    open: Option<&CStr>,
    scoped: u64,
    ways: &[CString],
    lent: Option<&CStr>,
) -> io::Result<()> {
    // The attribute's fields: the handled rights of files, of the network (none), the scopes.
    let handled = [rights, 0, scoped];
    // A kernel older than the scopes refuses a longer attribute: the rights' field alone then.
    let size = match scoped {
        0 => std::mem::size_of::<u64>(),
        _ => std::mem::size_of_val(&handled),
    };
    // SAFETY: the attribute is `u64` fields of the size given, which the kernel reads as the
    // first fields of its own, the rest zero.
    let ruleset =
        unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, handled.as_ptr(), size, 0) };
    if ruleset < 0 {
        return Err(io::Error::last_os_error());
    }
    let ruleset = ruleset as libc::c_int;
    let ruled = paths
        .iter()
        .map(|path| (path.as_c_str(), rights))
        .chain(open.map(|path| (path, rights & !REFER)))
        .chain(
            ways.iter()
                .map(|way| (way.as_c_str(), rights & !RESOLVE_UNIX)),
        )
        .chain(lent.map(|socket| (socket, RESOLVE_UNIX)));
    for (path, granted) in ruled {
        // SAFETY: a valid C string; O_PATH opens nothing but a handle.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            continue;
        }
        // SAFETY: an all-zero stat is a valid value for fstat to fill.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: fd is open and stat is ours.
        let is_dir = unsafe { libc::fstat(fd, &mut stat) } == 0
            && stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
        let mut access = if is_dir {
            granted
        } else {
            granted & FILE_RIGHTS
        };
        loop {
            let rule = PathBeneath {
                allowed_access: access,
                parent_fd: fd,
            };
            // SAFETY: the rule is the packed attribute the kernel reads, alive for the call.
            let added = unsafe {
                libc::syscall(
                    libc::SYS_landlock_add_rule,
                    ruleset,
                    RULE_PATH_BENEATH,
                    &rule as *const PathBeneath,
                    0,
                )
            };
            // A kernel that holds connecting by path a folder's right only: the file is ruled
            // without it. Any other refusal leaves the path out, which denies it.
            if added < 0 && !is_dir && access & RESOLVE_UNIX != 0 {
                access &= !RESOLVE_UNIX;
                continue;
            }
            break;
        }
        // SAFETY: fd is ours.
        unsafe { libc::close(fd) };
    }
    // SAFETY: plain calls on numbers; no_new_privs is what lets an unprivileged process bind
    // itself.
    let bound = unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            -1
        } else {
            libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0)
        }
    };
    let error = io::Error::last_os_error();
    // SAFETY: the ruleset is ours.
    unsafe { libc::close(ruleset) };
    if bound != 0 {
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_way_to_a_denied_folder_is_opened_up_and_the_folder_left_out() {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("run/user/1000");
        std::fs::create_dir_all(runtime.join("bus-folder")).unwrap();
        std::fs::create_dir_all(root.path().join("run/lock")).unwrap();
        std::fs::create_dir_all(root.path().join("home")).unwrap();
        std::os::unix::fs::symlink(root.path().join("run"), root.path().join("var-run")).unwrap();

        let mut paths = Vec::new();
        allowed(root.path(), &[runtime], &mut paths);
        let mut paths: Vec<String> = paths
            .into_iter()
            .map(|path| path.into_string().unwrap())
            .collect();
        paths.sort();

        let at = |sub: &str| root.path().join(sub).to_string_lossy().into_owned();
        assert_eq!(paths, [at("home"), at("run/lock")]);
    }

    /// Whether a child confined out of `runtime`, `open` left within its reach, connects to the
    /// socket at `path`.
    fn connects(runtime: &Path, open: &Path, path: &Path) -> bool {
        use std::os::unix::process::CommandExt;
        let mut paths = Vec::new();
        allowed(
            Path::new("/"),
            &[runtime.canonicalize().unwrap()],
            &mut paths,
        );
        let open = CString::new(open.as_os_str().as_bytes()).unwrap();
        let rights = rights(abi());
        let mut command = std::process::Command::new("python3");
        command.args([
            "-c",
            "import socket, sys\ns = socket.socket(socket.AF_UNIX)\ns.connect(sys.argv[1])",
        ]);
        command.arg(path).stderr(std::process::Stdio::null());
        // SAFETY: as in `confined_but`.
        unsafe { command.pre_exec(move || restrict(rights, &paths, Some(&open), 0, &[], None)) };
        command.status().unwrap().success()
    }

    #[test]
    fn a_shown_browser_reaches_the_wayland_socket() {
        if abi() < 2 {
            eprintln!(
                "skipped: this kernel's Landlock (ABI {}) is not used",
                abi()
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let folder = runtime.join("flottille/display/browser");
        std::fs::create_dir_all(&folder).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(runtime.join("wayland-9")).unwrap();
        std::fs::hard_link(runtime.join("wayland-9"), folder.join("wayland-9")).unwrap();

        assert!(connects(&runtime, &folder, &folder.join("wayland-9")));
        // The session's own socket, by its own path, stays out of reach where connecting is ruled.
        if abi() >= 9 {
            assert!(!connects(&runtime, &folder, &runtime.join("wayland-9")));
        }
    }

    #[test]
    fn a_display_folder_made_again_is_out_of_reach_of_a_browser_confined_before() {
        // Landlock's rules hold the folder's inode, not its path: agent-browser's daemon, which
        // outlives its server, cannot reach the socket in a folder made again at the same path.
        use std::os::unix::process::CommandExt;
        if abi() < 9 {
            eprintln!(
                "skipped: this kernel's Landlock (ABI {}) does not rule connecting",
                abi()
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let folder = runtime.join("flottille/display/browser");
        std::fs::create_dir_all(&folder).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(runtime.join("wayland-9")).unwrap();
        std::fs::hard_link(runtime.join("wayland-9"), folder.join("wayland-9")).unwrap();
        let go = temp.path().join("go");

        let mut paths = Vec::new();
        allowed(
            Path::new("/"),
            &[runtime.canonicalize().unwrap()],
            &mut paths,
        );
        let open = CString::new(folder.as_os_str().as_bytes()).unwrap();
        let rights = rights(abi());
        let mut command = std::process::Command::new("python3");
        command.args([
            "-c",
            "import os, socket, sys, time\n\
             while not os.path.exists(sys.argv[2]): time.sleep(0.01)\n\
             s = socket.socket(socket.AF_UNIX)\ns.connect(sys.argv[1])",
        ]);
        command
            .arg(folder.join("wayland-9"))
            .arg(&go)
            .stderr(std::process::Stdio::null());
        // SAFETY: as in `confined_but`.
        unsafe { command.pre_exec(move || restrict(rights, &paths, Some(&open), 0, &[], None)) };
        let mut child = command.spawn().unwrap();
        // The folder made again, as `display::share` does at the next start.
        std::fs::remove_dir_all(&folder).unwrap();
        std::fs::create_dir(&folder).unwrap();
        std::fs::hard_link(runtime.join("wayland-9"), folder.join("wayland-9")).unwrap();
        std::fs::write(&go, "").unwrap();
        assert!(!child.wait().unwrap().success());
    }
}
