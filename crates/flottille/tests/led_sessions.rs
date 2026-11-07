//! The kinds of `agent_launch` and the one `ask` tool (#521, `docs/tools.md`, "Sessions an agent
//! starts", "Where a question goes"): a launch names its kind, never guessed, and a question goes
//! where the daemon says for the caller. The fake ACP agent, temporary folders, no real agent.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository, and a root of it in Full (nothing is judged
/// here), its first turn ended: its id and its MCP session.
fn lead(user: &User) -> (String, Mcp) {
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

/// What the lead gets for `agent_launch` with `arguments`: the answer, and the agent's id and MCP
/// session once it started.
fn launch(user: &User, mcp: &Mcp, arguments: Value) -> (Answer, Option<(String, Mcp)>) {
    let opened = user.received_requests("session/new").len();
    let title = arguments["title"].clone();
    let answer = mcp.call("agent_launch", arguments);
    if answer.error {
        return (answer, None);
    }
    let (url, authorization) = server_of(user, opened);
    let id = agent(user, |a| a["title"] == title)["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (answer, Some((id, Mcp::initialize(&url, &authorization))))
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

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn turns(user: &User, agent: &str) -> usize {
    store(user)
        .last_agent_events(agent, "agent.turn_ended", 200)
        .unwrap()
        .len()
}

/// The bodies of the deliveries of `kind` stored for `agent`, newest first.
fn delivered(user: &User, agent: &str, kind: &str) -> Vec<String> {
    store(user)
        .last_deliveries(agent, 64)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == kind)
        .map(|delivery| delivery.body)
        .collect()
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

/// A child of the lead, of kind `child`: its id and MCP session.
fn child(user: &User, mcp: &Mcp) -> (String, Mcp) {
    let (answer, child) = launch(
        user,
        mcp,
        json!({"kind": "child", "title": "worker", "brief": "Write the tests."}),
    );
    assert!(!answer.error, "{}", answer.text);
    child.unwrap()
}

#[test]
fn agent_launch_refuses_a_call_without_kind_naming_each_kind() {
    let user = User::with_fake_agent();
    let (_, mcp) = lead(&user);

    let (answer, started) = launch(
        &user,
        &mcp,
        json!({"title": "worker", "brief": "Write the tests."}),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(started.is_none());
    for word in ["refused", "kind", "child", "led", "handed"] {
        assert!(answer.text.contains(word), "{word}: {}", answer.text);
    }
    assert!(agents(&user).iter().all(|a| a["title"] != "worker"));
}

#[test]
fn agent_launch_refuses_root_and_follow_naming_kind() {
    let user = User::with_fake_agent();
    let (_, mcp) = lead(&user);

    for arguments in [
        json!({"title": "Login", "brief": "Fix it.", "root": true, "follow": true}),
        json!({"title": "Login", "brief": "Fix it.", "root": true, "follow": false}),
        json!({"title": "Login", "brief": "Fix it.", "kind": "led", "follow": true}),
        json!({"title": "Login", "brief": "Fix it.", "kind": "child", "root": false}),
    ] {
        let (answer, started) = launch(&user, &mcp, arguments.clone());

        assert!(answer.error, "{arguments}: {}", answer.text);
        assert!(started.is_none());
        for word in ["refused", "no longer", "kind", "child", "led", "handed"] {
            assert!(answer.text.contains(word), "{word}: {}", answer.text);
        }
    }
    assert!(agents(&user).iter().all(|a| a["title"] != "Login"));
}

#[test]
fn a_child_launch_is_unchanged_with_kind_child() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);

    let (answer, started) = launch(
        &user,
        &mcp,
        json!({"kind": "child", "title": "worker", "brief": "Write the tests."}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.starts_with("Launched"), "{}", answer.text);
    let (id, _) = started.unwrap();
    let child = agent(&user, |a| a["id"] == id);
    assert_eq!(child["parent"], lead.as_str());
    assert_eq!(child["role"], "worker");
    assert!(child["launched_by"].is_null(), "{child}");
}

#[test]
fn a_child_refuses_a_workspace_as_before() {
    let user = User::with_fake_agent();
    let (_, mcp) = lead(&user);

    let (answer, _) = launch(
        &user,
        &mcp,
        json!({"kind": "child", "title": "worker", "brief": "Go.", "new_workspace": "w"}),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("\"kind\": \"led\""), "{}", answer.text);
}

#[test]
fn ask_from_a_child_reaches_its_parent() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);
    let (child, child_mcp) = child(&user, &mcp);

    let answer = child_mcp.call("ask", json!({"question": "Which test framework?"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("parent"), "{}", answer.text);
    eventually("the parent told", || {
        delivered(&user, &lead, "child-question")
            .iter()
            .any(|body| body.contains("Which test framework?"))
    });
    assert!(questions_in_inbox(&user, &child).is_empty());
}

#[test]
fn ask_from_a_plain_root_reaches_the_user() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);

    let answer = mcp.call("ask", json!({"question": "Push now?"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("user"), "{}", answer.text);
    assert_eq!(questions_in_inbox(&user, &lead), ["Push now?"]);
}

#[test]
fn ask_says_where_the_question_goes_for_the_caller() {
    let user = User::with_fake_agent();
    let (_, mcp) = lead(&user);
    let (_, child_mcp) = child(&user, &mcp);

    let description = |mcp: &Mcp| {
        mcp.tools()
            .into_iter()
            .find(|tool| tool["name"] == "ask")
            .unwrap()["description"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    assert!(description(&mcp).contains("goes to the user"));
    assert!(description(&child_mcp).contains("goes to your parent"));
    for mcp in [&mcp, &child_mcp] {
        let names: Vec<Value> = mcp.tools().iter().map(|t| t["name"].clone()).collect();
        assert!(!names.contains(&json!("ask_user")), "{names:?}");
        assert!(!names.contains(&json!("ask_parent")), "{names:?}");
    }
}

#[test]
fn ask_user_and_ask_parent_still_answer_routed_as_ask() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);
    let (child, child_mcp) = child(&user, &mcp);

    // Each old name is routed as ask would be, whoever calls it.
    let root_asks = mcp.call("ask_parent", json!({"question": "Push now?"}));
    let child_asks = child_mcp.call("ask_user", json!({"question": "Which framework?"}));

    assert!(!root_asks.error, "{}", root_asks.text);
    assert!(!child_asks.error, "{}", child_asks.text);
    assert_eq!(questions_in_inbox(&user, &lead), ["Push now?"]);
    assert!(questions_in_inbox(&user, &child).is_empty());
    eventually("the parent told", || {
        delivered(&user, &lead, "child-question")
            .iter()
            .any(|body| body.contains("Which framework?"))
    });
}

// Led sessions.

/// The kind of the delivery that wakes a pilot.
const WAKE: &str = "session-turn-ended";

fn wakes(user: &User, agent: &str) -> Vec<String> {
    delivered(user, agent, WAKE)
}

/// A pause long enough for a wake that was going to come to have come.
fn settle() {
    std::thread::sleep(Duration::from_millis(600));
}

/// A session the lead leads, started with `brief`: its id and MCP session.
fn led(user: &User, mcp: &Mcp, brief: &str) -> (String, Mcp) {
    let (answer, session) = launch(
        user,
        mcp,
        json!({"kind": "led", "title": "Login", "brief": brief}),
    );
    assert!(!answer.error, "{}", answer.text);
    session.unwrap()
}

fn shown(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
}

/// The entry of `id` in its tree, as `agent.trees` gives it.
fn entry(user: &User, id: &str) -> Value {
    user.call("agent.trees", json!({})).unwrap()["trees"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|tree| tree["agents"].as_array().unwrap().clone())
        .find(|entry| entry["agent"]["id"] == id)
        .unwrap()
}

fn asks(question: &str) -> Value {
    json!({"mcp": {"tool": "ask", "arguments": {"question": question}}})
}

#[test]
fn a_led_session_s_question_wakes_its_pilot_not_the_user() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [asks("Cover the logout too?"), {"say": "I asked."}]}
    ]}));
    let (lead, mcp) = lead(&user);

    let (session, session_mcp) = led(&user, &mcp, "Fix the login.");

    eventually("the pilot woken", || wakes(&user, &lead).len() == 1);
    let woken = wakes(&user, &lead).remove(0);
    for word in [
        "\"Login\"",
        "asked you",
        "Cover the logout too?",
        "I asked.",
    ] {
        assert!(woken.contains(word), "{word}: {woken}");
    }
    assert!(questions_in_inbox(&user, &session).is_empty());
    let waiting = entry(&user, &session);
    assert_eq!(waiting["waiting_on"], "pilot", "{waiting}");
    assert_eq!(waiting["needs_user"], false, "{waiting}");
    // The user sees it in the session too, asked of its pilot.
    let asked = store(&user)
        .last_agent_events(&session, "agent.asked", 1)
        .unwrap();
    assert_eq!(asked[0].data["to"], "pilot");
    assert_eq!(asked[0].data["pilot"], lead.as_str());
    let description = session_mcp
        .tools()
        .into_iter()
        .find(|tool| tool["name"] == "ask")
        .unwrap()["description"]
        .clone();
    assert!(
        description
            .as_str()
            .unwrap()
            .contains("goes to the agent that leads"),
        "{description}"
    );

    let answered = mcp.call(
        "session_send",
        json!({"session": session, "text": "Yes, the logout too."}),
    );

    assert!(!answered.error, "{}", answered.text);
    eventually("the question answered", || {
        entry(&user, &session)["waiting_on"].is_null()
    });
}

