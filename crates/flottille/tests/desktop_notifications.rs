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
pub(crate) fn project(user: &User) {
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
pub(crate) fn root(user: &User, message: &str) -> (String, Mcp) {
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

/// The state of request `number` of the tree of `root`: `pending`, `allowed`, `denied`.
fn state_of(user: &User, root: &str, number: u64) -> String {
    let requests = user.call("approval.list", json!({"tree": root})).unwrap()["requests"].clone();
    let request = requests
        .as_array()
        .unwrap()
        .iter()
        .find(|request| request["number"] == number)
        .unwrap_or_else(|| panic!("no request #{number}: {requests}"))
        .clone();
    request["state"].as_str().unwrap().to_owned()
}

/// What the gate said of request `number`, while it waits.
fn reasons_of(user: &User, number: u64) -> Vec<String> {
    let request = pending(user)
        .into_iter()
        .find(|request| request["number"] == number)
        .unwrap();
    request["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|reason| reason.as_str().unwrap().to_owned())
        .collect()
}

/// A read the gate holds whose path is too long to be said whole on one line: what the
/// notification says of it is cut.
fn long_path() -> String {
    format!("{}/.env", ["folder"; 24].join("/"))
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
    let bus = Bus::start(&user)?;
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
    // Allow is offered on what the user reads whole: the call and every reason the gate held
    // it for, uncut.
    let reasons = reasons_of(&desk.user, number);
    assert!(!reasons.is_empty());
    for reason in reasons {
        assert!(
            notification.body.contains(&reason),
            "{reason}: {notification:?}"
        );
    }
    assert!(!notification.body.contains('…'), "{notification:?}");
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
    assert_eq!(state_of(&desk.user, &desk.root, number), "allowed");
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
    assert_eq!(state_of(&desk.user, &desk.root, number), "denied");
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
    let watch = desk.user.watch(&[]);
    let _window = Window::open(&desk.user, true, Some(&desk.root));

    let seen = held(&desk.mcp);
    let unseen = held(&other_mcp);

    // The daemon follows the notifications in order: that the other session's rang says the
    // first was left alone, in the window and on the desktop alike.
    let events = watch
        .until(|event| event["name"] == "notification.rang" && event["data"]["request"] == unseen);
    let rang: Vec<&Value> = events
        .iter()
        .filter(|event| event["name"] == "notification.rang")
        .collect();
    assert_eq!(rang.len(), 1, "{rang:?}");
    assert_ne!(rang[0]["data"]["request"], seen);
    assert!(
        desk.server.notified().is_empty(),
        "{:?}",
        desk.server.notified()
    );
    // One sound, the other session's: the one in sight rang nothing.
    eventually("the sound", || !desktop::played(&desk.played).is_empty());
    assert_eq!(desktop::played(&desk.played), ["needs-you"]);
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

#[test]
fn a_request_too_long_to_read_whole_offers_deny_and_open_but_no_allow() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let number = held_number(desk.mcp.call("fs_read", json!({"path": long_path()})));

    let told = desk.server.wait_for(1);
    assert_eq!(told[0].label_of("allow"), None, "{:?}", told[0]);
    assert_eq!(told[0].label_of("deny"), Some("Deny"), "{:?}", told[0]);
    assert_eq!(told[0].label_of("default"), Some("Open"), "{:?}", told[0]);

    // An Allow the notification did not offer, said anyway, answers nothing.
    desk.server.invoke(told[0].id, "allow");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(state_of(&desk.user, &desk.root, number), "pending");
    desk.server.invoke(told[0].id, "deny");
    eventually("the request was denied", || {
        state_of(&desk.user, &desk.root, number) == "denied"
    });
}

#[test]
fn a_write_whose_content_the_notification_does_not_show_offers_no_allow() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    held_number(desk.mcp.call(
        "fs_write",
        json!({"path": ".env", "content": "TOKEN=whatever the danger is\n"}),
    ));

    let told = desk.server.wait_for(1);
    assert_eq!(told[0].label_of("allow"), None, "{:?}", told[0]);
    assert_eq!(told[0].label_of("deny"), Some("Deny"), "{:?}", told[0]);
}

#[test]
fn with_the_sandbox_gap_open_no_allow_is_offered() {
    // Where the kernel refuses user namespaces, a confined command reaches the session bus by its
    // path: what says it is the notification server may be a run's.
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_NO_USER_NAMESPACES", "1");
    let Some(bus) = Bus::start(&user) else {
        return;
    };
    let server = Notifications::start(&bus, &["actions", "body"]);
    desktop::with_fake_audio(&user);
    project(&user);
    let (_, mcp) = root(&user, "hello");

    held(&mcp);

    let told = server.wait_for(1);
    assert_eq!(told[0].label_of("allow"), None, "{:?}", told[0]);
    assert_eq!(told[0].label_of("deny"), Some("Deny"), "{:?}", told[0]);
}

#[test]
fn a_click_on_another_servers_notification_of_the_same_number_answers_nothing() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let number = held(&desk.mcp);
    let told = desk.server.wait_for(1);
    let Desk {
        user,
        _bus: bus,
        server,
        root,
        ..
    } = desk;

    // The server leaves; another takes the name, and numbers its own from 1 again.
    server.stop();
    let other = Notifications::start(&bus, &["actions", "body"]);
    other.invoke(told[0].id, "allow");
    other.invoke(told[0].id, "default");

    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(state_of(&user, &root, number), "pending");
}

#[test]
fn an_action_said_by_a_connection_that_is_not_the_server_is_ignored() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let number = held(&desk.mcp);
    let told = desk.server.wait_for(1);

    Notifications::forge(&desk._bus, told[0].id, "allow");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(state_of(&desk.user, &desk.root, number), "pending");

    // The server's own is heard.
    desk.server.invoke(told[0].id, "deny");
    eventually("the request was denied", || {
        state_of(&desk.user, &desk.root, number) == "denied"
    });
}

