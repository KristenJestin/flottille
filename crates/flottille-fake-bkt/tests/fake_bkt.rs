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

#[test]
fn a_merge_by_a_strategy_the_repository_forbids_fails() {
    let dir = tempfile::tempdir().unwrap();
    script(
        dir.path(),
        json!({"strategies": {"acme/api": ["merge_commit"]}}),
    );
    let merge = |strategy: &str| {
        fake(dir.path())
            .args([
                "pr",
                "merge",
                "12",
                "--context",
                "cloud",
                "--workspace",
                "acme",
                "--repo",
                "api",
                "--strategy",
                strategy,
                "--close-source=false",
            ])
            .output()
            .unwrap()
    };

    let squash = merge("squash");
    let merged = merge("merge_commit");

    assert!(!squash.status.success());
    assert!(
        String::from_utf8_lossy(&squash.stderr).contains("squash"),
        "{squash:?}"
    );
    assert!(merged.status.success(), "{merged:?}");
}

#[test]
fn checks_of_an_unreadable_shape_have_no_statuses() {
    let dir = tempfile::tempdir().unwrap();
    script(
        dir.path(),
        json!({"checks": {"acme/api": {"12": "unreadable"}}}),
    );

    let checked = fake(dir.path())
        .args([
            "pr",
            "checks",
            "12",
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

    assert!(checked.status.success(), "{checked:?}");
    let checked: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert!(checked.get("statuses").is_none(), "{checked}");
    assert_eq!(checked["pull_request"], 12);
}

#[test]
fn checks_of_a_commit_without_a_status_answer_null_statuses() {
    let dir = tempfile::tempdir().unwrap();
    script(dir.path(), json!({}));

    let checked = json_of(
        fake(dir.path())
            .args([
                "pr",
                "checks",
                "12",
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

    // As `bkt` v0.37.0 does: a nil Go slice, `null`, and the key is there.
    assert!(
        checked.get("statuses").is_some_and(Value::is_null),
        "{checked}"
    );
}

#[test]
fn a_cloud_page_size_above_the_maximum_is_refused_as_bitbucket_does() {
    let dir = tempfile::tempdir().unwrap();
    script(
        dir.path(),
        json!({"pulls": {"acme/api": [{"id": 12, "state": "OPEN"}]},
            "api": {"/repositories/acme/api/pullrequests/12/comments": {"values": []}}}),
    );
    let list = |limit: &str| {
        fake(dir.path())
            .args(["pr", "list", "--limit", limit, "--json"])
            .args(["--context", "cloud", "--workspace", "acme", "--repo", "api"])
            .output()
            .unwrap()
    };
    let comments = |pagelen: &str| {
        fake(dir.path())
            .args(["api", "/repositories/acme/api/pullrequests/12/comments"])
            .args(["--context", "cloud", "--param", pagelen])
            .output()
            .unwrap()
    };

    let (list_50, list_51) = (list("50"), list("51"));
    let (comments_50, comments_51) = (comments("pagelen=50"), comments("pagelen=51"));

    assert!(list_50.status.success(), "{list_50:?}");
    assert!(comments_50.status.success(), "{comments_50:?}");
    for refused in [list_51, comments_51] {
        assert_eq!(refused.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("400 Bad Request: Invalid pagelen"),
            "{refused:?}"
        );
    }
}

#[test]
fn a_body_with_more_values_than_a_page_is_served_page_by_page_through_next() {
    let dir = tempfile::tempdir().unwrap();
    let values: Vec<Value> = (1..=5).map(|id| json!({"id": id})).collect();
    script(
        dir.path(),
        json!({"api": {
            "/repositories/acme/api/pullrequests/12/comments": {"values": values},
            "/rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities": {"values": values}
        }}),
    );
    let api = |path: &str, params: &[&str]| {
        let mut command = fake(dir.path());
        command.args(["api", path, "--context", "cloud"]);
        for param in params {
            command.args(["--param", param]);
        }
        json_of(command.output().unwrap())
    };

    let first = api(
        "/repositories/acme/api/pullrequests/12/comments",
        &["pagelen=2"],
    );
    let next = first["next"].as_str().unwrap().to_owned();
    let second = api(&next, &[]);
    let third = api(second["next"].as_str().unwrap(), &[]);
    let dc_first = api(
        "/rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities",
        &["limit=3"],
    );
    let dc_second = api(
        "/rest/api/1.0/projects/APP/repos/web/pull-requests/7/activities",
        &["limit=3", "start=3"],
    );

    let ids = |page: &Value| -> Vec<u64> {
        page["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(ids(&first), [1, 2]);
    assert_eq!(
        next,
        "https://api.bitbucket.org/2.0/repositories/acme/api/pullrequests/12/comments?pagelen=2&page=2"
    );
    assert_eq!(ids(&second), [3, 4]);
    assert_eq!(ids(&third), [5]);
    assert!(third["next"].is_null());
    assert_eq!(
        (ids(&dc_first), &dc_first["nextPageStart"]),
        (vec![1, 2, 3], &json!(3))
    );
    assert_eq!(
        (ids(&dc_second), &dc_second["isLastPage"]),
        (vec![4, 5], &json!(true))
    );
}

#[test]
fn pipeline_fail_makes_the_pipeline_list_alone_fail() {
    let dir = tempfile::tempdir().unwrap();
    script(
        dir.path(),
        json!({"pipelines": {"acme/api": [{"build_number": 1}]},
            "pipeline_fail": {"acme/api": "Error: 502 Bad Gateway"}}),
    );
    let run = |args: &[&str]| {
        fake(dir.path())
            .args(args)
            .args(["--context", "cloud", "--workspace", "acme", "--repo", "api"])
            .output()
            .unwrap()
    };

    let listed = run(&["pipeline", "list", "--json"]);
    let pulls = run(&["pr", "list", "--json"]);

    assert_eq!(listed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&listed.stderr).contains("502 Bad Gateway"));
    assert!(pulls.status.success());
}
