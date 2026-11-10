//! The fake MCP server does what its script says, on stdio. This test is also what makes a plain
//! `cargo test` (or `cargo nextest run --workspace`) at the workspace's root build its binary,
//! which the daemon's connection tests run: cargo builds a package's binary for its own
//! integration tests only.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

#[test]
fn a_change_notice_comes_when_its_file_appears_and_the_listing_is_read_again() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.json");
    let trigger = dir.path().join("notify");
    let tool = |description: &str| json!([{"name": "search", "description": description,
        "inputSchema": {"type": "object"}}]);
    std::fs::write(
        &script,
        json!({"server": {"name": "almanac", "version": "1.0.0"}, "tools": tool("Finds."),
            "notify": {"when": trigger, "times": 2}})
        .to_string(),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_flottille-fake-mcp"))
        .arg("--script")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, lines) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let _ = sender.send(serde_json::from_str(&line).unwrap());
        }
    });
    let next = || lines.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut send = |message: Value| writeln!(stdin, "{message}").unwrap();

    send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}));
    assert_eq!(next()["result"]["tools"], tool("Finds."));

    std::fs::write(
        &script,
        json!({"server": {"name": "almanac", "version": "1.0.0"}, "tools": tool("Mails.")})
            .to_string(),
    )
    .unwrap();
    std::fs::write(&trigger, "").unwrap();
    for _ in 0..2 {
        assert_eq!(next()["method"], "notifications/tools/list_changed");
    }
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}));
    assert_eq!(next()["result"]["tools"], tool("Mails."));

    let _ = child.kill();
    let _ = child.wait();
}