#[test]
fn a_notification_server_that_never_answers_holds_nothing_up() {
    let user = User::with_fake_agent();
    let Some(bus) = Bus::start(&user) else {
        return;
    };
    let _server = Notifications::hung(&bus);
    let played = desktop::with_fake_audio(&user);
    project(&user);
    let (_, mcp) = root(&user, "hello");

    held(&mcp);
    held(&mcp);

    // Each rings in turn: the second's sound plays although the desktop never answered the
    // first.
    let deadline = Instant::now() + Duration::from_secs(40);
    while desktop::played(&played).len() < 2 {
        assert!(Instant::now() < deadline, "{:?}", desktop::played(&played));
        std::thread::sleep(Duration::from_millis(50));
    }
    let log = desktop::daemon_log(&user);
    assert_eq!(log.matches("no notification server").count(), 1, "{log}");
}

#[test]
fn a_sound_server_that_stalls_holds_no_later_sound_up() {
    let user = User::with_fake_agent();
    let pulse = desktop::Pulse::start(&user, true);
    project(&user);
    let (_, mcp) = root(&user, "hello");

    held(&mcp);
    pulse.wait_played(1);
    // The first sound never ends; a later one still plays.
    std::thread::sleep(Duration::from_secs(1));
    held(&mcp);
    let deadline = Instant::now() + Duration::from_secs(40);
    while pulse.heard().played < 2 {
        assert!(Instant::now() < deadline, "{:?}", pulse.heard());
        held(&mcp);
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[test]
fn the_sounds_go_to_the_sound_server_on_one_connection_and_the_daemon_rests_after() {
    let user = User::with_fake_agent();
    let pulse = desktop::Pulse::start(&user, false);
    project(&user);
    let (_, mcp) = root(&user, "hello");
    let pid = user.status()["pid"].as_u64().unwrap();

    held(&mcp);
    let first = pulse.wait_played(1);
    assert!(first.bytes > 4_000, "the samples of the sound: {first:?}");
    std::thread::sleep(Duration::from_millis(300));
    let threads = desktop::threads(pid);
    for count in 2..=4 {
        held(&mcp);
        pulse.wait_played(count);
    }
    std::thread::sleep(Duration::from_millis(300));

    let heard = pulse.heard();
    assert_eq!(heard.clients, 1, "one client for every sound: {heard:?}");
    assert_eq!(
        desktop::threads(pid),
        threads,
        "no thread left behind by a sound"
    );
}

#[test]
fn a_refused_answer_from_the_notification_is_told() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let number = held(&desk.mcp);
    let told = desk.server.wait_for(1);

    // A second click, before the first one's answer withdrew the notification: the request is
    // decided already, and the daemon says why nothing more was done.
    desk.server.invoke(told[0].id, "deny");
    desk.server.invoke(told[0].id, "deny");

    let all = desk.server.wait_for(2);
    let said = &all[1];
    assert_eq!(
        said.summary,
        format!("Request #{number} was not answered"),
        "{said:?}"
    );
    assert!(said.body.contains("already"), "{said:?}");
    assert!(
        desktop::daemon_log(&desk.user).contains(&format!("request #{number}")),
        "{}",
        desktop::daemon_log(&desk.user)
    );
}

#[test]
fn a_focus_that_is_not_told_again_counts_as_away() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    let Some(bus) = Bus::start(&user) else {
        return;
    };
    let server = Notifications::start(&bus, &["actions", "body"]);
    desktop::with_fake_audio(&user);
    project(&user);
    let (_, mcp) = root(&user, "hello");
    // A window that said it was focused, then hung: it never says it again.
    let _window = Window::open(&user, true, None);
    user.call("test.advance", json!({"ms": 120_000})).unwrap();

    let number = held(&mcp);

    let told = server.wait_for(1);
    assert_eq!(told[0].summary, format!("Request #{number} waits for you"));
}

#[test]
fn a_toast_the_window_could_not_show_goes_to_the_desktop() {
    let Some(desk) = desk(&["actions", "body"]) else {
        return;
    };
    let watch = desk.user.watch(&[]);
    let _window = Window::open(&desk.user, true, None);
    let number = held(&desk.mcp);
    let events = watch.until(|event| event["name"] == "notification.rang");
    let id = events.last().unwrap()["data"]["id"].clone();
    assert!(desk.server.notified().is_empty());

    // The window lost the focus before it drew the toast: it says so.
    desk.user
        .call("notifications.unshown", json!({"id": id}))
        .unwrap();

    let told = desk.server.wait_for(1);
    assert_eq!(told[0].summary, format!("Request #{number} waits for you"));
    // Its sound played once, with the toast that was not drawn.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(desktop::played(&desk.played), ["needs-you"]);
}

#[test]
fn the_session_and_its_brief_are_masked_with_the_daemons_secrets() {
    let user = User::with_fake_agent().with_env("SHOP_API_TOKEN", "plainvalue42");
    let Some(bus) = Bus::start(&user) else {
        return;
    };
    let server = Notifications::start(&bus, &["actions", "body"]);
    desktop::with_fake_audio(&user);
    project(&user);
    let (_, mcp) = root(&user, "deploy with plainvalue42 now");

    held(&mcp);

    let told = server.wait_for(1);
    let said = format!("{} {}", told[0].summary, told[0].body);
    assert!(!said.contains("plainvalue42"), "{said}");
}
