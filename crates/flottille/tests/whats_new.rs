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

const BODY: &str = "### Features\n\n- **gui**: the sidebar says what is new (#1101)\n- **cli**: changes prints it (#1102)\n\n### Fixes\n\n- **gui**: a link opens (#1103)";

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
    let releases = Releases::serve();
    let list = json!([{
        "tag_name": format!("v{}", this_version()),
        "published_at": "2026-11-10T10:00:00Z",
        "body": BODY,
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
    let groups = shown["groups"].as_array().unwrap();
    assert_eq!(
        groups.iter().map(|g| g["key"].clone()).collect::<Vec<_>>(),
        ["features", "fixes"]
    );
    assert_eq!(groups[0]["title"], "Features");
    // By scope: cli before gui.
    let scopes = groups[0]["scopes"].as_array().unwrap();
    assert_eq!(scopes[0]["scope"], "cli");
    assert_eq!(scopes[0]["changes"][0]["text"], "changes prints it");
    assert_eq!(scopes[0]["changes"][0]["pr"], 1102);
    assert_eq!(
        scopes[0]["changes"][0]["url"],
        "https://github.com/KristenJestin/flottille/pull/1102"
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
        shown["groups"][0]["scopes"][0]["changes"][0]["text"],
        "changes prints it"
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
fn changes_prints_what_is_new_since_a_version() {
    let releases = releases_with_notes();
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());

    let output = user.run(&["changes", "--since", "0.0.1"]);

    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(text.contains("What's new since 0.0.1"), "{text}");
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    for wanted in [
        "Features",
        "cli",
        "- changes prints it (#1102)",
        "gui",
        "- the sidebar says what is new (#1101)",
        "Fixes",
        "- a link opens (#1103)",
    ] {
        assert!(lines.contains(&wanted), "{wanted:?} is missing from {text}");
    }
    let json = user.run(&["changes", "--since", "0.0.1", "--json"]);
    let shown: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(shown["since"], "0.0.1");
    assert_eq!(shown["groups"][0]["key"], "features");
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
