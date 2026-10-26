//! The scenarios of the daemon's skeleton, against the real binary, with temporary XDG folders.

mod common;

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::time::Duration;

use common::{User, mode, processes_of};

#[test]
fn the_first_command_starts_a_daemon() {
    let user = User::new();
    assert!(!user.path("runtime/flottille/daemon.sock").exists());

    user.status();

    assert!(user.path("runtime/flottille/daemon.sock").exists());
    assert!(user.path("state/flottille/flottille.db").exists());
}

#[test]
fn the_folders_are_created_for_the_user_only() {
    let user = User::new();
    user.status();

    assert_eq!(mode(user.path("runtime/flottille")), 0o700);
    assert_eq!(mode(user.path("state/flottille")), 0o700);
}

#[test]
fn a_folder_open_to_others_is_refused() {
    let user = User::new();
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(user.path("runtime/flottille"))
        .unwrap();
    std::fs::set_permissions(
        user.path("runtime/flottille"),
        PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let output = user.run(&["daemon"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("other users"),
        "unexpected refusal: {stderr}"
    );
}

#[test]
fn a_second_daemon_refuses_to_start() {
    let user = User::new();
    let pid = user.status()["pid"].as_u64().unwrap();

    let output = user.run(&["daemon"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already running"),
        "unexpected refusal: {stderr}"
    );
    assert!(
        stderr.contains(&pid.to_string()),
        "the refusal names the running daemon: {stderr}"
    );
    assert_eq!(
        user.status()["pid"].as_u64().unwrap(),
        pid,
        "the first daemon still answers"
    );
}

#[test]
fn status_json_answers() {
    let user = User::new();

    let status = user.status();

    assert_eq!(
        status["version"].as_str().unwrap(),
        version_of_the_binary(&user)
    );
    assert!(status["uptime_s"].is_u64());
    assert!(status["pid"].is_u64());
    assert_eq!(status["agents"]["running"], 0);
    assert_eq!(status["agents"]["waiting_for_user"], 0);
    assert_eq!(status["agents"]["failed"], 0);
}

#[test]
fn status_without_json_is_read_by_a_person() {
    let user = User::new();

    let output = user.run(&["status"]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 running, 0 waiting for you, 0 failed"),
        "unexpected status: {stdout}"
    );
}

#[test]
fn version_prints_the_full_version() {
    let user = User::new();

    let version = version_of_the_binary(&user);

    assert!(
        version.starts_with(env!("CARGO_PKG_VERSION")),
        "unexpected version: {version}"
    );
}

fn version_of_the_binary(user: &User) -> String {
    let output = user.run(&["--version"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout.trim().strip_prefix("flottille ").unwrap().to_owned()
}

#[test]
fn watch_prints_an_event_emitted_by_a_request_and_resumes_after_it() {
    let user = User::new();
    let watch = user.watch(&["--after", "0"]);
    let started = watch.next_event();
    assert_eq!(started["name"], "daemon.started");
    let started_seq = started["seq"].as_u64().unwrap();

    // `daemon stop` is a request; the daemon tells it before it goes.
    assert!(user.run(&["daemon", "stop"]).status.success());
    let stopping = watch.next_event();
    assert_eq!(stopping["name"], "daemon.stopping");
    assert_eq!(stopping["data"]["reason"], "request");
    assert!(stopping["seq"].as_u64().unwrap() > started_seq);
    drop(watch);

    // Resuming after the first event gives what came after it, from the store, then what is new.
    let resumed = user.watch(&["--after", &started_seq.to_string()]);
    assert_eq!(resumed.next_event(), stopping);
    let restarted = resumed.next_event();
    assert_eq!(restarted["name"], "daemon.started");
    assert!(restarted["seq"].as_u64().unwrap() > stopping["seq"].as_u64().unwrap());
}

#[test]
fn watch_without_after_prints_only_what_is_new() {
    let user = User::new();
    user.status();
    let watch = user.watch(&[]);
    // The subscription is answered before anything is printed; give it a moment to be in place.
    std::thread::sleep(Duration::from_secs(1));

    assert!(user.run(&["daemon", "stop"]).status.success());

    assert_eq!(watch.next_event()["name"], "daemon.stopping");
}

#[test]
fn a_stopped_daemon_is_restarted_by_the_next_command() {
    let user = User::new();
    let first = user.status()["pid"].as_u64().unwrap();

    let stop = user.run(&["daemon", "stop"]);
    assert!(stop.status.success());
    assert!(!user.path("runtime/flottille/daemon.sock").exists());

    let second = user.status()["pid"].as_u64().unwrap();
    assert_ne!(first, second);
}

#[test]
fn stopping_when_no_daemon_runs_starts_none() {
    let user = User::new();

    let output = user.run(&["daemon", "stop"]);

    assert!(output.status.success());
    assert!(!user.path("runtime/flottille/daemon.sock").exists());
}

/// Waits up to `seconds` for `done`, polling.
fn within(seconds: u64, what: &str, done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "not within {seconds} s: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_dropped_user_leaves_no_daemon_and_no_folder_whatever_client_it_handed_out() {
    let user = User::new();
    let folder = user.path("");
    // A client the test still holds after the user, as the window's link is.
    let connect = user.connector();
    connect().unwrap();
    assert!(!processes_of(&folder).is_empty());

    drop(user);

    assert!(
        connect().is_err(),
        "a client of a dropped user started a daemon again"
    );
    assert!(!folder.exists(), "the folder came back");
    assert!(
        processes_of(&folder).is_empty(),
        "a process of the dropped user is alive"
    );
}

#[test]
fn a_daemon_whose_runtime_folder_was_removed_exits() {
    let user = User::new();
    let pid = user.status()["pid"].as_u64().unwrap();
    let alive = || std::path::Path::new(&format!("/proc/{pid}")).exists();

    std::fs::remove_dir_all(user.path("runtime")).unwrap();

    within(10, "the daemon exits", || !alive());
}

#[test]
fn a_daemon_whose_socket_leads_to_another_daemon_exits_and_leaves_that_one_alone() {
    let user = User::new();
    let first = user.status()["pid"].as_u64().unwrap();
    let alive = |pid: u64| std::path::Path::new(&format!("/proc/{pid}")).exists();

    // The runtime folder is replaced under it, and a second daemon takes its place.
    std::fs::remove_dir_all(user.path("runtime")).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(user.path("runtime"))
        .unwrap();
    let second = user.status()["pid"].as_u64().unwrap();
    assert_ne!(first, second);

    within(10, "the first daemon exits", || !alive(first));
    assert!(alive(second));
    assert_eq!(user.status()["pid"].as_u64().unwrap(), second);
}
