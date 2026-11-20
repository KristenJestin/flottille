//! The scenarios of #1120: after an update the window says, quietly, that there is something new
//! (a mark with a dot at the foot of the sidebar, never a page that opens by itself), the page
//! lists every change since the version last run, and it is found again in Settings, General,
//! Version. On GPUI's test platform in front of a real daemon in temporary folders: no window
//! opens on a screen and nothing reaches the network. The daemon's memory of the version last
//! run (`whats-new.json` of the state folder) is written by the test, as a daemon that ran an
//! older build left it.

use crate::common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// The version the user last ran.
const SINCE: &str = "0.4.0-beta.103";

/// This build's version, which the daemon runs as.
fn this_version() -> String {
    let said = std::process::Command::new(env!("CARGO_BIN_EXE_flottille"))
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8_lossy(&said.stdout)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned()
}

/// What a daemon that ran this build before left in its state folder: the version last run
/// before it (`since`, none when this build is the first the user ran), whether the page was
/// opened, and the release notes of the versions between, as the daemon's download kept them.
fn kept(user: &User, since: Option<&str>, seen: bool, notes: Value) {
    let folder = user.path("state/flottille");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .unwrap();
    std::fs::write(
        folder.join("whats-new.json"),
        json!({"version": this_version(), "since": since, "seen": seen, "notes": notes})
            .to_string(),
    )
    .unwrap();
}

use std::os::unix::fs::DirBuilderExt as _;

fn note(version: &str, body: &str) -> Value {
    json!({"version": version, "published_at": "2026-11-10T10:00:00Z", "body": body})
}

/// Three betas' notes, one change in two of them (a rerun of the same fix), and a line without
/// a scope.
fn betas() -> Value {
    json!([
        note(
            "0.4.0-beta.104",
            "### Features\n\n- **gui**: the sidebar says what is new (#1101)\n- **cli**: changes prints it (#1102)\n\n### Fixes\n\n- **gui**: a link opens (#1103)"
        ),
        note(
            "0.4.0-beta.105",
            "### Fixes\n\n- **gui**: a link opens (#1103)\n- **daemon**: the check waits (#1104)"
        ),
        note(
            "0.4.0-beta.106",
            "### Breaking changes\n\n- **protocol**: update.show says more (#1105)\n\n### Features\n\n- **protocol**: update.show says more (#1105)\n- one line with no scope (#1106)\n\n### Other changes\n\n- **repo**: tidy (#1107)"
        ),
    ])
}

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
        // Its clock moves, or what folds in (the mark) never gets a height.
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

fn opened_links(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Arc<Mutex<Vec<PathBuf>>> {
    let opened: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let given = opened.clone();
    shell.update(cx, |s, _| {
        s.set_opener(Arc::new(move |path: &Path| {
            given.lock().unwrap().push(path.to_owned())
        }))
    });
    opened
}

/// The change lines the page draws, in order: their words.
fn changes(cx: &mut VisualTestContext) -> Vec<String> {
    (0..)
        .map(|n| label(cx, &format!("whats-new-change-{n}")))
        .take_while(Option::is_some)
        .flatten()
        .collect()
}

#[gpui_kit::test]
fn a_newer_version_marks_what_is_new_without_opening_it(cx: &mut TestAppContext) {
    let user = User::new();
    kept(&user, Some(SINCE), false, betas());
    let (shell, cx) = window(&user, cx);

    until(&shell, cx, "the mark appears", |cx| {
        visible(cx, "sidebar-whats-new")
    });
    settle(cx);

    assert_eq!(
        label(cx, "sidebar-whats-new").as_deref(),
        Some("What's new")
    );
    assert!(
        visible(cx, "sidebar-whats-new-dot"),
        "a dot until it is opened"
    );
    // Nothing opened by itself: no page, and the daemon still says it was not seen.
    assert!(!visible(cx, "whats-new-page"));
    assert_eq!(
        user.call("whatsnew.show", json!({})).unwrap()["seen"],
        false
    );
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

#[gpui_kit::test]
fn clicking_the_mark_opens_what_is_new_and_clears_its_dot(cx: &mut TestAppContext) {
    let user = User::new();
    kept(&user, Some(SINCE), false, betas());
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the mark appears", |cx| {
        visible(cx, "sidebar-whats-new")
    });

    click(cx, "sidebar-whats-new");

    until(&shell, cx, "the page opens", |cx| {
        visible(cx, "whats-new-page")
    });
    until(&shell, cx, "the dot clears", |cx| {
        !visible(cx, "sidebar-whats-new-dot")
    });
    settle(cx);
    assert_eq!(
        label(cx, "whats-new-header-title").as_deref(),
        Some(format!("What's new since {SINCE}").as_str())
    );
    assert!(!visible(cx, "sidebar-whats-new-dot"));
    // Opened once, for good: the daemon keeps it.
    assert_eq!(user.call("whatsnew.show", json!({})).unwrap()["seen"], true);
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

#[gpui_kit::test]
fn what_is_new_spans_every_beta_since_the_version_last_run(cx: &mut TestAppContext) {
    let user = User::new();
    kept(&user, Some(SINCE), false, betas());
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the mark appears", |cx| {
        visible(cx, "sidebar-whats-new")
    });
    let opened = opened_links(&shell, cx);
    click(cx, "sidebar-whats-new");
    until(&shell, cx, "the changes are listed", |cx| {
        visible(cx, "whats-new-change-0")
    });
    settle(cx);

    // As the changelog groups them (breaking changes, features, fixes, other), then by scope,
    // each change once: the fix two betas share is one line, and a change that is both breaking
    // and a feature is said under each group, as the changelog says it.
    assert_eq!(
        [
            "whats-new-group-breaking",
            "whats-new-group-features",
            "whats-new-group-fixes",
            "whats-new-group-other",
        ]
        .map(|id| visible(cx, id)),
        [true; 4]
    );
    assert!(
        !visible(cx, "whats-new-group-performance"),
        "a group with nothing is not drawn"
    );
    assert_eq!(
        changes(cx),
        [
            "update.show says more",
            "changes prints it",
            "the sidebar says what is new",
            "update.show says more",
            "one line with no scope",
            "the check waits",
            "a link opens",
            "tidy",
        ]
    );
    assert_eq!(label(cx, "whats-new-change-0-pr").as_deref(), Some("#1105"));

    // A pull request opens with the system's handler.
    click(cx, "whats-new-change-2-pr");
    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from(
            "https://github.com/KristenJestin/flottille/pull/1101"
        )]
    );
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
}

