//! The window's side of #1119: the Version section of Settings, General, and the sidebar's foot
//! say the update, Restart to update asks first when agents run, and a window whose daemon came
//! back on another build starts again from the new binary, on the page it showed. On GPUI's test
//! platform in front of a real daemon in temporary folders, its agents the fake ACP agent, its
//! binary a link to the real one in a folder of its own: no window opens on a screen, nothing is
//! written beside the real binary.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::shell::Selection;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// A version above any this build can have.
const NEWER: &str = "99.0.0";

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
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

fn click(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn visible(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some_and(|f| f.visible()))
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// Flottille's binary in a folder of its own (a link to the real one): the daemon started
/// through it stages beside it, never beside the real binary.
struct Installed {
    _folder: tempfile::TempDir,
    path: PathBuf,
}

fn installed() -> Installed {
    let folder = tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let path = folder.path().join("flottille");
    if std::fs::hard_link(env!("CARGO_BIN_EXE_flottille"), &path).is_err() {
        let copied = std::process::Command::new("cp")
            .arg(env!("CARGO_BIN_EXE_flottille"))
            .arg(&path)
            .status()
            .unwrap();
        assert!(copied.success());
    }
    Installed {
        _folder: folder,
        path,
    }
}

/// The daemon of `user` started through `binary`: the window's connections then reach it. It
/// runs from that copy, never from the shared test binary, beside which it would stage.
fn start_through(user: &User, binary: &Path) {
    let output = user
        .command_through(binary, &["status", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    let pid = status["pid"].as_u64().unwrap_or_else(|| panic!("{status}"));
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    assert_eq!(exe, binary.canonicalize().unwrap());
}

/// A build at `NEWER` staged beside `binary`, as a download leaves it.
fn staged_beside(binary: &Path) {
    let staging = binary.with_file_name("flottille.next");
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(staging.join("flottille"), "#!/bin/sh\n").unwrap();
    std::fs::write(
        staging.join("staged.json"),
        json!({"version": NEWER, "sha256": "0", "channel": "stable", "notes": []}).to_string(),
    )
    .unwrap();
}

/// A user whose fake agent works long on "work long", its project `app` added.
fn with_a_long_turn() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "work long", "do": [{"wait": 60000}, {"say": "done"}]},
        {"when": "hello", "do": [{"say": "hi"}]}
    ]}));
    let folder = user.path("work/app");
    user.git_init(&folder);
    user.git(
        &folder,
        &["commit", "--quiet", "--allow-empty", "-m", "start"],
    );
    user
}

fn add_project(user: &User) {
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
}

