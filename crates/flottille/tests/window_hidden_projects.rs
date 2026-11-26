//! The scenarios of #1526: the user hides a project on one machine (`docs/design.md` §10, the
//! sidebar; `docs/cli.md`, `flottille project hide`). The choice belongs to the machine that
//! hides: it is kept by that machine's daemon, hides nothing on another and sends nothing to the
//! project's machine, and a question waiting for the user is never hidden with its project. On
//! GPUI's test platform in front of real daemons, each with its own temporary HOME and XDG
//! folders, fake agents and throwaway repositories, and a hub served in the test on 127.0.0.1. No
//! window opens on a screen.

use crate::common;
use crate::remote::{Machine, pc_and_server_on};
use crate::window_notifications::{self as notified, BRIEF};

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::tree::{project_id, project_options_id, session_id};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// How long a link, a session or a read of the machines may take.
const WAIT: Duration = Duration::from_secs(60);
const FRAME: Duration = Duration::from_millis(16);

fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The project `name` of `user`, over a throwaway repository whose remote is `remote` (none when
/// it is empty).
fn add(user: &User, name: &str, remote: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    if !remote.is_empty() {
        user.git(&folder, &["remote", "add", "origin", remote]);
    }
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A session the user starts in `project`, its first turn ended: its id.
fn session(user: &User, project: &str) -> String {
    let started = user
        .call("agent.run", json!({"project": project, "message": "hello"}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id}))
            .is_ok_and(|shown| shown["agent"]["state"] == "idle")
    });
    id
}

/// What the machine hides, as its daemon says it.
fn hidden(user: &User) -> Vec<String> {
    let answer = user.call("project.hidden", json!({})).unwrap();
    answer["hidden"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect()
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
    let connect: Connect = Arc::new(user.connector());
    let device = Some(Device::in_folder(user.path("state/flottille/remote")));
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
}

fn step(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(by);
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
    let deadline = Instant::now() + WAIT;
    loop {
        step(shell, FRAME, cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..60 {
        step(shell, FRAME, cx);
    }
}

fn present(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

/// Hides `name` from its row's menu: the pointer on the row, the menu, its entry.
fn hide_from_its_row(name: &str, shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.hover(project_id(name), cx));
    settle(shell, cx);
    cx.update(|window, cx| window.click(project_options_id(name), cx));
    settle(shell, cx);
    assert_eq!(
        label(cx, format!("project-hide-{name}-label")).as_deref(),
        Some("Hide on this machine"),
        "the row's menu offers it"
    );
    cx.update(|window, cx| window.click(format!("project-hide-{name}"), cx));
}

fn label(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

#[gpui_kit::test]
fn a_project_hidden_on_this_machine_leaves_the_sidebar_with_its_sessions(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    let held = session(&user, "alpha");
    let kept = session(&user, "bravo");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "both rows with their sessions", |cx| {
        present(cx, session_id(&held)) && present(cx, session_id(&kept))
    });
    settle(&shell, cx);

    hide_from_its_row("alpha", &shell, cx);
    settle(&shell, cx);

    assert!(!present(cx, project_id("alpha")), "its row folded away");
    assert!(!present(cx, session_id(&held)), "and its sessions with it");
    assert!(present(cx, project_id("bravo")), "another stays");
    assert!(present(cx, session_id(&kept)));
    // The window asks the daemon from a thread of its own: the answer is waited for.
    until(&shell, cx, "the daemon keeps the choice", |_| {
        hidden(&user) == ["alpha"]
    });
    let shown = user.call("agent.show", json!({"agent": held})).unwrap();
    assert_eq!(
        shown["agent"]["state"], "idle",
        "the project's sessions go on"
    );
}

#[gpui_kit::test]
fn a_hidden_project_is_not_offered_when_choosing_a_project(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    user.call("project.hide", json!({"name": "bravo"})).unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| present(cx, project_id("alpha")));
    settle(&shell, cx);
    assert!(!present(cx, project_id("bravo")));

    // The vault's entry names the project whose agents see it.
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("settings-tabs-vault", cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("add-vault-entry", cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("vault-project", cx));
    settle(&shell, cx);

    assert!(
        present(cx, "vault-project-alpha"),
        "a shown project is offered"
    );
    assert!(
        !present(cx, "vault-project-bravo"),
        "a hidden project is not"
    );
}

#[gpui_kit::test]
fn a_hidden_project_comes_back_from_settings(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    let held = session(&user, "bravo");
    user.call("project.hide", json!({"name": "bravo"})).unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| present(cx, project_id("alpha")));
    settle(&shell, cx);
    assert!(!present(cx, project_id("bravo")));

    // General is long: the window is tall enough to show the whole section.
    cx.simulate_resize(size(px(1280.), px(2400.)));
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("settings-tabs-general", cx));
    settle(&shell, cx);
    assert_eq!(
        label(cx, "hidden-projects-title").as_deref(),
        Some("Hidden projects"),
        "the section is on General"
    );
    assert!(
        present(cx, "hidden-project-row-bravo"),
        "the project is listed"
    );
    assert!(
        !present(cx, "hidden-project-row-alpha"),
        "only the hidden ones"
    );
    assert_eq!(
        label(cx, "hidden-project-show-bravo").as_deref(),
        Some("Show")
    );

    cx.update(|window, cx| window.click("hidden-project-show-bravo", cx));
    settle(&shell, cx);

    assert!(
        !present(cx, "hidden-project-row-bravo"),
        "its row leaves the list"
    );
    assert!(
        present(cx, "hidden-projects-empty"),
        "an empty list says what it is for"
    );
    assert!(hidden(&user).is_empty(), "the daemon forgot it");
    until(&shell, cx, "the row is back", |cx| {
        present(cx, project_id("bravo"))
    });
    assert!(present(cx, session_id(&held)), "with its session");
}

#[gpui_kit::test]
fn a_hidden_project_is_still_shown_on_another_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server_on(Machine::of(user()), user());
    let shared = "https://git.example/acme/shop.git";
    add(&pc.user, "shop", shared);
    add(&server.user, "shop", shared);

    // Hidden on the pc, and only there.
    pc.user
        .call("project.hide", json!({"name": "shop"}))
        .unwrap();
    assert!(hidden(&server.user).is_empty());

    let (shell, cx) = window(&server.user, cx);
    until(&shell, cx, "the row on the server", |cx| {
        present(cx, project_id("shop"))
    });
    settle(&shell, cx);
    assert!(present(cx, project_id("shop")), "the server shows it");
    assert!(hidden(&server.user).is_empty());
    assert_eq!(hidden(&pc.user), ["shop"], "the pc keeps its own choice");
}

