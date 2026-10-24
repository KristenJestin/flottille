//! The `flottille project` commands, against the real binary, with throwaway repositories and
//! temporary XDG folders. No test waits on a terminal: the standard input is closed or piped.

mod common;

use std::path::Path;
use std::process::Output;

use common::{User, snapshot};
use serde_json::{Value, json};

fn path(folder: &Path) -> &str {
    folder.to_str().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A project `app` of one repository, registered.
fn registered(user: &User) -> std::path::PathBuf {
    let app = user.path("work/app");
    user.git_init(&app);
    let output = user.run(&["project", "add", path(&app), "--yes"]);
    assert!(output.status.success(), "{}", stderr(&output));
    app
}

#[test]
fn add_shows_the_repositories_and_registers_them_with_yes() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    user.git_init(&app.join("web"));

    let output = user.run(&["project", "add", path(&app), "--yes"]);

    assert!(output.status.success(), "{}", stderr(&output));
    let shown = stderr(&output);
    assert!(
        shown.contains("Project app") && shown.contains("2 repositories"),
        "{shown}"
    );
    assert!(shown.contains("  web"), "{shown}");
    assert!(
        stdout(&output).contains("Registered project app"),
        "{}",
        stdout(&output)
    );
    assert!(user.project_file("app").is_file());
}

#[test]
fn add_with_a_name_registers_under_that_name() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);

    let output = user.run(&[
        "project",
        "add",
        path(&app),
        "--name",
        "shop",
        "--yes",
        "--json",
    ]);

    assert!(output.status.success(), "{}", stderr(&output));
    let project: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(project["name"], "shop");
    assert!(user.project_file("shop").is_file());
}

#[test]
fn add_without_yes_and_a_closed_input_refuses_and_writes_nothing() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);

    let output = user.run(&["project", "add", path(&app)]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("--yes"), "{}", stderr(&output));
    assert!(!user.project_file("app").exists());
}

#[test]
fn add_registers_only_when_the_answer_is_yes() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);

    for no in ["n\n", "\n", "maybe\n"] {
        let output = user.run_with_input(&["project", "add", path(&app)], no);
        assert!(!output.status.success(), "{no:?}");
        assert!(
            stderr(&output).contains("not registered"),
            "{no:?}: {}",
            stderr(&output)
        );
        assert!(!user.project_file("app").exists(), "{no:?}");
    }
    let output = user.run_with_input(&["project", "add", path(&app)], "Yes\n");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(user.project_file("app").is_file());
}

#[test]
fn add_takes_a_folder_relative_to_where_the_command_runs() {
    let user = User::new();
    user.git_init(&user.path("work/app"));

    let output = user
        .command(&["project", "add", "app", "--yes"])
        .current_dir(user.path("work"))
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    let shown = user.run(&["project", "show", "app", "--json"]);
    let project: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(project["folder"], path(&user.path("work/app")));
}

#[test]
fn add_refuses_a_folder_with_no_repository_in_a_sentence() {
    let user = User::new();
    let notes = user.path("work/notes");
    std::fs::create_dir_all(&notes).unwrap();

    let output = user.run(&["project", "add", path(&notes), "--yes"]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).starts_with("flottille: no Git repository in "),
        "{}",
        stderr(&output)
    );
    assert!(!user.project_file("notes").exists());
}

#[test]
fn list_shows_the_projects_and_says_when_there_are_none() {
    let user = User::new();
    assert!(stdout(&user.run(&["project", "list"])).contains("No project is registered"));
    assert_eq!(
        stdout(&user.run(&["project", "list", "--json"])).trim(),
        r#"{"projects":[],"errors":[]}"#
    );
    let app = registered(&user);

    let text = stdout(&user.run(&["project", "list"]));
    let json: Value =
        serde_json::from_slice(&user.run(&["project", "list", "--json"]).stdout).unwrap();

    let row = text.lines().nth(1).unwrap();
    assert!(text.starts_with("NAME"), "{text}");
    assert!(
        row.starts_with("app") && row.contains(path(&app)) && row.ends_with('1'),
        "{text}"
    );
    assert_eq!(json["projects"][0]["name"], "app");
}

