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
    assert_eq!(project["max_children"], 12);
    assert!(user.project_file("app").is_file());
}

#[test]
fn a_registered_project_s_file_does_not_pin_where_its_workspaces_go() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);

    add(&user, &app);

    // Pinned, the user's default for the workspaces would never reach this project.
    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!text.contains("workspaces_root"), "{text}");
    user.call("workspaces_folder.set", json!({"place": "beside"}))
        .unwrap();
    let project = user.call("project.show", json!({"name": "app"})).unwrap();
    assert_eq!(
        project["workspaces_root"],
        user.path("work/app-work").to_str().unwrap()
    );
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
    let mut listed = list["projects"][0].clone();
    for (repository, shown) in listed["repositories"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(shown["repositories"].as_array().unwrap())
    {
        repository["branch"] = shown["branch"].clone();
    }
    assert_eq!(
        shown, listed,
        "shown is listed, with each repository's branch"
    );
    let (code, message) = user
        .call("project.show", json!({"name": "nope"}))
        .unwrap_err();
    assert_eq!(code, "not-found");
    assert!(message.contains("no project named nope"), "{message}");
}

#[test]
fn a_shown_project_says_the_branch_each_repository_is_on() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    for name in ["lib", "tool"] {
        let repository = app.join(name);
        user.git_init(&repository);
        std::fs::write(repository.join("README.md"), "made up\n").unwrap();
        user.git(&repository, &["add", "README.md"]);
        user.git(&repository, &["commit", "--quiet", "-m", "start"]);
    }
    user.git(
        &app.join("lib"),
        &["checkout", "--quiet", "-b", "feature/x"],
    );
    user.git(&app.join("tool"), &["checkout", "--quiet", "--detach"]);
    add(&user, &app);

    let shown = user.call("project.show", json!({"name": "app"})).unwrap();

    let branches: Vec<(&str, &Value)> = shown["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|repository| (repository["path"].as_str().unwrap(), &repository["branch"]))
        .collect();
    assert_eq!(
        branches,
        [
            (".", &json!("main")),
            ("lib", &json!("feature/x")),
            ("tool", &Value::Null),
        ],
        "a detached checkout is on no branch"
    );
    let listed = user.call("project.list", json!({})).unwrap();
    assert!(
        listed["projects"][0]["repositories"][0]
            .get("branch")
            .is_none(),
        "a listing runs no git"
    );
}

#[test]
fn a_repository_folder_that_is_not_a_repository_yet_is_on_no_branch() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    std::fs::write(app.join("README.md"), "made up\n").unwrap();
    user.git(&app, &["add", "README.md"]);
    user.git(&app, &["commit", "--quiet", "-m", "start"]);
    // Inside the project's repository, not cloned yet: git would answer for the parent.
    std::fs::create_dir_all(app.join("lib")).unwrap();
    user.write_project_file(
        "app",
        &format!(
            "folder = \"{}\"\n[[repositories]]\npath = \".\"\n[[repositories]]\npath = \"lib\"\n",
            app.display()
        ),
    );

    let shown = user.call("project.show", json!({"name": "app"})).unwrap();

    assert_eq!(shown["repositories"][0]["branch"], "main");
    assert_eq!(
        shown["repositories"][1]["branch"],
        Value::Null,
        "not the branch of the repository around it"
    );
}

#[test]
fn a_file_that_cannot_be_used_is_reported_with_its_file_and_key() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    add(&user, &app);
    let invalid =
        |extra: &str| format!("folder = \"/work/x\"\n{extra}\n[[repositories]]\npath = \"a\"\n");
    user.write_project_file("cap", &invalid("max_children = 13"));
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
            "projects/cap.toml: max_children: must be between 1 and 12, not 13",
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
        text.replace("max_children = 12", "max_children = 5"),
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

#[test]
fn a_file_that_cannot_be_read_is_reported_as_itself_not_as_a_name_taken() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    user.write_project_file(
        "app",
        "folder = \"/work/old\"\n[[repositories]]\npath = \".\"\n",
    );
    std::fs::set_permissions(
        user.project_file("app"),
        std::os::unix::fs::PermissionsExt::from_mode(0o000),
    )
    .unwrap();

    let (_, message) = user.call("project.add", folder(&app)).unwrap_err();

    assert!(message.contains("ermission denied"), "{message}");
    assert!(!message.contains("already exists"), "{message}");
}

/// A project `app` of two repositories, `api` whose `origin` is on GitHub and `lib` on a path of
/// this machine, the project's file holding a comment of its user.
fn settings_fixture(user: &User) {
    let app = user.path("work/app");
    for name in ["api", "lib"] {
        user.git_init(&app.join(name));
    }
    user.git(
        &app.join("api"),
        &["remote", "add", "origin", "git@github.com:acme/api.git"],
    );
    user.git(
        &app.join("lib"),
        &["remote", "add", "origin", "/srv/git/lib.git"],
    );
    add(user, &app);
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("# my project, by hand\n{text}")).unwrap();
}

fn settings_repository<'a>(settings: &'a Value, path: &str) -> &'a Value {
    settings["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"] == path)
        .unwrap()
}

#[test]
fn the_settings_propose_each_forge_from_its_remote_and_nothing_is_read_before_saving() {
    let user = User::new().with_fake_gh();
    settings_fixture(&user);

    let settings = user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();

    let api = settings_repository(&settings, "api");
    assert_eq!(api["url"], "git@github.com:acme/api.git");
    assert_eq!(
        api["proposal"],
        json!({"kind": "github", "repository": "acme/api"})
    );
    assert_eq!(api["forge"], Value::Null, "proposed, not declared");
    let lib = settings_repository(&settings, "lib");
    assert_eq!(lib["proposal"], Value::Null);
    assert_eq!(settings["tracker"], Value::Null);
    let shown = user.call("project.show", json!({"name": "app"})).unwrap();
    assert_eq!(shown["repositories"][0].get("forge"), None, "{shown}");
    // Nothing is asked of a forge for a proposal.
    assert!(user.gh_received().is_empty(), "{:?}", user.gh_received());
}