#[gpui_kit::test]
fn what_is_new_is_found_again_in_settings(cx: &mut TestAppContext) {
    let user = User::new();
    // Opened before: no mark, and the page is still there.
    kept(&user, Some(SINCE), true, betas());
    let (shell, cx) = window(&user, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.open_settings_at(flottille_window::settings::Reveal::Version, window, cx)
    });
    until(&shell, cx, "Settings says what is new", |cx| {
        visible(cx, "version-whats-new-open")
    });
    assert!(
        !visible(cx, "sidebar-whats-new"),
        "no mark once it was opened"
    );
    assert!(
        label(cx, "version-whats-new-open").is_some_and(|said| said.contains(SINCE)),
        "{:?}",
        label(cx, "version-whats-new-open")
    );

    click(cx, "version-whats-new-open");

    until(&shell, cx, "the page opens", |cx| {
        visible(cx, "whats-new-change-0")
    });
    assert_eq!(changes(cx).len(), 8);
    // The page says the build it is of: version, channel and commit.
    assert!(
        label(cx, "whats-new-build").is_some_and(|said| said.starts_with(&this_version())),
        "{:?}",
        label(cx, "whats-new-build")
    );
}

#[gpui_kit::test]
fn the_same_version_shows_nothing_new(cx: &mut TestAppContext) {
    let user = User::new();
    // This build is the one last run: nothing is newer.
    kept(&user, None, true, Value::Null);
    let (shell, cx) = window(&user, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.open_settings_at(flottille_window::settings::Reveal::Version, window, cx)
    });
    until(&shell, cx, "the Version section is read", |cx| {
        visible(cx, "version-check-now")
    });
    settle(cx);

    assert!(!visible(cx, "sidebar-whats-new"));
    assert!(!visible(cx, "version-whats-new-open"));
    let shown = user.call("whatsnew.show", json!({})).unwrap();
    assert_eq!(shown["since"], Value::Null);
    assert_eq!(shown["groups"], Value::Null);
}

#[gpui_kit::test]
fn notes_that_cannot_be_fetched_are_said(cx: &mut TestAppContext) {
    // The notes were not kept (a build installed by hand) and nothing answers where the releases
    // are read: the page says so, and offers to read them again.
    let user = User::new().with_update_check();
    kept(&user, Some(SINCE), false, Value::Null);
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the mark appears", |cx| {
        visible(cx, "sidebar-whats-new")
    });

    click(cx, "sidebar-whats-new");

    until(&shell, cx, "the page says it", |cx| {
        visible(cx, "whats-new-failed")
    });
    assert!(
        label(cx, "whats-new-failed").is_some_and(|said| said.contains("could not be read")),
        "{:?}",
        label(cx, "whats-new-failed")
    );
    assert!(!visible(cx, "whats-new-change-0"));
    assert!(visible(cx, "whats-new-again"));
}
