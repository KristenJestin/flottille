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

/// Three betas' notes in the shape the changelog gives them (#1178): one note in two of them (a
/// rerun of the same fix), a Remote and a Window note in the last, and a commit that has no note,
/// which is only in the raw list.
fn betas() -> Value {
    json!([
        note(
            "0.4.0-beta.104",
            "### Window\n\n- See what is new from the sidebar. (#1101)\n\n### Other\n\n- Read what is new from the command line. (#1102)\n\n### Fixed\n\n- A link opens. (#1103)\n\n### All changes\n\n#### Features\n\n- **gui**: the sidebar says what is new (#1101)\n- **cli**: changes prints it (#1102)\n\n#### Fixes\n\n- **gui**: a link opens (#1103)"
        ),
        note(
            "0.4.0-beta.105",
            "### Fixed\n\n- A link opens. (#1103)\n- The check for updates waits for the network. (#1104)\n\n### All changes\n\n#### Fixes\n\n- **gui**: a link opens (#1103)\n- **daemon**: the check waits (#1104)"
        ),
        note(
            "0.4.0-beta.106",
            "### Remote\n\n- Update a machine from the hub. (#1105)\n\n### Window\n\n- Open a link from a note. (#1106)\n\n### All changes\n\n#### Features\n\n- **protocol**: update.show says more (#1105)\n- **gui**: a link in a note (#1106)\n- one line with no scope (#1107)\n\n#### Other changes\n\n- **repo**: tidy (#1108)"
        ),
    ])
}

/// The notes of `betas()`, in the order of the page: the areas in theirs, each oldest first.
const NOTES: [&str; 6] = [
    "Update a machine from the hub.",
    "See what is new from the sidebar.",
    "Open a link from a note.",
    "Read what is new from the command line.",
    "A link opens.",
    "The check for updates waits for the network.",
];

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

/// What the page's title says of this build: `What's new in 0.4`.
fn title() -> String {
    let version = this_version();
    let line: Vec<&str> = version.split('.').take(2).collect();
    format!("What's new in {}", line.join("."))
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
    // Unfolded: a click on a mark still folding in lands on nothing.
    settle(cx);

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
        Some(title().as_str())
    );
    assert_eq!(
        label(cx, "whats-new-range").as_deref(),
        Some(format!("since {SINCE}").as_str())
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
    // Unfolded: a click on a mark still folding in lands on nothing.
    settle(cx);
    let opened = opened_links(&shell, cx);
    click(cx, "sidebar-whats-new");
    until(&shell, cx, "the changes are listed", |cx| {
        visible(cx, "whats-new-change-0")
    });
    settle(cx);

    // One section per area, in the order of the areas, each with its count; the fixes last. A
    // note two betas share is one row, and an area with nothing is not drawn.
    assert_eq!(
        [
            "remote", "window", "agents", "tools", "projects", "other", "fixed"
        ]
        .map(|key| visible(cx, &format!("whats-new-area-{key}-title"))),
        [true, true, false, false, false, true, true]
    );
    assert_eq!(
        label(cx, "whats-new-area-window-title").as_deref(),
        Some("Window")
    );
    assert_eq!(
        ["remote", "window", "other", "fixed"].map(|key| label(
            cx,
            &format!("whats-new-area-{key}-count")
        )
        .unwrap()),
        ["1", "2", "1", "2"]
    );
    assert_eq!(changes(cx), NOTES);
    assert_eq!(label(cx, "whats-new-change-0-pr").as_deref(), Some("#1105"));

    // A pull request opens with the system's handler.
    click(cx, "whats-new-change-1-pr");
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
    assert_eq!(changes(cx).len(), NOTES.len());
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
    // Unfolded: a click on a mark still folding in lands on nothing.
    settle(cx);

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

/// The page opened on `notes`, settled.
fn opened_on<'a>(
    user: &User,
    notes: Value,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    kept(user, Some(SINCE), false, notes);
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the mark appears", |cx| {
        visible(cx, "sidebar-whats-new")
    });
    settle(cx);
    click(cx, "sidebar-whats-new");
    until(&shell, cx, "the page opens", |cx| {
        visible(cx, "whats-new-page")
    });
    until(&shell, cx, "the notes are drawn", |cx| {
        visible(cx, "whats-new-change-0")
    });
    settle(cx);
    (shell, cx)
}

#[gpui_kit::test]
fn a_pull_request_without_a_note_is_not_on_the_page(cx: &mut TestAppContext) {
    let user = User::new();
    let (_, cx) = opened_on(&user, betas(), cx);

    // "one line with no scope" and "tidy" have no note: they are not rows of an area...
    let said = changes(cx);
    assert!(
        !said
            .iter()
            .any(|row| row.contains("tidy") || row.contains("no scope")),
        "{said:?}"
    );
    // ... and no section is made for them: their commits only exist in the raw list.
    assert!(!visible(cx, "whats-new-area-agents-title"));
    assert_eq!(said.len(), NOTES.len());
    assert!(!visible(cx, "whats-new-raw-0"), "the raw list is folded");
}

#[gpui_kit::test]
fn the_raw_list_is_in_a_closed_fold(cx: &mut TestAppContext) {
    let user = User::new();
    let (_, cx) = opened_on(&user, betas(), cx);

    // Closed by default, at the foot, with how many lines it holds.
    assert!(visible(cx, "whats-new-all"));
    assert_eq!(label(cx, "whats-new-all").as_deref(), Some("All changes"));
    assert!(!visible(cx, "whats-new-raw-0"));
    assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);

    click(cx, "whats-new-all");

    // The raw subjects, as the release bodies list them: by type, then scope, each once.
    assert!(visible(cx, "whats-new-raw-0"));
    let raw: Vec<String> = (0..)
        .map(|n| label(cx, &format!("whats-new-raw-{n}")))
        .take_while(Option::is_some)
        .flatten()
        .collect();
    assert!(
        raw.iter().any(|line| line == "update.show says more"),
        "{raw:?}"
    );
    assert!(raw.iter().any(|line| line == "tidy"), "{raw:?}");
    assert!(
        raw.iter().any(|line| line == "one line with no scope"),
        "{raw:?}"
    );
    assert_eq!(
        raw.iter().filter(|line| *line == "a link opens").count(),
        1,
        "{raw:?}"
    );

    click(cx, "whats-new-all");
    assert!(!visible(cx, "whats-new-raw-0"), "it folds again");
}

#[gpui_kit::test]
fn highlights_are_shown_when_the_release_has_some(cx: &mut TestAppContext) {
    let user = User::new();
    let mut notes = betas();
    notes[2]["body"] = json!(format!(
        "### Highlights\n\n- Drive another machine from the window.\n- Read what changed in your own words.\n\n{}",
        notes[2]["body"].as_str().unwrap()
    ));
    let (_, cx) = opened_on(&user, notes, cx);

    assert!(visible(cx, "whats-new-highlights-title"));
    assert_eq!(
        [0, 1].map(|n| label(cx, &format!("whats-new-highlight-{n}")).unwrap()),
        [
            "Drive another machine from the window.",
            "Read what changed in your own words."
        ]
    );
    // They are not counted among the notes.
    assert_eq!(changes(cx), NOTES);
}

#[gpui_kit::test]
fn a_release_without_highlights_draws_no_block_for_them(cx: &mut TestAppContext) {
    let user = User::new();
    let (_, cx) = opened_on(&user, betas(), cx);

    assert!(!visible(cx, "whats-new-highlights-title"));
}