#[test]
fn saving_the_settings_writes_the_file_its_comments_kept() {
    let user = User::new();
    settings_fixture(&user);

    let saved = user
        .call(
            "project.set_settings",
            json!({"name": "app",
                "repositories": [{"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}],
                "tracker": {"kind": "jira", "site": "Example.atlassian.net", "projects": ["abc"]}}),
        )
        .unwrap();

    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(text.starts_with("# my project, by hand\n"), "{text}");
    assert!(
        text.contains("forge = { kind = \"github\", repository = \"acme/api\" }"),
        "{text}"
    );
    assert!(text.contains("[tracker]\nkind = \"jira\""), "{text}");
    assert_eq!(
        settings_repository(&saved, "api")["forge"],
        json!({"kind": "github", "repository": "acme/api"})
    );
    assert_eq!(
        saved["tracker"],
        json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]})
    );
    let shown = user.call("project.show", json!({"name": "app"})).unwrap();
    assert_eq!(shown["tracker"]["site"], "example.atlassian.net");
    let text = String::from_utf8_lossy(&user.run(&["project", "show", "app"]).stdout).into_owned();
    assert!(
        text.contains("  api: remote origin, base main, on GitHub acme/api\n"),
        "{text}"
    );
    assert!(
        text.contains("  lib: remote origin, base main, no forge set\n"),
        "{text}"
    );
    assert!(
        text.contains("Tickets: Jira on example.atlassian.net (ABC)\n"),
        "{text}"
    );

    // None declared again: the key is gone from the file, the comment still there.
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [{"path": "api", "forge": null}], "tracker": null}),
    )
    .unwrap();
    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!text.contains("forge ="), "{text}");
    assert!(!text.contains("[tracker]"), "{text}");
    assert!(text.starts_with("# my project, by hand\n"), "{text}");
}

#[test]
fn an_empty_branch_prefix_saved_by_the_settings_is_kept_in_the_file() {
    let user = User::new();
    settings_fixture(&user);

    let saved = user
        .call(
            "project.set_settings",
            json!({"name": "app", "branch_prefix": ""}),
        )
        .unwrap();

    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(text.starts_with("# my project, by hand\n"), "{text}");
    assert!(text.contains("branch_prefix = \"\"\n"), "{text}");
    assert_eq!(saved["branch_prefix"], "");
    let shown = user.call("project.show", json!({"name": "app"})).unwrap();
    assert_eq!(shown["branch_prefix"], "");
}

#[test]
fn a_save_without_branch_prefix_leaves_the_slug_and_the_file_alone() {
    let user = User::new();
    settings_fixture(&user);

    let settings = user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();
    assert_eq!(settings["branch_prefix"], "app");
    let before = std::fs::read_to_string(user.project_file("app")).unwrap();
    user.call(
        "project.set_settings",
        json!({"name": "app", "tracker": null}),
    )
    .unwrap();

    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with("branch_prefix"))
            .count(),
        before
            .lines()
            .filter(|l| l.starts_with("branch_prefix"))
            .count()
    );
    assert!(!text.contains("branch_prefix = \"\""), "{text}");
}

#[test]
fn a_branch_prefix_with_a_space_is_refused_and_nothing_is_saved() {
    let user = User::new();
    settings_fixture(&user);
    let before = std::fs::read_to_string(user.project_file("app")).unwrap();

    let (code, message) = user
        .call(
            "project.set_settings",
            json!({"name": "app", "branch_prefix": "my app"}),
        )
        .unwrap_err();

    assert_eq!(code, "invalid-config");
    assert!(
        message.contains("branch_prefix: must not hold spaces"),
        "{message}"
    );
    assert_eq!(
        std::fs::read_to_string(user.project_file("app")).unwrap(),
        before
    );
}

#[test]
fn a_forge_its_remote_does_not_lead_to_is_never_saved() {
    let user = User::new();
    settings_fixture(&user);
    let before = std::fs::read_to_string(user.project_file("app")).unwrap();

    for (path, forge, why) in [
        (
            "api",
            json!({"kind": "github", "repository": "acme/web"}),
            "api cannot be declared on GitHub acme/web: its remote origin: it leads to \
             github.com/acme/api, not to GitHub acme/web; nothing was saved",
        ),
        (
            "lib",
            json!({"kind": "bitbucket-cloud", "repository": "acme/lib"}),
            "lib cannot be declared on Bitbucket Cloud acme/lib: its remote origin: it leads to \
             /srv/git/lib.git, which is not on a forge; nothing was saved",
        ),
    ] {
        let (code, message) = user
            .call(
                "project.set_settings",
                json!({"name": "app", "repositories": [{"path": path, "forge": forge}]}),
            )
            .unwrap_err();
        assert_eq!(code, "refused");
        assert_eq!(message, why);
    }
    assert_eq!(
        std::fs::read_to_string(user.project_file("app")).unwrap(),
        before
    );
}

#[test]
fn a_saved_forge_is_written_as_checked_not_as_typed() {
    let user = User::new();
    settings_fixture(&user);
    user.git(
        &user.path("work/app/lib"),
        &[
            "remote",
            "set-url",
            "origin",
            "ssh://git@bitbucket.example.com:7999/PROJ/lib.git",
        ],
    );

    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [{"path": "lib", "forge": {
            "kind": "bitbucket-data-center", "host": " Bitbucket.Example.COM ", "repository": "PROJ/lib"
        }}]}),
    )
    .unwrap();

    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(
        text.contains(
            "forge = { kind = \"bitbucket-data-center\", host = \"bitbucket.example.com\", \
             repository = \"PROJ/lib\" }"
        ),
        "{text}"
    );
}

#[test]
fn a_forge_is_checked_against_the_real_remote_and_shown_against_the_masked_one() {
    let user = User::new().with_env("APP_SECRET", "apisecret1");
    settings_fixture(&user);
    user.git(
        &user.path("work/app/api"),
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:acme/apisecret1.git",
        ],
    );

    let settings = user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();
    let saved = user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [
            {"path": "api", "forge": {"kind": "github", "repository": "acme/apisecret1"}}
        ]}),
    );

    assert_eq!(
        settings_repository(&settings, "api")["url"],
        "git@github.com:acme/[masked].git"
    );
    let saved = saved.unwrap();
    assert_eq!(
        settings_repository(&saved, "api")["disagrees"],
        Value::Null,
        "{saved}"
    );
}

