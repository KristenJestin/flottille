//! One window (`docs/cli.md`, `flottille window`): the first window holds a lock beside the
//! daemon's, in the runtime folder, and listens on a socket there. A later `flottille window`
//! finds the lock held, says "raise" on that socket and leaves; the first window brings itself
//! to the front. The kernel lets the lock go when the window's process ends, however it ends, so
//! a window that died blocks nobody, and its socket file is replaced by the next.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

use flottille_core::paths::{Paths, ensure_private_dir};

/// What a command that asks to raise says.
const RAISE: &str = "raise";
/// What is read of a peer: a word and its end of line, and a little more.
const LONGEST: usize = 16;
/// How long a peer has, from its connection to its end of line.
const DEADLINE: Duration = Duration::from_secs(1);

pub enum Start {
    /// Nobody else has a window open: this one is the window, and serves the raises.
    First(Instance),
    /// A window is open, and was asked to come to the front.
    Raised,
}

/// The place of the one window, held for as long as it lives.
pub struct Instance {
    _lock: File,
    listener: UnixListener,
}

/// How many times the place is tried, and its window asked, before giving up; and the wait
/// between two.
const TRIES: usize = 20;
const PAUSE: Duration = Duration::from_millis(100);

/// Takes the place of the one window, or asks the window that has it to come to the front.
///
/// A place that looks held is not always a window: a child process between its `fork` and its
/// `exec` holds a copy of the lock for an instant, after the window that took it is gone. So
/// the lock is tried again after each refused connection, and a window that is opening (it holds
/// the place and has not bound its socket yet) is waited for just the same.
pub fn acquire(paths: &Paths) -> io::Result<Start> {
    ensure_private_dir(&paths.runtime).map_err(io::Error::other)?;
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(paths.window_lock())?;
    let mut last = None;
    for _ in 0..TRIES {
        match lock.try_lock() {
            Ok(()) => return first(paths, lock),
            Err(TryLockError::WouldBlock) => match UnixStream::connect(paths.window_socket()) {
                Ok(mut stream) => {
                    writeln!(stream, "{RAISE}")?;
                    return Ok(Start::Raised);
                }
                Err(error) => last = Some(error),
            },
            Err(TryLockError::Error(error)) => return Err(error),
        }
        std::thread::sleep(PAUSE);
    }
    Err(last.unwrap_or_else(|| io::Error::other("no window answers")))
}

/// This process is the window: its socket replaces the one a window that died, or closed, left.
fn first(paths: &Paths, lock: File) -> io::Result<Start> {
    match std::fs::remove_file(paths.window_socket()) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let listener = UnixListener::bind(paths.window_socket())?;
    Ok(Start::First(Instance {
        _lock: lock,
        listener,
    }))
}

impl Instance {
    /// Calls `raise` each time a command asks the window to come to the front, from a thread of
    /// its own that lives as long as the process. What was asked before this is kept by the
    /// socket and arrives now.
    pub fn serve(self, raise: impl Fn() + Send + 'static) {
        std::thread::spawn(move || {
            // The lock lives as long as this thread: the process.
            let _lock = self._lock;
            for stream in self.listener.incoming() {
                let Ok(stream) = stream else { continue };
                if says_raise(stream) {
                    raise();
                }
            }
        });
    }
}

/// Whether the peer says "raise" and ends its line. It is read for a short while and a few
/// bytes only: a local peer that says nothing, trickles bytes or never ends its line cannot hold
/// the others back, nor make the window keep what it sends.
fn says_raise(mut stream: UnixStream) -> bool {
    let deadline = Instant::now() + DEADLINE;
    let mut said = [0u8; LONGEST];
    let mut length = 0;
    while length < LONGEST && !said[..length].contains(&b'\n') {
        // The time that is left of the whole line, not a new one at each read.
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        if stream.set_read_timeout(Some(left)).is_err() {
            return false;
        }
        match stream.read(&mut said[length..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => length += read,
        }
    }
    said[..length].trim_ascii() == RAISE.as_bytes()
}
