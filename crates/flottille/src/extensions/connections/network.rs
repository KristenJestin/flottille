//! The network of a connection's browser (`docs/vault.md`, "The browser's network"): agent-browser's
//! daemon and its Chrome run in a network namespace of their own, so that their loopback is not
//! the runs'. Chrome's DevTools listens there on a port with no token, and agent-browser's daemon
//! on another: a run, in the user's network, reaches neither. passt's `pasta` gives the namespace
//! its way out, and the host's loopback ports (a developer's server), never a way in.
//!
//! The namespace is held by a process of its own, `flottille browser-network`, started in a user
//! and a network namespace it makes (the user mapped to themself, as `mounts.rs` does): it
//! outlives a server, as the browser does, and is found again by the next server, a login and
//! `close` through the record in the connection's vault folder. Each command of agent-browser
//! enters it before anything else of its sandbox.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// The hidden command that holds the namespace.
pub const HOLD: &str = "browser-network";
/// What the user installs where the isolation is missing.
pub const INSTALL: &str = "install passt (pasta) to let the agent's browser sign in";
/// Why the isolation is missing where pasta is.
const NO_PASTA: &str = "pasta is not on the PATH";
/// The program that gives the namespace its way out.
const PASTA: &str = "pasta";
/// In the vault folder: the holder's pid, its namespaces' inodes and its start time, and pasta's
/// pid.
const RECORD: &str = "network";
const PASTA_PID: &str = "network.pasta";
/// How long pasta has to set the namespace up.
const PASTA_LIMIT: Duration = Duration::from_secs(10);

/// The namespace of a browser, entered by the commands that start or reach it.
pub struct Network {
    user: OwnedFd,
    net: OwnedFd,
}

impl Network {
    /// `command` started in the namespace: called before the sandbox's own `pre_exec`
    /// (`landlock::confined_given`), whose user namespace is then made inside this one.
    pub fn entered<'a>(
        &self,
        command: &'a mut tokio::process::Command,
    ) -> &'a mut tokio::process::Command {
        let (user, net) = (self.user.as_raw_fd(), self.net.as_raw_fd());
        // SAFETY: setns alone, on descriptors this value holds open until the spawn returns.
        unsafe {
            command.pre_exec(move || {
                check(libc::setns(user, libc::CLONE_NEWUSER))?;
                check(libc::setns(net, libc::CLONE_NEWNET))
            })
        }
    }

    /// Whether the process `pid` runs in this namespace's network.
    pub fn holds(&self, pid: u32) -> bool {
        let ours = inode(&self.net);
        std::fs::metadata(format!("/proc/{pid}/ns/net")).is_ok_and(|ns| Some(ns.ino()) == ours)
    }
}

fn check(result: libc::c_int) -> io::Result<()> {
    match result {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

fn inode(fd: &OwnedFd) -> Option<u64> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat writes the structure it is given on success.
    (unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } == 0)
        // SAFETY: initialised by the fstat that succeeded.
        .then(|| unsafe { stat.assume_init() }.st_ino)
}

/// The program `pasta` names: `pasta` on the daemon's `PATH`, or in a test build what
/// `FLOTTILLE_TEST_PASTA` names (a path, or a name looked for on the `PATH`).
fn pasta() -> Option<PathBuf> {
    let name = std::env::var("FLOTTILLE_TEST_PASTA")
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .unwrap_or_else(|| PASTA.to_owned());
    if name.contains('/') {
        return Path::new(&name).is_file().then(|| PathBuf::from(name));
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|folder| folder.join(&name))
        .find(|program| program.is_file())
}

/// Whether a browser can be given a network of its own here, and why not: what `flottille
/// doctor` says.
pub fn available() -> Result<(), String> {
    pasta().ok_or_else(|| format!("{NO_PASTA}: {INSTALL}"))?;
    if crate::mounts::namespaces() == crate::mounts::Namespaces::Refused {
        return Err(
            "the kernel refuses user namespaces to this user: allowing them (on Ubuntu from \
             23.10, kernel.apparmor_restrict_unprivileged_userns=0, or an AppArmor profile that \
             gives Flottille userns) lets the agent's browser sign in"
                .into(),
        );
    }
    out_of_reach(crate::landlock::abi())
}

