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
use flottille_window::shell::Selection;
use flottille_window::tree::{
    by_machine_id, machine_id, project_id, project_on_id, project_options_id, projects_menu_id,
    session_id,
};
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

/// The tallest height of the frames drawn under `id`, nothing while none is drawn (two frames of
/// one row are drawn while it moves between two levels).
fn height(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> f32 {
    let id = id.into();
    cx.update(|window, _| {
        window
            .find_all(id)
            .iter()
            .map(|found| f32::from(found.bounds().size.height))
            .fold(0., f32::max)
    })
}

/// The heights of the frame `id` over the next `frames` frames of 16 ms: what a motion draws.
fn heights_over(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    id: &gpui_kit::ElementId,
    frames: usize,
) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            step(shell, FRAME, cx);
            height(cx, id.clone())
        })
        .collect()
}

/// Said of a frame that folds away: it was `full`, was seen between nothing and `full`, and ends
/// at nothing.
fn folds_away(seen: &[f32], full: f32, what: &str) {
    assert!(full > 0., "{what}: it was drawn before: {seen:?}");
    assert!(
        seen.iter().any(|h| *h > 0. && *h < full),
        "{what}: part-way through its height is between nothing and {full}: {seen:?}"
    );
    assert_eq!(seen.last(), Some(&0.), "{what}: it ends folded: {seen:?}");
}

/// Said of a frame that unfolds: it was nothing, is seen between nothing and its height, and ends
/// at it.
fn unfolds(seen: &[f32], what: &str) {
    let full = seen.last().copied().unwrap_or(0.);
    assert!(full > 0., "{what}: it ends drawn: {seen:?}");
    assert!(
        seen.iter().any(|h| *h > 0. && *h < full),
        "{what}: part-way through its height is between nothing and {full}: {seen:?}"
    );
}

fn frame(prefix: &str, node: u64) -> gpui_kit::ElementId {
    gpui_kit::ElementId::Name(format!("{prefix}-{node}").into())
}

/// The id of the entity of the row of `name`, which its list frames it by (`tree-project-<id>`).
fn node_of(shell: &Entity<Shell>, name: &str, cx: &mut VisualTestContext) -> u64 {
    shell.read_with(cx, |s, _| {
        s.project(name).expect("the row").entity_id().as_u64()
    })
}

/// The window on Settings, General, tall enough for the whole of its sections.
fn general_settings(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.simulate_resize(size(px(1280.), px(2400.)));
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(shell, cx);
    cx.update(|window, cx| window.click("settings-tabs-general", cx));
    settle(shell, cx);
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
    // The window asks the daemon from a thread of its own, and an older read of the list that
    // answers after the click is put right by the daemon's answer to the click: the state is
    // looked at once the daemon has forgotten it.
    until(&shell, cx, "the daemon forgot it", |_| {
        hidden(&user).is_empty()
    });
    settle(&shell, cx);

    assert!(
        !present(cx, "hidden-project-row-bravo"),
        "its row leaves the list"
    );
    assert!(
        present(cx, "hidden-projects-empty"),
        "an empty list says what it is for"
    );
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

// The motion of #1526 (AGENTS.md, "Motion in the window"): a frame is seen part-way through its
// way, not only where it ends.

#[gpui_kit::test]
fn the_row_of_a_hidden_project_folds_away_over_the_motion_not_at_once(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "both rows", |cx| {
        present(cx, project_id("alpha")) && present(cx, project_id("bravo"))
    });
    settle(&shell, cx);
    let row = frame("tree-project", node_of(&shell, "alpha", cx));
    let full = height(cx, row.clone());

    hide_from_its_row("alpha", &shell, cx);
    let seen = heights_over(&shell, cx, &row, 40);

    folds_away(&seen, full, "the row of alpha");
    assert!(present(cx, project_id("bravo")), "another stays");
}