#[test]
fn the_daemon_s_start_brings_a_former_file_to_the_new_keys_and_keeps_its_comments() {
    let user = User::new();
    // A copy of the fixture, over a folder of this test's own.
    let folder = user.path("work/app");
    std::fs::create_dir_all(&folder).unwrap();
    let former =
        include_str!("../../flottille-core/tests/fixtures/multi-repo-case-former-keys.toml")
            .replace(
                "folder = \"/work/app\"",
                &format!("folder = {:?}", folder.display().to_string()),
            );
    user.write_project_file("app", &former);

    user.status();

    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    for kept in [
        "# What we do before every review.\n[commands.open-prs]\n",
        "[skills.style] # the team's own words\n",
        "# Libraries before their consumers.\n[[setup]]\n",
        "[tasks.generate-data]\n",
        "task = \"generate-data\"",
    ] {
        assert!(text.contains(kept), "{kept}\n---\n{text}");
    }
    for former in ["[procedures", "[references", "[[recipe]]", "agent_actions"] {
        assert!(!text.contains(former), "{former}\n---\n{text}");
    }
    // No temporary file is left beside it.
    let files: Vec<_> = std::fs::read_dir(user.project_file("app").parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files, ["app.toml"]);
}

/// A file of this test's own project `app`, with a former key, over a folder that exists.
fn former_file(user: &User, extra: &str) -> String {
    let folder = user.path("work/app");
    std::fs::create_dir_all(&folder).unwrap();
    format!(
        "folder = {:?}\n{extra}\n# Our style.\n[references.style] # theirs\ndescription = \"The style\"\n\
         path = \"docs/style.md\"\n\n[[repositories]]\npath = \".\"\n",
        folder.display().to_string()
    )
}

#[test]
fn a_file_invalid_for_an_unrelated_reason_is_left_byte_for_byte() {
    let user = User::new();
    let text = former_file(&user, "max_children = 13\n");
    user.write_project_file("app", &text);

    user.status();

    assert_eq!(
        std::fs::read_to_string(user.project_file("app")).unwrap(),
        text
    );
}

#[test]
fn a_task_under_both_keys_is_left_with_both_and_nothing_is_lost() {
    let user = User::new();
    let text = former_file(
        &user,
        "\n# The build we had.\n[commands.build]\nline = \"make\"\n\n\
         # The build we have.\n[tasks.build]\nline = \"make all\"\n",
    );
    user.write_project_file("app", &text);

    user.status();

    assert_eq!(
        std::fs::read_to_string(user.project_file("app")).unwrap(),
        text
    );
}

#[test]
fn the_same_file_without_its_fault_is_migrated() {
    let user = User::new();
    let text = former_file(&user, "");
    user.write_project_file("app", &text);

    user.status();

    let written = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(
        written.contains("# Our style.\n[skills.style] # theirs\n"),
        "{written}"
    );
}

#[test]
fn a_project_pinning_the_old_default_follows_the_new_default() {
    let user = User::new();
    let folder = user.path("work/app");
    std::fs::create_dir_all(&folder).unwrap();
    // What `project add` wrote until the default could be chosen: the data folder, pinned.
    let pinned = user.path("data/flottille/workspaces/app");
    let text = format!(
        "# Mine.\nfolder = {:?}\nworkspaces_root = {:?}\n\n[[repositories]]\npath = \".\"\n",
        folder.display().to_string(),
        pinned.display().to_string()
    );
    user.write_project_file("app", &text);

    user.status();

    let written = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!written.contains("workspaces_root"), "{written}");
    assert!(written.contains("# Mine."), "{written}");
    user.call("workspaces_folder.set", json!({"place": "beside"}))
        .unwrap();
    let project = user.call("project.show", json!({"name": "app"})).unwrap();
    assert_eq!(
        project["workspaces_root"],
        user.path("work/app-work").to_str().unwrap()
    );
    assert!(project.get("workspaces_root_own").is_none(), "{project}");
}

#[test]
fn a_project_pinning_another_folder_keeps_it() {
    let user = User::new();
    let folder = user.path("work/app");
    std::fs::create_dir_all(&folder).unwrap();
    let own = user.path("elsewhere/app");
    let text = format!(
        "folder = {:?}\nworkspaces_root = {:?}\n\n[[repositories]]\npath = \".\"\n",
        folder.display().to_string(),
        own.display().to_string()
    );
    user.write_project_file("app", &text);

    user.status();

    assert_eq!(
        std::fs::read_to_string(user.project_file("app")).unwrap(),
        text
    );
}

/// A project `app` of three repositories: `api` with its `origin` on GitHub and a `develop`
/// branch, `lib` with no remote, and `web`, found in the folder but left out of the file, which
/// holds comments of its user.
fn repositories_fixture(user: &User) {
    let app = user.path("work/app");
    for name in ["api", "lib"] {
        let repository = app.join(name);
        user.git_init(&repository);
        std::fs::write(repository.join("README.md"), format!("{name}\n")).unwrap();
        user.git(&repository, &["add", "."]);
        user.git(&repository, &["commit", "--quiet", "-m", "start"]);
    }
    user.git(&app.join("api"), &["branch", "develop"]);
    user.git(
        &app.join("api"),
        &["remote", "add", "origin", "git@github.com:acme/api.git"],
    );
    let file = user.project_file("app");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        format!(
            "# my project, by hand\nfolder = \"{}\"\n\n# the API\n[[repositories]]\npath = \
             \"api\"\nbase = [\"develop\", \"main\"]\nforge = {{ kind = \"github\", repository = \
             \"acme/api\" }}\n\n# the library\n[[repositories]]\npath = \"lib\"\n",
            app.display()
        ),
    )
    .unwrap();
    user.git_init(&app.join("web"));
}

