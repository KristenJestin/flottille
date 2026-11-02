//! The scenarios of #279 (the user chooses the window's theme: Omarchy, or Flottille light or
//! dark), on GPUI's test platform in front of a real daemon, with a throwaway user whose desktop
//! is described by temporary files: an os-release and an invented Omarchy theme in its own state
//! folder, never the machine's. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::Palette;
use flottille_ui::color::hex;
use flottille_ui::omarchy::{self, Environment};
use flottille_ui::theme::palette;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::settings::SettingsPage;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// An invented dark Omarchy theme.
const THEME: &str = "background = \"#0e1116\"\nforeground = \"#c9d1d9\"\naccent = \"#f0883e\"\n";

/// A user on an Omarchy desktop with [`THEME`] as its current theme, or on another desktop.
fn user(omarchy: bool) -> User {
    let user = User::new();
    let id = if omarchy { "omarchy" } else { "test" };
    std::fs::write(user.path("os-release"), format!("NAME=Test\nID={id}\n")).unwrap();
    if omarchy {
        write_theme(&user, THEME);
    }
    user
}

fn write_theme(user: &User, text: &str) {
    let path = env_of(user).colors_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    // A later modification time, whatever the filesystem's resolution.
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
}

fn env_of(user: &User) -> Environment {
    Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    }
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    cx.update(|cx| flottille_ui::init_with(env_of(user), cx));
    let connect: Connect = Arc::new(user.connector());
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

/// Lets every motion end: the palette's cross-fade, the sections' entrances.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Opens Settings on its Appearance tab.
fn open_appearance(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SettingsPage> {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click("settings-tabs-appearance", cx));
    settle(cx);
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap();
    until(shell, cx, "the appearance is read", |cx| {
        page.read_with(cx, |p, _| p.appearance_known())
    });
    settle(cx);
    page
}

fn click(id: &str, cx: &mut VisualTestContext) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
}

fn selected(id: &str, cx: &mut VisualTestContext) -> Option<bool> {
    let id = id.to_owned();
    cx.update(|window, _| window.find(id).selected())
}

fn shown(id: &str, cx: &mut VisualTestContext) -> bool {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).is_some_and(|found| found.visible()))
}

fn stored(user: &User) -> serde_json::Value {
    user.call("appearance.show", json!({})).unwrap()
}

#[gpui_kit::test]
fn choosing_flottille_light_changes_the_drawn_background_and_ink(cx: &mut TestAppContext) {
    let user = user(true);
    let (shell, cx) = window(&user, cx);
    open_appearance(&shell, cx);
    assert_eq!(selected("appearance-theme-omarchy", cx), Some(true));
    assert_eq!(cx.update(|_, cx| palette(cx)).acc, hex(0xf0883e));
    assert!(
        !shown("appearance-shade", cx),
        "Light and Dark are Flottille's"
    );

    click("appearance-theme-flottille", cx);
    until(&shell, cx, "Flottille is kept", |_| {
        stored(&user) == json!({"theme": "flottille", "shade": "dark"})
    });
    settle(cx);
    assert_eq!(cx.update(|_, cx| palette(cx)), Palette::base());
    assert!(shown("appearance-shade", cx), "the shade unfolds");
    assert_eq!(selected("appearance-shade-dark", cx), Some(true));

    click("appearance-shade-light", cx);
    until(&shell, cx, "the light shade is kept", |_| {
        stored(&user) == json!({"theme": "flottille", "shade": "light"})
    });
    // The colours cross-fade: partway they are neither palette.
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(Duration::from_millis(120));
    let partway = cx.update(|_, cx| palette(cx));
    assert!(
        partway != Palette::base() && partway != Palette::light(),
        "no jump"
    );
    settle(cx);
    let p = cx.update(|_, cx| palette(cx));
    assert_eq!(p, Palette::light());
    assert!(p.bg.l > 0.9 && p.text.l < 0.2, "a light page, a dark ink");
    assert_eq!(selected("appearance-shade-light", cx), Some(true));
}

#[gpui_kit::test]
fn choosing_omarchy_with_a_test_theme_folder_follows_it(cx: &mut TestAppContext) {
    let user = user(true);
    user.call(
        "appearance.set",
        json!({"theme": "flottille", "shade": "light"}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    open_appearance(&shell, cx);
    assert_eq!(cx.update(|_, cx| palette(cx)), Palette::light());
    assert_eq!(selected("appearance-theme-flottille", cx), Some(true));

    click("appearance-theme-omarchy", cx);
    until(&shell, cx, "Omarchy is kept", |_| {
        stored(&user)["theme"] == "omarchy"
    });
    settle(cx);
    assert_eq!(cx.update(|_, cx| palette(cx)).acc, hex(0xf0883e));
    assert!(
        !shown("appearance-shade", cx),
        "the shade folds away with Omarchy"
    );

    // The desktop switches theme: the window follows it.
    write_theme(&user, &THEME.replace("#f0883e", "#7aa2f7"));
    cx.executor()
        .advance_clock(omarchy::POLL + Duration::from_millis(10));
    cx.run_until_parked();
    settle(cx);
    assert_eq!(cx.update(|_, cx| palette(cx)).acc, hex(0x7aa2f7));
}

#[gpui_kit::test]
fn off_omarchy_the_omarchy_choice_is_disabled_and_says_why(cx: &mut TestAppContext) {
    let user = user(false);
    let (shell, cx) = window(&user, cx);
    open_appearance(&shell, cx);
    assert_eq!(
        selected("appearance-theme-flottille", cx),
        Some(true),
        "the window uses Flottille"
    );
    assert!(shown("appearance-theme-omarchy", cx));
    assert!(shown("appearance-omarchy-why", cx), "one line says why");
    let why = cx.update(|window, _| {
        window
            .find("appearance-omarchy-why")
            .label()
            .map(str::to_owned)
    });
    assert!(
        why.is_some_and(|why| why.contains("Omarchy")),
        "it names what is missing"
    );

    click("appearance-theme-omarchy", cx);
    settle(cx);
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    assert!(
        !user.path("config/flottille/appearance.toml").exists(),
        "nothing was asked of the daemon"
    );
    assert_eq!(selected("appearance-theme-flottille", cx), Some(true));
}

#[gpui_kit::test]
fn the_choice_survives_a_new_window_read_back_from_the_daemon(cx: &mut TestAppContext) {
    let user = user(true);
    // Chosen by an earlier window: this one opens on it, before Settings is ever opened.
    user.call(
        "appearance.set",
        json!({"theme": "flottille", "shade": "light"}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the appearance is read", |cx| {
        cx.update(|_, cx| omarchy::shown(cx)).source == omarchy::Source::Flottille
    });
    settle(cx);
    assert_eq!(cx.update(|_, cx| palette(cx)), Palette::light());

    // Another client changes it: the window follows.
    user.call(
        "appearance.set",
        json!({"theme": "omarchy", "shade": "light"}),
    )
    .unwrap();
    until(&shell, cx, "the change is followed", |cx| {
        cx.update(|_, cx| omarchy::shown(cx)).source == omarchy::Source::Omarchy
    });
    settle(cx);
    assert_eq!(cx.update(|_, cx| palette(cx)).acc, hex(0xf0883e));
}
