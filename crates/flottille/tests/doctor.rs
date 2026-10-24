//! `flottille doctor` and `flottille agent install`, with a fake PATH: no real `node`, `npm` or
//! `claude` is ever run.

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

fn doctor(user: &User) -> (bool, Value) {
    let output = user.run(&["doctor", "--json"]);
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

#[test]
fn doctor_reports_node_claude_its_login_and_the_adapter() {
    let user = user_with_bin();
    write_script(&user.path("bin/node"), "#!/bin/sh\necho v24.11.0\n");
    write_script(
        &user.path("bin/claude"),
        "#!/bin/sh\n[ \"$1\" = --version ] && echo '2.1.300 (Claude Code)'\n",
    );
    // A login file Flottille could not read even if it tried: only its presence counts.
    let login = user.path("home/.claude/.credentials.json");
    std::fs::create_dir_all(login.parent().unwrap()).unwrap();
    std::fs::write(&login, "invented").unwrap();
    std::fs::set_permissions(&login, std::fs::Permissions::from_mode(0o000)).unwrap();
    let package = user
        .path("data/flottille/adapters/claude/node_modules/@agentclientprotocol/claude-agent-acp");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name": "@agentclientprotocol/claude-agent-acp", "version": "0.95.0"}"#,
    )
    .unwrap();

    let (ok, report) = doctor(&user);

    assert!(ok, "{report}");
    assert_eq!(report["node"]["version"], "24.11.0");
    assert_eq!(
        report["claude"]["path"],
        user.path("bin/claude").to_str().unwrap()
    );
    assert_eq!(report["claude"]["version"], "2.1.300");
    assert_eq!(report["login"]["present"], true);
    assert_eq!(report["adapter"]["version"], "0.95.0");
    assert_eq!(report["adapter"]["pinned"], "0.95.0");
    assert_eq!(report["ready"], true);
    let text = String::from_utf8_lossy(&user.run(&["doctor"]).stdout).into_owned();
    assert!(text.contains("2.1.300"), "{text}");
}

#[test]
fn doctor_says_what_is_missing() {
    let user = user_with_bin();

    let (ok, report) = doctor(&user);

    assert!(!ok);
    assert_eq!(report["ready"], false);
    assert_eq!(report["node"]["present"], false);
    assert_eq!(report["claude"]["present"], false);
    assert_eq!(report["login"]["present"], false);
    assert_eq!(report["adapter"]["present"], false);
    let text = String::from_utf8_lossy(&user.run(&["doctor"]).stdout).into_owned();
    assert!(text.contains("flottille agent install"), "{text}");
}

#[test]
fn doctor_finds_the_login_in_claude_config_dir() {
    let user = user_with_bin();
    let dir = user.path("claude-config");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(".credentials.json"), "invented").unwrap();
    let user = user.with_env("CLAUDE_CONFIG_DIR", dir.to_str().unwrap());

    let (_, report) = doctor(&user);

    assert_eq!(report["login"]["present"], true);
    assert_eq!(
        report["login"]["path"],
        dir.join(".credentials.json").to_str().unwrap()
    );
}

#[test]
fn the_adapter_is_installed_with_npm_at_the_pinned_version_into_the_data_folder() {
    let user = user_with_bin();
    let args = user.path("npm-args");
    write_script(
        &user.path("bin/npm"),
        &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n", args.display()),
    );
    write_script(&user.path("bin/npx"), "#!/bin/sh\nexit 99\n");

    let output = user.run(&["agent", "install"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let args = std::fs::read_to_string(args).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let prefix = user.path("data/flottille/adapters/claude");
    assert_eq!(args[0], "install");
    assert!(args.contains(&"--prefix"), "{args:?}");
    assert!(args.contains(&prefix.to_str().unwrap()), "{args:?}");
    assert_eq!(
        args.last().unwrap(),
        &"@agentclientprotocol/claude-agent-acp@0.95.0"
    );
}
