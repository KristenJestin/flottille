//! The program answers to the name it was called by (`docs/cli.md`, "The `flot` name"): a link
//! named `flot` to the one binary behaves as `flottille` does, and its help and errors say `flot`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use common::User;

/// The user's binary under another name, a link in a folder of its own.
fn linked_as(user: &User, name: &str) -> PathBuf {
    std::fs::create_dir_all(user.path("links")).unwrap();
    let link = user.path("links").join(name);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_flottille"), &link).unwrap();
    link
}

fn through(user: &User, link: &Path, args: &[&str]) -> Output {
    user.command_through(link, args).output().unwrap()
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn flot_status_run_through_a_link_named_flot_behaves_as_flottille_status() {
    let user = User::new();
    let flot = linked_as(&user, "flot");

    let through_the_link = through(&user, &flot, &["status", "--json"]);
    let as_flottille = user.run(&["status", "--json"]);

    assert!(
        through_the_link.status.success(),
        "{}",
        stderr_of(&through_the_link)
    );
    let (a, b): (serde_json::Value, serde_json::Value) = (
        serde_json::from_slice(&through_the_link.stdout).unwrap(),
        serde_json::from_slice(&as_flottille.stdout).unwrap(),
    );
    assert_eq!((&a["version"], &a["pid"]), (&b["version"], &b["pid"]));
    let text = through(&user, &flot, &["status"]);
    assert!(stdout_of(&text).starts_with("Flottille "));
}

#[test]
fn the_help_of_a_link_named_flot_names_flot() {
    let user = User::new();
    let flot = linked_as(&user, "flot");

    let help = stdout_of(&through(&user, &flot, &["--help"]));
    let sub = stdout_of(&through(&user, &flot, &["agent", "send", "--help"]));

    assert!(help.contains("Usage: flot "), "{help}");
    assert!(!help.contains("Usage: flottille"), "{help}");
    assert!(sub.contains("Usage: flot agent send"), "{sub}");
    let own = stdout_of(&user.run(&["--help"]));
    assert!(own.contains("Usage: flottille "), "{own}");
}

#[test]
fn the_errors_of_a_link_named_flot_name_flot() {
    let user = User::new();
    let flot = linked_as(&user, "flot");

    let wrong = through(&user, &flot, &["nonsense"]);
    let refused = through(&user, &flot, &["agent", "show", "000000000000"]);
    let own = user.run(&["agent", "show", "000000000000"]);

    assert!(
        stderr_of(&wrong).contains("Usage: flot "),
        "{}",
        stderr_of(&wrong)
    );
    assert!(
        stderr_of(&refused).starts_with("flot: "),
        "{}",
        stderr_of(&refused)
    );
    assert!(
        stderr_of(&own).starts_with("flottille: "),
        "{}",
        stderr_of(&own)
    );
}

#[test]
fn a_daemon_started_through_the_link_is_the_same_daemon() {
    let user = User::new();
    let flot = linked_as(&user, "flot");

    let started = through(&user, &flot, &["status", "--json"]);
    let pid = serde_json::from_slice::<serde_json::Value>(&started.stdout).unwrap()["pid"].clone();

    // `flottille` finds the daemon `flot` started, and `flot` the one `flottille` would.
    assert_eq!(user.status()["pid"], pid);
}
