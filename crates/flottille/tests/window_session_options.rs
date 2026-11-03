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
        "turns": [{"when": "slow", "do": [{"hold": "release"}, {"say": "done"}]}]
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
        // The platform's clock stands still while the test waits, and a picker that fades in
        // after the first frame would stay at nothing: it moves with the time spent waiting.
        cx.executor().advance_clock(Duration::from_millis(25));
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
fn the_effort_picker_comes_when_the_efforts_are_learned_after_the_window_opened(
    cx: &mut TestAppContext,
) {
    let user = user();
    // A first session whose agent has no effort: its picker has nothing to offer.
    user.fake_script(json!({"models": ["fake-small", "fake-large"]}));
    project(&user, "app");
    let id = start(&user, "hello", true);
    let (shell, cx) = open_session(&user, &id, cx);
    assert!(!has("session-effort-picker", cx));

    // Another session says the efforts of the model: the window reads them once drawn, as on a
    // machine slow enough to read the providers after the first frame.
    user.fake_script(json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}));
    start(&user, "again", true);
    until(&shell, cx, "the efforts are offered", |cx| {
        has("session-effort-picker", cx)
    });
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
    until(&shell, cx, "the pending mark shows", marked);

    // The mark may come from the window's own echo: the daemon's record is what is waited for,
    // while the turn is still held.
    eventually("the daemon keeps the change as pending", || {
        shown(&user, &id)["pending"]["model"] == "fake-large"
    });
    assert_eq!(shown(&user, &id)["state"], "working");
    assert!(
        options(&shell, &id, cx).read_with(cx, |o, _| o.model_shown().contains("fake-large")),
        "the picker already says the model asked"
    );
    assert!(marked(cx), "the mark stays while the turn is held");
    std::fs::write(user.path("fake/release"), "").unwrap();
    eventually("the turn ends", || shown(&user, &id)["state"] == "idle");
    until(&shell, cx, "the mark is cleared", |cx| {
        settle(cx);
        !marked(cx)
    });
    assert_eq!(shown(&user, &id)["taken"]["model"], "fake-large");
}

/// The toasts shown, as their title and their second line.
fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<(String, String)> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| {
                (
                    toast.title().to_owned(),
                    toast.second_line().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    })
}

fn pick_model(shell: &Entity<Shell>, model: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("session-model-picker", cx));
    let choice = format!("model-choice-{model}");
    until(shell, cx, "the models are offered", |cx| {
        has(choice.clone(), cx)
    });
    cx.update(|window, cx| window.click(choice.clone(), cx));
}

#[gpui_kit::test]
fn a_change_of_a_session_between_turns_is_set_at_once_and_leaves_no_mark(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "hello", true);
    let (shell, cx) = open_session(&user, &id, cx);

    pick_model(&shell, "fake-large", cx);
    eventually("the model is set on the agent", || {
        shown(&user, &id)["taken"]["model"] == "fake-large"
    });

    until(
        &shell,
        cx,
        "the session says it runs on the new model",
        |cx| {
            settle(cx);
            options(&shell, &id, cx).read_with(cx, |o, _| o.shown().contains("fake-large"))
                && !marked(cx)
        },
    );
}

#[gpui_kit::test]
fn a_change_made_from_the_command_line_shows_in_the_session_s_pickers(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "hello", true);
    let (shell, cx) = open_session(&user, &id, cx);

    user.call(
        "agent.options",
        json!({"agent": id, "model": "fake-large", "effort": "high"}),
    )
    .unwrap();

    until(&shell, cx, "the pickers say the change", |cx| {
        settle(cx);
        options(&shell, &id, cx).read_with(cx, |o, _| {
            o.shown().contains("fake-large")
                && o.model_shown().contains("fake-large")
                && o.effort_shown().contains("high")
        }) && !marked(cx)
    });
}

#[gpui_kit::test]
fn a_change_the_session_refuses_is_said_in_a_toast_naming_what_it_offers(cx: &mut TestAppContext) {
    let user = user();
    // The provider lists `fake-large`, and the session refuses to take it.
    user.fake_script(json!({
        "models": ["fake-small", "fake-large"],
        "efforts": ["low", "high"],
        "refuse": ["fake-large"]
    }));
    project(&user, "app");
    let id = start(&user, "hello", true);
    let (shell, cx) = open_session(&user, &id, cx);

    pick_model(&shell, "fake-large", cx);

    until(&shell, cx, "a toast says the refusal", |cx| {
        toasts(&shell, cx).iter().any(|(title, detail)| {
            title == "The model or effort was not changed"
                && detail.contains("fake-large")
                && detail.contains("fake-small")
        })
    });
    until(
        &shell,
        cx,
        "the picker is back on the model it runs",
        |cx| {
            settle(cx);
            options(&shell, &id, cx).read_with(cx, |o, _| o.model_shown().contains("fake-small"))
                && !marked(cx)
        },
    );
}

#[gpui_kit::test]
fn a_change_made_elsewhere_during_a_turn_is_said_pending_until_it_is_set(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "slow", false);
    eventually("the turn runs", || shown(&user, &id)["state"] == "working");
    let (shell, cx) = open_session(&user, &id, cx);

    user.call("agent.options", json!({"agent": id, "model": "fake-large"}))
        .unwrap();

    until(&shell, cx, "the pending mark shows", |cx| {
        settle(cx);
        marked(cx)
            && options(&shell, &id, cx).read_with(cx, |o, _| o.model_shown().contains("fake-large"))
    });
    std::fs::write(user.path("fake/release"), "").unwrap();
    until(&shell, cx, "the mark is cleared once it is set", |cx| {
        settle(cx);
        !marked(cx)
            && options(&shell, &id, cx).read_with(cx, |o, _| o.shown().contains("fake-large"))
    });
}

