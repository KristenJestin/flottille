//! The scenarios of #1350 (window part of #1345): a project whose configuration file is gone but
//! which still holds live sessions or workspaces is in the sidebar of its own machine, marked in
//! the user's words, with its sessions; its menu offers Remove, with the confirmation of any
//! project, and once removed its row folds away (`docs/projects.md`, "Removing a project"). On
//! GPUI's test platform in front of a real daemon whose agents are the fake ACP agent, with a
//! throwaway user. No window opens on a screen.

use crate::common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::tree::{
    project_id, project_menu_id, project_remove_id, project_unconfigured_id, session_id,
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
        present(cx, project_id("app"))
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
        !present(cx, project_menu_id("other")),
        "only a project with no file has the menu"
    );
    cx.update(|window, cx| window.hover(project_id("app"), cx));
    settle(&shell, cx);
    assert!(
        present(cx, project_menu_id("app")),
        "its menu is there for Remove"
    );
}

/// Asks to remove `app` from its row's menu, and waits for the confirmation to have read.
fn ask_to_remove_app(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the row of the project", |cx| {
        present(cx, project_id("app"))
    });
    settle(shell, cx);
    cx.update(|window, cx| window.hover(project_id("app"), cx));
    settle(shell, cx);
    cx.update(|window, cx| window.click(project_menu_id("app"), cx));
    settle(shell, cx);
    cx.update(|window, cx| window.click(project_remove_id("app"), cx));
    until(shell, cx, "the confirmation says what it does", |cx| {
        present(cx, "project-removal-workspace-clean")
    });
    settle(shell, cx);
    assert!(
        present(cx, project_id("app")),
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

    assert!(!present(cx, project_id("app")), "the row folded away");
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
    assert!(present(cx, project_id("app")));
    assert!(dirty.join("draft.txt").exists(), "its work is not lost");
}
