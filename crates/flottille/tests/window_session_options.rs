//! A running session's model and effort, changed from its composer (#311), on GPUI's test platform
//! in front of a real daemon whose agent is the fake ACP agent, with a throwaway user. No window
//! opens on a screen.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::session_options::SessionOptions;
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "efforts": ["low", "high"],
        "turns": [{"when": "slow", "do": [{"wait": 2500}, {"say": "done"}]}]
    }));
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

fn project(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
    folder
}

fn shown(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone()
}

/// Starts a session in `main` of `app`; with `settle`, waits until its first turn ended.
fn start(user: &User, message: &str, settled: bool) -> String {
    let answer = user
        .call("agent.start", json!({"project": "app", "message": message}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    if settled {
        eventually("the first turn ends", || {
            shown(user, &id)["state"] == "idle"
        });
    }
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
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1440.), px(800.)));
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// The window on `agent`'s page, its pickers drawn.
fn open_session<'a>(
    user: &User,
    agent: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(agent), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(agent), cx));
    until(&shell, cx, "the session's composer shows its model", |cx| {
        has("session-model-picker", cx)
    });
    settle(cx);
    (shell, cx)
}

fn options(
    shell: &Entity<Shell>,
    agent: &str,
    cx: &mut VisualTestContext,
) -> Entity<SessionOptions> {
    let reply = shell
        .read_with(cx, |s, _| s.reply_of(agent).cloned())
        .expect("the session's composer exists");
    reply.read_with(cx, |reply, _| reply.options().clone())
}

#[gpui_kit::test]
fn the_session_composer_offers_the_model_and_effort_not_the_provider(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "hello", true);
    let (shell, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "the efforts are offered", |cx| {
        has("session-effort-picker", cx)
    });

    assert!(has("session-model-picker", cx));
    assert!(has("session-effort-picker", cx));
    assert!(
        !has("session-provider-picker", cx) && !has("provider-picker", cx),
        "the provider is shown, not picked"
    );
    assert!(
        label("reply-provider", cx).is_some_and(|label| label.eq_ignore_ascii_case("claude")),
        "the provider is named"
    );
    cx.update(|window, cx| window.click("session-model-picker", cx));
    until(&shell, cx, "the models are offered", |cx| {
        has("model-choice-fake-large", cx)
    });
    assert!(has("model-choice-fake-small", cx));
    cx.update(|window, cx| window.click("model-choice-fake-large", cx));
    eventually("the model is set on the agent", || {
        shown(&user, &id)["taken"]["model"] == "fake-large"
    });
    until(&shell, cx, "the picker says the new model", |cx| {
        options(&shell, &id, cx).read_with(cx, |o, _| o.model_shown().contains("fake-large"))
    });
    until(&shell, cx, "the new model's efforts are offered", |cx| {
        has("session-effort-picker", cx)
    });
    cx.update(|window, cx| window.click("session-effort-picker", cx));
    until(&shell, cx, "the efforts are listed", |cx| {
        has("effort-choice-high", cx)
    });
    cx.update(|window, cx| window.click("effort-choice-high", cx));
    eventually("the effort is set on the agent", || {
        shown(&user, &id)["taken"]["effort"] == "high"
    });
    until(&shell, cx, "the picker says the new effort", |cx| {
        options(&shell, &id, cx).read_with(cx, |o, _| o.effort_shown().contains("high"))
    });
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing is redrawn at rest"
    );
}

#[gpui_kit::test]
fn a_change_chosen_during_a_turn_is_said_pending_and_the_mark_goes_when_it_is_set(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "slow", false);
    eventually("the turn runs", || shown(&user, &id)["state"] == "working");
    let (shell, cx) = open_session(&user, &id, cx);

    cx.update(|window, cx| window.click("session-model-picker", cx));
    until(&shell, cx, "the models are offered", |cx| {
        has("model-choice-fake-large", cx)
    });
    cx.update(|window, cx| window.click("model-choice-fake-large", cx));
    until(&shell, cx, "the pending mark shows", |cx| {
        has("session-options-pending", cx)
    });

    assert_eq!(shown(&user, &id)["pending"]["model"], "fake-large");
    assert!(
        options(&shell, &id, cx).read_with(cx, |o, _| o.model_shown().contains("fake-large")),
        "the picker already says the model asked"
    );
    eventually("the turn ends", || shown(&user, &id)["state"] == "idle");
    until(&shell, cx, "the mark is cleared", |cx| {
        settle(cx);
        !has("session-options-pending", cx)
    });
    assert_eq!(shown(&user, &id)["taken"]["model"], "fake-large");
}
