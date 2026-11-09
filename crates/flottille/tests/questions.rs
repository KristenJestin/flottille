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

/// The session the lead starts with `brief`, of `kind` (`led`: its questions go to the lead,
/// `handed`: to the user): its id.
fn launched(user: &User, mcp: &Mcp, kind: &str, brief: &str) -> String {
    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Charts", "brief": brief, "kind": kind}),
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
    let steps: Vec<Value> = questions
        .iter()
        .map(|q| json!({"mcp": {"tool": "ask_user", "arguments": {"question": q}}}))
        .collect();
    asks_then_works_steps(steps)
}

/// The same, the agent making this one call (an `ask`) instead.
fn asks_then_works_with(call: Value) -> Value {
    asks_then_works_steps(vec![call])
}

fn asks_then_works_steps(mut steps: Vec<Value>) -> Value {
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
    asking_for(user, "handed", mode, questions).1
}

/// The same, with the lead's id.
fn asking_for(user: &User, kind: &str, mode: &str, questions: &[&str]) -> (String, (String, Mcp)) {
    asking_with(user, kind, mode, asks_then_works(questions))
}

/// The same, the agent playing `script`.
fn asking_with(user: &User, kind: &str, mode: &str, script: Value) -> (String, (String, Mcp)) {
    project(user);
    user.fake_script(script);
    let (lead, mcp) = lead(user);
    let session = launched(user, &mcp, kind, "Draw the charts.");
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
fn a_led_session_s_question_reaches_its_pilot_at_the_turn_s_end() {
    let user = User::with_fake_agent();
    let (lead, (session, _)) = asking_for(
        &user,
        "led",
        "full",
        &["POC charts: blocked by the minimum?"],
    );
    // Asked mid-turn, the question wakes nobody before the turn's end.
    std::thread::sleep(Duration::from_millis(600));
    assert!(wakes(&user, &lead).is_empty(), "{:?}", wakes(&user, &lead));

    release(&user);
    eventually("the wake of the turn's end", || {
        wakes(&user, &lead).len() == 1
    });

    let woken = wakes(&user, &lead).remove(0);
    assert!(woken.contains("asked you"), "{woken}");
    assert!(
        woken.contains("POC charts: blocked by the minimum?"),
        "{woken}"
    );
    assert!(woken.contains(&session), "{woken}");
}

/// A root of `app` in Full mode whose agent plays `script`, its first turn ended: its id and its
/// MCP session.
fn root(user: &User, script: Value) -> (String, Mcp) {
    project(user);
    user.fake_script(script);
    lead(user)
}

fn ask(question: &str) -> Value {
    json!({"mcp": {"tool": "ask_user", "arguments": {"question": question}}})
}

fn send(user: &User, id: &str, message: &str) {
    user.call("agent.send", json!({"agent": id, "message": message}))
        .unwrap();
}

/// The open questions of the root, as its tree says them.
fn open_of(user: &User, id: &str) -> Vec<String> {
    root_of_tree(user, id)["questions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|q| q["question"].as_str().unwrap().to_owned())
        .collect()
}

fn events_of(user: &User, id: &str, name: &str) -> usize {
    Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .last_agent_events(id, name, 100)
        .unwrap()
        .len()
}

#[test]
fn a_question_answered_before_a_replacement_is_not_open() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        json!({"turns": [
            {"when": "ask then die", "times": 1, "do": [ask("Q1: which chart?"), {"die": 3}]},
            {"when": "ask again", "do": [ask("Q2: which colour?"), {"hold": "release"}]}
        ]}),
    );
    send(&user, &id, "ask then die");
    eventually("the replacement", || {
        events_of(&user, &id, "agent.replaced") == 1
    });
    eventually("its new session at rest", || state(&user, &id) == "idle");
    assert!(open_of(&user, &id).is_empty(), "{:?}", open_of(&user, &id));
    assert!(
        events_of(&user, &id, "agent.answered") >= 1,
        "the replacement closes the question"
    );

    send(&user, &id, "ask again");
    eventually("the second question", || !open_of(&user, &id).is_empty());
    assert_eq!(open_of(&user, &id), ["Q2: which colour?"]);
    release(&user);
}

