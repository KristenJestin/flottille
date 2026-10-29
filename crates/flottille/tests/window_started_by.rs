//! A session an agent started for the user (#141) in the window, on GPUI's test platform in front
//! of a real daemon whose agents are the fake ACP agent: its row in the tree and its item in the
//! bell keep the title of its brief and say, small, which agent started it. No window opens on a
//! screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::tree::{session_id, session_started_by_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "root": true}}}]},
        {"when": "Write the docs", "do": [
            {"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn project(user: &User) {
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    std::fs::write(folder.join(".env"), "TOKEN=made-up-made-up\n").unwrap();
    user.git(&folder, &["add", "README.md", ".gitignore"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/atlas.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
}

fn agents(user: &User) -> Vec<serde_json::Value> {
    user.call("agent.list", json!({})).unwrap()["agents"]
        .as_array()
        .unwrap()
        .clone()
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
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

fn label(cx: &mut VisualTestContext, id: gpui_kit::ElementId) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// The lead the user started and the session it started for them, whose first turn reads a
/// file the gate holds: their ids.
fn lead_and_launched(user: &User) -> (String, String) {
    project(user);
    let lead = user
        .call(
            "agent.start",
            json!({"project": "atlas", "message": "begin the docs session"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The launch comes to the user; the session starts once they allow it.
    eventually("the launch is held", || {
        !user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    let number = user.call("approval.list", json!({})).unwrap()["requests"][0]["number"]
        .as_u64()
        .unwrap();
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the session exists", || agents(user).len() == 2);
    let launched = agents(user)
        .into_iter()
        .find(|agent| agent["launched_by"] == lead.as_str())
        .expect("a session launched by the lead")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, launched)
}

#[gpui_kit::test]
fn a_session_an_agent_started_keeps_its_title_and_says_who_started_it_in_its_row(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (lead, launched) = lead_and_launched(&user);
    let (shell, cx) = window(&user, cx);

    until(&shell, cx, "the launched session's row", |cx| {
        label(cx, session_started_by_id(&launched)).is_some()
    });

    assert_eq!(
        label(cx, session_started_by_id(&launched)).as_deref(),
        Some("started by begin the docs session")
    );
    assert_eq!(
        shell.read_with(cx, |s, cx| s
            .session(&launched)
            .map(|row| row.read(cx).title().to_owned())),
        Some("Write the docs".to_owned())
    );
    assert!(
        cx.update(|window, _| window.try_find(session_id(&lead)).is_some()),
        "the lead's row is there"
    );
    assert_eq!(
        label(cx, session_started_by_id(&lead)),
        None,
        "a session the user started names nobody"
    );
}

#[gpui_kit::test]
fn the_bell_item_of_a_session_an_agent_started_says_who_started_it(cx: &mut TestAppContext) {
    let user = user();
    let (_, launched) = lead_and_launched(&user);
    let (shell, cx) = window(&user, cx);

    until(&shell, cx, "the bell counts the request", |cx| {
        shell.read_with(cx, |s, cx| s.top().read(cx).count()) == 1
    });

    let session = shell.read_with(cx, |s, _| s.snapshot().bell[0].session.clone());
    assert_eq!(session, launched);
    assert_eq!(
        shell.read_with(cx, |s, cx| s.bell_sessions(cx)),
        ["Write the docs · started by begin the docs session"]
    );
}
