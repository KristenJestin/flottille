//! The command line's side of the socket: connect, start the daemon when it is not running, ask.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use flottille_core::paths::{Paths, ensure_private_dir};
use flottille_core::protocol::{Event, Message, Request};
use serde_json::Value;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(25);

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

impl Client {
    /// The running daemon, or `None` when there is none.
    pub fn connect(paths: &Paths) -> Result<Option<Client>> {
        match UnixStream::connect(paths.socket()) {
            Ok(stream) => Ok(Some(Client {
                reader: BufReader::new(stream.try_clone()?),
                writer: stream,
                next_id: 1,
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

    /// Sends a request and returns its result; events arriving meanwhile are not for a request.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value> {
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
        loop {
            let message = self.read()?.ok_or("the daemon closed the connection")?;
            if message.id.as_deref() != Some(&id) {
                continue;
            }
            if let Some(error) = message.error {
                return Err(error.message.into());
            }
            return Ok(message.result.unwrap_or(Value::Null));
        }
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

    fn read(&mut self) -> Result<Option<Message>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&line)?))
    }
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
    let mut command = Command::new(std::env::current_exe()?);
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

/// Waits until the daemon has let go of its lock, so that the next command starts a new one.
pub fn wait_until_stopped(paths: &Paths) -> Result<()> {
    let lock = match File::open(paths.lock()) {
        Ok(lock) => lock,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let deadline = Instant::now() + START_TIMEOUT;
    while lock.try_lock().is_err() {
        if Instant::now() > deadline {
            return Err("the daemon was asked to stop but is still running".into());
        }
        std::thread::sleep(POLL);
    }
    lock.unlock()?;
    Ok(())
}
