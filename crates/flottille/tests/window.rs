//! The scenarios of the window's shell (#11), on GPUI's test platform in front of a real daemon
//! whose agents are the fake ACP agent, with a throwaway user (temporary home and XDG folders).
//! No window opens on a screen.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_ui::state::icon_id;
use flottille_window::Shell;
use flottille_window::link::{self, Connect};
use flottille_window::shell::{Layout, Selection};
use flottille_window::tree::{place_id, project_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A user whose fake agent answers "ok", reads `.env` when asked to (a call the gate holds for
/// the user), asks the user a question when asked to, and works a few seconds when asked to.
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "read the env", "do": [{"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}}]},
        {"when": "ask me", "do": [{"mcp": {"tool": "ask_user", "arguments": {"question": "Which database?"}}}]},
        {"when": "work a while", "do": [{"wait": 4000}, {"say": "done"}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A project over a throwaway repository with a remote and a `.env`.
fn project(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), "TOKEN=made-up-made-up\n").unwrap();
    user.git(&folder, &["add", "README.md", ".gitignore"]);
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
    folder
}

fn workspace(user: &User, project: &str, name: &str) {
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
}

/// Starts a session: a root agent with its first message. Its id.
fn start(user: &User, project: &str, workspace: Option<&str>, message: &str) -> String {
    let mut params = json!({"project": project, "message": message});
    if let Some(workspace) = workspace {
        params["workspace"] = json!(workspace);
    }
    let answer = user.call("agent.start", params).unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == id.as_str() && a["state"] == "idle")
    });
    id
}

fn send(user: &User, agent: &str, message: &str) {
    user.call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The window's shell in a test window of `width` × 800, following the user's daemon through
/// `connect`.
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
    cx.simulate_resize(size(px(width), px(800.)));
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the link sent, then draws what changed
/// (nothing else), as a real frame would.
fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        shell.update(cx, |shell, cx| shell.poll(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A fresh read of what the shell shows, on a connection of its own.
fn fresh(user: &User) -> flottille_window::model::Snapshot {
    link::read_once((user.connector())().unwrap()).unwrap()
}

fn mark_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<AgentState> {
    shell.read_with(cx, |shell, cx| {
        shell.session(id).and_then(|row| row.read(cx).mark())
    })
}

#[gpui_kit::test]
fn two_projects_main_and_a_workspace_with_sessions_under_each_appear_in_the_tree(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    project(&user, "notes");
    workspace(&user, "atlas", "export-csv");
    let in_main = start(&user, "atlas", None, "Release notes for 2.4");
    let in_workspace = start(&user, "atlas", Some("export-csv"), "CSV export by tenant");
    let in_notes = start(&user, "notes", None, "Offline sync spike");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);

    until(&shell, cx, "the three sessions are shown", |cx| {
        cx.update(|window, _| {
            [&in_main, &in_workspace, &in_notes]
                .iter()
                .all(|id| window.try_find(session_id(id)).is_some())
        })
    });
    cx.update(|window, _| {
        for id in [
            project_id("atlas"),
            project_id("notes"),
            place_id("atlas", &None),
            place_id("atlas", &Some("export-csv".into())),
            place_id("notes", &None),
        ] {
            assert!(window.try_find(id.clone()).is_some(), "{id:?} is not shown");
        }
        // One level below the other, each on its own guide line.
        let x = |id| window.find(id).bounds().origin.x;
        assert!(x(place_id("atlas", &None)) > x(project_id("atlas")));
        assert_eq!(
            x(place_id("atlas", &None)),
            x(place_id("atlas", &Some("export-csv".into()))),
            "main and the workspaces sit at the same level"
        );
        assert!(x(session_id(&in_main)) > x(place_id("atlas", &None)));
    });
    let sessions_of = |project: &str, place: Option<&str>, cx: &mut VisualTestContext| {
        shell.read_with(cx, |shell, cx| {
            shell
                .place(project, &place.map(str::to_owned))
                .unwrap()
                .read(cx)
                .sessions(cx)
                .iter()
                .map(|row| row.read(cx).title().to_owned())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(sessions_of("atlas", None, cx), ["Release notes for 2.4"]);
    assert_eq!(
        sessions_of("atlas", Some("export-csv"), cx),
        ["CSV export by tenant"]
    );
    assert_eq!(sessions_of("notes", None, cx), ["Offline sync spike"]);
}

#[gpui_kit::test]
fn a_session_that_starts_waiting_marks_its_row_its_folded_place_and_its_folded_project_and_the_bell_count_goes_up(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let id = start(&user, "atlas", None, "Sessions expire too early");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the session is shown", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    assert_eq!(shell.read_with(cx, |s, cx| s.top().read(cx).count()), 0);

    // Fold the place, then the project.
    cx.update(|window, cx| window.click(place_id("atlas", &None), cx));
    cx.update(|window, cx| window.click(project_id("atlas"), cx));
    let (place, project) = shell.read_with(cx, |shell, _| {
        (
            shell.place("atlas", &None).unwrap().clone(),
            shell.project("atlas").unwrap().clone(),
        )
    });
    assert!(!place.read_with(cx, |p, _| p.is_open()));
    assert!(!project.read_with(cx, |p, _| p.is_open()));

    send(&user, &id, "read the env");
    until(&shell, cx, "the session waits for the user", |cx| {
        mark_of(&shell, &id, cx) == Some(AgentState::NeedsYou)
    });
    assert_eq!(
        place.read_with(cx, |p, _| p.mark()),
        Some(AgentState::NeedsYou)
    );
    assert_eq!(
        project.read_with(cx, |p, _| p.mark()),
        Some(AgentState::NeedsYou),
        "folding never hides what waits"
    );
    assert_eq!(shell.read_with(cx, |s, cx| s.top().read(cx).count()), 1);
    cx.update(|window, cx| window.render_frame(cx));
    let shown = cx.update(|window, _| window.find_all(icon_id(AgentState::NeedsYou)).len());
    assert_eq!(shown, 3, "the row, the place and the project show the mark");
}

#[gpui_kit::test]
fn choosing_an_item_of_the_bell_opens_its_session(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let other = start(&user, "atlas", None, "Tidy the aliases");
    let id = start(&user, "atlas", None, "Sessions expire too early");
    send(&user, &id, "read the env");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the bell counts the request", |cx| {
        shell.read_with(cx, |s, cx| s.top().read(cx).count()) == 1
    });
    cx.update(|window, cx| window.click(session_id(&other), cx));
    cx.update(|window, cx| window.click("bell", cx));
    assert!(shell.read_with(cx, |s, _| s.bell_open()));
    let key = shell.read_with(cx, |s, _| s.snapshot().bell[0].key.clone());
    cx.update(|window, cx| window.click(format!("bell-{key}"), cx));
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(id))
    );
    assert!(
        !shell.read_with(cx, |s, _| s.bell_open()),
        "a choice closes the list"
    );
}

#[gpui_kit::test]
fn a_root_s_question_marks_its_session_and_rings_the_bell_until_the_user_writes(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let id = start(&user, "atlas", None, "Sessions expire too early");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the session is shown", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });

    send(&user, &id, "ask me");
    until(&shell, cx, "the question waits for the user", |cx| {
        mark_of(&shell, &id, cx) == Some(AgentState::NeedsYou)
    });
    assert_eq!(shell.read_with(cx, |s, cx| s.top().read(cx).count()), 1);
    let item = shell.read_with(cx, |s, _| s.snapshot().bell[0].clone());
    assert_eq!(item.session, id);
    assert_eq!(item.detail, "Which database?");

    send(&user, &id, "postgres");
    until(&shell, cx, "the answer is taken", |cx| {
        shell.read_with(cx, |s, cx| s.top().read(cx).count()) == 0
            && mark_of(&shell, &id, cx) != Some(AgentState::NeedsYou)
    });
}

#[gpui_kit::test]
fn one_session_changing_state_redraws_its_row_and_not_the_others(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "login");
    let quiet = [
        start(&user, "atlas", None, "First"),
        start(&user, "atlas", None, "Second"),
        start(&user, "atlas", Some("login"), "Third"),
    ];
    let busy = start(&user, "atlas", Some("login"), "Fourth");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the sessions are shown", |cx| {
        cx.update(|window, _| window.try_find(session_id(&busy)).is_some())
    });
    let renders = |id: &str, cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| s.session(id).unwrap().read(cx).renders())
    };
    let before: Vec<usize> = quiet.iter().map(|id| renders(id, cx)).collect();
    let busy_before = renders(&busy, cx);

    send(&user, &busy, "work a while");
    until(&shell, cx, "the session runs", |cx| {
        mark_of(&shell, &busy, cx) == Some(AgentState::Running)
    });
    until(&shell, cx, "the session is at rest again", |cx| {
        mark_of(&shell, &busy, cx).is_none()
    });

    assert!(renders(&busy, cx) > busy_before, "its row is drawn again");
    let after: Vec<usize> = quiet.iter().map(|id| renders(id, cx)).collect();
    assert_eq!(after, before, "the other rows are not drawn again");
}

