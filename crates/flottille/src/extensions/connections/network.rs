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
/// The program that gives the namespace its way out.
const PASTA: &str = "pasta";
/// In the vault folder: the holder's pid and its namespaces' inodes, and pasta's pid.
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
    pasta().ok_or_else(|| format!("{PASTA} is not on the PATH"))?;
    if crate::mounts::namespaces() == crate::mounts::Namespaces::Refused {
        return Err("the kernel refuses user namespaces to this user".into());
    }
    Ok(())
}

/// The namespace recorded in the vault folder `vault`, while its holder runs.
pub fn found(vault: &Path) -> Option<Network> {
    let text = std::fs::read_to_string(vault.join(RECORD)).ok()?;
    let mut words = text.split_whitespace();
    let (pid, user, net): (u32, u64, u64) = (
        words.next()?.parse().ok()?,
        words.next()?.parse().ok()?,
        words.next()?.parse().ok()?,
    );
    let network = opened(pid).ok()?;
    // The same namespaces, not those of a process that took the pid since.
    (inode(&network.user) == Some(user) && inode(&network.net) == Some(net)).then_some(network)
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
    let pasta = pasta().ok_or_else(|| format!("{PASTA} is not on the PATH"))?;
    ended(vault);
    let program = std::env::current_exe()
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
    let record = format!(
        "{pid} {} {}\n",
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

/// The pids agent-browser's daemons in the socket folder `sockets` say they run as (`<session>.pid`),
/// alive.
pub fn daemons(sockets: &Path) -> Vec<u32> {
    let Ok(listed) = std::fs::read_dir(sockets) else {
        return Vec::new();
    };
    listed
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|kind| kind == "pid"))
        .filter_map(|entry| {
            std::fs::read_to_string(entry.path())
                .ok()?
                .trim()
                .parse()
                .ok()
        })
        .filter(|pid: &u32| Path::new(&format!("/proc/{pid}")).exists())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
