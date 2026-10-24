//! The scenarios of registering projects, asked of the daemon over its socket, with throwaway
//! repositories and temporary XDG folders.

mod common;

use std::path::Path;

use common::{User, snapshot};
use serde_json::{Value, json};

fn folder(path: &Path) -> Value {
    json!({"folder": path})
}

fn add(user: &User, path: &Path) -> Value {
    user.call("project.add", folder(path)).unwrap()
}

fn repository_paths(project: &Value) -> Vec<&str> {
    project["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|repository| repository["path"].as_str().unwrap())
        .collect()
}

#[test]
fn a_single_repository_is_registered() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);

    let project = add(&user, &app);

    assert_eq!(project["name"], "app");
    assert_eq!(project["folder"], app.to_str().unwrap());
    assert_eq!(repository_paths(&project), ["."]);
    assert_eq!(project["repositories"][0]["remote"], "origin");
    assert_eq!(project["repositories"][0]["base"], json!(["main"]));
    assert_eq!(project["repositories"][0]["depends_on"], json!([]));
    assert_eq!(project["branch_prefix"], "app");
    assert_eq!(
        project["workspaces_root"],
        user.path("data/flottille/workspaces/app").to_str().unwrap()
    );
    assert_eq!(project["default_repositories"], "all");
    assert_eq!(project["max_children"], 3);
    assert!(user.project_file("app").is_file());
}

#[test]
fn a_folder_holding_three_repositories_and_being_one_registers_all_four() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    for name in ["web", "api", "lib"] {
        user.git_init(&app.join(name));
    }
    std::fs::write(app.join(".gitignore"), "web/\napi/\nlib/\n").unwrap();

    let project = add(&user, &app);

    assert_eq!(repository_paths(&project), [".", "api", "lib", "web"]);
}

#[test]
fn a_repository_two_levels_down_is_registered() {
    let user = User::new();
    let shop = user.path("work/shop");
    user.git_init(&shop.join("services/orders"));
    std::fs::create_dir_all(shop.join("docs")).unwrap();

    let project = add(&user, &shop);

    assert_eq!(repository_paths(&project), ["services/orders"]);
}

#[test]
fn a_git_file_counts_as_a_repository() {
    let user = User::new();
    let shop = user.path("work/shop");
    user.git_init(&shop.join("main"));
    user.git(
        &shop.join("main"),
        &["commit", "--quiet", "--allow-empty", "-m", "start"],
    );
    user.git(
        &shop.join("main"),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "topic",
            shop.join("topic").to_str().unwrap(),
        ],
    );
    assert!(shop.join("topic/.git").is_file());

    let project = add(&user, &shop);

    assert_eq!(repository_paths(&project), ["main", "topic"]);
}

#[test]
fn a_folder_with_no_repository_is_refused_with_a_clear_message() {
    let user = User::new();
    let empty = user.path("work/notes");
    std::fs::create_dir_all(empty.join("a/b")).unwrap();

    for method in ["project.detect", "project.add"] {
        let (code, message) = user.call(method, folder(&empty)).unwrap_err();

        assert_eq!(code, "refused", "{method}");
        assert!(
            message.contains("no Git repository") && message.contains("one or two levels down"),
            "{method}: {message}"
        );
    }
    assert!(!user.project_file("notes").exists());
    assert_eq!(
        user.call("project.list", json!({})).unwrap()["projects"],
        json!([])
    );
}

#[test]
fn detecting_says_what_would_be_registered_and_registers_nothing() {
    let user = User::new();
    let app = user.path("work/My App");
    user.git_init(&app);
    user.git_init(&app.join("web"));

    let found = user.call("project.detect", folder(&app)).unwrap();

    assert_eq!(found["name"], "my-app");
    assert_eq!(found["folder"], app.to_str().unwrap());
    assert_eq!(found["repositories"], json!([".", "web"]));
    assert!(!user.project_file("my-app").exists());
}

#[test]
fn the_name_is_given_or_comes_from_the_folder() {
    let user = User::new();
    let app = user.path("work/My App");
    user.git_init(&app);

    let (code, message) = user
        .call("project.add", json!({"folder": app, "name": "a b"}))
        .unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("letters, digits"), "{message}");

    let project = user
        .call("project.add", json!({"folder": app, "name": "Shop_2"}))
        .unwrap();
    assert_eq!(project["name"], "Shop_2");
    assert_eq!(project["branch_prefix"], "shop-2");
    assert!(user.project_file("Shop_2").is_file());
}