#[test]
fn the_repositories_list_with_add_remove_and_detect_writes_the_file() {
    let user = User::new();
    repositories_fixture(&user);

    let listed = user
        .call("project.repositories", json!({"name": "app"}))
        .unwrap();

    let api = settings_repository(&listed, "api");
    assert_eq!(api["found"], true);
    assert_eq!(api["branch"], "main");
    assert_eq!(api["remote"], "origin");
    assert_eq!(api["url"], "git@github.com:acme/api.git");
    assert_eq!(
        api["forge"],
        json!({"kind": "github", "repository": "acme/api"})
    );
    assert_eq!(api["base"], json!(["develop", "main"]));
    assert_eq!(api["branches"], json!(["develop", "main"]));
    let lib = settings_repository(&listed, "lib");
    assert_eq!(lib["url"], Value::Null, "local only: {lib}");
    assert_eq!(lib["forge"], Value::Null);
    // Detected again: what the folder holds that the file does not name.
    assert_eq!(listed["new"], json!(["web"]));

    let added = user
        .call(
            "project.add_repository",
            json!({"name": "app", "path": "web"}),
        )
        .unwrap();
    assert_eq!(added["new"], json!([]));
    assert!(settings_repository(&added, "web")["found"] == true);
    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(text.starts_with("# my project, by hand\n"), "{text}");
    assert!(text.contains("# the library\n"), "{text}");
    assert!(
        text.contains("[[repositories]]\npath = \"web\"\n"),
        "{text}"
    );

    let removed = user
        .call(
            "project.remove_repository",
            json!({"name": "app", "path": "lib"}),
        )
        .unwrap();
    let paths: Vec<_> = removed["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["api", "web"]);
    // Forgotten, never touched: the folder is still there, and found again.
    assert!(user.path("work/app/lib/.git").is_dir());
    assert_eq!(removed["new"], json!(["lib"]));
    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!text.contains("path = \"lib\""), "{text}");
    assert!(text.contains("# the API\n"), "{text}");
    let shown = user.call("project.show", json!({"name": "app"})).unwrap();
    assert_eq!(repository_paths(&shown), ["api", "web"]);
}

#[test]
fn a_link_leading_outside_the_project_folder_is_not_added_as_a_repository() {
    let user = User::new();
    repositories_fixture(&user);
    let elsewhere = user.path("elsewhere/secret");
    std::fs::create_dir_all(&elsewhere).unwrap();
    user.git_init(&elsewhere);
    std::os::unix::fs::symlink(&elsewhere, user.path("work/app/outside")).unwrap();
    // A link that stays inside is still a folder of the project.
    std::os::unix::fs::symlink(user.path("work/app/web"), user.path("work/app/inside")).unwrap();
    let before = std::fs::read_to_string(user.project_file("app")).unwrap();

    let (code, message) = user
        .call(
            "project.add_repository",
            json!({"name": "app", "path": "outside"}),
        )
        .unwrap_err();

    assert_eq!(code, "refused");
    assert_eq!(message, "outside leads outside the project folder");
    assert_eq!(
        std::fs::read_to_string(user.project_file("app")).unwrap(),
        before
    );
    user.call(
        "project.add_repository",
        json!({"name": "app", "path": "inside"}),
    )
    .unwrap();
}

#[test]
fn a_repository_that_cannot_be_added_or_removed_leaves_the_file_as_it_was() {
    let user = User::new();
    repositories_fixture(&user);
    std::fs::create_dir_all(user.path("work/app/notes")).unwrap();
    let before = std::fs::read_to_string(user.project_file("app")).unwrap();

    let refusals = [
        (
            "project.add_repository",
            "notes",
            "notes is not a Git repository",
        ),
        (
            "project.add_repository",
            "api",
            "api is already a repository of app",
        ),
        (
            "project.add_repository",
            "../elsewhere",
            "../elsewhere is not a folder of app",
        ),
        (
            "project.remove_repository",
            "web",
            "app has no repository web",
        ),
    ]
    .map(|(method, path, said)| {
        let (code, message) = user
            .call(method, json!({"name": "app", "path": path}))
            .unwrap_err();
        (code, message, said)
    });
    let unchanged = std::fs::read_to_string(user.project_file("app")).unwrap();
    user.call(
        "project.remove_repository",
        json!({"name": "app", "path": "lib"}),
    )
    .unwrap();
    let (code, last) = user
        .call(
            "project.remove_repository",
            json!({"name": "app", "path": "api"}),
        )
        .unwrap_err();

    for (code, message, said) in refusals {
        assert_eq!(code, "refused", "{message}");
        assert!(message.contains(said), "{message}");
    }
    assert_eq!(code, "refused");
    assert!(last.contains("at least one repository"), "{last}");
    assert_eq!(unchanged, before);
    let after = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(after.contains("path = \"api\""), "{after}");
}

#[test]
fn the_project_setting_delete_the_branch_on_merge_is_saved_in_the_file_and_shown() {
    let user = User::new();
    let app = user.path("work/app");
    user.git_init(&app);
    user.git(
        &app,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    add(&user, &app);
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [{"path": ".",
            "forge": {"kind": "github", "repository": "acme/app"}}]}),
    )
    .unwrap();
    let shown = |user: &User| {
        let settings = user
            .call("project.settings", json!({"name": "app"}))
            .unwrap();
        settings["repositories"][0]["delete_branch_on_merge"].clone()
    };
    assert_eq!(shown(&user), json!(false));

    // What is left out is kept: the forge too.
    let saved = user
        .call(
            "project.set_settings",
            json!({"name": "app", "repositories": [{"path": ".",
                "forge": {"kind": "github", "repository": "acme/app"},
                "delete_branch_on_merge": true}]}),
        )
        .unwrap();
    assert_eq!(saved["repositories"][0]["delete_branch_on_merge"], true);
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(file.contains("delete_branch_on_merge = true"), "{file}");
    assert_eq!(shown(&user), json!(true));

    let kept = user
        .call(
            "project.set_settings",
            json!({"name": "app", "repositories": [{"path": ".",
                "forge": {"kind": "github", "repository": "acme/app"}}]}),
        )
        .unwrap();
    assert_eq!(kept["repositories"][0]["delete_branch_on_merge"], true);

    // Off again: the key leaves the file.
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [{"path": ".",
            "forge": {"kind": "github", "repository": "acme/app"},
            "delete_branch_on_merge": false}]}),
    )
    .unwrap();
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!file.contains("delete_branch_on_merge"), "{file}");
}

