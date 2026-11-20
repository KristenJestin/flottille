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
const MOUNT_ATTR_RDONLY: u64 = 0x1;

/// `struct mount_attr` of `mount_setattr(2)`.
#[repr(C)]
struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}

/// The variable that plays a kernel refusing user namespaces, read by a test build only: the
/// tests' daemon is given it, never a packaged one.
const REFUSED: &str = "FLOTTILLE_TEST_NO_USER_NAMESPACES";

/// What a kernel that refuses user namespaces leaves open, as the daemon's log and
/// `flottille doctor` say it.
pub const REFUSED_GAP: &str = "the kernel gives no user namespace to the commands Flottille \
     confines: they run without their private mount namespace, and reach the sockets of the \
     user's runtime folder (the session bus) and X11's by their path where Landlock does not \
     deny connecting, and can write ~/.ssh (docs/vault.md, \"The gaps\")";

/// The session bus's socket, where the daemon's `DBUS_SESSION_BUS_ADDRESS` names one by its path
/// (`unix:path=…`, as a `dbus-launch` outside the runtime folders makes): read at the daemon's
/// start, for every confined command's namespace to hide. A bus on an abstract socket is no path:
/// Landlock's scope keeps it out (`landlock.rs`).
static BUS: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Finds the session bus's socket at the daemon's start, before anything runs.
pub fn find_bus() {
    BUS.get_or_init(|| {
        use std::os::unix::fs::FileTypeExt;
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok()?;
        let path = bus_path(&address)?.canonicalize().ok()?;
        let socket = std::fs::symlink_metadata(&path).ok()?;
        socket.file_type().is_socket().then_some(path)
    });
}

/// The session bus's socket found at the start.
pub fn bus() -> Option<&'static Path> {
    BUS.get()?.as_deref()
}

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
/// ABI, whether confined commands get their private mount namespace, whether the SSH agent is
/// lent (and why not), and the warnings that say each gap.
pub fn sandbox() -> serde_json::Value {
    let namespaces = namespaces();
    let unlent = unlent();
    let mut warnings: Vec<String> = Vec::new();
    match namespaces {
        Namespaces::Refused => warnings.push(REFUSED_GAP.into()),
        Namespaces::Unknown => warnings.push(UNKNOWN_GAP.into()),
        Namespaces::Available => {}
    }
    if namespaces != Namespaces::Refused && !recursive_read_only() {
        warnings.push(SHALLOW_GAP.into());
    }
    // A gap only where there is an agent to lend.
    if let Some(why) = unlent
        .as_ref()
        .filter(|_| crate::ssh_agent::socket().is_some())
    {
        warnings.push(format!(
            "the SSH agent is lent to no push and to no fetch of the daemon: {why}"
        ));
    }
    serde_json::json!({
        "landlock_abi": crate::landlock::abi(),
        "private_mounts": namespaces == Namespaces::Available,
        "ssh_agent": {"lent": unlent.is_none(), "why": unlent},
        "warnings": warnings,
    })
}

/// Why the SSH agent is lent to nothing now, if it is not: the kernel gives the commands
/// Flottille confines no namespace, where nothing `ssh` reads is read-only to them; or a walk of
/// the SSH configuration found a path no mount keeps, now or once in the daemon's life, since a
/// command may have written what `ssh` reads while it was open.
pub fn unlent() -> Option<String> {
    if namespaces() == Namespaces::Refused {
        return Some(
            "the kernel gives no user namespace to the commands Flottille confines, so nothing \
             ssh reads is read-only to them"
                .into(),
        );
    }
    let now = ssh().unlent.clone();
    let once = ONCE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match (now, &*once) {
        (Some(why), _) => Some(why),
        (None, Some(was)) => Some(format!(
            "{was}, as the daemon found earlier: a confined command may have written what ssh \
             reads since, so the agent is lent again only once the daemon restarts"
        )),
        (None, None) => None,
    }
}

/// What a kernel without `mount_setattr` (before 5.12) leaves open, as `flottille doctor` says it.
const SHALLOW_GAP: &str = "the kernel has no mount_setattr: a mount below a read-only SSH folder \
     stays writable to the commands Flottille confines";

