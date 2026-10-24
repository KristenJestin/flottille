//! The fake agent does what its script says, over ACP on stdio.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

struct Fake {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<Value>,
}

impl Fake {
    fn start(dir: &Path) -> Fake {
        let mut child = Command::new(env!("CARGO_BIN_EXE_flottille-fake-agent"))
            .env("FLOTTILLE_FAKE_AGENT_DIR", dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let _ = sender.send(serde_json::from_str(&line).unwrap());
            }
        });
        Fake {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
    }

    /// Sends a request; the messages before its answer, and its answer.
    fn request(&mut self, id: u64, method: &str, params: Value) -> (Vec<Value>, Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut before = Vec::new();
        loop {
            let message = self.lines.recv_timeout(Duration::from_secs(10)).unwrap();
            if message["id"] == id && message.get("method").is_none() {
                return (before, message);
            }
            before.push(message);
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn received(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("received.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn an_mcp_step_calls_the_tool_on_the_session_server() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"turns": [{"when": "list", "do": [{"mcp": {"tool": "fs_list", "arguments": {"path": "."}}}]}]})
            .to_string(),
    )
    .unwrap();
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", server.local_addr().unwrap());
    let (seen, requests) = mpsc::channel();
    std::thread::spawn(move || {
        for (n, stream) in server.incoming().enumerate() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let _ = seen.send((head, String::from_utf8(body).unwrap()));
            let answer = json!({"jsonrpc": "2.0", "id": n + 1, "result": {"content": [{"type": "text", "text": "a.txt"}]}});
            let answer = format!("data: {answer}\n\n");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nMcp-Session-Id: s-1\r\n\
                 Content-Length: {}\r\n\r\n{answer}",
                answer.len()
            )
            .unwrap();
        }
    });
    let mut fake = Fake::start(dir.path());
    fake.request(1, "initialize", json!({"protocolVersion": 1}));
    let servers = json!([{"type": "http", "name": "flottille", "url": url,
        "headers": [{"name": "Authorization", "value": "Bearer test-token"}]}]);
    let (_, opened) = fake.request(2, "session/new", json!({"cwd": "/", "mcpServers": servers}));
    let session = opened["result"]["sessionId"].clone();

    let (updates, answer) = fake.request(
        3,
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": "list the files"}]}),
    );

    assert_eq!(answer["result"]["stopReason"], "end_turn");
    let (head, body) = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(head.contains("Authorization: Bearer test-token"), "{head}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["method"],
        "initialize"
    );
    requests.recv_timeout(Duration::from_secs(5)).unwrap();
    let (head, body) = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(head.contains("Mcp-Session-Id: s-1"), "{head}");
    let call: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        call["params"],
        json!({"name": "fs_list", "arguments": {"path": "."}})
    );
    assert_eq!(
        updates.last().unwrap()["params"]["update"]["status"],
        "completed"
    );
    let recorded = received(dir.path());
    let mcp = recorded.iter().find(|e| e.get("mcp").is_some()).unwrap();
    assert_eq!(mcp["mcp"]["body"]["result"]["content"][0]["text"], "a.txt");
}

#[test]
fn a_loaded_session_replays_its_history_and_a_resumed_one_does_not() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("script.json"),
        json!({"turns": [{"when": "hello", "do": [{"say": "Hello!"}]}]}).to_string(),
    )
    .unwrap();
    let session = {
        let mut fake = Fake::start(dir.path());
        fake.request(1, "initialize", json!({"protocolVersion": 1}));
        let (_, opened) = fake.request(2, "session/new", json!({"cwd": "/", "mcpServers": []}));
        let session = opened["result"]["sessionId"].clone();
        fake.request(
            3,
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": "hello"}]}),
        );
        session
    };

    let mut fake = Fake::start(dir.path());
    fake.request(1, "initialize", json!({"protocolVersion": 1}));
    let (replayed, _) = fake.request(
        2,
        "session/load",
        json!({"sessionId": session, "cwd": "/", "mcpServers": []}),
    );
    let (resumed, answer) = fake.request(
        3,
        "session/resume",
        json!({"sessionId": session, "cwd": "/"}),
    );

    let kinds: Vec<&str> = replayed
        .iter()
        .map(|m| m["params"]["update"]["sessionUpdate"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["user_message_chunk", "agent_message_chunk"]);
    assert!(resumed.is_empty());
    assert!(answer.get("result").is_some(), "{answer}");
}
