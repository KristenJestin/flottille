//! The scenarios of a daemon restart (`docs/design.md` §5.5): the sessions are resumed as they
//! were, a session at rest gets no prompt and a message already delivered is never delivered
//! again, while one still queued is delivered once. The agents and the compactor are fake ACP
//! agents (`crates/flottille-fake-agent`) recording every prompt, in throwaway repositories and
//! temporary XDG folders.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// What the scripted compactor answers.
const SUMMARY: &str = "## Goal\nFix the login.\n## Next steps\nCover the logout.";

/// The kind of the delivery that wakes the pilot of a led session.
const WAKE: &str = "session-turn-ended";

/// What the agents play. A session that starts from a summary (the brief given again with it)
/// only picks up; a first message alone reports (a child's only works, a child that reports
/// ending), so that a led session given it wakes its pilot, and says enough that a summary is
/// smaller than its conversation.
fn script() -> Value {
    let details = format!(
        "Details: {}",
        "the form posts to the API and shows its errors. ".repeat(60)
    );
    json!({"turns": [
        {"when": "earlier_conversation_summary", "do": [{"say": "Picking up."}]},
        {"when": "Fix the login.", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "The login is fixed."}}},
            {"say": details}]},
        {"when": "Write the tests.", "do": [{"say": details}]},
        {"when": "HANG", "do": [{"hold": "release"}]}
    ]})
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(script());
    user.with_compactor(json!([{"say": SUMMARY}]), "")
}

fn store(user: &User) -> Store {
    Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A project `app` over a throwaway repository, and a root of it started in Full (nothing is
/// judged here, and no switch is told) whose first message is `message`: its id and its MCP
/// session.
fn root(user: &User, message: &str) -> (String, Mcp) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": message, "mode": "full"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    eventually("its first turn", || turns(user, &id) >= 1);
    (id, Mcp::initialize(&url, &authorization))
}

/// The session the root started with `agent_launch` of `kind` (`led` or `handed`), briefed
/// `Fix the login.`: its id, once its first turn ended.
fn launched(user: &User, mcp: &Mcp, kind: &str) -> String {
    launched_with(user, mcp, kind, "Fix the login.")
}

/// `launched`, briefed `brief`.
fn launched_with(user: &User, mcp: &Mcp, kind: &str, brief: &str) -> String {
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": kind, "title": "Login", "brief": brief}),
    );
    assert!(!answer.error, "{}", answer.text);
    let id = user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Login")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the session's first turn", || turns(user, &id) >= 1);
    id
}

/// How many turns of the agent ended.
fn turns(user: &User, agent: &str) -> usize {
    store(user)
        .last_agent_events(agent, "agent.turn_ended", 200)
        .unwrap()
        .len()
}

/// The texts of the prompts the fake agents received, in order.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|p| {
            p["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn wakes(user: &User, agent: &str) -> usize {
    store(user)
        .last_deliveries(agent, 64)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == WAKE)
        .count()
}

/// The agent's conversation is summarised by hand, and its new session has taken its brief, the
/// summary and its latest messages.
fn compact(user: &User, agent: &str) {
    user.call("agent.compact", json!({"agent": agent})).unwrap();
    eventually("the new session's first prompt", || {
        prompts(user).iter().any(|p| p.contains(SUMMARY))
    });
}

/// Every agent is idle with nothing queued nor being sent, and nothing more comes for a while.
fn at_rest(user: &User) {
    let quiet = || {
        let agents = user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .clone();
        agents.iter().all(|agent| {
            let id = agent["id"].as_str().unwrap();
            agent["state"] == "idle"
                && ["queued", "sending"].iter().all(|state| {
                    store(user)
                        .deliveries(id, state)
                        .is_ok_and(|d| d.is_empty())
                })
        })
    };
    eventually("every agent at rest", quiet);
    let before = prompts(user).len();
    std::thread::sleep(Duration::from_millis(1_500));
    assert!(quiet(), "an agent left its rest");
    assert_eq!(prompts(user).len(), before, "{:#?}", prompts(user));
}

/// The daemon is stopped, and the next request starts another.
fn restart(user: &User) {
    assert!(user.run(&["daemon", "stop"]).status.success());
    user.status();
}

/// Long enough after a restart for what it would send to have been sent and played.
fn settle() {
    std::thread::sleep(Duration::from_millis(3_000));
}

/// The prompts the fake agents received after the first `before`.
fn since(user: &User, before: usize) -> Vec<String> {
    prompts(user).split_off(before)
}

#[test]
fn a_led_session_at_rest_gets_no_prompt_after_a_restart() {
    let user = user();
    let (_, mcp) = root(&user, "hello");
    let session = launched(&user, &mcp, "led");
    compact(&user, &session);
    at_rest(&user);
    let before = prompts(&user).len();

    restart(&user);
    settle();

    assert_eq!(since(&user, before), Vec::<String>::new());
}