/// Whether a picker carries the mark of a change waiting.
fn marked(cx: &mut VisualTestContext) -> bool {
    has("session-model-pending", cx) || has("session-effort-pending", cx)
}

/// The height of the session's composer.
fn composer_height(cx: &mut VisualTestContext) -> gpui_kit::Pixels {
    cx.update(|window, _| window.find("reply").bounds().size.height)
}

#[gpui_kit::test]
fn a_pending_change_does_not_change_the_composer_s_height(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "slow", false);
    eventually("the turn runs", || shown(&user, &id)["state"] == "working");
    let (shell, cx) = open_session(&user, &id, cx);
    let before = composer_height(cx);

    pick_model(&shell, "fake-large", cx);
    until(&shell, cx, "the change is marked", |cx| {
        settle(cx);
        marked(cx)
    });
    assert_eq!(composer_height(cx), before, "while the change waits");
    assert!(
        !has("session-options-pending-line", cx),
        "no line under the bar"
    );

    std::fs::write(user.path("fake/release"), "").unwrap();
    until(&shell, cx, "the mark is cleared once it is set", |cx| {
        settle(cx);
        !marked(cx)
    });
    assert_eq!(composer_height(cx), before, "once it is set");
}

#[gpui_kit::test]
fn a_pending_change_is_marked_on_its_picker(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "slow", false);
    eventually("the turn runs", || shown(&user, &id)["state"] == "working");
    let (shell, cx) = open_session(&user, &id, cx);
    assert!(!marked(cx), "nothing waits");

    pick_model(&shell, "fake-large", cx);
    // The mark fades in: drawn at once, not yet whole.
    until(&shell, cx, "the model's picker is marked", |cx| {
        has("session-model-pending", cx)
    });
    assert!(
        !has("session-effort-pending", cx),
        "only the picker changed"
    );
    assert!(
        options(&shell, &id, cx).read_with(cx, |o, _| o.model_shown().contains("fake-large")),
        "the picker says the model asked"
    );
    settle(cx);
    assert!(
        has("session-model-pending", cx),
        "the mark stays while the change waits"
    );

    std::fs::write(user.path("fake/release"), "").unwrap();
    until(&shell, cx, "the mark leaves once the change is set", |cx| {
        settle(cx);
        !marked(cx)
    });
    assert_eq!(shown(&user, &id)["taken"]["model"], "fake-large");
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

#[gpui_kit::test]
fn the_picker_s_menu_says_when_a_change_applies(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "hello", true);
    let (shell, cx) = open_session(&user, &id, cx);
    let o = options(&shell, &id, cx);
    let first = |cx: &mut VisualTestContext| {
        o.read_with(cx, |o, _| {
            o.entries(flottille_window::composer::Menu::Model)
        })[0]
            .label
            .clone()
    };
    assert_ne!(first(cx), flottille_window::session_options::WITH_NEXT);

    // The tree says a change waits, the agent between turns.
    o.update(cx, |o, cx| {
        o.set_runs(
            flottille_window::session_options::Runs {
                provider: "claude".into(),
                model: Some("fake-small".into()),
                effort: Some("low".into()),
                pending: flottille_window::session_link::Pending {
                    model: Some("fake-large".into()),
                    effort: None,
                },
                working: false,
            },
            cx,
        )
    });
    assert_eq!(first(cx), flottille_window::session_options::WITH_NEXT);
    let entry = o.read_with(cx, |o, _| {
        o.entries(flottille_window::composer::Menu::Model)
    })[0]
        .clone();
    assert!(entry.pick.is_none(), "the line picks nothing");
}

#[gpui_kit::test]
fn only_the_picker_whose_change_waits_says_when_it_applies(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "hello", true);
    let (shell, cx) = open_session(&user, &id, cx);
    let o = options(&shell, &id, cx);
    // An effort change waits, the model's does not.
    o.update(cx, |o, cx| {
        o.set_runs(
            flottille_window::session_options::Runs {
                provider: "claude".into(),
                model: Some("fake-small".into()),
                effort: Some("low".into()),
                pending: flottille_window::session_link::Pending {
                    model: None,
                    effort: Some("high".into()),
                },
                working: false,
            },
            cx,
        )
    });
    let first = |menu, cx: &mut VisualTestContext| {
        o.read_with(cx, |o, _| o.entries(menu))
            .first()
            .map(|e| e.label.clone())
    };
    let with_next = Some(flottille_window::session_options::WITH_NEXT.to_owned());
    assert_ne!(
        first(flottille_window::composer::Menu::Model, cx),
        with_next
    );
    assert_eq!(
        first(flottille_window::composer::Menu::Effort, cx),
        with_next
    );
}

#[gpui_kit::test]
fn the_line_that_says_when_a_change_applies_does_not_answer_the_pointer(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "slow", false);
    eventually("the turn runs", || shown(&user, &id)["state"] == "working");
    let (shell, cx) = open_session(&user, &id, cx);
    pick_model(&shell, "fake-large", cx);
    until(&shell, cx, "the change is marked", |cx| {
        settle(cx);
        marked(cx)
    });
    cx.update(|window, cx| window.click("session-model-picker", cx));
    until(&shell, cx, "the menu says when it applies", |cx| {
        has("session-options-applies", cx)
    });
    settle(cx);
    cx.update(|window, cx| window.hover("session-options-applies", cx));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "no hover fades on a line that is not a choice"
    );
}
