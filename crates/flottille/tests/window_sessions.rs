//! The scenarios of ending inactive sessions in the window (`docs/design.md` §5.7), on GPUI's
//! test platform in front of a real daemon, with a throwaway user. The setting is off by default,
//! its days field unfolds when it is switched on, and every movement is animated. No window opens
//! on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::settings::SettingsPage;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let connect: Connect = Arc::new(user.connector());
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1600.)));
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the links sent, then draws what changed.
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

/// Lets every motion end.
fn settle(cx: &mut VisualTestContext) {
    run_for(Duration::from_millis(1_440), cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn run_for(by: Duration, cx: &mut VisualTestContext) {
    let step = Duration::from_millis(16);
    let mut gone = Duration::ZERO;
    while gone < by {
        cx.executor().advance_clock(step);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += step;
    }
}

/// Opens Settings on its Sessions tab, once the page knows the setting.
fn open_sessions(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SettingsPage> {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click("settings-tabs-sessions", cx));
    settle(cx);
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap();
    until(shell, cx, "the page knows the setting", |cx| {
        page.read_with(cx, |p, _| p.sessions_known())
    });
    settle(cx);
    page
}

fn click(id: &str, cx: &mut VisualTestContext) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
}

/// The height the days field takes once a frame is drawn; `None` when it is not drawn at all.
fn days_height(cx: &mut VisualTestContext) -> Option<f32> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        let field = window.try_find("sessions-days-block")?;
        Some(f32::from(field.bounds().size.height))
    })
}

#[gpui_kit::test]
fn ending_inactive_sessions_is_off_by_default_and_its_days_unfold_when_it_is_on(
    cx: &mut TestAppContext,
) {
    let user = User::new();
    let (shell, cx) = window(&user, cx);
    let page = open_sessions(&shell, cx);

    assert_eq!(page.read_with(cx, |p, _| p.sessions_said()), Some(None));
    assert_eq!(days_height(cx), None, "off: the days field is not drawn");
    assert!(!user.path("config/flottille/sessions.toml").exists());

    // On: 7 days, the field grows into place, and the page rests.
    click("sessions-switch", cx);
    let first = days_height(cx).expect("drawn from the first frame");
    run_for(Duration::from_millis(60), cx);
    let part_way = days_height(cx).unwrap();
    run_for(Duration::from_millis(600), cx);
    let open = days_height(cx).unwrap();
    assert!(
        first < part_way && part_way < open,
        "it unfolds: {first} then {part_way} then {open}"
    );
    until(&shell, cx, "the daemon says 7", |cx| {
        page.read_with(cx, |p, _| p.sessions_said() == Some(Some(7)))
    });
    assert_eq!(
        user.call("sessions.show", json!({})).unwrap()["days"],
        7,
        "7 when first turned on"
    );
    settle(cx);
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);

    // The days are chosen in the field.
    click("sessions-days", cx);
    settle(cx);
    click("sessions-days-30", cx);
    until(&shell, cx, "the daemon says 30", |cx| {
        page.read_with(cx, |p, _| p.sessions_said() == Some(Some(30)))
    });
    assert_eq!(user.call("sessions.show", json!({})).unwrap()["days"], 30);
    settle(cx);

    // Off: the field folds away, still drawn part way, gone once the fold ended.
    click("sessions-switch", cx);
    run_for(Duration::from_millis(60), cx);
    let leaving = days_height(cx).expect("still drawn, leaving");
    assert!(leaving > 0. && leaving < open + 1., "it shrinks: {leaving}");
    until(&shell, cx, "the daemon says off", |cx| {
        page.read_with(cx, |p, _| p.sessions_said() == Some(None))
    });
    run_for(Duration::from_millis(600), cx);
    assert_eq!(days_height(cx), None, "gone once the fold ended");
    assert_eq!(
        user.call("sessions.show", json!({})).unwrap()["days"],
        "off"
    );
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

#[gpui_kit::test]
fn the_setting_chosen_from_elsewhere_is_followed_by_the_page(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = window(&user, cx);
    let page = open_sessions(&shell, cx);

    user.call("sessions.set", json!({"days": 14})).unwrap();

    until(&shell, cx, "the page shows 14", |cx| {
        page.read_with(cx, |p, _| p.sessions_said() == Some(Some(14)))
    });
    settle(cx);
    assert!(
        days_height(cx).is_some_and(|h| h > 0.),
        "the field unfolded"
    );
}
