//! The command line's side of the socket: connect, start the daemon when it is not running, ask.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use flottille_core::paths::{Paths, ensure_private_dir};
use flottille_core::protocol::{Event, Message, Request};
use serde_json::Value;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(25);
/// How long `daemon stop` waits for the daemon to answer, then to stop, before it ends it.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// An error the daemon answered a request with: its code (`docs/protocol.md`) and its sentence.
#[derive(Debug)]
pub struct DaemonError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DaemonError {}

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
    /// The start of a line read before a timeout cut the read short.
    partial: String,
}

impl Client {
    /// The running daemon, or `None` when there is none.
    pub fn connect(paths: &Paths) -> Result<Option<Client>> {
        match UnixStream::connect(paths.socket()) {
            Ok(stream) => Ok(Some(Client {
                reader: BufReader::new(stream.try_clone()?),
                writer: stream,
                next_id: 1,
                partial: String::new(),
            })),
            // No socket, or one left by a daemon that died: nobody is listening.
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::NotFound | ErrorKind::ConnectionRefused
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(format!(
                "cannot reach the daemon at {}: {error}",
                paths.socket().display()
            )
            .into()),
        }
    }

    pub fn connect_or_start(paths: &Paths) -> Result<Client> {
        if let Some(client) = Client::connect(paths)? {
            return Ok(client);
        }
        let deadline = Instant::now() + START_TIMEOUT;
        let mut daemon = start_daemon(paths)?;
        loop {
            if let Some(client) = Client::connect(paths)? {
                return Ok(client);
            }
            // Another command may have started a daemon at the same moment: ours then refuses to
            // run, and the other one is about to listen. One that is stopping holds the lock for a
            // moment too: try again until the deadline.
            if daemon.try_wait()?.is_some() {
                daemon = start_daemon(paths)?;
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "the daemon did not start; its log is {}",
                    paths.log().display()
                )
                .into());
            }
            std::thread::sleep(POLL);
        }
    }

    /// The connection itself, for a client that speaks the protocol on its own (the window).
    pub fn into_stream(self) -> UnixStream {
        self.writer
    }

    /// Sends a request and returns its result; events arriving meanwhile are not for a request.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.send(method, params)?;
        loop {
            let message = self.read()?.ok_or("the daemon closed the connection")?;
            if message.id.as_deref() != Some(&id) {
                continue;
            }
            if let Some(error) = message.error {
                return Err(Box::new(DaemonError {
                    code: error.code,
                    message: error.message,
                }));
            }
            return Ok(message.result.unwrap_or(Value::Null));
        }
    }

    /// Sends a request without waiting for its answer: its id.
    pub fn send(&mut self, method: &str, params: Value) -> Result<String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let request = Request {
            id: id.clone(),
            method: method.to_owned(),
            params,
        };
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        Ok(id)
    }

    /// The next event of a subscription, `None` when the daemon closed the connection.
    pub fn next_event(&mut self) -> Result<Option<Event>> {
        while let Some(message) = self.read()? {
            if let Some(event) = message.event {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }

    /// The next event of a subscription, `None` when none came within `wait`.
    pub fn next_event_within(&mut self, wait: Duration) -> Result<Option<Event>> {
        let deadline = Instant::now() + wait;
        let event = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break None;
            }
            self.writer.set_read_timeout(Some(left))?;
            match self.read() {
                Ok(Some(Message {
                    event: Some(event), ..
                })) => break Some(event),
                Ok(Some(_)) => {}
                Ok(None) => {
                    self.writer.set_read_timeout(None)?;
                    return Err("the daemon closed the connection".into());
                }
                Err(error) if is_timeout(error.as_ref()) => break None,
                Err(error) => return Err(error),
            }
        };
        self.writer.set_read_timeout(None)?;
        Ok(event)
    }

    fn read(&mut self) -> Result<Option<Message>> {
        // A line cut short by a timeout stays in `partial` for the next read.
        if self.reader.read_line(&mut self.partial)? == 0 {
            return Ok(None);
        }
        let line = std::mem::take(&mut self.partial);
        Ok(Some(serde_json::from_str(&line)?))
    }
}

