//! A `run` line that only prints a part of a file is answered as the `fs_read` it is (`docs/tools.md`,
//! "Running"): `sed -n 5,7p`, `head`, `tail` and `cat` of one file inside the workspace. The agents
//! are the fake agent's sessions, driven through their MCP server as their client would, on
//! throwaway repositories and temporary XDG folders.

mod common;

use std::path::PathBuf;

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A project `app` and its ready workspace `login`, whose `notes.md` has forty numbered lines.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    let folder = PathBuf::from(workspace["folder"].as_str().unwrap());
    let notes: String = (1..=40).map(|n| format!("line {n}\n")).collect();
    std::fs::write(folder.join("notes.md"), notes).unwrap();
    std::fs::write(folder.join("other.md"), "other\n").unwrap();
    folder
}

fn root(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "hello", "workspace": "login"}),
        )
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn judged(user: &User) -> usize {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .count()
}

fn pending(user: &User) -> usize {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .len()
}

/// The text of an answer that was served at once.
fn served(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

/// The calls the gate served of an agent: their tool and the gate's word.
fn tools_served(user: &User, agent: &str) -> Vec<(String, Option<String>)> {
    let store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    store
        .agent_events(agent, 0, 1000)
        .unwrap()
        .into_iter()
        .filter(|event| event.name == "agent.tool_served")
        .map(|event| {
            (
                event.data["tool"].as_str().unwrap().to_owned(),
                event.data["gate"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

/// A session in Ask, a fake `sudo` first on the PATH, and the workspace's folder.
fn session() -> (User, String, Mcp, PathBuf) {
    let user = User::with_fake_agent()
        .with_fake_escalation()
        .with_allowing_judge();
    let workspace = project(&user);
    let (agent, mcp) = root(&user);
    set_mode(&user, &agent, "ask");
    (user, agent, mcp, workspace)
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(_, text)| text.to_owned())
        .collect()
}

fn said(text: &str) -> &str {
    text.lines().next().unwrap()
}

#[test]
fn a_sed_range_is_answered_as_a_read() {
    let (user, agent, mcp, _) = session();

    let text = served(mcp.call("run", json!({"line": "sed -n 5,7p notes.md"})));
    assert!(
        said(&text).starts_with(r#"read with fs_read {"path": "notes.md", "range": {"offset": "#)
            && said(&text).ends_with("}}: use it directly"),
        "{text}"
    );
    assert_eq!(lines(&text), ["line 5", "line 6", "line 7"], "{text}");

    let text = served(mcp.call("run", json!({"line": "sed -n 12p notes.md"})));
    assert_eq!(lines(&text), ["line 12"], "{text}");
    let text = served(mcp.call("run", json!({"line": "sed -n '39,$p' notes.md"})));
    assert_eq!(lines(&text), ["line 39", "line 40"], "{text}");
    let text = served(mcp.call("run", json!({"line": "sed -n 1,2p ./notes.md"})));
    assert_eq!(lines(&text), ["line 1", "line 2"], "{text}");

    // The gate saw the read it is, not a run: nothing was started.
    let served_calls = tools_served(&user, &agent);
    assert_eq!(served_calls.len(), 4, "{served_calls:?}");
    for (tool, gate) in served_calls {
        assert_eq!((tool.as_str(), gate.as_deref()), ("fs_read", Some("allow")));
    }
}

#[test]
fn head_and_tail_of_a_file_are_answered_as_a_read() {
    let (_user, _, mcp, _) = session();

    let text = served(mcp.call("run", json!({"line": "head -n 3 notes.md"})));
    assert!(said(&text).starts_with("read with fs_read "), "{text}");
    assert_eq!(lines(&text), ["line 1", "line 2", "line 3"], "{text}");
    let text = served(mcp.call("run", json!({"line": "head notes.md"})));
    assert_eq!(lines(&text).len(), 10, "{text}");
    assert_eq!(lines(&text)[9], "line 10", "{text}");
    let text = served(mcp.call("run", json!({"line": "tail -n 2 notes.md"})));
    assert!(said(&text).starts_with("read with fs_read "), "{text}");
    assert_eq!(lines(&text), ["line 39", "line 40"], "{text}");
    let text = served(mcp.call("run", json!({"line": "tail notes.md"})));
    assert_eq!(lines(&text).len(), 10, "{text}");
    assert_eq!(lines(&text)[0], "line 31", "{text}");
    let text = served(mcp.call("run", json!({"line": "cat other.md"})));
    assert!(said(&text).starts_with("read with fs_read "), "{text}");
    assert_eq!(lines(&text), ["other"], "{text}");
}

#[test]
fn a_read_through_run_does_not_ask_in_ask_mode() {
    let (user, agent, mcp, _) = session();
    for line in ["sed -n 295,316p notes.md", "head -n 2 notes.md"] {
        // A range past the end of the file is today's: only what is there is a read.
        let answer = mcp.call("run", json!({ "line": line }));
        if line.starts_with("head") {
            served(answer);
        }
    }
    served(mcp.call("run", json!({"line": "sed -n 30,31p notes.md"})));
    served(mcp.call("run", json!({"line": "cat other.md"})));
    assert_eq!(judged(&user), 0, "a read reached the judge");
    set_mode(&user, &agent, "auto");
    served(mcp.call("run", json!({"line": "tail -n 1 notes.md"})));
    assert_eq!(judged(&user), 0, "a read reached the judge in Auto");
    assert_eq!(user.escalations(), Vec::<String>::new());
    let held = pending(&user);
    assert_eq!(held, 1, "only the range past the end waits");
}

/// The request a call held: its class.
fn held_class(user: &User, answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    let rest = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    let number: u64 = rest.split('.').next().unwrap().parse().unwrap();
    let listed: Value = user.call("approval.list", json!({})).unwrap();
    let request = listed["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .cloned()
        .unwrap();
    request["class"].as_str().unwrap().to_owned()
}

#[test]
fn a_sed_command_that_edits_is_handled_as_today() {
    let (user, agent, mcp, workspace) = session();
    for line in [
        "sed -i s/line/LINE/ notes.md",
        "sed -n -i 5,7p notes.md",
        "sed 5d notes.md",
        "sed -n 5,7w out.md notes.md",
        "sed -n 5,7p notes.md other.md",
        "sed -n /line/p notes.md",
        "sed -n 7,5p notes.md",
        "sed -n 0p notes.md",
        "head -c 5 notes.md",
        "head -n +3 notes.md",
        "tail -n +3 notes.md",
        "tail -f notes.md",
        "cat -n notes.md",
        "cat notes.md other.md",
        "cat",
    ] {
        let answer = mcp.call("run", json!({ "line": line }));
        assert!(
            !answer.text.starts_with("read with fs_read"),
            "{line}: {}",
            answer.text
        );
    }
    assert!(
        std::fs::read_to_string(workspace.join("notes.md"))
            .unwrap()
            .starts_with("line 1\n"),
        "a sed edited the file"
    );
    let served_calls = tools_served(&user, &agent);
    assert!(
        served_calls.iter().all(|(tool, _)| tool != "fs_read"),
        "{served_calls:?}"
    );
    // Shell syntax stays refused, not read.
    let piped = mcp.call("run", json!({"line": "cat notes.md | head"}));
    assert!(
        piped.error && piped.text.contains("shell syntax"),
        "{}",
        piped.text
    );
    // A run with a task or its own options is not a bare line.
    let timed = mcp.call("run", json!({"line": "cat other.md", "timeout": 5}));
    assert!(
        !timed.text.starts_with("read with fs_read"),
        "{}",
        timed.text
    );
    // What fs_read holds for the user it still holds when asked through run: base, not a read.
    std::fs::write(workspace.join(".env"), "TOKEN=abc\n").unwrap();
    let secret = mcp.call("run", json!({"line": "cat .env"}));
    assert_eq!(held_class(&user, secret), "base");
    let missing = mcp.call("run", json!({"line": "cat missing.md"}));
    assert!(
        !missing.text.starts_with("read with fs_read"),
        "{}",
        missing.text
    );
    assert_eq!(user.escalations(), Vec::<String>::new());
}

#[test]
fn a_path_outside_the_workspace_is_handled_as_today() {
    let (user, _, mcp, workspace) = session();
    let outside = user.path("home/notes.txt");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, "outside\n").unwrap();
    for line in [
        format!("cat {}", outside.display()),
        format!("sed -n 1,1p {}", outside.display()),
        "head -n 1 ../../../home/notes.txt".to_owned(),
        "tail -n 1 ../notes.txt".to_owned(),
    ] {
        let answer = mcp.call("run", json!({ "line": line }));
        assert!(
            !answer.text.starts_with("read with fs_read"),
            "{line}: {}",
            answer.text
        );
    }
    // A link inside the workspace that leads outside is outside too.
    std::os::unix::fs::symlink(&outside, workspace.join("link.md")).unwrap();
    let linked = mcp.call("run", json!({"line": "cat link.md"}));
    assert!(
        !linked.text.starts_with("read with fs_read"),
        "{}",
        linked.text
    );
    assert_eq!(user.escalations(), Vec::<String>::new());
}

#[test]
fn a_last_line_without_a_newline_is_read_whole() {
    let (_user, _, mcp, workspace) = session();
    std::fs::write(workspace.join("bare.md"), "one\ntwo\nthree").unwrap();
    let text = served(mcp.call("run", json!({"line": "tail -n 2 bare.md"})));
    assert_eq!(lines(&text), ["two", "three"], "{text}");
    let text = served(mcp.call("run", json!({"line": "sed -n '3,$p' bare.md"})));
    assert_eq!(lines(&text), ["three"], "{text}");
}

#[test]
fn a_range_longer_than_a_page_is_handled_as_today() {
    let (_user, _, mcp, workspace) = session();
    let long: String = (1..=5000).map(|n| format!("line number {n}\n")).collect();
    std::fs::write(workspace.join("long.md"), long).unwrap();
    for line in [
        "cat long.md",
        "tail -n 4000 long.md",
        "sed -n '2,$p' long.md",
    ] {
        let answer = mcp.call("run", json!({ "line": line }));
        assert!(
            !answer.text.starts_with("read with fs_read"),
            "{line}: {}",
            answer.text
        );
    }
    let text = served(mcp.call("run", json!({"line": "sed -n 4999,5000p long.md"})));
    assert_eq!(
        lines(&text),
        ["line number 4999", "line number 5000"],
        "{text}"
    );
}
