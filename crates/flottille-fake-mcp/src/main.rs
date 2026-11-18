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
//! its own to the client), and logs each request's headers and query (`http`). Over HTTP the script is read
//! again at each request.
//!
//! With `oauth` in the script, the HTTP server is protected (MCP authorization): `/mcp` answers
//! `401` with a `WWW-Authenticate` challenge until it is given an access token it issued, and it
//! is its own authorization server too, at `/as` (its protected resource metadata, its metadata,
//! `register`, `authorize` with PKCE `S256`, which redirects at once with a code, its `state` and
//! its `iss`, and `token`). `oauth` takes `expires_in` (the tokens' life, 3600 seconds by
//! default), `refresh: "refused"` (every refresh answers `invalid_grant`), `refresh_error`
//! (every refresh answers that OAuth error instead, and keeps the refresh token valid),
//! `no_expiry` (the tokens carry no `expires_in`), `revoked` (`/mcp` answers `401` to every token),
//! `refresh_hold` (the path of a file: a refresh is answered once it exists, and logs `hold` as it
//! waits), `registration: false`
//! (no dynamic registration), `authorize_at`, `token_at` and `register_at: "elsewhere"` (that
//! endpoint named on `localhost`, another origin than the issuer's `127.0.0.1`), `iss: false`
//! (no `iss` in the authorization response, and none promised), and `tokens_file`, where every token it issues is written, so that a
//! test looks for them where they must not be. Its log never holds a token: `bearer` is the number
//! of the one a request carried, and `register`, `authorize` and `token` say what was asked.
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
//!  "abort": true,
//!  "close_streams": true,
//!  "notify": {"when": "<file>", "times": 50},
//!  "list_sleep_ms": 100,
//!  "list_when": "<file>",
//!  "endless_listing": true,
//!  "list_error": "the text of a JSON-RPC error answered to every listing"}
//! ```
//!
//! On stdio, `tools/list` reads the script again: a test changes the tools of a running server
//! by writing it. `exit` ends it with that code at the start and `abort` by the signal of an abort,
//! after the `stderr` line; `close_streams` closes its input and output at the start and stays
//! alive. `notify` says its tools changed `times` times, at once, when the file `when`
//! appears; `list_sleep_ms` slows each listing; `list_when` holds each listing until that file
//! exists, so a test answers when it chooses and not after a delay; `endless_listing` answers
//! every page with a next one; `list_error` fails each listing with that text; `echo_env` ends
//! its instructions and every tool's description with the value of that variable, as a careless
//! server echoes its configuration.
//!
//! An answer is, in this order of precedence, any of: `sleep_ms` (waits first), `ask`
//! (`"sampling"`, `"elicitation"` or `"roots"`: sends that request to the client and answers with
//! what it got back), `reload` (reads the script again and says its tools changed), `env` (the
//! value of that variable), `echo` (the arguments, as JSON), `text`; `image` adds a picture after
//! the text, `images` that many pictures of `image_bytes` bytes each (a PNG's signature, then
//! zeros), `image_data` one picture of those base64 bytes, of the type `image_mime` (png by
//! default), `audio` a sound and `resource` a resource link after it, `error` answers that text as
//! an error. `rpc_error` answers that text as a JSON-RPC
//! error instead, on stdio. `big` (`{"bytes": 1000, "chunk": 100, "pause_ms": 5}`) answers a text of
//! that many bytes as one message, written `chunk` bytes at a time with a pause after each, and
//! logs `wrote` (the bytes of the text written so far) after every piece and `stopped` when the
//! reader is gone before the end; over HTTP, `"sse": true` makes it one event of an event stream
//! and `"unsized": true` sends no length. With no answer, `ok`.
//!
//! With `auth login <name>` among its arguments, it plays agent-browser's command line instead
//! (`agent-browser auth login <name> --credential-provider <plugin> --item <ref> [--url <url>]`,
//! with `--config <file>`): it runs the plugin that file declares with `credential.read`, asks it
//! for the credential over `agent-browser.plugin.v1`, and writes what it got to the file `form`
//! of the script's `login`, as a page's login form would be filled. `login` takes `token`
//! (`"drop"`: the plugin is run without `FLOTTILLE_VAULT_TOKEN`; `"wrong"`: with another one),
//! `replay` (the plugin is asked once more, with the same token) and `echo` (the password is
//! written on its output, as a careless command line would) and `fail` (the password is written
//! on its error output, and it fails; the form is filled first, as a page that refused it keeps it).
//! With `open` among its arguments (and no `mcp`), it plays `agent-browser open <url>`: its log line
//! `open` says its arguments and folder, and it empties the form, as a page loaded afresh does;
//! `open_fail` in `login` makes it fail instead. It keeps the address of the page it lands on
//! beside the form (`lands_on` in `login`: a page that redirects there), and `auth login
//! --no-navigate` checks that page's origin against `--url`'s, as agent-browser does, failing
//! before the plugin is asked. `asks_for` in `login` asks the plugin for that URL instead of
//! `--url`'s. `--url` and `--item` are read as `--url <url>` or
//! `--url=<url>`. Its log line, `login`, says its
//! arguments, its folder and what the plugin answered, never a password. The lines `start`, `login`
//! and `open` carry `config`, the JSON of the file `--config` names when the line was written.
//!
//! With `AGENT_BROWSER_SOCKET_DIR` set, a login and each tool call reach the browser agent-browser's
//! daemon runs there, which outlives the command that started it: the file `browser.json` in that
//! folder, written by the first one with the mode (`headed`, from the configuration) and the
//! `XDG_RUNTIME_DIR` it started in, and found again by the next; the log line `browser` says which
//! (`by`: `login` or `call`, `started` or not). With `close` among its arguments (and no `mcp`), it
//! plays `agent-browser close`: the file is removed, and the log line `close` says its arguments.
//!
//! The log, one JSON line each: `start` (its pid, process group, session, folder and arguments,
//! the child's pid with `--child`), `initialize` (the client's capabilities), `list` (the cursor
//! asked), `call` (the tool, the arguments, when it started and ended, in milliseconds since the
//! epoch), written once the call is answered; `received` (the tool) is written as a call arrives,
//! before it sleeps or answers, so a test can wait for a call in progress.