#[gpui_kit::test]
fn a_project_shown_again_folds_from_the_hidden_list_and_swaps_with_its_empty_line(
    cx: &mut TestAppContext,
) {
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    user.call("project.hide", json!({"name": "bravo"})).unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| present(cx, project_id("alpha")));
    settle(&shell, cx);
    general_settings(&shell, cx);
    assert!(present(cx, "hidden-project-row-bravo"));
    let list = gpui_kit::ElementId::from("hidden-projects-list-block");
    let empty = gpui_kit::ElementId::from("hidden-projects-empty-block");
    let full = height(cx, list.clone());
    assert_eq!(
        height(cx, empty.clone()),
        0.,
        "no empty line while a row is listed"
    );

    cx.update(|window, cx| window.click("hidden-project-show-bravo", cx));
    let (mut going, mut coming) = (Vec::new(), Vec::new());
    for _ in 0..40 {
        step(&shell, FRAME, cx);
        going.push(height(cx, list.clone()));
        coming.push(height(cx, empty.clone()));
    }

    folds_away(&going, full, "the list block");
    unfolds(&coming, "the empty line's block");
    assert!(
        going
            .iter()
            .zip(&coming)
            .any(|(list, empty)| *list > 0. && *empty > 0.),
        "the two swap, one folding as the other unfolds: {going:?} {coming:?}"
    );
}

