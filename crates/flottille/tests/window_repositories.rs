//! The scenarios of a project's repositories in the window (#140, #167): the project's settings
//! list them and add, remove and detect them again; the new-workspace form takes some of them, each from
//! the base chosen for it; a workspace's card and the session's rail say where each repository
//! stands. On GPUI's test platform in front of a real daemon, with throwaway repositories (one
//! with no remote) and temporary XDG folders. No window opens on a screen.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::project::{PlaceCard, ProjectPage};
use flottille_window::project_settings::ProjectSettingsPage;
use flottille_window::repositories::ProjectRepositories;
use flottille_window::session::SessionPage;
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A project `trio` of three repositories with a commit each: `api`, pushed to a bare remote
/// and with a `develop` branch there and here; `web`, pushed too; `lib`, with no remote at all.
/// Its file names the three; `docs`, a fourth repository of the folder, is left out of it.
fn trio(user: &User) -> PathBuf {
    let folder = user.path("work/trio");
    for name in ["api", "web", "lib", "docs"] {
        let dir = folder.join(name);
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), format!("{name}\n")).unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
        if name == "api" || name == "web" {
            let remote = user.path(&format!("remotes/{name}.git"));
            user.git(
                &user.path(""),
                &["init", "--quiet", "--bare", remote.to_str().unwrap()],
            );
            user.git(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
            user.git(&dir, &["push", "--quiet", "origin", "main"]);
        }
    }
    let api = folder.join("api");
    user.git(&api, &["branch", "develop"]);
    user.git(&api, &["push", "--quiet", "origin", "develop"]);
    user.write_project_file(
        "trio",
        &format!(
            "# the trio, by hand\nfolder = \"{}\"\n\n[[repositories]]\npath = \"api\"\nbase = \
             [\"develop\", \"main\"]\n\n[[repositories]]\npath = \"web\"\n\n# no remote\n\
             [[repositories]]\npath = \"lib\"\n",
            folder.display()
        ),
    );
    folder
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn window<'a>(
    user: &User,
    connect: Connect,
    width: f32,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(width), px(1600.)));
    (shell, cx)
}

/// Runs the window until `check` holds, as real frames would.
fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Lets every motion end, then draws.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// How many frames the window still asks for: none at rest.
fn pending_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

fn page_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ProjectPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).project_page().cloned())
        .expect("a project's page is open")
}

fn section_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ProjectRepositories> {
    page_of(shell, cx).read_with(cx, |page, _| page.repositories().clone())
}

fn open_project(shell: &Entity<Shell>, project: &str, cx: &mut VisualTestContext) {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project(project.into()), window, cx)
    });
    until(shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == project)
        })
    });
    settle(cx);
}

/// Opens the project's settings, tall enough for every section, and returns its repositories.
fn open_settings(
    shell: &Entity<Shell>,
    project: &str,
    cx: &mut VisualTestContext,
) -> Entity<ProjectRepositories> {
    cx.simulate_resize(size(px(1280.), px(2400.)));
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings(project.into()), window, cx)
    });
    let page = |cx: &mut VisualTestContext| -> Option<Entity<ProjectSettingsPage>> {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
    };
    until(shell, cx, "the settings are read", |cx| {
        page(cx).is_some_and(|page| page.read_with(cx, |page, cx| page.has_read(cx)))
    });
    settle(cx);
    let page = page(cx).unwrap();
    page.read_with(cx, |page, _| page.repositories().clone())
}

fn label_of(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn has(id: &str, cx: &mut VisualTestContext) -> bool {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).is_some_and(|found| found.visible()))
}

