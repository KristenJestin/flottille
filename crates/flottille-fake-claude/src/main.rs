//! A scripted `claude` for the tests of Flottille's Claude bridge: it speaks the stream-json mode
//! of Claude Code on stdio, as the version the bridge is pinned to does, and does what a script
//! says, so that no test ever runs the real one.
//!
//! # Its folder
//!
//! Everything goes through one folder, named by `FLOTTILLE_FAKE_CLAUDE_DIR`:
//!
//! - `script.json`, written by the test (optional: without it, every prompt is answered `ok`);
//! - `received.jsonl`, written by the fake: one JSON object per line, appended. When it starts:
//!   `{"pid", "started": {"argv", "env", "cwd", "mcp_config"}}`, its arguments, its whole
//!   environment, its folder and the content of the file `--mcp-config` named. Then every line it read on its standard input
//!   (`{"pid", "received": <the message>}`), and what it did that a test checks (`{"pid",
//!   "permission": <the answer>}`);
//! - `sessions/<id>`, one empty file per session `--session-id` made: `--resume` of a session
//!   that has none fails, as the real one does.
//!
//! # The script
//!
//! ```json
//! {"turns": [{"when": "hello", "do": [{"think": "a greeting"}, {"say": "Hello!"}]}]}
//! ```
//!
//! `models`, when given, replaces the models the answer to `initialize` lists (`MODELS`).
//!
//! A prompt plays the first turn whose `when` is in its text; a prompt matching none is answered
//! `ok`. A turn ends with a `result` message: `"stop_reason"` (`end_turn` by default), `"subtype"`
//! (`success` by default) and `"error"` (a text: the result is an error) shape it.
//!
//! | Step | What the fake does |
//! |---|---|
//! | `{"say": "text"}` | a `text_delta` stream event, then an `assistant` message with the text |
//! | `{"stream": ["a", "b"]}` | a `text_delta` stream event for each part, then one `assistant` message with them all |
//! | `{"think": "text"}` | the same with a `thinking_delta` and a thinking block |
//! | `{"tool": {"id", "name", "input", "result"}}` | an `assistant` message with the `tool_use`, then a `user` message with its `tool_result` |
//! | `{"permission": {"tool": "Bash", "input": {}}}` | a `can_use_tool` control request, the answer recorded |
//! | `{"compaction": true}` | a `compacting` status, then a `compact_boundary` |
//! | `{"retry": true}` | an `api_retry` message: the provider is overloaded |
//! | `{"limit": "five_hour"}` | a `rate_limit_event` rejected for that limit, then the synthetic message Claude Code writes, with its `error`; the result is an error with its text |
//! | `{"garbage": true}` | a line on its output that is not UTF-8 |
//! | `{"wait": 500}` | waits that many milliseconds; an interrupt ends the turn |
//! | `{"hang": true}` | goes silent until an interrupt |
//! | `{"die": 3}` | exits at once with that code |

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

/// The version the bridge was checked against: `claude --version` prints it.
const VERSION: &str = "2.1.295";
const MODEL: &str = "claude-fake-1";

/// The models the answer to `initialize` lists, as Claude Code describes them.
fn models() -> Value {
    json!([
        {"value": "default", "displayName": "Default (recommended)",
            "description": "The model Claude Code picks", "supportsEffort": true,
            "supportedEffortLevels": ["low", "medium", "high", "xhigh"]},
        {"value": "fable", "displayName": "Fable", "description": "The largest",
            "supportsEffort": true,
            "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
        {"value": "haiku", "displayName": "Haiku", "description": "The fastest"}
    ])
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--version") {
        println!("{VERSION} (Claude Code)");
        return;
    }
    let dir = PathBuf::from(
        std::env::var_os("FLOTTILLE_FAKE_CLAUDE_DIR")
            .expect("FLOTTILLE_FAKE_CLAUDE_DIR is not set"),
    );
    let record = Record(dir.join("received.jsonl"));
    let mcp_config = flag(&args, "--mcp-config")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let env: Map<String, Value> = std::env::vars().map(|(k, v)| (k, v.into())).collect();
    let cwd = std::env::current_dir().unwrap();
    record.write(
        json!({"started": {"argv": args, "env": env, "cwd": cwd, "mcp_config": mcp_config}}),
    );

    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let session = if let Some(id) = flag(&args, "--resume") {
        if !sessions.join(&id).exists() {
            eprintln!("No conversation found with session ID: {id}");
            std::process::exit(1);
        }
        id
    } else {
        let id = flag(&args, "--session-id").unwrap_or_else(|| "fake-session".into());
        std::fs::write(sessions.join(&id), "").unwrap();
        id
    };
    let script = match std::fs::read_to_string(dir.join("script.json")) {
        Ok(text) => serde_json::from_str(&text).expect("script.json is not JSON"),
        Err(_) => json!({}),
    };

    // A reader thread, so that an interrupt or an answer arrives while a turn plays.
    let (prompts, incoming) = mpsc::channel::<Value>();
    let (answers, answered) = mpsc::channel::<Value>();
    let interrupted = Arc::new(Mutex::new(false));
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let reader = Reader {
        models: script.get("models").cloned().unwrap_or_else(models),
        record: record.clone(),
        out: out.clone(),
        interrupted: interrupted.clone(),
    };
    std::thread::spawn(move || reader.run(prompts, answers));

    let mut fake = Fake {
        record,
        out,
        session,
        script,
        interrupted,
        next_request: 0,
    };
    for prompt in incoming {
        fake.play(&prompt, &answered);
    }
}

