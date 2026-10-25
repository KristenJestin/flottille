//! The fake `claude` speaks stream-json as its script says. Its being tested here is also what
//! makes `cargo test` build its binary, which the bridge's tests run.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn fake() -> Command {
    Command::new(env!("CARGO_BIN_EXE_flottille-fake-claude"))
}

#[test]
fn it_prints_the_version_the_bridge_is_pinned_to() {
    let output = fake().arg("--version").output().unwrap();

    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "2.1.295 (Claude Code)\n"
    );
}

#[test]
fn a_prompt_plays_its_turn_and_ends_with_a_result() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"turns": [{"when": "hello", "do": [{"say": "Hello!"}]}]}).to_string(),
    )
    .unwrap();
    let mut child = fake()
        .args(["--session-id=s-1"])
        .env("FLOTTILLE_FAKE_CLAUDE_DIR", dir.path())
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
        json!({"type": "control_request", "request_id": "r-1", "request": {"subtype": "initialize"}})
    )
    .unwrap();
    let answer = next();
    assert_eq!(answer["type"], "control_response");
    assert_eq!(answer["response"]["request_id"], "r-1");
    writeln!(
        stdin,
        "{}",
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}})
    )
    .unwrap();
    let mut said = Vec::new();
    let result = loop {
        let message = next();
        if message["type"] == "result" {
            break message;
        }
        said.push(message);
    };

    assert!(
        said.iter()
            .any(|m| m["message"]["content"][0]["text"] == "Hello!")
    );
    assert_eq!(result["subtype"], "success");
    assert_eq!(result["is_error"], false);
    assert_eq!(result["session_id"], "s-1");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