#[gpui_kit::test]
fn a_session_of_a_hidden_project_that_needs_the_user_is_still_in_needs_you(
    cx: &mut TestAppContext,
) {
    let user = notified::user();
    let (_, mcp) = notified::lead(&user);
    user.call("project.hide", json!({"name": "app"})).unwrap();
    let (shell, cx) = notified::window(&user, cx);
    notified::until(&shell, cx, "the window has read", |cx| {
        shell.read_with(cx, |s, _| {
            s.snapshot().projects.iter().any(|p| p.name == "app")
        })
    });
    notified::settle(cx);
    assert!(
        !notified::has(project_id("app"), cx),
        "the project is hidden from the sidebar"
    );

    let number = notified::held_number(mcp.call("fs_read", json!({"path": ".env"})));
    notified::until(&shell, cx, "the bell counts the request", |cx| {
        notified::bell_count(&shell, cx) == 1
    });

    let said = notified::bell_said(&shell, cx);
    assert_eq!(
        said,
        [(
            format!("Request #{number} waits for you"),
            "Read .env".to_owned(),
            BRIEF.to_owned()
        )],
        "the question of a hidden project still waits for the user, and rings"
    );
    assert!(
        !notified::has(project_id("app"), cx),
        "and its row stays hidden"
    );
}

#[gpui_kit::test]
fn hiding_a_project_sends_nothing_to_its_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server_on(Machine::of(user()), user());
    add(&server.user, "site", "git@git.example:acme/site.git");
    let running = session(&server.user, "site");
    eventually("pc is told server's project", || {
        let lists = pc.user.call("remote.sessions", json!({})).unwrap();
        lists["machines"][0]["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|p| p["name"] == "site")
    });
    let (shell, cx) = window(&pc.user, cx);
    until(&shell, cx, "the row of the server's project", |cx| {
        present(cx, project_id("site"))
    });
    settle(&shell, cx);
    let before: Value = server.user.call("window.get", json!({})).unwrap();

    hide_from_its_row("site", &shell, cx);
    settle(&shell, cx);

    assert!(!present(cx, project_id("site")), "hidden on the pc");
    until(&shell, cx, "the pc keeps the choice", |_| {
        hidden(&pc.user) == ["site"]
    });
    assert!(hidden(&server.user).is_empty(), "the server hides nothing");
    assert_eq!(
        server.user.call("window.get", json!({})).unwrap(),
        before,
        "the server's own state is as it was"
    );
    let shown = server
        .user
        .call("agent.show", json!({"agent": running}))
        .unwrap();
    assert_eq!(shown["agent"]["state"], "idle", "its session goes on");
    assert!(
        server.user.call("project.list", json!({})).unwrap()["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "site"),
        "the project is where it was"
    );
}