#[test]
fn a_led_session_s_plain_turn_end_does_not_wake_its_pilot() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);

    let (session, _) = led(&user, &mcp, "Fix the login.");

    eventually("its first turn", || turns(&user, &session) == 1);
    user.call("agent.send", json!({"agent": session, "message": "Go on."}))
        .unwrap();
    eventually("its second turn", || turns(&user, &session) == 2);
    settle();
    assert!(wakes(&user, &lead).is_empty());
}

#[test]
fn a_led_session_s_report_and_end_wake_its_pilot() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "Login fixed."}}},
            {"say": "Reported."}]}
    ]}));
    let (lead, mcp) = lead(&user);

    let (session, _) = led(&user, &mcp, "Fix the login.");

    eventually("the report's wake", || wakes(&user, &lead).len() == 1);
    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("it reported"), "{woken}");
    user.call("agent.stop", json!({"agent": session})).unwrap();
    eventually("the end's wake", || wakes(&user, &lead).len() == 2);
    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("ended"), "{woken}");
}

/// The user allows the launch `answer` held, when it was: the started agent's id and MCP session.
fn allowed_launch(user: &User, answer: Answer, title: &str, opened: usize) -> (String, Mcp) {
    assert!(!answer.error, "{}", answer.text);
    if let Some(number) = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse::<u64>().unwrap())
    {
        user.call("approval.approve", json!({"number": number}))
            .unwrap();
    }
    eventually("the launched session", || {
        agents(user).iter().any(|a| a["title"] == title)
    });
    let (url, authorization) = server_of(user, opened);
    let id = agent(user, |a| a["title"] == title)["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (id, Mcp::initialize(&url, &authorization))
}

