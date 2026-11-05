//! The scenarios of `web {action: "show_link"}` (`docs/tools.md`, "Showing a link"): an agent
//! hands the user a link, the daemon checks it and says it in an event, and opens nothing. The
//! agent is a fake ACP agent, in throwaway repositories and temporary XDG folders; the programs
//! that open a link on a desktop are fakes first on `PATH`, and none of them may have run.

mod common;

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// A root agent in project `app`'s folder: its id and its session.
fn session(user: &User) -> (String, Mcp) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization))
}

fn user() -> User {
    User::with_fake_agent().with_fake_launchers()
}

fn shown(user: &User, agent: &str) -> Vec<Value> {
    user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["name"] == "agent.link_shown")
        .cloned()
        .collect()
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    assert!(answer.text.starts_with("refused: "), "{}", answer.text);
    answer.text
}

fn link(url: &str) -> Value {
    json!({"action": "show_link", "url": url})
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn show_link_says_the_link_in_an_event_and_opens_nothing() {
    let user = user();
    let (agent, mcp) = session(&user);

    let said = done(mcp.call(
        "web",
        json!({"action": "show_link", "url": "https://example.org/t/42?x=1#top",
            "title": "Ticket 42"}),
    ));

    assert!(said.contains("Shown to the user"), "{said}");
    let events = shown(&user, &agent);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0]["data"],
        json!({"agent": agent, "kind": "url", "target": "https://example.org/t/42?x=1#top",
            "title": "Ticket 42", "host": "example.org"})
    );
    assert!(user.launches().is_empty(), "{:?}", user.launches());
}

#[test]
fn show_link_without_a_title_is_named_by_its_host() {
    let user = user();
    let (agent, mcp) = session(&user);

    let said = done(mcp.call("web", link("http://localhost:3000/")));

    assert!(said.contains("localhost"), "{said}");
    let data = &shown(&user, &agent)[0]["data"];
    assert_eq!(data["title"], Value::Null);
    assert_eq!(data["host"], "localhost");
}

#[test]
fn show_link_refuses_a_non_http_url() {
    let user = user();
    let (agent, mcp) = session(&user);

    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "ftp://example.org/x",
        "ssh://example.org",
        "mailto:someone@example.org",
        "https://",
        "example.org/page",
        "not a url",
    ] {
        let why = refused(mcp.call("web", link(url)));
        assert!(why.contains("nothing was shown"), "{url}: {why}");
    }
    assert!(shown(&user, &agent).is_empty());
}

#[test]
fn show_link_takes_one_of_a_url_and_a_path() {
    let user = user();
    let (agent, mcp) = session(&user);

    refused(mcp.call("web", json!({"action": "show_link"})));
    refused(mcp.call(
        "web",
        json!({"action": "show_link", "url": "https://example.org", "path": "README.md"}),
    ));
    refused(mcp.call(
        "web",
        json!({"action": "show_link", "url": "https://example.org", "title": "x".repeat(500)}),
    ));
    assert!(shown(&user, &agent).is_empty());
}

#[test]
fn show_link_shows_a_file_of_the_place() {
    let user = user();
    let (agent, mcp) = session(&user);
    let folder = user.path("work/app");
    std::fs::create_dir(folder.join("docs")).unwrap();
    std::fs::write(folder.join("docs/report.html"), "<p>report</p>").unwrap();

    done(mcp.call(
        "web",
        json!({"action": "show_link", "path": "docs/../docs/report.html", "title": "Report"}),
    ));

    let data = &shown(&user, &agent)[0]["data"];
    assert_eq!(data["kind"], "file");
    assert_eq!(data["shown"], "docs/report.html");
    assert_eq!(data["title"], "Report");
    let target = std::fs::canonicalize(folder.join("docs/report.html")).unwrap();
    assert_eq!(data["target"], target.display().to_string());
    assert!(user.launches().is_empty());
}

#[test]
fn show_link_refuses_a_path_outside_the_place_and_says_why() {
    let user = user();
    let (agent, mcp) = session(&user);
    let folder = user.path("work/app");
    let outside = user.path("work/secret.txt");
    std::fs::write(&outside, "outside\n").unwrap();
    std::os::unix::fs::symlink(&outside, folder.join("shortcut.txt")).unwrap();
    std::fs::create_dir(folder.join("docs")).unwrap();

    for path in [
        "../secret.txt",
        "docs/../../secret.txt",
        outside.to_str().unwrap(),
        "shortcut.txt",
        "/etc/hostname",
    ] {
        let why = refused(mcp.call("web", json!({"action": "show_link", "path": path})));
        assert!(why.contains("not inside your place"), "{path}: {why}");
    }
    // Inside, but not there or not a file.
    let why = refused(mcp.call("web", json!({"action": "show_link", "path": "missing.md"})));
    assert!(why.contains("not a file"), "{why}");
    let why = refused(mcp.call("web", json!({"action": "show_link", "path": "docs"})));
    assert!(why.contains("not a file"), "{why}");
    assert!(shown(&user, &agent).is_empty());
}

