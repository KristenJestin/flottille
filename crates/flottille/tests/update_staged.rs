//! The scenarios of #1119: the daemon checks its channel for a newer build, downloads it beside the
//! installed binary under a staging name when the user asks (the window, `flottille update
//! --stage`), and the next start of the daemon applies it; a restart now, or once no agent runs.
//! The releases are a local server standing for GitHub (`release.rs`), the binary a link to the
//! real one in a folder of its own, the user's folders throwaway ones: nothing reaches the network
//! and nothing touches `~/.local/bin`.

use crate::common;
use crate::release::{Releases, installed, sha256, stable_at, standing_for, this_version};

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// The version every newer build here is published at: above any version this build can have.
const NEWER: &str = "99.0.0";

/// A user whose daemon reads the releases from `releases`, and checks them as everyone's does.
fn checking(releases: &Releases) -> User {
    User::new()
        .with_update_check()
        .with_env("FLOTTILLE_RELEASES", &releases.api())
}

/// The folder a build is staged in, beside the binary.
fn staging(binary: &Path) -> PathBuf {
    binary.with_file_name("flottille.next")
}

/// The daemon started through `binary`, as the window or a command does: it then runs from it.
fn start_through(user: &User, binary: &Path) -> Value {
    let output = user
        .command_through(binary, &["status", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The daemon whose status is `status` runs from `binary`, the copy in a folder of its own: a
/// daemon of the shared test binary would download or restart beside it.
fn runs_from(status: &Value, binary: &Path) {
    let pid = status["pid"].as_u64().unwrap_or_else(|| panic!("{status}"));
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    assert_eq!(
        exe,
        binary.canonicalize().unwrap(),
        "the daemon runs from the installed copy, not the shared test binary"
    );
}

/// `update.show` until `check` holds of it, 20 s at most.
fn shown_until(user: &User, what: &str, check: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let shown = user.call("update.show", json!({})).unwrap();
        if check(&shown) {
            return shown;
        }
        assert!(Instant::now() < deadline, "{what} never happened: {shown}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A program standing for a newer build that runs the daemon: it notes each of its starts in
/// `marker`, then is the real binary, so that a daemon started from it serves.
fn runs_the_daemon(dir: &Path, marker: &Path) -> PathBuf {
    let path = dir.join("flottille-newer");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            marker.display(),
            env!("CARGO_BIN_EXE_flottille")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path
}

/// A stable release at `NEWER` whose binary is `program`, listed with its notes too.
fn newer_stable(releases: &Releases, program: &Path) -> Value {
    let stable = releases.release(&format!("v{NEWER}"), NEWER, program, None);
    releases.put(
        "/repos/o/r/releases/latest",
        stable.to_string().into_bytes(),
    );
    releases.list_as_github(&[&stable]);
    stable
}

/// `flottille update --stage` through `binary`.
fn stage(user: &User, binary: &Path) -> std::process::Output {
    user.command_through(binary, &["update", "--stage"])
        .output()
        .unwrap()
}

fn requests(releases: &Releases) -> Vec<String> {
    releases.asked.lock().unwrap().clone()
}

#[test]
fn the_daemon_finds_a_newer_build_of_its_channel() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    stable_at(&releases, programs.path(), NEWER);
    let user = checking(&releases);

    let shown = shown_until(&user, "the check", |shown| shown["latest"] == NEWER);

    assert_eq!(shown["version"], this_version());
    // A local build follows the stable channel, as `flottille update` does.
    assert_eq!(shown["channel"], "stable");
    assert_eq!(shown["check"], "automatically");
    assert_eq!(shown["failed"], Value::Null);
    assert_eq!(shown["staged"], Value::Null);
    assert!(
        requests(&releases).contains(&"/repos/o/r/releases/latest".to_owned()),
        "{:?}",
        requests(&releases)
    );
    assert!(
        !requests(&releases).iter().any(|p| p.contains("/download/")),
        "nothing is downloaded without the user's click"
    );
}

#[test]
fn a_curlrc_of_the_user_does_not_change_the_check() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    stable_at(&releases, programs.path(), NEWER);
    let user = checking(&releases);
    // What an agent could have written: curl obeying either file fails every request.
    let refused = "max-filesize = 1\nproxy = \"http://127.0.0.1:9\"\nnoproxy = \"\"\n";
    std::fs::create_dir_all(user.path("home")).unwrap();
    std::fs::write(user.path("home/.curlrc"), refused).unwrap();
    std::fs::create_dir_all(user.path("config")).unwrap();
    std::fs::write(user.path("config/curlrc"), refused).unwrap();

    let shown = shown_until(&user, "the check", |shown| {
        shown["latest"] == NEWER || shown["failed"].is_string()
    });

    assert_eq!(shown["latest"], NEWER, "{shown}");
    assert_eq!(shown["failed"], Value::Null);
}

#[test]
fn a_failed_check_is_said_once_in_the_update_s_place() {
    // Nothing listens on that port.
    let user = User::new()
        .with_update_check()
        .with_env("FLOTTILLE_RELEASES", "http://127.0.0.1:9/repos/o/r");

    let shown = shown_until(&user, "the failed check", |shown| {
        shown["failed"].is_string()
    });

    assert_eq!(shown["latest"], Value::Null);
    assert!(
        shown["failed"].as_str().unwrap().contains("127.0.0.1:9"),
        "{shown}"
    );
}

#[test]
fn turning_the_check_off_makes_no_request() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    stable_at(&releases, programs.path(), NEWER);
    let user = checking(&releases);
    shown_until(&user, "the first check", |shown| shown["latest"] == NEWER);

    let set = user.call("update.set", json!({"check": "never"})).unwrap();
    assert_eq!(set["check"], "never");
    let file = std::fs::read_to_string(user.path("config/flottille/updates.toml")).unwrap();
    assert!(file.contains("check = \"never\""), "{file}");
    // Restarted, the daemon reads the setting from the file: it asks nothing at its start.
    user.run(&["daemon", "stop"]);
    releases.asked.lock().unwrap().clear();
    user.status();
    std::thread::sleep(Duration::from_secs(2));

    let shown = user.call("update.show", json!({})).unwrap();
    assert_eq!(shown["check"], "never");
    assert_eq!(shown["latest"], Value::Null);
    assert!(requests(&releases).is_empty(), "{:?}", requests(&releases));
}

#[test]
fn a_broken_updates_toml_means_never_and_is_said() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    stable_at(&releases, programs.path(), NEWER);
    let user = checking(&releases);
    let file = user.path("config/flottille/updates.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "check = [\n").unwrap();

    user.status();
    std::thread::sleep(Duration::from_secs(2));

    let shown = user.call("update.show", json!({})).unwrap();
    assert!(requests(&releases).is_empty(), "{:?}", requests(&releases));
    assert_eq!(shown["check"], "never", "{shown}");
    assert_eq!(shown["latest"], Value::Null);
    assert!(
        shown["failed"]
            .as_str()
            .is_some_and(|failed| failed.contains("updates.toml")),
        "{shown}"
    );
}

#[test]
fn check_now_checks_once_with_the_check_off() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    stable_at(&releases, programs.path(), NEWER);
    // The harness's daemon never checks on its own.
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    user.status();
    std::thread::sleep(Duration::from_secs(1));
    assert!(requests(&releases).is_empty(), "{:?}", requests(&releases));
    assert_eq!(
        user.call("update.show", json!({})).unwrap()["check"],
        "never"
    );

    user.call("update.check", json!({})).unwrap();

    let shown = shown_until(&user, "the check asked", |shown| shown["latest"] == NEWER);
    assert_eq!(shown["check"], "never");
    let asked = requests(&releases);
    assert_eq!(
        asked
            .iter()
            .filter(|p| *p == "/repos/o/r/releases/latest")
            .count(),
        1,
        "{asked:?}"
    );
    assert!(!asked.iter().any(|p| p.contains("/download/")), "{asked:?}");
}

#[test]
fn a_build_downloaded_from_the_window_waits_staged() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    newer_stable(&releases, &standing_for(programs.path(), NEWER));
    let user = checking(&releases);
    let binary = installed();
    let before = sha256(&binary);
    runs_from(&start_through(&user, &binary), &binary);
    shown_until(&user, "the check", |shown| shown["latest"] == NEWER);

    user.call("update.download", json!({})).unwrap();

    let shown = shown_until(&user, "the download", |shown| shown["staged"] == NEWER);
    assert_eq!(shown["downloading"], Value::Null);
    // Nothing running is touched: the daemon still runs from the binary, which is the same.
    assert_eq!(sha256(&binary), before);
    let staged = staging(&binary);
    assert_eq!(
        std::fs::read_to_string(staged.join("flottille")).unwrap(),
        std::fs::read_to_string(programs.path().join(format!("flottille-{NEWER}"))).unwrap()
    );
    assert!(
        staged
            .join("share/applications/flottille.desktop")
            .is_file()
    );
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(staged.join("staged.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], NEWER);
    assert_eq!(manifest["sha256"], sha256(staged.join("flottille")));
    // The notes of every release between this build and the staged one, for "What's new".
    let notes = manifest["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{manifest}");
    assert_eq!(notes[0]["version"], NEWER);
    assert!(
        notes[0]["body"].as_str().unwrap().contains("Changelog"),
        "{manifest}"
    );
    // Nothing else is left beside the binary.
    let mut left: Vec<String> = std::fs::read_dir(binary.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["flottille", "flottille.next"]);
}

#[test]
fn cancel_stops_the_download_and_stages_nothing() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    newer_stable(&releases, &standing_for(programs.path(), NEWER));
    let hold = releases.hold(&format!(
        "/download/v{NEWER}/flottille-{NEWER}-x86_64-linux.tar.gz"
    ));
    let user = checking(&releases);
    let binary = installed();
    runs_from(&start_through(&user, &binary), &binary);
    shown_until(&user, "the check", |shown| shown["latest"] == NEWER);
    user.call("update.download", json!({})).unwrap();
    shown_until(&user, "the download to read some", |shown| {
        shown["downloading"]["done"]
            .as_u64()
            .is_some_and(|done| done > 0)
    });

    let answer = user.call("update.cancel", json!({})).unwrap();

    assert_eq!(answer["download_failed"], Value::Null, "{answer}");
    let shown = shown_until(&user, "the download to stop", |shown| {
        shown["downloading"] == Value::Null
    });
    assert_eq!(
        shown["download_failed"],
        Value::Null,
        "a cancel is not a failure"
    );
    assert_eq!(shown["staged"], Value::Null);
    assert_eq!(shown["latest"], NEWER, "the build can be downloaded again");
    hold.release();
    // Nothing is left beside the binary, not the staging, not what was being read.
    let left: Vec<String> = std::fs::read_dir(binary.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["flottille"]);
}

#[test]
fn cancel_with_no_download_changes_nothing() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    newer_stable(&releases, &standing_for(programs.path(), NEWER));
    let user = checking(&releases);
    let binary = installed();
    runs_from(&start_through(&user, &binary), &binary);
    shown_until(&user, "the check", |shown| shown["latest"] == NEWER);

    let answer = user.call("update.cancel", json!({})).unwrap();
    assert_eq!(answer["downloading"], Value::Null);

    // A cancel with nothing to cancel is not remembered: the next download runs through.
    user.call("update.download", json!({})).unwrap();
    shown_until(&user, "the download", |shown| shown["staged"] == NEWER);
}

#[test]
fn flottille_update_stage_stages_while_the_daemon_runs() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    newer_stable(&releases, &standing_for(programs.path(), NEWER));
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let before = sha256(&binary);
    let status = start_through(&user, &binary);

    let output = stage(&user, &binary);

    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("applied at the next start"),
        "{output:?}"
    );
    assert_eq!(sha256(&binary), before);
    assert!(staging(&binary).join("flottille").is_file());
    assert_eq!(user.status()["pid"], status["pid"], "the daemon runs on");
    assert_eq!(
        user.call("update.show", json!({})).unwrap()["staged"],
        NEWER
    );
}

