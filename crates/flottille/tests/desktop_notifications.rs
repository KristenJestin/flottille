//! What the daemon says to the desktop (#1002): the system's own notifications, through the
//! freedesktop specification on a session bus, with a sound, or a toast in the window when the
//! window is focused. A private `dbus-daemon` and a fake notification server on it, a fake audio
//! sink, the fake ACP agent and a throwaway user: never the machine's bus, nor its sound.

use crate::common;

use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::desktop::{self, Bus, Notifications, Window};
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A project `app` over a throwaway repository.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

fn state(user: &User, id: &str) -> String {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A root of `app` whose first turn ended: its id and its MCP session.
fn root(user: &User, message: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || state(user, &id) == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

fn held_number(answer: Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    let rest = answer
        .text
        .split_once("request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
        .1
        .to_owned();
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// The number of the request a call the gate holds made.
fn held(mcp: &Mcp) -> u64 {
    held_number(mcp.call("fs_read", json!({"path": ".env"})))
}

fn pending(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

fn rows(user: &User) -> Vec<Value> {
    user.call("notifications.list", json!({})).unwrap()["notifications"]
        .as_array()
        .unwrap()
        .clone()
}

/// A user with a private bus, a notification server that supports actions on it, a fake audio
/// sink and a root session with its first turn done, or `None` when this machine has no
/// `dbus-daemon`.
struct Desk {
    user: User,
    _bus: Bus,
    server: Notifications,
    played: PathBuf,
    root: String,
    mcp: Mcp,
}

fn desk(capabilities: &[&str]) -> Option<Desk> {
    let user = User::with_fake_agent();
    let Some(bus) = Bus::start(&user) else {
        return None;
    };
    let server = Notifications::start(&bus, capabilities);
    let played = desktop::with_fake_audio(&user);
    project(&user);
    let (root, mcp) = root(&user, "hello");
    Some(Desk {
        user,
        _bus: bus,
        server,
        played,
        root,
        mcp,
    })
}

#[test]
fn a_request_sends_a_desktop_notification_with_allow_and_deny() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };

    let number = held(&desk.mcp);

    let told = desk.server.wait_for(1);
    let notification = &told[0];
    assert_eq!(notification.app_name, "Flottille");
    assert_eq!(notification.app_icon, "flottille");
    assert_eq!(
        notification.summary,
        format!("Request #{number} waits for you")
    );
    assert!(notification.body.contains("Read .env"), "{notification:?}");
    assert!(notification.body.contains("app"), "{notification:?}");
    assert_eq!(notification.label_of("allow"), Some("Allow"));
    assert_eq!(notification.label_of("deny"), Some("Deny"));
    assert!(
        notification.label_of("default").is_some(),
        "{notification:?}"
    );
    assert_eq!(
        notification.hints.get("desktop-entry"),
        Some(&Some("flottille".to_owned()))
    );
    // The sound is Flottille's own, played by the daemon: the desktop plays none of its own.
    for hint in ["sound-file", "sound-name", "suppress-sound"] {
        assert!(!notification.hints.contains_key(hint), "{notification:?}");
    }
    assert_eq!(notification.replaces_id, 0);
}

#[test]
fn a_server_without_actions_gets_a_plain_notification() {
    let Some(desk) = desk(&["body"]) else {
        return;
    };

    held(&desk.mcp);

    let told = desk.server.wait_for(1);
    assert!(told[0].actions.is_empty(), "{:?}", told[0]);
}

#[test]
fn allow_from_the_notification_answers_the_request() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let number = held(&desk.mcp);
    let told = desk.server.wait_for(1);

    desk.server.invoke(told[0].id, "allow");

    eventually("the request was decided", || pending(&desk.user).is_empty());
    let row = rows(&desk.user)
        .into_iter()
        .find(|row| row["title"] == format!("Request #{number} waits for you"))
        .unwrap();
    assert_eq!(row["resolved"], true, "{row}");
}

#[test]
fn deny_from_the_notification_denies_the_request() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let number = held(&desk.mcp);
    let told = desk.server.wait_for(1);

    desk.server.invoke(told[0].id, "deny");

    eventually("the request was decided", || pending(&desk.user).is_empty());
    let row = rows(&desk.user)
        .into_iter()
        .find(|row| row["title"] == format!("Request #{number} waits for you"))
        .unwrap();
    assert_eq!(row["resolved"], true, "{row}");
}

#[test]
fn a_notification_answered_in_the_window_is_withdrawn() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let number = held(&desk.mcp);
    let told = desk.server.wait_for(1);

    desk.user
        .call("approval.approve", json!({"number": number}))
        .unwrap();

    desk.server.wait_closed(told[0].id);
}

