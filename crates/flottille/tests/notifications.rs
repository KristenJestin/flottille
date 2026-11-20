//! The notification centre the daemon keeps (#1001): one list of what the user is told, which
//! every client reads and marks. The fake ACP agent, temporary folders, no real agent.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

const DAY: u64 = 24 * 3_600_000;

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

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
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

/// The request a call the gate holds made, as its notification.
fn held(user: &User, mcp: &Mcp) -> u64 {
    let _ = user;
    held_number(mcp.call("fs_read", json!({"path": ".env"})))
}

fn listed(user: &User, params: Value) -> Value {
    user.call("notifications.list", params).unwrap()
}

fn rows(user: &User, params: Value) -> Vec<Value> {
    listed(user, params)["notifications"]
        .as_array()
        .unwrap()
        .clone()
}

/// The notification a title says, once the daemon wrote it.
fn titled(user: &User, title: &str) -> Value {
    let mut found = None;
    eventually(title, || {
        found = rows(user, json!({}))
            .into_iter()
            .find(|row| row["title"] == title);
        found.is_some()
    });
    found.unwrap()
}

/// A root that asks the user one question as soon as it is told `ask`.
fn asking(user: &User) -> (String, Mcp) {
    user.fake_script(json!({"turns": [
        {"when": "ask", "do": [
            {"mcp": {"tool": "ask_user", "arguments": {"question": "Which database?"}}},
            {"say": "Waiting."}]},
        {"when": "Postgres", "do": [{"say": "Postgres it is."}]}
    ]}));
    project(user);
    root(user, "ask")
}

#[test]
fn a_held_request_is_one_unread_notification_that_says_why() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, mcp) = root(&user, "hello");

    let number = held(&user, &mcp);

    let row = titled(&user, &format!("Request #{number} waits for you"));
    assert_eq!(row["kind"], "request", "{row}");
    assert_eq!(row["session"], id, "{row}");
    assert_eq!(row["project"], "app", "{row}");
    assert_eq!(row["workspace"], Value::Null, "{row}");
    assert_eq!(row["excerpt"], "Read .env", "{row}");
    assert_eq!(row["read_at"], Value::Null, "{row}");
    assert_eq!(row["resolved"], false, "{row}");
    assert_eq!(row["needs_you"], true, "{row}");
    assert!(row["at"].as_u64().unwrap() > 0, "{row}");
    assert!(row["id"].as_u64().unwrap() > 0, "{row}");
    let answer = listed(&user, json!({"unread": true}));
    assert_eq!(
        answer["unread"],
        json!({"needs_you": 1, "new": 0}),
        "{answer}"
    );
}

#[test]
fn an_excerpt_is_one_line_cut_and_masked_never_a_whole_call() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, "hello");

    let line = format!(
        "echo ghp_0123456789abcdefghijklmnopqrstuvwxyz01 {}",
        "and a long argument ".repeat(30)
    );
    let number = held_number(mcp.call("run", json!({"line": line, "timeout": 5})));

    let row = titled(&user, &format!("Request #{number} waits for you"));
    let excerpt = row["excerpt"].as_str().unwrap();
    assert!(!excerpt.contains("ghp_0123"), "{excerpt}");
    assert!(!excerpt.contains('\n'), "{excerpt}");
    assert!(excerpt.chars().count() <= 121, "{excerpt}");
    assert!(excerpt.ends_with('…'), "{excerpt}");
}