/// Whether the browser is out of a run's reach as a process: Landlock confines runs from its
/// second ABI, and a confined process neither attaches to one outside its domain (`ptrace`, which
/// reads the password filled in its memory) nor opens its `/proc/<pid>/root`, which leads to the
/// vault folder and the browser's socket. Yama's `ptrace_scope` keeps the first alone, so it is
/// no substitute. The network keeps DevTools out of reach; this keeps the process.
fn out_of_reach(landlock: i64) -> Result<(), String> {
    if landlock >= 2 {
        return Ok(());
    }
    Err(format!(
        "the kernel's Landlock is below its second ABI ({landlock}), so a run could open \
         /proc/<pid>/root of the user's processes, reach the browser's socket and read the \
         password filled: a kernel with Landlock's second ABI or later (Linux 5.19), Landlock \
         enabled (lsm=landlock), lets the agent's browser sign in"
    ))
}

/// The network recorded in the vault folder `vault`, once this host is known to keep a run out
/// of the browser's reach (`out_of_reach`): checked each time one is used, not once when it was
/// made.
pub fn present(vault: &Path, landlock: i64) -> Result<Option<Network>, String> {
    out_of_reach(landlock)?;
    Ok(found(vault))
}

/// The namespace recorded in the vault folder `vault`, while its holder runs.
pub fn found(vault: &Path) -> Option<Network> {
    let text = std::fs::read_to_string(vault.join(RECORD)).ok()?;
    let mut words = text.split_whitespace();
    let (pid, user, net, started): (u32, u64, u64, u64) = (
        words.next()?.parse().ok()?,
        words.next()?.parse().ok()?,
        words.next()?.parse().ok()?,
        words.next()?.parse().ok()?,
    );
    let network = opened(pid).ok()?;
    // The same namespaces and the same process, not one that took the pid since (and may have
    // entered them): its start time, read once the namespaces are open, is the holder's.
    (inode(&network.user) == Some(user)
        && inode(&network.net) == Some(net)
        && crate::agents::start_time(pid) == Some(started))
    .then_some(network)
}

fn opened(pid: u32) -> io::Result<Network> {
    let open =
        |kind: &str| std::fs::File::open(format!("/proc/{pid}/ns/{kind}")).map(OwnedFd::from);
    Ok(Network {
        user: open("user")?,
        net: open("net")?,
    })
}