#[test]
fn a_notification_read_elsewhere_is_withdrawn() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    held(&desk.mcp);
    let told = desk.server.wait_for(1);

    desk.user
        .call("notifications.read", json!({"session": desk.root}))
        .unwrap();

    desk.server.wait_closed(told[0].id);
}

#[test]
fn nothing_is_sent_for_the_session_open_in_the_focused_window() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let (_, other_mcp) = root(&desk.user, "hello again");
    let mut window = Window::open(&desk.user, true, Some(&desk.root));

    let seen = held(&desk.mcp);
    // The window looks elsewhere, and unfocused: what follows is told to the desktop, and it
    // comes after the first in the daemon's order, so that its coming says the first was not.
    window.tell(false, None);
    let unseen = held(&other_mcp);

    let told = desk.server.wait_for(1);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].summary, format!("Request #{unseen} waits for you"));
    assert!(told[0].summary != format!("Request #{seen} waits for you"));
    // Nothing rang for the session in sight, the sound neither.
    eventually("the sound of the second", || {
        desktop::played(&desk.played).len() == 1
    });
}

#[test]
fn with_the_window_focused_the_daemon_tells_it_and_sends_no_system_notification() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let (_, other_mcp) = root(&desk.user, "hello again");
    let watch = desk.user.watch(&[]);
    let mut window = Window::open(&desk.user, true, Some(&desk.root));

    let number = held(&other_mcp);

    let events = watch.until(|event| event["name"] == "notification.rang");
    let rang = &events.last().unwrap()["data"];
    assert_eq!(rang["in"], "window", "{rang}");
    assert_eq!(rang["kind"], "request", "{rang}");
    assert_eq!(rang["request"], number, "{rang}");
    assert_eq!(rang["title"], format!("Request #{number} waits for you"));
    assert_eq!(rang["excerpt"], "Read .env");
    assert_eq!(rang["needs_you"], true);
    // The window then goes away: the next is the desktop's.
    window.tell(false, None);
    let second = held(&desk.mcp);
    let told = desk.server.wait_for(1);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].summary, format!("Request #{second} waits for you"));
}

#[test]
fn with_the_window_unfocused_or_closed_it_is_a_system_notification() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let (_, other_mcp) = root(&desk.user, "hello again");
    let window = Window::open(&desk.user, false, Some(&desk.root));

    let first = held(&desk.mcp);
    let told = desk.server.wait_for(1);
    assert_eq!(told[0].summary, format!("Request #{first} waits for you"));

    drop(window);
    let second = held(&other_mcp);
    let told = desk.server.wait_for(2);
    assert_eq!(told[1].summary, format!("Request #{second} waits for you"));
}

#[test]
fn a_request_plays_the_needs_you_sound_once() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let (_, other_mcp) = root(&desk.user, "hello again");

    held(&desk.mcp);
    eventually("the sound", || !desktop::played(&desk.played).is_empty());
    // What follows (the notification read, another request) goes by the same keeper: the first
    // is not rung again.
    desk.user
        .call("notifications.read", json!({"session": desk.root}))
        .unwrap();
    held(&other_mcp);

    eventually("the second sound", || {
        desktop::played(&desk.played).len() >= 2
    });
    assert_eq!(
        desktop::played(&desk.played),
        ["needs-you", "needs-you"],
        "one sound for each request, once"
    );
}

#[test]
fn the_sound_plays_once_for_a_notification_the_window_shows_too() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let (_, other_mcp) = root(&desk.user, "hello again");
    let _window = Window::open(&desk.user, true, Some(&desk.root));

    held(&other_mcp);

    eventually("the sound", || !desktop::played(&desk.played).is_empty());
    held(&desk.mcp);
    held(&other_mcp);
    eventually("the others", || desktop::played(&desk.played).len() >= 2);
    // The one for the session in sight rang nothing: two sounds, for the two that were not.
    assert_eq!(desktop::played(&desk.played), ["needs-you", "needs-you"]);
    assert!(desk.server.notified().is_empty());
}

