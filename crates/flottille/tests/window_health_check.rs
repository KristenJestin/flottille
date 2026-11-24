//! The health check in the window (#1179): what `flottille doctor` checks is a section of
//! Settings, General, **Check Flottille**: each check a row with its state and, where the window
//! can make it, its fix. On GPUI's test platform in front of a real daemon in temporary folders,
//! its providers the fake ones. No window opens on a screen.

use crate::common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
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
        cx.executor().advance_clock(Duration::from_millis(16));
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

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn click(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn user() -> User {
    let user = User::with_fake_providers();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn enabled(user: &User, name: &str) -> bool {
    user.call("provider.list", json!({})).unwrap()["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["name"] == name)
        .unwrap()["enabled"]
        == true
}

#[gpui_kit::test]
fn the_health_check_is_a_page_of_settings(cx: &mut TestAppContext) {
    let user = user();
    assert!(!enabled(&user, "codex"));
    let (shell, cx) = window(&user, cx);
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        present(cx, "settings-tabs-general")
    });
    click(cx, "settings-tabs-general");
    until(&shell, cx, "the checks are listed", |cx| {
        present(cx, "check-flottille-provider-claude")
    });
    settle(cx);

    // A section of General, with a verb for the read made anew.
    assert_eq!(
        label(cx, "check-flottille-title").as_deref(),
        Some("Check Flottille")
    );
    assert_eq!(
        label(cx, "check-flottille-again").as_deref(),
        Some("Check again")
    );
    // Each provider is a row with its state, said once: one that runs, one that is off.
    assert_eq!(
        label(cx, "check-flottille-provider-claude-state").as_deref(),
        Some("Can run")
    );
    assert_eq!(
        label(cx, "check-flottille-provider-codex-state").as_deref(),
        Some("Off")
    );
    assert!(!present(cx, "check-flottille-provider-claude-fix"));
    // What sandboxes the agents' commands is a row of its own, as `doctor` says it.
    assert!(present(cx, "check-flottille-sandbox"));

    // Its fix is where the window can make it: Turn on, as the provider's switch does.
    assert_eq!(
        label(cx, "check-flottille-provider-codex-fix").as_deref(),
        Some("Turn on")
    );
    click(cx, "check-flottille-provider-codex-fix");
    until(&shell, cx, "the provider is on", |_| {
        enabled(&user, "codex")
    });
    until(&shell, cx, "its row says it", |cx| {
        label(cx, "check-flottille-provider-codex-state").as_deref() != Some("Off")
    });
    // The fix folds away with the state it mended.
    until(&shell, cx, "its fix folds away", |cx| {
        !present(cx, "check-flottille-provider-codex-fix")
    });
}

fn height(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| f32::from(window.find(id).bounds().size.height))
}

/// Opens Settings, General, with the checks listed and the sandbox read.
fn open_checks(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        present(cx, "settings-tabs-general")
    });
    click(cx, "settings-tabs-general");
    until(shell, cx, "the checks are listed", |cx| {
        label(cx, "check-flottille-sandbox-state").is_some_and(|state| state != "Reading…")
            && present(cx, "check-flottille-provider-codex-fix")
    });
    settle(cx);
}

const STATES: [&str; 3] = [
    "check-flottille-provider-codex-state",
    "check-flottille-provider-claude-state",
    "check-flottille-sandbox-state",
];

/// #1285: `Off` was drawn "O" over "ff" beside its `Turn on` button, in a narrow window.
fn a_state_word_stays_on_one_line(shade: &str, cx: &mut TestAppContext) {
    let user = user();
    user.call(
        "appearance.set",
        json!({"theme": "flottille", "shade": shade}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    open_checks(&shell, cx);
    let wide: Vec<f32> = STATES.iter().map(|id| height(cx, id)).collect();
    assert!(wide.iter().all(|height| *height > 0.), "{wide:?}");

    // From a narrow window to one barely wider than its sidebar.
    for width in [420., 320., 240., 160.] {
        cx.simulate_resize(size(px(width), px(2400.)));
        settle(cx);
        for (id, wide) in STATES.iter().zip(&wide) {
            assert_eq!(
                height(cx, id),
                *wide,
                "{id} is one line high at {width} px, as in a wide window"
            );
        }
    }
}

#[gpui_kit::test]
fn a_state_word_beside_a_button_stays_on_one_line(cx: &mut TestAppContext) {
    a_state_word_stays_on_one_line("dark", cx);
}

#[gpui_kit::test]
fn a_state_word_beside_a_button_stays_on_one_line_in_the_light_theme(cx: &mut TestAppContext) {
    a_state_word_stays_on_one_line("light", cx);
}

#[gpui_kit::test]
fn every_row_of_the_check_has_its_leading_mark(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    open_checks(&shell, cx);
    for id in [
        "check-flottille-provider-claude-mark",
        "check-flottille-provider-codex-mark",
        "check-flottille-sandbox-mark",
    ] {
        assert!(
            present(cx, id),
            "{id} is drawn: no empty slot before a title"
        );
    }
}