#[test]
fn a_folder_that_cannot_be_registered_is_refused_in_a_sentence() {
    let user = User::new();
    let file = user.path("work/file");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "x").unwrap();
    for (folder, expected) in [
        (json!("relative/folder"), "must be an absolute path"),
        (json!(user.path("work/nope")), "does not exist"),
        (json!(file), "is not a folder"),
    ] {
        let (code, message) = user
            .call("project.add", json!({ "folder": folder }))
            .unwrap_err();

        assert_eq!(code, "refused");
        assert!(message.contains(expected), "{message}");
    }
}

#[test]
fn a_name_or_a_folder_cannot_be_registered_twice() {
    let user = User::new();
    let app = user.path("work/app");
    let other = user.path("work/other");
    user.git_init(&app);
    user.git_init(&other);
    add(&user, &app);

    let (code, same_folder) = user
        .call("project.add", json!({"folder": app, "name": "second"}))
        .unwrap_err();
    let (_, same_name) = user
        .call("project.add", json!({"folder": other, "name": "app"}))
        .unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        same_folder.contains("already registered as app"),
        "{same_folder}"
    );
    assert!(same_name.contains("already exists"), "{same_name}");
    assert!(!user.project_file("second").exists());
}

#[test]
fn registered_projects_are_listed_and_shown() {
    let user = User::new();
    for name in ["b-app", "a-app"] {
        user.git_init(&user.path(&format!("work/{name}")));
        add(&user, &user.path(&format!("work/{name}")));
    }

    let list = user.call("project.list", json!({})).unwrap();
    let shown = user.call("project.show", json!({"name": "a-app"})).unwrap();

    let names: Vec<_> = list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|project| project["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a-app", "b-app"]);
    assert_eq!(list["errors"], json!([]));
    assert_eq!(shown, list["projects"][0]);
    let (code, message) = user
        .call("project.show", json!({"name": "nope"}))
        .unwrap_err();
    assert_eq!(code, "not-found");
    assert!(message.contains("no project named nope"), "{message}");
}

#[test]
fn a_file_that_cannot_be_used_is_reported_with_its_file_and_key() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    add(&user, &app);
    let invalid =
        |extra: &str| format!("folder = \"/work/x\"\n{extra}\n[[repositories]]\npath = \"a\"\n");
    user.write_project_file("cap", &invalid("max_children = 9"));
    user.write_project_file("base", &invalid("").replace("path", "base = []\npath"));
    user.write_project_file(
        "dependency",
        &invalid("").replace("path = \"a\"", "path = \"a\"\ndepends_on = [\"x\"]"),
    );

    let list = user.call("project.list", json!({})).unwrap();

    let errors: Vec<String> = list["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            format!(
                "{}: {}",
                e["file"].as_str().unwrap(),
                e["message"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        errors,
        [
            "projects/base.toml: repositories[0].base: must name at least one branch",
            "projects/cap.toml: max_children: must be between 1 and 6, not 9",
            r#"projects/dependency.toml: repositories[0].depends_on: unknown repository "x""#,
        ]
    );
    assert_eq!(
        list["projects"].as_array().unwrap().len(),
        1,
        "app is still listed"
    );
    let (code, message) = user
        .call("project.show", json!({"name": "cap"}))
        .unwrap_err();
    assert_eq!(code, "invalid-config");
    assert!(
        message.starts_with("projects/cap.toml: max_children"),
        "{message}"
    );
}

#[test]
fn registering_writes_nothing_into_the_users_folders() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    user.git_init(&app.join("web"));
    let before = snapshot(&app);

    user.call("project.detect", folder(&app)).unwrap();
    add(&user, &app);
    user.call("project.check", json!({"name": "app"})).unwrap();

    assert_eq!(snapshot(&app), before);
}

#[test]
fn removing_a_project_never_touches_the_folder() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    user.git_init(&app.join("web"));
    std::fs::write(app.join("notes.txt"), "keep me").unwrap();
    add(&user, &app);
    let before = snapshot(&app);

    let removed = user.call("project.remove", json!({"name": "app"})).unwrap();

    assert_eq!(removed["removed"], "app");
    assert_eq!(snapshot(&app), before);
    assert_eq!(
        std::fs::read_to_string(app.join("notes.txt")).unwrap(),
        "keep me"
    );
    assert!(!user.project_file("app").exists());
    let (code, _) = user
        .call("project.show", json!({"name": "app"}))
        .unwrap_err();
    assert_eq!(code, "not-found");
    let (code, _) = user
        .call("project.remove", json!({"name": "app"}))
        .unwrap_err();
    assert_eq!(code, "not-found");
}