use std::io::{BufRead, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// The file `--config` names, as JSON, at the moment the command line runs: what agent-browser
/// would load (and, for its MCP server's tool calls, the only file its child processes load).
fn loaded_config(args: &[String]) -> Value {
    args.iter()
        .position(|arg| arg == "--config")
        .and_then(|at| args.get(at + 1))
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

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
    if args
        .windows(2)
        .any(|pair| pair[0] == "auth" && pair[1] == "login")
    {
        return login(&args, &script, &log);
    }
    if !args.iter().any(|arg| arg == "mcp") && args.iter().any(|arg| arg == "open") {
        return open(&args, &script, &log);
    }
    if !args.iter().any(|arg| arg == "mcp") && args.iter().any(|arg| arg == "close") {
        return close(&args, &log);
    }
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
    // `link_socket_to` in the script: tries to hard-link the display's socket out of its folder.
    let linked = script["link_socket_to"].as_str().map(|to| {
        let from = std::env::var_os("XDG_RUNTIME_DIR")
            .zip(std::env::var_os("WAYLAND_DISPLAY"))
            .map(|(dir, name)| std::path::Path::new(&dir).join(name));
        let _ = std::fs::remove_file(to);
        from.is_some_and(|from| std::fs::hard_link(from, to).is_ok())
    });
    write_log(
        &log,
        json!({"event": "start", "pid": std::process::id(), "linked": linked,
        "pgid": fields.get(2).and_then(|f| f.parse::<u32>().ok()),
        "sid": fields.get(3).and_then(|f| f.parse::<u32>().ok()),
        "cwd": std::env::current_dir().ok(), "args": args, "child": child,
        "config": loaded_config(&args),
        "display": display_environment()}),
    );
    if let Some(line) = script["stderr"].as_str() {
        eprintln!("{line}");
    }
    if let Some(code) = script["exit"].as_i64() {
        std::process::exit(code as i32);
    }
    if script["abort"].as_bool() == Some(true) {
        std::process::abort();
    }
    if script["close_streams"].as_bool() == Some(true) {
        // No `libc` here: the two calls are declared as they are.
        unsafe extern "C" {
            fn close(fd: i32) -> i32;
        }
        // SAFETY: closing the standard streams, which nothing uses after.
        unsafe {
            close(0);
            close(1);
        }
        std::thread::sleep(std::time::Duration::from_secs(600));
        return;
    }
    if let Some(port_file) = option("--http") {
        return http(&script_path, &log, &port_file);
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
                    result["instructions"] = json!(format!("{instructions}{}", echoed(&script)));
                }
                Ok(result)
            }
            "tools/list" => {
                script = read(&script_path);
                write_log(&log, json!({"event": "list", "cursor": params["cursor"]}));
                if let Some(ms) = script["list_sleep_ms"].as_u64() {
                    std::thread::sleep(Duration::from_millis(ms));
                }
                if let Some(file) = script["list_when"].as_str() {
                    while !std::path::Path::new(file).exists() {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                if let Some(message) = script["list_error"].as_str() {
                    send(&json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32000, "message": message}}));
                    continue;
                }
                let mut tools = script["tools"].clone();
                if !echoed(&script).is_empty() {
                    for tool in tools.as_array_mut().into_iter().flatten() {
                        let said = tool["description"].as_str().unwrap_or_default();
                        tool["description"] = json!(format!("{said}{}", echoed(&script)));
                    }
                }
                let mut result = json!({"tools": tools});
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
                write_log(&log, json!({"event": "received", "tool": tool}));
                browser(&args, &log, "call");
                let answer = script["answers"][&tool].clone();
                if let Some(message) = answer["rpc_error"].as_str() {
                    send(&json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32000, "message": message}}));
                    continue;
                }
                if answer["big"].is_object() {
                    let head = format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"result":{{"content":[{{"type":"text","text":""#
                    );
                    let mut out = std::io::stdout().lock();
                    slowly(&mut out, &head, &answer["big"], "\"}]}}\n", &log);
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

/// The environment a browser's display and sockets come from, for the tests to read: the
/// display variables, the folders agent-browser puts its sockets in, and what the runtime folder
/// held.
fn display_environment() -> Value {
    json!({"WAYLAND_DISPLAY": std::env::var("WAYLAND_DISPLAY").ok(),
    "DISPLAY": std::env::var("DISPLAY").ok(),
    "XDG_RUNTIME_DIR": std::env::var("XDG_RUNTIME_DIR").ok(),
    "DBUS_SESSION_BUS_ADDRESS": std::env::var("DBUS_SESSION_BUS_ADDRESS").ok(),
    "AGENT_BROWSER_SOCKET_DIR": std::env::var("AGENT_BROWSER_SOCKET_DIR").ok(),
    "runtime_files": std::env::var("XDG_RUNTIME_DIR").ok().and_then(|dir| {
        let mut names: Vec<String> = std::fs::read_dir(dir).ok()?
            .filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
        names.sort();
        Some(names)
    })})
}

/// The browser on `AGENT_BROWSER_SOCKET_DIR`, started by this command when none runs there: what
/// agent-browser's daemon does, which then serves every command of that folder until it is closed.
fn browser(args: &[String], log: &Option<String>, by: &str) {
    let Some(folder) = std::env::var_os("AGENT_BROWSER_SOCKET_DIR") else {
        return;
    };
    let file = std::path::Path::new(&folder).join("browser.json");
    let running = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let started = running.is_none();
    let browser = running.unwrap_or_else(|| {
        let browser = json!({"headed": loaded_config(args)["headed"],
            "XDG_RUNTIME_DIR": std::env::var("XDG_RUNTIME_DIR").ok()});
        std::fs::write(&file, browser.to_string()).unwrap();
        browser
    });
    write_log(
        log,
        json!({"event": "browser", "by": by, "started": started, "browser": browser}),
    );
}

/// `agent-browser close`: the browser on `AGENT_BROWSER_SOCKET_DIR` ends.
fn close(args: &[String], log: &Option<String>) {
    if let Some(folder) = std::env::var_os("AGENT_BROWSER_SOCKET_DIR") {
        let _ = std::fs::remove_file(std::path::Path::new(&folder).join("browser.json"));
    }
    write_log(log, json!({"event": "close", "args": args}));
    println!("✓ Browser closed");
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

/// What `echo_env` appends: a space and the value of the variable, nothing without it.
fn echoed(script: &Value) -> String {
    script["echo_env"]
        .as_str()
        .and_then(|name| std::env::var(name).ok())
        .map(|value| format!(" {value}"))
        .unwrap_or_default()
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
    if let Some(data) = answer["image_data"].as_str() {
        let mime = answer["image_mime"].as_str().unwrap_or("image/png");
        content.push(json!({"type": "image", "mimeType": mime, "data": data}));
    }
    if answer["audio"] == true {
        content.push(json!({"type": "audio", "mimeType": "audio/wav", "data": "UklGRgAAAAA="}));
    }
    if answer["resource"] == true {
        content.push(json!({"type": "resource_link", "name": "notes",
            "uri": "file:///notes/today.md"}));
    }
    if let Some(count) = answer["images"].as_u64() {
        use base64::Engine as _;
        let mut bytes = vec![0u8; answer["image_bytes"].as_u64().unwrap_or(64) as usize];
        let signature = b"\x89PNG\r\n\x1a\n";
        let kept = signature.len().min(bytes.len());
        bytes[..kept].copy_from_slice(&signature[..kept]);
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        for _ in 0..count {
            content.push(json!({"type": "image", "mimeType": "image/png", "data": data}));
        }
    }
    json!({"content": content})
}

/// `head`, `big.bytes` bytes of filler and `tail`, written `big.chunk` bytes at a time with
/// `big.pause_ms` between: slow enough that a reader which stops taking it is seen to stop. What
/// was written is logged after each piece, and `stopped` where the reader was gone.
fn slowly(out: &mut impl Write, head: &str, big: &Value, tail: &str, log: &Option<String>) {
    let bytes = big["bytes"].as_u64().unwrap_or(0) as usize;
    let chunk = (big["chunk"].as_u64().unwrap_or(1 << 20) as usize).max(1);
    let pause = Duration::from_millis(big["pause_ms"].as_u64().unwrap_or(0));
    let mut written = 0;
    let mut put = |data: &[u8]| out.write_all(data).and_then(|_| out.flush()).is_ok();
    let mut alive = put(head.as_bytes());
    while alive && written < bytes {
        let piece = chunk.min(bytes - written);
        alive = put(&vec![b'x'; piece]);
        if alive {
            written += piece;
            write_log(log, json!({"event": "wrote", "bytes": written}));
            std::thread::sleep(pause);
        }
    }
    alive = alive && put(tail.as_bytes());
    if !alive {
        write_log(log, json!({"event": "stopped", "bytes": written}));
    }
}

/// Streamable HTTP, one request a connection, each on its own thread: a request the script holds
/// does not hold the others. The script is read again at each request.
fn http(script_path: &str, log: &Option<String>, port_file: &str) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::fs::write(port_file, port.to_string()).unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let issuer = std::sync::Arc::new(std::sync::Mutex::new(Issuer::default()));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let (script_path, log, base, issuer) = (
            script_path.to_owned(),
            log.clone(),
            base.clone(),
            issuer.clone(),
        );
        std::thread::spawn(move || serve(stream, &script_path, &log, &base, &issuer));
    }
}

fn serve(
    mut stream: std::net::TcpStream,
    script_path: &str,
    log: &Option<String>,
    base: &str,
    issuer: &std::sync::Mutex<Issuer>,
) {
    use std::io::{BufRead, BufReader, Read};
    let script = &read(script_path);
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
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
    let mut words = first.split_whitespace();
    let method = words.next().unwrap_or_default().to_owned();
    let target = words.next().unwrap_or_default().to_owned();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let reply = |status: &str, body: Option<Value>| {
        let text = body.map(|body| body.to_string()).unwrap_or_default();
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nMcp-Session-Id: fake\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
            text.len()
        )
    };
    let oauth = &script["oauth"];
    if oauth.is_object() {
        // A token is never written to the log: which one it is, by its number.
        let bearer = headers.remove("authorization").and_then(|value| {
            let token = value.as_str()?.strip_prefix("Bearer ")?;
            // A revoked token is one it issued and no longer knows.
            let serial = issuer.lock().unwrap().serial_of(token);
            serial.filter(|_| oauth["revoked"] != true)
        });
        if path != "/mcp" || method != "POST" || bearer.is_none() {
            write_log(
                log,
                json!({"event": "http", "method": method, "path": path, "headers": headers,
                    "bearer": bearer}),
            );
        }
        if path != "/mcp" {
            // A refresh the script holds is answered once its file exists, the others meanwhile.
            let held = (path == "/as/token" && form(&body)["grant_type"] == "refresh_token")
                .then(|| oauth["refresh_hold"].as_str())
                .flatten();
            let answer = issuer
                .lock()
                .unwrap()
                .answer(oauth, base, &method, path, query, &body, log, &reply);
            if let Some(file) = held {
                write_log(log, json!({"event": "hold"}));
                while !std::path::Path::new(file).exists() {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            let _ = stream.write_all(answer.as_bytes());
            return;
        }
        if bearer.is_none() {
            let challenge = format!(
                "Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\", \
                 scope=\"notes:read\""
            );
            let text = r#"{"error":"invalid_token"}"#;
            let answer = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\n\
                 WWW-Authenticate: {challenge}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{text}",
                text.len()
            );
            let _ = stream.write_all(answer.as_bytes());
            return;
        }
        let message: Value = serde_json::from_slice(&body).unwrap_or_default();
        write_log(
            log,
            json!({"event": "http", "method": method, "path": path, "headers": headers,
                "bearer": bearer, "rpc": message["method"]}),
        );
    } else {
        write_log(
            log,
            json!({"event": "http", "method": method, "headers": headers, "query": query}),
        );
    }
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
                    if answer["big"].is_object() {
                        // `sse`: the message is one event of a stream, not a JSON body.
                        let sse = answer["big"]["sse"] == true;
                        let (kind, data, end) = match sse {
                            true => ("text/event-stream", "data: ", "\n\n"),
                            false => ("application/json", "", ""),
                        };
                        let head = format!(
                            r#"{data}{{"jsonrpc":"2.0","id":{id},"result":{{"content":[{{"type":"text","text":""#
                        );
                        let tail = format!(r#""}}]}}}}{end}"#);
                        let length = head.len()
                            + answer["big"]["bytes"].as_u64().unwrap_or(0) as usize
                            + tail.len();
                        // `unsized`: no length, the body ends where the connection does.
                        let length = match answer["big"]["unsized"] == true {
                            true => String::new(),
                            false => format!("Content-Length: {length}\r\n"),
                        };
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\n\
                             Mcp-Session-Id: fake\r\n{length}Connection: close\r\n\r\n{head}"
                        );
                        slowly(&mut stream, &head, &answer["big"], &tail, log);
                        return;
                    }
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

/// The authorization server of a protected server (`oauth` in the script): its tokens, the codes
/// it gave, and the clients it registered, in its memory.
#[derive(Default)]
struct Issuer {
    issued: u64,
    /// Access token → its number.
    access: std::collections::HashMap<String, u64>,
    /// Refresh token → the number of the access token it came with.
    refresh: std::collections::HashMap<String, u64>,
    /// Code → the request it answers: its challenge, its redirect, its resource.
    codes: std::collections::HashMap<String, (String, String, String)>,
}

impl Issuer {
    fn serial_of(&self, token: &str) -> Option<u64> {
        self.access.get(token).copied()
    }

    /// A pair of tokens, written to the script's `tokens_file` for the test that looks for them
    /// where they must not be.
    fn issue(&mut self, oauth: &Value) -> Value {
        self.issued += 1;
        let n = self.issued;
        let access = format!("fake-access-{n}-{}", random_hex());
        let refresh = format!("fake-refresh-{n}-{}", random_hex());
        if let Some(file) = oauth["tokens_file"].as_str() {
            write_log(&Some(file.to_owned()), json!(access));
            write_log(&Some(file.to_owned()), json!(refresh));
        }
        self.access.insert(access.clone(), n);
        self.refresh.insert(refresh.clone(), n);
        let mut tokens = json!({"access_token": access, "token_type": "Bearer",
            "refresh_token": refresh});
        if oauth["no_expiry"] != true {
            tokens["expires_in"] = json!(oauth["expires_in"].as_u64().unwrap_or(3600));
        }
        tokens
    }

    #[allow(clippy::too_many_arguments)]
    fn answer(
        &mut self,
        oauth: &Value,
        base: &str,
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
        log: &Option<String>,
        reply: &dyn Fn(&str, Option<Value>) -> String,
    ) -> String {
        let issuer = format!("{base}/as");
        match (method, path) {
            ("GET", "/.well-known/oauth-protected-resource/mcp") => reply(
                "200 OK",
                Some(json!({"resource": format!("{base}/mcp"),
                    "authorization_servers": [issuer], "scopes_supported": ["notes:read"]})),
            ),
            ("GET", "/.well-known/oauth-authorization-server/as") => {
                // `localhost` is this server too, and another origin than `127.0.0.1`.
                let elsewhere = issuer.replace("127.0.0.1", "localhost");
                let at = |key: &str| match oauth[key] == "elsewhere" {
                    true => elsewhere.clone(),
                    false => issuer.clone(),
                };
                let mut metadata = json!({"issuer": issuer,
                    "authorization_endpoint": format!("{}/authorize", at("authorize_at")),
                    "token_endpoint": format!("{}/token", at("token_at")),
                    "code_challenge_methods_supported": ["S256"],
                    "authorization_response_iss_parameter_supported": oauth["iss"] != false});
                if oauth["registration"] != false {
                    metadata["registration_endpoint"] =
                        json!(format!("{}/register", at("register_at")));
                }
                reply("200 OK", Some(metadata))
            }
            ("POST", "/as/register") => {
                let asked: Value = serde_json::from_slice(body).unwrap_or_default();
                write_log(log, json!({"event": "register", "asked": asked}));
                let mut client = asked.clone();
                client["client_id"] = json!(format!("fake-client-{}", random_hex()));
                reply("201 Created", Some(client))
            }
            ("GET", "/as/authorize") => {
                let asked = form(query.as_bytes());
                write_log(log, json!({"event": "authorize", "asked": asked}));
                let field = |key: &str| asked[key].as_str().unwrap_or_default().to_owned();
                if field("response_type") != "code" || field("code_challenge_method") != "S256" {
                    return reply("400 Bad Request", Some(json!({"error": "invalid_request"})));
                }
                let code = format!("code-{}", random_hex());
                self.codes.insert(
                    code.clone(),
                    (
                        field("code_challenge"),
                        field("redirect_uri"),
                        field("resource"),
                    ),
                );
                let iss = match oauth["iss"] == false {
                    true => String::new(),
                    false => format!("&iss={}", encoded(&issuer)),
                };
                let location = format!(
                    "{}?code={code}&state={}{iss}",
                    field("redirect_uri"),
                    encoded(&field("state")),
                );
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\
                     Connection: close\r\n\r\n"
                )
            }
            ("POST", "/as/token") => {
                let asked = form(body);
                let field = |key: &str| asked[key].as_str().unwrap_or_default().to_owned();
                let grant = field("grant_type");
                let ok = match grant.as_str() {
                    "authorization_code" => self.codes.remove(&field("code")).is_some_and(
                        |(challenge, redirect, resource)| {
                            challenge == s256(&field("code_verifier"))
                                && redirect == field("redirect_uri")
                                && resource == field("resource")
                        },
                    ),
                    "refresh_token" => {
                        oauth["refresh"] != "refused"
                            && !oauth["refresh_error"].is_string()
                            && self.refresh.remove(&field("refresh_token")).is_some()
                    }
                    _ => false,
                };
                let answer = ok.then(|| self.issue(oauth));
                write_log(
                    log,
                    json!({"event": "token", "grant": grant, "resource": field("resource"),
                        "client_id": field("client_id"), "ok": ok,
                        "serial": ok.then_some(self.issued)}),
                );
                match answer {
                    Some(tokens) => reply("200 OK", Some(tokens)),
                    None => reply(
                        "400 Bad Request",
                        Some(json!({"error": oauth["refresh_error"].as_str()
                                .filter(|_| grant == "refresh_token")
                                .unwrap_or("invalid_grant"),
                            "error_description": "the grant is not valid"})),
                    ),
                }
            }
            _ => reply("404 Not Found", None),
        }
    }
}

fn random_hex() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 8];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .unwrap();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The PKCE challenge of a verifier, `S256`.
fn s256(verifier: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier))
}

/// A form (or a query), decoded, as a JSON object.
fn form(body: &[u8]) -> Value {
    let text = String::from_utf8_lossy(body);
    let mut fields = serde_json::Map::new();
    for pair in text.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        fields.insert(decoded(key), json!(decoded(value)));
    }
    Value::Object(fields)
}