#[test]
fn a_handed_session_at_rest_gets_no_prompt_after_a_restart() {
    let user = user();
    let (_, mcp) = root(&user, "hello");
    let session = launched(&user, &mcp, "handed");
    compact(&user, &session);
    at_rest(&user);
    let before = prompts(&user).len();

    restart(&user);
    settle();

    assert_eq!(since(&user, before), Vec::<String>::new());
}

#[test]
fn a_child_at_rest_gets_no_prompt_after_a_restart() {
    let user = user();
    let (_, mcp) = root(&user, "hello");
    // A child that reports ends: this one only works, and is left idle.
    let child = launched_with(&user, &mcp, "child", "Write the tests.");
    compact(&user, &child);
    at_rest(&user);
    let before = prompts(&user).len();

    restart(&user);
    settle();

    assert_eq!(since(&user, before), Vec::<String>::new());
}

#[test]
fn a_root_at_rest_gets_no_prompt_after_a_restart() {
    let user = user();
    let (agent, _) = root(&user, "Fix the login.");
    compact(&user, &agent);
    at_rest(&user);
    let before = prompts(&user).len();

    restart(&user);
    settle();

    assert_eq!(since(&user, before), Vec::<String>::new());
}

#[test]
fn a_delivery_already_given_is_not_given_again_after_a_restart() {
    let user = user();
    let (agent, _) = root(&user, "hello");
    user.call(
        "agent.send",
        json!({"agent": agent, "message": "Hold on HANG"}),
    )
    .unwrap();
    eventually("the held turn", || {
        prompts(&user).iter().any(|p| p.contains("Hold on HANG"))
    });
    let before = prompts(&user).len();

    // Stopped in the turn: the prompt that started it was given, the turn is cut.
    restart(&user);
    eventually("the word on the cut turn", || {
        since(&user, before).len() == 1
    });
    settle();

    let given = since(&user, before);
    assert_eq!(given.len(), 1, "{given:#?}");
    assert!(given[0].contains("your last turn was cut"), "{given:#?}");
    assert!(!given[0].contains("Hold on HANG"), "{given:#?}");
}

#[test]
fn a_message_queued_and_not_yet_delivered_is_delivered_once_after_a_restart() {
    let user = user();
    let (agent, _) = root(&user, "hello");
    user.call(
        "agent.send",
        json!({"agent": agent, "message": "Hold on HANG"}),
    )
    .unwrap();
    eventually("the held turn", || {
        prompts(&user).iter().any(|p| p.contains("Hold on HANG"))
    });
    // It waits for the turn's end, which the stop cuts.
    user.call(
        "agent.send",
        json!({"agent": agent, "message": "Mind the footer."}),
    )
    .unwrap();
    let before = prompts(&user).len();

    restart(&user);
    eventually("the queued message", || {
        since(&user, before)
            .iter()
            .any(|p| p.contains("Mind the footer."))
    });
    settle();

    let given = since(&user, before);
    let footer = given
        .iter()
        .filter(|p| p.contains("Mind the footer."))
        .count();
    assert_eq!(footer, 1, "{given:#?}");
    assert!(
        given.iter().all(|p| !p.contains("Hold on HANG")),
        "{given:#?}"
    );
}

#[test]
fn a_message_taken_but_not_yet_sent_is_delivered_once_after_a_restart() {
    let user = user();
    let (agent, _) = root(&user, "hello");
    at_rest(&user);
    assert!(user.run(&["daemon", "stop"]).status.success());
    // The daemon was killed once it had taken the message for a prompt, before the agent's
    // process started: nothing was sent.
    let mut store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let message = flottille_core::approval::Delivery {
        id: "taken-1".into(),
        agent: agent.clone(),
        kind: "user".into(),
        body: "Mind the header.".into(),
        urgency: flottille_core::approval::BETWEEN_TURNS.into(),
        state: "queued".into(),
        created_at: 0,
        sent_at: None,
        images: Vec::new(),
        typed: None,
        commands: Vec::new(),
    };
    store.deliver(&message).unwrap();
    store
        .move_deliveries(
            &agent,
            std::slice::from_ref(&message.id),
            "queued",
            "sending",
            "prompt",
        )
        .unwrap();
    drop(store);
    let before = prompts(&user).len();

    user.status();
    eventually("the message taken", || {
        since(&user, before)
            .iter()
            .any(|p| p.contains("Mind the header."))
    });
    settle();

    let given = since(&user, before);
    let header = given
        .iter()
        .filter(|p| p.contains("Mind the header."))
        .count();
    assert_eq!(header, 1, "{given:#?}");
}

#[test]
fn the_pilot_is_not_woken_by_a_restart() {
    let user = user();
    let (lead, mcp) = root(&user, "hello");
    let session = launched(&user, &mcp, "led");
    eventually("the report's wake", || wakes(&user, &lead) == 1);
    compact(&user, &session);
    at_rest(&user);
    let woken = wakes(&user, &lead);

    restart(&user);
    settle();

    assert_eq!(wakes(&user, &lead), woken);
}
