//! A session led by the agent that started it (#521) in the window, on GPUI's test platform in
//! front of a real daemon whose agents are the fake ACP agent: its row sits under its pilot's and
//! folds with it, and the user detaches it from its pilot from the row's menu. No window opens on a
//! screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_ui::toggle_id;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::tree::{session_detach_id, session_id, session_led_id, session_menu_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "begin the docs session", "do": [{"mcp": {"tool": "agent_launch",
            "arguments": {"title": "Docs", "brief": "Write the docs", "kind": "led"}}}]}
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

/// The lead the user started and the session it leads: their ids.
fn lead_and_led(user: &User) -> (String, String) {
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
        .find(|agent| agent["led_by"] == lead.as_str())
        .expect("a session led by the lead")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (lead, launched)
}

/// The ids of the rows of the project's level, in the order drawn.
fn listed(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.project("atlas")
            .map(|node| {
                node.read(cx)
                    .sessions(cx)
                    .iter()
                    .map(|row| row.read(cx).id().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn led_by(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<String> {
    shell.read_with(cx, |s, cx| {
        s.session(id)
            .and_then(|row| row.read(cx).led_by().map(str::to_owned))
    })
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_sidebar_nests_a_led_session_under_its_pilot(cx: &mut TestAppContext) {
    let user = user();
    let (lead, led) = lead_and_led(&user);
    let (shell, cx) = window(&user, cx);

    until(&shell, cx, "the led session's row", |cx| {
        label(cx, session_id(&led)).is_some()
    });

    // Under its pilot: right after the pilot's row, in its own style, with who leads it.
    assert_eq!(listed(&shell, cx), [lead.clone(), led.clone()]);
    assert_eq!(
        led_by(&shell, &led, cx).as_deref(),
        Some("begin the docs session")
    );
    assert_eq!(
        label(cx, session_led_id(&led)).as_deref(),
        Some("led by begin the docs session")
    );
    assert_eq!(led_by(&shell, &lead, cx), None);

    // It folds with its pilot's rows and keeps its own row meanwhile.
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    settle(cx);
    assert_eq!(listed(&shell, cx), std::slice::from_ref(&lead));
    assert!(shell.read_with(cx, |s, _| s.session(&led).is_some()));
    cx.update(|window, cx| window.click(toggle_id(session_id(&lead)), cx));
    settle(cx);
    assert_eq!(listed(&shell, cx), [lead.clone(), led.clone()]);
}

#[gpui_kit::test]
fn the_window_offers_detach_and_moves_the_session_under_its_workspace(cx: &mut TestAppContext) {
    let user = user();
    let (lead, led) = lead_and_led(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the led session's row", |cx| {
        label(cx, session_id(&led)).is_some()
    });
    // Its page says who leads it, and that the user's messages go straight to it.
    cx.update(|window, cx| window.click(session_id(&led), cx));
    until(&shell, cx, "the page says who leads it", |cx| {
        label(cx, "session-led-by".into()).as_deref() == Some("led by begin the docs session")
    });
    let note = label(cx, "session-led-note".into()).expect("the note");
    assert!(note.contains("straight to this session"), "{note}");

    cx.update(|window, cx| window.hover(session_id(&led), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_menu_id(&led), cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_detach_id(&led), cx));

    until(&shell, cx, "the session detached", |cx| {
        led_by(&shell, &led, cx).is_none()
    });
    let shown = user.call("agent.show", json!({"agent": led})).unwrap();
    assert_eq!(shown["agent"]["detached_from"], lead.as_str());
    // Back among the project's sessions, its own: no longer under its pilot.
    let rows = listed(&shell, cx);
    assert!(rows.contains(&led) && rows.contains(&lead), "{rows:?}");
    assert_eq!(
        shell.read_with(cx, |s, cx| s
            .toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect::<Vec<_>>()),
        ["Detached from begin the docs session"]
    );
    // The line that said who led it folds away, the note with it.
    settle(cx);
    until(&shell, cx, "the led line gone", |cx| {
        label(cx, "session-led-by".into()).is_none()
            && label(cx, "session-led-note".into()).is_none()
    });
}