#[test]
fn list_names_the_file_and_the_key_of_a_configuration_it_cannot_use() {
    let user = User::new();
    registered(&user);
    user.write_project_file(
        "bad",
        "folder = \"/work/bad\"\nmax_children = 9\n[[repositories]]\npath = \".\"\n",
    );

    let output = user.run(&["project", "list"]);

    assert!(!output.status.success());
    let file = user.project_file("bad");
    assert!(
        stderr(&output).contains(&format!(
            "{}: max_children: must be between 1 and 6, not 9",
            file.display()
        )),
        "{}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("app"),
        "the usable project is still listed"
    );
}

#[test]
fn show_of_a_configuration_it_cannot_use_names_the_file_by_its_path() {
    let user = User::new();
    user.write_project_file(
        "bad",
        "folder = \"/work/bad\"\nmax_children = 9\n[[repositories]]\npath = \".\"\n",
    );

    let output = user.run(&["project", "show", "bad"]);

    assert!(!output.status.success());
    assert_eq!(
        stderr(&output).trim(),
        format!(
            "flottille: {}: max_children: must be between 1 and 6, not 9",
            user.project_file("bad").display()
        )
    );
}

#[test]
fn show_prints_the_configuration_for_a_person_and_as_json() {
    let user = User::new();
    let app = registered(&user);
    user.write_project_file(
        "rich",
        &format!(
            r#"folder = "{folder}"
never = [["git", "push"], ["rm", "-rf"]]
[[repositories]]
path = "."
base = ["develop", "main"]
[[repositories]]
path = "lib"
depends_on = ["."]
[variables]
STORE = "{{workspace.path}}/.store"
[commands."api:dev"]
line = "npm start"
repository = "lib"
serve = true
check = true
"#,
            folder = path(&app)
        ),
    );

    let text = stdout(&user.run(&["project", "show", "rich"]));
    let json: Value =
        serde_json::from_slice(&user.run(&["project", "show", "rich", "--json"]).stdout).unwrap();

    for expected in [
        "rich",
        &format!("folder: {}", path(&app)),
        "branch prefix: rich",
        "workspaces take: all repositories",
        "children at once: 3",
        ".: remote origin, base develop then main",
        "lib: remote origin, base main, after .",
        "STORE = {workspace.path}/.store",
        "api:dev: npm start (in lib; serve, check)",
        "never: git push, rm -rf",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert_eq!(json["name"], "rich");
    assert_eq!(json["repositories"][1]["depends_on"], json!(["."]));
}

#[test]
fn show_of_an_unknown_project_fails_with_a_sentence() {
    let user = User::new();

    let output = user.run(&["project", "show", "nope"]);

    assert!(!output.status.success());
    assert_eq!(stderr(&output).trim(), "flottille: no project named nope");
}

#[test]
fn remove_forgets_the_project_and_leaves_its_folder() {
    let user = User::new();
    let app = registered(&user);
    let before = snapshot(&app);

    let output = user.run(&["project", "remove", "app"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Removed project app"),
        "{}",
        stdout(&output)
    );
    assert!(stdout(&output).contains("folder was not touched"));
    assert_eq!(snapshot(&app), before);
    assert!(!user.project_file("app").exists());
    let again = user.run(&["project", "remove", "app", "--json"]);
    assert!(!again.status.success());
}

#[test]
fn remove_with_json_says_what_was_removed() {
    let user = User::new();
    registered(&user);

    let output = user.run(&["project", "remove", "app", "--json"]);

    assert_eq!(stdout(&output).trim(), r#"{"removed":"app"}"#);
}

#[test]
fn check_is_quiet_when_nothing_differs_and_fails_when_something_does() {
    let user = User::new();
    let app = registered(&user);
    let same = user.run(&["project", "check", "app"]);
    assert!(same.status.success(), "{}", stderr(&same));
    assert!(
        stdout(&same).contains("match the configuration"),
        "{}",
        stdout(&same)
    );

    user.git_init(&app.join("api"));
    let output = user.run(&["project", "check", "app"]);

    assert!(!output.status.success());
    assert!(
        stdout(&output).contains("not in the configuration: api"),
        "{}",
        stdout(&output)
    );
    let json = user.run(&["project", "check", "app", "--json"]);
    assert!(!json.status.success());
    let result: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(result["new"], json!(["api"]));

    std::fs::remove_dir_all(app.join(".git")).unwrap();
    let missing = user.run(&["project", "check", "app"]);
    assert!(
        stdout(&missing).contains("in the configuration, not found: ."),
        "{}",
        stdout(&missing)
    );
}