fn is_timeout(error: &(dyn std::error::Error + 'static)) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

/// Starts `flottille daemon` in its own session, detached from this terminal, its output in the
/// daemon's log.
fn start_daemon(paths: &Paths) -> Result<Child> {
    ensure_private_dir(&paths.state)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(paths.log())?;
    let mut command = Command::new(program_to_start(std::env::current_exe()?));
    command
        .arg("daemon")
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe, which is all a pre_exec closure may call.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command.spawn()?)
}

/// The program a daemon is started from: this one, or, when it was replaced while it ran
/// (`flottille update`, `install.sh`), the new file at its path. Linux then names the running
/// one `<path> (deleted)`, which cannot be started.
fn program_to_start(exe: PathBuf) -> PathBuf {
    match exe
        .to_str()
        .and_then(|path| path.strip_suffix(" (deleted)"))
    {
        Some(path) if std::path::Path::new(path).exists() => PathBuf::from(path),
        _ => exe,
    }
}

/// The process id the daemon holding the lock wrote in it; empty when there is none.
pub fn locked_by(paths: &Paths) -> String {
    std::fs::read_to_string(paths.lock())
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// How `daemon stop` ended the daemon.
pub enum Stopped {
    /// It stopped as it was asked.
    Asked,
    /// It did not, for the reason said: it was ended, with the processes it ran, counted.
    Ended(String, usize),
}

/// Asks the daemon `client` reaches to stop and waits for it. Its refusal is the error; a daemon
/// that does not answer within `STOP_WAIT` is ended at once; one that answered stops its agents one after another, up
/// to `STOPPING_EACH` each, so it is given that much more for each process it runs. Past it, it
/// and the processes it ran are ended (`end`), its agents' bridges included, which a stuck daemon
/// leaves unable to end on their own.
pub fn stop(paths: &Paths, mut client: Client) -> Result<Stopped> {
    let pid = locked_by(paths);
    // Opened before it is asked: the pid it held may be taken by another process once it ends.
    let daemon = pid
        .parse()
        .ok()
        .and_then(|pid| Daemon::open(pid, paths.sessions()));
    let wait = std::env::var("FLOTTILLE_STOP_WAIT_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(STOP_WAIT, Duration::from_millis);
    client.writer.set_read_timeout(Some(wait))?;
    let asked = client.request("daemon.stop", Value::Null);
    drop(client);
    // A daemon that closed the connection as it stopped is waited for all the same.
    let (asked, deadline) = match asked {
        // It answered and refused (a client an agent started): it runs, and is not to be ended.
        Err(error) if error.is::<DaemonError>() => return Err(error),
        Err(error) if is_timeout(error.as_ref()) => (
            Err(format!("it did not answer daemon.stop within {} s", wait.as_secs()).into()),
            Instant::now(),
        ),
        asked => {
            let running = daemon.as_ref().map_or(0, |daemon| daemon.tree().len());
            let running = u32::try_from(running).unwrap_or(u32::MAX);
            (asked, Instant::now() + wait + STOPPING_EACH * running)
        }
    };
    let why = match wait_until(paths, &pid, deadline) {
        Ok(()) => return Ok(Stopped::Asked),
        Err(still) => asked.err().unwrap_or(still),
    };
    let Some(daemon) = daemon else {
        return Err(
            format!("the daemon's lock names no process ({pid:?}): it was not ended").into(),
        );
    };
    let ended = daemon.end();
    wait_until(paths, &pid, Instant::now() + START_TIMEOUT)?;
    Ok(Stopped::Ended(why.to_string(), ended))
}

/// How long a stopping daemon may take for each process it runs: an agent's task is waited for
/// twice its grace (`agents::stop_all`), a run's for its grace (`runs`).
const STOPPING_EACH: Duration = Duration::from_secs(5);
/// How long the processes of a daemon being ended have to go on `SIGTERM` before `SIGKILL`: a git
/// killed outright leaves its `index.lock`.
const TERM_GRACE: Duration = Duration::from_secs(2);

/// A process, through a pidfd: signalled through it, never one that took its pid since.
struct Process {
    pid: u32,
    fd: libc::c_int,
}

impl Process {
    fn open(pid: u32) -> Option<Process> {
        // SAFETY: pidfd_open takes a pid and flags and returns a descriptor or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        (fd >= 0).then_some(Process {
            pid,
            fd: fd as libc::c_int,
        })
    }

    fn signal(&self, signal: libc::c_int) {
        // SAFETY: the descriptor is a pidfd of ours; no siginfo is passed.
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.fd,
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
    }

    /// Whether it ended: its pidfd reads as ready then.
    fn ended(&self) -> bool {
        let mut poll = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd, valid for the call.
        unsafe { libc::poll(&mut poll, 1, 0) == 1 }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the descriptor is ours and closed once.
        unsafe { libc::close(self.fd) };
    }
}

/// The daemon being stopped, and the file where it says the sessions it started
/// (`Paths::sessions`).
struct Daemon(Process, PathBuf);

impl Daemon {
    fn open(pid: u32, sessions: PathBuf) -> Option<Daemon> {
        Process::open(pid).map(|process| Daemon(process, sessions))
    }

    /// The leaders of the sessions the daemon started that still run: never one that took the
    /// pid of one since, nor a child of its that leads a session it did not start.
    fn leaders(&self) -> Vec<u32> {
        let said = std::fs::read_to_string(&self.1).unwrap_or_default();
        said.lines()
            .filter_map(|line| {
                let (pid, at) = line.split_once(' ')?;
                let (pid, at) = (pid.parse().ok()?, at.parse().ok()?);
                (crate::agents::start_time(pid) == Some(at)).then_some(pid)
            })
            .collect()
    }

    /// The processes below the daemon that are its own: those of its session (its agents and
    /// their groups) and of the sessions it started (runs, setup steps, probes). As their
    /// subreaper it holds more: a helper something it ran left detached in a session of its own
    /// (an ssh control master, an agent of gpg, git's credential cache), even one handed to it as
    /// its child, may serve the user's terminals too, and is never its to end.
    fn tree(&self) -> Vec<u32> {
        let daemon = self.0.pid;
        let leaders = self.leaders();
        let mut tree = Vec::new();
        let mut next = vec![daemon];
        while let Some(pid) = next.pop() {
            // A detached helper's own children are left with it.
            for child in children_of(pid) {
                if session(child).is_some_and(|session| owned(daemon, &leaders, session)) {
                    tree.push(child);
                    next.push(child);
                }
            }
        }
        tree
    }

    /// Ends the daemon and its processes (`tree`): `SIGTERM`, then `SIGKILL` to those left after
    /// `TERM_GRACE`, each through a pidfd opened, then checked to name a process still in one of
    /// its sessions, so that a pid taken since by another process is never signalled. How many
    /// processes it ran.
    fn end(self) -> usize {
        let daemon = self.0.pid;
        let leaders = self.leaders();
        let processes: Vec<Process> = self
            .tree()
            .into_iter()
            .filter_map(Process::open)
            .filter(|process| {
                session(process.pid).is_some_and(|session| owned(daemon, &leaders, session))
            })
            .collect();
        let all: Vec<&Process> = std::iter::once(&self.0).chain(&processes).collect();
        for process in &all {
            process.signal(libc::SIGTERM);
        }
        let deadline = Instant::now() + TERM_GRACE;
        while Instant::now() < deadline && !all.iter().all(|process| process.ended()) {
            std::thread::sleep(POLL);
        }
        for process in all.iter().filter(|process| !process.ended()) {
            process.signal(libc::SIGKILL);
        }
        processes.len()
    }
}

/// Whether a process of `session` is the daemon's own: its session, or one it started.
fn owned(daemon: u32, leaders: &[u32], session: u32) -> bool {
    session == daemon || leaders.contains(&session)
}

/// The session of `pid`, while it runs.
fn session(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name, field 2, is in parentheses and may hold spaces.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    if matches!(fields.first()?.chars().next()?, 'Z' | 'X') {
        return None;
    }
    fields.get(3)?.parse().ok()
}

/// The children of `pid`, of every thread of it.
fn children_of(pid: u32) -> Vec<u32> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|children| {
            children
                .split_whitespace()
                .filter_map(|child| child.parse().ok())
                .collect::<Vec<u32>>()
        })
        .collect()
}

/// Waits until the daemon `pid` (`locked_by`, read before it was asked to stop) has let go of its
/// lock, so that the next command starts a new one; an error past `deadline`.
fn wait_until(paths: &Paths, pid: &str, deadline: Instant) -> Result<()> {
    let lock = match File::open(paths.lock()) {
        Ok(lock) => lock,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while lock.try_lock().is_err() {
        // A client that connected meanwhile (a window reconnecting) may have started a daemon,
        // which took the lock as soon as it was free: the stopped one is gone.
        if locked_by(paths) != pid {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err("it is still running".into());
        }
        std::thread::sleep(POLL);
    }
    lock.unlock()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_is_started_from_the_new_binary_when_this_one_was_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille");
        let deleted = dir.path().join("flottille (deleted)");

        // Replaced: the path holds the new binary again.
        std::fs::write(&path, "").unwrap();
        assert_eq!(program_to_start(deleted.clone()), path);
        // Removed, nothing at the path: the name is kept, and its start fails saying it.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(program_to_start(deleted.clone()), deleted);
        // Not replaced.
        assert_eq!(program_to_start(path.clone()), path);
    }
}
