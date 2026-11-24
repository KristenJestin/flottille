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
    let tool = |description: &str| {
        json!([{"name": "search", "description": description,
        "inputSchema": {"type": "object"}}])
    };
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

#[test]
fn every_line_of_the_log_is_whole_when_requests_come_at_once() {
    // Over HTTP each request has a thread of its own, and each logs a line: two at once must not
    // mix their bytes, or a test that looks for a line (the `stopped` of a reader that went) never
    // finds it.
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.json");
    let log = dir.path().join("log.jsonl");
    let port_file = dir.path().join("port");
    std::fs::write(
        &script,
        json!({"server": {"name": "almanac", "version": "1.0.0"}, "tools": []}).to_string(),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_flottille-fake-mcp"))
        .arg("--script")
        .arg(&script)
        .arg("--log")
        .arg(&log)
        .arg("--http")
        .arg(&port_file)
        .env_clear()
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let port = loop {
        match std::fs::read_to_string(&port_file) {
            Ok(port) if !port.is_empty() => break port,
            _ => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the server never listened"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let (threads, each) = (8, 25);
    let senders: Vec<_> = (0..threads)
        .map(|_| {
            let at = format!("127.0.0.1:{port}");
            std::thread::spawn(move || {
                for _ in 0..each {
                    let mut stream = std::net::TcpStream::connect(&at).unwrap();
                    let request = "GET /mcp HTTP/1.1\r\nHost: localhost\r\nX-Filler: \
                                   a-header-long-enough-to-take-a-few-writes\r\n\r\n";
                    stream.write_all(request.as_bytes()).unwrap();
                    let mut answer = String::new();
                    let _ = std::io::Read::read_to_string(&mut stream, &mut answer);
                }
            })
        })
        .collect();
    for sender in senders {
        sender.join().unwrap();
    }
    let _ = child.kill();
    let _ = child.wait();

    let written = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    let broken: Vec<&&str> = lines
        .iter()
        .filter(|line| serde_json::from_str::<Value>(line).is_err())
        .collect();
    assert!(
        broken.is_empty(),
        "{} of {} lines are not whole: {:?}",
        broken.len(),
        lines.len(),
        broken.first()
    );
    assert_eq!(
        lines.len(),
        threads * each + 1,
        "a start and one line a request"
    );
}
