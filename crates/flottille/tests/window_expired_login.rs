//! An expired login in the window (#631), on GPUI's test platform in front of a real daemon whose
//! agent is the fake ACP agent answering a prompt with Claude's sign-in error: the session stays,
//! its composer too, and its state line says it once; a failed root whose session is known shows
//! Resume. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::session::SessionPage;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

const EXPIRED: &str = "Failed to authenticate: OAuth session expired and could not be refreshed";

const LOGIN_LINE: &str =
    "Claude's login expired: sign in again (run claude and /login), then send your message again";

fn user(error: &str) -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "hello", "do": [{"say": "Looking"}], "error": error, "times": 1},
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

/// A project `app` and a root agent whose first turn failed: its id, once its process is gone.
fn failed_turn(user: &User) -> String {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the turn failed and its process went", || {
        let shown = user.call("agent.show", json!({"agent": id})).unwrap();
        shown["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "agent.released" || e["name"] == "agent.failed")
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
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1600.)));
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

fn one_frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..60 {
        one_frame(cx);
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn status_line(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find("reply-status")
            .map(|found| found.label().unwrap_or_default().to_owned())
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
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

#[gpui_kit::test]
fn an_expired_login_says_it_once_and_keeps_the_composer(cx: &mut TestAppContext) {
    let user = user(EXPIRED);
    let id = failed_turn(&user);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is in the sidebar", |cx| {
        cx.update(|window, _| window.try_find(session_id(&id)).is_some())
    });
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "the state line says the login expired", |cx| {
        one_frame(cx);
        status_line(cx).as_deref() == Some(LOGIN_LINE)
    });
    settle(cx);
    let page = session_page(&shell, cx);

    assert!(present(cx, "reply-field"), "the composer stays");
    assert!(!present(cx, "session-ended"), "the session did not end");
    assert_eq!(page.read_with(cx, |p, _| p.ended_line()), None);
    let said = blocks(&page, cx)
        .iter()
        .filter(|block| {
            let block = format!("{block:?}");
            block.contains("login") || block.contains("authenticate")
        })
        .count();
    assert_eq!(said, 0, "the conversation does not say it again");
    assert!(
        !blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Notice { kind, .. } if kind == "failed")),
        "no failed line"
    );
    assert_eq!(toasts(&shell, cx), Vec::<String>::new(), "no toast either");
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);

    // The next message takes the line away: the session goes on.
    cx.update(|window, cx| window.click("reply-field", cx));
    cx.simulate_input("again");
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the line goes once the turn runs", |cx| {
        one_frame(cx);
        status_line(cx).as_deref() != Some(LOGIN_LINE)
    });
    eventually("the message is answered on the same session", || {
        let shown = user.call("agent.show", json!({"agent": id})).unwrap();
        shown["agent"]["state"] == "idle"
            && shown["agent"]["state_reason"] == Value::Null
            && shown["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["name"] == "agent.started" && e["data"]["opened"] == "resumed")
    });
}

#[gpui_kit::test]
fn a_failed_root_shows_resume_when_resumable(cx: &mut TestAppContext) {
    let user = user("something nobody knows broke");
    let id = failed_turn(&user);
    assert_eq!(
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"],
        "failed"
    );
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the Ended group is in the tree", |cx| {
        present(cx, "ended")
    });
    cx.update(|window, cx| window.click("ended", cx));
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(
        &shell,
        cx,
        "the failed session's page, Resume offered",
        |cx| present(cx, "session-resume"),
    );
    settle(cx);
    let page = session_page(&shell, cx);
    let (said, resumable) = page.read_with(cx, |p, _| p.ended_line()).unwrap();
    assert!(said.starts_with("This session ended: failed"), "{said}");
    assert!(resumable);
    assert!(page.read_with(cx, |p, _| p.can_resume()));

    cx.update(|window, cx| window.click("session-resume", cx));
    until(&shell, cx, "the composer is back", |cx| {
        present(cx, "reply-field") && !present(cx, "session-ended")
    });
    assert_eq!(
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"],
        "idle"
    );
}