#[gpui_kit::test]
fn a_project_shown_again_folds_its_own_row_away_from_a_list_of_two(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    add(&user, "charlie", "");
    user.call("project.hide", json!({"name": "bravo"})).unwrap();
    user.call("project.hide", json!({"name": "charlie"}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the row", |cx| present(cx, project_id("alpha")));
    settle(&shell, cx);
    general_settings(&shell, cx);
    let row = gpui_kit::ElementId::from("hidden-project-item-bravo");
    let full = height(cx, row.clone());

    cx.update(|window, cx| window.click("hidden-project-show-bravo", cx));
    let seen = heights_over(&shell, cx, &row, 40);

    folds_away(&seen, full, "the row of bravo in Settings");
    assert!(
        present(cx, "hidden-project-row-charlie"),
        "the other row stays"
    );
    assert!(
        !present(cx, "hidden-projects-empty"),
        "no empty line while one is listed"
    );
}

#[gpui_kit::test]
fn a_project_hidden_while_settings_is_open_unfolds_in_the_list_and_the_empty_line_folds(
    cx: &mut TestAppContext,
) {
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "both rows", |cx| {
        present(cx, project_id("alpha")) && present(cx, project_id("bravo"))
    });
    settle(&shell, cx);
    general_settings(&shell, cx);
    let list = gpui_kit::ElementId::from("hidden-projects-list-block");
    let empty = gpui_kit::ElementId::from("hidden-projects-empty-block");
    let full = height(cx, empty.clone());
    assert!(present(cx, "hidden-projects-empty"));

    hide_from_its_row("bravo", &shell, cx);
    let (mut going, mut coming) = (Vec::new(), Vec::new());
    for _ in 0..40 {
        step(&shell, FRAME, cx);
        going.push(height(cx, empty.clone()));
        coming.push(height(cx, list.clone()));
    }

    folds_away(&going, full, "the empty line's block");
    unfolds(&coming, "the list block");
}

#[gpui_kit::test]
fn a_hide_the_daemon_refuses_brings_the_row_back_and_says_so(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    let held = session(&user, "alpha");
    let (down, real) = (Arc::new(AtomicBool::new(false)), user.connector());
    let connect: Connect = {
        let down = down.clone();
        Arc::new(move || match down.load(Ordering::SeqCst) {
            true => Err(std::io::Error::other("the daemon is not answering")),
            false => real(),
        })
    };
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let device = Some(Device::in_folder(user.path("state/flottille/remote")));
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    until(&shell, cx, "the row and its session", |cx| {
        present(cx, project_id("alpha")) && present(cx, session_id(&held))
    });
    settle(&shell, cx);
    let row = frame("tree-project", node_of(&shell, "alpha", cx));
    let full = height(cx, row.clone());

    // The daemon does not answer the hide.
    down.store(true, Ordering::SeqCst);
    hide_from_its_row("alpha", &shell, cx);
    let mut folding = Vec::new();
    let deadline = Instant::now() + WAIT;
    while !toasts(&shell, cx).iter().any(|title| title == "Not hidden") {
        assert!(Instant::now() < deadline, "the failure was never said");
        step(&shell, FRAME, cx);
        folding.push(height(cx, row.clone()));
        std::thread::sleep(Duration::from_millis(5));
    }
    down.store(false, Ordering::SeqCst);

    // What was shown at once is taken back: the row unfolds again, with its session.
    let seen = heights_over(&shell, cx, &row, 40);
    assert_eq!(
        seen.last(),
        Some(&full),
        "the row is back as it was: {seen:?} after {folding:?}"
    );
    assert!(present(cx, project_id("alpha")));
    assert!(present(cx, session_id(&held)), "with its session");
    assert!(
        shell.read_with(cx, |s, _| !s.is_hidden("alpha")),
        "the window no longer holds it hidden"
    );
    assert!(hidden(&user).is_empty(), "the daemon never had it");
}

#[gpui_kit::test]
fn a_hidden_project_is_not_offered_to_the_switcher_new_session_or_a_task_copy(
    cx: &mut TestAppContext,
) {
    let user = user();
    // `abacus` is listed first, and hidden: the first project is not the one to start in.
    add(&user, "abacus", "");
    add(&user, "bravo", "");
    add(&user, "cedar", "");
    let made = user
        .call(
            "workspace.create",
            json!({"project": "abacus", "name": "side", "offline": true}),
        )
        .unwrap();
    assert_eq!(made["state"], "ready", "{made}");
    user.call(
        "task.add",
        json!({"project": "bravo", "name": "check", "line": "cargo test"}),
    )
    .unwrap();
    user.call("project.hide", json!({"name": "abacus"}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the rows", |cx| {
        present(cx, project_id("bravo")) && present(cx, project_id("cedar"))
    });
    settle(&shell, cx);
    assert!(!present(cx, project_id("abacus")));

    // The switcher finds the projects and the workspaces that are shown, not the hidden one's.
    cx.simulate_keystrokes("ctrl-k");
    settle(&shell, cx);
    cx.simulate_input("a");
    settle(&shell, cx);
    let said = shell.read_with(cx, |s, cx| s.switcher().read(cx).said());
    let found: Vec<&String> = said
        .iter()
        .filter(|(heading, _)| heading != "Sessions")
        .flat_map(|(_, titles)| titles)
        .collect();
    assert!(
        found.iter().any(|title| title.as_str() == "bravo"),
        "a shown project is found: {said:?}"
    );
    assert!(
        found
            .iter()
            .all(|title| !title.contains("abacus") && title.as_str() != "side"),
        "neither the hidden project, its workspace nor its settings are: {said:?}"
    );
    cx.simulate_keystrokes("escape");
    settle(&shell, cx);

    // "New session" with no project in front starts in the first shown one.
    shell.update_in(cx, |s, window, cx| s.open(Selection::Settings, window, cx));
    settle(&shell, cx);
    shell.update_in(cx, |s, window, cx| s.new_session(window, cx));
    settle(&shell, cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("bravo".into())),
        "the first project is hidden: the next one is"
    );

    // The dialog of a task's copy offers the shown projects only.
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("bravo".into()), window, cx)
    });
    until(&shell, cx, "the project's settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    cx.update(|window, cx| window.click("project-settings-tabs-tasks", cx));
    until(&shell, cx, "the task is listed", |cx| {
        present(cx, "task-more-check")
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("task-more-check", cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("copy-task-check", cx));
    settle(&shell, cx);
    assert!(present(cx, "copy-task-to-cedar"), "a shown project is");
    assert!(
        !present(cx, "copy-task-to-abacus"),
        "a hidden project is not"
    );
}

#[gpui_kit::test]
fn hiding_the_open_project_moves_to_the_next_one(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the page of the first project", |cx| {
        shell.read_with(cx, |s, _| {
            s.selected() == Some(&Selection::Project("alpha".into()))
        })
    });
    settle(&shell, cx);

    hide_from_its_row("alpha", &shell, cx);
    settle(&shell, cx);

    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("bravo".into())),
        "the page of a project that is hidden is not left in front"
    );
    assert!(present(cx, project_id("bravo")));
}

