//! One window (#302): a second `flottille window` brings the open one to the front instead of
//! opening another. Tested without a display: the first window is a listener of the test's own,
//! and the second command is the real binary, which must talk to it and leave. No window opens.

mod common;

use std::process::Stdio;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use common::User;
use flottille_core::paths::Paths;
use flottille_window::instance::{self, Instance, Start};

/// The paths of a throwaway user, as its commands see them.
fn paths(user: &User) -> Paths {
    Paths {
        runtime: user.path("runtime/flottille"),
        state: user.path("state/flottille"),
        config: user.path("config/flottille"),
        data: user.path("data/flottille"),
    }
}

/// The first window of a user.
fn first(paths: &Paths) -> Instance {
    match instance::acquire(paths).unwrap() {
        Start::First(instance) => instance,
        Start::Raised => panic!("another window is open"),
    }
}

/// The first window, serving: what it is asked to raise arrives on the receiver.
fn serving(paths: &Paths) -> Receiver<()> {
    let (raise, raises) = channel();
    first(paths).serve(move || {
        let _ = raise.send(());
    });
    raises
}

fn raised(raises: &Receiver<()>) -> bool {
    raises.recv_timeout(Duration::from_secs(5)).is_ok()
}

#[test]
fn a_second_window_command_brings_the_open_window_to_the_front() {
    let user = User::new();
    let raises = serving(&paths(&user));

    // The real command, with no display to open a window on: it has to leave on its own, saying
    // what it did.
    let output = user
        .command(&["window"])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("already open: asked it to come to the front"),
        "{output:?}"
    );
    assert!(raised(&raises));
}

#[test]
fn a_window_command_while_one_is_open_does_not_become_the_first() {
    let user = User::new();
    let paths = paths(&user);
    let _open = first(&paths);

    assert!(matches!(instance::acquire(&paths).unwrap(), Start::Raised));
}

#[test]
fn a_lock_held_for_an_instant_by_a_forked_child_does_not_stop_the_next_window() {
    use std::fs::File;

    let user = User::new();
    let paths = paths(&user);
    // What a window that closed leaves: its socket file, nobody listening. And its lock, still
    // held by a child between its fork and its exec: a second open file description.
    drop(first(&paths));
    let child = File::open(paths.window_lock()).unwrap();
    child.try_lock().unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        drop(child);
    });

    assert!(matches!(
        instance::acquire(&paths).unwrap(),
        Start::First(_)
    ));
    release.join().unwrap();
}

#[test]
fn a_window_that_is_open_is_still_raised() {
    let user = User::new();
    let raises = serving(&paths(&user));

    assert!(matches!(
        instance::acquire(&paths(&user)).unwrap(),
        Start::Raised
    ));
    assert!(raised(&raises));
}

#[test]
fn once_the_first_window_is_closed_the_next_command_opens_its_own() {
    let user = User::new();
    let paths = paths(&user);
    // Closed: the lock goes with the instance, the socket file stays.
    drop(first(&paths));
    assert!(paths.window_socket().exists());

    drop(first(&paths));
}

#[test]
fn a_socket_left_by_a_window_that_died_blocks_nobody() {
    let user = User::new();
    let paths = paths(&user);
    flottille_core::paths::ensure_private_dir(&paths.runtime).unwrap();
    // What a killed window leaves: its socket file, and nobody holding the lock.
    drop(std::os::unix::net::UnixListener::bind(paths.window_socket()).unwrap());
    assert!(paths.window_socket().exists());

    let raises = serving(&paths);

    assert!(matches!(instance::acquire(&paths).unwrap(), Start::Raised));
    assert!(raised(&raises));
}

#[test]
fn a_raise_sent_before_the_window_serves_is_kept_for_it() {
    // The window takes a moment to open between holding the place and serving it.
    let user = User::new();
    let paths = paths(&user);
    let open = first(&paths);
    assert!(matches!(instance::acquire(&paths).unwrap(), Start::Raised));

    let (raise, raises) = channel();
    open.serve(move || {
        let _ = raise.send(());
    });

    assert!(raised(&raises));
}

#[test]
fn something_else_said_to_the_socket_raises_nothing() {
    use std::io::Write as _;

    let user = User::new();
    let paths = paths(&user);
    let raises = serving(&paths);

    let mut stranger = std::os::unix::net::UnixStream::connect(paths.window_socket()).unwrap();
    stranger.write_all(b"something else\n").unwrap();
    drop(stranger);
    // A real raise after it: it is the only one to arrive.
    assert!(matches!(instance::acquire(&paths).unwrap(), Start::Raised));

    assert!(raised(&raises));
    assert!(
        raises.recv_timeout(Duration::from_millis(300)).is_err(),
        "a second raise came"
    );
}

#[test]
fn a_peer_that_sends_a_huge_line_neither_grows_memory_nor_holds_the_window() {
    use std::io::Write as _;

    let user = User::new();
    let paths = paths(&user);
    let raises = serving(&paths);

    // A megabyte and no end of line: only the first bytes are read, then the peer is let go.
    let mut stranger = std::os::unix::net::UnixStream::connect(paths.window_socket()).unwrap();
    let _ = stranger.write_all(&vec![b'x'; 1 << 20]);
    assert!(matches!(instance::acquire(&paths).unwrap(), Start::Raised));

    assert!(raised(&raises));
    assert!(raises.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn a_peer_that_trickles_bytes_holds_the_window_for_one_deadline_not_one_per_read() {
    use std::io::Write as _;

    let user = User::new();
    let paths = paths(&user);
    let raises = serving(&paths);

    // A byte every 300 ms and no end of line, for eight seconds: with a deadline per read it
    // would hold the window all that time.
    let socket = paths.window_socket();
    let trickle = std::thread::spawn(move || {
        let mut stranger = std::os::unix::net::UnixStream::connect(socket).unwrap();
        for _ in 0..26 {
            if stranger.write_all(b"r").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    std::thread::sleep(Duration::from_millis(100));
    let asked = std::time::Instant::now();
    assert!(matches!(instance::acquire(&paths).unwrap(), Start::Raised));

    assert!(raises.recv_timeout(Duration::from_secs(5)).is_ok());
    assert!(
        asked.elapsed() < Duration::from_secs(3),
        "the raise waited {:?} behind the peer",
        asked.elapsed()
    );
    trickle.join().unwrap();
}
