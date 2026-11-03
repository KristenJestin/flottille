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
        String::from_utf8_lossy(&output.stdout).contains("already open"),
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
