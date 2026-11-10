//! A scripted MCP server on stdio, for the tests of Flottille's connections (`docs/extensions.md`,
//! "Connections"): newline-delimited JSON-RPC, as the stdio transport is, written by hand so that
//! a test can make it do what a well-behaved server does not (ask the client for sampling, change
//! its tools while it runs).
//!
//! ```text
//! flottille-fake-mcp --script <file> [--log <file>] [--child] [--http <port file>] [other
//! arguments, logged]
//! ```
//!
//! With `--http`, it serves Streamable HTTP on a free port of `127.0.0.1` instead of stdio,
//! written to that file, each request answered with one JSON body (no stream, so no request of
//! its own to the client), and logs each request's headers (`http`).
//!
//! The script, read at the start and again when an answer says `reload`:
//!
//! ```json
//! {"server": {"name": "almanac", "version": "1.0.0"},
//!  "instructions": "Search before you write.",
//!  "tools": [{"name": "search", "description": "…", "inputSchema": {…}, "annotations": {…}}],
//!  "answers": {"search": {"text": "found it"}},
//!  "stderr": "a line written to the error output at the start",
//!  "exit": 3,
//!  "notify": {"when": "<file>", "times": 50},
//!  "list_sleep_ms": 100,
//!  "endless_listing": true,
//!  "list_error": "the text of a JSON-RPC error answered to every listing"}
//! ```
//!
//! On stdio, `tools/list` reads the script again: a test changes the tools of a running server
//! by writing it. `notify` says its tools changed `times` times, at once, when the file `when`
//! appears; `list_sleep_ms` slows each listing; `endless_listing` answers every page with a next
//! one; `list_error` fails each listing with that text.
//!
//! An answer is, in this order of precedence, any of: `sleep_ms` (waits first), `ask`
//! (`"sampling"`, `"elicitation"` or `"roots"`: sends that request to the client and answers with
//! what it got back), `reload` (reads the script again and says its tools changed), `env` (the
//! value of that variable), `echo` (the arguments, as JSON), `text`; `image` adds a picture after
//! the text, `error` answers that text as an error. `rpc_error` answers that text as a JSON-RPC
//! error instead, on stdio. With no answer, `ok`.
//!
//! The log, one JSON line each: `start` (its pid, process group, session, folder and arguments,
//! the child's pid with `--child`), `initialize` (the client's capabilities), `list` (the cursor
//! asked), `call` (the tool, the arguments, when it started and ended, in milliseconds since the
//! epoch).

use std::io::{BufRead, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let option = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let script_path = option("--script").expect("--script <file>");
    let log = option("--log");
    let mut script = read(&script_path);
    let child = args.iter().any(|arg| arg == "--child").then(|| {
        std::process::Command::new("sleep")
            .arg("1000")
            .spawn()
            .expect("a child")
            .id()
    });
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().collect())
        .unwrap_or_default();
    write_log(
        &log,
        json!({"event": "start", "pid": std::process::id(),
            "pgid": fields.get(2).and_then(|f| f.parse::<u32>().ok()),
            "sid": fields.get(3).and_then(|f| f.parse::<u32>().ok()),
            "cwd": std::env::current_dir().ok(), "args": args, "child": child}),
    );
    if let Some(line) = script["stderr"].as_str() {
        eprintln!("{line}");
    }
    if let Some(code) = script["exit"].as_i64() {
        std::process::exit(code as i32);
    }
    if let Some(port_file) = option("--http") {
        return http(&script, &log, &port_file);
    }
    if let Some(when) = script["notify"]["when"].as_str() {
        let (when, times) = (
            when.to_owned(),
            script["notify"]["times"].as_u64().unwrap_or(1),
        );
        std::thread::spawn(move || {
            while !std::path::Path::new(&when).exists() {
                std::thread::sleep(Duration::from_millis(20));
            }
            for _ in 0..times {
                send(&json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}));
            }
        });
    }
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut asked = 0u64;
    while let Some(Ok(line)) = lines.next() {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(method) = message["method"].as_str() else {
            continue;
        };
        let id = message["id"].clone();
        let params = &message["params"];
        let result = match method {
            "initialize" => {
                write_log(
                    &log,
                    json!({"event": "initialize", "capabilities": params["capabilities"]}),
                );
                let mut result = json!({
                    "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                    "capabilities": {"tools": {"listChanged": true}},
                    "serverInfo": script["server"],
                });
                if let Some(instructions) = script["instructions"].as_str() {
                    result["instructions"] = json!(instructions);
                }
                Ok(result)
            }
            "tools/list" => {
                script = read(&script_path);
                write_log(&log, json!({"event": "list", "cursor": params["cursor"]}));
                if let Some(ms) = script["list_sleep_ms"].as_u64() {
                    std::thread::sleep(Duration::from_millis(ms));
                }
                if let Some(message) = script["list_error"].as_str() {
                    send(&json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32000, "message": message}}));
                    continue;
                }
                let mut result = json!({"tools": script["tools"]});
                if script["endless_listing"] == true {
                    let next = params["cursor"]
                        .as_str()
                        .and_then(|c| c.parse::<u64>().ok());
                    result["nextCursor"] = json!((next.unwrap_or(0) + 1).to_string());
                }
                Ok(result)
            }
            "tools/call" => {
                let tool = params["name"].as_str().unwrap_or_default().to_owned();
                let started = now_ms();
                let answer = script["answers"][&tool].clone();
                if let Some(message) = answer["rpc_error"].as_str() {
                    send(&json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32000, "message": message}}));
                    continue;
                }
                let result = call(&answer, &params["arguments"], &mut lines, &mut asked);
                if answer["reload"] == true {
                    script = read(&script_path);
                    send(&json!({"jsonrpc": "2.0",
                        "method": "notifications/tools/list_changed"}));
                }
                write_log(
                    &log,
                    json!({"event": "call", "tool": tool, "arguments": params["arguments"],
                        "start_ms": started, "end_ms": now_ms(),
                        "answered": result["content"][0]["text"]}),
                );
                Ok(result)
            }
            "ping" => Ok(json!({})),
            _ if id.is_null() => continue,
            _ => Err(json!({"code": -32601, "message": format!("no method {method}")})),
        };
        if id.is_null() {
            continue;
        }
        send(&match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        });
    }
}

