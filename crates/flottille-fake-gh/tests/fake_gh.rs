//! The fake `gh` answers as its script says. Its being tested here is also what makes `cargo test`
//! build its binary, which the forge's tests run.

use std::process::Command;

use serde_json::{Value, json};

fn fake(dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flottille-fake-gh"));
    command.env("FLOTTILLE_FAKE_GH_DIR", dir);
    command
}

#[test]
fn auth_status_follows_the_script_and_every_call_is_recorded() {
    let dir = tempfile::tempdir().unwrap();

    let signed_in = fake(dir.path()).args(["auth", "status"]).status().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"signed_in": false}).to_string(),
    )
    .unwrap();
    let signed_out = fake(dir.path()).args(["auth", "status"]).status().unwrap();

    assert!(signed_in.success());
    assert!(!signed_out.success());
    let received = std::fs::read_to_string(dir.path().join("received.jsonl")).unwrap();
    let calls: Vec<Value> = received
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["args"], json!(["auth", "status"]));
}
