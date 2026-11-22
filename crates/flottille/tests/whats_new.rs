//! The daemon's side of #1120: it keeps the version last run, and what is new since it, gathered
//! from the release notes (`docs/releases.md`, "What's new"). A build the window downloaded
//! brings its notes; a build installed by hand reads them at its first start. The releases are a
//! local server standing for GitHub (`release.rs`): nothing reaches the network.

use crate::common;
use crate::release::{Releases, this_version};

use std::time::{Duration, Instant};

use std::os::unix::fs::DirBuilderExt as _;

use common::User;
use serde_json::{Value, json};

const BODY: &str = "### Window\n\n- See what is new from the sidebar. (#1101)\n\n### Other\n\n- Read what is new from the command line. (#1102)\n\n### Fixed\n\n- A link opens. (#1103)\n\n### All changes\n\n#### Features\n\n- **gui**: the sidebar says what is new (#1101)\n- **cli**: changes prints it (#1102)\n\n#### Fixes\n\n- **gui**: a link opens (#1103)";

/// The body of a release made before the notes (#1178): the subjects, grouped by type.
const OLD_BODY: &str = "### Features\n\n- **gui**: the sidebar says what is new (#1101)\n- **cli**: changes prints it (#1102)\n\n### Fixes\n\n- **gui**: a link opens (#1103)";

/// The state the daemon of an older build left: it ran `version`, and nothing was new.
fn ran(user: &User, version: &str) {
    let folder = user.path("state/flottille");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .unwrap();
    std::fs::write(
        folder.join("whats-new.json"),
        json!({"version": version, "since": null, "seen": true, "notes": null}).to_string(),
    )
    .unwrap();
}

/// A server whose list of releases holds this build's, with `BODY` as its notes.
fn releases_with_notes() -> Releases {
    releases_saying(BODY)
}

/// A server whose list of releases holds this build's, with `body` as its notes.
fn releases_saying(body: &str) -> Releases {
    let releases = Releases::serve();
    let list = json!([{
        "tag_name": format!("v{}", this_version()),
        "published_at": "2026-11-10T10:00:00Z",
        "body": body,
        "assets": [],
    }]);
    releases.put(
        "/repos/o/r/releases?per_page=100",
        list.to_string().into_bytes(),
    );
    releases
}

fn asked(releases: &Releases) -> Vec<String> {
    releases.asked.lock().unwrap().clone()
}