/// Whether the kernel has `mount_setattr`, which makes a folder read-only with every mount below
/// it.
fn recursive_read_only() -> bool {
    // SAFETY: a call on no file: it fails at once, and says only whether the call exists.
    let result = unsafe {
        libc::syscall(
            libc::SYS_mount_setattr,
            -1,
            c"".as_ptr(),
            0,
            std::ptr::null::<MountAttr>(),
            0,
        )
    };
    result == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ENOSYS)
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
    pinned: Vec<CString>,
    read_only: Vec<(CString, libc::c_ulong)>,
    links: Vec<CString>,
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
    let mut plan = plan(&[PathBuf::from("/")], &[], &Ssh::default(), &[], None)?;
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

/// The namespace that empties the `emptied` folders, puts the `kept` paths in them back, does to
/// the SSH configuration what `ssh` says, hides each `masked` socket under `/dev/null`, and goes
/// back to `cwd` in it. None where a path cannot be a C string.
pub fn plan(
    emptied: &[PathBuf],
    kept: &[PathBuf],
    ssh: &Ssh,
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
    let mut kept_read_only = Vec::new();
    for path in &ssh.read_only {
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
        pinned: ssh.pinned.iter().map(|p| c(p)).collect::<Option<_>>()?,
        read_only: kept_read_only,
        links: ssh.links.iter().map(|p| c(p)).collect::<Option<_>>()?,
        // A socket gone since the start (its bus ended) is no path to hide: one mounted over
        // would fail every start.
        masked: masked
            .iter()
            .filter(|p| std::fs::symlink_metadata(p).is_ok())
            .map(|p| c(p))
            .collect::<Option<_>>()?,
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
            // A mount point can be neither renamed nor removed: the way to what is read-only
            // stays the way to it.
            for path in &self.pinned {
                check(libc::mount(
                    path.as_ptr(),
                    path.as_ptr(),
                    none,
                    libc::MS_BIND | libc::MS_REC,
                    std::ptr::null(),
                ) as libc::c_long)?;
            }
            for (path, flags) in &self.read_only {
                check(libc::mount(
                    path.as_ptr(),
                    path.as_ptr(),
                    none,
                    libc::MS_BIND | libc::MS_REC,
                    std::ptr::null(),
                ) as libc::c_long)?;
                // Read-only with every mount below it; a kernel without the call (before 5.12)
                // gets the folder's own mount alone, and the gap is said (`SHALLOW_GAP`).
                let attr = MountAttr {
                    attr_set: MOUNT_ATTR_RDONLY,
                    attr_clr: 0,
                    propagation: 0,
                    userns_fd: 0,
                };
                let set = libc::syscall(
                    libc::SYS_mount_setattr,
                    libc::AT_FDCWD,
                    path.as_ptr(),
                    AT_RECURSIVE,
                    &attr as *const MountAttr,
                    std::mem::size_of::<MountAttr>(),
                );
                if set != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ENOSYS) {
                    check(libc::mount(
                        none,
                        path.as_ptr(),
                        none,
                        libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY | flags,
                        std::ptr::null(),
                    ) as libc::c_long)?;
                } else {
                    check(set)?;
                }
            }
            for link in &self.links {
                let fd = libc::syscall(
                    libc::SYS_open_tree,
                    libc::AT_FDCWD,
                    link.as_ptr(),
                    OPEN_TREE_CLONE
                        | libc::O_CLOEXEC as libc::c_uint
                        | libc::AT_SYMLINK_NOFOLLOW as libc::c_uint,
                );
                check(fd)?;
                let moved = libc::syscall(
                    libc::SYS_move_mount,
                    fd as libc::c_int,
                    c"".as_ptr(),
                    libc::AT_FDCWD,
                    link.as_ptr(),
                    MOVE_MOUNT_F_EMPTY_PATH,
                );
                libc::close(fd as libc::c_int);
                check(moved)?;
            }
            for socket in &self.masked {
                check(libc::mount(
                    c"/dev/null".as_ptr(),
                    socket.as_ptr(),
                    none,
                    libc::MS_BIND,
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

/// What a confined command's namespace does to the user's SSH configuration, and whether the
/// SSH agent is lent at all. `ProxyCommand` and `Match exec` there run with the user's
/// credentials when a push signs in, so no confined command may change what `ssh` reads: the
/// files, nor the way to them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ssh {
    /// Read-only: the user's `~/.ssh` (of `HOME` and of the account, when they differ), where
    /// each link in it leads, the folder of each pattern its `config` includes (so that no file
    /// is added to a glob's folder nor made where one is missing), and where each file included
    /// leads, through the files they include in turn.
    pub read_only: Vec<PathBuf>,
    /// Bound onto themselves: every folder on the way to one of them that the user could rename
    /// or remove (a mount point can be neither), from below the home down.
    pub pinned: Vec<PathBuf>,
    /// The links on the way to one of them (`~/.ssh` itself when it is one), bound onto
    /// themselves for the same reason.
    pub links: Vec<PathBuf>,
    /// Why the SSH agent is lent to nothing: a path `ssh` reads that no mount can keep, which a
    /// run could make or change before the next push.
    pub unlent: Option<String>,
}

/// The largest SSH configuration file read: one larger is not read, and the agent is not lent.
const LARGEST: u64 = 1 << 20;

/// What the file at a path was when the SSH configuration was last read: its `lstat`.
type Stamp = (u64, u64, i64, i64, i64, i64, u64, u32);

fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path).ok()?;
    Some((
        m.dev(),
        m.ino(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.size(),
        m.mode(),
    ))
}

/// The SSH configuration as read last, with the homes it was read for and the stamp of every
/// path that was looked at.
struct Walked {
    homes: Vec<PathBuf>,
    seen: Vec<(PathBuf, Option<Stamp>)>,
    ssh: std::sync::Arc<Ssh>,
}

/// The first reason a walk of the SSH configuration found to lend the agent to nothing, kept for
/// the daemon's life.
static ONCE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// [`Ssh`] for the user's homes now. Read again only when a path it looked at changed: each
/// confined spawn costs an `lstat` of those, not a walk.
pub fn ssh() -> std::sync::Arc<Ssh> {
    static WALKED: std::sync::Mutex<Option<Walked>> = std::sync::Mutex::new(None);
    let homes = homes();
    let mut walked = WALKED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(last) = &*walked
        && last.homes == homes
        && last.seen.iter().all(|(path, was)| stamp(path) == *was)
    {
        return last.ssh.clone();
    }
    let mut walk = Walk {
        homes: &homes,
        ..Walk::default()
    };
    walk.find_work();
    walk.homes_ssh();
    let ssh = std::sync::Arc::new(walk.done());
    if let Some(why) = &ssh.unlent {
        ONCE.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_or_insert_with(|| why.clone());
    }
    let seen = std::mem::take(&mut walk.seen);
    *walked = Some(Walked {
        homes,
        seen,
        ssh: ssh.clone(),
    });
    ssh
}

/// The user's homes: `HOME`'s and the account's, each where its links lead, so that a home
/// behind a link is known as the folder it is. None in the crate's own tests, which never walk
/// the developer's home: they give theirs to [`Walk`].
fn homes() -> Vec<PathBuf> {
    if cfg!(test) {
        return Vec::new();
    }
    real_homes(
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .into_iter()
            .chain(account_home()),
    )
}

fn real_homes(homes: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut real: Vec<PathBuf> = Vec::new();
    for home in homes.filter(|home| home.is_absolute()) {
        let home = home.canonicalize().unwrap_or(home);
        if !real.contains(&home) {
            real.push(home);
        }
    }
    real
}

/// Makes each home's `~/.ssh` (0700) where it is missing, at the daemon's start: a missing one
/// is a folder a confined command could make, with a configuration in it, and the agent would be
/// lent to nothing for the daemon's life.
pub fn make_ssh_folders() {
    use std::os::unix::fs::DirBuilderExt;
    for home in homes() {
        let ssh = home.join(".ssh");
        if std::fs::symlink_metadata(&ssh).is_err()
            && let Err(error) = std::fs::DirBuilder::new().mode(0o700).create(&ssh)
        {
            eprintln!("flottille daemon: cannot make {}: {error}", ssh.display());
        }
    }
}

#[derive(Default)]
struct Walk<'a> {
    homes: &'a [PathBuf],
    /// The folders Flottille's commands work in: its own, each project's and where its
    /// workspaces go. None is made read-only.
    work: Vec<PathBuf>,
    seen: Vec<(PathBuf, Option<Stamp>)>,
    read_only: Vec<PathBuf>,
    links: Vec<PathBuf>,
    included: Vec<PathBuf>,
    unlent: Vec<String>,
}

impl Walk<'_> {
    /// A path looked at, stamped before it is read.
    fn see(&mut self, path: &Path) {
        if !self.seen.iter().any(|(seen, _)| seen == path) {
            self.seen.push((path.to_owned(), stamp(path)));
        }
    }

    /// Where `path` leads, each link on the way recorded, and whether it is there: a path whose
    /// last part alone is missing leads to that name in its folder. None when a folder on the
    /// way is missing, or the links go round.
    fn resolve(&mut self, path: &Path) -> Option<(PathBuf, bool)> {
        use std::path::Component;
        let mut done = PathBuf::from("/");
        let mut rest: Vec<std::ffi::OsString> = Vec::new();
        let push = |rest: &mut Vec<std::ffi::OsString>, path: &Path| {
            let parts: Vec<_> = path
                .components()
                .filter_map(|part| match part {
                    Component::Normal(name) => Some(name.to_owned()),
                    Component::ParentDir => Some("..".into()),
                    _ => None,
                })
                .collect();
            rest.extend(parts.into_iter().rev());
        };
        push(&mut rest, path);
        let mut hops = 0;
        while let Some(part) = rest.pop() {
            if part == ".." {
                done.pop();
                continue;
            }
            let next = done.join(&part);
            self.see(&next);
            match std::fs::symlink_metadata(&next) {
                Err(_) if rest.is_empty() => return Some((next, false)),
                Err(_) => return None,
                Ok(meta) if meta.file_type().is_symlink() => {
                    hops += 1;
                    let target = std::fs::read_link(&next).ok().filter(|_| hops <= 40)?;
                    self.links.push(next);
                    if target.is_absolute() {
                        done = PathBuf::from("/");
                    }
                    push(&mut rest, &target);
                }
                Ok(_) => done = next,
            }
        }
        Some((done, true))
    }

    /// The folders Flottille's commands work in, as the daemon's configuration says them now:
    /// the projects' files are looked at, so that a project added is a change of the walk.
    fn find_work(&mut self) {
        if cfg!(test) {
            return;
        }
        let Ok(paths) = flottille_core::paths::Paths::from_env() else {
            return;
        };
        let projects = paths.projects();
        self.see(&projects);
        if let Ok(entries) = std::fs::read_dir(&projects) {
            for entry in entries.flatten() {
                self.see(&entry.path());
            }
        }
        self.see(&paths.workspaces_place());
        let mut work = vec![
            paths.data.clone(),
            paths.state.clone(),
            paths.config.clone(),
        ];
        if let Ok(loaded) = flottille_core::registry::Registry::new(&paths).load_all() {
            for project in loaded.projects {
                work.push(project.folder);
                work.push(project.workspaces_root);
            }
        }
        self.work = work
            .into_iter()
            .map(|folder| folder.canonicalize().unwrap_or(folder))
            .collect();
    }

    /// `path` made read-only, unless it holds a home or the work of Flottille's commands, or is
    /// shared with others: the agent is then lent to nothing.
    fn read_only(&mut self, path: PathBuf, what: &str) {
        let shown = path.display();
        let why = if self.homes.iter().any(|home| home.starts_with(&path)) {
            format!("{what} is {shown}, which holds the home")
        } else if let Some(work) = self.work.iter().find(|work| work.starts_with(&path)) {
            format!(
                "{what} is {shown}, which holds {}, where Flottille's commands work",
                work.display()
            )
        } else if shared(&path) {
            format!("{what} is {shown}, which is shared: others write in it too")
        } else {
            self.read_only.push(path);
            return;
        };
        self.unlent.push(format!(
            "{why}: it cannot be made read-only to the commands Flottille confines"
        ));
    }

    /// What `written` leads to read-only; where it is missing, its folder, so that nothing is
    /// made there.
    fn protect(&mut self, written: &Path, what: &str) -> Option<PathBuf> {
        match self.resolve(written) {
            Some((path, true)) => {
                self.read_only(path.clone(), what);
                Some(path)
            }
            Some((path, false)) => {
                let folder = path.parent().unwrap_or(Path::new("/")).to_owned();
                self.read_only(folder, what);
                None
            }
            None => {
                self.unlent.push(format!(
                    "{what} ({}) is in a folder that does not exist, which a confined command \
                     could make",
                    written.display()
                ));
                None
            }
        }
    }

    fn homes_ssh(&mut self) {
        let mut folders: Vec<(PathBuf, PathBuf)> = Vec::new();
        for home in self.homes {
            let written = home.join(".ssh");
            let what = format!("{}", written.display());
            match self.resolve(&written) {
                Some((folder, true)) => {
                    self.read_only(folder.clone(), &what);
                    if !folders.iter().any(|(_, f)| *f == folder) {
                        folders.push((home.clone(), folder));
                    }
                }
                _ => self.unlent.push(format!(
                    "{what} does not exist: a confined command could make it, and the \
                     configuration in it"
                )),
            }
        }
        for (_, folder) in &folders {
            self.links_in(folder, 0);
        }
        for (home, folder) in &folders {
            let config = folder.join("config");
            self.config(&config, home, folder, 0);
        }
    }

    /// Where each link under `folder` leads, followed through the folders in it.
    fn links_in(&mut self, folder: &Path, depth: usize) {
        self.see(folder);
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_symlink() {
                let what = format!("the link {}", path.display());
                self.protect(&path, &what);
            } else if kind.is_dir() && depth < 8 {
                self.links_in(&path, depth + 1);
            }
        }
    }

    /// The files an SSH configuration includes, and those they include, each once: a relative
    /// path is the user's `~/.ssh`'s, as `ssh` reads the user's own configuration.
    fn config(&mut self, config: &Path, home: &Path, folder: &Path, depth: usize) {
        // Its stamp: a file written in place changes no folder's.
        self.see(config);
        let text = match read_config(config) {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(why) => {
                self.unlent.push(why);
                return;
            }
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
                let what = format!("the folder of `Include {word}` in {}", config.display());
                // The folder no file of the pattern can be added to: the last one above its
                // first part that is a pattern.
                let fixed: PathBuf = pattern
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .components()
                    .take_while(|part| {
                        !part
                            .as_os_str()
                            .as_bytes()
                            .iter()
                            .any(|b| matches!(b, b'*' | b'?' | b'['))
                    })
                    .collect();
                match self.resolve(&fixed) {
                    // The links in it too: a pattern's part may go through one.
                    Some((path, true)) => {
                        self.links_in(&path, 0);
                        self.read_only(path, &what);
                    }
                    _ => self.unlent.push(format!(
                        "{what} ({}) does not exist: a confined command could make it, and the \
                         file in it",
                        fixed.display()
                    )),
                }
                for file in glob(&pattern) {
                    let what = format!("{}, included by {}", file.display(), config.display());
                    let Some(file) = self.protect(&file, &what) else {
                        continue;
                    };
                    if self.included.contains(&file) {
                        continue;
                    }
                    self.included.push(file.clone());
                    if depth < 16 {
                        self.config(&file, home, folder, depth + 1);
                    }
                }
            }
        }
    }

    /// The lists, each path once, none inside another read-only one, and the folders on the way
    /// to them that the user could rename.
    fn done(&mut self) -> Ssh {
        let mut sorted = std::mem::take(&mut self.read_only);
        sorted.sort();
        sorted.dedup();
        let mut read_only: Vec<PathBuf> = Vec::new();
        for path in sorted {
            if !read_only.iter().any(|done| path.starts_with(done)) {
                read_only.push(path);
            }
        }
        let inside = |path: &Path| read_only.iter().any(|done| path.starts_with(done));
        let mut links: Vec<PathBuf> = std::mem::take(&mut self.links)
            .into_iter()
            .filter(|link| !inside(link) && changeable(link.parent()))
            .collect();
        links.sort();
        links.dedup();
        let mut pinned: Vec<PathBuf> = read_only
            .iter()
            .chain(&links)
            .flat_map(|path| path.ancestors().skip(1))
            .filter(|folder| {
                folder.parent().is_some()
                    && !self.homes.iter().any(|home| home.starts_with(folder))
                    && !inside(folder)
                    && changeable(folder.parent())
            })
            .map(Path::to_owned)
            .collect();
        pinned.sort();
        pinned.dedup();
        Ssh {
            read_only,
            pinned,
            links,
            unlent: (!self.unlent.is_empty()).then(|| self.unlent.join("; ")),
        }
    }
}

