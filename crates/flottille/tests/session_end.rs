//! An agent asks to end a session of its project it did not start as a child (#475,
//! `docs/tools.md`, "Sessions that talk to each other", `agent_stop`): held for the user every
//! time, in every mode, Full included, and ended as the window's "End session" ends it once
//! allowed. Against the real daemon with the fake ACP agent, throwaway repositories and temporary
//! XDG folders.

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

/// A session the user starts in the project, its first turn ended: its id and its MCP session.
fn root(user: &User, project: &str) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call("agent.run", json!({"project": project, "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    idle(user, &id);
    (id, Mcp::initialize(&url, &authorization))
}

/// What `mcp` launches of `kind`, titled `title`: the launched agent's id, its first turn ended.
fn launched(user: &User, mcp: &Mcp, kind: &str, title: &str) -> String {
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": kind, "title": title, "brief": "Fix the login."}),
    );
    assert!(!answer.error, "{}", answer.text);
    eventually("the launched agent", || {
        store(user)
            .agents()
            .unwrap()
            .iter()
            .any(|agent| agent.title.as_deref() == Some(title))
    });
    let id = store(user)
        .agents()
        .unwrap()
        .into_iter()
        .find(|agent| agent.title.as_deref() == Some(title))
        .unwrap()
        .id;
    idle(user, &id);
    id
}

fn shown(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone()
}

fn idle(user: &User, agent: &str) {
    eventually("the turn's end", || shown(user, agent)["state"] == "idle");
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

fn stop(mcp: &Mcp, agent: &str) -> Answer {
    mcp.call("agent_stop", json!({"agent_id": agent}))
}

fn ended(user: &User, agent: &str) {
    eventually("the session's end", || {
        shown(user, agent)["state"] == "ended"
    });
}

#[test]
fn an_agent_asks_to_end_a_session_it_launched_for_the_user_and_the_user_allows_it() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let session = launched(&user, &mcp, "handed", "Login");
    let judged = judge_prompts(&user);

    let number = held(&stop(&mcp, &session));

    let listed = requests(&user);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let says = listed[0]["says"].to_string();
    assert!(
        says.contains(&format!(
            "End the session \\\"Login\\\" (agent {session}), in main, started by this agent"
        )) && says.contains(
            "its tree, its runs and what was queued for it are stopped; its workspace stays"
        ),
        "{says}"
    );
    assert_eq!(judge_prompts(&user), judged, "the judge was asked");
    assert_eq!(shown(&user, &session)["state"], "idle");

    let done = answer(&user, number, "approval.approve");

    assert!(done.contains("and Flottille has now done it"), "{done}");
    assert!(done.contains("Ended the session"), "{done}");
    ended(&user, &session);
    assert_eq!(shown(&user, &a)["state"], "idle");

    let description = mcp
        .tools()
        .iter()
        .find(|tool| tool["name"] == "agent_stop")
        .unwrap()["description"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        description.contains("any other session of your project")
            && description.contains("every time"),
        "{description}"
    );
}

#[test]
fn ending_a_session_the_agent_did_not_start_is_held_every_time_even_in_full() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    set_mode(&user, &a, "full");
    let judged = judge_prompts(&user);

    let first = held(&stop(&mcp, &x));
    let says = requests(&user)[0]["says"].to_string();
    assert!(
        says.contains(&format!(
            "End the session agent {x}, in main, started by you, not by this agent"
        )),
        "{says}"
    );
    answer(&user, first, "approval.approve");
    ended(&user, &x);

    // No leave is kept: another session is asked for again, in Full too.
    let (y, _) = root(&user, "app");
    let second = held(&stop(&mcp, &y));
    assert!(second > first);
    answer(&user, second, "approval.approve");
    ended(&user, &y);
    assert_eq!(judge_prompts(&user), judged, "the judge was asked");
}