#[test]
fn opening_the_session_marks_its_notifications_read() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, mcp) = root(&user, "hello");
    let (other, other_mcp) = root(&user, "hello again");
    let first = held(&user, &mcp);
    let second = held(&user, &other_mcp);
    titled(&user, &format!("Request #{first} waits for you"));
    titled(&user, &format!("Request #{second} waits for you"));
    let watch = user.watch(&[]);

    let read = user
        .call("notifications.read", json!({"session": id}))
        .unwrap();

    assert_eq!(read["read"].as_array().unwrap().len(), 1, "{read}");
    let ours = rows(&user, json!({}))
        .into_iter()
        .find(|row| row["session"] == id)
        .unwrap();
    assert!(ours["read_at"].as_u64().unwrap() > 0, "{ours}");
    let theirs = rows(&user, json!({}))
        .into_iter()
        .find(|row| row["session"] == other)
        .unwrap();
    assert_eq!(theirs["read_at"], Value::Null, "{theirs}");
    // A request waits until it is answered: reading it does not answer it.
    assert_eq!(ours["resolved"], false, "{ours}");
    let seen = watch.until(|event| event["name"] == "notifications.changed");
    assert_eq!(
        seen.last().unwrap()["data"]["ids"],
        json!([ours["id"]]),
        "{seen:?}"
    );
}

#[test]
fn a_request_answered_elsewhere_is_resolved_in_the_centre() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, "hello");
    let number = held(&user, &mcp);
    let row = titled(&user, &format!("Request #{number} waits for you"));
    assert_eq!(row["resolved"], false);

    // From the command line, not from a window.
    let output = user.run(&["deny", &number.to_string()]);
    assert!(output.status.success(), "{output:?}");

    eventually("the request resolved", || {
        rows(&user, json!({}))
            .iter()
            .any(|r| r["id"] == row["id"] && r["resolved"] == true)
    });
    let row = rows(&user, json!({}))
        .into_iter()
        .find(|r| r["id"] == row["id"])
        .unwrap();
    assert!(
        row["read_at"].as_u64().unwrap() > 0,
        "answered is read: {row}"
    );
    let answer = listed(&user, json!({"unread": true}));
    assert_eq!(
        answer["unread"],
        json!({"needs_you": 0, "new": 0}),
        "{answer}"
    );
}

#[test]
fn a_question_answered_is_resolved_and_read() {
    let user = User::with_fake_agent();
    let (id, _) = asking(&user);
    let row = titled(&user, "Asks you a question");
    assert_eq!(row["kind"], "question", "{row}");
    assert_eq!(row["session"], id, "{row}");
    assert_eq!(row["excerpt"], "Which database?", "{row}");
    assert_eq!(row["resolved"], false, "{row}");

    user.call("agent.send", json!({"agent": id, "message": "Postgres."}))
        .unwrap();

    eventually("the question resolved", || {
        rows(&user, json!({}))
            .iter()
            .any(|r| r["id"] == row["id"] && r["resolved"] == true && r["read_at"].is_number())
    });
}

#[test]
fn the_centre_groups_by_session_unread_first() {
    let user = User::with_fake_agent();
    project(&user);
    let (first, first_mcp) = root(&user, "first");
    let (second, second_mcp) = root(&user, "second");
    let one = held(&user, &first_mcp);
    let two = held(&user, &second_mcp);
    let three = held(&user, &first_mcp);
    for number in [one, two, three] {
        titled(&user, &format!("Request #{number} waits for you"));
    }
    user.call("notifications.read", json!({"session": first}))
        .unwrap();

    let all = rows(&user, json!({}));

    // The daemon says each notification's session; a client groups. What it keeps in order is
    // the time: oldest first, so a client reads `after` the last id it saw.
    let ids: Vec<u64> = all.iter().map(|row| row["id"].as_u64().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "{all:?}");
    let sessions: Vec<&Value> = all.iter().map(|row| &row["session"]).collect();
    assert_eq!(sessions, [&json!(first), &json!(second), &json!(first)]);
    let unread = rows(&user, json!({"unread": true}));
    assert_eq!(unread.len(), 1, "{unread:?}");
    assert_eq!(unread[0]["session"], second, "{unread:?}");
    // `after` reads on from the last id seen, and `limit` cuts.
    let newer = rows(&user, json!({"after": ids[0], "limit": 1}));
    assert_eq!(newer.len(), 1);
    assert_eq!(newer[0]["id"], ids[1]);
}