// The project's conventions (#466): its branch format and its pull request template.

#[test]
fn an_invalid_pattern_is_refused_when_saved() {
    let user = User::new();
    settings_fixture(&user);
    let before = std::fs::read_to_string(user.project_file("app")).unwrap();

    let (code, message) = user
        .call(
            "project.set_conventions",
            json!({"name": "app", "branch_format": "^(feat|fix/[A-Z]+$"}),
        )
        .unwrap_err();

    assert_eq!(code, "refused");
    assert!(message.contains("^(feat|fix/[A-Z]+$"), "{message}");
    assert!(message.contains("unclosed group"), "{message}");
    assert!(message.contains("nothing was saved"), "{message}");
    assert_eq!(
        std::fs::read_to_string(user.project_file("app")).unwrap(),
        before
    );
}

#[test]
fn the_conventions_are_saved_in_the_file_and_emptied_away() {
    let user = User::new();
    settings_fixture(&user);

    let saved = user
        .call(
            "project.set_conventions",
            json!({"name": "app", "branch_format": "^feat/.+$",
                "pull_request_template": "## Why\n\n## How to verify\n"}),
        )
        .unwrap();
    assert_eq!(saved["branch_format"], "^feat/.+$");
    assert_eq!(
        saved["pull_request_template"],
        "## Why\n\n## How to verify\n"
    );
    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(text.starts_with("# my project, by hand\n"), "{text}");
    let settings = user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();
    assert_eq!(settings["branch_format"], "^feat/.+$");
    assert_eq!(
        settings["pull_request_template"],
        "## Why\n\n## How to verify\n"
    );

    // Left out, a convention is kept; empty, it is removed.
    let saved = user
        .call(
            "project.set_conventions",
            json!({"name": "app", "branch_format": ""}),
        )
        .unwrap();
    assert_eq!(saved["branch_format"], Value::Null);
    assert_eq!(
        saved["pull_request_template"],
        "## Why\n\n## How to verify\n"
    );
    let text = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(!text.contains("branch_format"), "{text}");
}

// Removing a project with what Flottille made for it: its agents, its runs, its workspaces.

mod removal {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::common::User;
    use super::common::mcp::Mcp;
    use serde_json::{Value, json};

    /// A user whose agents are the fake agent: a message holding `HANG` keeps its agent mid-turn,
    /// the others answer and end their turn.
    fn user() -> User {
        let user = User::with_fake_agent();
        user.fake_script(json!({"turns": [{"when": "HANG", "do": [{"hang": true}]}]}));
        user
    }

