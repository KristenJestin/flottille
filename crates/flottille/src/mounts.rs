//! The private mount namespace a confined command starts in (`docs/vault.md`, "The runtime
//! folder"): a user namespace of its own, where the user's runtime folders and X11's socket
//! folder are empty, so that no socket in them is reached by its path, whatever Landlock's ABI.
//! Only what the daemon itself gives the command is put back in the emptied folders: its display
//! folder, the SSH agent's socket it is lent. Landlock's rules (`landlock.rs`) apply after it,
//! unchanged: this is a layer more.
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
     deny connecting (docs/vault.md, \"The gaps\")";

/// The session bus's socket, where the daemon's `DBUS_SESSION_BUS_ADDRESS` names one by its path
/// (`unix:path=…`, as a `dbus-launch` outside the runtime folders makes): read at the daemon's
/// start, with what tells that socket from another made at its path later, for every confined
/// command's namespace to hide. A bus on an abstract socket is no path: Landlock's scope keeps it
/// out (`landlock.rs`).
static BUS: OnceLock<Option<(PathBuf, Socket)>> = OnceLock::new();

/// A socket's device, inode and change time: an inode freed is often given again to the next
/// file made, and the change time of a new one is its making, which no process sets.
type Socket = (u64, u64, i64, i64);

/// Finds the session bus's socket at the daemon's start, before anything runs.
pub fn find_bus() {
    BUS.get_or_init(|| {
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok()?;
        let path = bus_path(&address)?.canonicalize().ok()?;
        let socket = socket_at(&path)?;
        Some((path, socket))
    });
}

/// The socket at `path`, if a socket is there.
fn socket_at(path: &Path) -> Option<Socket> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let found = std::fs::symlink_metadata(path).ok()?;
    found
        .file_type()
        .is_socket()
        .then(|| (found.dev(), found.ino(), found.ctime(), found.ctime_nsec()))
}

/// The path of the session bus's socket found at the start, which every confined command's
/// namespace hides whatever is there now: the bus, a bus started there again, or what a command
/// made in its place.
pub fn bus() -> Option<&'static Path> {
    BUS.get()?.as_ref().map(|(path, _)| path.as_path())
}

/// Whether the session bus the daemon found at its start is gone from its path, or something
/// else is there now, which a confined command may have made: no push it lets through is given
/// the bus's address then.
pub fn bus_replaced() -> bool {
    BUS.get()
        .and_then(Option::as_ref)
        .is_some_and(|(path, socket)| socket_at(path) != Some(*socket))
}

/// What a bus gone or replaced leaves, as `flottille doctor` says it.
const BUS_REPLACED_GAP: &str = "the session bus the daemon found at its start is gone from its \
     path, or something else is there now: confined commands still have that path hidden, and \
     a push let through is not given the bus's address (docs/vault.md, \"The gaps\")";

/// The path of the first `unix:path=` of a D-Bus address, its `%xx` escapes undone.
fn bus_path(address: &str) -> Option<PathBuf> {
    let value = address.split(';').find_map(|one| {
        one.strip_prefix("unix:")?
            .split(',')
            .find_map(|pair| pair.strip_prefix("path="))
    })?;
    let mut bytes = Vec::new();
    let mut rest = value.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        if byte == b'%' && after.len() >= 2 {
            let hex = std::str::from_utf8(&after[..2]).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = &after[2..];
        } else {
            bytes.push(byte);
            rest = after;
        }
    }
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
}

/// The sandbox's state as the daemon has it, for `daemon.status` and `flottille doctor`: Landlock's
/// ABI, whether confined commands get their private mount namespace, and the warnings that say
/// each gap.
pub fn sandbox() -> serde_json::Value {
    let namespaces = namespaces();
    let mut warnings: Vec<&str> = match namespaces {
        Namespaces::Refused => vec![REFUSED_GAP],
        Namespaces::Unknown => vec![UNKNOWN_GAP],
        Namespaces::Available => Vec::new(),
    };
    warnings.extend(bus_replaced().then_some(BUS_REPLACED_GAP));
    serde_json::json!({
        "landlock_abi": crate::landlock::abi(),
        "private_mounts": namespaces == Namespaces::Available,
        "warnings": warnings,
    })
}

/// What a probe that concluded nothing leaves, as `flottille doctor` says it.
const UNKNOWN_GAP: &str = "whether the kernel gives the commands Flottille confines a user \
     namespace could not be probed yet: each starts in its private mount namespace, or fails to \
     start";

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
    masked: Vec<CString>,
    emptied: Vec<CString>,
    kept: Vec<Kept>,
    trees: Vec<libc::c_int>,
    cwd: Option<CString>,
}

/// Whether the kernel gives a confined command a private mount namespace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Namespaces {
    Available,
    /// Refused for good (a kernel built without user namespaces, a distribution that forbids
    /// them to unprivileged users): the command runs without it, and the gap is said
    /// (`flottille doctor`, the daemon's log).
    Refused,
    /// The probe concluded nothing (no process could be made, the user's namespaces are all
    /// taken): probed again next time, and the command still starts in one, or fails to.
    Unknown,
}