#[test]
fn show_link_passes_the_gate_in_ask_auto_and_full_without_a_request() {
    let user = user();
    let (agent, mcp) = session(&user);

    for mode in ["ask", "auto", "full"] {
        set_mode(&user, &agent, mode);
        done(mcp.call("web", link("https://example.org/a")));
    }

    assert_eq!(shown(&user, &agent).len(), 3);
    let requests = user.call("approval.list", json!({})).unwrap();
    assert_eq!(requests["requests"].as_array().map_or(0, Vec::len), 0);
}

#[test]
fn run_refuses_xdg_open_and_points_to_show_link() {
    let user = user();
    let (agent, mcp) = session(&user);

    for line in [
        "xdg-open https://example.org",
        "open https://example.org",
        "start https://example.org",
        "nice -n 5 xdg-open https://example.org",
    ] {
        let why = refused(mcp.call("run", json!({"line": line})));
        assert!(why.contains("show_link"), "{line}: {why}");
    }
    assert!(user.launches().is_empty(), "{:?}", user.launches());
    assert!(shown(&user, &agent).is_empty());
}

/// A file of the place with these bytes and this mode.
fn written(user: &User, name: &str, text: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let path = user.path("work/app").join(name);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_shown_executable_file_is_refused() {
    let user = user();
    let (agent, mcp) = session(&user);
    // No extension a desktop runs: the execute bit alone makes it a program.
    for mode in [0o755, 0o700, 0o644 | 0o010, 0o644 | 0o001] {
        written(&user, "notes", "#!/bin/sh\necho hi\n", mode);
        let why = refused(mcp.call("web", json!({"action": "show_link", "path": "notes"})));
        assert!(why.contains("executable"), "{mode:o}: {why}");
    }
    assert!(shown(&user, &agent).is_empty());
    assert!(user.launches().is_empty());
}

#[test]
fn a_shown_desktop_entry_is_refused() {
    let user = user();
    let (agent, mcp) = session(&user);
    let entry = "[Desktop Entry]\nType=Application\nName=Report\nExec=touch /tmp/pwned\n";
    for name in ["report.desktop", "Report.DESKTOP"] {
        written(&user, name, entry, 0o644);
        let why = refused(mcp.call("web", json!({"action": "show_link", "path": name})));
        assert!(why.contains("a desktop runs"), "{name}: {why}");
    }
    assert!(shown(&user, &agent).is_empty());
}

#[test]
fn a_shown_script_by_extension_is_refused() {
    let user = user();
    let (agent, mcp) = session(&user);
    for name in [
        "build.sh",
        "run.py",
        "Tool.AppImage",
        "app.JAR",
        "setup.exe",
        "go.command",
        "x.bash",
        "x.zsh",
        "x.fish",
        "x.pl",
        "x.rb",
        "x.js",
        "x.run",
        "x.bin",
        "x.msi",
        "x.bat",
        "x.cmd",
        "x.ps1",
        "x.app",
        "x.deb",
        "x.rpm",
        "x.flatpakref",
    ] {
        written(&user, name, "data\n", 0o644);
        let why = refused(mcp.call("web", json!({"action": "show_link", "path": name})));
        assert!(why.contains("a desktop runs"), "{name}: {why}");
    }
    assert!(shown(&user, &agent).is_empty());
}

#[test]
fn a_shown_document_still_opens() {
    let user = user();
    let (agent, mcp) = session(&user);
    for name in ["report.html", "notes.md", "plot.png", "Makefile"] {
        written(&user, name, "data\n", 0o644);
        done(mcp.call("web", json!({"action": "show_link", "path": name})));
    }
    assert_eq!(shown(&user, &agent).len(), 4);
}

#[test]
fn a_shown_fifo_is_refused() {
    let user = user();
    let (agent, mcp) = session(&user);
    let fifo = user.path("work/app/pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let why = refused(mcp.call("web", json!({"action": "show_link", "path": "pipe"})));
    assert!(why.contains("not a file"), "{why}");
    assert!(shown(&user, &agent).is_empty());
}
