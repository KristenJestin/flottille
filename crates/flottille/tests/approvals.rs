//! The scenarios of the gate's requests (`docs/design.md` §7): a held call answered at once, the
//! user's answers from the command line, and the result stored for the agent that asked. The
//! agent is the fake ACP agent, in a throwaway repository and temporary XDG folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::approval::{self, Delivery, Request, RequestState};
use flottille_core::store::Store;
use serde_json::{Value, json};

const WAITING: &str = "This action has not happened; do not assume it did. Continue with work \
    that does not depend on it, or end your turn saying you wait. The answer will be handed to \
    you when the user gives it.";

/// A project `app` over a throwaway repository with a `.env`, its workspace `w` (where the
/// `.env` is copied), and the session of an agent working there: the agent's id, its session,
/// the workspace's folder.
fn session(user: &User) -> (String, Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), "DB_PASSWORD=hunter2-hunter2\n").unwrap();
    user.git(&folder, &["add", "README.md", ".gitignore"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\n[[setup]]\ncopy = [\".env\"]\n");
    std::fs::write(&file, text).unwrap();
    let workspace = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    let workspace = PathBuf::from(workspace["folder"].as_str().unwrap());
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "hello"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization), workspace)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The number of the request a held call's answer names, checking the answer is the waiting
/// text, not an error.
fn held(answer: Answer) -> u64 {
    assert!(!answer.error, "an error: {}", answer.text);
    let rest = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    let (number, after) = rest.split_once('.').unwrap();
    assert_eq!(after.trim_start(), WAITING);
    assert!(answer.text.chars().count() < 2_000);
    number.parse().unwrap()
}

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
}

/// The user's pending requests, as `flottille approvals --json` gives them.
fn approvals(user: &User) -> Vec<Value> {
    let output = user.run(&["approvals", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    listed["requests"].as_array().unwrap().clone()
}

/// A command line of the user's: its standard output, or its error.
fn cli(user: &User, args: &[&str]) -> Result<String, String> {
    let output = user.run(args);
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// The answer stored for the agent once the request is settled.
fn settled(user: &User, number: u64) -> Delivery {
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap()
}

#[test]
fn a_held_call_answers_at_once_with_the_waiting_text() {
    let user = User::with_fake_agent();
    let (id, mcp, _) = session(&user);
    let watch = user.watch(&["--after", "0"]);

    let started = Instant::now();
    let number = held(mcp.call("fs_read", json!({"path": ".env"})));

    assert!(started.elapsed() < Duration::from_secs(5));
    let listed = approvals(&user);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["number"], number);
    assert_eq!(listed[0]["agent"], id.as_str());
    assert_eq!(listed[0]["tool"], "fs_read");
    assert_eq!(listed[0]["call"], json!({"path": ".env"}));
    assert_eq!(listed[0]["class"], "base");
    assert_eq!(listed[0]["held_by"], "user");
    assert_eq!(
        listed[0]["reasons"],
        json!([".env is a sensitive place (.env)"])
    );
    let requested = watch.until(|e| e["name"] == "approval.requested");
    assert_eq!(requested.last().unwrap()["data"]["number"], number);
    assert_eq!(user.status()["agents"]["waiting_for_user"], 1);
    // The text form names the request, its call and why.
    let text = cli(&user, &["approvals"]).unwrap();
    assert!(text.contains(&format!("#{number}")), "{text}");
    assert!(text.contains("fs_read"), "{text}");
    assert!(text.contains("sensitive place"), "{text}");
}

#[test]
fn approving_it_performs_the_action_and_stores_the_result() {
    let user = User::with_fake_agent();
    let (id, mcp, _) = session(&user);
    let elsewhere = user.path("elsewhere/note.txt");
    let number = held(mcp.call(
        "fs_write",
        json!({"path": elsewhere.to_str().unwrap(), "content": "written\n"}),
    ));
    assert!(!elsewhere.exists());

    let said = cli(&user, &["approve", &number.to_string()]).unwrap();

    assert!(said.contains(&format!("#{number}")), "{said}");
    let delivery = settled(&user, number);
    assert_eq!(delivery.agent, id);
    assert_eq!(delivery.kind, "approval");
    assert!(
        delivery.body.starts_with(&format!(
            "Request #{number} (fs_write) was approved by the user, and Flottille has now done \
             it. Its result:\n\n"
        )),
        "{}",
        delivery.body
    );
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "written\n");
    assert!(approvals(&user).is_empty());
    let request = store(&user).request(number).unwrap().unwrap();
    assert_eq!(request.state, RequestState::Allowed);

    // A sensitive read: its result is masked like every result.
    let number = held(mcp.call("fs_read", json!({"path": ".env"})));
    cli(&user, &["approve", &number.to_string()]).unwrap();
    let delivery = settled(&user, number);
    assert!(delivery.body.contains("DB_PASSWORD="), "{}", delivery.body);
    assert!(!delivery.body.contains("hunter2"), "{}", delivery.body);
    let events = user
        .watch(&["--after", "0"])
        .until(|e| e["name"] == "approval.settled" && e["data"]["number"] == number);
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "approval.decided" && e["data"]["decision"] == "once")
    );
}