    fn eventually(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !check() {
            assert!(Instant::now() < deadline, "{what} never happened");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The project `name` over a throwaway repository pushed to its own bare remote, with a
    /// task `long` that runs until it is stopped: its folder.
    fn project(user: &User, name: &str) -> PathBuf {
        let folder = user.path(&format!("work/{name}"));
        user.git_init(&folder);
        std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
        user.git(&folder, &["add", "README.md"]);
        user.git(&folder, &["commit", "--quiet", "-m", "start"]);
        let remote = user.path(&format!("remotes/{name}.git"));
        user.git(
            &user.path(""),
            &["init", "--quiet", "--bare", remote.to_str().unwrap()],
        );
        user.git(
            &folder,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        user.git(&folder, &["push", "--quiet", "origin", "main"]);
        user.call("project.add", json!({"folder": folder, "name": name}))
            .unwrap();
        let tools = user.path("tools");
        std::fs::create_dir_all(&tools).unwrap();
        let long = tools.join("long.sh");
        std::fs::write(&long, "#!/bin/sh\necho started\nexec sleep 60\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&long, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = user.project_file(name);
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str(&format!("\n[tasks.long]\nline = \"{}\"\n", long.display()));
        std::fs::write(&file, text).unwrap();
        folder
    }

    /// The workspace `name` of the project, ready: its folder and its branch.
    fn workspace(user: &User, project: &str, name: &str) -> (PathBuf, String) {
        user.call(
            "workspace.create",
            json!({"project": project, "name": name}),
        )
        .unwrap();
        let mut shown = Value::Null;
        eventually("the workspace", || {
            shown = user
                .call("workspace.show", json!({"project": project, "name": name}))
                .unwrap();
            shown["state"] == "ready"
        });
        (
            PathBuf::from(shown["folder"].as_str().unwrap()),
            shown["branch"].as_str().unwrap().to_owned(),
        )
    }

    fn workspace_exists(user: &User, project: &str, name: &str) -> bool {
        user.call("workspace.show", json!({"project": project, "name": name}))
            .is_ok()
    }

    /// A root the user starts in the workspace, mid-turn: its id.
    fn hanging_root(user: &User, project: &str, workspace: &str) -> String {
        let answer = user
            .call(
                "agent.run",
                json!({"project": project, "workspace": workspace, "message": "HANG here"}),
            )
            .unwrap();
        let id = answer["agent"]["id"].as_str().unwrap().to_owned();
        eventually("the root works", || state(user, &id) == "working");
        id
    }

    fn state(user: &User, agent: &str) -> String {
        user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The MCP session of the last agent opened whose instructions hold `part`.
    fn session(user: &User, part: &str) -> Mcp {
        let mut found = None;
        eventually("the session", || {
            found = user
                .received_requests("session/new")
                .into_iter()
                .rev()
                .find(|opened| {
                    opened["params"]["_meta"]["claudeCode"]["options"]["systemPrompt"]["prompt"][2]
                        .as_str()
                        .unwrap_or_default()
                        .contains(part)
                });
            found.is_some()
        });
        let server = &found.unwrap()["params"]["mcpServers"][0];
        Mcp::initialize(
            server["url"].as_str().unwrap(),
            server["headers"][0]["value"].as_str().unwrap(),
        )
    }

    fn ids(list: &Value) -> Vec<String> {
        let mut ids: Vec<String> = list
            .as_array()
            .unwrap_or_else(|| panic!("not a list: {list}"))
            .iter()
            .map(|entry| {
                entry["id"]
                    .as_str()
                    .or(entry["run"].as_str())
                    .or(entry["name"].as_str())
                    .unwrap()
                    .to_owned()
            })
            .collect();
        ids.sort();
        ids
    }

    /// Removes `app`; with `discard`, the workspaces the preview showed losing work too, sent back
    /// as it showed them, as a client does.
    fn remove(user: &User, discard: bool) -> Value {
        let mut asked = json!({"name": "app"});
        if discard {
            asked["discard"] = shown(user);
        }
        user.call("project.remove", asked).unwrap()
    }

    /// The workspaces `project.removal` shows, with what each would lose.
    fn shown(user: &User) -> Value {
        let said = user
            .call("project.removal", json!({"name": "app"}))
            .unwrap();
        said["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| json!({"name": w["name"], "losses": w["losses"]}))
            .collect()
    }

    fn branches(user: &User, folder: &Path) -> String {
        user.git_output(folder, &["branch", "--list", "--format=%(refname:short)"])
    }

    #[test]
    fn removing_a_project_stops_its_running_agents_and_their_children() {
        let user = user();
        project(&user, "app");
        workspace(&user, "app", "w");
        let root = hanging_root(&user, "app", "w");
        let lead = session(&user, "You are the root");
        let launched = lead.call(
            "agent_launch",
            json!({"kind": "child", "title": "worker", "brief": "HANG on", "workspace": "w"}),
        );
        assert!(!launched.error, "{}", launched.text);
        let child = user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["parent"] == root.as_str())
            .map(|agent| agent["id"].as_str().unwrap().to_owned())
            .expect("the child");

        let removed = remove(&user, false);

        let mut both = vec![root.clone(), child.clone()];
        both.sort();
        assert_eq!(ids(&removed["stopped"]["agents"]), both, "{removed}");
        assert_eq!(state(&user, &root), "ended");
        assert_eq!(state(&user, &child), "ended");
        assert!(
            !workspace_exists(&user, "app", "w"),
            "its place went with it"
        );
        assert!(!user.project_file("app").exists());
    }

    #[test]
    fn removing_a_project_stops_its_running_command_runs() {
        let user = user();
        project(&user, "app");
        let run = user
            .call("task.run", json!({"project": "app", "name": "long"}))
            .unwrap()["run"]
            .as_str()
            .unwrap()
            .to_owned();
        eventually("the run started", || {
            user.call("run.output", json!({"run": run})).unwrap()["output"] == "started"
        });

        let removed = remove(&user, false);

        assert_eq!(
            ids(&removed["stopped"]["runs"]),
            std::slice::from_ref(&run),
            "{removed}"
        );
        let shown = user.call("run.output", json!({"run": run})).unwrap();
        assert_eq!(shown["state"], "stopped", "{shown}");
    }

    #[test]
    fn removing_a_project_removes_its_clean_workspaces_and_their_branches() {
        let user = user();
        let folder = project(&user, "app");
        let (w, branch) = workspace(&user, "app", "w");
        assert!(branches(&user, &folder).contains(&branch));

        let removed = remove(&user, false);

        assert_eq!(ids(&removed["workspaces"]["removed"]), ["w"], "{removed}");
        assert_eq!(removed["workspaces"]["kept"], json!([]));
        assert!(!w.exists(), "its worktree is gone");
        assert!(!workspace_exists(&user, "app", "w"));
        assert!(
            !branches(&user, &folder).contains(&branch),
            "its branch is gone: {}",
            branches(&user, &folder)
        );
        assert!(!user.project_file("app").exists());
    }

    #[test]
    fn removing_a_project_keeps_a_workspace_with_uncommitted_changes_and_says_so() {
        let user = user();
        project(&user, "app");
        let (w, _) = workspace(&user, "app", "w");
        std::fs::write(w.join("draft.txt"), "not committed\n").unwrap();

        let removed = remove(&user, false);

        assert_eq!(removed["workspaces"]["removed"], json!([]), "{removed}");
        let kept = &removed["workspaces"]["kept"][0];
        assert_eq!(kept["name"], "w", "{removed}");
        assert!(kept["why"].to_string().contains("draft.txt"), "{kept}");
        assert!(kept["why"].to_string().contains("not committed"), "{kept}");
        assert_eq!(
            std::fs::read_to_string(w.join("draft.txt")).unwrap(),
            "not committed\n"
        );
        assert!(
            !user.project_file("app").exists(),
            "the project is forgotten"
        );
        // Its work saved, the workspace kept is removed as any other.
        std::fs::remove_file(w.join("draft.txt")).unwrap();
        user.call("workspace.remove", json!({"project": "app", "name": "w"}))
            .unwrap();
        assert!(!w.exists());
    }

    #[test]
    fn removing_a_project_keeps_a_workspace_with_unpushed_commits_and_says_so() {
        let user = user();
        let folder = project(&user, "app");
        let (w, branch) = workspace(&user, "app", "w");
        std::fs::write(w.join("work.txt"), "done\n").unwrap();
        user.git(&w, &["add", "work.txt"]);
        user.git(&w, &["commit", "--quiet", "-m", "the work"]);

        let removed = remove(&user, false);

        let kept = &removed["workspaces"]["kept"][0];
        assert_eq!(kept["name"], "w", "{removed}");
        assert!(kept["why"].to_string().contains("the work"), "{kept}");
        assert!(kept["why"].to_string().contains("no remote"), "{kept}");
        assert!(w.join("work.txt").exists());
        assert!(branches(&user, &folder).contains(&branch));
    }

    #[test]
    fn removing_a_project_with_discard_removes_unsaved_workspaces_too() {
        let user = user();
        let folder = project(&user, "app");
        let (dirty, dirty_branch) = workspace(&user, "app", "dirty");
        std::fs::write(dirty.join("draft.txt"), "not committed\n").unwrap();
        let (ahead, ahead_branch) = workspace(&user, "app", "ahead");
        std::fs::write(ahead.join("work.txt"), "done\n").unwrap();
        user.git(&ahead, &["add", "work.txt"]);
        user.git(&ahead, &["commit", "--quiet", "-m", "the work"]);

        let removed = remove(&user, true);

        assert_eq!(
            ids(&removed["workspaces"]["removed"]),
            ["ahead", "dirty"],
            "{removed}"
        );
        assert_eq!(removed["workspaces"]["kept"], json!([]));
        assert!(!dirty.exists());
        assert!(!ahead.exists());
        // As `discard` does for one workspace: a branch is deleted once its work has landed, so
        // the commits on no remote stay on theirs, and the result says why.
        let left = branches(&user, &folder);
        assert!(
            !left.contains(&dirty_branch),
            "the branch with no commit of its own is gone: {left}"
        );
        assert!(left.contains(&ahead_branch), "{left}");
        let fate = |name: &str| {
            removed["workspaces"]["removed"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["name"] == name)
                .unwrap()["branches"][0]
                .clone()
        };
        assert_eq!(fate("dirty")["deleted"], true, "{removed}");
        assert_eq!(fate("ahead")["deleted"], false, "{removed}");
    }

    #[test]
    fn discard_keeps_a_workspace_whose_losses_appeared_after_the_preview() {
        let user = user();
        project(&user, "app");
        let (clean, _) = workspace(&user, "app", "clean");
        let (dirty, _) = workspace(&user, "app", "dirty");
        std::fs::write(dirty.join("draft.txt"), "not committed\n").unwrap();
        let (grown, _) = workspace(&user, "app", "grown");
        std::fs::write(grown.join("draft.txt"), "not committed\n").unwrap();
        let shown = shown(&user);
        // Written after the preview, as an agent still working would: the user never saw it.
        std::fs::write(clean.join("late.txt"), "the agent's work\n").unwrap();
        std::fs::write(grown.join("late.txt"), "the agent's work\n").unwrap();

        let removed = user
            .call("project.remove", json!({"name": "app", "discard": shown}))
            .unwrap();

        assert_eq!(
            ids(&removed["workspaces"]["removed"]),
            ["dirty"],
            "{removed}"
        );
        assert_eq!(
            ids(&removed["workspaces"]["kept"]),
            ["clean", "grown"],
            "{removed}"
        );
        for kept in removed["workspaces"]["kept"].as_array().unwrap() {
            assert_eq!(kept["changed"], true, "{kept}");
            assert!(kept["why"].to_string().contains("late.txt"), "{kept}");
        }
        assert!(!dirty.exists());
        assert_eq!(
            std::fs::read_to_string(clean.join("late.txt")).unwrap(),
            "the agent's work\n"
        );
        assert!(grown.join("late.txt").exists());
        assert!(grown.join("draft.txt").exists());
    }

    /// What a checkout is, but git's own folder: its files, their contents and its branch.
    fn checkout(user: &User, folder: &Path) -> Vec<String> {
        let mut seen: Vec<String> = super::snapshot(folder)
            .into_iter()
            .filter(|line| !line.starts_with(".git"))
            .collect();
        seen.push(user.git_output(folder, &["status", "--porcelain=v1", "--branch"]));
        seen.push(user.git_output(folder, &["rev-parse", "HEAD"]));
        seen
    }

    #[test]
    fn removing_a_project_never_touches_its_folder_or_its_repositories_checkouts() {
        let user = user();
        let folder = project(&user, "app");
        std::fs::write(folder.join("notes.txt"), "mine\n").unwrap();
        let (w, _) = workspace(&user, "app", "w");
        std::fs::write(w.join("draft.txt"), "kept\n").unwrap();
        workspace(&user, "app", "clean");
        let before = checkout(&user, &folder);

        remove(&user, false);

        assert_eq!(checkout(&user, &folder), before);
        assert_eq!(
            std::fs::read_to_string(folder.join("notes.txt")).unwrap(),
            "mine\n"
        );
    }

    #[test]
    fn removing_a_project_leaves_other_projects_agents_and_workspaces_alone() {
        let user = user();
        project(&user, "app");
        project(&user, "other");
        workspace(&user, "app", "w");
        let (theirs, _) = workspace(&user, "other", "w");
        let ours = hanging_root(&user, "app", "w");
        let other = hanging_root(&user, "other", "w");
        let run = user
            .call("task.run", json!({"project": "other", "name": "long"}))
            .unwrap()["run"]
            .as_str()
            .unwrap()
            .to_owned();

        let removed = remove(&user, false);

        assert_eq!(
            ids(&removed["stopped"]["agents"]),
            std::slice::from_ref(&ours)
        );
        assert_eq!(state(&user, &ours), "ended");
        assert_eq!(state(&user, &other), "working");
        assert!(workspace_exists(&user, "other", "w"));
        assert!(theirs.exists());
        let shown = user.call("run.output", json!({"run": run})).unwrap();
        assert_eq!(shown["state"], "running", "{shown}");
        user.call("project.show", json!({"name": "other"})).unwrap();
    }

    #[test]
    fn the_removal_says_beforehand_what_runs_and_each_workspace_s_state() {
        let user = user();
        project(&user, "app");
        workspace(&user, "app", "clean");
        let (dirty, _) = workspace(&user, "app", "dirty");
        std::fs::write(dirty.join("draft.txt"), "not committed\n").unwrap();
        let (ahead, _) = workspace(&user, "app", "ahead");
        std::fs::write(ahead.join("work.txt"), "done\n").unwrap();
        user.git(&ahead, &["add", "work.txt"]);
        user.git(&ahead, &["commit", "--quiet", "-m", "the work"]);
        let root = hanging_root(&user, "app", "clean");
        let run = user
            .call("task.run", json!({"project": "app", "name": "long"}))
            .unwrap()["run"]
            .as_str()
            .unwrap()
            .to_owned();

        let said = user
            .call("project.removal", json!({"name": "app"}))
            .unwrap();

        assert_eq!(
            ids(&said["sessions"]),
            std::slice::from_ref(&root),
            "{said}"
        );
        assert_eq!(ids(&said["runs"]), [run], "{said}");
        let states: Vec<(String, String)> = said["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["name"].as_str().unwrap().to_owned(),
                    w["state"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            states,
            [
                ("ahead".to_owned(), "unpushed".to_owned()),
                ("clean".to_owned(), "clean".to_owned()),
                ("dirty".to_owned(), "uncommitted".to_owned()),
            ],
            "{said}"
        );
        assert_eq!(state(&user, &root), "working", "asking stops nothing");
        assert!(user.project_file("app").exists());
    }

    /// The user with a `git` that runs the real one, but first `on_remove` (shell) for a
    /// `git worktree remove`.
    fn wrapping_removals(user: User, on_remove: &str) -> User {
        let real = std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let real = String::from_utf8(real.stdout).unwrap().trim().to_owned();
        let bin = user.path("fake-git");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("git");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ncase \"$*\" in *\"worktree remove\"*)\n{on_remove};;\nesac\nexec \
                 '{real}' \"$@\"\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        user.with_env("PATH", &path)
    }

    /// A user whose `git` holds `git worktree remove` until the test releases it: `held` exists
    /// once it waits, `release` lets it go on.
    fn holding_removals() -> (User, PathBuf, PathBuf) {
        let user = user();
        let (held, release) = (user.path("removal-held"), user.path("removal-released"));
        let on_remove = format!(
            "  touch '{}'\n  while [ ! -e '{}' ]; do sleep 0.05; done",
            held.display(),
            release.display()
        );
        (wrapping_removals(user, &on_remove), held, release)
    }

    #[test]
    fn nothing_starts_in_a_project_being_removed() {
        let (user, held, release) = holding_removals();
        project(&user, "app");
        workspace(&user, "app", "w");
        let (mut started, mut created, mut ran) = (None, None, None);
        std::thread::scope(|scope| {
            let removing = scope.spawn(|| user.call("project.remove", json!({"name": "app"})));
            eventually("the removal holds", || held.exists());
            started = Some(user.call("agent.run", json!({"project": "app", "message": "hello"})));
            created = Some(user.call(
                "workspace.create",
                json!({"project": "app", "name": "late"}),
            ));
            ran = Some(user.call("task.run", json!({"project": "app", "name": "long"})));
            std::fs::write(&release, "").unwrap();
            removing.join().unwrap().unwrap();
        });

        for (what, answer) in [("agent", started), ("workspace", created), ("run", ran)] {
            let answer = answer.unwrap();
            let (code, message) = answer.expect_err(what);
            assert_eq!(code, "refused", "{what}: {message}");
            assert!(message.contains("being removed"), "{what}: {message}");
        }
    }

    #[test]
    fn a_project_being_removed_refuses_edits() {
        let (user, held, release) = holding_removals();
        project(&user, "app");
        workspace(&user, "app", "w");
        let mut edited = None;
        std::thread::scope(|scope| {
            let removing = scope.spawn(|| user.call("project.remove", json!({"name": "app"})));
            eventually("the removal holds", || held.exists());
            edited = Some(user.call(
                "project.set_conventions",
                json!({"name": "app", "branch_format": "^late/"}),
            ));
            std::fs::write(&release, "").unwrap();
            removing.join().unwrap().unwrap();
        });

        let (code, message) = edited.unwrap().expect_err("an edit");
        assert_eq!(code, "refused", "{message}");
        assert!(message.contains("being removed"), "{message}");
    }

    #[test]
    fn a_failed_workspace_removal_keeps_the_project_and_a_retry_continues_where_it_stopped() {
        let user = user();
        let failing = user.path("removals-fail");
        let user = wrapping_removals(
            user,
            &format!(
                "  case \"$*\" in *broken*) [ -e '{}' ] && {{ echo refused >&2; exit 1; }};; esac",
                failing.display()
            ),
        );
        std::fs::write(&failing, "").unwrap();
        project(&user, "app");
        let (fine, _) = workspace(&user, "app", "fine");
        let (broken, _) = workspace(&user, "app", "broken");

        let (code, message) = user
            .call("project.remove", json!({"name": "app"}))
            .expect_err("a removal stopped partway");

        assert_eq!(code, "refused", "{message}");
        assert!(message.contains("still known"), "{message}");
        assert!(message.contains("removed the workspaces fine"), "{message}");
        assert!(!fine.exists());
        assert!(broken.exists());
        assert!(user.project_file("app").exists());

        std::fs::remove_file(&failing).unwrap();
        let removed = remove(&user, false);

        assert_eq!(
            ids(&removed["workspaces"]["removed"]),
            ["broken"],
            "{removed}"
        );
        assert!(!broken.exists());
        assert!(!user.project_file("app").exists());
    }

    #[test]
    fn a_retry_after_a_timeout_waits_for_a_process_still_alive() {
        let user = User::with_fake_agent();
        // Stuck while it opens its session, it never sees the order to stop: its process
        // outlives the wait.
        user.fake_script(json!({"stall": true}));
        project(&user, "app");
        let (w, _) = workspace(&user, "app", "w");
        let agent = user
            .call(
                "agent.run",
                json!({"project": "app", "workspace": "w", "message": "hello"}),
            )
            .unwrap()["agent"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        eventually("the agent opens", || {
            !user.received_requests("session/new").is_empty()
        });

        let (_, first) = user
            .call_within(
                "project.remove",
                json!({"name": "app"}),
                Duration::from_secs(60),
            )
            .expect_err("the wait times out");
        assert!(first.contains(&agent) && first.contains("still"), "{first}");
        assert!(w.exists());

        let started = Instant::now();
        let (code, again) = user
            .call_within(
                "project.remove",
                json!({"name": "app"}),
                Duration::from_secs(60),
            )
            .expect_err("the retry waits again");

        assert_eq!(code, "refused", "{again}");
        assert!(again.contains(&agent) && again.contains("still"), "{again}");
        assert!(
            started.elapsed() > Duration::from_secs(10),
            "it waited for the process again"
        );
        assert!(w.exists(), "nothing is removed under a running process");
        assert!(user.project_file("app").exists());
        let (code, refused) = user
            .call("workspace.remove", json!({"project": "app", "name": "w"}))
            .expect_err("its process works in it");
        assert_eq!(code, "refused", "{refused}");
        assert!(refused.contains(&agent), "{refused}");
    }
}
