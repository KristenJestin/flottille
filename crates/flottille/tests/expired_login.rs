//! A turn that fails for a reason that leaves the agent's session usable (an expired login) does
//! not end the session, and a failed root whose session is known can be resumed (#631). Against
//! the fake agent, scripted to answer a prompt with Claude's own sign-in error.

mod common;

use common::User;
use serde_json::{Value, json};

/// What Claude Code says when its login expired, as the bridge fails the prompt with it.
const EXPIRED: &str = "Failed to authenticate: OAuth session expired and could not be refreshed";

/// What the session's state line says then.
const LOGIN_LINE: &str =
    "Claude's login expired: sign in again (run claude and /login), then send your message again";

fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

fn run(user: &User, message: &str) -> (String, u64) {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        answer["after"].as_u64().unwrap(),
    )
}

/// The agent's events from `after`, up to the first one named one of `names`.
fn events_until(user: &User, agent: &str, after: u64, names: &[&str]) -> Vec<Value> {
    let of = |event: &Value| event["data"]["agent"] == agent;
    user.watch(&["--after", &after.to_string()])
        .until(|e| of(e) && names.iter().any(|name| e["name"] == *name))
        .into_iter()
        .filter(of)
        .collect()
}

fn show(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone()
}

fn in_short(user: &User, agent: &str) -> Value {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == agent)
        .cloned()
        .unwrap()
}

fn said(events: &[Value]) -> String {
    events
        .iter()
        .filter(|e| e["name"] == "agent.message")
        .filter_map(|e| e["data"]["text"].as_str())
        .collect()
}

#[test]
fn an_expired_login_fails_the_turn_without_ending_the_session() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "hello", "do": [{"say": "Looking"}], "error": EXPIRED}
    ]}));
    project(&user);
    let (id, after) = run(&user, "hello");

    let events = events_until(&user, &id, after, &["agent.released", "agent.failed"]);

    let names: Vec<&str> = events.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(!names.contains(&"agent.failed"), "{names:?}");
    let ended = events
        .iter()
        .find(|e| e["name"] == "agent.turn_ended")
        .unwrap_or_else(|| panic!("the turn ends: {names:?}"));
    assert_eq!(ended["data"]["stop_reason"], "failed");
    let released = events.last().unwrap();
    assert_eq!(released["name"], "agent.released");
    assert_eq!(released["data"]["reason"], "failed");
    let agent = show(&user, &id);
    assert_eq!(agent["state"], "idle", "the session stays");
    assert_eq!(agent["state_reason"], LOGIN_LINE);
    assert_eq!(agent["ended_at"], Value::Null);
    let listed = in_short(&user, &id);
    assert_eq!(listed["state"], "idle", "it stays in the sidebar");
    assert_eq!(listed["state_reason"], LOGIN_LINE);
}

#[test]
fn the_next_message_after_an_expired_login_resumes_the_same_session() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "hello", "error": EXPIRED, "times": 1},
        {"when": "recall", "do": [{"recall": true}]}
    ]}));
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, &["agent.released", "agent.failed"]);
    let before = show(&user, &id);
    assert_eq!(before["state"], "idle");

    let sent = user
        .call("agent.send", json!({"agent": id, "message": "recall"}))
        .unwrap();

    let events = events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        &["agent.turn_ended", "agent.failed"],
    );
    let started = events
        .iter()
        .find(|e| e["name"] == "agent.started")
        .unwrap();
    assert_eq!(started["data"]["opened"], "resumed");
    let resumes = user.received_requests("session/resume");
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0]["params"]["sessionId"], before["native_id"]);
    assert_eq!(user.received_requests("session/new").len(), 1);
    let text = said(&events);
    assert!(
        text.starts_with("earlier you said:") && text.contains("hello"),
        "the same conversation: {text}"
    );
    let agent = show(&user, &id);
    assert_eq!(agent["epoch"], before["epoch"], "not a replacement");
    assert_eq!(agent["native_id"], before["native_id"]);
    assert_eq!(agent["state"], "idle");
    assert_eq!(agent["state_reason"], Value::Null, "the login line goes");
}

#[test]
fn a_failed_root_whose_session_is_known_can_be_resumed() {
    let user = User::with_fake_agent();
    // A reason no recogniser knows still fails the root, as every login did before.
    user.fake_script(json!({"turns": [
        {"when": "hello", "error": "something nobody knows broke", "times": 1},
        {"when": "recall", "do": [{"recall": true}]}
    ]}));
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, &["agent.failed"]);
    let before = show(&user, &id);
    assert_eq!(before["state"], "failed");
    assert!(before["native_id"].is_string(), "its session is known");
    let tree = user.call("agent.tree", json!({"agent": id})).unwrap();
    assert_eq!(tree["agents"][0]["agent"]["resumable"], true);
    assert_eq!(in_short(&user, &id)["resumable"], true);

    let resumed = user.resume(&id);

    assert_eq!(resumed["agent"]["state"], "idle");
    assert_eq!(resumed["agent"]["state_reason"], Value::Null);
    let sent = user
        .call("agent.send", json!({"agent": id, "message": "recall"}))
        .unwrap();
    let events = events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        &["agent.turn_ended", "agent.failed"],
    );
    let started = events
        .iter()
        .find(|e| e["name"] == "agent.started")
        .unwrap();
    assert_eq!(started["data"]["opened"], "resumed");
    assert_eq!(
        user.received_requests("session/resume")[0]["params"]["sessionId"],
        before["native_id"]
    );
    let text = said(&events);
    assert!(text.contains("hello"), "the same conversation: {text}");
    let prompts = user.received_requests("session/prompt");
    let told: String = prompts.last().unwrap()["params"]["prompt"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(told.contains("failed"), "it reads that it failed: {told}");
}

#[test]
fn a_session_that_cannot_start_at_all_still_ends() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"open_error": EXPIRED}));
    project(&user);
    let (id, after) = run(&user, "hello");

    let events = events_until(&user, &id, after, &["agent.failed", "agent.released"]);

    assert_eq!(events.last().unwrap()["name"], "agent.failed");
    let agent = show(&user, &id);
    assert_eq!(agent["state"], "failed");
    assert_eq!(agent["native_id"], Value::Null);
    assert_eq!(
        in_short(&user, &id)["resumable"],
        false,
        "no session to resume"
    );
}
