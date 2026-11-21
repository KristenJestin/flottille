//! `flottille open <agent>` (`docs/cli.md`): the window comes forward on the agent's session, as
//! the default action of a desktop notification asks it (#1002). The open window is played by a
//! listener on its socket, in a throwaway user's runtime folder: no window opens on a screen.

use crate::common;
use crate::desktop_notifications::{project, root};

use std::io::BufRead as _;
use std::os::unix::net::UnixListener;

use common::User;

#[test]
fn open_asks_the_open_window_for_the_session_of_the_agent() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, _) = root(&user, "hello");
    let window = UnixListener::bind(user.path("runtime/flottille/window.sock")).unwrap();

    let output = user.run(&["open", &id]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (stream, _) = window.accept().unwrap();
    let mut line = String::new();
    std::io::BufReader::new(stream)
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), format!("open {id}"));
}

#[test]
fn open_names_no_agent_that_is_not_there() {
    let user = User::with_fake_agent();
    project(&user);

    let output = user.run(&["open", "nobody"]);

    assert!(!output.status.success());
}