fn paths(section: &Entity<ProjectRepositories>, cx: &mut VisualTestContext) -> Vec<String> {
    section.read_with(cx, |section, _| {
        section
            .repositories()
            .map(|read| read.listed.iter().map(|r| r.path.clone()).collect())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn the_project_s_settings_list_its_repositories_and_add_remove_and_detect_them(
    cx: &mut TestAppContext,
) {
    let user = user();
    let folder = trio(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let section = open_settings(&shell, "trio", cx);
    until(&shell, cx, "the repositories are read", |cx| {
        section.read_with(cx, |s, _| s.repositories().is_some())
    });
    settle(cx);

    assert_eq!(paths(&section, cx), ["api", "web", "lib"]);
    // Each value has its label: the base branch, the remote in short, the forge.
    assert_eq!(
        label_of("repository-base-api", cx).as_deref(),
        Some("develop, then main")
    );
    let remote = label_of("repository-remote-api", cx).unwrap();
    assert!(remote.starts_with("origin · "), "{remote}");
    assert_eq!(
        label_of("repository-forge-api", cx).as_deref(),
        Some("None")
    );
    assert_eq!(
        label_of("repository-remote-lib", cx).as_deref(),
        Some("origin: none, local only")
    );
    assert!(
        has("found-repository-docs", cx),
        "found, not in the project"
    );
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");

    // Add the one found: it joins the list, written in the file.
    cx.update(|window, cx| window.click("add-repository-docs", cx));
    until(&shell, cx, "docs is added", |cx| {
        paths(&section, cx).contains(&"docs".to_owned())
    });
    settle(cx);
    assert!(has("repository-docs", cx));
    assert!(!has("found-repository-docs", cx), "it folded away");
    let text = std::fs::read_to_string(user.project_file("trio")).unwrap();
    assert!(text.starts_with("# the trio, by hand\n"), "{text}");
    assert!(text.contains("path = \"docs\""), "{text}");

    // Remove…, in the row's "⋯", asks first, and says the folder stays.
    cx.update(|window, cx| window.click("repository-more-lib", cx));
    settle(cx);
    cx.update(|window, cx| window.click("remove-repository-lib", cx));
    settle(cx);
    let note = label_of("repository-removal-sentence", cx);
    assert!(
        note.as_deref()
            .is_some_and(|note| note.contains("repository itself is not touched")),
        "{note:?}"
    );
    assert!(paths(&section, cx).contains(&"lib".to_owned()));
    cx.update(|window, cx| window.click("repository-removal-action", cx));
    until(&shell, cx, "lib is forgotten", |cx| {
        !paths(&section, cx).contains(&"lib".to_owned())
    });
    settle(cx);
    assert!(folder.join("lib/.git").is_dir(), "its folder is kept");
    assert!(has("found-repository-lib", cx), "found again in the folder");
    let text = std::fs::read_to_string(user.project_file("trio")).unwrap();
    assert!(!text.contains("path = \"lib\""), "{text}");

    // Detect again: a repository made since shows up.
    user.git_init(&folder.join("tools"));
    cx.update(|window, cx| window.click("detect-repositories", cx));
    until(&shell, cx, "tools is found", |cx| {
        section.read_with(cx, |s, _| {
            s.repositories()
                .is_some_and(|r| r.new.contains(&"tools".to_owned()))
        })
    });
    settle(cx);
    assert!(has("found-repository-tools", cx));
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn an_older_answer_never_replaces_a_newer_one_and_busy_waits_for_the_last_answer(
    cx: &mut TestAppContext,
) {
    let user = user();
    trio(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "trio", cx);
    let section = section_of(&shell, cx);
    until(&shell, cx, "the repositories are read", |cx| {
        section.read_with(cx, |s, _| s.repositories().is_some())
    });
    settle(cx);
    let listed = |paths: &[&str]| {
        json!({"new": [], "repositories": paths.iter()
            .map(|path| json!({"path": path, "found": true, "remote": "origin",
                "base": ["main"], "branches": ["main"]}))
            .collect::<Vec<_>>()})
    };
    let first = section.read_with(cx, |s, _| s.last_request());
    // Detect again, while the answer to an earlier request is still on its way. The answers are
    // given here by hand, in the wrong order, before the window reads any of the real ones.
    section.update(cx, |s, cx| s.detect(cx));
    assert!(section.read_with(cx, |s, _| s.is_busy()));
    // A read asked for meanwhile (the file was edited) is not made: it would outdate the answer
    // of the user's request, a refusal included.
    section.update(cx, |s, cx| s.read(cx));
    assert_eq!(section.read_with(cx, |s, _| s.last_request()), first + 1);

    section.update_in(cx, |s, window, cx| {
        s.answered(first, Ok(listed(&["old"])), window, cx)
    });
    assert!(
        section.read_with(cx, |s, _| s.is_busy()),
        "an older answer does not end what the last request began"
    );
    assert_eq!(paths(&section, cx), ["api", "web", "lib"]);
    section.update_in(cx, |s, window, cx| {
        s.answered(first + 1, Ok(listed(&["new"])), window, cx)
    });
    assert!(!section.read_with(cx, |s, _| s.is_busy()));
    assert_eq!(paths(&section, cx), ["new"]);
    section.update_in(cx, |s, window, cx| {
        s.answered(first, Ok(listed(&["old"])), window, cx)
    });
    assert_eq!(paths(&section, cx), ["new"], "the older one came last");
}

#[gpui_kit::test]
fn a_project_edited_by_hand_shows_its_repositories_again_without_a_click(cx: &mut TestAppContext) {
    let user = user();
    let folder = trio(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    let section = open_settings(&shell, "trio", cx);
    until(&shell, cx, "the repositories are read", |cx| {
        section.read_with(cx, |s, _| s.repositories().is_some())
    });
    settle(cx);
    assert_eq!(paths(&section, cx), ["api", "web", "lib"]);

    // The file is the user's to edit: the daemon notices at its next request.
    let mut text = std::fs::read_to_string(user.project_file("trio")).unwrap();
    text.push_str("\n[[repositories]]\npath = \"docs\"\n");
    user.write_project_file("trio", &text);
    assert!(folder.join("docs/.git").is_dir());
    user.call("project.list", json!({})).unwrap();

    until(&shell, cx, "docs joins the list", |cx| {
        paths(&section, cx).contains(&"docs".to_owned())
    });
    settle(cx);
    assert!(has("repository-docs", cx));
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn the_new_workspace_form_creates_a_workspace_with_two_of_three_repositories(
    cx: &mut TestAppContext,
) {
    let user = user();
    trio(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "trio", cx);
    let form = shell
        .read_with(cx, |s, _| s.new_workspace_of("trio").cloned())
        .unwrap();
    cx.update(|window, cx| window.click("new-workspace", cx));
    until(&shell, cx, "the rules are read", |cx| {
        form.read_with(cx, |f, _| {
            f.base_said("api").is_some_and(|s| s.contains("develop"))
        })
    });
    settle(cx);
    let placeholder = form.read_with(cx, |f, _| f.base_said("api").map(str::to_owned));
    assert_eq!(
        placeholder.as_deref(),
        Some("base: develop, else main"),
        "the default is the project's rule"
    );

    cx.simulate_input("login");
    cx.update(|window, cx| window.click("new-workspace-take-web", cx));
    settle(cx);
    assert!(!has("new-workspace-base-web", cx), "its base folds away");
    cx.update(|window, cx| window.click("new-workspace-base-api", cx));
    cx.simulate_input("main");
    cx.update(|window, cx| window.click("create-workspace", cx));
    until(&shell, cx, "the workspace is made", |cx| {
        !form.read_with(cx, |f, _| f.is_open() || f.creating().is_some())
    });

    let shown = user
        .call(
            "workspace.show",
            json!({"project": "trio", "name": "login"}),
        )
        .unwrap();
    let bases: Vec<String> = shown["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| format!("{} {}", r["path"].as_str().unwrap(), r["base"]["branch"]))
        .collect();
    assert_eq!(bases, ["api \"main\"", "lib \"main\""], "{shown}");
    assert_eq!(
        shown["repositories"][1]["base"]["note"],
        "no remote origin: started from the local main"
    );
}

#[gpui_kit::test]
fn a_workspace_s_card_and_the_rail_show_where_each_repository_stands(cx: &mut TestAppContext) {
    let user = user();
    trio(&user);
    user.call(
        "workspace.create",
        json!({"project": "trio", "name": "login", "repositories": ["api", "lib"]}),
    )
    .unwrap();
    eventually("the workspace is ready", || {
        user.call(
            "workspace.show",
            json!({"project": "trio", "name": "login"}),
        )
        .unwrap()["state"]
            == "ready"
    });
    let api = user.path("data/flottille/workspaces/trio/login/api");
    std::fs::write(api.join("a.rs"), "fn a() {}\n").unwrap();
    user.git(&api, &["add", "."]);
    user.git(&api, &["commit", "--quiet", "-m", "a"]);
    std::fs::write(api.join("draft.rs"), "draft\n").unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "trio", cx);
    let card = |cx: &mut VisualTestContext| -> Option<Entity<PlaceCard>> {
        page_of(&shell, cx).read_with(cx, |page, cx| page.card(&Some("login".to_owned()), cx))
    };
    until(
        &shell,
        cx,
        "the card says where its repositories stand",
        |cx| card(cx).is_some_and(|card| card.read_with(cx, |c, _| !c.statuses().is_empty())),
    );
    settle(cx);

    assert_eq!(
        label_of("repository-status-login-api", cx).as_deref(),
        Some("api trio/login · 1 ahead of develop · 1 uncommitted")
    );
    assert_eq!(
        label_of("repository-status-login-lib", cx).as_deref(),
        Some(
            "lib trio/login · 0 ahead of main · clean · no remote origin: started from the \
             local main"
        )
    );
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");

    // The rail of a session in the workspace says the same.
    let lead = {
        let answer = user
            .call(
                "agent.start",
                json!({"project": "trio", "workspace": "login", "message": "hello"}),
            )
            .unwrap();
        answer["agent"]["id"].as_str().unwrap().to_owned()
    };
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the rail reads the workspace", |cx| {
        shell
            .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
            .is_some_and(|page: Entity<SessionPage>| {
                page.read_with(cx, |page, cx| {
                    page.rail()
                        .read(cx)
                        .place()
                        .is_some_and(|place| !place.statuses.is_empty())
                })
            })
    });
    settle(cx);
    assert_eq!(
        label_of("rail-status-api", cx).as_deref(),
        Some("trio/login · 1 ahead of develop · 1 uncommitted")
    );
    assert_eq!(
        label_of("rail-status-lib", cx).as_deref(),
        Some(
            "trio/login · 0 ahead of main · clean · no remote origin: started from the local main"
        )
    );
}

/// The rail of a session in the workspace `login` of `trio` over its three repositories, once
/// it has read the forges.
fn rail_over_three<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    user.call(
        "workspace.create",
        json!({"project": "trio", "name": "login", "repositories": ["api", "web", "lib"]}),
    )
    .unwrap();
    eventually("the workspace is ready", || {
        user.call(
            "workspace.show",
            json!({"project": "trio", "name": "login"}),
        )
        .unwrap()["state"]
            == "ready"
    });
    let (shell, cx) = window(user, Arc::new(user.connector()), 1280., cx);
    open_project(&shell, "trio", cx);
    let lead = user
        .call(
            "agent.start",
            json!({"project": "trio", "workspace": "login", "message": "hello"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&lead), cx));
    until(&shell, cx, "the rail reads the forges", |cx| {
        shell
            .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
            .is_some_and(|page: Entity<SessionPage>| {
                page.read_with(cx, |page, cx| {
                    page.rail()
                        .read(cx)
                        .place()
                        .is_some_and(|place| place.forges.len() == 3)
                })
            })
    });
    settle(cx);
    (shell, cx)
}

#[gpui_kit::test]
fn the_rail_says_once_that_several_repositories_have_no_forge(cx: &mut TestAppContext) {
    let user = user();
    trio(&user);
    let (_shell, cx) = rail_over_three(&user, cx);

    assert_eq!(
        label_of("rail-no-forge", cx).as_deref(),
        Some("No forge set for api, web, lib: choose it in the project's Settings")
    );
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn the_rail_names_only_the_repositories_with_no_forge(cx: &mut TestAppContext) {
    let user = user().with_fake_gh();
    trio(&user);
    user.git(
        &user.path("work/trio/api"),
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    user.call(
        "project.set_settings",
        json!({"name": "trio", "repositories": [
            {"path": "api", "forge": {"kind": "github", "repository": "acme/api"}}]}),
    )
    .unwrap();
    let (_shell, cx) = rail_over_three(&user, cx);

    assert_eq!(
        label_of("rail-no-forge", cx).as_deref(),
        Some("No forge set for web, lib: choose it in the project's Settings")
    );
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn a_repository_of_the_settings_says_what_it_depends_on(cx: &mut TestAppContext) {
    let user = user();
    trio(&user);
    let text = std::fs::read_to_string(user.project_file("trio"))
        .unwrap()
        .replace(
            "path = \"web\"\n",
            "path = \"web\"\ndepends_on = [\"api\"]\n",
        );
    user.write_project_file("trio", &text);
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    open_settings(&shell, "trio", cx);
    let page = shell
        .read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
        .unwrap();
    page.read_with(cx, |page, _| {
        let depends = |path: &str| {
            page.forms()
                .iter()
                .find(|form| form.path == path)
                .map(|form| form.depends_on.clone())
        };
        assert_eq!(depends("web"), Some(vec!["api".to_owned()]));
        assert_eq!(depends("api"), Some(Vec::new()));
    });
}