#[test]
fn mark_all_read_empties_the_count() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, first) = root(&user, "first");
    let (_, second) = root(&user, "second");
    let one = held(&user, &first);
    let two = held(&user, &second);
    titled(&user, &format!("Request #{one} waits for you"));
    titled(&user, &format!("Request #{two} waits for you"));
    assert_eq!(
        listed(&user, json!({}))["unread"],
        json!({"needs_you": 2, "new": 0})
    );

    let read = user
        .call("notifications.read", json!({"all": true}))
        .unwrap();

    assert_eq!(read["read"].as_array().unwrap().len(), 2, "{read}");
    let answer = listed(&user, json!({}));
    assert_eq!(
        answer["unread"],
        json!({"needs_you": 0, "new": 0}),
        "{answer}"
    );
    assert!(rows(&user, json!({"unread": true})).is_empty());
    // Everything stays in the centre, read.
    assert_eq!(answer["notifications"].as_array().unwrap().len(), 2);
}

#[test]
fn two_clients_see_the_same_read_state() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, "hello");
    let number = held(&user, &mcp);
    let row = titled(&user, &format!("Request #{number} waits for you"));
    let id = row["id"].as_u64().unwrap();

    // One client marks it by its id, the command line.
    let output = user.run(&["notifications", "read", &id.to_string()]);
    assert!(output.status.success(), "{output:?}");

    // The other, the protocol, sees it read; so does the command line's own list.
    let seen = rows(&user, json!({}));
    assert!(seen[0]["read_at"].is_number(), "{seen:?}");
    let output = user.run(&["notifications", "--unread", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed["notifications"], json!([]), "{listed}");
    let output = user.run(&["notifications", "--json"]);
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed["notifications"][0]["id"], id, "{listed}");
}

#[test]
fn the_command_line_lists_and_marks_every_notification_read() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, "hello");
    let number = held(&user, &mcp);
    titled(&user, &format!("Request #{number} waits for you"));

    let output = user.run(&["notifications"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains(&format!("Request #{number} waits for you")),
        "{text}"
    );
    assert!(text.contains("Read .env"), "{text}");
    assert!(text.contains("1 need you"), "{text}");

    let output = user.run(&["notifications", "read", "--all"]);
    assert!(output.status.success(), "{output:?}");
    let output = user.run(&["notifications", "--unread"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Nothing is new."), "{text}");
}

#[test]
fn notifications_older_than_thirty_days_are_dropped() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    project(&user);
    let (_, mcp) = root(&user, "hello");
    let number = held(&user, &mcp);
    titled(&user, &format!("Request #{number} waits for you"));

    user.call("test.advance", json!({"ms": 29 * DAY})).unwrap();
    assert_eq!(rows(&user, json!({})).len(), 1, "29 days old is kept");

    user.call("test.advance", json!({"ms": 2 * DAY})).unwrap();
    assert!(rows(&user, json!({})).is_empty());
}

#[test]
fn the_inbox_stays_a_view_of_the_stored_events() {
    let user = User::with_fake_agent();
    let (id, _) = asking(&user);
    titled(&user, "Asks you a question");

    user.call("notifications.read", json!({"all": true}))
        .unwrap();

    // Marking read never touches the inbox: it is the agents' view, nothing is marked there.
    let inbox = user.call("inbox.list", json!({})).unwrap();
    let entries = inbox["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e["kind"] == "question" && e["agent"] == id),
        "{inbox}"
    );
    assert!(
        entries.iter().all(|e| e.get("read_at").is_none()),
        "{inbox}"
    );
}

#[test]
fn reading_names_what_it_reads_one_way_only() {
    let user = User::with_fake_agent();
    for params in [
        json!({}),
        json!({"all": true, "session": "x"}),
        json!({"ids": [1], "all": true}),
        json!({"all": false}),
    ] {
        let refused = user.call("notifications.read", params.clone());
        assert!(refused.is_err(), "{params} is refused: {refused:?}");
    }
    assert!(user.call("notifications.read", json!({"ids": []})).is_ok());
}
