//! A pilot is always woken by the question and the turn end of a session it leads (#1540,
//! `docs/design.md` §5.2, `docs/tools.md`, "Following a session"): whatever the led session did
//! earlier in its turn, whether its turn was cut to take a delivery, and whether the pilot is idle,
//! in a turn or compacting. The fake ACP agent, temporary folders, no real agent.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// The kind of the delivery that wakes a pilot.
const WAKE: &str = "session-turn-ended";

/// What the scripted compactor answers.
const SUMMARY: &str = "## Goal\nLead the login work.\n## Next steps\nAnswer the session.";

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

/// A pause long enough for a wake that was going to come to have come.
fn settle() {
    std::thread::sleep(Duration::from_millis(600));
}

fn turns(user: &User, agent: &str) -> usize {
    store(user)
        .last_agent_events(agent, "agent.turn_ended", 200)
        .unwrap()
        .len()
}

/// The pilot's wakes, newest first, each with its state (`queued`, `sending`, `sent`).
fn wakes(user: &User, pilot: &str) -> Vec<(String, String)> {
    store(user)
        .last_deliveries(pilot, 64)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == WAKE)
        .map(|delivery| (delivery.body, delivery.state))
        .collect()
}

/// The texts of the prompts the fake agents received, in order.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|p| {
            p["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

fn ask(question: &str) -> Value {
    json!({"mcp": {"tool": "ask", "arguments": {"question": question}}})
}

/// A call that stands for the work a turn does.
fn work() -> Value {
    json!({"mcp": {"tool": "fs_list", "arguments": {}}})
}

fn release(user: &User, name: &str) {
    std::fs::write(user.path("fake").join(name), "").unwrap();
}

/// A project `app` over a throwaway repository, and a pilot of it in Full, its first turn ended:
/// its id and MCP session.
fn pilot(user: &User) -> (String, Mcp) {
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
    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    eventually("its first turn", || turns(user, &id) == 1);
    (id, Mcp::initialize(&url, &authorization))
}

/// A session the pilot leads, started with `brief`: its id.
fn led(user: &User, mcp: &Mcp, brief: &str) -> String {
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "led", "title": "Login", "brief": brief}),
    );
    assert!(!answer.error, "{}", answer.text);
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Login")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The script of the pilot's and the led session's turns, the pilot's id known.
fn script(user: &User, led_turns: Value) {
    let mut turns =
        vec![json!({"when": "Hold on.", "do": [{"hold": "pilot-release"}, {"say": "Held."}]})];
    turns.extend(led_turns.as_array().cloned().unwrap_or_default());
    user.fake_script(json!({ "turns": turns }));
}

/// The exact case of #1540: the led session wrote to its pilot early in a turn, worked on, then
/// asked it a question and ended the turn. The early message once stood for the turn's end, and
/// the pilot was told nothing more: neither the question nor the end.
#[test]
fn a_question_asked_after_a_message_to_the_pilot_in_the_same_turn_wakes_it() {
    let user = User::with_fake_agent();
    let (pilot, mcp) = pilot(&user);
    script(
        &user,
        json!([{"when": "Fix the login.", "do": [
            {"mcp": {"tool": "session_send",
                "arguments": {"session": pilot, "text": "The end of my plan: tests last."}}},
            work(),
            ask("May I run the end-to-end tests locally?"),
            work(),
            {"say": "I wait for your answer on the end-to-end tests."}]}]),
    );

    let session = led(&user, &mcp, "Fix the login.");

    eventually("the led session's turn", || turns(&user, &session) == 1);
    eventually("the pilot woken", || !wakes(&user, &pilot).is_empty());
    let (woken, _) = wakes(&user, &pilot).remove(0);
    for word in [
        "asked you",
        "May I run the end-to-end tests locally?",
        "I wait for your answer on the end-to-end tests.",
    ] {
        assert!(woken.contains(word), "{word}: {woken}");
    }
    eventually("the wake taken by the pilot", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("May I run the end-to-end tests locally?"))
    });
}

/// Its plain sibling: a turn that went on working after its message to the pilot still wakes it
/// at its end, with its last message.
#[test]
fn a_turn_that_went_on_after_its_message_to_the_pilot_wakes_it_at_its_end() {
    let user = User::with_fake_agent();
    let (pilot, mcp) = pilot(&user);
    script(
        &user,
        json!([{"when": "Fix the login.", "do": [
            {"mcp": {"tool": "session_send",
                "arguments": {"session": pilot, "text": "Starting with the form."}}},
            work(),
            {"say": "The form is done; the tests are next."}]}]),
    );

    let session = led(&user, &mcp, "Fix the login.");

    eventually("the led session's turn", || turns(&user, &session) == 1);
    eventually("the pilot woken", || !wakes(&user, &pilot).is_empty());
    let (woken, _) = wakes(&user, &pilot).remove(0);
    assert!(
        woken.contains("The form is done; the tests are next."),
        "{woken}"
    );
}

