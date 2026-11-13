//! The daemon's children (`docs/reference/processes.md`): every process it starts goes through
//! here and is recorded, so that the sweep waits for the ones it was handed as their subreaper
//! and never for one an owner waits itself; and the commands of runs, setup steps and probes
//! each get a session of their own, ended whole with them.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

use crate::agents::start_time;

/// The children the daemon started, by pid, with when each started: what tells one from a
/// process that took its pid since. Held while a child is spawned and recorded, and while the
/// sweep runs, so the sweep never sees a child of the daemon's that is not recorded yet.
static STARTED: crate::locks::Lock<Option<HashMap<u32, u64>>> =
    crate::locks::Lock::named("STARTED", None);

/// The children the daemon started in a session of their own (`in_own_session`), by pid, with
/// when each started, and the file that says them (`Paths::sessions`): what `daemon stop` ends
/// with the daemon, read by another process, which cannot tell them from a helper that detached
/// in a session of its own and was handed to the daemon. Taken only while `STARTED` is held.
static SESSIONS: crate::locks::Lock<Option<(PathBuf, HashMap<u32, u64>)>> =
    crate::locks::Lock::named("SESSIONS", None);

/// At the daemon's start: the sessions it starts are said in `file`, none yet.
pub fn say_sessions_in(file: PathBuf) -> io::Result<()> {
    let _started = STARTED.lock().unwrap();
    write_sessions(&file, &HashMap::new())?;
    *SESSIONS.lock().unwrap() = Some((file, HashMap::new()));
    Ok(())
}

fn write_sessions(file: &Path, sessions: &HashMap<u32, u64>) -> io::Result<()> {
    let text: String = sessions
        .iter()
        .map(|(pid, at)| format!("{pid} {at}\n"))
        .collect();
    // Whole or not at all, for a `daemon stop` reading it meanwhile.
    let next = file.with_extension("sessions.next");
    std::fs::write(&next, text)?;
    std::fs::rename(&next, file)
}

/// Spawns a child its caller waits for itself, recorded as the daemon's own.
pub fn spawn(command: &mut tokio::process::Command) -> io::Result<tokio::process::Child> {
    recorded(|| command.spawn(), tokio::process::Child::id)
}

/// `spawn`, for a child of `std::process`.
pub fn spawn_std(command: &mut std::process::Command) -> io::Result<std::process::Child> {
    recorded(|| command.spawn(), |child| Some(child.id()))
}

fn recorded<C>(
    spawn: impl FnOnce() -> io::Result<C>,
    pid: impl Fn(&C) -> Option<u32>,
) -> io::Result<C> {
    let mut started = STARTED.lock().unwrap();
    let child = spawn()?;
    // Not waited for until it is handed out: it is still there, a zombie at worst.
    if let Some(pid) = pid(&child)
        && let Some(child) = stat(pid)
    {
        started.get_or_insert_default().insert(pid, child.started);
        // The spawn returns once the child ran its `pre_exec`: one started in a session of
        // its own leads it already.
        if child.session == pid {
            started_session(pid, child.started);
        }
    }
    Ok(child)
}

fn started_session(pid: u32, at: u64) {
    let mut guard = SESSIONS.lock().unwrap();
    let Some((file, sessions)) = guard.as_mut() else {
        return;
    };
    // The ones that ended since are forgotten here, the only time the file is written again.
    sessions.retain(|pid, at| start_time(*pid) == Some(*at));
    sessions.insert(pid, at);
    if let Err(error) = write_sessions(file, sessions) {
        eprintln!(
            "flottille daemon: cannot write {} ({error}): daemon stop may leave a run running",
            file.display()
        );
    }
}

/// The command starts a session of its own, which it leads: what it starts stays in it, even in
/// a process group of its own (pnpm's commands), and is ended with it (`end_session`).
pub fn in_own_session(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    // SAFETY: setsid is async-signal-safe, which is all a pre_exec closure may call.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
    }
}

/// Kills every process of the session `leader` started (`in_own_session`), whatever its group,
/// off the async workers: `end_session_now` reads all of `/proc`, more than once, and pauses.
pub async fn end_session(leader: u32) {
    let _ = tokio::task::spawn_blocking(move || end_session_now(leader)).await;
}

/// `end_session`, on the calling thread. Each process is signalled through a pidfd opened before
/// its session is read again, so a process that took the pid of one that ended meanwhile is
/// never signalled. Again while one is found, up to `ROUNDS` times, after a pause that grows: a
/// process may start another as it is killed, and a killed one takes a moment to leave.
pub fn end_session_now(leader: u32) {
    for round in 0..ROUNDS {
        if round > 0 {
            std::thread::sleep(Duration::from_millis(2 * round as u64));
        }
        let mut found = false;
        for pid in processes() {
            if !in_session(pid, leader) {
                continue;
            }
            // SAFETY: pidfd_open takes a pid and flags and returns a descriptor or -1.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
            if fd < 0 {
                let error = io::Error::last_os_error();
                // One that ended meanwhile is no failure; anything else (a kernel before 5.3, a
                // seccomp filter) is, and no process is ever signalled by its bare pid instead.
                if error.raw_os_error() != Some(libc::ESRCH) {
                    PIDFD_FAILED.call_once(|| {
                        eprintln!(
                            "flottille daemon: cannot open a pidfd ({error}): what a run leaves \
                             outside its process group is not ended with it"
                        );
                    });
                }
                continue;
            }
            let fd = fd as libc::c_int;
            if in_session(pid, leader) {
                found = true;
                // SAFETY: the descriptor is the pidfd just opened; no siginfo is passed.
                unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        fd,
                        libc::SIGKILL,
                        std::ptr::null::<libc::siginfo_t>(),
                        0,
                    );
                }
            }
            // SAFETY: the descriptor is ours and closed once.
            unsafe { libc::close(fd) };
        }
        if !found {
            return;
        }
    }
}