/// The lead's session asked its pilot "Cover the logout too?", and the pilot was woken with it:
/// the lead, its MCP session and the session's id.
fn asked_its_pilot(user: &User) -> (String, Mcp, String) {
    user.fake_script(json!({"turns": [
        {"when": "Fix the login.", "do": [asks("Cover the logout too?"), {"say": "I asked."}]}
    ]}));
    let (lead, mcp) = lead(user);
    let (session, _) = led(user, &mcp, "Fix the login.");
    eventually("the pilot woken", || wakes(user, &lead).len() == 1);
    assert_eq!(entry(user, &session)["waiting_on"], "pilot");
    assert!(questions_in_inbox(user, &session).is_empty());
    (lead, mcp, session)
}

/// The question the session asked its pilot reached the user, and the session waits on nobody
/// but them.
fn reached_the_user(user: &User, session: &str) {
    eventually("the question in the inbox", || {
        questions_in_inbox(user, session) == ["Cover the logout too?"]
    });
    let waiting = entry(user, session);
    assert_ne!(waiting["waiting_on"], "pilot", "{waiting}");
    assert_eq!(waiting["needs_user"], true, "{waiting}");
}

#[test]
fn a_paused_following_sends_the_question_to_the_user() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Ask now", "do": [asks("Ship it?"), {"say": "Asked."}]}
    ]}));
    let (lead, mcp) = lead(&user);
    let (session, _) = led(&user, &mcp, "Fix the login.");
    eventually("its first turn", || turns(&user, &session) == 1);

    // The fifth message between them pauses the following; its turn asks.
    for text in ["Go on.", "Go on.", "Go on.", "Go on.", "Ask now"] {
        let sent = mcp.call("session_send", json!({"session": session, "text": text}));
        assert!(!sent.error, "{}", sent.text);
    }

    eventually("the question in the inbox", || {
        questions_in_inbox(&user, &session) == ["Ship it?"]
    });
    let asked = store(&user)
        .last_agent_events(&session, "agent.asked", 1)
        .unwrap();
    assert_eq!(asked[0].data["to"], "user");
    let _ = lead;
}

