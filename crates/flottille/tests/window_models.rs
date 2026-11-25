//! A provider's models are in the composer before any session opened (#307, `docs/agents.md`,
//! "The models"). On GPUI's test platform in front of a real daemon whose providers are fake ACP
//! agents, with a throwaway user. No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::shell::Selection;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

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
    cx.update(|cx| flottille_window::init_with(env, cx));
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
    let deadline = Instant::now() + Duration::from_secs(30);
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

/// What the agent picker says closed: `Provider · Model · effort`.
fn said(cx: &mut VisualTestContext) -> String {
    cx.update(|window, _| {
        window
            .try_find("agent-picker")
            .and_then(|found| found.label().map(str::to_owned))
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn the_composer_offers_the_models_before_any_session(cx: &mut TestAppContext) {
    let user = User::with_fake_providers().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.provider_script(
        "claude",
        json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}),
    );
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();

    let (shell, cx) = window(&user, Arc::new(user.connector()), 1280., cx);
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("atlas".into()), window, cx)
    });
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == "atlas")
        })
    });
    settle(cx);
    // No session ever opened: the daemon read the models, and the window shows them as they
    // come (the existing live update), in the one picker.
    until(&shell, cx, "the models are offered", |cx| {
        said(cx).contains("fake-small")
    });
    settle(cx);
    cx.update(|window, cx| window.click("agent-picker", cx));
    settle(cx);
    assert!(has("agent-picker-model-claude-fake-small", cx));
    assert!(has("agent-picker-model-claude-fake-large", cx));
    // The provider's real default is named, and already selected.
    cx.update(|window, _| {
        assert_eq!(
            window
                .find("agent-picker-model-claude-fake-small")
                .selected(),
            Some(true)
        );
        assert_eq!(
            window
                .find("agent-picker-model-claude-fake-large")
                .selected(),
            Some(false)
        );
    });
    let agents = user.call("agent.list", serde_json::Value::Null).unwrap();
    assert_eq!(agents["agents"], json!([]), "no session was opened");
}

/// Whether an element of that id is drawn.
fn has(id: &str, cx: &mut VisualTestContext) -> bool {
    let id: gpui_kit::ElementId = id.to_owned().into();
    cx.update(|window, _| window.try_find(id).is_some())
}

/// A user whose Claude says each model's efforts as Flottille's bridge does: `fake-small` has
/// `low` and `high`, `fake-large` the efforts `large` gives (none when empty). No session ever
/// runs on either before the test picks one.
fn efforts_user(large: serde_json::Value, more: serde_json::Value) -> User {
    let user = User::with_fake_providers().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    let mut script = json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"],
        "model_efforts": {"fake-large": large}, "said_efforts": true});
    for (key, value) in more.as_object().into_iter().flatten() {
        script[key] = value.clone();
    }
    user.provider_script("claude", script);
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();
    user
}

/// The window on the project's page, its composer drawn.
fn atlas<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, Arc::new(user.connector()), 1280., cx);
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("atlas".into()), window, cx)
    });
    until(&shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == "atlas")
        })
    });
    (shell, cx)
}

/// Chooses `fake-large` in the one picker, whose menu stays open when the model has efforts.
fn pick_large(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the models are offered", |cx| {
        said(cx).contains("fake-small")
    });
    cx.update(|window, cx| window.click("agent-picker", cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.click("agent-picker-model-claude-fake-large", cx));
    settle(cx);
}

#[gpui_kit::test]
fn picking_a_model_never_run_offers_its_efforts_once_learned(cx: &mut TestAppContext) {
    let user = efforts_user(json!(["low", "medium", "max"]), json!({}));
    let (shell, cx) = atlas(&user, cx);
    pick_large(&shell, cx);

    until(&shell, cx, "the efforts are offered", |cx| {
        has("agent-picker-effort-max", cx)
    });
    settle(cx);
    for level in ["low", "medium", "max"] {
        assert!(has(&format!("agent-picker-effort-{level}"), cx), "{level}");
    }
    assert!(!has("agent-picker-effort-high", cx), "only the model's own");
    assert!(
        said(cx).starts_with("Claude · fake-large · "),
        "{}",
        said(cx)
    );
    let agents = user.call("agent.list", serde_json::Value::Null).unwrap();
    assert_eq!(agents["agents"], json!([]), "no session ran on it");
}

