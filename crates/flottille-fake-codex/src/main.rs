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
//!   that has none fails, as the real one does, and `thread/delete` removes it.
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
//! (`{"message", "codexErrorInfo"}`) ends `failed` with it. A turn's `then` is the steps of a
//! turn Codex starts on its own once that one completed (its id `own-<n>`). With `answer_late`,
//! the answer to `turn/start` comes after the turn completed, as Codex may send it. With
//! `resume_error`, every `thread/resume` fails as one of a thread that has none does.
//!
//! | Step | What the fake does |
//! |---|---|
//! | `{"say": "text"}` | an `agentMessage` item, its text as one delta |
//! | `{"say": ["te", "xt"]}` | an `agentMessage` item, streamed as one delta per piece |
//! | `{"think": "text"}` | a `reasoning` item, its text as one summary delta |
//! | `{"tool": {"id", "server", "tool", "arguments", "result", "error"}}` | an `mcpToolCall` item of the server (`flottille` when none is given), completed with the result, or failed with the error |
//! | `{"item": {…}}` | `item/started` with that item, as it is |
//! | `{"completed": {…}}` | `item/completed` with that item, as it is, never started |
//! | `{"compaction": true}` | a `contextCompaction` item |
//! | `{"retry": "text"}` | an `error` notification that Codex retries |
//! | `{"request": "method"}` | a request of Codex's own to the client, its answer recorded |
//! | `{"context": 45000}` | a `thread/tokenUsage/updated` mid-turn: that many tokens in the last request, the window 272000 |
//! | `{"wait": 500}` | waits that many milliseconds; an interrupt ends the turn |
//! | `{"hang": true}` | goes silent until an interrupt |
//! | `{"die": 3}` | exits at once with that code |
//!
//! `codex login status`, `codex login --device-auth` and `codex logout` play the scripted
//! sign-in every fake shares (`crates/flottille-fake-gh/src/sign_in.rs`, #1185).

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

#[path = "../../flottille-fake-gh/src/sign_in.rs"]
mod sign_in;

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

