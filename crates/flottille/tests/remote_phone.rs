//! The scenarios of #827: the phone is told only while the user is away from a client
//! (`docs/remote.md`, "Notifications"). A hub served in the test on 127.0.0.1, the runner `server`
//! (its agents the fake ACP agent, its clock moved by `test.advance`), and a phone: a device paired
//! with the hub as the app pairs it, whose session keeps what the hub tells it. No push service,
//! no real agent, temporary folders.

use crate::common;
use crate::remote;

use std::time::Duration;

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_link::{Notice, NoticeKind};
use remote::{Machine, Phone, TestHub, eventually, key_of, pair};
use serde_json::{Value, json};

/// How long a test looks for a notice that must not come.
const QUIET: Duration = Duration::from_secs(3);

/// The runner `server`, paired with `hub`, its clock moved by the test, its session to the hub
/// open; and a phone paired with the same hub.
fn server_and_phone() -> (TestHub, Machine, Phone) {
    let hub = TestHub::new();
    let server = Machine::of(User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1"));
    pair(&hub, &server, "server", "runner");
    connected(&server);
    let phone = Phone::pair(&hub, "pixel");
    (hub, server, phone)
}

/// Until `machine`'s session to its hub is open.
fn connected(machine: &Machine) {
    machine.wait_until("the hub connected", |seen| {
        seen["hub"]["state"] == "connected"
    });
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
fn root(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || state(user, &id) == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

/// The number of the request a held call answered with.
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

/// A root of `app` on `server` whose call the gate holds for the user: the root's id, the
/// request's number and its notification's row.
fn held_request(server: &Machine) -> (String, u64, Value) {
    project(&server.user);
    let (id, mcp) = root(&server.user);
    let number = held_number(mcp.call("fs_read", json!({"path": ".env"})));
    let row = row_titled(server, &format!("Request #{number} waits for you"));
    (id, number, row)
}

/// The notification a title says, once the daemon wrote it.
fn row_titled(machine: &Machine, title: &str) -> Value {
    let found = std::cell::RefCell::new(None);
    eventually(title, || {
        *found.borrow_mut() = rows(machine).into_iter().find(|row| row["title"] == title);
        found.borrow().is_some()
    });
    found.into_inner().unwrap()
}

fn rows(machine: &Machine) -> Vec<Value> {
    machine.user.call("notifications.list", json!({})).unwrap()["notifications"]
        .as_array()
        .unwrap()
        .clone()
}

fn advance(machine: &Machine, ms: u64) {
    machine
        .user
        .call("test.advance", json!({"ms": ms}))
        .unwrap();
}

/// What the window says when the user types or clicks in it.
fn input(machine: &Machine) {
    machine
        .user
        .call("client.presence", json!({"focused": true, "input": true}))
        .unwrap();
}

fn notice(kind: NoticeKind, row: &Value, session: &str) -> Notice {
    Notice {
        kind,
        notification: row["id"].as_u64().unwrap(),
        session: Some(session.parse().unwrap()),
    }
}

#[test]
fn a_request_reaches_the_phone_after_thirty_seconds_when_no_client_is_active() {
    let (_hub, server, phone) = server_and_phone();
    let (id, _, row) = held_request(&server);

    advance(&server, 29_000);
    assert!(
        phone.told_nothing_more(0, QUIET),
        "told before 30 s: {:?}",
        phone.told()
    );

    advance(&server, 1_000);
    let told = phone.wait_told(1);
    assert_eq!(told.len(), 1, "{told:?}");
    let (from, said) = &told[0];
    assert_eq!(*from, key_of(&server), "from the server's machine");
    assert_eq!(*said, notice(NoticeKind::Request, &row, &id));
}

#[test]
fn nothing_reaches_the_phone_while_the_window_had_an_input_in_the_last_two_minutes() {
    let (_hub, server, phone) = server_and_phone();
    input(&server);
    held_request(&server);

    advance(&server, 30_000);
    assert!(phone.told_nothing_more(0, QUIET), "{:?}", phone.told());
    // Decided once, at 30 s: it does not ring later either.
    advance(&server, 3 * 60_000);
    assert!(phone.told_nothing_more(0, QUIET), "{:?}", phone.told());
}

#[test]
fn an_input_older_than_two_minutes_lets_the_phone_be_told() {
    let (_hub, server, phone) = server_and_phone();
    input(&server);
    advance(&server, 2 * 60_000 + 1_000);
    let (id, _, row) = held_request(&server);

    advance(&server, 30_000);
    let told = phone.wait_told(1);
    assert_eq!(told[0].1, notice(NoticeKind::Request, &row, &id));
}

#[test]
fn presence_at_a_client_of_another_machine_stops_the_notice_through_the_hub() {
    let (hub, server, phone) = server_and_phone();
    let pc = Machine::new();
    pair(&hub, &pc, "pc", "runner");
    connected(&pc);

    // The window of the pc, not the server's: the hub tells the server.
    input(&pc);
    let member = pc.device().public();
    eventually("the pc's presence on the hub", || {
        hub.with_hub(|hub| hub.presence(&member)).is_some()
    });
    // The hub tells the runners at once; a moment for the server to read it.
    std::thread::sleep(Duration::from_secs(1));
    held_request(&server);

    advance(&server, 30_000);
    assert!(phone.told_nothing_more(0, QUIET), "{:?}", phone.told());
}

#[test]
fn a_request_answered_within_the_delay_never_reaches_the_phone() {
    let (_hub, server, phone) = server_and_phone();
    let (_, number, row) = held_request(&server);

    advance(&server, 10_000);
    let output = server.user.run(&["deny", &number.to_string()]);
    assert!(output.status.success(), "{output:?}");
    eventually("the request resolved", || {
        rows(&server)
            .iter()
            .any(|r| r["id"] == row["id"] && r["resolved"] == true)
    });

    advance(&server, 30_000);
    assert!(phone.told_nothing_more(0, QUIET), "{:?}", phone.told());
}

#[test]
fn a_notice_holds_only_a_kind_and_ids() {
    let (_hub, server, phone) = server_and_phone();
    let (id, _, row) = held_request(&server);
    advance(&server, 30_000);

    let (from, said) = phone.wait_told(1).remove(0);

    let frame = serde_json::to_value(&said).unwrap();
    let mut keys: Vec<&str> = frame
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["kind", "notification", "session"], "{frame}");
    assert_eq!(frame["kind"], "request");
    assert_eq!(frame["notification"], row["id"]);
    assert_eq!(frame["session"], id.as_str());
    // Nothing of what the row says: neither its title, its excerpt (the path), nor its project.
    let whole = format!("{frame} {}", serde_json::to_string(&from).unwrap());
    for said in ["Request", "waits", ".env", "Read", "app"] {
        assert!(!whole.contains(said), "{said} in {whole}");
    }
}

#[test]
fn answering_elsewhere_sends_dismissed() {
    let (_hub, server, phone) = server_and_phone();
    let (id, number, row) = held_request(&server);
    advance(&server, 30_000);
    phone.wait_told(1);

    let output = server.user.run(&["deny", &number.to_string()]);
    assert!(output.status.success(), "{output:?}");

    let told = phone.wait_told(2);
    assert_eq!(told[1].0, key_of(&server));
    assert_eq!(told[1].1, notice(NoticeKind::Dismissed, &row, &id));
    assert!(phone.told_nothing_more(2, QUIET), "{told:?}");
}

#[test]
fn seeing_it_at_a_client_sends_dismissed() {
    let (_hub, server, phone) = server_and_phone();
    let (id, _, row) = held_request(&server);
    advance(&server, 30_000);
    phone.wait_told(1);

    // The session opened in a window: its notifications are read.
    server
        .user
        .call("notifications.read", json!({"session": id}))
        .unwrap();

    let told = phone.wait_told(2);
    assert_eq!(told[1].1, notice(NoticeKind::Dismissed, &row, &id));
}

#[test]
fn always_notifies_even_while_active() {
    let (_hub, server, phone) = server_and_phone();
    let set = server
        .user
        .call("remote.configure", json!({"notify": "always"}))
        .unwrap();
    assert_eq!(set["notify"], "always", "{set}");
    input(&server);
    let (id, _, row) = held_request(&server);

    advance(&server, 30_000);
    let told = phone.wait_told(1);
    assert_eq!(told[0].1, notice(NoticeKind::Request, &row, &id));
}

#[test]
fn notify_the_phone_is_when_i_am_away_by_default_and_kept_in_daemon_toml() {
    let user = User::new();
    let set = user.call("remote.settings", json!({})).unwrap();
    assert_eq!(set["notify"], "away", "{set}");

    let set = user
        .call("remote.configure", json!({"notify": "always"}))
        .unwrap();
    assert_eq!(set["notify"], "always", "{set}");
    let file = std::fs::read_to_string(user.path("config/flottille/daemon.toml")).unwrap();
    assert!(file.contains("notify = \"always\""), "{file}");

    let refused = user
        .call("remote.configure", json!({"notify": "sometimes"}))
        .unwrap_err();
    assert_eq!(refused.0, "invalid-params", "{refused:?}");
    let set = user
        .call("remote.configure", json!({"notify": "away"}))
        .unwrap();
    assert_eq!(set["notify"], "away", "{set}");
}