#[test]
fn denying_it_stores_the_refusal_for_the_agent() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);
    let number = held(mcp.call("run", json!({"line": "cat /etc/hostname"})));

    cli(&user, &["deny", &number.to_string()]).unwrap();

    assert_eq!(
        settled(&user, number).body,
        format!("Request #{number} (run) was refused by the user. Nothing was done.")
    );
    assert!(approvals(&user).is_empty());
    let error = cli(&user, &["approve", &number.to_string()]).unwrap_err();
    assert!(error.contains("already"), "{error}");
}

#[test]
fn the_same_call_key_asked_twice_gives_the_same_request() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);

    let first = held(mcp.call_keyed("fs_read", json!({"path": ".env"}), "toolu_01"));
    let again = held(mcp.call_keyed("fs_read", json!({"path": ".env"}), "toolu_01"));
    let other = held(mcp.call_keyed("fs_read", json!({"path": ".env"}), "toolu_02"));

    assert_eq!(first, again);
    assert_ne!(first, other);
    assert_eq!(approvals(&user).len(), 2);
}

#[test]
fn an_approval_allows_that_call_once_and_the_same_call_asks_again() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);
    std::fs::write(user.path("shared.txt"), "shared\n").unwrap();
    let line = format!("cat {}", user.path("shared.txt").display());
    let number = held(mcp.call("run", json!({"line": line})));

    let said = cli(&user, &["approve", &number.to_string()]).unwrap();
    assert!(said.contains("is allowed"), "{said}");
    assert!(
        settled(&user, number)
            .body
            .contains("and Flottille has now done it"),
    );

    // Nothing is remembered: the same call asks again.
    let next = held(mcp.call("run", json!({"line": line})));
    assert_ne!(next, number);
    let error = cli(&user, &["approve", &next.to_string(), "--for-tree"]).unwrap_err();
    assert!(
        error.contains("--for-tree"),
        "no answer for the tree: {error}"
    );
}

#[test]
fn an_allowed_request_is_checked_again_before_it_is_performed() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);
    std::fs::write(user.path("shared.txt"), "shared\n").unwrap();
    let line = format!("cat {}", user.path("shared.txt").display());
    let number = held(mcp.call("run", json!({"line": line})));
    // Meanwhile the project comes to never allow it.
    let file = user.project_file("app");
    let text: String = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("never"))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&file, format!("never = [[\"cat\"]]\n{text}")).unwrap();

    cli(&user, &["approve", &number.to_string()]).unwrap();

    let body = settled(&user, number).body;
    assert!(
        body.starts_with(&format!(
            "Request #{number} (run) was not executed: the situation changed since you \
             allowed it: the project's never list holds cat"
        )),
        "{body}"
    );
}

#[test]
fn a_request_held_by_an_agent_is_not_the_user_s_until_it_is_forwarded_to_them() {
    let user = User::with_fake_agent();
    let (id, _, workspace) = session(&user);
    // Until the tree (#7) every agent is a root, whose requests are the user's: a child's
    // request, held by its parent, is written as #7 will write it.
    let (request, _) = store(&user)
        .ask(&Request {
            number: 0,
            agent: id.clone(),
            call_key: None,
            tool: "fs_write".into(),
            arguments: json!({"path": "/tmp/flottille-never-written", "content": "x"}),
            shown: json!({"path": "/tmp/flottille-never-written", "content": "x"}),
            reasons: vec!["/tmp/flottille-never-written is outside this agent's place".into()],
            base: false,
            judgement: None,
            kind: flottille_core::approval::Kind::Call,
            says: Vec::new(),
            held_by: "parent0000ab".into(),
            forwards: Vec::new(),
            state: RequestState::Pending,
            decision: None,
            created_at: 0,
            decided_at: None,
        })
        .unwrap();
    let number = request.number.to_string();

    assert!(approvals(&user).is_empty());
    for args in [
        vec!["approve", number.as_str()],
        vec!["deny", number.as_str()],
    ] {
        let error = cli(&user, &args).unwrap_err();
        assert!(error.contains("held by agent parent0000ab"), "{error}");
    }

    store(&user)
        .forward(
            request.number,
            "parent0000ab",
            "the child needs it for its report",
            approval::USER,
        )
        .unwrap()
        .unwrap();

    let listed = approvals(&user);
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0]["forwards"],
        json!([{"by": "parent0000ab", "why": "the child needs it for its report"}])
    );
    cli(&user, &["deny", &number]).unwrap();
    settled(&user, request.number);
    assert!(!workspace.join("flottille-never-written").exists());
}

#[test]
fn an_unknown_request_is_not_found() {
    let user = User::with_fake_agent();
    let error = cli(&user, &["approve", "42"]).unwrap_err();
    assert!(error.contains("no request #42"), "{error}");
}

#[test]
fn a_result_never_shows_a_value_of_the_place_s_env_files() {
    let user = User::with_fake_agent();
    let (_, mcp, workspace) = session(&user);
    std::fs::write(
        workspace.join("notes.txt"),
        "the password hunter2-hunter2\n",
    )
    .unwrap();

    let answer = mcp.call("search", json!({"pattern": "hunter2"}));

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("notes.txt:1: the password [masked]"),
        "{}",
        answer.text
    );
    assert!(!answer.text.contains("hunter2-hunter2"), "{}", answer.text);
}