/// `whatsnew.show` until `check` holds of it, 20 s at most.
fn shown_until(user: &User, what: &str, check: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let shown = user.call("whatsnew.show", json!({})).unwrap();
        if check(&shown) {
            return shown;
        }
        assert!(Instant::now() < deadline, "{what} never happened: {shown}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_build_installed_by_hand_gets_its_notes_at_its_first_start() {
    let releases = releases_with_notes();
    let user = User::new()
        .with_update_check()
        .with_env("FLOTTILLE_RELEASES", &releases.api());
    ran(&user, "0.0.1");

    let shown = shown_until(&user, "the notes", |shown| !shown["groups"].is_null());

    assert_eq!(shown["version"], this_version());
    assert_eq!(shown["since"], "0.0.1");
    assert_eq!(shown["seen"], false);
    assert_eq!(shown["failed"], Value::Null);
    // The areas of the notes, fixes last, each note with its pull request.
    let groups = shown["groups"].as_array().unwrap();
    assert_eq!(
        groups.iter().map(|g| g["key"].clone()).collect::<Vec<_>>(),
        ["window", "other", "fixed"]
    );
    assert_eq!(groups[0]["title"], "Window");
    let change = &groups[1]["changes"][0];
    assert_eq!(change["text"], "Read what is new from the command line.");
    assert_eq!(change["pr"], 1102);
    assert_eq!(
        change["url"],
        "https://github.com/KristenJestin/flottille/pull/1102"
    );
    assert_eq!(shown["highlights"], json!([]));
    // The raw list, as the changelog groups it, for the fold at the foot.
    assert_eq!(shown["all"][0]["key"], "features");
    assert_eq!(shown["all"][0]["scopes"][0]["scope"], "cli");
    assert_eq!(
        shown["all"][0]["scopes"][0]["changes"][0]["text"],
        "changes prints it"
    );
    // Read once.
    let reads = asked(&releases)
        .iter()
        .filter(|path| path.contains("per_page=100"))
        .count();
    assert_eq!(reads, 1, "{:?}", asked(&releases));
}

#[test]
fn an_update_from_the_window_keeps_its_notes_so_nothing_is_fetched() {
    let releases = Releases::serve();
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    ran(&user, "0.0.1");
    // What #1119's start of the new build left: the manifest it applied.
    std::fs::write(
        user.path("state/flottille/updated.json"),
        json!({
            "version": this_version(),
            "from": "0.0.1",
            "channel": "stable",
            "notes": [{"version": this_version(), "published_at": "2026-11-10T10:00:00Z", "body": BODY}],
        })
        .to_string(),
    )
    .unwrap();

    let shown = shown_until(&user, "the notes", |shown| !shown["groups"].is_null());

    assert_eq!(shown["since"], "0.0.1");
    assert_eq!(
        shown["groups"][0]["changes"][0]["text"],
        "See what is new from the sidebar."
    );
    assert_eq!(asked(&releases), Vec::<String>::new());
}

#[test]
fn with_the_check_off_no_request_is_made_for_the_notes_until_asked() {
    let releases = releases_with_notes();
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    ran(&user, "0.0.1");

    let shown = shown_until(&user, "the daemon's first answer", |shown| {
        shown["since"] == "0.0.1"
    });

    assert_eq!(shown["groups"], Value::Null);
    assert!(
        shown["failed"]
            .as_str()
            .is_some_and(|why| why.contains("off")),
        "{shown}"
    );
    assert_eq!(asked(&releases), Vec::<String>::new());

    // The user asks: one request, whatever the setting.
    user.call("whatsnew.fetch", json!({})).unwrap();
    let shown = shown_until(&user, "the notes", |shown| !shown["groups"].is_null());
    assert_eq!(shown["failed"], Value::Null);
    assert_eq!(asked(&releases).len(), 1);
}

#[test]
fn flottille_changes_prints_the_areas() {
    let releases = releases_saying(&format!(
        "### Highlights\n\n- Drive another machine from the window.\n\n{BODY}"
    ));
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());

    let output = user.run(&["changes", "--since", "0.0.1"]);

    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(text.contains("What's new since 0.0.1"), "{text}");
    // The areas, in the order of the page, each with its notes and their numbers; the raw list
    // of subjects is not printed (`--json` has it).
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let wanted = [
        "Highlights",
        "- Drive another machine from the window.",
        "Window",
        "- See what is new from the sidebar. (#1101)",
        "Other",
        "- Read what is new from the command line. (#1102)",
        "Fixed",
        "- A link opens. (#1103)",
    ];
    let at: Vec<usize> = wanted
        .iter()
        .map(|line| {
            lines
                .iter()
                .position(|got| got == line)
                .unwrap_or_else(|| panic!("{line:?} is missing from {text}"))
        })
        .collect();
    assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    assert!(!text.contains("changes prints it"), "{text}");
    let json = user.run(&["changes", "--since", "0.0.1", "--json"]);
    let shown: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(shown["since"], "0.0.1");
    assert_eq!(shown["groups"][0]["key"], "window");
    assert_eq!(
        shown["highlights"][0],
        "Drive another machine from the window."
    );
}

#[test]
fn an_old_body_without_notes_falls_back_to_its_subjects() {
    let releases = releases_saying(OLD_BODY);
    let user = User::new()
        .with_update_check()
        .with_env("FLOTTILLE_RELEASES", &releases.api());
    ran(&user, "0.0.1");

    let shown = shown_until(&user, "the notes", |shown| !shown["groups"].is_null());

    // Each subject with a capital and no scope, all under Other.
    let groups = shown["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{shown}");
    assert_eq!(groups[0]["key"], "other");
    let texts: Vec<&str> = groups[0]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts,
        [
            "The sidebar says what is new",
            "Changes prints it",
            "A link opens"
        ]
    );
    assert_eq!(groups[0]["changes"][2]["pr"], 1103);
    assert_eq!(shown["highlights"], json!([]));
}

#[test]
fn changes_with_nothing_new_says_so() {
    let user = User::new();
    ran(&user, &this_version());

    let output = user.run(&["changes"]);

    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Nothing new"),
        "{output:?}"
    );
}