#[test]
fn a_question_answered_before_the_upgrade_is_not_open() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        json!({"turns": [
            {"when": "ask first", "do": [ask("Q1: which database?")]},
            {"when": "Postgres", "do": [{"say": "Postgres it is."}]},
            {"when": "ask again", "do": [ask("Q2: which port?"), {"hold": "release"}]}
        ]}),
    );
    send(&user, &id, "ask first");
    eventually("the first question", || open_of(&user, &id).len() == 1);
    eventually("its turn's end", || state(&user, &id) == "idle");
    send(&user, &id, "Postgres.");
    eventually("the answer taken", || open_of(&user, &id).is_empty());
    eventually("its turn's end", || state(&user, &id) == "idle");
    // An older version stored no `agent.answered`.
    let db = rusqlite::Connection::open(user.path("state/flottille/flottille.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db.execute(
        "DELETE FROM agent_events WHERE seq IN (SELECT seq FROM events WHERE name = 'agent.answered')",
        [],
    )
    .unwrap();
    db.execute("DELETE FROM events WHERE name = 'agent.answered'", [])
        .unwrap();
    assert_eq!(events_of(&user, &id, "agent.answered"), 0);

    send(&user, &id, "ask again");
    eventually("the second question", || !open_of(&user, &id).is_empty());
    assert_eq!(open_of(&user, &id), ["Q2: which port?"]);
    release(&user);
}

#[test]
fn a_message_written_before_a_question_does_not_answer_it() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        json!({"turns": [
            {"when": "work then ask", "do": [
                {"hold": "asked"},
                ask("Q: keep the minimum?"),
                {"mcp": {"tool": "fs_list", "arguments": {"path": "."}}}]},
            {"when": "Also check the logs.", "do": [{"say": "Checked."}]}
        ]}),
    );
    send(&user, &id, "work then ask");
    eventually("the turn runs", || state(&user, &id) == "working");
    // Written while the agent works, before it asks: it waits for the turn's end.
    send(&user, &id, "Also check the logs.");
    std::fs::write(user.path("fake/asked"), "").unwrap();
    let taken = || {
        Store::open(&user.path("state/flottille/flottille.db"))
            .unwrap()
            .last_deliveries_of(&id, "user", 10)
            .unwrap()
            .into_iter()
            .any(|d| d.body.contains("Also check the logs.") && d.state == "sent")
    };
    eventually("the message read after the question", taken);
    eventually("its turn's end", || state(&user, &id) == "idle");

    assert_eq!(open_of(&user, &id), ["Q: keep the minimum?"]);
    assert_eq!(root_of_tree(&user, &id)["needs_user"], true);
}

#[test]
fn the_wake_carries_at_most_4000_characters_of_questions() {
    let user = User::with_fake_agent();
    project(&user);
    let long = |n: usize| format!("Question {n}: {}", "x".repeat(1_900));
    user.fake_script(json!({"turns": [{"when": "Ask a lot.", "do": [
        ask(&long(1)), ask(&long(2)), ask(&long(3)), ask(&long(4)), {"say": "Asked."}]}]}));
    let (lead, mcp) = lead(&user);
    launched(&user, &mcp, "led", "Ask a lot.");
    eventually("the wake", || wakes(&user, &lead).len() == 1);

    let woken = wakes(&user, &lead).remove(0);
    assert_eq!(woken.matches("Its question to you").count(), 2, "{woken}");
    assert!(
        woken.contains("Question 1:") && woken.contains("Question 2:"),
        "{woken}"
    );
    assert!(
        woken.contains("[2 more questions; session_read reads them]"),
        "{woken}"
    );
    assert!(woken.contains("Asked."), "{woken}");
}

#[test]
fn session_read_cuts_each_question_and_counts_them_in_its_page() {
    let user = User::with_fake_agent();
    project(&user);
    let long = |n: usize| format!("Question {n}: {}", "y".repeat(5_000));
    user.fake_script(json!({"turns": [{"when": "Ask a lot.", "do": [
        ask(&long(1)), ask(&long(2)), ask(&long(3)), ask(&long(4)), ask(&long(5)),
        {"say": "z".repeat(3_000)}]}]}));
    let (_, mcp) = lead(&user);
    let session = launched(&user, &mcp, "handed", "Ask a lot.");
    eventually("the session's turn", || {
        state(&user, &session) == "idle" && open_of(&user, &session).len() == 5
    });

    let read = mcp.call("session_read", json!({"session": session}));

    assert!(!read.error, "{}", read.text);
    let longest = read.text.split(|c| c != 'y').map(str::len).max().unwrap();
    assert!(
        longest <= 4_000,
        "a question is cut to a message's size: {longest}"
    );
    assert!(read.text.contains("Question 1:"), "the oldest first");
    assert!(
        read.text.contains("more open questions"),
        "{}",
        &read.text[read.text.len() - 600..]
    );
    assert!(
        read.text.chars().count() < 30_000,
        "{}",
        read.text.chars().count()
    );
}

