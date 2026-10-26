//! A scripted `codex app-server` for the tests of Flottille's Codex bridge: it speaks the
//! app-server protocol on stdio (JSON-RPC lines without a `jsonrpc` field), as the version the
//! bridge is pinned to does, and does what a script says, so that no test ever runs the real one.
//!
//! # Its folder
//!
//! Everything goes through one folder, named by `FLOTTILLE_FAKE_CODEX_DIR`:
//!
//! - `script.json`, written by the test (optional: without it, every prompt is answered `ok`);
//! - `received.jsonl`, written by the fake: one JSON object per line, appended. When it starts:
//!   `{"pid", "started": {"argv", "env", "cwd"}}`. Then every message it read
//!   (`{"pid", "received": <the message>}`), and the answers to its own requests
//!   (`{"pid", "answered": <the answer>}`);
//! - `threads/<id>`, one empty file per thread `thread/start` made: `thread/resume` of a thread
//!   that has none fails, as the real one does.
//!
//! # The script
//!
//! ```json
//! {"turns": [{"when": "hello", "do": [{"think": "a greeting"}, {"say": "Hello!"}]}]}
//! ```
//!
//! `account`, when given, replaces the account `account/read` answers (`null`: not signed in);
//! `models` the models `model/list` answers (`MODELS`). A prompt plays the first turn whose
//! `when` is in its text; a prompt matching none is answered `ok`. A turn whose `error` is given
//! (`{"message", "codexErrorInfo"}`) ends `failed` with it.
//!
//! | Step | What the fake does |
//! |---|---|
//! | `{"say": "text"}` | an `agentMessage` item, its text as one delta |
//! | `{"think": "text"}` | a `reasoning` item, its text as one summary delta |
//! | `{"tool": {"id", "tool", "arguments", "result", "error"}}` | an `mcpToolCall` item of the server `flottille`, completed with the result, or failed with the error |
//! | `{"item": {…}}` | `item/started` with that item, as it is |
//! | `{"compaction": true}` | a `contextCompaction` item |
//! | `{"retry": "text"}` | an `error` notification that Codex retries |
//! | `{"request": "method"}` | a request of Codex's own to the client, its answer recorded |
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

/// The version the bridge was checked against: `codex --version` prints it.
const VERSION: &str = "0.162.0";

/// The models `model/list` answers, as Codex describes them.
fn models() -> Value {
    json!([
        {"id": "gpt-fake", "model": "gpt-fake", "displayName": "GPT Fake", "description": "",
            "hidden": false, "isDefault": true, "defaultReasoningEffort": "medium",
            "supportedReasoningEfforts": [
                {"reasoningEffort": "low", "description": ""},
                {"reasoningEffort": "medium", "description": ""},
                {"reasoningEffort": "high", "description": ""}]},
        {"id": "gpt-fake-mini", "model": "gpt-fake-mini", "displayName": "GPT Fake Mini",
            "description": "", "hidden": false, "isDefault": false,
            "defaultReasoningEffort": "low",
            "supportedReasoningEfforts": [
                {"reasoningEffort": "minimal", "description": ""},
                {"reasoningEffort": "low", "description": ""}]},
        {"id": "gpt-fake-hidden", "model": "gpt-fake-hidden", "displayName": "Hidden",
            "description": "", "hidden": true, "isDefault": false,
            "defaultReasoningEffort": "low", "supportedReasoningEfforts": []}
    ])
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--version") {
        println!("codex-cli {VERSION}");
        return;
    }
    let dir = PathBuf::from(
        std::env::var_os("FLOTTILLE_FAKE_CODEX_DIR").expect("FLOTTILLE_FAKE_CODEX_DIR is not set"),
    );
    let record = Record(dir.join("received.jsonl"));
    let env: Map<String, Value> = std::env::vars().map(|(k, v)| (k, v.into())).collect();
    let cwd = std::env::current_dir().unwrap();
    record.write(json!({"started": {"argv": args, "env": env, "cwd": cwd}}));
    let threads = dir.join("threads");
    std::fs::create_dir_all(&threads).unwrap();
    let script = match std::fs::read_to_string(dir.join("script.json")) {
        Ok(text) => serde_json::from_str(&text).expect("script.json is not JSON"),
        Err(_) => json!({}),
    };

    // A reader thread, so that an interrupt or an answer arrives while a turn plays.
    let (turns, incoming) = mpsc::channel::<(String, String)>();
    let (answers, answered) = mpsc::channel::<Value>();
    let interrupted = Arc::new(Mutex::new(false));
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let reader = Reader {
        script: script.clone(),
        threads,
        record: record.clone(),
        out: out.clone(),
        interrupted: interrupted.clone(),
        next_turn: 0,
    };
    std::thread::spawn(move || reader.run(turns, answers));

    let mut fake = Fake {
        record,
        out,
        script,
        interrupted,
        next: 0,
    };
    for (thread, turn) in incoming {
        fake.play(&thread, &turn, &answered);
    }
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
    script: Value,
    threads: PathBuf,
    record: Record,
    out: Arc<Mutex<std::io::Stdout>>,
    interrupted: Arc<Mutex<bool>>,
    next_turn: u64,
}

