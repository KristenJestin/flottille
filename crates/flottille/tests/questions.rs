//! A question a root asks the user with `ask_user` in the middle of its turn (#519): it is open at
//! once, whatever the agent does next, in the inbox, on its tree (`needs_user`, `questions`) and
//! in what `session_read` gives the agent that started the session, until it is answered. The
//! fake ACP agent, temporary folders, no real agent.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

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

/// A root of `app` in Full mode (its launch is not held), its first turn ended: its id and its
/// MCP session.
fn lead(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    user.call("agent.mode", json!({"agent": id, "mode": "full"}))
        .unwrap();
    eventually("its first turn", || state(user, &id) == "idle");
    (id, Mcp::initialize(&url, &authorization))
}

/// The session the lead starts for the user with `brief`, followed: its id.
fn launched(user: &User, mcp: &Mcp, brief: &str) -> String {
    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Charts", "brief": brief, "root": true, "follow": true}),
    );
    assert!(!answer.error, "{}", answer.text);
    agent(user, |a| a["title"] == "Charts")["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// What the agent does while it holds: asks `questions`, then reads a folder, then waits for the
/// test, as an agent that asked and went on with a long command.
fn asks_then_works(questions: &[&str]) -> Value {
    let mut steps: Vec<Value> = questions
        .iter()
        .map(|q| json!({"mcp": {"tool": "ask_user", "arguments": {"question": q}}}))
        .collect();
    steps.push(json!({"mcp": {"tool": "fs_list", "arguments": {"path": "."}}}));
    steps.push(json!({"hold": "release"}));
    steps.push(json!({"say": "Done for now."}));
    json!({"turns": [
        {"when": "Draw the charts.", "do": steps},
        {"when": "Use the median", "do": [{"say": "Median it is."}]}
    ]})
}

fn agents(user: &User) -> Vec<Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

fn agent(user: &User, which: impl Fn(&Value) -> bool) -> Value {
    agents(user).into_iter().find(which).unwrap()
}

fn state(user: &User, id: &str) -> String {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The root of the session as `agent.trees` gives it: its entry of its tree.
fn root_of_tree(user: &User, id: &str) -> Value {
    let trees = user.call("agent.trees", json!({})).unwrap();
    trees["trees"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|tree| tree["agents"].as_array().unwrap().clone())
        .find(|entry| entry["agent"]["id"] == id)
        .unwrap()
}

fn questions_in_inbox(user: &User, id: &str) -> Vec<String> {
    user.call("inbox.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "question" && e["agent"] == id)
        .map(|e| e["text"].as_str().unwrap().to_owned())
        .collect()
}

/// The fs_list call after the question: the agent went on.
fn went_on(user: &User) -> bool {
    user.received()
        .iter()
        .any(|entry| entry["mcp"]["body"].to_string().contains("README.md"))
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn release(user: &User) {
    std::fs::write(user.path("fake/release"), "").unwrap();
}

/// The wakes of the starter `lead` at the ends of the turns of the session it follows.
fn wakes(user: &User, lead: &str) -> Vec<String> {
    Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .last_deliveries(lead, 64)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == "session-turn-ended")
        .map(|delivery| delivery.body)
        .collect()
}

/// A session that asked mid-turn and still works: its id, and the lead's MCP session.
fn asking(user: &User, mode: &str, questions: &[&str]) -> (String, Mcp) {
    asking_for(user, mode, questions).1
}

/// The same, with the lead's id.
fn asking_for(user: &User, mode: &str, questions: &[&str]) -> (String, (String, Mcp)) {
    project(user);
    user.fake_script(asks_then_works(questions));
    let (lead, mcp) = lead(user);
    let session = launched(user, &mcp, "Draw the charts.");
    // Each tree keeps its own mode: the user puts the session's in `mode`.
    user.call("agent.mode", json!({"agent": session, "mode": mode}))
        .unwrap();
    eventually("the call after the question", || went_on(user));
    assert_eq!(state(user, &session), "working");
    (lead, (session, mcp))
}

#[test]
fn a_question_asked_mid_turn_is_in_the_inbox_and_marks_the_row() {
    let user = User::with_fake_agent();
    let (session, _) = asking(&user, "ask", &["POC charts: blocked by the minimum?"]);

    let listed = agent(&user, |a| a["id"] == session);
    assert_eq!(listed["waiting_on_answer"], true, "{listed}");
    let root = root_of_tree(&user, &session);
    assert_eq!(root["needs_user"], true, "{root}");
    let open = root["questions"].as_array().cloned().unwrap_or_default();
    assert_eq!(open.len(), 1, "{root}");
    assert_eq!(open[0]["question"], "POC charts: blocked by the minimum?");
    assert!(open[0]["at"].as_u64().unwrap() > 0, "{root}");
    assert_eq!(
        questions_in_inbox(&user, &session),
        ["POC charts: blocked by the minimum?"]
    );
    release(&user);
}

#[test]
fn a_question_asked_in_full_mode_shows_like_in_ask_mode() {
    let user = User::with_fake_agent();
    let (session, _) = asking(&user, "full", &["POC charts: blocked by the minimum?"]);

    let root = root_of_tree(&user, &session);
    assert_eq!(root["needs_user"], true, "{root}");
    assert_eq!(
        root["questions"].as_array().map(Vec::len),
        Some(1),
        "{root}"
    );
    assert_eq!(questions_in_inbox(&user, &session).len(), 1);

    // It stays open once the turn ends, until the user answers.
    release(&user);
    eventually("the turn's end", || state(&user, &session) == "idle");
    let root = root_of_tree(&user, &session);
    assert_eq!(root["needs_user"], true, "{root}");
    user.call(
        "agent.send",
        json!({"agent": session, "message": "Use the median."}),
    )
    .unwrap();
    eventually("the answer taken", || {
        root_of_tree(&user, &session)["questions"]
            .as_array()
            .is_some_and(Vec::is_empty)
    });
    let root = root_of_tree(&user, &session);
    assert_eq!(root["needs_user"], false, "{root}");
}

#[test]
fn two_open_questions_both_show_in_order() {
    let user = User::with_fake_agent();
    let (session, _) = asking(
        &user,
        "full",
        &["First: which chart?", "Second: which colour?"],
    );

    let root = root_of_tree(&user, &session);
    let open: Vec<&str> = root["questions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["question"].as_str().unwrap())
        .collect();
    assert_eq!(open, ["First: which chart?", "Second: which colour?"]);
    assert_eq!(
        questions_in_inbox(&user, &session),
        ["First: which chart?", "Second: which colour?"]
    );
    release(&user);
}

#[test]
fn session_read_says_the_open_questions_of_a_session() {
    let user = User::with_fake_agent();
    let (session, mcp) = asking(&user, "full", &["POC charts: blocked by the minimum?"]);

    let read = mcp.call("session_read", json!({"session": session}));

    assert!(!read.error, "{}", read.text);
    assert!(read.text.contains("open question"), "{}", read.text);
    assert!(
        read.text.contains("POC charts: blocked by the minimum?"),
        "{}",
        read.text
    );
    assert!(read.text.contains("waits for the user"), "{}", read.text);

    release(&user);
    eventually("the turn's end", || state(&user, &session) == "idle");
    user.call(
        "agent.send",
        json!({"agent": session, "message": "Use the median."}),
    )
    .unwrap();
    eventually("the answer taken", || {
        !mcp.call("session_read", json!({"session": session}))
            .text
            .contains("open question")
    });
}

#[test]
fn a_followed_session_s_question_reaches_its_starter_at_the_turn_s_end() {
    let user = User::with_fake_agent();
    let (lead, (session, _)) = asking_for(&user, "full", &["POC charts: blocked by the minimum?"]);
    // Asked mid-turn, the question wakes nobody before the turn's end.
    std::thread::sleep(Duration::from_millis(600));
    assert!(wakes(&user, &lead).is_empty(), "{:?}", wakes(&user, &lead));

    release(&user);
    eventually("the wake of the turn's end", || {
        wakes(&user, &lead).len() == 1
    });

    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("asked the user"), "{woken}");
    assert!(
        woken.contains("POC charts: blocked by the minimum?"),
        "{woken}"
    );
    assert!(woken.contains(&session), "{woken}");
}