/// The three questions of the issue (#637): one with options, one in words, one multiple.
fn three_questions() -> Value {
    json!({"mcp": {"tool": "ask", "arguments": {"questions": [
        {"question": "Push `feat/42-export` and open the pull request?",
         "options": [{"label": "Yes"}, {"label": "Not yet", "description": "Keep it local"}]},
        {"question": "Title of the pull request?"},
        {"question": "Who reviews it?", "multiple": true,
         "options": [{"label": "Alice"}, {"label": "Bob"}, {"label": "Carol"}]}
    ]}}})
}

/// The number of the root's oldest open ask, as its tree says it.
fn open_seqs(user: &User, id: &str) -> Vec<u64> {
    root_of_tree(user, id)["questions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|q| q["seq"].as_u64().unwrap())
        .collect()
}

fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|r| {
            r["params"]["prompt"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

#[test]
fn the_answer_message_quotes_each_question_and_its_answer() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        json!({"turns": [
            {"when": "ship it", "do": [three_questions(), {"say": "Asked."}]},
            {"when": "Export orders as CSV", "do": [{"say": "Shipping."}]}
        ]}),
    );
    send(&user, &id, "ship it");
    eventually("the ask", || open_seqs(&user, &id).len() == 1);
    let entry = root_of_tree(&user, &id)["questions"][0].clone();
    assert_eq!(entry["questions"].as_array().unwrap().len(), 3, "{entry}");
    eventually("its turn's end", || state(&user, &id) == "idle");

    let seq = open_seqs(&user, &id)[0];
    user.call(
        "agent.answer",
        json!({"agent": id, "question": seq, "answers": [
            {"choices": ["Yes"], "text": "Squash it first."},
            {"text": "Export orders as CSV"},
            {"choices": ["Alice", "Carol"]}
        ]}),
    )
    .unwrap();
    eventually("the agent took it", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("Export orders as CSV"))
    });
    let prompt = prompts(&user)
        .into_iter()
        .find(|p| p.contains("Export orders as CSV"))
        .unwrap();
    assert!(
        prompt.contains(
            "> Push `feat/42-export` and open the pull request?\n\nYes\nSquash it first.\n\n\
             > Title of the pull request?\n\nExport orders as CSV\n\n> Who reviews it?\n\n\
             Alice, Carol"
        ),
        "{prompt}"
    );
    eventually("no longer open", || open_seqs(&user, &id).is_empty());
    // The answers are kept with the ask they answer.
    let answered = Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .last_agent_events(&id, "agent.answered", 10)
        .unwrap()
        .into_iter()
        .find(|e| e.data["asked"] == seq)
        .expect("the answers kept");
    assert_eq!(
        answered.data["answers"][2]["choices"],
        json!(["Alice", "Carol"])
    );
}

#[test]
fn a_single_question_ask_still_works_as_before() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        json!({"turns": [
            {"when": "colour", "do": [
                {"mcp": {"tool": "ask", "arguments": {"question": "Which colour for the axis?"}}},
                {"say": "Asked."}]},
            {"when": "Blue", "do": [{"say": "Blue it is."}]}
        ]}),
    );
    send(&user, &id, "colour");
    eventually("the ask", || open_seqs(&user, &id).len() == 1);
    eventually("its turn's end", || state(&user, &id) == "idle");
    assert_eq!(open_of(&user, &id), ["Which colour for the axis?"]);
    let seq = open_seqs(&user, &id)[0];

    // A single answer, as `agent answer` gives it for a single question.
    user.call(
        "agent.answer",
        json!({"agent": id, "question": seq, "answers": [{"text": "Blue."}]}),
    )
    .unwrap();
    eventually("the agent took it", || {
        prompts(&user).iter().any(|p| p.contains("Blue."))
    });
    let prompt = prompts(&user)
        .into_iter()
        .find(|p| p.contains("Blue."))
        .unwrap();
    assert!(
        !prompt.contains("> Which colour"),
        "the words alone: {prompt}"
    );
    eventually("no longer open", || open_seqs(&user, &id).is_empty());
}

#[test]
fn a_message_of_the_user_still_answers_every_open_ask() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        json!({"turns": [
            {"when": "ask twice", "do": [ask("Q1: which chart?"), ask("Q2: which colour?"),
                {"say": "Asked."}]},
            {"when": "Bars, blue", "do": [{"say": "Fine."}]}
        ]}),
    );
    send(&user, &id, "ask twice");
    eventually("both asks", || open_seqs(&user, &id).len() == 2);
    eventually("its turn's end", || state(&user, &id) == "idle");
    send(&user, &id, "Bars, blue");
    eventually("none open", || open_seqs(&user, &id).is_empty());
}

