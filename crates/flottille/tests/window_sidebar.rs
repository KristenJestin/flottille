//! The scenarios of #136: one spinner where the work is, nothing left spinning once an agent
//! ended, "Ended" at the bottom of the sidebar, a session ended or resumed and a session started
//! from a row, Stop in Send's place, the user's own messages selectable. On GPUI's test platform
//! in front of a real daemon whose agents are the fake ACP agent, with a throwaway user. No window
//! opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_ui::state::icon_id;
use flottille_ui::toggle_id;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::session::{Reply, SessionPage};
use flottille_window::tree::{place_id, project_id, session_id};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// A user whose fake agent works while it is told to.
fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "work briefly", "do": [{"wait": 1500}, {"say": "done"}]}
    ]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn state_of(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Starts a session and waits until its first turn ended.
fn start(user: &User, project: &str, message: &str) -> String {
    let answer = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || state_of(user, &id) == "idle");
    id
}

fn send(user: &User, agent: &str, message: &str) {
    user.call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
}

fn cancel(user: &User, agent: &str) {
    user.call("agent.cancel", json!({"agent": agent})).unwrap();
    eventually("the turn ends", || state_of(user, agent) == "idle");
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
    cx.simulate_resize(size(px(width), px(800.)));
    (shell, cx)
}

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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// A frame of the clock and a few of drawing: what a window that is looked at does while a
/// block folds away.
fn moving(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    frames(cx);
}

/// A few frames, as a window draws them after a change.
fn frames(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.update(|window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }
}

fn open_session<'a>(
    user: &User,
    lead: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, Entity<Reply>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, Arc::new(user.connector()), 1280., cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(lead)).is_some())
    });
    // Its row unfolds into the tree: it is there to be clicked once it has.
    settle(cx);
    cx.update(|window, cx| window.click(session_id(lead), cx));
    until(
        &shell,
        cx,
        "the session's page shows its conversation",
        |cx| {
            page_of(&shell, cx).read_with(cx, |page, cx| {
                !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
            })
        },
    );
    settle(cx);
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(lead).cloned())
        .expect("the session has a composer");
    (shell, reply, cx)
}

fn page_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn has(id: &'static str, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| window.try_find(id).is_some())
}

#[gpui_kit::test]
fn stop_takes_send_s_place_while_the_agent_works_and_the_swap_moves(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let lead = start(&user, "app", "hello");
    let (shell, reply, cx) = open_session(&user, &lead, cx);
    assert!(
        has("reply-send", cx) && !has("reply-stop", cx),
        "idle: Send alone"
    );

    send(&user, &lead, "HANG");
    until(&shell, cx, "the turn runs", |cx| {
        reply.read_with(cx, |r, _| r.is_running())
    });
    // The swap is a cross-fade: for a few frames both are drawn.
    let mut crossed = false;
    for _ in 0..12 {
        moving(cx);
        crossed |= has("reply-send", cx) && has("reply-stop", cx);
    }
    assert!(crossed, "Send fades out while Stop fades in");
    settle(cx);
    assert!(has("reply-stop", cx), "the turn runs: Stop");
    assert!(
        !has("reply-send", cx),
        "Stop replaces Send, it is not beside it"
    );

    // Something to send brings Send back, and takes Stop away.
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("one more thing");
    settle(cx);
    assert!(
        has("reply-send", cx) && !has("reply-stop", cx),
        "text typed: Send"
    );

    // Stop interrupts the turn, and Send comes back for good.
    for _ in 0..14 {
        cx.simulate_keystrokes("backspace");
    }
    settle(cx);
    assert!(has("reply-stop", cx) && !has("reply-send", cx));
    cx.update(|window, cx| window.click("reply-stop", cx));
    until(&shell, cx, "the turn ends", |cx| {
        !reply.read_with(cx, |r, _| r.is_running())
    });
    settle(cx);
    assert!(
        has("reply-send", cx) && !has("reply-stop", cx),
        "the turn ended: Send"
    );
    assert_eq!(state_of(&user, &lead), "idle", "the agent stays");
}