#[test]
fn a_staged_build_is_applied_at_the_next_start() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let marker = user.path("started");
    let newer = runs_the_daemon(programs.path(), &marker);
    newer_stable(&releases, &newer);
    let binary = installed();
    let output = stage(&user, &binary);
    assert!(output.status.success(), "{output:?}");

    start_through(&user, &binary);

    // The daemon was started again from the new file, which is now the binary.
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "daemon");
    assert_eq!(
        std::fs::read_to_string(&*binary).unwrap(),
        std::fs::read_to_string(&newer).unwrap()
    );
    assert!(!staging(&binary).exists());
    let entry = std::fs::read_to_string(user.path("data/applications/flottille.desktop")).unwrap();
    assert!(
        entry.contains(&format!("Exec={} window", binary.display())),
        "{entry}"
    );
    // What was applied is kept, with its notes, for the window to say what is new.
    let applied: Value =
        serde_json::from_slice(&std::fs::read(user.path("state/flottille/updated.json")).unwrap())
            .unwrap();
    assert_eq!(applied["version"], NEWER);
    assert_eq!(applied["from"], this_version());
    assert_eq!(applied["notes"][0]["version"], NEWER);
}

/// A build staged by hand beside `binary`: `program` at `version`, its manifest saying `sum`.
fn staged_by_hand(binary: &Path, program: &Path, version: &str, sum: &str) {
    let staged = staging(binary);
    std::fs::create_dir(&staged).unwrap();
    std::fs::copy(program, staged.join("flottille")).unwrap();
    std::fs::write(
        staged.join("staged.json"),
        json!({"version": version, "sha256": sum, "channel": "stable", "notes": []}).to_string(),
    )
    .unwrap();
}