/// Whether the user may change the entries of `folder`: rename or remove what is in it, now or
/// once they gave themselves the right (a folder of theirs they cannot write yet).
fn changeable(folder: Option<&Path>) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(folder) = folder else {
        return false;
    };
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    std::fs::metadata(folder).is_ok_and(|meta| meta.uid() == uid) || writable(folder)
}

/// Whether the user may write `path` now.
fn writable(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a valid C string.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Whether others write in `path` too: it is everyone's (world-writable, or sticky as `/tmp`),
/// or another's that the user may write. A read-only mount of it would take it from them, in
/// every command Flottille confines. Another's that the user cannot write is no one's to change
/// in a command of theirs: read-only, it changes nothing.
fn shared(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    meta.mode() & 0o1002 != 0 || (meta.uid() != uid && writable(path))
}

/// An SSH configuration file's text: None when it is not there or cannot be read. Opened without
/// waiting (a FIFO answers at once) and read only when it is a regular file of at most
/// [`LARGEST`] bytes; anything else is said, as `ssh` would hang on it or read what a writer
/// feeds it.
fn read_config(path: &Path) -> Result<Option<String>, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
    {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let Ok(meta) = file.metadata() else {
        return Ok(None);
    };
    if !meta.is_file() {
        return Err(format!(
            "{} is not a regular file: ssh would read from it what a confined command writes",
            path.display()
        ));
    }
    let larger = || {
        format!(
            "{} is larger than {LARGEST} bytes, and is not read",
            path.display()
        )
    };
    if meta.len() > LARGEST {
        return Err(larger());
    }
    let mut bytes = Vec::new();
    match file.take(LARGEST + 1).read_to_end(&mut bytes) {
        Ok(_) if bytes.len() as u64 > LARGEST => Err(larger()),
        Ok(_) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(_) => Ok(None),
    }
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
            &Ssh::default(),
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
            &Ssh::default(),
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

    /// [`Ssh`] of the one home `home`.
    fn walked(home: &Path) -> Ssh {
        let homes = real_homes(std::iter::once(home.to_owned()));
        let mut walk = Walk {
            homes: &homes,
            ..Walk::default()
        };
        walk.homes_ssh();
        walk.done()
    }

    #[test]
    fn the_ssh_includes_out_of_the_folder_are_found_through_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap();
        let home = top.join("home");
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::create_dir_all(top.join("dots/conf.d")).unwrap();
        std::fs::create_dir_all(top.join("more")).unwrap();
        std::fs::write(
            ssh.join("config"),
            format!(
                "Host a\n  User me\nInclude {}/dots/conf.d/*.conf local\n",
                top.display()
            ),
        )
        .unwrap();
        std::fs::write(ssh.join("local"), "include ~/../more/last\n").unwrap();
        std::fs::write(top.join("more/last"), "").unwrap();
        std::fs::write(top.join("dots/conf.d/one.conf"), "").unwrap();

        let walked = walked(&home);

        assert_eq!(
            walked.read_only,
            [top.join("dots/conf.d"), ssh.clone(), top.join("more")]
        );
        assert_eq!(walked.unlent, None);
    }

    #[test]
    fn the_folders_and_the_links_on_the_way_are_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap();
        let home = top.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(top.join("dots/deep/ssh")).unwrap();
        std::os::unix::fs::symlink(top.join("dots/deep/ssh"), home.join(".ssh")).unwrap();

        let walked = walked(&home);

        assert_eq!(walked.read_only, [top.join("dots/deep/ssh")]);
        assert_eq!(walked.links, [home.join(".ssh")]);
        // Not the home, nor the folders above it: the home's own way is the system's.
        assert_eq!(walked.pinned, [top.join("dots"), top.join("dots/deep")]);
    }

    #[test]
    fn the_agent_is_lent_to_nothing_where_ssh_or_an_include_s_folder_cannot_be_kept() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        std::fs::create_dir_all(&home).unwrap();
        assert!(
            walked(&home)
                .unlent
                .unwrap()
                .contains(".ssh does not exist")
        );

        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/config"), "Include ~/forge.conf\n").unwrap();
        assert!(walked(&home).unlent.unwrap().contains("holds the home"));

        std::fs::write(home.join(".ssh/config"), "Include /nowhere/at/all/*\n").unwrap();
        assert!(walked(&home).unlent.unwrap().contains("/nowhere/at/all"));
    }

    #[test]
    fn a_home_behind_a_link_is_the_home_it_leads_to() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap();
        let home = top.join("real/home");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/config"), "Include ~/forge.conf\n").unwrap();
        std::os::unix::fs::symlink(&home, top.join("home")).unwrap();

        let walked = walked(&top.join("home"));

        assert!(!walked.read_only.contains(&home), "{walked:?}");
        assert!(
            walked
                .unlent
                .as_deref()
                .unwrap_or("")
                .contains("holds the home"),
            "{walked:?}"
        );
    }

    #[test]
    fn a_link_in_the_wildcard_part_of_an_include_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap();
        let home = top.join("home");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(top.join("dots")).unwrap();
        std::fs::create_dir_all(top.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(top.join("elsewhere"), top.join("dots/one")).unwrap();
        std::fs::write(
            home.join(".ssh/config"),
            format!("Include {}/dots/*/forge.conf\n", top.display()),
        )
        .unwrap();

        let walked = walked(&home);

        assert!(
            walked.read_only.contains(&top.join("elsewhere")),
            "{walked:?}"
        );
        assert_eq!(walked.unlent, None);
    }

    #[test]
    fn a_folder_the_user_owns_but_cannot_write_now_is_pinned() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap();
        let home = top.join("home");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(top.join("dots/a/conf.d")).unwrap();
        std::fs::write(
            home.join(".ssh/config"),
            format!("Include {}/dots/a/conf.d/*.conf\n", top.display()),
        )
        .unwrap();
        let mode = |m| std::fs::Permissions::from_mode(m);
        std::fs::set_permissions(top.join("dots"), mode(0o555)).unwrap();

        let walked = walked(&home);
        std::fs::set_permissions(top.join("dots"), mode(0o755)).unwrap();

        // A command may `chmod u+w` the folder, and rename what is in it.
        assert!(walked.pinned.contains(&top.join("dots/a")), "{walked:?}");
    }

    #[test]
    fn read_only_reaches_the_mounts_below_a_protected_folder() {
        use std::os::unix::process::CommandExt;
        if !available() {
            eprintln!("skipped: this kernel refuses user namespaces to this user");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let ssh = temp.path().canonicalize().unwrap().join("ssh");
        std::fs::create_dir_all(ssh.join("below")).unwrap();
        let ssh_c = CString::new(ssh.join("below").as_os_str().as_bytes()).unwrap();
        let protected = Ssh {
            read_only: vec![ssh.clone()],
            ..Ssh::default()
        };
        let mut plan = plan(&[], &[], &protected, &[], None).unwrap();
        let mut outer = super::plan(&[], &[], &Ssh::default(), &[], None).unwrap();
        let mut command = std::process::Command::new("python3");
        command
            .args([
                "-c",
                "import sys\n\
                 try:\n\
                 \x20   open(sys.argv[1] + '/x', 'w')\n\
                 \x20   print('done')\n\
                 except OSError as e:\n\
                 \x20   print(e.errno)",
            ])
            .arg(ssh.join("below"));
        // SAFETY: as in `landlock::confine`; the outer namespace mounts a tmpfs below the folder,
        // as a user's own mount there would be, before the command's namespace is made in it.
        unsafe {
            command.pre_exec(move || {
                outer.enter()?;
                let tmpfs = c"tmpfs";
                if libc::mount(
                    tmpfs.as_ptr(),
                    ssh_c.as_ptr(),
                    tmpfs.as_ptr(),
                    0,
                    std::ptr::null(),
                ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                plan.enter()
            })
        };
        let output = command.output().unwrap();

        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            libc::EROFS.to_string(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn a_fifo_or_a_huge_file_is_not_read_as_a_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("pipe");
        let c = CString::new(pipe.as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid C string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let huge = dir.path().join("huge");
        std::fs::File::create(&huge)
            .unwrap()
            .set_len(LARGEST + 1)
            .unwrap();

        assert!(
            read_config(&pipe)
                .unwrap_err()
                .contains("not a regular file")
        );
        assert!(read_config(&huge).unwrap_err().contains("larger"));
        assert_eq!(read_config(&dir.path().join("none")), Ok(None));
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
        let mut plan = plan(&[temp.path().to_owned()], &[], &Ssh::default(), &[], None).unwrap();
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
