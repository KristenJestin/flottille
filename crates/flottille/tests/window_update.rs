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

/// The daemon of `user` started through `binary`: the window's connections then reach it.
fn start_through(user: &User, binary: &Path) {
    let output = user
        .command_through(binary, &["status", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
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
    // The section says the amount, in place: what fades is the state, not each amount.
    assert_eq!(start.state_line(), "Downloading… 1.0 MB of 40.0 MB");
    assert_eq!(later.state_line(), "Downloading… 20.0 MB of 40.0 MB");
    assert_eq!(start.state_said(), "Downloading…");
    assert_eq!(later.state_said(), start.state_said());
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
