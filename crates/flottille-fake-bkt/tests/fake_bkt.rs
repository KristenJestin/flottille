//! The fake `bkt` answers as its script says. Its being tested here is also what makes `cargo test`
//! build its binary, which the Bitbucket tests run.

use std::process::Command;

use serde_json::{Value, json};

fn fake(dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flottille-fake-bkt"));
    command.env("FLOTTILLE_FAKE_BKT_DIR", dir);
    command
}

fn script(dir: &std::path::Path, script: Value) {
    std::fs::write(dir.join("script.json"), script.to_string()).unwrap();
}

fn json_of(output: std::process::Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn contexts_follow_the_script_and_every_call_is_recorded() {
    let dir = tempfile::tempdir().unwrap();

    let default = json_of(
        fake(dir.path())
            .args(["context", "list", "--json"])
            .output()
            .unwrap(),
    );
    script(
        dir.path(),
        json!({"contexts": [{"name": "work", "host": "bitbucket.example.com"}]}),
    );
    let scripted = json_of(
        fake(dir.path())
            .args(["context", "list", "--json"])
            .output()
            .unwrap(),
    );

    assert_eq!(default["contexts"][0]["host"], "bitbucket.org");
    assert_eq!(
        scripted["contexts"],
        json!([{"name": "work", "host": "bitbucket.example.com"}])
    );
    let received = std::fs::read_to_string(dir.path().join("received.jsonl")).unwrap();
    let calls: Vec<Value> = received
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["args"], json!(["context", "list", "--json"]));
}

#[test]
fn a_command_needs_its_context_and_its_repository_as_the_real_one_does() {
    let dir = tempfile::tempdir().unwrap();

    let no_context = fake(dir.path())
        .args([
            "pr",
            "list",
            "--workspace",
            "acme",
            "--repo",
            "api",
            "--json",
        ])
        .status()
        .unwrap();
    let no_repository = fake(dir.path())
        .args(["pr", "list", "--context", "cloud", "--json"])
        .status()
        .unwrap();
    let both = fake(dir.path())
        .args([
            "pr",
            "list",
            "--context",
            "cloud",
            "--workspace",
            "acme",
            "--repo",
            "api",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!no_context.success());
    assert!(!no_repository.success());
    let listed = json_of(both);
    assert_eq!(listed["workspace"], "acme");
    assert_eq!(listed["pull_requests"], json!([]));
}

#[test]
fn pull_requests_are_listed_by_state_and_a_failing_repository_fails() {
    let dir = tempfile::tempdir().unwrap();
    script(
        dir.path(),
        json!({
            "pulls": {"acme/api": [
                {"id": 1, "state": "OPEN"}, {"id": 2, "state": "MERGED"}
            ]},
            "checks": {"acme/api": {"1": [{"state": "FAILED", "name": "build"}]}},
            "fail": {"acme/web": "404 Not Found"}
        }),
    );
    let list = |state: &str, repo: &str| {
        fake(dir.path())
            .args([
                "pr",
                "list",
                "--context",
                "cloud",
                "--workspace",
                "acme",
                "--repo",
                repo,
                "--state",
                state,
                "--json",
            ])
            .output()
            .unwrap()
    };

    let open = json_of(list("OPEN", "api"));
    let all = json_of(list("all", "api"));
    let failing = list("all", "web");
    let checks = json_of(
        fake(dir.path())
            .args([
                "pr",
                "checks",
                "1",
                "--context",
                "cloud",
                "--workspace",
                "acme",
                "--repo",
                "api",
                "--json",
            ])
            .output()
            .unwrap(),
    );

    assert_eq!(open["pull_requests"], json!([{"id": 1, "state": "OPEN"}]));
    assert_eq!(all["pull_requests"].as_array().unwrap().len(), 2);
    assert_eq!(failing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failing.stderr).contains("404 Not Found"));
    assert_eq!(checks["statuses"][0]["name"], "build");
}

#[test]
fn the_writes_answer_as_bkt_does_and_api_answers_the_scripted_body() {
    let dir = tempfile::tempdir().unwrap();
    script(
        dir.path(),
        json!({"created": 12, "api": {"/repositories/acme/api/pullrequests/1": {"id": 1}}}),
    );
    let run = |args: &[&str]| {
        fake(dir.path())
            .args(args)
            .args(["--context", "cloud", "--workspace", "acme", "--repo", "api"])
            .output()
            .unwrap()
    };

    let created = run(&[
        "pr", "create", "--source", "a", "--target", "main", "--title", "t",
    ]);
    let merged = run(&["pr", "merge", "12", "--strategy", "squash"]);
    let triggered = run(&["pipeline", "run", "--ref", "a"]);
    let api = fake(dir.path())
        .args([
            "api",
            "/repositories/acme/api/pullrequests/1",
            "--context",
            "cloud",
        ])
        .output()
        .unwrap();
    let missing = fake(dir.path())
        .args(["api", "/nothing", "--context", "cloud"])
        .status()
        .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&created.stdout),
        "✓ Created pull request #12\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&merged.stdout),
        "✓ Merged pull request #12\n"
    );
    assert!(String::from_utf8_lossy(&triggered.stdout).starts_with("✓ Triggered pipeline"));
    assert_eq!(json_of(api), json!({"id": 1}));
    assert!(!missing.success());
}

#[test]
fn a_comment_and_an_edit_answer_as_bkt_does_for_a_pull_request_it_holds() {
    let dir = tempfile::tempdir().unwrap();
    script(
        dir.path(),
        json!({"pulls": {"acme/api": [{"id": 12, "state": "OPEN"}]}}),
    );
    let run = |args: &[&str]| {
        let mut all = args.to_vec();
        all.extend(["--context", "cloud", "--workspace", "acme", "--repo", "api"]);
        fake(dir.path()).args(all).output().unwrap()
    };

    let commented = run(&["pr", "comment", "12", "--text", "Looks good"]);
    let edited = run(&["pr", "edit", "12", "--title", "New title"]);
    let empty_edit = run(&["pr", "edit", "12"]);
    let missing = run(&["pr", "comment", "13", "--text", "x"]);

    assert_eq!(
        String::from_utf8_lossy(&commented.stdout).trim(),
        "✓ Commented on pull request #12"
    );
    assert_eq!(
        String::from_utf8_lossy(&edited.stdout).trim(),
        "✓ Updated pull request #12"
    );
    assert!(!empty_edit.status.success());
    assert!(!missing.status.success());
}