/// How many times `end_session` reads `/proc` at most.
const ROUNDS: u32 = 10;

/// Said once in the daemon's log, not at every run's end.
static PIDFD_FAILED: Once = Once::new();

/// Whether the process runs in the session `leader` leads; one that ended runs nothing.
fn in_session(pid: u32, leader: u32) -> bool {
    stat(pid).is_some_and(|stat| stat.session == leader && !matches!(stat.state, 'Z' | 'X'))
}

fn processes() -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect()
}

/// Waits for every child of the daemon that ended and that the daemon did not start: the
/// processes it was handed as their subreaper, which nobody else waits for, whatever their
/// session or group. A child it started is waited for by whoever started it (tokio,
/// `std::process`), by its pid: waiting for it here would take its status away.
pub fn reap_strays() {
    let mut guard = STARTED.lock().unwrap();
    let started = guard.get_or_insert_default();
    // The ones waited for since: gone, or their pid taken by another process.
    started.retain(|pid, at| start_time(*pid) == Some(*at));
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return;
    };
    for task in tasks.flatten() {
        let Ok(children) = std::fs::read_to_string(task.path().join("children")) else {
            continue;
        };
        for pid in children
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok())
        {
            if let Some(child) = stat(pid)
                && child.state == 'Z'
                && started.get(&pid) != Some(&child.started)
            {
                // SAFETY: waitpid has no memory precondition with a null status.
                unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG) };
            }
        }
    }
}

/// The fields of `/proc/<pid>/stat` read here.
struct Stat {
    state: char,
    parent: u32,
    session: u32,
    /// In clock ticks after boot.
    started: u64,
}

fn stat(pid: u32) -> Option<Stat> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name, field 2, is in parentheses and may hold spaces.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(Stat {
        state: fields.first()?.chars().next()?,
        parent: fields.get(1)?.parse().ok()?,
        session: fields.get(3)?.parse().ok()?,
        started: fields.get(19)?.parse().ok()?,
    })
}

/// Whether `pid` is `ancestor` or one of its descendants, walking up the parents; a process that
/// cannot be read (gone already, or out of sight) counts as one.
pub fn descends_from(pid: u32, ancestor: u32) -> bool {
    let mut pid = pid;
    let Some(mut process) = stat(pid) else {
        return true;
    };
    loop {
        if pid == ancestor {
            return true;
        }
        // Up to init, or to a process with no parent in sight.
        if process.parent == 0 {
            return false;
        }
        let Some(parent) = stat(process.parent) else {
            return true;
        };
        // A parent that started after its child is another process under the pid of one that
        // ended: the child was taken in since, and its ancestry is not known.
        if parent.started > process.started {
            return true;
        }
        pid = process.parent;
        process = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_descends_from_its_parents_not_from_its_children_and_one_gone_counts_as_one() {
        let own = std::process::id();
        let parent = stat(own).unwrap().parent;

        assert!(descends_from(own, own));
        assert!(descends_from(own, parent));
        assert!(!descends_from(parent, own));
        assert!(!descends_from(1, own));
        // No such process: refused rather than guessed.
        assert!(descends_from(u32::MAX, own));
        assert!(descends_from(0, own));
    }

    /// The code of the daemon's, before its tests: what the daemon runs.
    fn daemon_code(text: &str) -> &str {
        text.split("#[cfg(test)]\nmod tests").next().unwrap()
    }

    #[test]
    fn every_child_the_daemon_starts_is_spawned_here() {
        // The command lines' own processes and the bridges run outside the daemon.
        let outside = [
            "children.rs",
            "client.rs",
            "update.rs",
            "bridge.rs",
            "codex_bridge.rs",
        ];
        // Not spawns, or not the daemon's: an HTTP answer's status, and what the web reader's
        // own process tries in a test of its sandbox.
        let not_spawns = ["response.status()", "Command::new(named(\"run\"))"];
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        let mut folders = vec![folder];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(folder).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name().into_string().unwrap();
                if path.is_dir() {
                    folders.push(path);
                } else if name.ends_with(".rs") && !outside.contains(&name.as_str()) {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (number, line) in daemon_code(&text).lines().enumerate() {
                        if [".spawn()", ".output()", ".status()"]
                            .iter()
                            .any(|call| line.contains(call))
                            && !not_spawns.iter().any(|known| line.contains(known))
                        {
                            found.push(format!("{}:{}", path.display(), number + 1));
                        }
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "spawned without children::spawn: {found:?}"
        );
    }
}