fn start(user: &User, message: &str) -> String {
    user.call("agent.start", json!({"project": "app", "message": message}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn shown(user: &User) -> Value {
    user.call("update.show", json!({})).unwrap()
}

#[gpui_kit::test]
fn the_version_section_and_the_sidebar_s_foot_say_a_build_staged(cx: &mut TestAppContext) {
    let user = User::new();
    let binary = installed();
    start_through(&user, &binary.path);
    staged_beside(&binary.path);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the window reads the update", |cx| {
        shell.read_with(cx, |s, cx| s.version().read(cx).shown().is_some())
    });
    // The window read it at its connection, before the build was staged: Check now reads again.
    shell.update_in(cx, |shell, window, cx| {
        shell.open_settings_at(flottille_window::settings::Reveal::Version, window, cx)
    });
    settle(cx);
    click(cx, "version-check-now");

    until(&shell, cx, "the build staged is said", |cx| {
        label(cx, "version-state-text").as_deref() == Some(flottille_window::version::READY)
    });
    let this = shown(&user)["version"].as_str().unwrap().to_owned();
    assert!(
        label(cx, "version-build-text").is_some_and(|said| said.starts_with(&this)),
        "{:?}",
        label(cx, "version-build-text")
    );
    assert!(visible(cx, "version-restart-button"));
    until(&shell, cx, "the sidebar's foot says it", |cx| {
        visible(cx, "sidebar-version")
            && label(cx, "sidebar-version").as_deref() == Some("Ready to restart")
    });
}

#[test]
fn the_download_s_progress_moves_its_amount_not_its_lines() {
    let at = |done: u64| {
        flottille_window::version::Shown::of(&json!({
            "latest": NEWER,
            "downloading": {"done": done, "total": 40_000_000},
        }))
    };
    let (start, later) = (at(1_000_000), at(20_000_000));
    // The sidebar's foot says it once, not every amount.
    assert_eq!(start.foot_line().as_deref(), Some("Downloading…"));
    assert_eq!(later.foot_line(), start.foot_line());
    // The section's line says the state alone; the amount is beside the bar.
    assert_eq!(start.state_line(), "Downloading…");
    assert_eq!(later.state_line(), start.state_line());
    assert_eq!(start.amount_line().as_deref(), Some("1.0 MB of 40.0 MB"));
    assert_eq!(later.amount_line().as_deref(), Some("20.0 MB of 40.0 MB"));
    // The share received, when the server said the size.
    assert_eq!(later.share(), Some(0.5));
}

#[test]
fn a_download_of_unknown_size_says_the_bytes_and_no_share() {
    let shown = flottille_window::version::Shown::of(&json!({
        "latest": NEWER,
        "downloading": {"done": 5_000_000, "total": null},
    }));
    assert_eq!(shown.amount_line().as_deref(), Some("5.0 MB"));
    assert_eq!(shown.share(), None);
    let none = flottille_window::version::Shown::of(&json!({"latest": NEWER}));
    assert_eq!(none.amount_line(), None);
}

#[gpui_kit::test]
fn restart_to_update_with_agents_running_asks_first(cx: &mut TestAppContext) {
    let user = with_a_long_turn();
    let binary = installed();
    start_through(&user, &binary.path);
    add_project(&user);
    staged_beside(&binary.path);
    let agent = start(&user, "work long");
    let deadline = Instant::now() + Duration::from_secs(20);
    while user.status()["agents"]["running"] != 1 {
        assert!(Instant::now() < deadline, "the agent never ran");
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid = user.status()["pid"].clone();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session runs in the window", |cx| {
        shell.read_with(cx, |s, _| {
            s.snapshot()
                .sessions
                .iter()
                .any(|session| session.id == agent && session.mark.is_some())
        })
    });
    shell.update_in(cx, |shell, window, cx| {
        shell.open_settings_at(flottille_window::settings::Reveal::Version, window, cx)
    });
    until(&shell, cx, "Restart to update is offered", |cx| {
        visible(cx, "version-restart-button")
    });

    click(cx, "version-restart-button");

    let section = shell.read_with(cx, |s, _| s.version().clone());
    // The daemon is asked which sessions a restart stops first.
    until(&shell, cx, "the restart asks first", |cx| {
        section.read_with(cx, |section, _| section.is_confirming())
    });
    settle(cx);
    assert!(
        visible(cx, "version-restart"),
        "the one confirmation dialog"
    );
    assert_eq!(
        section.read_with(cx, |section, _| section.stopping().len()),
        1,
        "it says which sessions stop"
    );
    assert!(visible(cx, "version-restart-session-0") || visible(cx, "version-restart-sessions"));
    // Nothing is asked of the daemon before the user answers.
    assert_eq!(shown(&user)["restart"], Value::Null);
    assert_eq!(user.status()["pid"], pid);

    click(cx, "version-restart-idle");

    until(&shell, cx, "the restart waits for no agent to run", |cx| {
        label(cx, "version-state-text").is_some_and(|said| said.contains("once no agent runs"))
    });
    assert_eq!(shown(&user)["restart"], "idle");
    assert_eq!(
        user.status()["pid"],
        pid,
        "the daemon runs on while the agent works"
    );
    assert!(!section.read_with(cx, |section, _| section.is_confirming()));
}