#[test]
fn a_question_asked_mid_turn_wakes_the_pilot() {
    let user = User::with_fake_agent();
    let (pilot, mcp) = pilot(&user);
    script(
        &user,
        json!([{"when": "Fix the login.", "do": [
            work(),
            ask("Cover the logout too?"),
            work(),
            work(),
            {"say": "Login fixed; I wait for your word on the logout."}]}]),
    );

    let session = led(&user, &mcp, "Fix the login.");

    eventually("the led session's turn", || turns(&user, &session) == 1);
    eventually("the pilot woken", || !wakes(&user, &pilot).is_empty());
    settle();
    let woken = wakes(&user, &pilot);
    assert_eq!(woken.len(), 1, "{woken:?}");
    for word in [
        "asked you",
        "Cover the logout too?",
        "I wait for your word on the logout.",
    ] {
        assert!(woken[0].0.contains(word), "{word}: {}", woken[0].0);
    }
}

/// A question asked in a turn cut to take a delivery is still pending when the turn that took
/// the delivery ends: that end wakes the pilot, with the question.
#[test]
fn the_turn_end_after_a_pending_question_still_wakes_the_pilot() {
    let user = User::with_fake_agent();
    let (pilot, mcp) = pilot(&user);
    script(
        &user,
        json!([
            {"when": "Fix the login.", "do": [
                ask("Which branch do I push to?"), {"hold": "led-release"}]},
            {"when": "Use the median.", "do": [{"say": "Using the median; I still wait on the branch."}]}
        ]),
    );
    let session = led(&user, &mcp, "Fix the login.");
    eventually("its question", || {
        !store(&user)
            .last_agent_events(&session, "agent.asked", 1)
            .unwrap()
            .is_empty()
    });
    user.call(
        "agent.send",
        json!({"agent": session, "message": "Use the median."}),
    )
    .unwrap();

    // What the 30 s fallback does to a turn that took no note: it is cancelled, and the delivery
    // is sent as the next prompt.
    user.call("agent.cancel", json!({"agent": session}))
        .unwrap();

    eventually("both turns ended", || turns(&user, &session) == 2);
    eventually("the pilot woken", || !wakes(&user, &pilot).is_empty());
    settle();
    let woken = wakes(&user, &pilot);
    assert_eq!(woken.len(), 1, "{woken:?}");
    for word in [
        "asked you",
        "Which branch do I push to?",
        "I still wait on the branch.",
    ] {
        assert!(woken[0].0.contains(word), "{word}: {}", woken[0].0);
    }
}

#[test]
fn a_question_to_a_pilot_in_a_turn_is_delivered_when_its_turn_ends() {
    let user = User::with_fake_agent();
    // The pilot's process reads the script it starts with.
    script(
        &user,
        json!([{"when": "Fix the login.", "do": [
            {"hold": "led-release"}, ask("Keep the old form?"), {"say": "Asked."}]}]),
    );
    let (pilot, mcp) = pilot(&user);
    let session = led(&user, &mcp, "Fix the login.");
    user.call("agent.send", json!({"agent": pilot, "message": "Hold on."}))
        .unwrap();
    // A pilot whose led session works is shown waiting on it: its prompt says it is in a turn.
    eventually("the pilot in a turn", || {
        prompts(&user).iter().any(|p| p.contains("Hold on."))
    });

    release(&user, "led-release");

    eventually("the led session's turn", || turns(&user, &session) == 1);
    eventually("the wake stored", || !wakes(&user, &pilot).is_empty());
    settle();
    assert_eq!(
        wakes(&user, &pilot)[0].1,
        "queued",
        "{:?}",
        wakes(&user, &pilot)
    );
    release(&user, "pilot-release");
    eventually("the wake taken once the pilot's turn ended", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("Keep the old form?"))
    });
    assert_eq!(wakes(&user, &pilot).len(), 1);
}

#[test]
fn a_question_to_a_compacting_pilot_is_delivered_after() {
    let user =
        User::with_fake_agent().with_compactor(json!([{"hold": "release"}, {"say": SUMMARY}]), "");
    // The pilot's process reads the script it starts with.
    script(
        &user,
        json!([{"when": "Fix the login.", "do": [
            {"hold": "led-release"}, ask("Keep the old form?"), {"say": "Asked."}]}]),
    );
    let (pilot, mcp) = pilot(&user);
    let session = led(&user, &mcp, "Fix the login.");
    user.call("agent.compact", json!({"agent": pilot})).unwrap();
    eventually("the pilot compacting", || {
        user.compactor_received()
            .iter()
            .any(|e| e["received"]["method"] == "session/prompt")
    });

    release(&user, "led-release");

    eventually("the led session's turn", || turns(&user, &session) == 1);
    eventually("the wake stored", || !wakes(&user, &pilot).is_empty());
    settle();
    assert_eq!(
        wakes(&user, &pilot)[0].1,
        "queued",
        "{:?}",
        wakes(&user, &pilot)
    );
    std::fs::write(user.path("fake-compactor/release"), "").unwrap();
    eventually("the wake taken by the new session", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("Keep the old form?"))
    });
    assert_eq!(wakes(&user, &pilot).len(), 1);
}