fn decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'+' => out.push(b' '),
            b'%' if at + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap_or("");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                at += 2;
            }
            byte => out.push(byte),
        }
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encoded(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            byte => format!("%{byte:02X}"),
        })
        .collect()
}

/// agent-browser's `auth login` with a credential provider: the plugin asked, the form filled.
fn login(args: &[String], script: &Value, log: &Option<String>) {
    let option = |name: &str| {
        let joined = format!("{name}=");
        args.iter()
            .find_map(|arg| arg.strip_prefix(&joined).map(str::to_owned))
            .or_else(|| {
                args.iter()
                    .position(|arg| arg == name)
                    .and_then(|at| args.get(at + 1))
                    .cloned()
            })
    };
    let behaviour = &script["login"];
    let name = args
        .windows(3)
        .find(|words| words[0] == "auth" && words[1] == "login")
        .map(|words| words[2].clone())
        .unwrap_or_default();
    // The first `--config <file>`, as agent-browser reads it (never `--config=<file>`).
    let config = loaded_config(args);
    let provider = option("--credential-provider").unwrap_or_default();
    let plugin = config["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|plugin| {
            plugin["name"] == provider.as_str()
                && plugin["capabilities"]
                    .as_array()
                    .is_some_and(|c| c.iter().any(|c| c == "credential.read"))
        })
        .cloned();
    // `--no-navigate`, as agent-browser 0.43.0 does it: the page that is open must be of the
    // origin of `--url`, or nothing is asked nor filled.
    if args.iter().any(|arg| arg == "--no-navigate") {
        let page = behaviour["form"]
            .as_str()
            .and_then(|form| std::fs::read_to_string(page_file(form)).ok())
            .unwrap_or_default();
        let expected = option("--url").unwrap_or_default();
        if page.is_empty() || origin(&page) != origin(&expected) {
            write_log(
                log,
                json!({"event": "login", "args": args, "config": loaded_config(args),
                "cwd": std::env::current_dir().ok(), "page": page}),
            );
            eprintln!(
                "✗ Current page origin {} does not match credential origin {}",
                origin(&page),
                origin(&expected)
            );
            std::process::exit(1);
        }
    }
    // `asks_for`: the plugin is asked for another page than `--url`.
    let url = behaviour["asks_for"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| option("--url"));
    let request = json!({"protocol": "agent-browser.plugin.v1", "type": "credential.resolve",
        "capability": "credential.read", "request": {"profileName": name,
            "itemRef": option("--item"), "url": url}});
    let token = std::env::var("FLOTTILLE_VAULT_TOKEN").ok();
    let ask = |plugin: &Value| -> Value {
        let mut command = std::process::Command::new(plugin["command"].as_str().unwrap_or("?"));
        for arg in plugin["args"].as_array().into_iter().flatten() {
            command.arg(arg.as_str().unwrap_or_default());
        }
        match behaviour["token"].as_str() {
            Some("drop") => {
                command.env_remove("FLOTTILLE_VAULT_TOKEN");
            }
            Some("wrong") => {
                command.env("FLOTTILLE_VAULT_TOKEN", "not-the-token-of-this-run");
            }
            _ => {
                if let Some(token) = &token {
                    command.env("FLOTTILLE_VAULT_TOKEN", token);
                }
            }
        }
        let child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            return json!({"success": false, "error": "the plugin did not start"});
        };
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{request}").ok();
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|_| json!({"success": false, "error": "no JSON on its output"}))
    };
    let Some(plugin) = plugin else {
        write_log(
            log,
            json!({"event": "login", "args": args, "config": loaded_config(args),
            "cwd": std::env::current_dir().ok(), "plugin": {"success": false,
                "error": format!("no plugin {provider} with credential.read")}}),
        );
        eprintln!("✗ no credential provider {provider}");
        std::process::exit(1);
    };
    browser(args, log, "login");
    let answer = ask(&plugin);
    let success = answer["success"] == true && answer["protocol"] == "agent-browser.plugin.v1";
    let mut line = json!({"event": "login", "args": args, "config": loaded_config(args),
        "cwd": std::env::current_dir().ok(), "display": display_environment(),
        "plugin": {"success": success, "error": answer["error"]}});
    if behaviour["replay"] == true {
        let again = ask(&plugin);
        line["replayed"] = json!({"success": again["success"] == true, "error": again["error"]});
    }
    write_log(log, line);
    if !success {
        eprintln!("✗ the credential provider failed");
        std::process::exit(1);
    }
    let credential = &answer["credential"];
    if behaviour["fail"] == true {
        // Filled, then refused: the secret stays in the form until the page is loaded again.
        if let Some(form) = behaviour["form"].as_str() {
            std::fs::write(form, credential.to_string()).unwrap();
        }
        eprintln!(
            "✗ the page refused {}",
            credential["password"].as_str().unwrap_or_default()
        );
        std::process::exit(1);
    }
    if let Some(form) = behaviour["form"].as_str() {
        std::fs::write(form, credential.to_string()).unwrap();
    }
    if behaviour["echo"] == true {
        println!(
            "typed {}",
            credential["password"].as_str().unwrap_or_default()
        );
    }
    println!(
        "✓ Logged in as {}",
        credential["username"].as_str().unwrap_or_default()
    );
}

