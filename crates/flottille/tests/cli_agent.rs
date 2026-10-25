//! The command line's side of driving agents from outside (`docs/cli.md`): the scenario of the
//! pilot door (`tests/pilot.rs`) through `flottille agent start|send|show|read|stop|list|tree` and
//! `flottille inbox`, all with `--json`. Against the real daemon with the fake ACP agent.

mod common;

use std::process::Output;
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "LEAD-LAUNCHES", "do": [
            {"mcp": {"tool": "agent_launch", "arguments": {"title": "worker one",
                "brief": "HANG while the pilot looks", "workspace": "w"}, "keep": "w1"}},
            {"say": "launched the worker"}]},
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "ASK-USER", "do": [
            {"mcp": {"tool": "ask_user", "arguments": {"question": "which database?"}}}]},
        {"when": "REPORT-NOW", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "all done, tests pass"}}}]},
        {"when": "die-always", "do": [{"die": 4}]}
    ]}));
    user
}

/// A project `app` over a throwaway repository, and its ready workspace `w`.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
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
    user.call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A command that succeeded: its standard output as JSON.
fn json_of(output: Output) -> Value {
    assert!(
        output.status.success(),
        "failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// A command that failed: its standard error.
fn failure_of(output: Output) -> String {
    assert!(!output.status.success(), "it succeeded");
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn the_cli_starts_a_root_agent_sends_it_a_message_reads_it_sees_its_child_reads_and_stops_it() {
    let user = user();
    project(&user);

    let started = json_of(user.run(&[
        "agent",
        "start",
        "app",
        "LEAD-LAUNCHES and wait",
        "--workspace",
        "w",
        "--json",
    ]));
    let root = started["agent"]["id"].as_str().unwrap().to_owned();
    assert!(started["after"].is_u64());
    assert_eq!(started["agent"]["role"], "lead");

    let mut child = String::new();
    eventually("the child", || {
        let tree = json_of(user.run(&["agent", "tree", &root, "--json"]));
        let agents = tree["agents"].as_array().unwrap();
        if let Some(found) = agents.iter().find(|a| a["agent"]["title"] == "worker one") {
            child = found["agent"]["id"].as_str().unwrap().to_owned();
            assert_eq!(found["agent"]["parent"], root.as_str());
            return true;
        }
        false
    });
    let tree = json_of(user.run(&["agent", "tree", &root, "--json"]));
    assert_eq!(
        tree["agents"][0]["agent"]["id"],
        root.as_str(),
        "the root comes first"
    );

    let listed = json_of(user.run(&["agent", "list", "--project", "app", "--json"]));
    assert_eq!(listed["agents"].as_array().unwrap().len(), 2);
    let listed = json_of(user.run(&["agent", "list", "--json"]));
    assert_eq!(listed["agents"].as_array().unwrap().len(), 2);

    let sent = json_of(user.run(&[
        "agent",
        "send",
        &root,
        "a second message from the user",
        "--json",
    ]));
    assert_eq!(sent["agent"]["id"], root.as_str());
    assert!(sent["after"].is_u64());

    let mut read = Value::Null;
    eventually("the second turn in the conversation", || {
        read = json_of(user.run(&["agent", "read", &root, "--json"]));
        read["lines"]
            .to_string()
            .contains("a second message from the user")
    });
    assert!(read["lines"].to_string().contains("launched the worker"));
    assert_eq!(read["more"], false);
    assert!(read["after"].as_u64().unwrap() > 0);
    let text = user.run(&["agent", "read", &root]);
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("said: launched the worker"), "{text}");

    let worker = json_of(user.run(&["agent", "read", &child, "--json"]));
    assert!(
        worker["lines"]
            .to_string()
            .contains("HANG while the pilot looks")
    );
    let shown = json_of(user.run(&["agent", "show", &child, "--json"]));
    assert_eq!(shown["agent"]["parent"], root.as_str());

    // A child belongs to its parent.
    let refused = failure_of(user.run(&["agent", "send", &child, "do it differently"]));
    assert!(
        refused.contains("only its parent") && refused.contains(&root),
        "{refused}"
    );
    let refused = failure_of(user.run(&["agent", "stop", &child]));
    assert!(
        refused.contains("only its parent") && refused.contains(&root),
        "{refused}"
    );

    let stopped = json_of(user.run(&["agent", "stop", &root, "--json"]));
    assert_eq!(stopped["agent"]["state"], "ended");
    let tree = json_of(user.run(&["agent", "tree", &root, "--json"]));
    for entry in tree["agents"].as_array().unwrap() {
        assert_eq!(entry["agent"]["state"], "ended");
    }
}

#[test]
fn agent_run_still_starts_an_agent_and_prints_its_reply() {
    let user = user();
    project(&user);
    let output = user.run(&["agent", "run", "app", "hello"]);
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("ok"));
}

#[test]
fn the_inbox_lists_a_root_s_report_its_question_and_its_failure_and_nothing_of_a_child() {
    let user = user();
    project(&user);
    let start = |message: &str| {
        let started = json_of(user.run(&["agent", "start", "app", message, "--json"]));
        started["agent"]["id"].as_str().unwrap().to_owned()
    };
    let reporter = start("REPORT-NOW");
    let asker = start("ASK-USER");
    let doomed = start("die-always");
    let lead = json_of(user.run(&[
        "agent",
        "start",
        "app",
        "LEAD-LAUNCHES",
        "--workspace",
        "w",
        "--json",
    ]))["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut entries = Vec::new();
    eventually("the three entries", || {
        let inbox = json_of(user.run(&["inbox", "--json"]));
        entries = inbox["entries"].as_array().unwrap().clone();
        entries.len() >= 3
    });
    let of = |agent: &str| entries.iter().find(|e| e["agent"] == agent).unwrap();
    assert_eq!(of(&reporter)["kind"], "report");
    assert_eq!(of(&reporter)["text"], "all done, tests pass");
    assert_eq!(of(&asker)["kind"], "question");
    assert_eq!(of(&asker)["text"], "which database?");
    assert_eq!(of(&doomed)["kind"], "failure");
    assert!(
        of(&doomed)["text"]
            .as_str()
            .unwrap()
            .contains("replacements"),
        "{entries:?}"
    );
    for entry in &entries {
        assert!(entry["seq"].is_u64() && entry["at"].is_u64());
        assert_eq!(entry["project"], "app");
        assert_ne!(entry["agent"], lead.as_str());
    }
    // A worker's own report goes to its lead, never to the user: the lead's worker hangs, and
    // nothing of it is in the inbox.
    assert_eq!(entries.len(), 3, "{entries:?}");

    // `--after` reads on from the last number seen.
    let last = entries.last().unwrap()["seq"].as_u64().unwrap();
    let inbox = json_of(user.run(&["inbox", "--json", "--after", &last.to_string()]));
    assert!(inbox["entries"].as_array().unwrap().is_empty());

    let text = user.run(&["inbox"]);
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        text.contains("all done, tests pass") && text.contains("which database?"),
        "{text}"
    );
    assert!(text.contains("failure of agent"), "{text}");
}

#[test]
fn an_empty_inbox_says_so() {
    let user = user();
    let text = user.run(&["inbox"]);
    assert!(text.status.success());
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "Nothing came for you.\n"
    );
    let inbox = json_of(user.run(&["inbox", "--json"]));
    assert_eq!(inbox, json!({"entries": []}));
}