#[gpui_kit::test]
fn restart_to_update_asks_while_a_session_only_waits(cx: &mut TestAppContext) {
    let user = with_a_long_turn();
    let binary = installed();
    start_through(&user, &binary.path);
    add_project(&user);
    staged_beside(&binary.path);
    // Its turn ended: no agent works, but its process lives on, and a restart stops it.
    let agent = start(&user, "hello");
    let deadline = Instant::now() + Duration::from_secs(20);
    while user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"] != "idle" {
        assert!(Instant::now() < deadline, "the turn never ended");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(user.status()["agents"]["running"], 0);
    let pid = user.status()["pid"].clone();
    let (shell, cx) = window(&user, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.open_settings_at(flottille_window::settings::Reveal::Version, window, cx)
    });
    until(&shell, cx, "Restart to update is offered", |cx| {
        visible(cx, "version-restart-button")
    });

    click(cx, "version-restart-button");

    let section = shell.read_with(cx, |s, _| s.version().clone());
    until(&shell, cx, "the restart asks first", |cx| {
        section.read_with(cx, |section, _| section.is_confirming())
    });
    assert_eq!(
        section.read_with(cx, |section, _| section.stopping().len()),
        1,
        "it says which sessions stop"
    );
    assert_eq!(shown(&user)["restart"], Value::Null);
    assert_eq!(
        user.status()["pid"],
        pid,
        "nothing restarted before the answer"
    );
}

#[gpui_kit::test]
fn the_window_comes_back_on_its_page_after_the_update(
    cx: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let user = with_a_long_turn();
    add_project(&user);
    let agent = start(&user, "hello");
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the session is listed", |cx| {
        shell.read_with(cx, |s, _| {
            s.snapshot()
                .sessions
                .iter()
                .any(|session| session.id == agent)
        })
    });
    shell.update_in(cx, |shell, window, cx| {
        shell.open(Selection::Session(agent.clone()), window, cx)
    });
    settle(cx);
    let asked: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let asking = asked.clone();
    // This window says another build than the daemon's: the daemon started again from a new one.
    shell.update(cx, |shell, _| {
        shell.set_build(
            "0.0.0 (test, 000000000000)".to_owned(),
            Arc::new(move |page| {
                asking.lock().unwrap().push(page);
                true
            }),
        )
    });

    // Anything the daemon says of the update shows its build.
    user.call("update.set", json!({"check": "never"})).unwrap();

    until(&shell, cx, "the window asks to start again", |_| {
        !asked.lock().unwrap().is_empty()
    });
    settle(cx);
    let page = asked.lock().unwrap().clone();
    assert_eq!(
        page,
        [Some(format!("session:{agent}"))],
        "once, with its page"
    );

    // The window started again from the new binary opens that page.
    let (again, cx) = window(&user, second);
    again.update(cx, |shell, _| shell.reopen(page[0].as_deref().unwrap()));
    until(&again, cx, "the page shown before", |cx| {
        again.read_with(cx, |s, _| {
            s.selected() == Some(&Selection::Session(agent.clone()))
        })
    });
}

#[gpui_kit::test]
fn a_window_of_the_daemon_s_build_does_not_start_again(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = window(&user, cx);
    let full = shown(&user)["full"].as_str().unwrap().to_owned();
    let asked: Arc<Mutex<usize>> = Arc::default();
    let asking = asked.clone();
    shell.update(cx, |shell, _| {
        shell.set_build(
            full,
            Arc::new(move |_| {
                *asking.lock().unwrap() += 1;
                true
            }),
        )
    });

    user.call("update.set", json!({"check": "never"})).unwrap();
    until(&shell, cx, "the window reads the update", |cx| {
        shell.read_with(cx, |s, cx| {
            s.version()
                .read(cx)
                .shown()
                .is_some_and(|shown| !shown.automatically)
        })
    });

    assert_eq!(*asked.lock().unwrap(), 0);
}

// ── The download's bar ─────────────────────────────────────────────────────────────────────────