/// The daemon started through `binary` refuses what is staged: it runs from the binary as it was,
/// the staging is gone, and `update.show` says why.
fn refused_at_start(user: &User, binary: &Path, before: &str, why: &str) {
    start_through(user, binary);
    assert_eq!(sha256(binary), before);
    assert!(
        std::fs::symlink_metadata(staging(binary)).is_err(),
        "the staging is removed"
    );
    let shown = user.call("update.show", json!({})).unwrap();
    assert!(
        shown["refused"]
            .as_str()
            .is_some_and(|said| said.contains(why)),
        "{shown}"
    );
    user.run(&["daemon", "stop"]);
}

#[test]
fn a_staged_build_with_a_wrong_sum_or_older_is_removed() {
    let programs = tempfile::tempdir().unwrap();
    let user = User::new();
    let binary = installed();
    let before = sha256(&binary);

    let newer = standing_for(programs.path(), NEWER);
    staged_by_hand(&binary, &newer, NEWER, &"0".repeat(64));
    refused_at_start(&user, &binary, &before, "sum");

    let older = standing_for(programs.path(), "0.0.1");
    staged_by_hand(&binary, &older, "0.0.1", &sha256(&older));
    refused_at_start(&user, &binary, &before, "not newer");

    // A partial staging: its binary is missing.
    staged_by_hand(&binary, &newer, NEWER, &sha256(&newer));
    std::fs::remove_file(staging(&binary).join("flottille")).unwrap();
    refused_at_start(&user, &binary, &before, "not a staged build");

    // No manifest.
    staged_by_hand(&binary, &newer, NEWER, &sha256(&newer));
    std::fs::remove_file(staging(&binary).join("staged.json")).unwrap();
    refused_at_start(&user, &binary, &before, "not a staged build");

    // Its binary is a link to another file.
    staged_by_hand(&binary, &newer, NEWER, &sha256(&newer));
    std::fs::remove_file(staging(&binary).join("flottille")).unwrap();
    std::os::unix::fs::symlink(&newer, staging(&binary).join("flottille")).unwrap();
    refused_at_start(&user, &binary, &before, "not a staged build");
    assert!(newer.is_file(), "the link's target is left alone");
}

