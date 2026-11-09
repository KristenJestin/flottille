//! Copying a task to another project (`task.copy`, `docs/protocol.md`, "The task methods"): it
//! is written in the target as adding it by hand writes it, a name the target has is refused and
//! nothing is written, and "where it runs" is kept only when the target has a repository of
//! that name. Throwaway repositories and temporary XDG folders.

mod common;

use common::User;
use serde_json::json;

/// A project `name` whose repositories are `at` below its folder (`""`: the folder itself).
fn project(user: &User, name: &str, repositories: &[&str]) {
    let folder = user.path(&format!("work/{name}"));
    for at in repositories {
        let dir = if at.is_empty() {
            folder.clone()
        } else {
            folder.join(at)
        };
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), "start\n").unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    }
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn file(user: &User, project: &str) -> String {
    std::fs::read_to_string(user.project_file(project)).unwrap()
}

#[test]
fn copying_a_task_writes_it_in_the_other_project() {
    let user = User::new();
    project(&user, "app", &[""]);
    project(&user, "site", &[""]);
    user.call(
        "task.add",
        json!({"project": "app", "name": "dev", "line": "bun run dev", "repository": ".",
               "serve": true, "ask_before_running": true,
               "ready": {"line": "ready in"}, "url": "http://localhost:3000"}),
    )
    .unwrap();

    let copied = user
        .call(
            "task.copy",
            json!({"project": "app", "name": "dev", "to": "site"}),
        )
        .unwrap();
    assert_eq!(copied["project"], "site");
    assert_eq!(copied["name"], "dev");

    let shown = user
        .call("task.show", json!({"project": "site", "name": "dev"}))
        .unwrap();
    assert_eq!(shown["line"], "bun run dev");
    assert_eq!(shown["repository"], ".");
    assert_eq!(shown["serve"], true);
    assert_eq!(shown["ask_before_running"], true);
    assert_eq!(shown["check"], false);
    assert_eq!(shown["ready"], json!({"line": "ready in"}));
    assert_eq!(shown["url"], "http://localhost:3000");
    // The source is untouched.
    assert!(file(&user, "app").contains("bun run dev"));
}

#[test]
fn a_name_the_target_has_is_refused_and_nothing_is_written() {
    let user = User::new();
    project(&user, "app", &[""]);
    project(&user, "site", &[""]);
    for (project, line) in [("app", "make check"), ("site", "make site")] {
        user.call(
            "task.add",
            json!({"project": project, "name": "check", "line": line}),
        )
        .unwrap();
    }
    let before = file(&user, "site");

    let (code, message) = user
        .call(
            "task.copy",
            json!({"project": "app", "name": "check", "to": "site"}),
        )
        .unwrap_err();
    assert_eq!(code, "refused");
    assert!(
        message.contains("already has a task named check"),
        "{message}"
    );
    assert_eq!(file(&user, "site"), before);
}

#[test]
fn where_it_runs_is_kept_when_the_target_has_that_repository_else_the_places_folder() {
    let user = User::new();
    project(&user, "duo", &["api", "web"]);
    project(&user, "same", &["api"]);
    project(&user, "other", &["server"]);
    user.call(
        "task.add",
        json!({"project": "duo", "name": "build", "line": "make", "repository": "api"}),
    )
    .unwrap();

    for (to, repository) in [("same", json!("api")), ("other", json!(null))] {
        user.call(
            "task.copy",
            json!({"project": "duo", "name": "build", "to": to}),
        )
        .unwrap();
        let shown = user
            .call("task.show", json!({"project": to, "name": "build"}))
            .unwrap();
        assert_eq!(shown["repository"], repository, "in {to}");
    }
}

#[test]
fn an_unknown_task_or_project_is_not_found_and_a_project_is_not_a_target_of_itself() {
    let user = User::new();
    project(&user, "app", &[""]);
    project(&user, "site", &[""]);
    user.call(
        "task.add",
        json!({"project": "app", "name": "dev", "line": "bun run dev"}),
    )
    .unwrap();

    for (params, code) in [
        (
            json!({"project": "app", "name": "nope", "to": "site"}),
            "not-found",
        ),
        (
            json!({"project": "app", "name": "dev", "to": "nowhere"}),
            "not-found",
        ),
        (
            json!({"project": "app", "name": "dev", "to": "app"}),
            "refused",
        ),
    ] {
        let (got, message) = user.call("task.copy", params.clone()).unwrap_err();
        assert_eq!(got, code, "{params}: {message}");
    }
}
