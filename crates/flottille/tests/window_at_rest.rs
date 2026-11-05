//! The window at rest (#444): once a page has settled, with every agent idle, no run and nothing
//! moving, the window draws no frame and asks for none, on GPUI's test platform in front of a
//! real daemon whose agents are the fake ACP agent. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// A user whose fake agent answers "ok", with the project `app` over a throwaway repository and
/// a session in it whose first turn ended: the session's id.
fn idle_session(user: &User) -> String {
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.start", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
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
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
}

/// One step of the window as the platform would run it: what the link sent, a frame if one was
/// asked, what it started. How many animation frames were asked.
fn step(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) -> usize {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    asked
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        step(shell, Duration::from_millis(16), cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Lets the page's entrance, every motion and every read it started end: two seconds of the
/// window, in real time too, so that what the daemon answers late has come.
fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(2) {
        step(shell, Duration::from_millis(16), cx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// What the window drew and asked for over ten seconds of its clock (and a few of the daemon's):
/// the frames drawn, the animation frames asked.
fn frames_at_rest(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> (u64, usize) {
    cx.update(|window, _| window.reset_layout_stats());
    let mut asked = 0;
    let mut gone = Duration::ZERO;
    while gone < Duration::from_secs(10) {
        asked += step(shell, Duration::from_millis(16), cx);
        gone += Duration::from_millis(16);
        std::thread::sleep(Duration::from_millis(5));
    }
    let drawn = cx.update(|window, _| window.layout_stats().frames);
    (drawn, asked)
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_session_page(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    let id = idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    settle(&shell, cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(
        &shell,
        cx,
        "the session's page shows its conversation",
        |cx| {
            shell.read_with(cx, |s, cx| {
                s.main()
                    .read(cx)
                    .session_page()
                    .is_some_and(|page| !page.read(cx).conversation().read(cx).rows().is_empty())
            })
        },
    );
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_the_project_page(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_a_project_s_rules_and_message_checks(
    cx: &mut TestAppContext,
) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings("app".into()), window, cx)
    });
    until(&shell, cx, "the project's settings are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_settings_page()
                .is_some_and(|page| page.read(cx).is_loaded())
        })
    });
    cx.update(|window, cx| window.click("project-settings-tabs-rules", cx));
    until(&shell, cx, "the message checks are shown", |cx| {
        cx.update(|window, _| window.try_find("message-checks-text-app").is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}

#[gpui_kit::test]
fn the_window_draws_no_frame_at_rest_on_settings(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    idle_session(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the project is known", |cx| {
        shell.read_with(cx, |s, _| !s.snapshot().projects.is_empty())
    });
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(&shell, cx);

    assert_eq!(
        frames_at_rest(&shell, cx),
        (0, 0),
        "(frames drawn, frames asked)"
    );
}
