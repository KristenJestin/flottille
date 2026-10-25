//! `flottille doctor`, with a fake PATH: no real `claude` is ever run.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::User;
use serde_json::Value;

fn write_script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A user whose PATH holds only `bin`: none of the machine's programs is found.
fn user_with_bin() -> User {
    let user = User::new();
    std::fs::create_dir(user.path("bin")).unwrap();
    let path = user.path("bin");
    user.with_env("PATH", path.to_str().unwrap())
}

fn claude(user: &User, version: &str) {
    write_script(
        &user.path("bin/claude"),
        &format!("#!/bin/sh\n[ \"$1\" = --version ] && echo '{version} (Claude Code)'\n"),
    );
}

fn login(user: &User) {
    // A login file Flottille could not read even if it tried: only its presence counts.
    let login = user.path("home/.claude/.credentials.json");
    std::fs::create_dir_all(login.parent().unwrap()).unwrap();
    std::fs::write(&login, "invented").unwrap();
    std::fs::set_permissions(&login, std::fs::Permissions::from_mode(0o000)).unwrap();
}

fn doctor(user: &User) -> (bool, Value, String) {
    let output = user.run(&["doctor", "--json"]);
    let text = String::from_utf8_lossy(&user.run(&["doctor"]).stdout).into_owned();
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap(),
        text,
    )
}

#[test]
fn doctor_reports_claude_its_version_and_its_login() {
    let user = user_with_bin();
    claude(&user, "2.1.295");
    login(&user);

    let (ok, report, text) = doctor(&user);

    assert!(ok, "{report}");
    assert_eq!(
        report["claude"]["path"],
        user.path("bin/claude").to_str().unwrap()
    );
    assert_eq!(report["claude"]["version"], "2.1.295");
    assert_eq!(report["claude"]["checked"], "2.1.295");
    assert_eq!(report["login"]["present"], true);
    assert_eq!(report["warnings"], serde_json::json!([]));
    assert_eq!(report["ready"], true);
    assert!(text.contains("2.1.295"), "{text}");
    for gone in ["Node", "node", "npm", "adapter"] {
        assert!(!text.contains(gone), "{gone}: {text}");
        assert!(!report.to_string().contains(gone), "{gone}: {report}");
    }
}

#[test]
fn doctor_warns_when_claude_is_not_the_version_flottille_was_checked_against() {
    let user = user_with_bin();
    claude(&user, "2.1.300");
    login(&user);

    let (ok, report, text) = doctor(&user);

    assert!(ok, "a warning, not a failure: {report}");
    let warning = report["warnings"][0].as_str().unwrap();
    assert!(
        warning.contains("2.1.300") && warning.contains("2.1.295"),
        "{warning}"
    );
    assert!(
        text.contains("2.1.300") && text.contains("2.1.295"),
        "{text}"
    );
}

#[test]
fn doctor_takes_an_api_key_for_a_login_and_never_prints_it() {
    let user = user_with_bin().with_env("ANTHROPIC_API_KEY", "sk-ant-invented-0123456789");
    claude(&user, "2.1.295");

    let (ok, report, text) = doctor(&user);

    assert!(ok, "{report}");
    assert_eq!(report["login"]["present"], false);
    assert_eq!(report["login"]["api_key"], true);
    assert!(!report.to_string().contains("invented"), "{report}");
    assert!(!text.contains("invented"), "{text}");
    assert!(text.contains("ANTHROPIC_API_KEY"), "{text}");
}

#[test]
fn doctor_says_what_is_missing() {
    let user = user_with_bin();

    let (ok, report, text) = doctor(&user);

    assert!(!ok);
    assert_eq!(report["ready"], false);
    assert_eq!(report["claude"]["present"], false);
    assert_eq!(report["login"]["present"], false);
    assert_eq!(report["login"]["api_key"], false);
    assert!(text.contains("claude"), "{text}");
    assert!(!text.contains("flottille agent install"), "{text}");
}

#[test]
fn doctor_finds_the_login_in_claude_config_dir() {
    let user = user_with_bin();
    let dir = user.path("claude-config");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(".credentials.json"), "invented").unwrap();
    let user = user.with_env("CLAUDE_CONFIG_DIR", dir.to_str().unwrap());

    let (_, report, _) = doctor(&user);

    assert_eq!(report["login"]["present"], true);
    assert_eq!(
        report["login"]["path"],
        dir.join(".credentials.json").to_str().unwrap()
    );
}

#[test]
fn there_is_no_agent_install_any_more() {
    let user = user_with_bin();

    let output = user.run(&["agent", "install"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unrecognized subcommand 'install'"),
        "{stderr}"
    );
}