impl Reader {
    fn run(mut self, turns: mpsc::Sender<(String, String)>, answers: mpsc::Sender<Value>) {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            self.record.write(json!({"received": message}));
            let Some(method) = message["method"].as_str() else {
                let _ = answers.send(message);
                continue;
            };
            let id = message["id"].clone();
            if id.is_null() {
                continue;
            }
            let params = &message["params"];
            let answer = match method {
                "initialize" => Ok(json!({"userAgent": format!("codex/{VERSION}"),
                    "codexHome": std::env::var("CODEX_HOME").unwrap_or_default(),
                    "platformFamily": "unix", "platformOs": "linux"})),
                "account/read" => Ok(json!({"account": self.script.get("account").cloned()
                    .unwrap_or_else(|| json!({"type": "chatgpt", "email": "someone@example.com",
                        "planType": "plus"})),
                    "requiresOpenaiAuth": true})),
                "model/list" => Ok(json!({"data": self.script.get("models").cloned()
                    .unwrap_or_else(models), "nextCursor": null})),
                "thread/start" => {
                    let thread = format!("thread-{}", std::process::id());
                    std::fs::write(self.threads.join(&thread), "").unwrap();
                    Ok(json!({"thread": {"id": thread}, "model": "gpt-fake",
                        "modelProvider": "openai", "cwd": params["cwd"],
                        "reasoningEffort": "medium"}))
                }
                "thread/resume" => {
                    let thread = params["threadId"].as_str().unwrap_or_default();
                    if self.threads.join(thread).exists() {
                        Ok(json!({"thread": {"id": thread}, "model": "gpt-fake",
                            "modelProvider": "openai", "cwd": params["cwd"],
                            "reasoningEffort": "medium"}))
                    } else {
                        Err(format!("no rollout found for thread id {thread}"))
                    }
                }
                "turn/start" => {
                    self.next_turn += 1;
                    let turn = format!("turn-{}", self.next_turn);
                    *self.interrupted.lock().unwrap() = false;
                    let text: String = params["input"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|input| input["text"].as_str())
                        .collect();
                    send(
                        &self.out,
                        json!({"id": id, "result": {"turn": {"id": turn, "items": [],
                            "status": "inProgress", "error": null}}}),
                    );
                    let thread = params["threadId"].as_str().unwrap_or_default().to_owned();
                    if turns.send((thread, format!("{turn}\n{text}"))).is_err() {
                        break;
                    }
                    continue;
                }
                "turn/interrupt" => {
                    *self.interrupted.lock().unwrap() = true;
                    Ok(json!({}))
                }
                _ => Err(format!("unknown method {method}")),
            };
            let reply = match answer {
                Ok(result) => json!({"id": id, "result": result}),
                Err(message) => json!({"id": id, "error": {"code": -32600, "message": message}}),
            };
            send(&self.out, reply);
        }
        std::process::exit(0);
    }
}

struct Fake {
    record: Record,
    out: Arc<Mutex<std::io::Stdout>>,
    script: Value,
    interrupted: Arc<Mutex<bool>>,
    next: u64,
}

impl Fake {
    fn notify(&self, method: &str, params: Value) {
        send(&self.out, json!({"method": method, "params": params}));
    }

    fn is_interrupted(&self) -> bool {
        *self.interrupted.lock().unwrap()
    }

    fn item(
        &mut self,
        thread: &str,
        turn: &str,
        started: Value,
        delta: Option<(&str, &str)>,
        completed: Value,
    ) {
        self.notify(
            "item/started",
            json!({"threadId": thread, "turnId": turn, "item": started}),
        );
        if let Some((method, text)) = delta {
            self.notify(
                method,
                json!({"threadId": thread, "turnId": turn, "itemId": started["id"],
                    "delta": text, "summaryIndex": 0}),
            );
        }
        self.notify(
            "item/completed",
            json!({"threadId": thread, "turnId": turn, "item": completed}),
        );
    }

