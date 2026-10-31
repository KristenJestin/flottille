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

#[test]
fn a_merge_by_a_method_the_repository_forbids_fails_and_repo_view_says_the_allowed_ones() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"pulls": {"acme/app": {"app/login": {"number": 12}}},
            "merge_methods": {"acme/app": ["merge", "rebase"]}})
        .to_string(),
    )
    .unwrap();
    let merge = |method: &str| {
        fake(dir.path())
            .args(["pr", "merge", "12", "-R", "github.com/acme/app", method])
            .output()
            .unwrap()
    };

    let squash = merge("--squash");
    let rebase = merge("--rebase");
    let allowed = fake(dir.path())
        .args([
            "repo",
            "view",
            "github.com/acme/app",
            "--json",
            "mergeCommitAllowed,rebaseMergeAllowed,squashMergeAllowed",
        ])
        .output()
        .unwrap();
    let viewed = fake(dir.path())
        .args([
            "pr",
            "view",
            "12",
            "-R",
            "github.com/acme/app",
            "--json",
            "headRefName",
        ])
        .output()
        .unwrap();

    assert!(!squash.status.success());
    assert!(
        String::from_utf8_lossy(&squash.stderr).contains("not allowed on this repository"),
        "{squash:?}"
    );
    assert!(rebase.status.success(), "{rebase:?}");
    assert!(allowed.status.success(), "{allowed:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&allowed.stdout).unwrap(),
        json!({"mergeCommitAllowed": true, "rebaseMergeAllowed": true,
            "squashMergeAllowed": false})
    );
    // A pull request's branch is the key it is held under.
    let viewed: Value = serde_json::from_slice(&viewed.stdout).unwrap();
    assert_eq!(viewed["headRefName"], "app/login");
}

#[test]
fn a_view_can_lose_the_fields_the_script_omits() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"pulls": {"acme/app": {"app/login": {"number": 12, "title": "Login"}}},
            "view_omits": {"acme/app": ["number"]}})
        .to_string(),
    )
    .unwrap();
    let view = |number: &str| {
        fake(dir.path())
            .args([
                "pr",
                "view",
                number,
                "-R",
                "github.com/acme/app",
                "--json",
                "number",
            ])
            .output()
            .unwrap()
    };

    let viewed = view("12");

    assert!(viewed.status.success(), "{viewed:?}");
    let viewed: Value = serde_json::from_slice(&viewed.stdout).unwrap();
    assert_eq!(viewed["title"], "Login");
    assert!(viewed.get("number").is_none(), "{viewed}");
}

#[test]
fn a_failed_job_s_log_is_printed_from_the_script_and_one_it_does_not_hold_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"logs": {"acme/app": {"456": "test\tRun tests\t2026-01-01T00:00:00.0000000Z boom\n"}}})
            .to_string(),
    )
    .unwrap();
    let view = |job: &str| {
        fake(dir.path())
            .args([
                "run",
                "view",
                "--job",
                job,
                "--log-failed",
                "-R",
                "github.com/acme/app",
            ])
            .output()
            .unwrap()
    };

    let held = view("456");
    let gone = view("457");

    assert!(held.status.success());
    assert_eq!(
        String::from_utf8_lossy(&held.stdout),
        "test\tRun tests\t2026-01-01T00:00:00.0000000Z boom\n"
    );
    assert!(!gone.status.success());
    assert!(String::from_utf8_lossy(&gone.stderr).contains("log not found"));
}
