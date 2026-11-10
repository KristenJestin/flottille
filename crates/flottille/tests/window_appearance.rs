//! The scenarios of #279 (the user chooses the window's theme: Omarchy, or Flottille light or
//! dark), on GPUI's test platform in front of a real daemon, with a throwaway user whose desktop
//! is described by temporary files: an os-release and an invented Omarchy theme in its own state
//! folder, never the machine's. No window opens on a screen.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
    let connect: Connect = Arc::new(user.connector());
    window_through(user, connect, cx)
}

/// A window whose every connection to the daemon is made through `connect`.
fn window_through<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    cx.update(|cx| flottille_window::init_with(env_of(user), cx));
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

/// `connect` as a window's connection, and the method of every request sent through it, in order:
/// what a test asserts on is the method a click would call, not how many requests the window
/// makes on its own.
fn tapped(
    connect: impl Fn() -> std::io::Result<UnixStream> + Send + Sync + 'static,
) -> (Connect, Arc<Mutex<Vec<String>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let record = asked.clone();
    let connect: Connect = Arc::new(move || {
        let (window, relay) = UnixStream::pair()?;
        let daemon = connect()?;
        let answers = (relay.try_clone()?, daemon.try_clone()?);
        std::thread::spawn(move || {
            let (mut to_window, mut from_daemon) = answers;
            let _ = std::io::copy(&mut from_daemon, &mut to_window);
        });
        let record = record.clone();
        std::thread::spawn(move || {
            let mut to_daemon = daemon;
            for line in BufReader::new(relay).lines().map_while(Result::ok) {
                if let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) {
                    let method = request["method"].as_str().unwrap_or_default();
                    record.lock().unwrap().push(method.to_owned());
                }
                if writeln!(to_daemon, "{line}").is_err() {
                    break;
                }
            }
        });
        Ok(window)
    });
    (connect, asked)
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
    // Flottille is already kept: a request for Omarchy would be a change.
    let kept = json!({"theme": "flottille", "shade": "light"});
    user.call("appearance.set", kept.clone()).unwrap();
    let (connect, asked) = tapped(user.connector());
    let own = connect.clone();
    let (shell, cx) = window_through(&user, connect, cx);
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

    // A request of the window's own (a refresh, a poll) may land at any moment: it is not
    // what is asserted, only that no request sets the appearance.
    let mut refresh = own().unwrap();
    writeln!(
        refresh,
        r#"{{"id": "1", "method": "appearance.show", "params": {{}}}}"#
    )
    .unwrap();
    click("appearance-theme-omarchy", cx);
    settle(cx);
    // A request would reach the daemon within milliseconds: the window is given the time of
    // many, polling as it does, before it is said to have asked nothing.
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(25));
    }
    let methods = asked.lock().unwrap().clone();
    assert!(
        methods.iter().any(|method| method == "appearance.show"),
        "the tap sees the requests that go through it: {methods:?}"
    );
    assert!(
        methods.iter().all(|method| method != "appearance.set"),
        "the choice asked nothing of the daemon to set the appearance: {methods:?}"
    );
    assert_eq!(stored(&user), kept, "nothing was kept");
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
        cx.update(|_, cx| omarchy::shown(cx)).source == omarchy::Source::Own
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

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

fn label_of(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

#[gpui_kit::test]
fn an_appearance_the_daemon_cannot_read_is_said_in_one_line_and_a_choice_writes_it_again(
    cx: &mut TestAppContext,
) {
    let user = user(false);
    let file = user.path("config/flottille/appearance.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "theme = [\n").unwrap();
    let (shell, cx) = window(&user, cx);
    open_appearance(&shell, cx);

    let line = label_of("appearance-unread", cx).expect("one line says it could not be read");
    assert!(
        line.contains("could not be read") && line.contains("appearance.toml"),
        "it gives the daemon's message: {line}"
    );
    assert!(
        user.call("appearance.show", json!({})).is_err(),
        "the daemon still cannot read it"
    );

    // The tab stays usable: a choice writes the file again.
    click("appearance-shade-light", cx);
    until(&shell, cx, "the choice is kept", |_| {
        stored_ok(&user) == Some(json!({"theme": "flottille", "shade": "light"}))
    });
    // It folds away: the clock moves along while the answer is waited for.
    until(&shell, cx, "the line goes once it is read", |cx| {
        cx.executor().advance_clock(Duration::from_millis(16));
        !shown("appearance-unread", cx)
    });
    settle(cx);
    assert_eq!(selected("appearance-shade-light", cx), Some(true));
}

fn stored_ok(user: &User) -> Option<serde_json::Value> {
    user.call("appearance.show", json!({})).ok()
}

/// The folder `path` cannot be written in until this is dropped.
struct Locked(std::path::PathBuf);

impl Locked {
    fn new(path: std::path::PathBuf) -> Locked {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
        Locked(path)
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

#[gpui_kit::test]
fn a_choice_the_daemon_refuses_says_so_in_a_toast_and_shows_the_kept_appearance_again(
    cx: &mut TestAppContext,
) {
    let user = user(false);
    let (shell, cx) = window(&user, cx);
    open_appearance(&shell, cx);
    assert_eq!(selected("appearance-shade-dark", cx), Some(true));

    // The daemon cannot write its file: it refuses the choice.
    let _locked = Locked::new(user.path("config/flottille"));
    click("appearance-shade-light", cx);
    until(&shell, cx, "a toast says it was not kept", |cx| {
        toasts(&shell, cx) == ["The appearance was not kept"]
    });
    until(&shell, cx, "the kept appearance is shown again", |cx| {
        cx.update(|_, cx| omarchy::shown(cx)).shade == flottille_ui::theme::Shade::Dark
    });
    settle(cx);
    assert_eq!(selected("appearance-shade-dark", cx), Some(true));
    assert_eq!(cx.update(|_, cx| palette(cx)), Palette::base());
}

#[gpui_kit::test]
fn two_quick_choices_end_on_the_last_one_in_the_window_and_in_the_daemon(cx: &mut TestAppContext) {
    let user = user(false);
    // The first connection made after the clicks is slow: the answer to the first choice would
    // come after the second one was made.
    let slow = Arc::new(AtomicBool::new(false));
    let late = slow.clone();
    let through = user.connector();
    let connect: Connect = Arc::new(move || {
        if late.swap(false, Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(600));
        }
        through()
    });
    let (shell, cx) = window_through(&user, connect, cx);
    open_appearance(&shell, cx);

    slow.store(true, Ordering::SeqCst);
    click("appearance-shade-light", cx);
    click("appearance-shade-dark", cx);
    let dark = flottille_ui::theme::Shade::Dark;
    let until_late = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until_late {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| omarchy::shown(cx)).shade,
            dark,
            "a late answer to the first choice does not take the window back to it"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    until(&shell, cx, "the last choice is kept", |_| {
        stored(&user) == json!({"theme": "flottille", "shade": "dark"})
    });
    settle(cx);
    assert_eq!(cx.update(|_, cx| palette(cx)), Palette::base());
    assert_eq!(selected("appearance-shade-dark", cx), Some(true));
}