#[test]
fn answering_one_ask_leaves_the_other_open() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        json!({"turns": [
            {"when": "ask twice", "do": [ask("Q1: which chart?"), ask("Q2: which colour?"),
                {"say": "Asked."}]},
            {"when": "Bars", "do": [{"say": "Fine."}]}
        ]}),
    );
    send(&user, &id, "ask twice");
    eventually("both asks", || open_seqs(&user, &id).len() == 2);
    eventually("its turn's end", || state(&user, &id) == "idle");
    let seqs = open_seqs(&user, &id);
    user.call(
        "agent.answer",
        json!({"agent": id, "question": seqs[0], "answers": [{"text": "Bars"}]}),
    )
    .unwrap();
    eventually("the agent took it", || {
        state(&user, &id) == "idle" && prompts(&user).iter().any(|p| p.contains("Bars"))
    });
    eventually("the first closed", || open_seqs(&user, &id) == [seqs[1]]);
    assert_eq!(
        root_of_tree(&user, &id)["needs_user"],
        true,
        "the second waits"
    );
}

#[test]
fn an_answer_that_does_not_fit_its_ask_is_refused() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        json!({"turns": [{"when": "ship it", "do": [three_questions(), {"say": "Asked."}]}]}),
    );
    send(&user, &id, "ship it");
    eventually("the ask", || open_seqs(&user, &id).len() == 1);
    let seq = open_seqs(&user, &id)[0];
    let refused = user
        .call(
            "agent.answer",
            json!({"agent": id, "question": seq, "answers": [{"text": "Yes"}]}),
        )
        .unwrap_err();
    assert!(
        refused.1.contains("1 answers for 3 questions"),
        "{refused:?}"
    );
    let refused = user
        .call(
            "agent.answer",
            json!({"agent": id, "question": seq, "answers": [
                {"choices": ["Maybe"]}, {"text": "T"}, {"choices": ["Bob"]}]}),
        )
        .unwrap_err();
    assert!(refused.1.contains("no option Maybe"), "{refused:?}");
}

#[test]
fn a_child_s_several_questions_reach_its_parent_numbered() {
    let user = User::with_fake_agent();
    project(&user);
    user.fake_script(json!({"turns": [
        {"when": "Ship the export.", "do": [three_questions(), {"say": "Asked my parent."}]}
    ]}));
    let (lead, mcp) = lead(&user);
    let answer = mcp.call(
        "agent_launch",
        json!({"title": "Export", "brief": "Ship the export.", "kind": "child"}),
    );
    assert!(!answer.error, "{}", answer.text);
    let question = || {
        Store::open(&user.path("state/flottille/flottille.db"))
            .unwrap()
            .last_deliveries(&lead, 64)
            .unwrap()
            .into_iter()
            .find(|d| d.kind == "child-question")
            .map(|d| d.body)
    };
    eventually("the parent is told", || question().is_some());
    let body = question().unwrap();
    assert!(
        body.contains(
            "1. Push `feat/42-export` and open the pull request?\n   Options: Yes; Not yet \
             (Keep it local)\n2. Title of the pull request?\n3. Who reviews it?\n   Options \
             (several may be chosen): Alice; Bob; Carol"
        ),
        "{body}"
    );
}

#[test]
fn flottille_agent_answer_answers_each_question_of_an_ask() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        json!({"turns": [
            {"when": "ship it", "do": [three_questions(), {"say": "Asked."}]},
            {"when": "Export orders", "do": [{"say": "Shipping."}]}
        ]}),
    );
    send(&user, &id, "ship it");
    eventually("the ask", || open_seqs(&user, &id).len() == 1);
    eventually("its turn's end", || state(&user, &id) == "idle");
    let inbox = user.run(&["inbox"]);
    let listed = String::from_utf8_lossy(&inbox.stdout);
    assert!(
        listed.contains("    2. Title of the pull request?"),
        "{listed}"
    );
    assert!(listed.contains("flottille agent answer"), "{listed}");

    let refused = user.run(&["agent", "answer", &id, "--answer", "Yes"]);
    assert!(!refused.status.success());
    let answered = user.run(&[
        "agent",
        "answer",
        &id,
        "--answer",
        "1=Yes",
        "--answer",
        "2=Export orders",
        "--answer",
        "3=Alice, Carol",
    ]);
    assert!(answered.status.success(), "{answered:?}");
    eventually("the agent took it", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("> Who reviews it?\n\nAlice, Carol"))
    });
    eventually("no longer open", || open_seqs(&user, &id).is_empty());
}