#[test]
fn a_staged_build_of_the_running_version_is_removed_quietly() {
    let programs = tempfile::tempdir().unwrap();
    let user = User::new();
    let binary = installed();
    let before = sha256(&binary);
    let same = standing_for(programs.path(), &this_version());
    staged_by_hand(&binary, &same, &this_version(), &sha256(&same));

    start_through(&user, &binary);

    // What was left is removed, the binary is as it was, and the window has nothing to say.
    assert_eq!(sha256(&binary), before);
    assert!(
        std::fs::symlink_metadata(staging(&binary)).is_err(),
        "the staging is removed"
    );
    let shown = user.call("update.show", json!({})).unwrap();
    assert_eq!(shown["refused"], Value::Null, "{shown}");
    // It is said in the daemon's log only.
    let log = std::fs::read_to_string(user.path("state/flottille/daemon.log")).unwrap_or_default();
    assert!(log.contains("the version that runs: removed"), "{log}");
    user.run(&["daemon", "stop"]);
}

#[test]
fn an_old_process_never_stages_the_build_already_installed() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let applied = standing_for(programs.path(), NEWER);
    newer_stable(&releases, &applied);
    let user = checking(&releases);
    let binary = installed();
    runs_from(&start_through(&user, &binary), &binary);
    shown_until(&user, "the check", |shown| shown["latest"] == NEWER);
    // The update is applied by another process while this daemon runs on its old binary: the
    // file at the install path is the newer build now.
    let moved = binary.with_file_name("flottille-applied");
    std::fs::copy(&applied, &moved).unwrap();
    std::fs::rename(&moved, &binary).unwrap();

    user.call("update.download", json!({})).unwrap();

    let shown = shown_until(&user, "the download", |shown| {
        shown["downloading"] == Value::Null
    });
    assert_eq!(shown["staged"], Value::Null, "{shown}");
    assert!(
        std::fs::symlink_metadata(staging(&binary)).is_err(),
        "nothing is staged beside the build that is installed"
    );
    // The command line stages nothing either.
    let output = stage(&user, &binary);
    assert!(!staging(&binary).exists(), "{output:?}");
}