#[gpui_kit::test]
fn a_model_without_effort_shows_none(cx: &mut TestAppContext) {
    let user = efforts_user(json!([]), json!({}));
    let (shell, cx) = atlas(&user, cx);
    until(&shell, cx, "the small model's efforts are said", |cx| {
        said(cx).starts_with("Claude · fake-small · ")
    });
    pick_large(&shell, cx);

    // The model offers none: the label says none, and the menu has no effort section.
    assert_eq!(said(cx), "Claude · fake-large");
    assert!(!has("agent-picker-effort-low", cx));
    assert!(!has("agent-picker-effort-high", cx));
}

#[gpui_kit::test]
fn while_efforts_are_learned_the_label_has_none_until_they_are_said(cx: &mut TestAppContext) {
    // The listing takes its time (three seconds, not ten): the efforts are being learned while
    // the page is shown.
    let user = efforts_user(json!(["low"]), json!({"init_wait": 3000}));
    let (shell, cx) = atlas(&user, cx);

    until(&shell, cx, "the efforts are said", |cx| {
        said(cx).starts_with("Claude · ") && said(cx).matches('·').count() == 2
    });
    settle(cx);
    assert!(
        said(cx).ends_with("low") || said(cx).ends_with("high"),
        "{}",
        said(cx)
    );
}

#[gpui_kit::test]
fn the_effort_picked_for_another_model_reaches_the_session(cx: &mut TestAppContext) {
    let user = efforts_user(json!(["low", "medium", "max"]), json!({}));
    let (shell, cx) = atlas(&user, cx);
    pick_large(&shell, cx);
    until(&shell, cx, "the efforts are offered", |cx| {
        has("agent-picker-effort-max", cx)
    });
    cx.update(|window, cx| window.click("agent-picker-effort-max", cx));
    cx.update(|window, cx| window.click("composer-field", cx));
    cx.simulate_input("Please look around");
    cx.simulate_keystrokes("enter");

    let deadline = Instant::now() + Duration::from_secs(20);
    let set = |user: &User| -> Vec<(String, String)> {
        user.provider_received("claude")
            .iter()
            .filter(|e| e["received"]["method"] == "session/set_config_option")
            .filter_map(|e| {
                let params = &e["received"]["params"];
                Some((
                    params["configId"].as_str()?.to_owned(),
                    params["value"].as_str()?.to_owned(),
                ))
            })
            .collect()
    };
    while !set(&user).contains(&("effort".into(), "max".into())) {
        assert!(
            Instant::now() < deadline,
            "the effort never reached the session: {:?}",
            set(&user)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let set = set(&user);
    let model = set
        .iter()
        .position(|s| *s == ("model".into(), "fake-large".into()));
    let effort = set
        .iter()
        .position(|s| *s == ("effort".into(), "max".into()));
    assert!(model.is_some() && model < effort, "{set:?}");
}

/// What the composer's line under its pickers says of its provider, `None` while it says nothing.
fn provider_warning(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find("composer-provider-warning")
            .and_then(|found| found.label().map(str::to_owned))
    })
}

#[gpui_kit::test]
fn the_picker_says_its_provider_needs_signing_in(cx: &mut TestAppContext) {
    // A Codex signed out answers the daemon's listing of its models with the sign-in line.
    let user = User::with_fake_codex().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.write_providers("[claude]\nenabled = false\n[codex]\nenabled = true\n");
    user.codex_script(json!({"account": serde_json::Value::Null}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let folder = user.path("work/atlas");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "atlas\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "atlas"}))
        .unwrap();

    let (shell, cx) = atlas(&user, cx);
    until(&shell, cx, "the sign-in line is said", |cx| {
        provider_warning(cx)
            .is_some_and(|said| said.contains("Codex's login expired: sign in again"))
    });
    settle(cx);
    // The picker still names its provider: the line is what says why it has no model.
    assert!(said(cx).contains("Codex"), "{}", said(cx));
    assert!(provider_warning(cx).unwrap().contains("run codex login"));
}
