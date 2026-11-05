//! The daemon's children (`docs/reference/processes.md`): every process it starts goes through
//! here and is recorded, so that the sweep waits for the ones it was handed as their subreaper
//! and never for one an owner waits itself.

use std::collections::HashMap;
use std::io;
use std::sync::Mutex;

use crate::agents::start_time;

/// The children the daemon started, by pid, with when each started: what tells one from a
/// process that took its pid since. Held while a child is spawned and recorded, and while the
/// sweep runs, so the sweep never sees a child of the daemon's that is not recorded yet.
static STARTED: Mutex<Option<HashMap<u32, u64>>> = Mutex::new(None);

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
        && let Some(at) = start_time(pid)
    {
        started.get_or_insert_default().insert(pid, at);
    }
    Ok(child)
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
    /// In clock ticks after boot.
    started: u64,
}

fn stat(pid: u32) -> Option<Stat> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name, field 2, is in parentheses and may hold spaces.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(Stat {
        state: fields.first()?.chars().next()?,
        started: fields.get(19)?.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
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