/// A new namespace for the vault folder `vault`, its holder started, pasta's way out set up and
/// both recorded. Whatever was recorded before is ended first.
pub async fn made(vault: &Path) -> Result<Network, String> {
    let pasta = pasta().ok_or_else(|| format!("{NO_PASTA}: {INSTALL}"))?;
    ended(vault);
    let program = crate::program::own(crate::program::Started::Here)
        .map_err(|error| format!("cannot find Flottille itself: {error}"))?;
    // SAFETY: getuid and getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let uid_map = format!("{uid} {uid} 1").into_bytes();
    let gid_map = format!("{gid} {gid} 1").into_bytes();
    let mut command = tokio::process::Command::new(program);
    command
        .arg(HOLD)
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: system calls alone, on buffers made before the fork.
    unsafe {
        command.pre_exec(move || {
            check(libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET))?;
            write_file(c"/proc/self/setgroups", b"deny")?;
            write_file(c"/proc/self/uid_map", &uid_map)?;
            write_file(c"/proc/self/gid_map", &gid_map)
        })
    };
    // Its own session, ended with the daemon's (`daemon stop`), which closes the browsers too.
    let holder = crate::children::spawn(crate::children::in_own_session(&mut command))
        .map_err(|error| format!("a network namespace cannot be made: {error}"))?;
    let pid = holder.id().ok_or("the namespace's holder ended at once")?;
    // Not waited for here: tokio waits for it once it ends.
    drop(holder);
    let network = match opened(pid) {
        Ok(network) => network,
        Err(error) => {
            kill(pid);
            return Err(format!("the namespace's holder ended at once: {error}"));
        }
    };
    let Some(started) = crate::agents::start_time(pid) else {
        kill(pid);
        return Err("the namespace's holder ended at once".into());
    };
    let record = format!(
        "{pid} {} {} {started}\n",
        inode(&network.user).unwrap_or_default(),
        inode(&network.net).unwrap_or_default()
    );
    if let Err(error) = std::fs::write(vault.join(RECORD), record) {
        kill(pid);
        return Err(format!("{}: {error}", vault.join(RECORD).display()));
    }
    // No port of the host's forwarded in (`-t none -u none`): DevTools stays inside. The host's
    // loopback ports are forwarded out (`-T auto -U auto`), so the browser reaches a developer's
    // server and the host's resolver.
    let mut way_out = tokio::process::Command::new(&pasta);
    way_out
        .args(["--config-net", "--quiet"])
        .args(["-t", "none", "-u", "none", "-T", "auto", "-U", "auto"])
        .arg("--pid")
        .arg(vault.join(PASTA_PID))
        .arg(pid.to_string())
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let set_up = async {
        // It forks into the background once the namespace is set up: its first process ends.
        let child = crate::children::spawn(&mut way_out).map_err(|error| error.to_string())?;
        let output = child
            .wait_with_output()
            .await
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&output.stderr);
        Err(said
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("it failed")
            .to_owned())
    };
    let failed = match tokio::time::timeout(PASTA_LIMIT, set_up).await {
        Ok(Ok(())) => None,
        Ok(Err(why)) => Some(why),
        Err(_) => Some(format!(
            "it did not end within {} seconds",
            PASTA_LIMIT.as_secs()
        )),
    };
    if let Some(why) = failed {
        ended(vault);
        return Err(format!(
            "{} could not set the network up: {why}",
            pasta.display()
        ));
    }
    Ok(network)
}

/// The namespace recorded in the vault folder `vault` ended: its holder, and pasta, which
/// serves it, stopped; the record removed. Nothing a record names that is not theirs.
pub fn ended(vault: &Path) {
    let record = vault.join(RECORD);
    let holder = std::fs::read_to_string(&record)
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse::<u32>().ok());
    if let Some(pid) = holder.filter(|_| found(vault).is_some()) {
        // pasta serves the holder's pid, the last word of its command line.
        let served = std::fs::read_to_string(vault.join(PASTA_PID))
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|pasta| {
                std::fs::read(format!("/proc/{pasta}/cmdline")).is_ok_and(|line| {
                    line.split(|byte| *byte == 0)
                        .rfind(|word| !word.is_empty())
                        .is_some_and(|last| last == pid.to_string().as_bytes())
                })
            });
        kill(pid);
        if let Some(pasta) = served {
            kill(pasta);
        }
    }
    let _ = std::fs::remove_file(record);
    let _ = std::fs::remove_file(vault.join(PASTA_PID));
}

fn kill(pid: u32) {
    // SAFETY: kill has no memory precondition.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
}

/// `bytes` written to the file at `path`, as one write.
fn write_file(path: &std::ffi::CStr, bytes: &[u8]) -> io::Result<()> {
    // SAFETY: a valid C string, and a buffer alive for the call.
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let written = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
        let error = io::Error::last_os_error();
        libc::close(fd);
        if written != bytes.len() as isize {
            return Err(error);
        }
    }
    Ok(())
}

/// `flottille browser-network`: holds the namespace it was started in until it is killed.
pub fn hold() -> ! {
    loop {
        std::thread::park();
    }
}

/// The pids of the processes that answer on the sockets of agent-browser's daemons in the socket
/// folder `sockets` and the folders in it (`<session>.sock`; a namespace puts a session's in a
/// subfolder), as the kernel says (`SO_PEERCRED`), never as a file says: 0 for one whose process
/// is gone or unknown, which no network holds, and for whatever cannot be told, which fails
/// closed: a socket that cannot be connected to whatever the error (it stops accepting, its path
/// is too long for a socket address, its mode refuses us, nobody listens on it any more), a link
/// named as a socket (agent-browser follows it), a folder or an entry that cannot be read.
pub fn daemons(sockets: &Path) -> Vec<u32> {
    connected(sockets)
        .into_iter()
        .map(|(_, result)| match result {
            Ok(stream) => peer(&stream)
                .filter(|pid| Path::new(&format!("/proc/{pid}")).exists())
                .unwrap_or(0),
            Err(_) => 0,
        })
        .collect()
}

