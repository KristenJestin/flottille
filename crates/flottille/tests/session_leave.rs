//! An agent writes to another session of its project with the user's leave, given once per pair
//! (#463, `docs/tools.md`, "Sessions that talk to each other"): the first message from A to X is
//! held for the user in every mode, Full included; once allowed, A writes to X as to a session it
//! started, until either session ends or the user withdraws it. Against the real daemon with the
//! fake ACP agent, throwaway repositories and temporary XDG folders.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A user whose agents answer every message, with a judge allowing every call.
fn user() -> User {
    let user = User::with_fake_agent().with_allowing_judge();
    project(&user, "app");
    user
}

/// A project over a throwaway repository.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

/// A session the user starts in the project, its first turn ended: its id, and the address and
/// token of its MCP server.
fn started(user: &User, project: &str) -> (String, String, String) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call("agent.run", json!({"project": project, "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    idle(user, &id);
    (id, url, authorization)
}

/// A session the user starts, and its MCP session.
fn root(user: &User, project: &str) -> (String, Mcp) {
    let (id, url, authorization) = started(user, project);
    (id, Mcp::initialize(&url, &authorization))
}

fn idle(user: &User, agent: &str) {
    eventually("the turn's end", || {
        user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"] == "idle"
    });
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    user.call("agent.mode", json!({"agent": agent, "mode": mode}))
        .unwrap();
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

fn judge_prompts(user: &User) -> usize {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .count()
}

/// A call held for the user: its request's number.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// The user answers request `number`: the text the agent is handed for it.
fn answer(user: &User, number: u64, method: &str) -> String {
    user.call(method, json!({"number": number})).unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

/// The messages other sessions wrote to `agent`, oldest last.
fn written_to(user: &User, agent: &str) -> Vec<String> {
    store(user)
        .last_deliveries(agent, 32)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == "session")
        .map(|delivery| delivery.body)
        .collect()
}

fn send(from: &Mcp, to: &str, text: &str) -> Answer {
    from.call("session_send", json!({"session": to, "text": text}))
}

/// A sent message: answered at once, delivered, no request made.
fn sent(user: &User, answer: Answer, to: &str, text: &str) {
    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.starts_with("Sent to "), "{}", answer.text);
    eventually("the message in the session", || {
        written_to(user, to).iter().any(|body| body.contains(text))
    });
}

/// The leaves the user gave that name `agent`, as `leave.list` lists them: `(writer, session)`.
fn leaves(user: &User, agent: &str) -> Vec<(String, String)> {
    user.call("leave.list", json!({"session": agent})).unwrap()["leaves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|leave| {
            (
                leave["writer"]["id"].as_str().unwrap().to_owned(),
                leave["session"]["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// A first send from `a` to `x`, held and allowed by the user, the text delivered.
fn allowed_pair(user: &User, a: &Mcp, x: &str) {
    let number = held(&send(a, x, "First."));
    let done = answer(user, number, "approval.approve");
    assert!(done.contains("and Flottille has now done it"), "{done}");
    eventually("the first message", || {
        written_to(user, x)
            .iter()
            .any(|body| body.contains("First."))
    });
}

#[test]
fn a_first_send_to_a_session_it_did_not_start_asks_the_user_once() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    let judged = judge_prompts(&user);

    let number = held(&send(&mcp, &x, "The login is fixed in main."));

    let listed = requests(&user);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let request = &listed[0];
    assert_eq!(request["number"], number);
    assert_eq!(request["kind"], "action", "{request}");
    let says = request["says"].to_string();
    assert!(
        says.contains(&format!(
            "Let the session agent {a} write to the session agent {x} (started by you, not by \
             this agent)"
        )) && says.contains("Its first message: The login is fixed in main."),
        "{says}"
    );
    assert_eq!(judge_prompts(&user), judged, "the judge was asked");
    assert!(written_to(&user, &x).is_empty());

    // Asked again while the user has not answered: no second request.
    let again = send(&mcp, &x, "Also the logout.");
    assert!(again.error, "{}", again.text);
    assert!(
        again.text.starts_with(&format!(
            "refused: request #{number} already asks the user to let you write to agent {x}"
        )),
        "{}",
        again.text
    );
    assert_eq!(requests(&user).len(), 1);

    let done = answer(&user, number, "approval.approve");

    assert!(done.contains("and Flottille has now done it"), "{done}");
    eventually("the message in the session", || {
        !written_to(&user, &x).is_empty()
    });
    let body = written_to(&user, &x).remove(0);
    assert!(
        body.starts_with(&format!("agent {a} wrote to this session:"))
            && body.contains("The login is fixed in main."),
        "{body}"
    );
    assert_eq!(leaves(&user, &x), [(a.clone(), x.clone())]);
}

#[test]
fn two_first_sends_at_once_ask_the_user_once() {
    let user = user();
    let (_, url, authorization) = started(&user, "app");
    let (x, _) = root(&user, "app");

    let answers: Vec<Answer> = std::thread::scope(|scope| {
        let calls: Vec<_> = ["One.", "Two."]
            .into_iter()
            .map(|text| {
                let (url, authorization, x) = (&url, &authorization, &x);
                scope.spawn(move || send(&Mcp::initialize(url, authorization), x, text))
            })
            .collect();
        calls.into_iter().map(|call| call.join().unwrap()).collect()
    });

    let held_ones = answers
        .iter()
        .filter(|answer| answer.text.starts_with("Waiting for the user's approval"))
        .count();
    assert_eq!(held_ones, 1, "{answers:?}");
    assert_eq!(requests(&user).len(), 1);
    assert!(written_to(&user, &x).is_empty());
}

#[test]
fn once_allowed_the_pair_writes_without_asking_again() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    allowed_pair(&user, &mcp, &x);
    let judged = judge_prompts(&user);

    // In Auto, judged as a write is: the judge allows it.
    sent(&user, send(&mcp, &x, "Second."), &x, "Second.");

    assert!(judge_prompts(&user) > judged, "the judge was not asked");
    assert!(requests(&user).is_empty());

    // In Ask, a card as for any write, naming the leave.
    set_mode(&user, &a, "ask");
    let number = held(&send(&mcp, &x, "Third."));
    let request = requests(&user).remove(0);
    assert_eq!(request["class"], "judge", "{request}");
    assert!(
        request["says"].to_string().contains(&format!(
            "Send to the session agent {x}, which the user let you write to: Third."
        )),
        "{request}"
    );
    answer(&user, number, "approval.approve");
    eventually("the third message", || {
        written_to(&user, &x)
            .iter()
            .any(|body| body.contains("Third."))
    });
}

#[test]
fn the_leave_is_one_way() {
    let user = user();
    let (a, a_mcp) = root(&user, "app");
    let (x, x_mcp) = root(&user, "app");
    allowed_pair(&user, &a_mcp, &x);

    let back = send(&x_mcp, &a, "Thanks.");

    let number = held(&back);
    let request = requests(&user).remove(0);
    assert_eq!(request["number"], number);
    assert!(
        request["says"].to_string().contains(&format!(
            "Let the session agent {x} write to the session agent {a}"
        )),
        "{request}"
    );
    assert!(written_to(&user, &a).is_empty());
}

#[test]
fn a_refused_leave_sends_nothing_and_says_so() {
    let user = user();
    let (_, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    let number = held(&send(&mcp, &x, "Hello."));

    let said = answer(&user, number, "approval.deny");

    assert!(said.contains("was refused by the user"), "{said}");
    assert!(said.contains("Nothing was done"), "{said}");
    assert!(written_to(&user, &x).is_empty());
    assert!(leaves(&user, &x).is_empty());
    // No leave was given: the next one asks again.
    held(&send(&mcp, &x, "Hello again."));
}

#[test]
fn the_leave_is_asked_in_full_mode_too() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    set_mode(&user, &a, "full");

    let number = held(&send(&mcp, &x, "Hello."));

    assert!(written_to(&user, &x).is_empty());
    answer(&user, number, "approval.approve");
    eventually("the first message", || {
        written_to(&user, &x)
            .iter()
            .any(|body| body.contains("Hello."))
    });
    // Afterwards, Full lets the writes through as it does any.
    sent(&user, send(&mcp, &x, "Second."), &x, "Second.");
    assert!(requests(&user).is_empty());
}

#[test]
fn the_leave_ends_with_either_session() {
    let user = user();
    let (a, a_mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    allowed_pair(&user, &a_mcp, &x);

    // The session written to ends, and is resumed: the leave went with its end.
    user.call("agent.stop", json!({"agent": x})).unwrap();
    assert!(leaves(&user, &x).is_empty());
    user.resume(&x);
    held(&send(&a_mcp, &x, "Still there?"));

    // The writer ends: its leave goes too.
    let (y, _) = root(&user, "app");
    let (b, b_mcp) = root(&user, "app");
    allowed_pair(&user, &b_mcp, &y);
    assert_eq!(leaves(&user, &y), [(b.clone(), y.clone())]);
    user.call("agent.stop", json!({"agent": b})).unwrap();
    assert!(leaves(&user, &y).is_empty());
    assert!(leaves(&user, &a).is_empty());
}

#[test]
fn a_withdrawn_leave_is_asked_again() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    allowed_pair(&user, &mcp, &x);

    let withdrawn = user
        .call("leave.withdraw", json!({"writer": a, "session": x}))
        .unwrap();

    assert_eq!(
        withdrawn["leave"]["writer"]["id"],
        a.as_str(),
        "{withdrawn}"
    );
    assert!(leaves(&user, &x).is_empty());
    let events = store(&user).events_after(0, 10_000).unwrap();
    assert!(
        events.iter().any(|e| e.name == "leave.granted"),
        "no leave.granted"
    );
    assert!(
        events.iter().any(|e| e.name == "leave.withdrawn"),
        "no leave.withdrawn"
    );
    held(&send(&mcp, &x, "Again."));
    let refused = user
        .call("leave.withdraw", json!({"writer": a, "session": x}))
        .unwrap_err();
    assert_eq!(refused.0, "not-found", "{refused:?}");
}

#[test]
fn a_child_or_another_project_s_session_is_still_refused() {
    let user = user();
    project(&user, "other");
    let (_, mcp) = root(&user, "app");
    let (x, x_mcp) = root(&user, "app");
    let (elsewhere, _) = root(&user, "other");
    let launched = x_mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let child = store(&user)
        .agents()
        .unwrap()
        .into_iter()
        .find(|agent| agent.parent.as_deref() == Some(x.as_str()))
        .unwrap()
        .id;
    let judged = judge_prompts(&user);

    let to_child = send(&mcp, &child, "Hello.");
    let to_elsewhere = send(&mcp, &elsewhere, "Hello.");

    assert!(to_child.error, "{}", to_child.text);
    assert!(
        to_child.text.starts_with(&format!(
            "refused: {child} is a child of another session: it is written to by its parent"
        )),
        "{}",
        to_child.text
    );
    assert!(to_elsewhere.error, "{}", to_elsewhere.text);
    assert!(
        to_elsewhere.text.starts_with(&format!(
            "refused: {elsewhere} is a session of another project, other, not of app"
        )),
        "{}",
        to_elsewhere.text
    );
    assert!(requests(&user).is_empty());
    assert_eq!(judge_prompts(&user), judged, "the judge was asked");
    assert!(written_to(&user, &child).is_empty());
    assert!(written_to(&user, &elsewhere).is_empty());
}

#[test]
fn a_started_session_needs_no_leave() {
    let user = user();
    let (a, url, authorization) = started(&user, "app");
    let mcp = Mcp::initialize(&url, &authorization);
    let opened = user.received_requests("session/new").len();
    let launched = mcp.call(
        "agent_launch",
        json!({"title": "Login", "brief": "Fix the login.", "kind": "handed"}),
    );
    assert!(
        launched.text.contains("Started a session"),
        "{}",
        launched.text
    );
    let (url, authorization) = server_of(&user, opened);
    let session_mcp = Mcp::initialize(&url, &authorization);
    let session = store(&user)
        .agents()
        .unwrap()
        .into_iter()
        .find(|agent| agent.title.as_deref() == Some("Login"))
        .unwrap()
        .id;

    sent(
        &user,
        send(&mcp, &session, "Also the logout."),
        &session,
        "logout",
    );
    sent(&user, send(&session_mcp, &a, "Done."), &a, "Done.");

    assert!(requests(&user).is_empty());
    assert!(leaves(&user, &a).is_empty());
}

#[test]
fn the_description_of_session_send_says_the_leave() {
    let user = user();
    let (_, mcp) = root(&user, "app");

    let tools = mcp.tools();

    let description = tools
        .iter()
        .find(|tool| tool["name"] == "session_send")
        .unwrap()["description"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        description.contains("any other session of your project")
            && description.contains("the user's leave"),
        "{description}"
    );
}

#[test]
fn a_leave_withdrawn_while_a_send_is_judged_stops_it() {
    let user = User::with_fake_agent().with_judge(json!([
        {"wait": 1500},
        {"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}
    ]));
    project(&user, "app");
    let (a, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    allowed_pair(&user, &mcp, &x);

    let judged = std::thread::scope(|scope| {
        let sending = scope.spawn(|| send(&mcp, &x, "Second."));
        eventually("the judge asked", || judge_prompts(&user) > 0);
        user.call("leave.withdraw", json!({"writer": a, "session": x}))
            .unwrap();
        sending.join().unwrap()
    });

    assert!(judged.error, "{}", judged.text);
    assert!(
        judged.text.contains("withdrew your leave"),
        "{}",
        judged.text
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !written_to(&user, &x)
            .iter()
            .any(|body| body.contains("Second.")),
        "delivered after the withdrawal"
    );
}

#[test]
fn an_unknown_id_an_ended_session_or_an_empty_text_is_refused_before_anything_is_asked() {
    let user = user();
    let (_, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    let (ended, _) = root(&user, "app");
    user.call("agent.stop", json!({"agent": ended})).unwrap();
    let judged = judge_prompts(&user);

    let unknown = send(&mcp, "000000000000", "Hello.");
    let to_ended = send(&mcp, &ended, "Hello.");
    let empty = send(&mcp, &x, "  ");

    for (answer, says) in [
        (&unknown, ["refused: 000000000000 names no session", ""]),
        (&to_ended, ["refused: ", " has ended"]),
        (&empty, ["refused: the text is empty", ""]),
    ] {
        assert!(answer.error, "{}", answer.text);
        assert!(answer.text.starts_with(says[0]), "{}", answer.text);
        assert!(answer.text.contains(says[1]), "{}", answer.text);
    }
    assert!(to_ended.text.contains(&ended), "{}", to_ended.text);
    assert!(requests(&user).is_empty());
    assert_eq!(judge_prompts(&user), judged, "the judge was asked");
    assert!(written_to(&user, &x).is_empty());
    assert!(written_to(&user, &ended).is_empty());
}

#[test]
fn the_writer_resumed_asks_again() {
    let user = user();
    let (a, a_mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    allowed_pair(&user, &a_mcp, &x);
    drop(a_mcp);

    user.call("agent.stop", json!({"agent": a})).unwrap();
    let reopened = user.received_requests("session/resume").len()
        + user.received_requests("session/load").len();
    user.resume(&a);
    user.call("agent.send", json!({"agent": a, "message": "again"}))
        .unwrap();
    let opened = || {
        let mut opened = user.received_requests("session/resume");
        opened.extend(user.received_requests("session/load"));
        opened
    };
    eventually("the session reopened", || opened().len() > reopened);
    let opened = opened();
    let server = &opened.last().unwrap()["params"]["mcpServers"][0];
    let authorization = server["headers"][0]["value"].as_str().unwrap();
    let mcp = Mcp::initialize(server["url"].as_str().unwrap(), authorization);

    held(&send(&mcp, &x, "Back."));

    assert!(leaves(&user, &x).is_empty());
    assert!(
        !written_to(&user, &x)
            .iter()
            .any(|body| body.contains("Back."))
    );
}

#[test]
fn a_send_allowed_after_its_session_ended_says_it_ended() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    allowed_pair(&user, &mcp, &x);
    set_mode(&user, &a, "ask");
    let number = held(&send(&mcp, &x, "Third."));
    user.call("agent.stop", json!({"agent": x})).unwrap();

    let said = answer(&user, number, "approval.approve");

    assert!(said.contains("was not executed"), "{said}");
    assert!(said.contains(&x) && said.contains(" has ended"), "{said}");
    assert!(!said.contains("situation changed"), "{said}");
}
