//! The scenarios of saying a file is unchanged instead of sending it again (`docs/tools.md`,
//! "Files"): the same page of a file, read again by the same session with no compaction since,
//! answers one line. The agents are the fake `claude`, whose compactions Flottille sees, and the
//! fake ACP agent, whose compactions it does not, in a throwaway repository and temporary XDG
//! folders.

mod common;

use std::path::PathBuf;

use common::User;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository holding `notes.txt`.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join("notes.txt"), "one\ntwo\nthree\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

/// A turn of `message`, waited for to its end.
fn turn(user: &User, agent: Option<&str>, message: &str) -> String {
    turn_on(user, agent, message, None)
}

/// The same, a new agent being of `provider` when it is given.
fn turn_on(user: &User, agent: Option<&str>, message: &str, provider: Option<&str>) -> String {
    let (id, after) = match agent {
        None => {
            let mut params = json!({"project": "app", "message": message});
            if let Some(provider) = provider {
                params["provider"] = provider.into();
            }
            let answer = user.call("agent.run", params).unwrap();
            (
                answer["agent"]["id"].as_str().unwrap().to_owned(),
                answer["after"].as_u64().unwrap(),
            )
        }
        Some(id) => {
            let answer = user
                .call("agent.send", json!({"agent": id, "message": message}))
                .unwrap();
            (id.to_owned(), answer["after"].as_u64().unwrap())
        }
    };
    user.watch(&["--after", &after.to_string()])
        .until(|e| e["data"]["agent"] == id.as_str() && e["name"] == "agent.turn_ended");
    id
}

/// The text of each tool answer the fake `claude` recorded, in order.
fn claude_answers(user: &User) -> Vec<String> {
    answers(user.claude_received())
}

/// The same for the fake ACP agent standing for OpenCode, whose compactions Flottille cannot see.
fn opencode_answers(user: &User) -> Vec<String> {
    answers(user.provider_received("opencode"))
}

fn answers(received: Vec<Value>) -> Vec<String> {
    received
        .iter()
        .filter(|entry| entry["mcp"]["body"]["result"]["content"].is_array())
        .map(|entry| {
            entry["mcp"]["body"]["result"]["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

fn read() -> Value {
    json!({"mcp": {"tool": "fs_read", "arguments": {"path": "notes.txt"}}})
}

fn claude_user(turns: Value) -> User {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": turns}));
    user
}

const UNCHANGED: &str = "unchanged since you read it (bytes 0-13 of 14, at ";

fn assert_whole(answer: &str) {
    assert!(answer.contains("     1\tone\n"), "{answer}");
    assert!(!answer.starts_with("unchanged"), "{answer}");
}

#[test]
fn a_second_read_of_an_unchanged_file_says_unchanged() {
    let user = claude_user(json!([{"when": "read", "do": [read(), read()]}]));
    project(&user);

    turn(&user, None, "read it twice");

    let answers = claude_answers(&user);
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_whole(&answers[0]);
    assert!(answers[1].starts_with(UNCHANGED), "{}", answers[1]);
    assert!(
        answers[1].ends_with("); give \"again\": true to read it whole"),
        "{}",
        answers[1]
    );
    assert_eq!(answers[1].lines().count(), 1, "one line: {}", answers[1]);
}

#[test]
fn a_read_after_a_compaction_is_whole() {
    let user = claude_user(json!([{"when": "read", "do": [
        read(), {"compaction": true}, {"wait": 700}, read()
    ]}]));
    project(&user);

    turn(&user, None, "read it, compact, read it");

    let answers = claude_answers(&user);
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_whole(&answers[0]);
    assert_whole(&answers[1]);
}

#[test]
fn a_changed_file_is_read_whole() {
    let user = claude_user(json!([
        {"when": "first", "do": [read()]},
        {"when": "second", "do": [read()]}
    ]));
    let folder = project(&user);
    let agent = turn(&user, None, "first");

    std::fs::write(folder.join("notes.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    turn(&user, Some(&agent), "second");

    let answers = claude_answers(&user);
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_whole(&answers[1]);
    assert!(answers[1].contains("     4\tfour\n"), "{}", answers[1]);
}

#[test]
fn a_read_of_another_range_is_whole() {
    let user = claude_user(json!([{"when": "read", "do": [
        read(),
        {"mcp": {"tool": "fs_read", "arguments": {"path": "notes.txt", "range": {"offset": 4}}}}
    ]}]));
    project(&user);

    turn(&user, None, "read it, then from the second line");

    let answers = claude_answers(&user);
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert!(answers[1].contains("     2\ttwo\n"), "{}", answers[1]);
    assert!(!answers[1].starts_with("unchanged"), "{}", answers[1]);
}

#[test]
fn again_reads_it_whole() {
    let user = claude_user(json!([{"when": "read", "do": [
        read(),
        {"mcp": {"tool": "fs_read", "arguments": {"path": "notes.txt", "again": true}}}
    ]}]));
    project(&user);

    turn(&user, None, "read it, then again");

    let answers = claude_answers(&user);
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_whole(&answers[1]);
}

#[test]
fn an_agent_whose_compactions_are_not_seen_always_reads_whole() {
    let user = User::with_fake_providers();
    user.provider_script(
        "opencode",
        json!({"turns": [{"when": "read", "do": [read(), read()]}]}),
    );
    project(&user);

    turn_on(&user, None, "read it twice", Some("opencode"));

    let answers = opencode_answers(&user);
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_whole(&answers[0]);
    assert_whole(&answers[1]);
}

#[test]
fn the_switch_off_reads_whole_and_is_kept_with_the_limit() {
    let user = claude_user(json!([{"when": "read", "do": [read(), read()]}]));
    project(&user);
    // A test daemon has no cut of results until one is chosen.
    let shown = user
        .call("tool_results.set", json!({"unchanged": false}))
        .unwrap();
    assert_eq!(shown, json!({"limit": "off", "unchanged": false}));
    let shown = user
        .call("tool_results.set", json!({"limit": 16000}))
        .unwrap();
    assert_eq!(shown, json!({"limit": 16000, "unchanged": false}));

    turn(&user, None, "read it twice");

    let answers = claude_answers(&user);
    assert_whole(&answers[1]);
    let shown = user
        .call("tool_results.set", json!({"unchanged": true}))
        .unwrap();
    assert_eq!(shown, json!({"limit": 16000, "unchanged": true}));
}