#[gpui_kit::test]
fn hiding_the_only_project_leaves_no_page_of_it_open(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "alpha", "");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the page of the project", |cx| {
        shell.read_with(cx, |s, _| s.selected().is_some())
    });
    settle(&shell, cx);

    hide_from_its_row("alpha", &shell, cx);
    settle(&shell, cx);

    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        None,
        "nothing is open when no project is shown"
    );
}

#[gpui_kit::test]
fn a_new_session_never_starts_in_a_hidden_project(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "alpha", "");
    add(&user, "bravo", "");
    let held = session(&user, "alpha");
    user.call("project.hide", json!({"name": "alpha"})).unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the page of the shown project", |cx| {
        shell.read_with(cx, |s, _| {
            s.selected() == Some(&Selection::Project("bravo".into()))
        })
    });
    settle(&shell, cx);

    // A notification, or a session found in the switcher, opens a page of the hidden project.
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("alpha".into()), window, cx)
    });
    settle(&shell, cx);
    shell.update_in(cx, |s, window, cx| s.new_session(window, cx));
    settle(&shell, cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("bravo".into())),
        "from the page of a hidden project"
    );

    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Session(held.clone()), window, cx)
    });
    settle(&shell, cx);
    shell.update_in(cx, |s, window, cx| s.new_session(window, cx));
    settle(&shell, cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("bravo".into())),
        "from a session of a hidden project"
    );
}

#[gpui_kit::test]
fn a_removal_never_falls_back_on_a_hidden_project(cx: &mut TestAppContext) {
    let user = user();
    add(&user, "abacus", "");
    add(&user, "bravo", "");
    add(&user, "cedar", "");
    user.call("project.hide", json!({"name": "bravo"})).unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the first shown project is open", |cx| {
        shell.read_with(cx, |s, _| {
            s.selected() == Some(&Selection::Project("abacus".into()))
        })
    });
    settle(&shell, cx);

    // `abacus` is removed from its settings page: the next of the list is bravo, hidden.
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("abacus".into()), window, cx)
    });
    until(&shell, cx, "the settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).has_read(cx))
        })
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click("project-more", cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("remove-project", cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click("project-removal-action", cx));
    until(&shell, cx, "the project is gone", |cx| {
        shell.read_with(cx, |s, _| {
            !matches!(s.selected(), Some(Selection::ProjectSettings(_)))
        })
    });
    settle(&shell, cx);

    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Project("cedar".into())),
        "the page that follows is a shown project's"
    );
}

#[gpui_kit::test]
fn a_machine_whose_projects_are_all_hidden_leaves_the_sections(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server_on(Machine::of(user()), user());
    add(&pc.user, "app", "git@git.example:acme/app.git");
    add(&server.user, "site", "git@git.example:acme/site.git");
    eventually("pc is told server's project", || {
        let lists = pc.user.call("remote.sessions", json!({})).unwrap();
        lists["machines"][0]["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|p| p["name"] == "site")
    });
    let (shell, cx) = window(&pc.user, cx);
    until(&shell, cx, "the rows of both machines", |cx| {
        present(cx, project_id("app")) && present(cx, project_id("site"))
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(projects_menu_id(), cx));
    settle(&shell, cx);
    cx.update(|window, cx| window.click(by_machine_id(), cx));
    until(&shell, cx, "the sections", |cx| {
        present(cx, machine_id("pc")) && present(cx, machine_id("server"))
    });
    settle(&shell, cx);
    let node = shell.read_with(cx, |s, _| s.section("server").unwrap().entity_id().as_u64());
    let section = frame("tree-machine", node);
    let full = height(cx, section.clone());

    shell.update_in(cx, |s, window, cx| s.hide_project("site", window, cx));
    let seen = heights_over(&shell, cx, &section, 40);

    folds_away(&seen, full, "the section of the server");
    assert!(
        !present(cx, machine_id("server")),
        "no empty header is left for a machine with nothing shown"
    );
    assert!(present(cx, machine_id("pc")), "the other machine stays");
    assert!(present(cx, project_on_id("app", "pc")));
}