/// The Version section of Settings open on a window whose daemon has read the update.
fn version_open<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the window reads the update", |cx| {
        shell.read_with(cx, |s, cx| s.version().read(cx).shown().is_some())
    });
    shell.update_in(cx, |shell, window, cx| {
        shell.open_settings_at(flottille_window::settings::Reveal::Version, window, cx)
    });
    settle(cx);
    (shell, cx)
}

/// What the daemon says while it downloads `done` bytes of `total`.
fn downloading(done: u64, total: Option<u64>) -> flottille_window::version::Shown {
    flottille_window::version::Shown::of(&json!({
        "latest": NEWER,
        "downloading": {"done": done, "total": total},
    }))
}

fn say(shell: &Entity<Shell>, shown: flottille_window::version::Shown, cx: &mut VisualTestContext) {
    shell.update(cx, |shell, cx| {
        shell
            .version()
            .update(cx, |section, cx| section.set_shown(shown, cx))
    });
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn advance(cx: &mut VisualTestContext, by: Duration) {
    let mut gone = Duration::ZERO;
    while gone < by {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += Duration::from_millis(16);
    }
}

fn width(cx: &mut VisualTestContext, id: &str) -> Option<f32> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .map(|found| f32::from(found.bounds().size.width))
    })
}

fn left(cx: &mut VisualTestContext, id: &str) -> f32 {
    cx.update(|window, _| f32::from(window.find(id.to_owned()).bounds().origin.x))
}

/// How much of the bar `bar` is filled, 0 to 1.
fn filled(cx: &mut VisualTestContext, bar: &str) -> f32 {
    let track = width(cx, bar).expect("the bar is drawn");
    width(cx, &format!("{bar}-fill")).expect("its fill is drawn") / track
}

#[gpui_kit::test]
fn a_download_shows_a_bar_that_fills(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = version_open(&user, cx);
    assert!(!visible(cx, "version-bar-1"), "no bar before a download");

    say(&shell, downloading(20_000_000, Some(40_000_000)), cx);
    advance(cx, Duration::from_millis(600));
    assert!(visible(cx, "version-bar-1"));
    assert!((filled(cx, "version-bar-1") - 0.5).abs() < 0.01);
    assert_eq!(
        label(cx, "version-download-amount").as_deref(),
        Some("20.0 MB of 40.0 MB")
    );
    assert!(visible(cx, "version-download-cancel"));

    // Between two readings the fill moves smoothly, and never back.
    say(&shell, downloading(30_000_000, Some(40_000_000)), cx);
    advance(cx, Duration::from_millis(100));
    let mid = filled(cx, "version-bar-1");
    assert!(mid > 0.5 && mid < 0.75, "on its way: {mid}");
    say(&shell, downloading(25_000_000, Some(40_000_000)), cx);
    advance(cx, Duration::from_millis(600));
    assert!(
        (filled(cx, "version-bar-1") - 0.75).abs() < 0.01,
        "a lower reading does not take it back"
    );
    assert_eq!(
        label(cx, "version-download-amount").as_deref(),
        Some("25.0 MB of 40.0 MB"),
        "the amount is the daemon's last word"
    );
}

#[gpui_kit::test]
fn a_download_of_unknown_size_shows_a_moving_bar(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = version_open(&user, cx);

    say(&shell, downloading(5_000_000, None), cx);
    advance(cx, Duration::from_millis(600));

    assert!(visible(cx, "version-bar-1"));
    assert_eq!(
        label(cx, "version-download-amount").as_deref(),
        Some("5.0 MB"),
        "the bytes received, no total"
    );
    let track = width(cx, "version-bar-1").unwrap();
    let stripe = width(cx, "version-bar-1-fill").unwrap();
    assert!(stripe > 0. && stripe < track, "a stripe, not a fill");
    let first = left(cx, "version-bar-1-fill");
    advance(cx, Duration::from_millis(300));
    assert_ne!(left(cx, "version-bar-1-fill"), first, "the stripe moves");
}