#[gpui_kit::test]
fn when_the_daemon_is_restarted_the_window_reconnects_and_catches_up_without_a_gap(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let first = start(&user, "atlas", None, "Before the restart");
    // The window's way to the daemon, which the test closes while it changes things.
    let closed = Arc::new(AtomicBool::new(false));
    let connector = user.connector();
    let gate = closed.clone();
    let connect: Connect = Arc::new(move || {
        if gate.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("closed by the test"));
        }
        connector()
    });
    let (shell, cx) = window(&user, connect, 1280., cx);
    until(&shell, cx, "the session is shown", |cx| {
        cx.update(|window, _| window.try_find(session_id(&first)).is_some())
    });

    closed.store(true, Ordering::SeqCst);
    let stopped = user.run(&["daemon", "stop"]);
    assert!(stopped.status.success());
    until(&shell, cx, "the window sees the daemon gone", |cx| {
        !shell.read_with(cx, |s, cx| s.top().read(cx).online())
    });
    // A new daemon, and changes the window has not seen.
    project(&user, "notes");
    let second = start(&user, "notes", None, "After the restart");
    send(&user, &second, "read the env");
    eventually("the request is held", || {
        !user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    closed.store(false, Ordering::SeqCst);

    until(&shell, cx, "the window catches up", |cx| {
        shell.read_with(cx, |s, _| s.snapshot() == &fresh(&user))
    });
    assert!(shell.read_with(cx, |s, cx| s.top().read(cx).online()));
    assert_eq!(mark_of(&shell, &second, cx), Some(AgentState::NeedsYou));
    let seen = shell.read_with(cx, |s, _| s.seen().clone());
    assert_eq!(seen.gaps.load(Ordering::SeqCst), 0, "no event was missed");
    let latest = (user.connector())().unwrap();
    let mut conn = std::io::BufReader::new(latest.try_clone().unwrap());
    use std::io::{BufRead, Write};
    writeln!(
        &latest,
        r#"{{"id":"1","method":"events.watch","params":{{}}}}"#
    )
    .unwrap();
    let mut line = String::new();
    conn.read_line(&mut line).unwrap();
    let answer: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        seen.last_seq.load(Ordering::SeqCst),
        answer["result"]["after"].as_u64().unwrap(),
        "the window saw every event up to the last"
    );
}

