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

#[test]
fn a_search_answers_the_scripted_items_up_to_its_limit_and_needs_its_jql() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"search": [{"key": "ABC-2"}, {"key": "ABC-1"}]}).to_string(),
    )
    .unwrap();
    let search = |extra: &[&str]| {
        let mut args = vec!["jira", "workitem", "search"];
        args.extend(extra);
        fake(dir.path()).args(args).output().unwrap()
    };

    let all = search(&["--jql", "project = ABC", "--json"]);
    let one = search(&["--jql", "project = ABC", "--json", "--limit", "1"]);
    let without_jql = search(&["--json"]);

    let items: Value = serde_json::from_slice(&all.stdout).unwrap();
    assert_eq!(items, json!([{"key": "ABC-2"}, {"key": "ABC-1"}]));
    let items: Value = serde_json::from_slice(&one.stdout).unwrap();
    assert_eq!(items, json!([{"key": "ABC-2"}]));
    assert_eq!(without_jql.status.code(), Some(2));
}

#[test]
fn the_fake_acli_refuses_a_field_search_does_not_allow() {
    let dir = tempfile::tempdir().unwrap();
    let search = |fields: &str| {
        fake(dir.path())
            .args(["jira", "workitem", "search", "--jql", "project = ABC"])
            .args(["--fields", fields, "--json"])
            .output()
            .unwrap()
    };

    let refused = search("key,updated,summary,created");
    let allowed = search("key,summary,status,assignee,reporter,labels,issuetype,priority");

    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&refused.stderr).trim(),
        "✗ Error: fields 'updated, created' are not allowed"
    );
    assert!(refused.stdout.is_empty());
    assert_eq!(allowed.status.code(), Some(0));
}

#[test]
fn the_writes_follow_the_script_and_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"items": {"ABC-1": {"key": "ABC-1"}}, "created": "ABC-7",
            "transitions": {"ABC-1": ["In Progress"]}})
        .to_string(),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut all = vec!["jira", "workitem"];
        all.extend(args);
        fake(dir.path()).args(all).output().unwrap()
    };

    let created = run(&["create", "--project", "ABC", "--summary", "S", "--json"]);
    let commented = run(&["comment", "create", "--key", "ABC-1", "--body", "b"]);
    let elsewhere = run(&["comment", "create", "--key", "ABC-2", "--body", "b"]);
    let edited = run(&["edit", "--key", "ABC-1", "--summary", "S", "--yes"]);
    let moved = run(&[
        "transition",
        "--key",
        "ABC-1",
        "--status",
        "in progress",
        "--yes",
    ]);
    let not_moved = run(&["transition", "--key", "ABC-1", "--status", "Done", "--yes"]);

    assert_eq!(
        serde_json::from_slice::<Value>(&created.stdout).unwrap()["key"],
        "ABC-7"
    );
    assert!(commented.status.success());
    assert_eq!(elsewhere.status.code(), Some(1));
    assert!(edited.status.success());
    assert!(moved.status.success());
    assert_eq!(not_moved.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&not_moved.stderr).contains("no transition to status Done"));
}

/// The status a view of ABC-1 shows.
fn status_of(dir: &std::path::Path) -> Value {
    let viewed = fake(dir)
        .args(["jira", "workitem", "view", "ABC-1", "--json"])
        .output()
        .unwrap();
    serde_json::from_slice::<Value>(&viewed.stdout).unwrap()["fields"]["status"]["name"].clone()
}

fn transition(dir: &std::path::Path, status: &str) -> std::process::Output {
    fake(dir)
        .args([
            "jira",
            "workitem",
            "transition",
            "--key",
            "ABC-1",
            "--status",
            status,
            "--yes",
        ])
        .output()
        .unwrap()
}

fn scripted(extra: Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut script = json!({
        "items": {"ABC-1": {"key": "ABC-1", "fields": {"status": {"name": "To Do"}}}},
        "transitions": {"ABC-1": ["In Progress", "Done"]}});
    for (name, value) in extra.as_object().unwrap() {
        script[name] = value.clone();
    }
    std::fs::write(dir.path().join("script.json"), script.to_string()).unwrap();
    dir
}

#[test]
fn a_transition_moves_the_work_item_to_the_status_as_the_workflow_spells_it() {
    let dir = scripted(json!({}));

    let before = status_of(dir.path());
    let moved = transition(dir.path(), "in progress");

    assert_eq!(before, "To Do");
    assert!(moved.status.success());
    assert_eq!(status_of(dir.path()), "In Progress");
}

#[test]
fn a_transition_the_script_says_does_not_move_ends_with_0_and_moves_nothing() {
    let dir = scripted(json!({"no_move": ["ABC-1"]}));

    let moved = transition(dir.path(), "Done");

    assert!(moved.status.success());
    assert_eq!(status_of(dir.path()), "To Do");
}