#[test]
fn without_an_audio_device_the_daemon_says_it_once() {
    let user = User::with_fake_agent();
    let Some(bus) = Bus::start(&user) else {
        return;
    };
    let _server = Notifications::start(&bus, &["actions", "body"]);
    project(&user);
    let (_, mcp) = root(&user, "hello");

    // No sound server answers in the user's own runtime folder: nothing is played, and the
    // daemon goes on with its notifications.
    held(&mcp);
    held(&mcp);
    _server.wait_for(2);

    let log = desktop::daemon_log(&user);
    assert_eq!(log.matches("no audio device").count(), 1, "{log}");
}

#[test]
fn the_default_action_opens_the_session() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    // The window, as `flottille window` listens for the commands that ask it to come forward.
    let window = UnixListener::bind(desk.user.path("runtime/flottille/window.sock")).unwrap();
    held(&desk.mcp);
    let told = desk.server.wait_for(1);

    desk.server.invoke(told[0].id, "default");

    let (stream, _) = window.accept().unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(stream), &mut line).unwrap();
    assert_eq!(line.trim(), format!("open {}", desk.root));
}

#[test]
fn without_a_notification_server_the_daemon_says_it_once() {
    let user = User::with_fake_agent();
    let Some(_bus) = Bus::start(&user) else {
        return;
    };
    let played = desktop::with_fake_audio(&user);
    project(&user);
    let (_, mcp) = root(&user, "hello");

    held(&mcp);
    held(&mcp);

    // The sound still plays, and the daemon went on: it says once that nothing shows.
    eventually("the sounds", || desktop::played(&played).len() == 2);
    let log = desktop::daemon_log(&user);
    assert_eq!(log.matches("no notification server").count(), 1, "{log}");
    let status = user.call("notifications.status", json!({})).unwrap();
    assert_eq!(status["desktop"]["shows"], false, "{status}");
    assert!(
        status["desktop"]["why"]
            .as_str()
            .unwrap()
            .contains("no notification server"),
        "{status}"
    );
}

#[test]
fn without_a_session_bus_the_daemon_says_it_once() {
    // The harness's bus is a path nobody listens on.
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, "hello");

    held(&mcp);
    held(&mcp);

    eventually("it said so", || {
        desktop::daemon_log(&user).contains("no session bus")
    });
    let log = desktop::daemon_log(&user);
    assert_eq!(log.matches("no session bus").count(), 1, "{log}");
    let status = user.call("notifications.status", json!({})).unwrap();
    assert_eq!(status["desktop"]["shows"], false, "{status}");
}

#[test]
fn a_secret_in_the_excerpt_is_masked() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };

    let path = "config/ghp_0123456789abcdefghijklmnopqrstuvwxyz01/.env";
    held_number(desk.mcp.call("fs_read", json!({"path": path})));

    let told = desk.server.wait_for(1);
    let said = format!("{} {}", told[0].summary, told[0].body);
    assert!(!said.contains("ghp_0123"), "{said}");
    assert!(
        said.contains("[masked]") || said.contains("config"),
        "{said}"
    );
}

#[test]
fn a_question_rings_too_and_what_only_tells_of_the_daemon_does_not() {
    let user = User::with_fake_agent();
    let Some(bus) = Bus::start(&user) else {
        return;
    };
    let server = Notifications::start(&bus, &["actions", "body"]);
    let played = desktop::with_fake_audio(&user);
    user.fake_script(json!({"turns": [
        {"when": "ask", "do": [
            {"mcp": {"tool": "ask_user", "arguments": {"question": "Which database?"}}},
            {"say": "Waiting."}]}
    ]}));
    project(&user);

    root(&user, "ask");

    let told = server.wait_for(1);
    assert_eq!(told[0].summary, "Asks you a question");
    assert!(told[0].body.contains("Which database?"), "{:?}", told[0]);
    eventually("its sound", || desktop::played(&played) == ["needs-you"]);
}