#[gpui_kit::test]
fn below_860_the_tree_is_a_drawer_opened_by_the_menu_button(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let id = start(&user, "atlas", None, "On a portrait screen");
    let (shell, cx) = window(&user, Arc::new(user.connector()), 720., cx);
    until(&shell, cx, "the session is known", |cx| {
        shell.read_with(cx, |s, _| s.session(&id).is_some())
    });
    assert_eq!(shell.read_with(cx, |s, _| s.layout()), Layout::Narrow);
    cx.update(|window, cx| window.render_frame(cx));
    assert!(
        cx.update(|window, _| window.try_find(session_id(&id)).is_none()),
        "the tree is not drawn beside the page"
    );
    cx.update(|window, cx| window.click("open-tree", cx));
    flottille_ui_advance(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(id))
    );
    let drawer = shell.read_with(cx, |s, _| s.left_drawer().clone());
    assert!(
        !drawer.read_with(cx, |d, _| d.is_open()),
        "a choice closes the drawer"
    );

    // Wider again: the tree is beside the page, the rail of a session a drawer.
    cx.simulate_resize(size(px(1000.), px(800.)));
    until(&shell, cx, "the layout follows the width", |cx| {
        shell.read_with(cx, |s, _| s.layout()) == Layout::Medium
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(cx.update(|window, _| window.try_find("open-tree").is_none()));
    assert!(cx.update(|window, _| window.try_find("open-rail").is_some()));
    cx.simulate_resize(size(px(1300.), px(800.)));
    until(&shell, cx, "three columns", |cx| {
        shell.read_with(cx, |s, _| s.layout()) == Layout::Wide
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(cx.update(|window, _| window.try_find("open-rail").is_none()));
}

/// Lets a drawer's slide end.
fn flottille_ui_advance(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}