/// The root's turn that asks `ask`, then holds, so that an answer waits under the conversation.
fn asks_and_holds(ask: Value) -> Value {
    json!({"turns": [
        {"when": "go", "do": [ask, {"hold": "release"}, {"say": "Asked."}]},
        {"when": "Blue", "do": [{"say": "Blue it is."}]}
    ]})
}

fn waiting_under(user: &User, id: &str) -> Vec<Value> {
    root_of_tree(user, id)["queued"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn a_waiting_card_answer_is_not_listed_as_a_user_message() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        asks_and_holds(json!({"mcp": {"tool": "ask", "arguments": {"question": "Colour?"}}})),
    );
    send(&user, &id, "go");
    eventually("the ask", || open_seqs(&user, &id).len() == 1);
    let seq = open_seqs(&user, &id)[0];
    let answered = user
        .call(
            "agent.answer",
            json!({"agent": id, "question": seq, "answers": [{"text": "Blue"}]}),
        )
        .unwrap();
    let waiting = answered["delivery"].as_str().unwrap().to_owned();

    // The answer waits for the turn's end, and the card says so: it is not a message of the
    // user to cancel, edit or send now.
    let stored = Store::open(&user.path("state/flottille/flottille.db"))
        .unwrap()
        .deliveries(&id, "queued")
        .unwrap();
    assert!(stored.iter().any(|d| d.id == waiting), "it waits");
    let listed = waiting_under(&user, &id);
    assert!(
        listed
            .iter()
            .all(|queued| queued["delivery"] != waiting.as_str()),
        "{listed:?}"
    );
    release(&user);
}

#[test]
fn a_withdrawn_answer_lets_the_question_be_answered_again() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        asks_and_holds(json!({"mcp": {"tool": "ask", "arguments": {"question": "Colour?"}}})),
    );
    send(&user, &id, "go");
    eventually("the ask", || open_seqs(&user, &id).len() == 1);
    let seq = open_seqs(&user, &id)[0];
    let first = user
        .call(
            "agent.answer",
            json!({"agent": id, "question": seq, "answers": [{"text": "Red"}]}),
        )
        .unwrap();
    user.call(
        "agent.unqueue",
        json!({"agent": id, "delivery": first["delivery"]}),
    )
    .unwrap();

    user.call(
        "agent.answer",
        json!({"agent": id, "question": seq, "answers": [{"text": "Blue"}]}),
    )
    .expect("the question is still open to an answer");
    release(&user);
    eventually("the agent took the second", || {
        prompts(&user).iter().any(|p| p.contains("Blue"))
    });
    assert!(prompts(&user).iter().all(|p| !p.contains("Red")));
    eventually("no longer open", || open_seqs(&user, &id).is_empty());
}

#[test]
fn a_several_question_ask_moved_to_the_user_keeps_its_questions_and_options() {
    let user = User::with_fake_agent();
    let (_, (session, _)) = asking_with(
        &user,
        "led",
        "full",
        asks_then_works_with(three_questions()),
    );
    user.call("agent.detach", json!({"agent": session}))
        .unwrap();

    eventually("the ask moved to the user", || {
        !open_seqs(&user, &session).is_empty()
    });
    let entry = root_of_tree(&user, &session)["questions"][0].clone();
    let questions = entry["questions"].as_array().unwrap();
    assert_eq!(questions.len(), 3, "{entry}");
    assert_eq!(questions[0]["options"][1]["label"], "Not yet", "{entry}");
    assert_eq!(questions[2]["multiple"], true, "{entry}");
    release(&user);
}

#[test]
fn a_second_answer_to_an_ask_whose_answer_waits_is_refused() {
    let user = User::with_fake_agent();
    let (id, _) = root(
        &user,
        asks_and_holds(json!({"mcp": {"tool": "ask", "arguments": {"question": "Colour?"}}})),
    );
    send(&user, &id, "go");
    eventually("the ask", || open_seqs(&user, &id).len() == 1);
    let seq = open_seqs(&user, &id)[0];
    let answer = |text: &str| {
        user.call(
            "agent.answer",
            json!({"agent": id, "question": seq, "answers": [{"text": text}]}),
        )
    };
    answer("Blue").unwrap();
    let refused = answer("Red").unwrap_err();
    assert!(refused.1.contains("answered already"), "{refused:?}");
    release(&user);
}
