//! `flottille doctor`, with a fake PATH: no real `claude` nor `opencode` is ever run.

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

fn opencode(user: &User, version: &str) {
    write_script(
        &user.path("bin/opencode"),
        &format!("#!/bin/sh\n[ \"$1\" = --version ] && echo '{version}'\n"),
    );
}

/// Claude's entry of the report.
fn claude_of(report: &Value) -> &Value {
    &report["providers"][0]
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
    let claude = claude_of(&report);
    assert_eq!(claude["name"], "claude");
    assert_eq!(
        claude["program"]["path"],
        user.path("bin/claude").to_str().unwrap()
    );
    assert_eq!(claude["program"]["version"], "2.1.295");
    assert_eq!(claude["program"]["checked"], "2.1.295");
    assert_eq!(claude["login"]["present"], true);
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
    assert_eq!(claude_of(&report)["login"]["present"], false);
    assert_eq!(claude_of(&report)["login"]["api_key"], "ANTHROPIC_API_KEY");
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
    let claude = claude_of(&report);
    assert_eq!(claude["ready"], false);
    assert_eq!(claude["program"]["present"], false);
    assert_eq!(claude["login"]["present"], false);
    assert_eq!(claude["login"]["api_key"], Value::Null);
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

    assert_eq!(claude_of(&report)["login"]["present"], true);
    assert_eq!(
        claude_of(&report)["login"]["path"],
        dir.join(".credentials.json").to_str().unwrap()
    );
}

#[test]
fn doctor_reports_claude_and_opencode_each_enabled_or_not_ready_or_not_by_login_or_by_key() {
    // Claude by its login file; OpenCode enabled, on PATH, signed in by an API key.
    let user = user_with_bin().with_env("OPENROUTER_API_KEY", "sk-or-invented-0123456789");
    claude(&user, "2.1.295");
    login(&user);
    opencode(&user, "1.19.0");
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/providers.toml"),
        "[opencode]\nenabled = true\n",
    )
    .unwrap();

    let (ok, report, text) = doctor(&user);

    assert!(ok, "{report}");
    let opencode = &report["providers"][1];
    assert_eq!(opencode["name"], "opencode");
    assert_eq!(opencode["enabled"], true);
    assert_eq!(opencode["ready"], true);
    assert_eq!(opencode["program"]["version"], "1.19.0");
    assert_eq!(opencode["program"]["checked"], "1.19.0");
    assert_eq!(opencode["login"]["present"], false);
    assert_eq!(opencode["login"]["api_key"], "OPENROUTER_API_KEY");
    assert_eq!(claude_of(&report)["login"]["present"], true);
    assert!(!report.to_string().contains("invented"), "{report}");
    assert!(!text.contains("invented"), "{text}");
    assert!(
        text.contains("OpenCode") && text.contains("OPENROUTER_API_KEY"),
        "{text}"
    );

    // OpenCode by its login file, which is never read.
    let file = user.path("data/opencode/auth.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "invented").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    let user = user.with_env("OPENROUTER_API_KEY", "");
    let (_, report, _) = doctor(&user);
    let login = &report["providers"][1]["login"];
    assert_eq!(login["present"], true, "{report}");
    assert_eq!(login["path"], file.to_str().unwrap());
    assert_eq!(login["api_key"], Value::Null);

    // Neither: enabled and not ready, which fails the doctor; Claude still can run.
    std::fs::remove_file(&file).unwrap();
    let (ok, report, text) = doctor(&user);
    assert!(!ok, "{report}");
    assert_eq!(report["providers"][1]["ready"], false);
    assert!(
        report["providers"][1]["why"]
            .as_str()
            .unwrap()
            .contains("sign in"),
        "{report}"
    );
    assert_eq!(claude_of(&report)["ready"], true);
    assert!(text.contains("OpenCode agents cannot run"), "{text}");
    assert!(text.contains("Claude agents can run"), "{text}");

    // Disabled: reported, and not a failure.
    std::fs::write(
        user.path("config/flottille/providers.toml"),
        "[opencode]\nenabled = false\n",
    )
    .unwrap();
    let (ok, report, _) = doctor(&user);
    assert!(ok, "{report}");
    assert_eq!(report["providers"][1]["enabled"], false);
    assert_eq!(report["providers"][1]["ready"], false);
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