#[test]
fn check_reports_the_differences_with_the_configuration() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    user.git_init(&app.join("web"));
    add(&user, &app);
    let same = user.call("project.check", json!({"name": "app"})).unwrap();
    assert_eq!(same["new"], json!([]));
    assert_eq!(same["missing"], json!([]));

    user.git_init(&app.join("api"));
    std::fs::remove_dir_all(app.join("web/.git")).unwrap();
    let changed = user.call("project.check", json!({"name": "app"})).unwrap();

    assert_eq!(changed["name"], "app");
    assert_eq!(changed["new"], json!(["api"]));
    assert_eq!(changed["missing"], json!(["web"]));
    let after = user.call("project.show", json!({"name": "app"})).unwrap();
    assert_eq!(
        repository_paths(&after),
        [".", "web"],
        "check changes nothing"
    );

    std::fs::remove_dir_all(&app).unwrap();
    let (code, message) = user
        .call("project.check", json!({"name": "app"}))
        .unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("no longer exists"), "{message}");
}

#[test]
fn adding_changing_and_removing_a_project_are_events() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    let watch = user.watch(&["--after", "0"]);
    assert_eq!(watch.next_event()["name"], "daemon.started");

    add(&user, &app);
    let added = watch.next_event();
    assert_eq!(added["name"], "project.added");
    assert_eq!(added["data"], json!({"name": "app", "folder": app}));

    // Edited by hand: the daemon sees it at the next request about projects.
    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    std::fs::write(
        user.project_file("app"),
        text.replace("max_children = 3", "max_children = 5"),
    )
    .unwrap();
    user.call("project.list", json!({})).unwrap();
    let changed = watch.next_event();
    assert_eq!(changed["name"], "project.changed");
    assert_eq!(changed["data"], json!({"name": "app", "folder": app}));

    user.call("project.remove", json!({"name": "app"})).unwrap();
    let removed = watch.next_event();
    assert_eq!(removed["name"], "project.removed");
    assert_eq!(removed["data"], json!({"name": "app"}));

    // A file put there, then taken away by hand.
    user.write_project_file(
        "late",
        "folder = \"/work/late\"\n[[repositories]]\npath = \".\"\n",
    );
    user.call("project.list", json!({})).unwrap();
    assert_eq!(watch.next_event()["name"], "project.added");
    std::fs::remove_file(user.project_file("late")).unwrap();
    user.call("project.list", json!({})).unwrap();
    assert_eq!(watch.next_event()["name"], "project.removed");
}

#[test]
fn a_project_already_there_when_the_daemon_starts_is_not_announced() {
    let user = User::new();
    user.write_project_file(
        "old",
        "folder = \"/work/old\"\n[[repositories]]\npath = \".\"\n",
    );
    let watch = user.watch(&["--after", "0"]);
    assert_eq!(watch.next_event()["name"], "daemon.started");

    assert_eq!(
        user.call("project.list", json!({})).unwrap()["projects"][0]["name"],
        "old"
    );
    user.call("project.remove", json!({"name": "old"})).unwrap();

    assert_eq!(watch.next_event()["name"], "project.removed");
}

#[test]
fn a_list_that_differs_from_the_detection_is_refused() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    let shown = user.call("project.detect", folder(&app)).unwrap();
    user.git_init(&app.join("web"));

    for confirmed in [shown["repositories"].clone(), json!([".", "api"])] {
        let (code, message) = user
            .call(
                "project.add",
                json!({"folder": app, "repositories": confirmed}),
            )
            .unwrap_err();

        assert_eq!(code, "refused");
        assert!(
            message.contains("changed since they were shown") && message.contains("run it again"),
            "{message}"
        );
    }
    assert!(!user.project_file("app").exists());

    let project = user
        .call(
            "project.add",
            json!({"folder": app, "repositories": [".", "web"]}),
        )
        .unwrap();
    assert_eq!(repository_paths(&project), [".", "web"]);
}