/// Probed by a child that makes one, until a probe concludes; what it concluded is kept for the
/// daemon's life.
pub fn namespaces() -> Namespaces {
    static CONCLUDED: OnceLock<bool> = OnceLock::new();
    if cfg!(debug_assertions) && std::env::var_os(REFUSED).is_some() {
        return Namespaces::Refused;
    }
    let concluded = match CONCLUDED.get() {
        Some(available) => Some(*available),
        None => probe().map(|available| *CONCLUDED.get_or_init(|| available)),
    };
    match concluded {
        Some(true) => Namespaces::Available,
        Some(false) => Namespaces::Refused,
        None => Namespaces::Unknown,
    }
}

/// Whether a confined command starts in its private mount namespace: unless the kernel was found
/// to refuse it.
pub fn available() -> bool {
    namespaces() != Namespaces::Refused
}

/// Whether the error a namespace's making failed with is the kernel refusing it, not a passing
/// lack.
fn refusal(errno: Option<i32>) -> bool {
    matches!(
        errno,
        Some(libc::EPERM | libc::EACCES | libc::EINVAL | libc::ENOSYS)
    )
}

/// What a probe's child said: made (0), refused (1), or a failure that concludes nothing; and
/// nothing when it could not be waited for.
fn concluded(waited: libc::pid_t, status: libc::c_int) -> Option<bool> {
    if waited < 0 || !libc::WIFEXITED(status) {
        return None;
    }
    match libc::WEXITSTATUS(status) {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

/// A child that makes the namespace and empties `/` in it, then ends.
fn probe() -> Option<bool> {
    let mut plan = plan(&[PathBuf::from("/")], &[], &[], None)?;
    // SAFETY: the child makes system calls alone, on what was allocated before the fork, then
    // ends without running anything of the parent's.
    unsafe {
        match libc::fork() {
            -1 => None,
            0 => libc::_exit(match plan.enter() {
                Ok(()) => 0,
                Err(error) if refusal(error.raw_os_error()) => 1,
                Err(_) => 2,
            }),
            child => {
                let mut status = 0;
                let waited = loop {
                    let waited = libc::waitpid(child, &mut status, 0);
                    if waited >= 0
                        || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
                    {
                        break waited;
                    }
                };
                concluded(waited, status)
            }
        }
    }
}

/// The namespace that empties the `emptied` folders, puts the `kept` paths in them back, hides
/// each `masked` socket under `/dev/null`, and goes back to `cwd` in it. None where a path cannot
/// be a C string.
pub fn plan(
    emptied: &[PathBuf],
    kept: &[PathBuf],
    masked: &[PathBuf],
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
    // SAFETY: getuid and getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    Some(Plan {
        uid_map: format!("{uid} {uid} 1").into_bytes(),
        gid_map: format!("{gid} {gid} 1").into_bytes(),
        masked: masked.iter().map(|p| c(p)).collect::<Option<_>>()?,
        emptied: folders.into_iter().map(|f| c(f)).collect::<Option<_>>()?,
        trees: vec![-1; put_back.len()],
        kept: put_back,
        cwd: cwd.filter(|cwd| cwd.is_absolute()).and_then(c),
    })
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
            // A program with file capabilities gains none in the namespace, where no Landlock
            // domain may stop it unmounting what hides the runtime folder.
            check(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) as libc::c_long)?;
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
            // Whatever is at a masked path now: nothing there (or a loop of links) is nothing to
            // reach, and a folder a command made there is hidden under an empty one.
            for socket in &self.masked {
                let masked = libc::mount(
                    c"/dev/null".as_ptr(),
                    socket.as_ptr(),
                    none,
                    libc::MS_BIND,
                    std::ptr::null(),
                );
                if masked != 0 {
                    let error = io::Error::last_os_error();
                    match error.raw_os_error() {
                        Some(libc::ENOENT | libc::ELOOP) => {}
                        Some(libc::ENOTDIR) => check(libc::mount(
                            c"tmpfs".as_ptr(),
                            socket.as_ptr(),
                            c"tmpfs".as_ptr(),
                            libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
                            c"mode=0".as_ptr().cast(),
                        ) as libc::c_long)?,
                        _ => return Err(error),
                    }
                }
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
    fn a_probe_concludes_only_on_a_child_it_waited_for() {
        let exited = |code: libc::c_int| code << 8;
        assert_eq!(concluded(42, exited(0)), Some(true));
        assert_eq!(concluded(42, exited(1)), Some(false));
        assert_eq!(concluded(42, exited(2)), None);
        // waitpid failed (ECHILD): the status it left is no answer.
        assert_eq!(concluded(-1, 0), None);
        assert!(refusal(Some(libc::EPERM)));
        assert!(!refusal(Some(libc::ENOSPC)));
    }

    #[test]
    fn the_bus_s_path_is_read_from_its_address() {
        assert_eq!(
            bus_path("unix:abstract=/tmp/x;unix:path=/tmp/dbus%2dab,guid=12"),
            Some(PathBuf::from("/tmp/dbus-ab"))
        );
        assert_eq!(bus_path("unix:abstract=/tmp/dbus-ab,guid=12"), None);
    }

    #[test]
    fn a_command_in_the_namespace_gains_no_privilege() {
        use std::os::unix::process::CommandExt;
        if !available() {
            eprintln!("skipped: this kernel refuses user namespaces to this user");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut plan = plan(&[temp.path().to_owned()], &[], &[], None).unwrap();
        let mut command = std::process::Command::new("grep");
        command.args(["NoNewPrivs", "/proc/self/status"]);
        // SAFETY: as in `landlock::confine`.
        unsafe { command.pre_exec(move || plan.enter()) };
        let output = command.output().unwrap();

        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1"),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