#[test]
fn a_symlink_at_the_staging_name_is_refused() {
    let programs = tempfile::tempdir().unwrap();
    let user = User::new();
    let binary = installed();
    let before = sha256(&binary);
    // A staging that looks right, elsewhere, reached by a link at the staging name.
    let elsewhere = programs.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let newer = standing_for(programs.path(), NEWER);
    staged_by_hand(&elsewhere.join("flottille"), &newer, NEWER, &sha256(&newer));
    std::os::unix::fs::symlink(elsewhere.join("flottille.next"), staging(&binary)).unwrap();

    refused_at_start(&user, &binary, &before, "not a staged build");

    assert!(
        elsewhere.join("flottille.next/flottille").is_file(),
        "what the link named is left alone"
    );
}

#[test]
fn staging_refuses_a_symlink_at_the_staging_name() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    newer_stable(&releases, &standing_for(programs.path(), NEWER));
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let elsewhere = programs.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, staging(&binary)).unwrap();

    let output = stage(&user, &binary);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("link"),
        "{output:?}"
    );
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert!(
        std::fs::symlink_metadata(staging(&binary))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn a_download_with_a_wrong_sum_stages_nothing() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let stable = releases.release(
        &format!("v{NEWER}"),
        NEWER,
        &standing_for(programs.path(), NEWER),
        Some(&"0".repeat(64)),
    );
    releases.put(
        "/repos/o/r/releases/latest",
        stable.to_string().into_bytes(),
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();

    let output = stage(&user, &binary);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("SHA-256"),
        "{output:?}"
    );
    let left: Vec<String> = std::fs::read_dir(binary.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["flottille"]);
}

#[test]
fn a_plain_update_installs_the_latest_and_removes_the_staged_build() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    newer_stable(&releases, &standing_for(programs.path(), NEWER));
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    assert!(stage(&user, &binary).status.success());

    let output = user.command_through(&binary, &["update"]).output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(!staging(&binary).exists());
    let said = std::process::Command::new(&binary)
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&said.stdout).contains(NEWER),
        "{said:?}"
    );
}