/// `codex login status`, `codex login --device-auth` and `codex logout`, as the sign-in every fake
/// shares (`crates/flottille-fake-gh/src/sign_in.rs`) plays them, in Codex's words (its device
/// code flow: the address, then the code, each on a line of its own); none for an app-server.
fn login(dir: &std::path::Path, args: &[String]) -> Option<i32> {
    let script = std::fs::read_to_string(dir.join("script.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}));
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    // `-c key=value` overrides come before the subcommand.
    let mut at = 0;
    while words.get(at) == Some(&"-c") {
        at += 2;
    }
    Some(match &words[at.min(words.len())..] {
        ["login", "status", ..] => match sign_in::signed_in(dir, &script) {
            true => {
                eprintln!("Logged in using ChatGPT");
                0
            }
            false => {
                eprintln!("Not logged in");
                1
            }
        },
        ["login", ..] => sign_in::login(dir, &script, args, |address, code| {
            eprintln!(
                "\nFollow these steps to sign in with ChatGPT using device code authorization:\n\n\
                 1. Open this link in your browser and sign in to your account\n   {address}\n\n\
                 2. Enter this one-time code (expires in 15 minutes)\n   {code}\n"
            );
        }),
        ["logout", ..] => sign_in::logout(dir, args),
        _ => return None,
    })
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
    if let Some(code) = login(&dir, &args) {
        std::process::exit(code);
    }
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
    let (turns, incoming) = mpsc::channel::<Prompt>();
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
    for prompt in incoming {
        fake.play(&prompt, &answered);
    }
}

/// A `turn/start` to play: its thread, its turn's id, its text, and its answer when it is late.
struct Prompt {
    thread: String,
    turn: String,
    text: String,
    answer: Option<Value>,
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
    fn run(mut self, turns: mpsc::Sender<Prompt>, answers: mpsc::Sender<Value>) {
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
                    if self.threads.join(thread).exists() && self.script["resume_error"] != true {
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
                    let answer = json!({"id": id, "result": {"turn": {"id": turn, "items": [],
                        "status": "inProgress", "error": null}}});
                    let late = self.script["answer_late"] == true;
                    if !late {
                        send(&self.out, answer.clone());
                    }
                    let prompt = Prompt {
                        thread: params["threadId"].as_str().unwrap_or_default().to_owned(),
                        turn,
                        text,
                        answer: late.then_some(answer),
                    };
                    if turns.send(prompt).is_err() {
                        break;
                    }
                    continue;
                }
                "thread/delete" => {
                    let thread = params["threadId"].as_str().unwrap_or_default();
                    match std::fs::remove_file(self.threads.join(thread)) {
                        Ok(()) => Ok(json!({})),
                        Err(_) => Err(format!("thread not found: {thread}")),
                    }
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
        delta: Option<(&str, &[&str])>,
        completed: Value,
    ) {
        self.notify(
            "item/started",
            json!({"threadId": thread, "turnId": turn, "item": started}),
        );
        let (method, pieces) = delta.unwrap_or_default();
        for text in pieces {
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

    fn play(&mut self, prompt: &Prompt, answered: &Receiver<Value>) {
        let script = self.script["turns"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| {
                t["when"]
                    .as_str()
                    .is_some_and(|when| prompt.text.contains(when))
            })
            .cloned()
            .unwrap_or_else(|| json!({"do": [{"say": "ok"}]}));
        self.turn(&prompt.thread, &prompt.turn, &script, answered);
        if let Some(answer) = &prompt.answer {
            send(&self.out, answer.clone());
        }
        if let Some(steps) = script.get("then") {
            *self.interrupted.lock().unwrap() = false;
            let own = self.id("own");
            self.turn(&prompt.thread, &own, &json!({"do": steps}), answered);
        }
    }

    /// One turn: started, its steps, the usage, completed.
    fn turn(&mut self, thread: &str, turn: &str, script: &Value, answered: &Receiver<Value>) {
        self.notify(
            "turn/started",
            json!({"threadId": thread, "turn": {"id": turn, "items": [], "status": "inProgress"}}),
        );
        for step in script["do"].as_array().into_iter().flatten() {
            if self.is_interrupted() {
                break;
            }
            self.step(thread, turn, step, answered);
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

    fn step(&mut self, thread: &str, turn: &str, step: &Value, answered: &Receiver<Value>) {
        if let Some(said) = step.get("say").filter(|s| !s.is_null()) {
            let pieces: Vec<&str> = match said.as_array() {
                Some(pieces) => pieces.iter().filter_map(Value::as_str).collect(),
                None => said.as_str().into_iter().collect(),
            };
            let id = self.id("msg");
            self.item(
                thread,
                turn,
                json!({"type": "agentMessage", "id": id, "text": ""}),
                Some(("item/agentMessage/delta", &pieces)),
                json!({"type": "agentMessage", "id": id, "text": pieces.concat()}),
            );
        } else if let Some(text) = step["think"].as_str() {
            let id = self.id("rs");
            self.item(
                thread,
                turn,
                json!({"type": "reasoning", "id": id, "summary": [], "content": []}),
                Some(("item/reasoning/summaryTextDelta", &[text])),
                json!({"type": "reasoning", "id": id, "summary": [text], "content": []}),
            );
        } else if let Some(tool) = step.get("tool") {
            let server = tool.get("server").cloned().unwrap_or("flottille".into());
            let call = json!({"type": "mcpToolCall", "id": tool["id"], "server": server,
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
        } else if let Some(item) = step.get("completed") {
            self.notify(
                "item/completed",
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
        } else if let Some(tokens) = step["context"].as_u64() {
            let usage = json!({"totalTokens": tokens, "inputTokens": tokens, "cachedInputTokens": 0,
                "cacheWriteInputTokens": 0, "outputTokens": 0, "reasoningOutputTokens": 0});
            self.notify(
                "thread/tokenUsage/updated",
                json!({"threadId": thread, "turnId": turn, "tokenUsage": {"total": usage,
                    "last": usage, "modelContextWindow": 272000}}),
            );
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
}