/// Whether a daemon may still be alive on a socket of the socket folder `sockets`: one that
/// accepts, or that cannot be told (it does not accept at once, its path is too long, its mode
/// refuses us). Only a socket nobody listens on is a browser that ended, which keeps nothing.
pub fn alive(sockets: &Path) -> bool {
    connected(sockets)
        .iter()
        .any(|(_, result)| result.as_ref().map_or_else(|error| !dead(error), |_| true))
}

/// The sockets of the socket folder `sockets` (and the folders in it) that nobody listens on any
/// more removed: a browser that crashed leaves its file, and holds nothing. One that cannot be
/// told (any other error) stays, and counts as outside (`daemons`).
pub fn swept(sockets: &Path) {
    for (path, result) in connected(sockets) {
        if result.as_ref().is_err_and(dead) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Whether `error` says nobody is behind the socket: refused, or the file went.
fn dead(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
    )
}

/// A connection, or its error, to each socket of agent-browser's in `sockets` and the folders in
/// it, never through a link. What cannot be looked at is an error too, never skipped: a folder
/// or an entry that cannot be read, and a link named as a socket.
fn connected(sockets: &Path) -> Vec<(PathBuf, io::Result<std::os::unix::net::UnixStream>)> {
    use std::os::unix::fs::FileTypeExt;
    let listed = match std::fs::read_dir(sockets) {
        Ok(listed) => listed,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => return vec![(sockets.to_owned(), Err(error))],
    };
    let mut found = Vec::new();
    for entry in listed {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                found.push((sockets.to_owned(), Err(error)));
                continue;
            }
        };
        let path = entry.path();
        // The type of the entry itself, never of what a link leads to.
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(error) => {
                found.push((path, Err(error)));
                continue;
            }
        };
        let named_as_socket = path.extension().is_some_and(|kind| kind == "sock");
        if kind.is_dir() {
            found.extend(connected(&path));
        } else if kind.is_socket() && named_as_socket {
            let result = connect_at_once(&path);
            found.push((path, result));
        } else if kind.is_symlink() && named_as_socket {
            let error = io::Error::other("a link named as a socket");
            found.push((path, Err(error)));
        }
    }
    found
}

/// A connection to the socket at `path` that never waits for the daemon to accept it: a daemon
/// whose backlog is full (`WouldBlock`) would hold a blocking `connect` for ever.
fn connect_at_once(path: &Path) -> io::Result<std::os::unix::net::UnixStream> {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: a zeroed sockaddr_un is valid, and `bytes` is copied within its bounds.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path too long"));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (to, from) in address.sun_path.iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    // SAFETY: plain system calls; the descriptor is owned by the stream or closed here.
    unsafe {
        let fd = libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let connected = libc::connect(
            fd,
            (&raw const address).cast(),
            std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
        );
        if connected != 0 {
            let error = io::Error::last_os_error();
            libc::close(fd);
            // A non-blocking connect to a full backlog says EAGAIN, which `WouldBlock` is.
            return Err(error);
        }
        Ok(std::os::unix::net::UnixStream::from_raw_fd(fd))
    }
}