/// A user whose fake agent works long on "work long", with a project to start it in.
fn with_a_long_turn(releases: &Releases) -> User {
    let user = User::with_fake_agent().with_env("FLOTTILLE_RELEASES", &releases.api());
    user.fake_script(
        json!({"turns": [{"when": "work long", "do": [{"wait": 4000}, {"say": "done"}]}]}),
    );
    let folder = user.path("work/app");
    user.git_init(&folder);
    user.git(
        &folder,
        &["commit", "--quiet", "--allow-empty", "-m", "start"],
    );
    user
}

#[test]
fn restart_when_idle_restarts_once_no_agent_runs() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let user = with_a_long_turn(&releases);
    let marker = user.path("started");
    newer_stable(&releases, &runs_the_daemon(programs.path(), &marker));
    let binary = installed();
    runs_from(&start_through(&user, &binary), &binary);
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
    assert!(stage(&user, &binary).status.success());
    user.call(
        "agent.start",
        json!({"project": "app", "message": "work long"}),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while user.status()["agents"]["running"] != 1 {
        assert!(Instant::now() < deadline, "the agent never ran");
        std::thread::sleep(Duration::from_millis(50));
    }

    let asked = user
        .call("update.restart", json!({"when": "idle"}))
        .unwrap();

    assert_eq!(asked["restart"], "idle");
    assert_eq!(
        user.call("update.show", json!({})).unwrap()["restart"],
        "idle"
    );
    assert!(!marker.exists(), "nothing restarts while the agent works");
    assert!(staging(&binary).exists());
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the daemon never restarted");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(user.status()["agents"]["running"], 0);
    assert!(!staging(&binary).exists());
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "daemon");
}