#[gpui_kit::test]
fn a_finished_download_folds_the_bar_and_offers_restart(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = version_open(&user, cx);
    say(&shell, downloading(20_000_000, Some(40_000_000)), cx);
    advance(cx, Duration::from_millis(600));

    let done = flottille_window::version::Shown::of(&json!({"latest": NEWER, "staged": NEWER}));
    say(&shell, done, cx);
    // The bar fills before it folds away.
    advance(cx, Duration::from_millis(100));
    assert!(visible(cx, "version-bar-1"));
    advance(cx, Duration::from_millis(200));
    assert!(
        filled(cx, "version-bar-1") > 0.95,
        "the bar fills: {}",
        filled(cx, "version-bar-1")
    );
    advance(cx, Duration::from_millis(1000));

    assert!(!visible(cx, "version-bar-1"), "the bar folded away");
    assert!(!visible(cx, "version-download-cancel"));
    assert_eq!(
        label(cx, "version-state-text").as_deref(),
        Some(flottille_window::version::READY)
    );
    assert!(visible(cx, "version-restart-button"));
}

#[gpui_kit::test]
fn a_failed_download_folds_the_bar_and_says_why(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = version_open(&user, cx);
    say(&shell, downloading(20_000_000, Some(40_000_000)), cx);
    advance(cx, Duration::from_millis(600));

    let failed = flottille_window::version::Shown::of(
        &json!({"latest": NEWER, "download_failed": "cannot read the archive"}),
    );
    say(&shell, failed, cx);
    advance(cx, Duration::from_millis(1000));

    assert!(!visible(cx, "version-bar-1"));
    assert_eq!(
        label(cx, "version-message").as_deref(),
        Some("cannot read the archive")
    );
    assert!(visible(cx, "version-download"), "it can be tried again");
}

#[gpui_kit::test]
fn cancel_stops_the_download(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = version_open(&user, cx);
    say(&shell, downloading(20_000_000, Some(40_000_000)), cx);
    advance(cx, Duration::from_millis(600));
    assert!(visible(cx, "version-download-cancel"));

    // The daemon, which downloads nothing here, answers that it does not: the bar goes.
    click(cx, "version-download-cancel");
    until(&shell, cx, "the bar folds away", |cx| {
        advance(cx, Duration::from_millis(300));
        !visible(cx, "version-bar-1")
    });

    assert!(!visible(cx, "version-download-cancel"));
    assert_eq!(label(cx, "version-message"), None, "a cancel is no failure");
}

#[gpui_kit::test]
fn a_second_download_starts_from_nothing(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = version_open(&user, cx);
    say(&shell, downloading(36_000_000, Some(40_000_000)), cx);
    advance(cx, Duration::from_millis(600));
    say(
        &shell,
        flottille_window::version::Shown::of(&json!({"latest": NEWER})),
        cx,
    );
    advance(cx, Duration::from_millis(1000));
    assert!(!visible(cx, "version-bar-1"));

    say(&shell, downloading(2_000_000, Some(40_000_000)), cx);
    advance(cx, Duration::from_millis(600));

    assert!(
        (filled(cx, "version-bar-2") - 0.05).abs() < 0.01,
        "not the 90 % of the one before: {}",
        filled(cx, "version-bar-2")
    );
}

#[gpui_kit::test]
fn the_what_s_new_row_is_a_link_aligned_with_the_other_values(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = version_open(&user, cx);
    shell.update(cx, |shell, cx| {
        shell.version().update(cx, |section, cx| {
            section.set_whats_new(Some("0.4.0-beta.133".into()), cx)
        })
    });
    advance(cx, Duration::from_millis(600));

    assert_eq!(
        label(cx, "version-whats-new-open").as_deref(),
        Some("Since 0.4.0-beta.133")
    );
    assert_eq!(
        left(cx, "version-whats-new-open"),
        left(cx, "version-state-text"),
        "it starts where the other values do, not indented like a button"
    );
}