    fn id(&mut self, kind: &str) -> String {
        self.next += 1;
        format!("{kind}-{}", self.next)
    }

    fn play(&mut self, thread: &str, prompt: &str, answered: &Receiver<Value>) {
        let (turn, text) = prompt.split_once('\n').unwrap_or((prompt, ""));
        let script = self.script["turns"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["when"].as_str().is_some_and(|when| text.contains(when)))
            .cloned()
            .unwrap_or_else(|| json!({"do": [{"say": "ok"}]}));
        self.notify(
            "turn/started",
            json!({"threadId": thread, "turn": {"id": turn, "items": [], "status": "inProgress"}}),
        );
        for step in script["do"].as_array().into_iter().flatten() {
            if self.is_interrupted() {
                break;
            }
            if let Some(text) = step["say"].as_str() {
                let id = self.id("msg");
                self.item(
                    thread,
                    turn,
                    json!({"type": "agentMessage", "id": id, "text": ""}),
                    Some(("item/agentMessage/delta", text)),
                    json!({"type": "agentMessage", "id": id, "text": text}),
                );
            } else if let Some(text) = step["think"].as_str() {
                let id = self.id("rs");
                self.item(
                    thread,
                    turn,
                    json!({"type": "reasoning", "id": id, "summary": [], "content": []}),
                    Some(("item/reasoning/summaryTextDelta", text)),
                    json!({"type": "reasoning", "id": id, "summary": [text], "content": []}),
                );
            } else if let Some(tool) = step.get("tool") {
                let call = json!({"type": "mcpToolCall", "id": tool["id"], "server": "flottille",
                    "tool": tool["tool"], "status": "inProgress", "arguments": tool["arguments"],
                    "result": null, "error": null, "durationMs": null});
                let mut done = call.clone();
                if let Some(error) = tool.get("error") {
                    done["status"] = "failed".into();
                    done["error"] = json!({"message": error});
                } else {
                    done["status"] = "completed".into();
                    done["result"] = json!({"content": tool["result"], "structuredContent": null});
                }
                done["durationMs"] = 12.into();
                self.item(thread, turn, call, None, done);
            } else if let Some(item) = step.get("item") {
                self.notify(
                    "item/started",
                    json!({"threadId": thread, "turnId": turn, "item": item}),
                );
            } else if step.get("compaction").is_some() {
                let id = self.id("compact");
                let item = json!({"type": "contextCompaction", "id": id});
                self.item(thread, turn, item.clone(), None, item);
            } else if let Some(text) = step["retry"].as_str() {
                self.notify(
                    "error",
                    json!({"threadId": thread, "turnId": turn, "willRetry": true,
                        "error": {"message": text, "codexErrorInfo": "serverOverloaded"}}),
                );
            } else if let Some(method) = step["request"].as_str() {
                let id = self.id("codex-request");
                send(
                    &self.out,
                    json!({"id": id, "method": method, "params": {"threadId": thread,
                        "turnId": turn, "itemId": "call-1", "command": "ls"}}),
                );
                let answer = loop {
                    match answered.recv_timeout(Duration::from_secs(30)) {
                        Ok(answer) if answer["id"] == id.as_str() => break answer,
                        Ok(_) => continue,
                        Err(RecvTimeoutError::Timeout) => break json!({"timeout": true}),
                        Err(RecvTimeoutError::Disconnected) => break json!({"closed": true}),
                    }
                };
                self.record.write(json!({"answered": answer}));
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
        let usage = json!({"totalTokens": 1300, "inputTokens": 1200, "cachedInputTokens": 1000,
            "cacheWriteInputTokens": 0, "outputTokens": 100, "reasoningOutputTokens": 40});
        self.notify(
            "thread/tokenUsage/updated",
            json!({"threadId": thread, "turnId": turn, "tokenUsage": {"total": usage,
                "last": usage, "modelContextWindow": 272000}}),
        );
        let (status, error) = if self.is_interrupted() {
            ("interrupted", Value::Null)
        } else if let Some(error) = script.get("error") {
            ("failed", error.clone())
        } else {
            ("completed", Value::Null)
        };
        self.notify(
            "turn/completed",
            json!({"threadId": thread, "turn": {"id": turn, "items": [], "status": status,
                "error": error}}),
        );
    }
}
