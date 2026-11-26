//! The scenarios of #1350 (window part of #1345): a project whose configuration file is gone but
//! which still holds live sessions or workspaces is in the sidebar of its own machine, marked in
//! the user's words, with its sessions; its menu offers Remove, with the confirmation of any
//! project, and once removed its row folds away (`docs/projects.md`, "Removing a project"). On
//! GPUI's test platform in front of a real daemon whose agents are the fake ACP agent, with a
//! throwaway user. No window opens on a screen.

use crate::common;
use crate::remote::{Machine, TestHub, pc_and_server_on};
use crate::window_any_machine as machines;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::model::elsewhere_id;
use flottille_window::shell::Selection;
use flottille_window::tree::{
    by_machine_id, project_gone_id, project_gone_on_id, project_id, project_menu_id, project_on_id,
    project_options_id, project_remove_id, project_unconfigured_id, projects_menu_id, session_id,
};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, size};
use serde_json::{Value, json};

const FRAME: Duration = Duration::from_millis(16);

fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn project(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
    folder
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn workspace(user: &User, project: &str, name: &str) -> PathBuf {
    user.call(
        "workspace.create",
        json!({"project": project, "name": name}),
    )
    .unwrap();
    eventually("the workspace is ready", || {
        user.call("workspace.show", json!({"project": project, "name": name}))
            .unwrap()["state"]
            == "ready"
    });
    let shown = user
        .call("workspace.show", json!({"project": project, "name": name}))
        .unwrap();
    PathBuf::from(shown["folder"].as_str().unwrap())
}

/// A root session that has answered and waits.
fn idle_root(user: &User, project: &str, workspace: &str) -> String {
    let answer = user
        .call(
            "agent.run",
            json!({"project": project, "workspace": workspace, "message": "hello"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the root is idle", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect = std::sync::Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(gpui_kit::px(1280.), gpui_kit::px(800.)));
    (shell, cx)
}

fn step(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(FRAME);
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        step(shell, cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..60 {
        step(shell, cx);
    }
}

fn present(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

/// `app` and `other`, `app` with a session and a clean workspace, its file then deleted while
/// the daemon runs. The session.
fn app_without_its_file(user: &User) -> String {
    project(user, "app");
    project(user, "other");
    workspace(user, "app", "clean");
    let session = idle_root(user, "app", "clean");
    std::fs::remove_file(user.project_file("app")).unwrap();
    session
}

/// `app_without_its_file` with a second workspace holding a file that is not committed.
fn app_without_its_file_but_dirty(user: &User) -> PathBuf {
    project(user, "app");
    let dirty = workspace(user, "app", "dirty");
    std::fs::write(dirty.join("draft.txt"), "not committed\n").unwrap();
    workspace(user, "app", "clean");
    idle_root(user, "app", "clean");
    std::fs::remove_file(user.project_file("app")).unwrap();
    dirty
}

#[gpui_kit::test]
fn a_project_whose_configuration_is_gone_is_in_the_sidebar(cx: &mut TestAppContext) {
    let user = user();
    let session = app_without_its_file(&user);
    let (shell, cx) = window(&user, cx);

    until(&shell, cx, "the row of the project", |cx| {
        present(cx, project_gone_id("app"))
    });
    settle(&shell, cx);

    assert_eq!(
        label(cx, project_unconfigured_id("app")).as_deref(),
        Some("settings file missing"),
        "the row says in words why it is marked"
    );
    assert!(
        present(cx, session_id(&session)),
        "its sessions are under it"
    );
    assert!(present(cx, project_id("other")));
    assert!(
        !present(cx, project_unconfigured_id("other")),
        "a project with its file is not marked"
    );
    // A row's menu shows under the pointer.
    cx.update(|window, cx| window.hover(project_id("other"), cx));
    settle(&shell, cx);
    assert!(
        present(cx, project_options_id("other")),
        "a project with its file has a menu, for Hide on this machine"
    );
    assert!(
        !present(cx, project_menu_id("other")),
        "the menu with Remove is a project with no file's"
    );
    cx.update(|window, cx| window.click(project_options_id("other"), cx));
    settle(&shell, cx);
    assert!(
        !present(cx, project_remove_id("other")),
        "but only a project with no file has Remove"
    );
    cx.update(|window, cx| window.click(project_options_id("other"), cx));
    cx.update(|window, cx| window.hover(project_gone_id("app"), cx));
    settle(&shell, cx);
    assert!(
        present(cx, project_menu_id("app")),
        "its menu is there for Remove"
    );
}

/// Asks to remove `app` from its row's menu, and waits for the confirmation to have read.
fn ask_to_remove_app(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the row of the project", |cx| {
        present(cx, project_gone_id("app"))
    });
    settle(shell, cx);
    cx.update(|window, cx| window.hover(project_gone_id("app"), cx));
    settle(shell, cx);
    cx.update(|window, cx| window.click(project_menu_id("app"), cx));
    settle(shell, cx);
    cx.update(|window, cx| window.click(project_remove_id("app"), cx));
    until(shell, cx, "the confirmation says what it does", |cx| {
        present(cx, "project-removal-workspace-clean")
    });
    settle(shell, cx);
    assert!(
        present(cx, project_gone_id("app")),
        "nothing is removed before the confirmation"
    );
}

fn said(shell: &Entity<Shell>, what: &str, cx: &mut VisualTestContext) {
    until(shell, cx, &format!("a toast says {what}"), |cx| {
        toasts(shell, cx).iter().any(|title| title.contains(what))
    });
    settle(shell, cx);
}

#[gpui_kit::test]
fn removing_a_project_whose_configuration_is_gone_from_the_window(cx: &mut TestAppContext) {
    let user = user();
    let session = app_without_its_file(&user);
    let (shell, cx) = window(&user, cx);
    ask_to_remove_app(&shell, cx);

    cx.update(|window, cx| window.click("project-removal-action", cx));
    said(&shell, "The project app was removed", cx);
    // The row folds away when the daemon's next list comes without the project, which the link
    // brings on its own thread: the toast does not say it came.
    until(&shell, cx, "the row folded away", |cx| {
        !present(cx, project_gone_id("app"))
    });
    assert_eq!(
        user.call("agent.show", json!({"agent": session})).unwrap()["agent"]["state"],
        "ended",
        "its session was ended (it moves to the Ended group)"
    );
    assert!(present(cx, project_id("other")));
    let listed: Value = user.call("project.list", json!({})).unwrap();
    assert_eq!(listed["unconfigured"], json!([]), "{listed}");
    assert!(user.path("work/app/README.md").exists());
}

#[gpui_kit::test]
fn a_workspace_holding_work_is_kept_and_the_confirmation_says_so(cx: &mut TestAppContext) {
    let user = user();
    let dirty = app_without_its_file_but_dirty(&user);
    let (shell, cx) = window(&user, cx);
    ask_to_remove_app(&shell, cx);
    assert_eq!(
        label(cx, "project-removal-workspace-dirty").as_deref(),
        Some("dirty uncommitted changes · kept"),
        "it says what is kept, and why"
    );

    cx.update(|window, cx| window.click("project-removal-action", cx));
    said(&shell, "The project app was removed", cx);

    let listed: Value = user.call("project.list", json!({})).unwrap();
    assert_eq!(
        listed["unconfigured"],
        json!([{"name": "app", "sessions": 0, "workspaces": 1}]),
        "what holds work stays, and so does its row: {listed}"
    );
    assert!(present(cx, project_gone_id("app")));
    assert!(dirty.join("draft.txt").exists(), "its work is not lost");
}

/// This machine (`pc`) has no settings file for `app` while its daemon still holds a session of
/// it, and another machine (`server`) has the project `app` with a session of its own. The
/// sessions: this machine's, and the server's.
fn app_gone_here_and_on_another_machine() -> (TestHub, Machine, Machine, String, String) {
    let (hub, pc, server) = pc_and_server_on(Machine::of(machines::runner()), machines::runner());
    machines::project(&pc.user, "app");
    machines::project(&server.user, "app");
    workspace(&pc.user, "app", "clean");
    let here = idle_root(&pc.user, "app", "clean");
    let there = server
        .user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the server's session is idle", || {
        server
            .user
            .call("agent.show", json!({"agent": there}))
            .is_ok_and(|shown| shown["agent"]["state"] == "idle")
    });
    eventually("pc is told server's project", || {
        let lists = pc.user.call("remote.sessions", json!({})).unwrap();
        lists["machines"][0]["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|p| p["name"] == "app")
    });
    let there = elsewhere_id("server", &there);
    std::fs::remove_file(pc.user.project_file("app")).unwrap();
    (hub, pc, server, here, there)
}

#[gpui_kit::test]
fn an_unconfigured_project_beside_another_machine_s_project_of_its_name_is_one_row_each(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, _server, here, there) = app_gone_here_and_on_another_machine();
    let (shell, cx) = machines::window(&pc, cx);
    machines::until(&shell, cx, "the server's session is in the tree", |cx| {
        present(cx, session_id(&there))
    });

    // Refresh after refresh, the group of the other machine stays the one node it was, never
    // the row with no file, and each session is under one row only.
    let group = shell.read_with(cx, |s, _| s.project("app").map(|node| node.entity_id()));
    assert!(group.is_some(), "the project app has a row");
    for _ in 0..30 {
        step(&shell, cx);
        shell.read_with(cx, |s, cx| {
            let node = s.project("app").expect("the row of app");
            assert_eq!(Some(node.entity_id()), group, "the row is the same node");
            assert!(
                !node.read(cx).unconfigured(),
                "the other machine's project is no project without its file"
            );
            let held: Vec<String> = node
                .read(cx)
                .sessions(cx)
                .iter()
                .map(|row| row.read(cx).id().to_owned())
                .collect();
            assert_eq!(
                held,
                std::slice::from_ref(&there),
                "it holds the server's session only"
            );
        });
    }
    assert!(
        present(cx, session_id(&here)),
        "this machine's session is under the row with no file"
    );
    // The row with no file is a node of its own, with ids of its own, beside the group's, said as
    // this machine's.
    assert!(present(cx, project_id("app")), "the group's row");
    assert!(present(cx, project_gone_id("app")), "the row with no file");
    assert_eq!(
        label(cx, project_unconfigured_id("app")).as_deref(),
        Some("settings file missing")
    );
    shell.read_with(cx, |s, cx| {
        let gone = s
            .unconfigured_project("app", None)
            .expect("the row with no file");
        assert_ne!(Some(gone.entity_id()), group);
        assert!(gone.read(cx).unconfigured());
        assert_eq!(gone.read(cx).apart(), Some("pc"), "it says whose it is");
        let held: Vec<String> = gone
            .read(cx)
            .sessions(cx)
            .iter()
            .map(|row| row.read(cx).id().to_owned())
            .collect();
        assert_eq!(
            held,
            std::slice::from_ref(&here),
            "it holds this machine's session only"
        );
    });
    // Both rows have a menu; only the row with no file has Remove in it.
    cx.update(|window, cx| window.hover(project_id("app"), cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click(project_options_id("app"), cx));
    settle(&shell, cx);
    assert!(!present(cx, project_remove_id("app")));
    cx.update(|window, cx| window.click(project_options_id("app"), cx));
    cx.update(|window, cx| window.hover(project_gone_id("app"), cx));
    settle(&shell, cx);
    assert!(present(cx, project_menu_id("app")));
}

#[gpui_kit::test]
fn an_unconfigured_project_beside_another_machine_s_project_is_one_row_each_by_machine(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, _server, here, there) = app_gone_here_and_on_another_machine();
    let (shell, cx) = machines::window(&pc, cx);
    machines::until(&shell, cx, "the server's session is in the tree", |cx| {
        present(cx, session_id(&there))
    });
    cx.update(|window, cx| window.click(projects_menu_id(), cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click(by_machine_id(), cx));
    until(&shell, cx, "the sections", |cx| {
        present(cx, project_on_id("app", "server")) && present(cx, project_gone_on_id("app", "pc"))
    });
    settle(&shell, cx);

    shell.read_with(cx, |s, cx| {
        let theirs = s.project_on("app", "server").expect("the server's row");
        let ours = s
            .unconfigured_project("app", Some("pc"))
            .expect("this machine's row");
        assert_ne!(theirs.entity_id(), ours.entity_id());
        assert!(!theirs.read(cx).unconfigured());
        assert!(ours.read(cx).unconfigured());
        let ids = |node: &Entity<flottille_window::tree::ProjectNode>| -> Vec<String> {
            node.read(cx)
                .sessions(cx)
                .iter()
                .map(|row| row.read(cx).id().to_owned())
                .collect()
        };
        assert_eq!(ids(theirs), std::slice::from_ref(&there));
        assert_eq!(ids(ours), std::slice::from_ref(&here));
    });
}

#[gpui_kit::test]
fn another_machine_s_project_of_the_same_name_still_drags(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, there) = app_gone_here_and_on_another_machine();
    let (shell, cx) = machines::window(&pc, cx);
    machines::until(&shell, cx, "the server's session is in the tree", |cx| {
        present(cx, session_id(&there))
    });

    shell.update_in(cx, |shell, window, cx| {
        shell.add_group("Work", cx);
        let work = shell.listing().groups[0].id.clone();
        shell.project_moved("app", Some(&work), 0, window, cx);
    });
    settle(&shell, cx);

    assert_eq!(
        shell.read_with(cx, |s, _| s.listing().groups[0].projects.clone()),
        ["app"],
        "the project of the other machine is in the group it was dragged to"
    );
}

#[gpui_kit::test]
fn removing_another_machine_s_project_of_the_same_name_is_answered_on_its_page(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server, _here, there) = app_gone_here_and_on_another_machine();
    let (shell, cx) = machines::window(&pc, cx);
    machines::until(&shell, cx, "the server's session is in the tree", |cx| {
        present(cx, session_id(&there))
    });
    shell.update_in(cx, |s, window, cx| {
        s.choose_machine(Some("server".to_owned()), window, cx)
    });
    machines::until(&shell, cx, "the server is shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    machines::until(&shell, cx, "the server's settings of app are read", |cx| {
        shell
            .read_with(cx, |s, cx| {
                s.main().read(cx).project_settings_page().cloned()
            })
            .is_some_and(|page| {
                page.read_with(cx, |page, cx| page.is_loaded() && page.has_read(cx))
            })
    });
    machines::click(&shell, "project-more", cx);
    machines::click(&shell, "remove-project", cx);
    machines::until(&shell, cx, "the confirmation is asked", |cx| {
        machines::present(cx, "project-removal-action")
    });
    machines::click(&shell, "project-removal-action", cx);

    machines::until(&shell, cx, "the server's page is left", |cx| {
        shell.read_with(cx, |s, _| {
            !matches!(s.selected(), Some(Selection::ProjectSettings(_)))
        })
    });
    let on_server = server.user.call("project.list", json!({})).unwrap();
    assert!(
        on_server["projects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|project| project["name"] != "app"),
        "the server's app was removed: {on_server}"
    );
    let here = pc.user.call("project.list", json!({})).unwrap();
    assert_eq!(
        here["unconfigured"][0]["name"], "app",
        "this machine's is untouched: {here}"
    );
}

/// How tall the row `name` with no file is drawn by its list item (`tree-project-<node>`), nothing
/// while it is not drawn.
fn item_height(cx: &mut VisualTestContext, node: u64) -> f32 {
    let id = gpui_kit::ElementId::Name(format!("tree-project-{node}").into());
    cx.update(|window, _| {
        window
            .find_all(id)
            .iter()
            .map(|found| f32::from(found.bounds().size.height))
            .fold(0., f32::max)
    })
}

/// The heights of the item of `node`, a frame of 16 ms and a poll of the daemon after another,
/// until `done` says so on what was seen.
fn heights(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    node: u64,
    done: impl Fn(&[f32]) -> bool,
) -> Vec<f32> {
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut seen = Vec::new();
    while !done(&seen) {
        assert!(Instant::now() < deadline, "the row never settled: {seen:?}");
        step(shell, cx);
        seen.push(item_height(cx, node));
        std::thread::sleep(Duration::from_millis(5));
    }
    seen
}

#[gpui_kit::test]
fn the_row_of_a_project_whose_file_goes_unfolds_into_the_list(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    project(&user, "other");
    workspace(&user, "app", "clean");
    idle_root(&user, "app", "clean");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row of the project", |cx| {
        present(cx, project_id("app"))
    });
    settle(&shell, cx);
    assert!(!present(cx, project_gone_id("app")));

    std::fs::remove_file(user.project_file("app")).unwrap();
    // The window reads the projects again when the daemon says something: another project's
    // session.
    user.call("agent.run", json!({"project": "other", "message": "hello"}))
        .unwrap();
    until(&shell, cx, "the row with no file", |cx| {
        shell.read_with(cx, |s, _| s.unconfigured_project("app", None).is_some())
    });
    let node = shell
        .read_with(cx, |s, _| {
            s.unconfigured_project("app", None)
                .map(|n| n.entity_id().as_u64())
        })
        .unwrap();
    let seen = heights(&shell, cx, node, |seen| seen.len() >= 30);
    let full = seen.iter().copied().fold(0., f32::max);
    assert!(full > 0., "the row is drawn: {seen:?}");
    assert!(
        seen.iter().any(|height| *height > 0. && *height < full),
        "the row unfolds over some frames, it does not jump: {seen:?}"
    );
    assert_eq!(
        label(cx, project_unconfigured_id("app")).as_deref(),
        Some("settings file missing")
    );
}

#[gpui_kit::test]
fn a_removed_project_s_row_folds_away_over_some_frames(cx: &mut TestAppContext) {
    let user = user();
    app_without_its_file(&user);
    let (shell, cx) = window(&user, cx);
    ask_to_remove_app(&shell, cx);
    let node = shell
        .read_with(cx, |s, _| {
            s.unconfigured_project("app", None)
                .map(|n| n.entity_id().as_u64())
        })
        .expect("the row");
    let full = item_height(cx, node);
    assert!(full > 0.);

    cx.update(|window, cx| window.click("project-removal-action", cx));
    let seen = heights(&shell, cx, node, |seen| seen.last() == Some(&0.));
    assert!(
        seen.iter().any(|height| *height > 0. && *height < full),
        "the row is still there and shrinking partway through the fold: {seen:?}"
    );
    assert_eq!(
        seen.last().copied(),
        Some(0.),
        "and gone at the end: {seen:?}"
    );
}