fn selected(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .is_some_and(|found| found.selected())
    })
}

/// A user whose last `flottille update --channel` chose `kept`, as the state folder says it.
fn remembering(kept: &str) -> User {
    let user = User::new();
    let file = user.path("state/flottille/update-channel.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, kept).unwrap();
    user
}

#[gpui_kit::test]
fn choosing_beta_remembers_it_and_checks_for_an_update(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = version_open(&user, cx);
    assert!(selected(cx, "version-channel-stable"));
    assert!(!selected(cx, "version-channel-beta"));

    click(cx, "version-channel-beta");

    // The daemon remembers it, as `--channel` does, and checks at once: nothing listens where the
    // test's releases are, so the check says it failed on the Update row.
    until(&shell, cx, "the channel is chosen and checked", |cx| {
        shell.read_with(cx, |s, cx| {
            s.version()
                .read(cx)
                .shown()
                .is_some_and(|shown| shown.channel == "beta" && shown.failed.is_some())
        })
    });
    assert_eq!(shown(&user)["channel"], "beta");
    let kept = std::fs::read_to_string(user.path("state/flottille/update-channel.json")).unwrap();
    assert!(kept.contains("beta"), "{kept}");
    settle(cx);
    assert!(selected(cx, "version-channel-beta"));
    assert!(!selected(cx, "version-channel-stable"));
    assert_eq!(
        label(cx, "version-state-text").as_deref(),
        Some("Could not check for an update")
    );
}

#[gpui_kit::test]
fn back_to_stable_keeps_a_newer_beta_until_a_stable_passes_it(cx: &mut TestAppContext) {
    let user = User::new();
    let (shell, cx) = version_open(&user, cx);

    say(
        &shell,
        flottille_window::version::Shown::of(&json!({"channel": "stable", "ahead_of": "0.4.0"})),
        cx,
    );
    advance(cx, Duration::from_millis(400));

    // Neither an update nor "up to date": the build stays, and the row says why.
    assert_eq!(
        label(cx, "version-state-text").as_deref(),
        Some("This build is newer than stable 0.4.0: it stays until a newer one is out")
    );
    assert!(!visible(cx, "version-download"));

    say(
        &shell,
        flottille_window::version::Shown::of(&json!({"channel": "stable", "latest": "0.5.0"})),
        cx,
    );
    advance(cx, Duration::from_millis(400));

    assert_eq!(
        label(cx, "version-state-text").as_deref(),
        Some("0.5.0 is available")
    );
    assert!(visible(cx, "version-download"));
}

#[gpui_kit::test]
fn an_alpha_build_shows_its_branch_and_offers_stable_or_beta(cx: &mut TestAppContext) {
    let user = remembering(r#"{"channel":"alpha","branch":"feat/x"}"#);
    let (shell, cx) = version_open(&user, cx);
    advance(cx, Duration::from_millis(400));

    // The command line's choice is said, never offered: no segment for it.
    assert_eq!(
        label(cx, "version-channel-other").as_deref(),
        Some("alpha · feat/x")
    );
    assert!(!visible(cx, "version-channel"));
    assert!(visible(cx, "version-channel-stable"));
    assert!(visible(cx, "version-channel-beta"));

    click(cx, "version-channel-stable");
    until(&shell, cx, "the way back", |cx| {
        shell.read_with(cx, |s, cx| {
            s.version()
                .read(cx)
                .shown()
                .is_some_and(|shown| shown.channel == "stable")
        })
    });
    advance(cx, Duration::from_millis(600));

    assert!(visible(cx, "version-channel"));
    assert!(selected(cx, "version-channel-stable"));
    assert!(!visible(cx, "version-channel-other"));
    let kept = std::fs::read_to_string(user.path("state/flottille/update-channel.json")).unwrap();
    assert!(
        !kept.contains("feat/x"),
        "the branch goes with the alpha: {kept}"
    );
}
