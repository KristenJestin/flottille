//! The fake `acli` answers as its script says. Its being tested here is also what makes `cargo
//! test` build its binary, which the ticket tests run.

use std::process::Command;

use serde_json::{Value, json};

fn fake(dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flottille-fake-acli"));
    command.env("FLOTTILLE_FAKE_ACLI_DIR", dir);
    command
}

#[test]
fn auth_status_and_a_work_item_follow_the_script_and_every_call_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"items": {"ABC-1": {"key": "ABC-1"}}}).to_string(),
    )
    .unwrap();

    let signed_in = fake(dir.path())
        .args(["jira", "auth", "status"])
        .status()
        .unwrap();
    let viewed = fake(dir.path())
        .args(["jira", "workitem", "view", "ABC-1", "--json"])
        .output()
        .unwrap();
    let missing = fake(dir.path())
        .args(["jira", "workitem", "view", "ABC-2", "--json"])
        .status()
        .unwrap();
    let write = fake(dir.path())
        .args(["jira", "workitem", "create"])
        .status()
        .unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"signed_in": false}).to_string(),
    )
    .unwrap();
    let signed_out = fake(dir.path())
        .args(["jira", "auth", "status"])
        .status()
        .unwrap();

    assert!(signed_in.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&viewed.stdout).unwrap()["key"],
        "ABC-1"
    );
    assert_eq!(missing.code(), Some(1));
    assert_eq!(write.code(), Some(2));
    assert_eq!(signed_out.code(), Some(1));
    let received = std::fs::read_to_string(dir.path().join("received.jsonl")).unwrap();
    assert_eq!(received.lines().count(), 5);
}