/// The value of a flag given as `--flag=value` or `--flag value`.
fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().enumerate().find_map(|(i, arg)| {
        if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
            Some(value.to_owned())
        } else if arg == name {
            args.get(i + 1).cloned()
        } else {
            None
        }
    })
}

#[derive(Clone)]
struct Record(PathBuf);

impl Record {
    fn write(&self, entry: Value) {
        let mut entry = entry;
        entry["pid"] = std::process::id().into();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.0)
            .expect("cannot write received.jsonl");
        file.write_all(format!("{entry}\n").as_bytes()).unwrap();
    }
}

fn send(out: &Mutex<std::io::Stdout>, message: Value) {
    let mut out = out.lock().unwrap();
    writeln!(out, "{message}").unwrap();
    out.flush().unwrap();
}

struct Reader {
    models: Value,
    record: Record,
    out: Arc<Mutex<std::io::Stdout>>,
    interrupted: Arc<Mutex<bool>>,
}

impl Reader {
    fn run(self, prompts: mpsc::Sender<Value>, answers: mpsc::Sender<Value>) {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            self.record.write(json!({"received": message}));
            match message["type"].as_str() {
                Some("user") => {
                    if prompts.send(message).is_err() {
                        break;
                    }
                }
                Some("control_response") => {
                    let _ = answers.send(message);
                }
                Some("control_request") => {
                    let mut answer = json!({});
                    match message["request"]["subtype"].as_str() {
                        Some("interrupt") => *self.interrupted.lock().unwrap() = true,
                        Some("initialize") => {
                            answer = json!({"commands": [], "agents": [], "output_style": "default",
                                "available_output_styles": ["default"], "models": self.models,
                                "account": {}});
                        }
                        _ => {}
                    }
                    send(
                        &self.out,
                        json!({"type": "control_response", "response": {
                            "subtype": "success", "request_id": message["request_id"], "response": answer}}),
                    );
                }
                _ => {}
            }
        }
        std::process::exit(0);
    }
}

struct Fake {
    record: Record,
    out: Arc<Mutex<std::io::Stdout>>,
    session: String,
    script: Value,
    interrupted: Arc<Mutex<bool>>,
    next_request: u64,
}

impl Fake {
    fn send(&self, message: Value) {
        send(&self.out, message);
    }

    fn is_interrupted(&self) -> bool {
        *self.interrupted.lock().unwrap()
    }

