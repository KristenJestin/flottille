//! The scenarios of `wake` (`docs/tools.md`, "Waking later"): an agent asks to be woken in a few
//! minutes with its reason, and a delivery wakes it then, a subscription whose source is a timer.
//! The fake ACP agent, temporary folders, and the clock the test moves: nobody waits for real.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

fn user() -> User {
    User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1")
}

/// A project `app` over a throwaway repository, and an agent of it whose first turn ended: its id
/// and its MCP session.
fn agent(user: &User) -> (String, Mcp) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || state(user, &id) == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

fn state(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wake(mcp: &Mcp, args: Value) -> String {
    let answer: Answer = mcp.call("wake", args);
    assert!(!answer.error, "{}", answer.text);
    answer.text
}

fn refused(mcp: &Mcp, args: Value) -> String {
    let answer: Answer = mcp.call("wake", args);
    assert!(answer.error, "not refused: {}", answer.text);
    answer.text
}

/// The wakes the agent was sent, oldest first.
fn woken(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .filter_map(|p| p["params"]["prompt"][0]["text"].as_str().map(str::to_owned))
        .filter(|p| p.contains("[flottille:subscription]"))
        .collect()
}

fn minutes(user: &User, minutes: u64) {
    user.call("test.advance", json!({"ms": minutes * 60_000}))
        .unwrap();
}

/// The agent's open subscriptions, as the window lists them.
fn listed(user: &User, agent: &str) -> Vec<Value> {
    user.call("subscription.list", json!({"tree": agent}))
        .unwrap()["subscriptions"]
        .as_array()
        .unwrap()
        .clone()
}

/// Nothing comes: the sweep had its chance, and the agent was not woken.
fn nothing_woke(user: &User) {
    std::thread::sleep(Duration::from_millis(500));
    assert!(woken(user).is_empty(), "{:?}", woken(user));
}

#[test]
fn an_agent_asks_a_wake_in_20_minutes_and_is_woken_then_with_its_why() {
    let user = user();
    let (id, mcp) = agent(&user);

    let answer = wake(
        &mcp,
        json!({"minutes": 20, "why": "the deploy of api should be done"}),
    );
    assert!(answer.contains("in 20 min"), "{answer}");
    let listed = listed(&user, &id);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["source"], "timer");
    assert_eq!(listed[0]["named"], "wake: the deploy of api should be done");

    minutes(&user, 19);
    nothing_woke(&user);
    minutes(&user, 2);
    eventually("the wake", || !woken(&user).is_empty());
    let woken = woken(&user);
    assert_eq!(woken.len(), 1, "{woken:?}");
    assert!(
        woken[0].contains("the deploy of api should be done"),
        "{woken:?}"
    );
    assert!(woken[0].contains("20 min later"), "{woken:?}");
    // The time it asked at, as a time: `…T…Z`.
    assert!(woken[0].contains("Z"), "{woken:?}");
    assert!(self::listed(&user, &id).is_empty(), "it is over");
}

#[test]
fn a_second_wake_replaces_the_first() {
    let user = user();
    let (id, mcp) = agent(&user);

    wake(&mcp, json!({"minutes": 10, "why": "first"}));
    let answer = wake(&mcp, json!({"minutes": 30, "why": "second"}));
    assert!(answer.contains("replaces"), "{answer}");
    assert!(answer.contains("first"), "{answer}");
    assert_eq!(listed(&user, &id).len(), 1);

    minutes(&user, 11);
    nothing_woke(&user);
    minutes(&user, 20);
    eventually("the second wake", || !woken(&user).is_empty());
    let woken = woken(&user);
    assert_eq!(woken.len(), 1, "{woken:?}");
    assert!(woken[0].contains("second"), "{woken:?}");
}

#[test]
fn cancel_removes_the_wake_and_nothing_wakes_the_agent() {
    let user = user();
    let (id, mcp) = agent(&user);

    wake(&mcp, json!({"minutes": 5, "why": "look again"}));
    let answer = wake(&mcp, json!({"cancel": true}));
    assert!(answer.contains("look again"), "{answer}");
    assert!(listed(&user, &id).is_empty());
    let again = refused(&mcp, json!({"cancel": true}));
    assert!(again.contains("no wake"), "{again}");

    minutes(&user, 6);
    nothing_woke(&user);
}

#[test]
fn minutes_out_of_1_to_6000_or_no_why_are_refused_before_anything() {
    let user = user();
    let (id, mcp) = agent(&user);

    for minutes in [0, 6001] {
        let text = refused(&mcp, json!({"minutes": minutes, "why": "later"}));
        assert!(text.contains("from 1 to 6000"), "{text}");
    }
    for args in [
        json!({"minutes": 5}),
        json!({"minutes": 5, "why": "  "}),
        json!({"why": "later"}),
    ] {
        refused(&mcp, args);
    }
    let long = "x".repeat(300);
    refused(&mcp, json!({"minutes": 5, "why": long}));
    assert!(listed(&user, &id).is_empty());
    assert!(wake(&mcp, json!({"minutes": 6000, "why": "later"})).contains("in 6000 min"));
}

/// Kills the daemon as a crash would; the next call starts another.
fn kill_daemon(user: &User) {
    let daemon = user.status()["pid"].as_u64().unwrap();
    // SAFETY: kill has no memory precondition.
    unsafe { libc::kill(daemon as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::path::Path::new(&format!("/proc/{daemon}")).exists() {
        assert!(Instant::now() < deadline, "the daemon did not die");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_wake_survives_a_daemon_restart() {
    let user = user();
    let (id, mcp) = agent(&user);
    wake(&mcp, json!({"minutes": 20, "why": "after the restart"}));
    drop(mcp);

    kill_daemon(&user);
    user.status();
    assert_eq!(listed(&user, &id).len(), 1);
    nothing_woke(&user);
    minutes(&user, 21);
    eventually("the wake", || !woken(&user).is_empty());
    assert!(woken(&user)[0].contains("after the restart"));
}

#[test]
fn a_wake_due_while_the_daemon_was_down_fires_at_start() {
    let user = user();
    let (_, mcp) = agent(&user);
    wake(&mcp, json!({"minutes": 20, "why": "due meanwhile"}));
    drop(mcp);

    kill_daemon(&user);
    // Twenty minutes passed while it was down.
    {
        let mut store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
        let mut due = store.subscriptions().unwrap().remove(0);
        due.created_at -= 20 * 60_000;
        due.expires_at -= 20 * 60_000;
        store.subscribe(&due).unwrap();
    }
    user.status();
    eventually("the wake", || !woken(&user).is_empty());
    assert!(woken(&user)[0].contains("due meanwhile"));
}

#[test]
fn an_ended_agent_s_wake_is_removed() {
    let user = user();
    let (id, mcp) = agent(&user);
    wake(&mcp, json!({"minutes": 5, "why": "later"}));

    user.call("agent.stop", json!({"agent": id})).unwrap();
    eventually("the wake is removed", || listed(&user, &id).is_empty());
    minutes(&user, 6);
    nothing_woke(&user);
}

#[test]
fn the_agent_waiting_on_its_wake_says_so_and_forge_never_counts_it() {
    let user = user();
    let (id, mcp) = agent(&user);
    wake(&mcp, json!({"minutes": 20, "why": "later"}));

    let tree = user.call("agent.tree", json!({"agent": id})).unwrap();
    let me = tree["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent"]["id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(me["waiting_on"], "subscription", "{me}");
    assert_eq!(me["doing"], "waiting for its wake in 20 min", "{me}");
}
