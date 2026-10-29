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
    let text = former_file(&user, "max_children = 9\n");
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