#[test]
fn a_pilot_that_ends_with_a_question_open_hands_it_to_the_user() {
    let user = User::with_fake_agent();
    let (lead, _, session) = asked_its_pilot(&user);

    user.call("agent.stop", json!({"agent": lead})).unwrap();

    reached_the_user(&user, &session);
    eventually("the session told", || {
        !delivered(&user, &session, "handed").is_empty()
    });
    let told = delivered(&user, &session, "handed");
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("went to the user"), "{}", told[0]);
}

#[test]
fn the_user_cancelling_the_following_hands_the_question_to_the_user() {
    let user = User::with_fake_agent();
    let (lead, _, session) = asked_its_pilot(&user);
    let following = user.call("subscription.list", json!({})).unwrap()["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["agent"] == lead.as_str())
        .unwrap()["id"]
        .clone();

    user.call("subscription.cancel", json!({"subscription": following}))
        .unwrap();

    reached_the_user(&user, &session);
    eventually("the session told", || {
        delivered(&user, &session, "unfollowed").len() == 1
    });
}

#[test]
fn only_its_pilot_answers_a_question_asked_of_it() {
    let user = User::with_fake_agent();
    let (_, _, session) = asked_its_pilot(&user);
    // The session starts a session of its own, which writes to it.
    let (url, authorization) = server_of(&user, 1);
    let session_mcp = Mcp::initialize(&url, &authorization);
    let opened = user.received_requests("session/new").len();
    let answer = session_mcp.call(
        "agent_launch",
        json!({"kind": "handed", "title": "Docs", "brief": "Write the docs."}),
    );
    let (_, third) = allowed_launch(&user, answer, "Docs", opened);

    let sent = third.call(
        "session_send",
        json!({"session": session, "text": "Not your pilot speaking."}),
    );

    assert!(!sent.error, "{}", sent.text);
    eventually("the session took it", || {
        delivered(&user, &session, "session")
            .iter()
            .any(|body| body.contains("Not your pilot speaking."))
    });
    settle();
    let waiting = entry(&user, &session);
    assert_eq!(waiting["waiting_on"], "pilot", "{waiting}");
}

#[test]
fn the_user_talks_to_a_led_session_directly() {
    let user = User::with_fake_agent();
    let (lead, mcp) = lead(&user);
    let (session, _) = led(&user, &mcp, "Fix the login.");
    eventually("its first turn", || turns(&user, &session) == 1);

    user.call(
        "agent.send",
        json!({"agent": session, "message": "Use the median."}),
    )
    .unwrap();

    eventually("its second turn", || turns(&user, &session) == 2);
    assert!(
        delivered(&user, &session, "user")
            .iter()
            .any(|body| body.contains("Use the median."))
    );
    settle();
    // The pilot is neither asked nor woken.
    assert!(wakes(&user, &lead).is_empty());
    let pending = user.call("approval.list", json!({})).unwrap();
    assert!(
        pending["requests"].as_array().unwrap().is_empty(),
        "{pending}"
    );
}

#[test]
fn a_led_session_goes_on_when_its_pilot_ends_and_is_handed() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "Ask now", "do": [asks("Ship it?"), {"say": "Asked."}]}
    ]}));
    let (lead, mcp) = lead(&user);
    let (session, _) = led(&user, &mcp, "Fix the login.");
    eventually("its first turn", || turns(&user, &session) == 1);
    assert_eq!(shown(&user, &session)["led_by"], lead.as_str());

    user.call("agent.stop", json!({"agent": lead})).unwrap();

    eventually("handed", || shown(&user, &session)["led_by"].is_null());
    let session_now = shown(&user, &session);
    assert_eq!(session_now["launched_by"], lead.as_str());
    assert!(
        !["ended", "failed"].contains(&session_now["state"].as_str().unwrap()),
        "{session_now}"
    );
    eventually("the session told once", || {
        delivered(&user, &session, "handed").len() == 1
    });
    // Its questions now reach the user.
    user.call(
        "agent.send",
        json!({"agent": session, "message": "Ask now"}),
    )
    .unwrap();
    eventually("the question in the inbox", || {
        questions_in_inbox(&user, &session) == ["Ship it?"]
    });
}
