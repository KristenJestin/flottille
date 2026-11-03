//! One window (`docs/cli.md`, `flottille window`): the first window holds a lock beside the
//! daemon's, in the runtime folder, and listens on a socket there. A later `flottille window`
//! finds the lock held, says "raise" on that socket and leaves; the first window brings itself
//! to the front. The kernel lets the lock go when the window's process ends, however it ends, so
//! a window that died blocks nobody, and its socket file is replaced by the next.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
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

/// Takes the place of the one window, or asks the window that has it to come to the front.
pub fn acquire(paths: &Paths) -> io::Result<Start> {
    ensure_private_dir(&paths.runtime).map_err(io::Error::other)?;
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(paths.window_lock())?;
    match lock.try_lock() {
        Ok(()) => {
            // The socket file of a window that died, or of one that closed.
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
        Err(TryLockError::WouldBlock) => {
            raise(&paths.window_socket())?;
            Ok(Start::Raised)
        }
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Says "raise" to the window that holds the place. It may not listen yet, between holding the
/// place and binding the socket: a moment is waited for.
fn raise(socket: &Path) -> io::Result<()> {
    let mut last = None;
    for _ in 0..20 {
        match UnixStream::connect(socket) {
            Ok(mut stream) => return writeln!(stream, "{RAISE}"),
            Err(error) => last = Some(error),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(last.unwrap_or_else(|| io::Error::other("no window answers")))
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
