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

#[test]
fn an_issue_is_answered_from_the_script_and_one_it_does_not_hold_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"issues": {"acme/api": {"12": {"number": 12, "title": "It fails"}}}}).to_string(),
    )
    .unwrap();

    let found = fake(dir.path())
        .args([
            "issue",
            "view",
            "12",
            "-R",
            "acme/api",
            "--json",
            "number,title",
        ])
        .output()
        .unwrap();
    let missing = fake(dir.path())
        .args([
            "issue",
            "view",
            "13",
            "-R",
            "acme/api",
            "--json",
            "number,title",
        ])
        .status()
        .unwrap();

    assert_eq!(
        serde_json::from_slice::<Value>(&found.stdout).unwrap()["title"],
        "It fails"
    );
    assert_eq!(missing.code(), Some(1));
}

#[test]
fn the_host_comes_from_r_first_then_from_gh_host_as_the_real_gh_takes_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"issues": {
            "acme/api": {"12": {"number": 12, "title": "On GitHub"}},
            "ghe.example.com/acme/api": {"12": {"number": 12, "title": "Elsewhere"}}
        }})
        .to_string(),
    )
    .unwrap();
    let title = |repository: &str| {
        let output = fake(dir.path())
            .env("GH_HOST", "ghe.example.com")
            .args(["issue", "view", "12", "-R", repository, "--json", "title"])
            .output()
            .unwrap();
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["title"].clone()
    };

    assert_eq!(title("acme/api"), "Elsewhere");
    assert_eq!(title("github.com/acme/api"), "On GitHub");
}

#[test]
fn issues_are_listed_by_state_label_assignee_and_words() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"issues": {"acme/api": {
            "1": {"number": 1, "title": "Login fails", "state": "OPEN",
                  "labels": [{"name": "bug"}], "assignees": [{"login": "ana"}]},
            "2": {"number": 2, "title": "Add search", "state": "OPEN", "labels": [], "assignees": []},
            "3": {"number": 3, "title": "Old login bug", "state": "CLOSED",
                  "labels": [{"name": "bug"}], "assignees": []}}}})
        .to_string(),
    )
    .unwrap();
    let list = |extra: &[&str]| -> Value {
        let mut args = vec!["issue", "list", "-R", "acme/api", "--json", "number"];
        args.extend(extra);
        let output = fake(dir.path()).args(args).output().unwrap();
        assert!(output.status.success());
        serde_json::from_slice(&output.stdout).unwrap()
    };

    assert_eq!(list(&[]), json!([{"number": 2}, {"number": 1}]));
    assert_eq!(
        list(&["--state", "all", "--label", "bug"]),
        json!([{"number": 3}, {"number": 1}])
    );
    assert_eq!(list(&["--assignee", "ana"]), json!([{"number": 1}]));
    assert_eq!(
        list(&["--state", "all", "--search", "LOGIN"]),
        json!([{"number": 3}, {"number": 1}])
    );
    assert_eq!(list(&["--limit", "1"]), json!([{"number": 2}]));
}

#[test]
fn issue_and_pull_request_writes_answer_for_what_the_script_holds() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"created_issue": 9,
               "issues": {"acme/api": {"1": {"number": 1, "state": "OPEN"}}},
               "pulls": {"acme/api": {"login": {"number": 4, "state": "OPEN"}}}})
        .to_string(),
    )
    .unwrap();
    let run = |args: &[&str]| fake(dir.path()).args(args).output().unwrap();

    let created = run(&[
        "issue", "create", "-R", "acme/api", "--title", "t", "--body", "b",
    ]);
    assert_eq!(
        String::from_utf8_lossy(&created.stdout).trim(),
        "https://github.com/acme/api/issues/9"
    );
    for verb in ["comment", "edit", "close", "reopen"] {
        assert!(
            run(&["issue", verb, "1", "-R", "acme/api"])
                .status
                .success()
        );
        assert!(
            !run(&["issue", verb, "5", "-R", "acme/api"])
                .status
                .success()
        );
    }
    for verb in ["comment", "edit"] {
        assert!(
            run(&["pr", verb, "4", "-R", "acme/api", "--body", "b"])
                .status
                .success()
        );
        assert!(
            !run(&["pr", verb, "5", "-R", "acme/api", "--body", "b"])
                .status
                .success()
        );
    }
}

#[test]
fn pull_requests_are_listed_by_state_and_branch_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"pulls": {"acme/api": {
            "login": {"number": 4, "state": "OPEN", "title": "Login"},
            "old": {"number": 2, "state": "MERGED", "title": "Old"},
            "search": {"number": 5, "state": "OPEN", "isDraft": true}}}})
        .to_string(),
    )
    .unwrap();
    let list = |extra: &[&str]| -> Value {
        let mut args = vec![
            "pr",
            "list",
            "-R",
            "acme/api",
            "--json",
            "number,headRefName",
        ];
        args.extend(extra);
        let output = fake(dir.path()).args(args).output().unwrap();
        serde_json::from_slice(&output.stdout).unwrap()
    };

    assert_eq!(
        list(&[]),
        json!([{"number": 5, "headRefName": "search"}, {"number": 4, "headRefName": "login"}])
    );
    assert_eq!(
        list(&["--state", "merged"]),
        json!([{"number": 2, "headRefName": "old"}])
    );
    assert_eq!(
        list(&["--state", "all", "--head", "login"]),
        json!([{"number": 4, "headRefName": "login"}])
    );
}