#[test]
fn update_show_lists_every_session_a_restart_stops() {
    let releases = Releases::serve();
    let user = with_a_long_turn(&releases);
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
    assert_eq!(
        user.call("update.show", json!({})).unwrap()["stops"],
        json!([])
    );
    let started = |message: &str| {
        user.call("agent.start", json!({"project": "app", "message": message}))
            .unwrap()["agent"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let working = started("work long");
    // Its turn ends at once, but its process lives on until it is released: a restart stops it.
    let idle = started("hello");
    let state = |agent: &str| {
        user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while state(&idle) != "idle" || state(&working) != "working" {
        assert!(
            Instant::now() < deadline,
            "the turns never went as scripted"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let shown = user.call("update.show", json!({})).unwrap();

    let mut stops: Vec<&str> = shown["stops"]
        .as_array()
        .unwrap_or_else(|| panic!("{shown}"))
        .iter()
        .map(|session| session["id"].as_str().unwrap())
        .collect();
    stops.sort_unstable();
    let mut expected = vec![working.as_str(), idle.as_str()];
    expected.sort_unstable();
    assert_eq!(stops, expected, "{shown}");
}

#[test]
fn restart_to_update_with_no_agent_restarts_at_once() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let marker = user.path("started");
    newer_stable(&releases, &runs_the_daemon(programs.path(), &marker));
    let binary = installed();
    runs_from(&start_through(&user, &binary), &binary);
    assert!(stage(&user, &binary).status.success());

    user.call("update.restart", json!({"when": "now"})).unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the daemon never restarted");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!staging(&binary).exists());
    assert_eq!(
        user.call("update.show", json!({})).unwrap()["staged"],
        Value::Null
    );
}

#[test]
fn restart_to_update_with_nothing_staged_is_refused() {
    let user = User::new();

    let refused = user.call("update.restart", json!({"when": "now"}));

    let (code, message) = refused.unwrap_err();
    assert_eq!(code, "nothing-staged", "{message}");
}

/// The beta channel's releases: each version listed, the newest first.
fn betas_at(releases: &Releases, programs: &Path, versions: &[&str]) {
    let made: Vec<Value> = versions
        .iter()
        .map(|version| {
            releases.release(
                &format!("v{version}"),
                version,
                &standing_for(programs, version),
                None,
            )
        })
        .collect();
    releases.list(&made.iter().collect::<Vec<_>>());
}

/// The channel remembered, as the state folder holds it.
fn kept(user: &User) -> Value {
    let file = user.path("state/flottille/update-channel.json");
    serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap()
}

#[test]
fn choosing_beta_remembers_it_and_checks_for_an_update() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    stable_at(&releases, programs.path(), &this_version());
    betas_at(&releases, programs.path(), &[&format!("{NEWER}-beta.1")]);
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    assert_eq!(
        user.call("update.show", json!({})).unwrap()["channel"],
        "stable"
    );

    let set = user.call("update.set", json!({"channel": "beta"})).unwrap();

    assert_eq!(set["channel"], "beta");
    assert_eq!(kept(&user)["channel"], "beta");
    // The check follows at once, with no click on Check now, and says the beta it found.
    let shown = shown_until(&user, "the check", |shown| {
        shown["latest"] == format!("{NEWER}-beta.1")
    });
    assert_eq!(shown["channel"], "beta");
    assert_eq!(
        shown["check"], "never",
        "the check's setting is not touched"
    );
    // The beta's list was read, not stable's alone.
    assert!(
        requests(&releases)
            .iter()
            .any(|p| p == "/repos/o/r/releases?per_page=100"),
        "{:?}",
        requests(&releases)
    );
}

#[test]
fn back_to_stable_keeps_a_newer_beta_until_a_stable_passes_it() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    // This build is newer than the latest stable, as a beta installed above it is.
    stable_at(&releases, programs.path(), "0.0.5");
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    user.call("update.set", json!({"channel": "beta"})).unwrap();

    user.call("update.set", json!({"channel": "stable"}))
        .unwrap();

    let shown = shown_until(&user, "the check", |shown| shown["ahead_of"] == "0.0.5");
    assert_eq!(shown["channel"], "stable");
    // Nothing is offered, and nothing goes down: the build stays.
    assert_eq!(shown["latest"], Value::Null);
    assert_eq!(kept(&user)["channel"], "stable");
    // A stable above this build ends the wait, and is offered like any update.
    stable_at(&releases, programs.path(), NEWER);
    user.call("update.check", json!({})).unwrap();
    let shown = shown_until(&user, "the newer stable", |shown| shown["latest"] == NEWER);
    assert_eq!(shown["ahead_of"], Value::Null);
}

#[test]
fn an_alpha_build_shows_its_branch_and_offers_stable_or_beta() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    stable_at(&releases, programs.path(), &this_version());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    // `flottille update --channel alpha --branch feat/x` left this behind.
    let file = user.path("state/flottille/update-channel.json");
    // The folder is the daemon's own: private, or it refuses to start.
    std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700)
        .create(file.parent().unwrap())
        .unwrap();
    std::fs::write(&file, r#"{"channel":"alpha","branch":"feat/x"}"#).unwrap();

    let shown = user.call("update.show", json!({})).unwrap();
    assert_eq!(shown["channel"], "alpha");
    assert_eq!(shown["branch"], "feat/x");

    // The way back: stable, and the branch is forgotten with the alpha.
    let back = user
        .call("update.set", json!({"channel": "stable"}))
        .unwrap();
    assert_eq!(back["channel"], "stable");
    assert_eq!(back["branch"], Value::Null);
    assert_eq!(kept(&user)["branch"], Value::Null);
}

#[test]
fn the_window_s_choice_of_channel_is_stable_or_beta_only() {
    let user = User::new();

    for channel in ["alpha", "rc", "next", "nightly"] {
        let refused = user.call("update.set", json!({"channel": channel}));

        let (code, _) = refused.unwrap_err();
        assert_eq!(code, "invalid-params", "{channel}");
    }
    assert_eq!(
        user.call("update.show", json!({})).unwrap()["channel"],
        "stable"
    );
    // A request that asks for nothing is not one.
    assert!(user.call("update.set", json!({})).is_err());
}
