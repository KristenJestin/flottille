//! The scenarios of #503: the user reloads a session's tools, and its agent's process restarts on
//! its conversation with the tools its role has now (`docs/tools.md`, "When the tools change").
//! Against the fake ACP agent, with throwaway repositories and temporary XDG folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository whose `origin` is GitHub's `acme/app`, with no
/// forge and no tracker declared.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

/// Starts a root in `app` with a first message: its id and the event number to follow it from.
fn run(user: &User, message: &str) -> (String, u64) {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        answer["after"].as_u64().unwrap(),
    )
}

/// The agent's events from `after`, up to the first one named `name`.
fn events_until(user: &User, agent: &str, after: u64, name: &str) -> Vec<Value> {
    let of = |event: &Value| event["data"]["agent"] == agent;
    user.watch(&["--after", &after.to_string()])
        .until(|e| of(e) && e["name"] == name)
        .into_iter()
        .filter(of)
        .collect()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn state(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"].clone()
}

/// The number of the agent's last event: what follows it is what a reload did.
fn last_event(user: &User, agent: &str) -> u64 {
    let shown = user.call("agent.show", json!({"agent": agent})).unwrap();
    shown["events"].as_array().unwrap().last().unwrap()["seq"]
        .as_u64()
        .unwrap()
}

/// The prompts the fake agent was sent, as text.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|prompt| prompt["params"]["prompt"].to_string())
        .collect()
}

/// The agent's process, between turns, is reloaded; the message sent after starts the new one.
fn reload_then_send(user: &User, id: &str) -> String {
    let after = last_event(user, id);
    let answer = user
        .call("agent.reload_tools", json!({"agent": id}))
        .unwrap();
    assert_eq!(answer["after_turn"], false, "{answer}");
    let released = events_until(user, id, after, "agent.released");
    assert_eq!(released.last().unwrap()["data"]["reason"], "reload");
    let sent = prompts(user).len();
    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    eventually("the message is sent", || prompts(user).len() > sent);
    prompts(user)[sent].clone()
}

#[test]
fn reload_tools_restarts_the_process_on_the_same_conversation() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "recall", "do": [{"recall": true}]}]}));
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    let native =
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["native_id"].clone();

    // The command line's own door, as the user types it.
    let out = user.run(&["agent", "reload-tools", &id, "--json"]);
    assert!(out.status.success(), "{out:?}");
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(answer["agent"]["id"], id.as_str());
    assert_eq!(answer["after_turn"], false);
    events_until(&user, &id, after, "agent.released");
    assert_eq!(state(&user, &id), "idle", "the session goes on");

    let sent = user
        .call("agent.send", json!({"agent": id, "message": "recall"}))
        .unwrap();
    let events = events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );
    let pids: Vec<u64> = user
        .received()
        .iter()
        .filter(|e| e["started"] == true)
        .map(|e| e["pid"].as_u64().unwrap())
        .collect();
    assert_eq!(pids.len(), 2, "a new process");
    assert_ne!(pids[0], pids[1]);
    let resumed = user.received_requests("session/resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["params"]["sessionId"], native, "its own session");
    let said: String = events
        .iter()
        .filter(|e| e["name"] == "agent.message")
        .filter_map(|e| e["data"]["text"].as_str())
        .collect();
    assert!(
        said.starts_with("earlier you said:") && said.contains("hello"),
        "the conversation went on: {said}"
    );
}

#[test]
fn a_reload_with_new_tools_sends_the_tools_delivery_with_what_changed() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    // The project gains a tracker: the role's list has `tickets` now.
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();

    let prompt = reload_then_send(&user, &id);

    assert!(prompt.contains("go on"), "with what woke it: {prompt}");
    assert!(prompt.contains("[flottille:tools]"), "{prompt}");
    assert!(prompt.contains("- tickets: new"), "{prompt}");
    assert!(!prompt.contains("nothing changed"), "{prompt}");
}

#[test]
fn a_reload_with_nothing_changed_says_so() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");

    let prompt = reload_then_send(&user, &id);

    assert!(prompt.contains("[flottille:tools]"), "{prompt}");
    assert!(
        prompt.contains("Your tools were reloaded: nothing changed"),
        "{prompt}"
    );
    // Said once: the process after holds the same list, and is told nothing.
    let sent = prompts(&user).len();
    user.call("agent.send", json!({"agent": id, "message": "and then"}))
        .unwrap();
    eventually("the message is sent", || prompts(&user).len() > sent);
    assert!(!prompts(&user)[sent].contains("[flottille:tools]"));
}

#[test]
fn a_reload_during_a_turn_waits_for_its_end() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "work", "do": [{"hold": "release"}, {"say": "Done."}]}
    ]}));
    project(&user);
    let (id, _) = run(&user, "work");
    eventually("the turn runs", || state(&user, &id) == "working");
    let after = last_event(&user, &id);

    let answer = user
        .call("agent.reload_tools", json!({"agent": id}))
        .unwrap();

    assert_eq!(answer["after_turn"], true, "{answer}");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(state(&user, &id), "working", "the turn is not cut");
    std::fs::write(user.path("fake/release"), "").unwrap();
    let events = events_until(&user, &id, after, "agent.released");
    let ended: Vec<&Value> = events
        .iter()
        .filter(|e| e["name"] == "agent.turn_ended")
        .collect();
    assert_eq!(ended.len(), 1, "released once the turn ended");
    assert_eq!(ended[0]["data"]["stop_reason"], "end_turn");
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "agent.message" && e["data"]["text"] == "Done.")
    );
    assert_eq!(events.last().unwrap()["data"]["reason"], "reload");
}

#[test]
fn an_ended_session_does_not_offer_reload_tools() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    user.call("agent.stop", json!({"agent": id})).unwrap();

    let refused = user
        .call("agent.reload_tools", json!({"agent": id}))
        .unwrap_err();

    assert_eq!(refused.0, "refused", "{refused:?}");
    let out = user.run(&["agent", "reload-tools", &id]);
    assert!(!out.status.success());
}