#[test]
fn a_refused_end_ends_nothing_and_says_so() {
    let user = user();
    let (_, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    let number = held(&stop(&mcp, &x));

    let said = answer(&user, number, "approval.deny");

    assert!(said.contains("was refused by the user"), "{said}");
    assert!(said.contains("Nothing was done"), "{said}");
    assert_eq!(shown(&user, &x)["state"], "idle");
    // Nothing is kept of a refusal either: asking again asks the user again.
    held(&stop(&mcp, &x));
}

#[test]
fn ending_a_session_of_another_project_or_itself_is_refused_before_asking() {
    let user = user();
    project(&user, "other");
    let (a, mcp) = root(&user, "app");
    let (_, x_mcp) = root(&user, "app");
    let (elsewhere, _) = root(&user, "other");
    let child = launched(&user, &x_mcp, "child", "worker");
    let judged = judge_prompts(&user);

    for (id, refusal) in [
        (
            a.clone(),
            format!("refused: {a} is your own session: agent_stop ends another"),
        ),
        (
            elsewhere.clone(),
            format!("refused: {elsewhere} is a session of another project, other, not of app"),
        ),
        (
            child.clone(),
            format!("refused: {child} is a child of another session: it is stopped by its parent"),
        ),
        (
            "000000000000".to_owned(),
            "refused: no session of your project has the id or the title \"000000000000\""
                .to_owned(),
        ),
    ] {
        let refused = stop(&mcp, &id);
        assert!(refused.error, "{}", refused.text);
        assert!(refused.text.starts_with(&refusal), "{}", refused.text);
    }

    assert!(requests(&user).is_empty());
    assert_eq!(judge_prompts(&user), judged, "the judge was asked");
    for id in [&a, &elsewhere, &child] {
        assert_ne!(shown(&user, id)["state"], "ended", "{id}");
    }
}

#[test]
fn ending_its_own_child_asks_nothing_as_today() {
    let user = user();
    let (_, mcp) = root(&user, "app");
    let child = launched(&user, &mcp, "child", "worker");

    let stopped = stop(&mcp, &child);

    assert!(!stopped.error, "{}", stopped.text);
    assert!(stopped.text.starts_with("Stopped "), "{}", stopped.text);
    assert!(requests(&user).is_empty());
    ended(&user, &child);
    assert_eq!(
        shown(&user, &child)["state_reason"],
        "stopped by its parent"
    );
}

#[test]
fn an_ended_session_says_who_asked_and_that_the_user_allowed_it() {
    let user = user();
    let (a, mcp) = root(&user, "app");
    let (x, _) = root(&user, "app");
    let number = held(&stop(&mcp, &x));

    answer(&user, number, "approval.approve");

    ended(&user, &x);
    assert_eq!(
        shown(&user, &x)["state_reason"],
        format!("ended at the request of the session agent {a}, allowed by you")
    );
}

#[test]
fn a_lead_stops_a_led_session_it_started_without_asking_the_user() {
    for mode in ["full", "auto"] {
        let user = user();
        let (a, mcp) = root(&user, "app");
        set_mode(&user, &a, mode);
        let session = launched(&user, &mcp, "led", "Login");
        let judged = judge_prompts(&user);

        let stopped = stop(&mcp, &session);

        assert!(!stopped.error, "{mode}: {}", stopped.text);
        assert!(
            stopped
                .text
                .starts_with(&format!("Ended the session \"Login\" (agent {session})")),
            "{mode}: {}",
            stopped.text
        );
        assert!(requests(&user).is_empty(), "{mode}: {:?}", requests(&user));
        ended(&user, &session);
        assert_eq!(
            shown(&user, &session)["state_reason"],
            format!("ended by the session that led it, agent {a}"),
            "{mode}"
        );
        // Full lets it through; Auto has the judge decide it, as any call of the tree's.
        let asked = judge_prompts(&user) - judged;
        match mode {
            "full" => assert_eq!(asked, 0, "the judge was asked in Full"),
            _ => assert_eq!(asked, 1, "the judge was not asked in Auto"),
        }
        assert_eq!(shown(&user, &a)["state"], "idle");
    }
}

#[test]
fn an_agent_stopping_a_session_the_user_opened_is_held_for_the_user_every_time() {
    for mode in ["full", "auto"] {
        let user = user();
        let (a, mcp) = root(&user, "app");
        let (x, _) = root(&user, "app");
        set_mode(&user, &a, mode);
        let judged = judge_prompts(&user);

        let first = held(&stop(&mcp, &x));
        let says = requests(&user)[0]["says"].to_string();
        assert!(
            says.contains(&format!(
                "End the session agent {x}, in main, started by you, not by this agent"
            )),
            "{mode}: {says}"
        );
        answer(&user, first, "approval.deny");
        assert_eq!(shown(&user, &x)["state"], "idle", "{mode}");

        let second = held(&stop(&mcp, &x));
        assert!(second > first, "{mode}");
        assert_eq!(judge_prompts(&user), judged, "{mode}: the judge was asked");
    }
}

#[test]
fn an_agent_stopping_a_handed_session_is_held_for_the_user_every_time() {
    for mode in ["full", "auto"] {
        let user = user();
        let (a, mcp) = root(&user, "app");
        set_mode(&user, &a, mode);
        let session = launched(&user, &mcp, "handed", "Login");
        let judged = judge_prompts(&user);

        let first = held(&stop(&mcp, &session));
        let says = requests(&user)[0]["says"].to_string();
        assert!(
            says.contains(&format!(
                "End the session \\\"Login\\\" (agent {session}), in main, started by this agent \
                 and handed to you: yours"
            )),
            "{mode}: {says}"
        );
        answer(&user, first, "approval.deny");
        assert_eq!(shown(&user, &session)["state"], "idle", "{mode}");

        let second = held(&stop(&mcp, &session));
        assert!(second > first, "{mode}");
        assert_eq!(judge_prompts(&user), judged, "{mode}: the judge was asked");
    }
}

#[test]
fn a_session_handed_while_the_judge_decides_is_not_ended() {
    // The judge allows once the test releases it.
    let user = User::with_fake_agent().with_judge(json!([
        {"hold": "release"},
        {"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}
    ]));
    project(&user, "app");
    let release = user.path("fake-judge/release");
    std::fs::write(&release, "").unwrap();
    let (a, mcp) = root(&user, "app");
    set_mode(&user, &a, "auto");
    let session = launched(&user, &mcp, "led", "Login");
    std::fs::remove_file(&release).unwrap();
    let judged = judge_prompts(&user);

    // While the judge decides, the user takes the session over.
    let stopped = std::thread::scope(|scope| {
        scope.spawn(|| {
            eventually("the judgement", || judge_prompts(&user) > judged);
            user.call("agent.detach", json!({"agent": session}))
                .unwrap();
            std::fs::write(&release, "").unwrap();
        });
        stop(&mcp, &session)
    });

    assert!(stopped.error, "{}", stopped.text);
    assert!(stopped.text.contains("no longer leads"), "{}", stopped.text);
    assert_eq!(shown(&user, &session)["state"], "idle");
    assert!(requests(&user).is_empty(), "{:?}", requests(&user));
}
