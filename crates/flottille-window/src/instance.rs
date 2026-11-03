//! One window (`docs/cli.md`, `flottille window`): the first window holds a lock beside the
//! daemon's, in the runtime folder, and listens on a socket there. A later `flottille window`
//! finds the lock held, says "raise" on that socket and leaves; the first window brings itself
//! to the front. The kernel lets the lock go when the window's process ends, however it ends, so
//! a window that died blocks nobody, and its socket file is replaced by the next.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use flottille_core::paths::{Paths, ensure_private_dir};

/// What a command that asks to raise says.
const RAISE: &str = "raise";

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
                // A client that connects and says nothing must not hold the others back.
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok() && line.trim() == RAISE {
                    raise();
                }
            }
        });
    }
}
