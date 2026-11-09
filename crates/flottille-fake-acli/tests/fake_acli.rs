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

    let keys = |output: &std::process::Output| -> Vec<String> {
        let items: Value = serde_json::from_slice(&output.stdout).unwrap();
        items
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["key"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(keys(&all), ["ABC-2", "ABC-1"]);
    assert_eq!(keys(&one), ["ABC-2"]);
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
fn a_search_asked_only_the_key_answers_a_null_per_work_item_as_the_real_one() {
    let dir = scripted(json!({"search": [{"key": "ABC-2"}, {"key": "ABC-1"}]}));
    let search = |fields: &str| {
        let output = fake(dir.path())
            .args(["jira", "workitem", "search", "--jql", "project = ABC"])
            .args(["--fields", fields, "--json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };

    assert_eq!(search("key"), json!([null, null]));
    assert_eq!(search("key,summary")[1]["key"], "ABC-1");
}

#[test]
fn a_search_prints_jira_s_issue_with_only_the_fields_asked_counts_and_pages() {
    let item = |key: &str| {
        json!({"key": key, "self": format!("https://example.atlassian.net/rest/api/3/issue/{key}"),
            "fields": {"summary": "Log in", "status": {"name": "To Do"}}})
    };
    let dir = scripted(json!({"search": [item("ABC-1"), item("ABC-2"), item("ABC-3")], "page": 2}));
    let search = |extra: &[&str]| {
        let output = fake(dir.path())
            .args([
                "jira",
                "workitem",
                "search",
                "--jql",
                "project = ABC",
                "--json",
            ])
            .args(extra)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        String::from_utf8(output.stdout).unwrap()
    };

    let found: Value = serde_json::from_str(&search(&["--fields", "key,summary"])).unwrap();
    assert_eq!(
        found.as_array().unwrap().len(),
        2,
        "a page without --paginate"
    );
    assert_eq!(found[0]["key"], "ABC-1");
    assert!(found[0]["changelog"].is_null() && found[0]["id"].is_string());
    let all: Value = serde_json::from_str(&search(&["--paginate", "--limit", "1000"])).unwrap();
    assert_eq!(all.as_array().unwrap().len(), 3);
    assert_eq!(
        search(&["--count", "--paginate"]).trim(),
        "✓ Number of work items in the search: 3"
    );
}

#[test]
fn a_search_prints_only_the_fields_it_was_asked_and_a_view_gives_updated() {
    let item = json!({"key": "ABC-1", "self": "https://example.atlassian.net/rest/api/3/issue/1",
        "fields": {"summary": "Log in", "status": {"name": "To Do"},
            "updated": "2026-11-05T11:23:09.006+0100", "comment": {"comments": [], "total": 0}}});
    let dir = scripted(json!({"search": [item.clone()], "items": {"ABC-1": item}}));
    let run = |args: &[&str]| {
        let output = fake(dir.path()).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };

    let found = run(&[
        "jira",
        "workitem",
        "search",
        "--jql",
        "project = ABC",
        "--json",
        "--fields",
        "key,summary",
    ]);
    assert_eq!(found[0]["fields"], json!({"summary": "Log in"}));
    let all = run(&[
        "jira",
        "workitem",
        "search",
        "--jql",
        "project = ABC",
        "--json",
    ]);
    assert!(
        all[0]["fields"]["updated"].is_string(),
        "no --fields, the default is the script's"
    );

    let viewed = run(&[
        "jira",
        "workitem",
        "view",
        "ABC-1",
        "--json",
        "--fields",
        "updated,comment",
    ]);
    assert_eq!(viewed["fields"]["updated"], "2026-11-05T11:23:09.006+0100");
    assert!(viewed["fields"]["summary"].is_null());
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
fn a_view_gives_the_scripted_attachments_only_when_asked_for_them_and_a_comment_keeps_its_media() {
    let attachment = json!({"id": "10001", "filename": "image-20260101-120000.png",
        "mimeType": "image/png", "size": 97717, "created": "2026-01-01T12:00:00.000+0100",
        "author": {"displayName": "Ana Example"},
        "content": "https://example.atlassian.net/rest/api/3/attachment/content/10001"});
    let media = json!({"type": "mediaSingle", "content": [{"type": "media", "attrs": {
        "alt": "image-20260101-120000.png", "type": "file",
        "id": "00000000-0000-4000-8000-000000000001"}}]});
    let dir = scripted(json!({"items": {"ABC-1": {"key": "ABC-1", "fields": {
        "attachment": [attachment.clone()],
        "comment": {"comments": [{"body": {"type": "doc", "content": [media.clone()]}}]}}}}}));

    let without = view_of(dir.path(), Some("comment"));
    let with = view_of(dir.path(), Some("comment,attachment"));

    assert_eq!(without["fields"].get("attachment"), None);
    assert_eq!(with["fields"]["attachment"], json!([attachment]));
    assert_eq!(
        with["fields"]["comment"]["comments"][0]["body"]["content"],
        json!([media])
    );
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

#[test]
fn a_project_s_boards_and_their_open_sprints_follow_the_script() {
    let dir = scripted(json!({
        "boards": {"ABC": [{"id": 7, "name": "ABC board", "type": "scrum"},
            {"id": 8, "name": "ABC kanban", "type": "simple"}]},
        "sprints": {"7": [{"id": 41, "name": "Sprint 41", "state": "closed"},
            {"id": 42, "name": "Sprint 42", "state": "active"},
            {"id": 43, "name": "Sprint 43", "state": "future"}]}}));
    let run = |args: &[&str]| fake(dir.path()).args(args).output().unwrap();

    let boards = run(&["jira", "board", "search", "--project", "ABC", "--json"]);
    let none = run(&["jira", "board", "search", "--project", "XYZ", "--json"]);
    let open = run(&[
        "jira",
        "board",
        "list-sprints",
        "--id",
        "7",
        "--state",
        "active,future",
        "--json",
    ]);
    let kanban = run(&["jira", "board", "list-sprints", "--id", "8", "--json"]);

    let boards: Value = serde_json::from_slice(&boards.stdout).unwrap();
    assert_eq!(boards["values"][0]["name"], "ABC board");
    assert_eq!(boards["values"].as_array().unwrap().len(), 2);
    let none: Value = serde_json::from_slice(&none.stdout).unwrap();
    assert_eq!(none["values"], json!([]));
    let open: Value = serde_json::from_slice(&open.stdout).unwrap();
    // Under `sprints`, not `values` as the boards' page: as a real board answered (#657).
    let names: Vec<&str> = open["sprints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Sprint 42", "Sprint 43"]);
    // As a real site answers for a board without sprints.
    assert_eq!(kanban.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&kanban.stderr).contains("does not support sprints"));
}

#[test]
fn an_assignee_the_site_does_not_know_is_refused_as_the_real_one_does() {
    let dir = scripted(json!({"users": ["ana@example.org"]}));
    let run = |args: &[&str]| {
        let mut all = vec!["jira", "workitem"];
        all.extend(args);
        fake(dir.path()).args(all).output().unwrap()
    };
    let create = |who: &str| {
        run(&[
            "create",
            "--project",
            "ABC",
            "--summary",
            "S",
            "--assignee",
            who,
            "--json",
        ])
    };

    let known = create("ana@example.org");
    let me = create("@me");
    let unknown = create("bob@example.org");
    let name = create("Kristen");
    let edited = run(&[
        "edit",
        "--key",
        "ABC-1",
        "--assignee",
        "bob@example.org",
        "--yes",
    ]);

    assert!(known.status.success());
    assert!(me.status.success());
    assert_eq!(unknown.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&unknown.stderr)
            .contains("User not found for email: bob@example.org")
    );
    assert_eq!(name.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&name.stderr).contains("invalid format for assignee email"));
    // The real `edit` is a bulk command: it ends with 0 and says its failure on its output.
    assert!(edited.status.success());
    assert!(
        String::from_utf8_lossy(&edited.stdout).starts_with("✗ Failure: ABC-1 can't be edited")
    );
}

#[test]
fn a_creation_from_json_is_recorded_and_needs_its_type() {
    let dir = scripted(json!({"created": "ABC-11"}));
    let file = dir.path().join("item.json");
    let create = |item: Value| {
        std::fs::write(&file, item.to_string()).unwrap();
        fake(dir.path())
            .args(["jira", "workitem", "create", "--from-json"])
            .arg(&file)
            .arg("--json")
            .output()
            .unwrap()
    };

    let made = create(json!({"projectKey": "ABC", "summary": "S", "type": "Task",
        "additionalAttributes": {"customfield_10020": 42}}));
    let untyped = create(json!({"projectKey": "ABC", "summary": "S"}));
    let me =
        create(json!({"projectKey": "ABC", "summary": "S", "type": "Task", "assignee": "@me"}));

    assert_eq!(
        serde_json::from_slice::<Value>(&made.stdout).unwrap()["key"],
        "ABC-11"
    );
    assert_eq!(untyped.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&untyped.stderr)
            .contains("no value given for required property type")
    );
    // As measured: `@me` is acli's flag's, not its JSON's.
    assert_eq!(me.status.code(), Some(1));
    let received = std::fs::read_to_string(dir.path().join("received.jsonl")).unwrap();
    let first: Value = serde_json::from_str(received.lines().next().unwrap()).unwrap();
    assert_eq!(
        first["files"][0]["json"]["additionalAttributes"]["customfield_10020"],
        42
    );
}

#[test]
fn a_view_asked_the_navigable_fields_gives_them_all() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"items": {"ABC-1": {"key": "ABC-1",
            "fields": {"summary": "Log in", "customfield_10020": [{"id": 42}]}}}})
        .to_string(),
    )
    .unwrap();

    let all = view_of(dir.path(), Some("summary,*navigable"));

    assert_eq!(all["fields"]["customfield_10020"][0]["id"], 42);
}

#[test]
fn a_creation_the_script_says_answers_no_key_prints_what_the_script_says() {
    let dir = scripted(json!({"created_said": "✓ Work item created"}));
    let file = dir.path().join("item.json");
    std::fs::write(
        &file,
        json!({"projectKey": "ABC", "summary": "S", "type": "Task"}).to_string(),
    )
    .unwrap();

    let from_json = fake(dir.path())
        .args(["jira", "workitem", "create", "--from-json"])
        .arg(&file)
        .arg("--json")
        .output()
        .unwrap();
    let flags = fake(dir.path())
        .args([
            "jira",
            "workitem",
            "create",
            "--project",
            "ABC",
            "--summary",
            "S",
            "--json",
        ])
        .output()
        .unwrap();

    for output in [from_json, flags] {
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "✓ Work item created"
        );
    }
}