/// `agent-browser open <url>`: loads the page afresh, which empties the form `login` filled;
/// `open_fail` in the script's `login` makes it fail instead.
fn open(args: &[String], script: &Value, log: &Option<String>) {
    write_log(
        log,
        json!({"event": "open", "args": args, "config": loaded_config(args),
            "cwd": std::env::current_dir().ok(), "display": display_environment()}),
    );
    let behaviour = &script["login"];
    if behaviour["open_fail"] == true {
        eprintln!("✗ the browser could not open the page");
        std::process::exit(1);
    }
    if let Some(form) = behaviour["form"].as_str() {
        let _ = std::fs::remove_file(form);
        // The page it lands on: `lands_on` when the page redirects elsewhere.
        let url = behaviour["lands_on"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                args.iter()
                    .position(|arg| arg == "open")
                    .and_then(|at| args.get(at + 1))
                    .cloned()
            })
            .unwrap_or_default();
        std::fs::write(page_file(form), url).unwrap();
    }
    println!("✓ opened");
}

/// Where the fake browser keeps the address of the page that is open, beside the form.
fn page_file(form: &str) -> String {
    format!("{form}.page")
}

/// The origin of `url`: its scheme, host and port, as agent-browser compares them.
fn origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return String::new();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    format!("{scheme}://{authority}")
}