fn read(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("the script")).expect("JSON")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn send(message: &Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{message}").unwrap();
    out.flush().unwrap();
}

fn write_log(log: &Option<String>, line: Value) {
    let Some(path) = log else {
        return;
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{line}").unwrap();
}

/// What a tool call answers, as the script says.
fn call(
    answer: &Value,
    arguments: &Value,
    lines: &mut impl Iterator<Item = std::io::Result<String>>,
    asked: &mut u64,
) -> Value {
    if let Some(ms) = answer["sleep_ms"].as_u64() {
        std::thread::sleep(Duration::from_millis(ms));
    }
    let text = if let Some(kind) = answer["ask"].as_str() {
        *asked += 1;
        let id = format!("fake-{asked}");
        let (method, params) = match kind {
            "sampling" => (
                "sampling/createMessage",
                json!({"messages": [{"role": "user",
                    "content": {"type": "text", "text": "hello"}}], "maxTokens": 10}),
            ),
            "elicitation" => (
                "elicitation/create",
                json!({"message": "Your name?", "requestedSchema": {"type": "object",
                    "properties": {"name": {"type": "string"}}}}),
            ),
            _ => ("roots/list", json!({})),
        };
        send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut got = Value::Null;
        for line in lines.by_ref() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["id"] == id.as_str() {
                got = message;
                break;
            }
        }
        match (&got["error"], &got["result"]) {
            (Value::Null, Value::Null) => format!("{method}: no answer"),
            (Value::Null, result) => format!("{method}: answered {result}"),
            (error, _) => format!("{method}: refused {error}"),
        }
    } else if let Some(name) = answer["env"].as_str() {
        std::env::var(name).unwrap_or_else(|_| format!("{name} is not set"))
    } else if answer["echo"] == true {
        arguments.to_string()
    } else if let Some(text) = answer["text"].as_str() {
        text.to_owned()
    } else {
        "ok".to_owned()
    };
    if let Some(error) = answer["error"].as_str() {
        return json!({"content": [{"type": "text", "text": error}], "isError": true});
    }
    let mut content = vec![json!({"type": "text", "text": text})];
    if answer["image"] == true {
        content.push(json!({"type": "image", "mimeType": "image/png",
            "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="}));
    }
    json!({"content": content})
}

/// Streamable HTTP, one request a connection.
fn http(script: &Value, log: &Option<String>, port_file: &str) {
    use std::io::{BufRead, BufReader, Read};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std::fs::write(port_file, listener.local_addr().unwrap().port().to_string()).unwrap();
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        if reader.read_line(&mut first).is_err() {
            continue;
        }
        let mut headers = serde_json::Map::new();
        let mut length = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                let (name, value) = (name.trim().to_lowercase(), value.trim().to_owned());
                if name == "content-length" {
                    length = value.parse().unwrap_or(0);
                }
                headers.insert(name, json!(value));
            }
        }
        let mut body = vec![0; length];
        let _ = reader.read_exact(&mut body);
        // reqwest sends a body of unknown length in chunks.
        if headers.get("transfer-encoding").and_then(Value::as_str) == Some("chunked") {
            loop {
                let mut size = String::new();
                if reader.read_line(&mut size).unwrap_or(0) == 0 {
                    break;
                }
                let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
                let mut chunk = vec![0; size + 2];
                if reader.read_exact(&mut chunk).is_err() || size == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..size]);
            }
        }
        let method = first
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        write_log(
            log,
            json!({"event": "http", "method": method, "headers": headers}),
        );
        let reply = |status: &str, body: Option<Value>| {
            let text = body.map(|body| body.to_string()).unwrap_or_default();
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nMcp-Session-Id: fake\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            )
        };
        let answer = match method.as_str() {
            "POST" => {
                let message: Value = serde_json::from_slice(&body).unwrap_or_default();
                let id = message["id"].clone();
                let params = &message["params"];
                let result = match message["method"].as_str().unwrap_or_default() {
                    "initialize" => Some(json!({
                        "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                        "capabilities": {"tools": {}}, "serverInfo": script["server"],
                        "instructions": script["instructions"]})),
                    "tools/list" => Some(json!({"tools": script["tools"]})),
                    "tools/call" => {
                        let tool = params["name"].as_str().unwrap_or_default();
                        let answer = &script["answers"][tool];
                        Some(call(
                            answer,
                            &params["arguments"],
                            &mut std::iter::empty::<std::io::Result<String>>(),
                            &mut 0,
                        ))
                    }
                    _ if id.is_null() => None,
                    _ => Some(json!({})),
                };
                match (result, id.is_null()) {
                    (Some(result), false) => reply(
                        "200 OK",
                        Some(json!({"jsonrpc": "2.0", "id": id, "result": result})),
                    ),
                    _ => reply("202 Accepted", None),
                }
            }
            "DELETE" => reply("200 OK", None),
            _ => reply("405 Method Not Allowed", None),
        };
        let _ = stream.write_all(answer.as_bytes());
    }
}
