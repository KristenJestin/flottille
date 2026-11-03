//! The fake `codex` speaks the app-server protocol as its script says. Its being tested here is
//! also what makes `cargo test` build its binary, which the bridge's tests run.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn fake() -> Command {
    Command::new(env!("CARGO_BIN_EXE_flottille-fake-codex"))
}

#[test]
fn it_prints_the_version_the_bridge_is_pinned_to() {
    let output = fake().arg("--version").output().unwrap();

    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "codex-cli 0.162.0\n"
    );
}

#[test]
fn a_turn_plays_its_script_and_ends_completed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"turns": [{"when": "hello", "do": [{"say": "Hello!"}]}]}).to_string(),
    )
    .unwrap();
    let mut child = fake()
        .args(["app-server", "--listen", "stdio://"])
        .env("FLOTTILLE_FAKE_CODEX_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut next = || -> Value { serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap() };

    writeln!(
        stdin,
        "{}",
        json!({"id": 1, "method": "thread/start", "params": {"cwd": "/w"}})
    )
    .unwrap();
    let thread = next()["result"]["thread"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    writeln!(
        stdin,
        "{}",
        json!({"id": 2, "method": "turn/start", "params": {"threadId": thread,
            "input": [{"type": "text", "text": "hello", "text_elements": []}]}})
    )
    .unwrap();

    assert_eq!(next()["result"]["turn"]["id"], "turn-1");
    let mut said = String::new();
    let completed = loop {
        let message = next();
        if message["method"] == "item/agentMessage/delta" {
            said.push_str(message["params"]["delta"].as_str().unwrap());
        }
        if message["method"] == "turn/completed" {
            break message;
        }
    };
    assert_eq!(said, "Hello!");
    assert_eq!(completed["params"]["turn"]["status"], "completed");
    drop(stdin);
    child.wait().unwrap();
}