/// The pid of the process at the other end of `stream`, as the kernel recorded it.
fn peer(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::zeroed();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `length` bytes into the structure it is given.
    let got = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    // SAFETY: zeroed, and written by the getsockopt that succeeded.
    let pid = unsafe { credentials.assume_init() }.pid;
    (got == 0 && pid > 0).then_some(pid as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_record_of_a_process_that_took_the_pid_since_is_not_found() {
        // The record names this process, its namespaces, and a start time that is not its own:
        // another process that took the pid, and entered the same namespaces, is not the holder.
        let vault = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let inode = |kind: &str| {
            std::fs::metadata(format!("/proc/self/ns/{kind}"))
                .unwrap()
                .ino()
        };
        let started = crate::agents::start_time(me).unwrap();
        let record = |start: u64| format!("{me} {} {} {start}\n", inode("user"), inode("net"));
        std::fs::write(vault.path().join(RECORD), record(started + 1)).unwrap();
        assert!(found(vault.path()).is_none(), "found another process's");
        std::fs::write(vault.path().join(RECORD), record(started)).unwrap();
        assert!(found(vault.path()).is_some(), "not found");
    }

    #[test]
    fn without_landlocks_second_abi_the_browser_is_within_a_runs_reach() {
        // Yama keeps a run from attaching to the browser, not from opening `/proc/<pid>/root`
        // of the user's processes, which leads to the vault folder and the browser's socket:
        // Landlock alone closes both, whatever Yama says.
        for landlock in [0, 1] {
            let refused = out_of_reach(landlock).unwrap_err();
            for words in ["Landlock", "/proc/<pid>/root", "5.19"] {
                assert!(refused.contains(words), "{landlock}: {refused}");
            }
        }
        assert!(out_of_reach(2).is_ok());
        assert!(out_of_reach(6).is_ok());
    }

    #[test]
    fn a_network_made_is_used_only_where_a_run_stays_out_of_reach() {
        // Not only when it is made: a network found again, or one a close makes, is used on a
        // host whose Landlock is below its second ABI all the same without this check.
        let vault = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let inode = |kind: &str| {
            std::fs::metadata(format!("/proc/self/ns/{kind}"))
                .unwrap()
                .ino()
        };
        let started = crate::agents::start_time(me).unwrap();
        std::fs::write(
            vault.path().join(RECORD),
            format!("{me} {} {} {started}\n", inode("user"), inode("net")),
        )
        .unwrap();
        assert!(present(vault.path(), 1).is_err());
        assert!(present(vault.path(), 0).is_err());
        assert!(present(vault.path(), 2).unwrap().is_some());
    }

    fn listening(path: &Path) -> std::os::unix::net::UnixListener {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::net::UnixListener::bind(path).unwrap()
    }

    #[test]
    fn a_socket_in_a_subfolder_is_a_browser_too() {
        // `--namespace` may put a session's socket in a subfolder of the socket folder.
        let sockets = tempfile::tempdir().unwrap();
        let _top = listening(&sockets.path().join("default.sock"));
        let _deep = listening(&sockets.path().join("other/default.sock"));
        let _deeper = listening(&sockets.path().join("other/inner/x.sock"));
        assert_eq!(daemons(sockets.path()).len(), 3);
    }

    #[test]
    fn a_link_to_a_folder_is_not_followed_to_find_sockets() {
        let sockets = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let _there = listening(&elsewhere.path().join("x.sock"));
        std::os::unix::fs::symlink(elsewhere.path(), sockets.path().join("link")).unwrap();
        assert!(daemons(sockets.path()).is_empty());
    }

    #[test]
    fn a_daemon_that_stops_accepting_does_not_hold_the_check_and_counts_as_outside() {
        // Its backlog full, a blocking connect waits for ever: the check ends at once and fails
        // closed, as a browser whose holder is unknown (pid 0), which no network holds.
        let sockets = tempfile::tempdir().unwrap();
        let path = sockets.path().join("default.sock");
        let _listener = listening(&path);
        let mut kept = Vec::new();
        while let Ok(stream) = connect_at_once(&path) {
            kept.push(stream);
            assert!(kept.len() < 10_000, "the backlog never fills");
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let folder = sockets.path().to_owned();
        std::thread::spawn(move || sender.send(daemons(&folder)));
        let found = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the check is held by a daemon that does not accept");
        assert_eq!(found, vec![0]);
    }

    /// A socket in a folder whose path is longer than a socket address holds: bound by a short
    /// path through the folder's descriptor, and listed by its long one.
    fn bound_too_long(sockets: &Path) -> (std::fs::File, std::os::unix::net::UnixListener) {
        let mut deep = sockets.to_owned();
        while deep.as_os_str().len() < 140 {
            deep.push("a-folder-of-a-long-name");
        }
        std::fs::create_dir_all(&deep).unwrap();
        let folder = std::fs::File::open(&deep).unwrap();
        let by_descriptor = format!("/proc/self/fd/{}/default.sock", folder.as_raw_fd());
        let listener = std::os::unix::net::UnixListener::bind(&by_descriptor).unwrap();
        (folder, listener)
    }

    #[test]
    fn a_socket_that_cannot_be_reached_counts_as_outside() {
        // Fails closed on every error, not on "would block" alone: a socket whose path is too
        // long for a socket address, and one nobody listens on any more, are sockets the check
        // cannot tell, so none is skipped.
        let sockets = tempfile::tempdir().unwrap();
        let (_folder, _listener) = bound_too_long(sockets.path());
        assert_eq!(daemons(sockets.path()), vec![0], "a path too long");
        // Its listener gone: the file stays, and a connect is refused.
        let gone = tempfile::tempdir().unwrap();
        drop(listening(&gone.path().join("default.sock")));
        assert_eq!(daemons(gone.path()), vec![0], "a socket nobody listens on");
    }

    #[test]
    fn a_subfolder_that_cannot_be_read_counts_as_outside() {
        // Nobody can tell what a folder holds that cannot be listed: a browser may answer in it.
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads every folder: nothing to refuse
        }
        let sockets = tempfile::tempdir().unwrap();
        let hidden = sockets.path().join("other");
        std::fs::create_dir(&hidden).unwrap();
        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o000)).unwrap();
        let found = daemons(sockets.path());
        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(found, vec![0]);
    }

    #[test]
    fn a_link_named_as_a_socket_counts_as_outside() {
        // agent-browser follows it to the daemon it leads to: never skipped.
        let sockets = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let _there = listening(&elsewhere.path().join("x.sock"));
        std::os::unix::fs::symlink(
            elsewhere.path().join("x.sock"),
            sockets.path().join("default.sock"),
        )
        .unwrap();
        assert_eq!(daemons(sockets.path()), vec![0]);
    }

    #[test]
    fn a_socket_that_refuses_to_be_connected_to_counts_as_outside() {
        // EACCES: the file's permissions keep the connection from being made, and what answers
        // behind it cannot be told.
        if unsafe { libc::geteuid() } == 0 {
            return; // root connects through any mode
        }
        let sockets = tempfile::tempdir().unwrap();
        let path = sockets.path().join("default.sock");
        let _listener = listening(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(daemons(sockets.path()), vec![0]);
        assert!(alive(sockets.path()), "a socket that cannot be told");
    }

    #[test]
    fn a_daemon_that_stops_accepting_is_alive() {
        let sockets = tempfile::tempdir().unwrap();
        let path = sockets.path().join("default.sock");
        let _listener = listening(&path);
        let mut kept = Vec::new();
        while let Ok(stream) = connect_at_once(&path) {
            kept.push(stream);
            assert!(kept.len() < 10_000, "the backlog never fills");
        }
        assert!(alive(sockets.path()));
    }

    #[test]
    fn a_socket_nobody_listens_on_is_not_alive() {
        let sockets = tempfile::tempdir().unwrap();
        drop(listening(&sockets.path().join("default.sock")));
        assert!(!alive(sockets.path()));
    }

    #[test]
    fn a_socket_whose_path_is_too_long_is_alive() {
        // It cannot be connected to, so what answers behind it cannot be told: its profile stays.
        let sockets = tempfile::tempdir().unwrap();
        let (_folder, _listener) = bound_too_long(sockets.path());
        assert!(alive(sockets.path()));
    }

    #[test]
    fn a_socket_nobody_listens_on_is_swept_and_a_live_one_is_not() {
        let sockets = tempfile::tempdir().unwrap();
        let live = sockets.path().join("live.sock");
        let _listener = listening(&live);
        let stale = sockets.path().join("other/stale.sock");
        drop(listening(&stale));
        let link = sockets.path().join("link.sock");
        std::os::unix::fs::symlink(&stale, &link).unwrap();
        swept(sockets.path());
        assert!(live.exists(), "a live socket was removed");
        assert!(!stale.exists(), "a stale socket was kept");
        assert!(link.is_symlink(), "a link was removed");
    }
}