#[test]
fn a_transition_the_script_kills_ends_by_a_signal_before_or_after_moving() {
    use std::os::unix::process::ExitStatusExt;
    for (when, status) in [("before", "To Do"), ("after", "Done")] {
        let dir = scripted(json!({"die": {"ABC-1": when}}));

        let killed = transition(dir.path(), "Done");

        assert!(killed.status.signal().is_some(), "{when}");
        assert_eq!(status_of(dir.path()), status, "{when}");
    }
}

#[test]
fn a_view_after_a_move_the_script_says_is_unreadable_ends_with_1() {
    let dir = scripted(json!({"unreadable_after_move": ["ABC-1"]}));

    assert_eq!(status_of(dir.path()), "To Do");
    transition(dir.path(), "Done");
    let viewed = fake(dir.path())
        .args(["jira", "workitem", "view", "ABC-1", "--json"])
        .output()
        .unwrap();

    assert_eq!(viewed.status.code(), Some(1));
}

/// What a view of ABC-1 prints, asked for `fields` (all of them without).
fn view_of(dir: &std::path::Path, fields: Option<&str>) -> Value {
    let mut args = vec!["jira", "workitem", "view", "ABC-1", "--json"];
    if let Some(fields) = fields {
        args.extend(["--fields", fields]);
    }
    let viewed = fake(dir).args(args).output().unwrap();
    serde_json::from_slice(&viewed.stdout).unwrap()
}

#[test]
fn a_view_shows_the_fields_it_was_asked_for_beside_its_key_and_self() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"items": {"ABC-1": {"key": "ABC-1", "self": "https://example.atlassian.net/x",
            "fields": {"summary": "Log in", "status": {"name": "To Do"},
                "comment": {"comments": []}}}}})
        .to_string(),
    )
    .unwrap();

    let status = view_of(dir.path(), Some("status"));
    let both = view_of(dir.path(), Some("summary,comment"));
    let all = view_of(dir.path(), None);

    assert_eq!(status["key"], "ABC-1");
    assert_eq!(status["self"], "https://example.atlassian.net/x");
    assert_eq!(status["fields"], json!({"status": {"name": "To Do"}}));
    assert_eq!(
        both["fields"],
        json!({"summary": "Log in", "comment": {"comments": []}})
    );
    assert_eq!(all["fields"].as_object().unwrap().len(), 3);
}

#[test]
fn a_comment_created_shows_in_the_next_view_as_the_script_s_user_wrote_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"user": "Casey Example",
            "items": {"ABC-1": {"key": "ABC-1", "fields": {"comment": {"comments": [
                {"author": {"displayName": "Alice"}, "body": "First"}]}}}}})
        .to_string(),
    )
    .unwrap();

    let posted = fake(dir.path())
        .args([
            "jira", "workitem", "comment", "create", "--key", "ABC-1", "--body", "Second",
        ])
        .status()
        .unwrap();

    assert!(posted.success());
    let comments = &view_of(dir.path(), Some("comment"))["fields"]["comment"]["comments"];
    assert_eq!(comments.as_array().unwrap().len(), 2, "{comments}");
    assert_eq!(comments[0]["body"], "First");
    assert_eq!(comments[1]["author"]["displayName"], "Casey Example");
    assert_eq!(comments[1]["body"], "Second");
}

#[test]
fn a_project_view_gives_the_scripted_types_and_a_creation_of_another_type_ends_with_1() {
    let dir = scripted(json!({"types": {"ABC": ["Task", {"name": "Sub-task", "subtask": true}]}}));

    let viewed = fake(dir.path())
        .args(["jira", "project", "view", "--key", "ABC", "--json"])
        .output()
        .unwrap();
    let unknown = fake(dir.path())
        .args(["jira", "project", "view", "--key", "XYZ", "--json"])
        .status()
        .unwrap();
    let create = |kind: &str, project: &str| {
        fake(dir.path())
            .args([
                "jira",
                "workitem",
                "create",
                "--project",
                project,
                "--summary",
                "s",
            ])
            .args(["--type", kind, "--json"])
            .status()
            .unwrap()
            .code()
    };

    let viewed: Value = serde_json::from_slice(&viewed.stdout).unwrap();
    assert_eq!(
        viewed["issueTypes"],
        json!([{"name": "Task", "subtask": false}, {"name": "Sub-task", "subtask": true}])
    );
    assert_eq!(unknown.code(), Some(1));
    assert_eq!(create("task", "ABC"), Some(0));
    assert_eq!(create("Tâche", "ABC"), Some(1));
    assert_eq!(
        create("Tâche", "XYZ"),
        Some(0),
        "no types scripted, none checked"
    );
}