    fn assistant(&self, block: Value) {
        self.send(
            json!({"type": "assistant", "parent_tool_use_id": null, "session_id": self.session,
            "message": {"model": MODEL, "role": "assistant", "content": [block],
                "usage": {"input_tokens": 100, "output_tokens": 20,
                    "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 50}}}),
        );
    }

    fn delta(&self, delta: Value) {
        self.send(
            json!({"type": "stream_event", "parent_tool_use_id": null, "session_id": self.session,
            "event": {"type": "content_block_delta", "index": 0, "delta": delta}}),
        );
    }

    fn play(&mut self, prompt: &Value, answered: &Receiver<Value>) {
        *self.interrupted.lock().unwrap() = false;
        let content = &prompt["message"]["content"];
        let text: String = match content {
            Value::String(text) => text.clone(),
            _ => content
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| block["text"].as_str())
                .collect(),
        };
        let turn = self.script["turns"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|turn| {
                turn["when"]
                    .as_str()
                    .is_some_and(|when| text.contains(when))
            })
            .cloned()
            .unwrap_or_else(|| json!({"do": [{"say": "ok"}]}));
        self.send(
            json!({"type": "system", "subtype": "init", "session_id": self.session,
            "model": MODEL, "claude_code_version": VERSION, "tools": [], "mcp_servers": []}),
        );
        let mut said = String::new();
        let mut limited = None;
        for step in turn["do"].as_array().into_iter().flatten() {
            if self.is_interrupted() {
                break;
            }
            if let Some(text) = step["say"].as_str() {
                said.push_str(text);
                self.delta(json!({"type": "text_delta", "text": text}));
                self.assistant(json!({"type": "text", "text": text}));
            } else if let Some(parts) = step["stream"].as_array() {
                let parts: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
                for part in &parts {
                    self.delta(json!({"type": "text_delta", "text": part}));
                }
                said.push_str(&parts.concat());
                self.assistant(json!({"type": "text", "text": parts.concat()}));
            } else if let Some(text) = step["think"].as_str() {
                self.delta(json!({"type": "thinking_delta", "thinking": text}));
                self.assistant(json!({"type": "thinking", "thinking": text, "signature": "x"}));
            } else if let Some(tool) = step.get("tool") {
                self.assistant(
                    json!({"type": "tool_use", "id": tool["id"], "name": tool["name"],
                    "input": tool["input"]}),
                );
                self.send(
                    json!({"type": "user", "parent_tool_use_id": null, "session_id": self.session,
                    "message": {"role": "user", "content": [{"type": "tool_result",
                        "tool_use_id": tool["id"], "content": tool["result"],
                        "is_error": tool["error"] == true}]}}),
                );
            } else if let Some(asked) = step.get("permission") {
                let answer = self.ask_permission(asked, answered);
                self.record.write(json!({"permission": answer}));
            } else if step.get("compaction").is_some() {
                self.send(
                    json!({"type": "system", "subtype": "status", "status": "compacting",
                    "session_id": self.session}),
                );
                self.send(json!({"type": "system", "subtype": "compact_boundary", "session_id": self.session,
                    "compact_metadata": {"trigger": "auto", "pre_tokens": 190000, "post_tokens": 12000}}));
            } else if step.get("retry").is_some() {
                self.send(
                    json!({"type": "system", "subtype": "api_retry", "session_id": self.session,
                    "attempt": 1, "max_retries": 10, "retry_delay_ms": 500, "error_status": 529,
                    "error": "overloaded"}),
                );
            } else if let Some(kind) = step["limit"].as_str() {
                self.send(
                    json!({"type": "rate_limit_event", "session_id": self.session,
                    "uuid": "u-limit", "rate_limit_info": {"status": "rejected",
                        "rateLimitType": kind, "resetsAt": 1792950000}}),
                );
                let text = "You've hit your limit · resets 3pm";
                self.send(json!({"type": "assistant", "parent_tool_use_id": null,
                    "session_id": self.session, "error": "rate_limit",
                    "message": {"model": "<synthetic>", "role": "assistant",
                        "content": [{"type": "text", "text": text}]}}));
                limited = Some(text);
            } else if step["garbage"] == true {
                let mut out = self.out.lock().unwrap();
                out.write_all(b"\xff\xfe not text\n").unwrap();
                out.flush().unwrap();
            } else if let Some(ms) = step["wait"].as_u64() {
                let deadline = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < deadline && !self.is_interrupted() {
                    std::thread::sleep(Duration::from_millis(10));
                }
            } else if step["hang"] == true {
                while !self.is_interrupted() {
                    std::thread::sleep(Duration::from_millis(10));
                }
            } else if let Some(code) = step["die"].as_i64() {
                std::process::exit(code as i32);
            }
        }
        let interrupted = self.is_interrupted();
        let error = turn["error"].as_str().or(limited);
        let subtype = match turn["subtype"].as_str() {
            Some(subtype) => subtype,
            None if interrupted => "error_during_execution",
            None => "success",
        };
        let mut result = json!({"type": "result", "subtype": subtype, "session_id": self.session,
            "is_error": error.is_some(), "num_turns": 1, "duration_ms": 10, "duration_api_ms": 8,
            "stop_reason": turn["stop_reason"].as_str().unwrap_or("end_turn"),
            "result": error.unwrap_or(&said), "total_cost_usd": 0.0125,
            "usage": {"input_tokens": 100, "output_tokens": 20,
                "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 50},
            "modelUsage": {MODEL: {"inputTokens": 100, "outputTokens": 20,
                "cacheReadInputTokens": 1000, "cacheCreationInputTokens": 50,
                "webSearchRequests": 0, "costUSD": 0.0125, "contextWindow": 200000,
                "maxOutputTokens": 32000}},
            "permission_denials": []});
        if let Some(error) = error {
            result["errors"] = json!([error]);
        }
        self.send(result);
    }

    fn ask_permission(&mut self, asked: &Value, answered: &Receiver<Value>) -> Value {
        let id = format!("permission-{}", self.next_request);
        self.next_request += 1;
        let tool = asked["tool"].as_str().unwrap_or("Bash");
        self.send(
            json!({"type": "control_request", "request_id": id, "request": {
            "subtype": "can_use_tool", "tool_name": tool, "input": asked["input"].clone(),
            "tool_use_id": format!("toolu_{id}")}}),
        );
        loop {
            match answered.recv_timeout(Duration::from_secs(30)) {
                Ok(answer) if answer["response"]["request_id"] == id.as_str() => {
                    return answer["response"].clone();
                }
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => return json!({"timeout": true}),
                Err(RecvTimeoutError::Disconnected) => return json!({"closed": true}),
            }
        }
    }
}
