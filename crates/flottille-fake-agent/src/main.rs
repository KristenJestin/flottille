//! A scripted ACP agent for Flottille's tests: it speaks ACP v1 on stdio, as an adapter does, and
//! does what a script says, so that no test ever runs a real agent.
//!
//! # Its folder
//!
//! Everything goes through one folder, named by `FLOTTILLE_FAKE_AGENT_DIR`, or, when it is not
//! set, the folder of the path it was started by (a link to it in a folder of its own, for each
//! provider a test stands it for):
//!
//! - `script.json`, written by the test (optional: without it, every prompt is answered `ok`);
//! - `received.jsonl`, written by the agent: one JSON object per line, appended, for every message
//!   it received (`{"pid", "received": <the JSON-RPC message as it came>}`), and for what it did
//!   that a test checks (`{"pid", "started": true, "env": {…}}`, `{"pid", "permission": <the
//!   client's answer>}`, `{"pid", "mcp": {"status", "body"}}`); `env` holds the variables it was
//!   started with whose names start with `OPENCODE_` or `XDG_`, a password's value as `set` (or
//!   empty, when it is);
//! - `sessions/<id>.jsonl`, the history of each session, so that a new process can load it;
//! - `played.json`, how many times each turn with `times` was played, across processes;
//! - `kept.json`, the values `keep` kept, across processes.
//!
//! # The script
//!
//! ```json
//! {
//!   "load": true,
//!   "resume": true,
//!   "linger": false,
//!   "child": false,
//!   "stall": false,
//!   "models": ["fake-small", "fake-large"],
//!   "efforts": ["low", "high"],
//!   "turns": [
//!     {"when": "hello", "do": [{"think": "a greeting"}, {"say": "Hello!"}], "stop": "end_turn"}
//!   ]
//! }
//! ```
//!
//! - `load`, `resume`: whether `initialize` offers `session/load` and `session/resume` (both by
//!   default). A loaded session replays its history first; a resumed one does not.
//! - `images`: whether `initialize` says the agent takes images in a prompt (false by default).
//! - `linger`: keep running once standard input closes, as an orphan would (false by default).
//! - `child`: start a `sleep` in the process group at startup, recording its pid (`{"child": <pid>}`).
//! - `stall`: never answer `session/new`, `session/load` or `session/resume`: an agent stuck while
//!   it opens.
//! - `models`, `efforts`: the values of the `model` and `thought_level` options, the first one
//!   current; no effort option without `efforts`.
//! - `turns`: a prompt plays the first turn whose `when` is in its text; a prompt matching none is
//!   answered `ok`. `stop` is the stop reason (`end_turn` by default). A turn with `"times": n`
//!   is played n times at most, by every process of the folder together: after that it is skipped,
//!   and the prompt plays the next turn that matches.
//!
//! The steps of a turn, in order:
//!
//! | Step | What the agent does |
//! |---|---|
//! | `{"say": "text"}` | an `agent_message_chunk` |
//! | `{"say": "text", "message": "m1"}` | the same, its `messageId` `m1`: the chunks that share one belong to one message |
//! | `{"say_variable": "NAME"}` | the value of that variable of its environment, in three `agent_message_chunk`s |
//! | `{"update": {…}}` | that session update, as it is |
//! | `{"recall": true}` | an `agent_message_chunk` saying what the user said earlier in this session (`earlier you said: a | b`): the proof that a session kept its history |
//! | `{"think": "text"}` | an `agent_thought_chunk` |
//! | `{"usage": {"used": 190000, "size": 200000}}` | a `usage_update` |
//! | `{"compaction": true}` | a completed tool call carrying `_meta.jetbrains.air.contextCompaction`, as the Claude adapter reports a compaction |
//! | `{"permission": {"tool": "Bash", "title": "ls"}}` | a `session/request_permission` for that tool, answer recorded |
//! | `{"permission": {"title": "x", "untyped": true}}` | the same with no `_meta.claudeCode.toolName`: a request that has only a title |
//! | `{"mcp": {"tool": "fs_list", "arguments": {}}}` | calls the tool on the session's first HTTP MCP server (initialize, then `tools/call`), as a tool call; the answer recorded |
//! | `{"mcp": {"tool": "agent_launch", "arguments": {…}, "keep": "w1"}}` | the same, and keeps the first agent id (12 hexadecimal digits) of the answer's text under that name |
//! | `{"mcp": {"tool": "fs_list", "arguments": {}, "quiet": true}}` | the same without its tool call's session updates: an MCP call that lands before the updates around it |
//!
//! In the texts of an `mcp` step's arguments, `{kept:w1}` is replaced by the value kept under
//! `w1`, `{prompt:request}` by the first request number (`#3`) of the turn's prompt (a number
//! when it is the whole text), and
//! `{prompt:agent}` by the first agent id of the turn's prompt. Together they let a scripted lead
//! launch a worker and write to it, or a parent answer the request its child's ask names.
//! | `{"wait": 500}` | waits that many milliseconds; a cancel ends the turn `cancelled` |
//! | `{"hang": true}` | goes silent until the turn is cancelled |
//! | `{"child": true}` | starts a `sleep` in its own process group, as the CLI the adapter runs would be, and records its pid (`{"child": <pid>}`) |
//! | `{"die": 3}` | exits at once with that code |

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

fn main() {
    let dir = match std::env::var_os("FLOTTILLE_FAKE_AGENT_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::args_os().next().expect("a program name"))
            .parent()
            .expect("FLOTTILLE_FAKE_AGENT_DIR is not set, and the program has no folder")
            .to_owned(),
    };
    let script = match std::fs::read_to_string(dir.join("script.json")) {
        Ok(text) => serde_json::from_str(&text).expect("script.json is not JSON"),
        Err(_) => json!({}),
    };
    let agent = Agent {
        record: Record(dir.join("received.jsonl")),
        sessions: dir.join("sessions"),
        script,
        out: Arc::new(Mutex::new(std::io::stdout())),
        next_request: 0,
        options: HashMap::new(),
    };
    let env: Map<String, Value> = std::env::vars()
        .filter(|(name, _)| name.starts_with("OPENCODE_") || name.starts_with("XDG_"))
        .map(|(name, value)| {
            let value = if name.ends_with("_PASSWORD") {
                // Never written down; an empty one shows.
                if value.is_empty() { "" } else { "set" }.to_owned()
            } else {
                value
            };
            (name, Value::String(value))
        })
        .collect();
    agent.record.write(json!({"started": true, "env": env}));
    if agent.script["child"] == true {
        agent.start_child();
    }
    let linger = agent.script["linger"] == true;

    // A reader thread so that a cancel or an answer to a permission request arrives while a turn
    // plays.
    let (requests, incoming) = mpsc::channel::<Value>();
    let (answers, answered) = mpsc::channel::<Value>();
    let record = agent.record.clone();
    let cancelled = Arc::new(Mutex::new(false));
    let cancel = cancelled.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            record.write(json!({"received": message}));
            if message.get("method").is_none() {
                let _ = answers.send(message);
            } else if message["method"] == "session/cancel" {
                *cancel.lock().unwrap() = true;
            } else if requests.send(message).is_err() {
                break;
            }
        }
        if linger {
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        std::process::exit(0);
    });
    let mut agent = agent;
    for request in incoming {
        let answer = agent.handle(&request, &answered, &cancelled);
        let id = request["id"].clone();
        match answer {
            Ok(result) => agent.send(json!({"jsonrpc": "2.0", "id": id, "result": result})),
            Err(message) => agent.send(
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": message}}),
            ),
        }
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
        // One write: the agents of a tree share the file, and their lines must not mix.
        file.write_all(format!("{entry}\n").as_bytes()).unwrap();
    }
}

struct Agent {
    record: Record,
    sessions: PathBuf,
    script: Value,
    out: Arc<Mutex<std::io::Stdout>>,
    next_request: u64,
    /// The current value of each option, per session.
    options: HashMap<String, Map<String, Value>>,
}

impl Agent {
    fn send(&self, message: Value) {
        let mut out = self.out.lock().unwrap();
        writeln!(out, "{message}").unwrap();
        out.flush().unwrap();
    }

    fn notify(&self, session: &str, update: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": "session/update",
            "params": {"sessionId": session, "update": update}}));
    }

    fn handle(
        &mut self,
        request: &Value,
        answered: &Receiver<Value>,
        cancelled: &Mutex<bool>,
    ) -> Result<Value, String> {
        let params = &request["params"];
        let session = params["sessionId"].as_str().unwrap_or_default().to_owned();
        let method = request["method"].as_str().unwrap_or_default();
        if self.script["stall"] == true
            && method.starts_with("session/")
            && method != "session/prompt"
        {
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        match method {
            "initialize" => {
                let mut capabilities = json!({
                    "loadSession": self.script["load"] != false,
                    "promptCapabilities": {"image": self.script["images"] == true, "embeddedContext": true},
                    "mcpCapabilities": {"http": true},
                    "sessionCapabilities": {},
                });
                if self.script["resume"] != false {
                    capabilities["sessionCapabilities"]["resume"] = json!({});
                }
                Ok(json!({
                    "protocolVersion": 1,
                    "agentCapabilities": capabilities,
                    "agentInfo": {"name": "flottille-fake-agent", "version": env!("CARGO_PKG_VERSION")},
                    "authMethods": [],
                }))
            }
            "session/new" => {
                let id = format!("fake-{}-{}", std::process::id(), self.next_request);
                self.next_request += 1;
                std::fs::create_dir_all(&self.sessions).unwrap();
                std::fs::write(self.history_file(&id), "").unwrap();
                self.keep_session(&id, params);
                Ok(json!({"sessionId": id, "configOptions": self.config_options(&id)}))
            }
            "session/load" => {
                let history = std::fs::read_to_string(self.history_file(&session))
                    .map_err(|_| format!("no session {session}"))?;
                self.keep_session(&session, params);
                for line in history.lines() {
                    let entry: Value = serde_json::from_str(line).unwrap();
                    let kind = if entry["user"].is_string() {
                        "user_message_chunk"
                    } else {
                        "agent_message_chunk"
                    };
                    let text = entry["user"].as_str().or(entry["agent"].as_str());
                    self.notify(
                        &session,
                        json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}),
                    );
                }
                Ok(json!({"configOptions": self.config_options(&session)}))
            }
            "session/resume" => {
                if !self.history_file(&session).exists() {
                    return Err(format!("no session {session}"));
                }
                self.keep_session(&session, params);
                Ok(json!({"configOptions": self.config_options(&session)}))
            }
            "session/set_config_option" => {
                let option = params["configId"].as_str().unwrap_or_default();
                let value = params["value"].as_str().unwrap_or_default();
                let offered = match option {
                    "model" => &self.script["models"],
                    "effort" => &self.script["efforts"],
                    _ => return Err(format!("no option {option}")),
                };
                if !self.values(offered, option).iter().any(|v| v == value) {
                    return Err(format!("{value} is not a value of {option}"));
                }
                self.options
                    .entry(session.clone())
                    .or_default()
                    .insert(option.to_owned(), value.into());
                Ok(json!({"configOptions": self.config_options(&session)}))
            }
            "session/prompt" => {
                *cancelled.lock().unwrap() = false;
                let text: String = params["prompt"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|block| block["text"].as_str())
                    .collect();
                self.remember(&session, json!({"user": text}));
                let turn = self.script["turns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .find(|(index, turn)| {
                        turn["when"]
                            .as_str()
                            .is_some_and(|when| text.contains(when))
                            && self.may_play(*index, turn)
                    })
                    .map(|(_, turn)| turn.clone())
                    .unwrap_or_else(|| json!({"do": [{"say": "ok"}]}));
                let stop = self.play(&session, &text, &turn, answered, cancelled);
                Ok(json!({"stopReason": stop}))
            }
            method => Err(format!("the fake agent does not know {method}")),
        }
    }

    /// Whether the turn may be played once more, counting this time when it may.
    fn may_play(&self, index: usize, turn: &Value) -> bool {
        let Some(times) = turn["times"].as_u64() else {
            return true;
        };
        let file = self.sessions.with_file_name("played.json");
        let mut played: Map<String, Value> = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let count = played
            .get(&index.to_string())
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if count >= times {
            return false;
        }
        played.insert(index.to_string(), (count + 1).into());
        std::fs::write(&file, Value::Object(played).to_string()).unwrap();
        true
    }

    fn kept(&self) -> Map<String, Value> {
        std::fs::read_to_string(self.sessions.with_file_name("kept.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn keep(&self, name: &str, value: &str) {
        let mut kept = self.kept();
        kept.insert(name.to_owned(), value.into());
        std::fs::write(
            self.sessions.with_file_name("kept.json"),
            Value::Object(kept).to_string(),
        )
        .unwrap();
    }

    /// The value with its placeholders filled: kept values, and what the prompt names.
    fn filled(&self, value: &Value, prompt: &str) -> Value {
        match value {
            // A number where a number is meant.
            Value::String(text) if text == "{prompt:request}" => request_number(prompt)
                .and_then(|n| n.parse::<u64>().ok())
                .map_or_else(|| value.clone(), Value::from),
            Value::String(text) => {
                let mut text = text.clone();
                for (name, kept) in self.kept() {
                    text = text.replace(&format!("{{kept:{name}}}"), kept.as_str().unwrap_or(""));
                }
                if let Some(number) = request_number(prompt) {
                    text = text.replace("{prompt:request}", &number);
                }
                if let Some(id) = agent_id(prompt) {
                    text = text.replace("{prompt:agent}", &id);
                }
                Value::String(text)
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|v| self.filled(v, prompt)).collect())
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.filled(v, prompt)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Plays a turn's steps; the stop reason.
    fn play(
        &mut self,
        session: &str,
        prompt: &str,
        turn: &Value,
        answered: &Receiver<Value>,
        cancelled: &Mutex<bool>,
    ) -> String {
        let is_cancelled = || *cancelled.lock().unwrap();
        for step in turn["do"].as_array().into_iter().flatten() {
            if is_cancelled() {
                return "cancelled".into();
            }
            if let Some(text) = step["say"].as_str() {
                self.say(session, text, step["message"].as_str());
            } else if let Some(name) = step["say_variable"].as_str() {
                let value: Vec<char> = std::env::var(name).unwrap_or_default().chars().collect();
                let third = value.len().div_ceil(3).max(1);
                for piece in value.chunks(third) {
                    self.say(session, &piece.iter().collect::<String>(), None);
                }
            } else if let Some(update) = step.get("update") {
                self.notify(session, update.clone());
            } else if step["recall"] == true {
                let said = self.said_by_the_user(session);
                self.say(
                    session,
                    &format!("earlier you said: {}", said.join(" | ")),
                    None,
                );
            } else if let Some(text) = step["think"].as_str() {
                self.notify(
                    session,
                    json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": text}}),
                );
            } else if let Some(usage) = step.get("usage") {
                self.notify(
                    session,
                    json!({"sessionUpdate": "usage_update", "used": usage["used"], "size": usage["size"]}),
                );
            } else if step.get("compaction").is_some() {
                self.notify(
                    session,
                    json!({"sessionUpdate": "tool_call", "toolCallId": "compaction-1",
                        "title": "Compacting the conversation", "kind": "other", "status": "completed",
                        "_meta": {"jetbrains": {"air": {"contextCompaction": {"version": 1}}}}}),
                );
            } else if let Some(asked) = step.get("permission") {
                let answer = self.ask_permission(session, asked, answered);
                self.record.write(json!({"permission": answer}));
            } else if let Some(call) = step.get("mcp") {
                let call = self.filled(call, prompt);
                self.call_mcp(session, &call);
            } else if let Some(ms) = step["wait"].as_u64() {
                let deadline = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < deadline {
                    if is_cancelled() {
                        return "cancelled".into();
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            } else if step["hang"] == true {
                while !is_cancelled() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                return "cancelled".into();
            } else if step["child"] == true {
                self.start_child();
            } else if let Some(code) = step["die"].as_i64() {
                std::process::exit(code as i32);
            }
        }
        if is_cancelled() {
            return "cancelled".into();
        }
        turn["stop"].as_str().unwrap_or("end_turn").to_owned()
    }

    /// A `sleep` in this process's group, never waited for: it is meant to outlive this process.
    #[expect(
        clippy::zombie_processes,
        reason = "the child is meant to outlive its parent"
    )]
    fn start_child(&self) {
        let child = std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .expect("sleep starts");
        self.record.write(json!({"child": child.id()}));
    }

    fn ask_permission(
        &mut self,
        session: &str,
        asked: &Value,
        answered: &Receiver<Value>,
    ) -> Value {
        let id = format!("permission-{}", self.next_request);
        self.next_request += 1;
        let tool = asked["tool"].as_str().unwrap_or("Bash");
        let mut call = json!({"toolCallId": format!("call-{id}"),
            "title": asked["title"].as_str().unwrap_or(tool)});
        if asked["untyped"] != true {
            call["_meta"] = json!({"claudeCode": {"toolName": tool}});
        }
        self.send(
            json!({"jsonrpc": "2.0", "id": id, "method": "session/request_permission",
            "params": {
                "sessionId": session,
                "toolCall": call,
                "options": [
                    {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                    {"optionId": "allow-always", "name": "Always allow", "kind": "allow_always"},
                    {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
                ],
            }}),
        );
        loop {
            match answered.recv_timeout(Duration::from_secs(30)) {
                Ok(answer) if answer["id"] == id.as_str() => {
                    return answer.get("result").cloned().unwrap_or(answer);
                }
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => return json!({"timeout": true}),
                Err(RecvTimeoutError::Disconnected) => return json!({"closed": true}),
            }
        }
    }

    fn call_mcp(&mut self, session: &str, call: &Value) {
        let tool = call["tool"].as_str().unwrap_or_default();
        let call_id = format!("mcp-{}", self.next_request);
        self.next_request += 1;
        let quiet = call["quiet"] == true;
        if !quiet {
            self.notify(
                session,
                json!({"sessionUpdate": "tool_call", "toolCallId": call_id, "title": tool,
                    "kind": "other", "status": "in_progress", "rawInput": call["arguments"]}),
            );
        }
        let servers = self.options[session]["mcpServers"].clone();
        let server = servers
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["type"] == "http");
        let outcome = match server {
            None => Err("the session has no HTTP MCP server".to_owned()),
            Some(server) => mcp_call(server, tool, &call["arguments"]),
        };
        let (status, body) = match &outcome {
            Ok((status, body)) => (*status, body.clone()),
            Err(error) => (0, json!({"error": error})),
        };
        self.record
            .write(json!({"mcp": {"status": status, "body": body}, "session": session}));
        if let Some(name) = call["keep"].as_str() {
            let text = body["result"]["content"][0]["text"].as_str().unwrap_or("");
            if let Some(id) = agent_id(text) {
                self.keep(name, &id);
            }
        }
        if !quiet {
            self.notify(
                session,
                json!({"sessionUpdate": "tool_call_update", "toolCallId": call_id,
                    "status": if status == 200 { "completed" } else { "failed" }, "rawOutput": body}),
            );
        }
    }

    fn say(&self, session: &str, text: &str, message: Option<&str>) {
        self.remember(session, json!({"agent": text}));
        let mut update = json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}});
        if let Some(message) = message {
            update["messageId"] = message.into();
        }
        self.notify(session, update);
    }

    /// What the user said in the session before the prompt that runs, the oldest first.
    fn said_by_the_user(&self, session: &str) -> Vec<String> {
        let history = std::fs::read_to_string(self.history_file(session)).unwrap_or_default();
        let mut said: Vec<String> = history
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|entry| entry["user"].as_str().map(str::to_owned))
            .collect();
        said.pop();
        said
    }

    fn history_file(&self, session: &str) -> PathBuf {
        self.sessions.join(format!("{session}.jsonl"))
    }

    fn remember(&self, session: &str, entry: Value) {
        if let Ok(mut file) = OpenOptions::new()
            .append(true)
            .open(self.history_file(session))
        {
            writeln!(file, "{entry}").unwrap();
        }
    }

    /// Keeps what the session was opened with: its MCP servers, and the first value of each
    /// option.
    fn keep_session(&mut self, session: &str, params: &Value) {
        let options = self.options.entry(session.to_owned()).or_default();
        options.insert("mcpServers".into(), params["mcpServers"].clone());
        for (option, values) in [("model", "models"), ("effort", "efforts")] {
            if !options.contains_key(option)
                && let Some(first) = self.script[values].get(0)
            {
                options.insert(option.into(), first.clone());
            }
        }
    }

    fn values(&self, offered: &Value, option: &str) -> Vec<String> {
        let defaults = if option == "model" {
            vec!["fake-default".to_owned()]
        } else {
            vec![]
        };
        match offered.as_array() {
            Some(values) => values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            None => defaults,
        }
    }

    fn config_options(&self, session: &str) -> Value {
        let current = self.options.get(session);
        let select = |id: &str, name: &str, category: &str, values: Vec<String>| {
            let value = current
                .and_then(|c| c.get(id))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| values.first().cloned())
                .unwrap_or_default();
            json!({"id": id, "name": name, "category": category, "type": "select",
                "currentValue": value,
                "options": values.iter().map(|v| json!({"value": v, "name": v})).collect::<Vec<_>>()})
        };
        let mut options = vec![select(
            "model",
            "Model",
            "model",
            self.values(&self.script["models"], "model"),
        )];
        if self.script["efforts"].is_array() {
            options.push(select(
                "effort",
                "Effort",
                "thought_level",
                self.values(&self.script["efforts"], "effort"),
            ));
        }
        Value::Array(options)
    }
}

/// The first request number of the text: the digits after its first `#`.
fn request_number(text: &str) -> Option<String> {
    text.split('#').skip(1).find_map(|rest| {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        (!digits.is_empty()).then_some(digits)
    })
}

/// The first agent id of the text: a word of 12 lowercase hexadecimal digits.
fn agent_id(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|word| {
            word.len() == 12
                && word
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
        .map(str::to_owned)
}

/// An MCP tool call over streamable HTTP: `initialize`, `notifications/initialized`, then
/// `tools/call`; the status and the JSON answer of the last one.
fn mcp_call(server: &Value, tool: &str, arguments: &Value) -> Result<(u16, Value), String> {
    let url = server["url"].as_str().ok_or("the MCP server has no url")?;
    let mut headers: Vec<(String, String)> = server["headers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|h| {
            (
                h["name"].as_str().unwrap_or_default().to_owned(),
                h["value"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let (_, _, response_headers) = post(
        url,
        &headers,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "flottille-fake-agent", "version": env!("CARGO_PKG_VERSION")}}}),
    )?;
    if let Some(session) = response_headers.get("mcp-session-id") {
        headers.push(("Mcp-Session-Id".into(), session.clone()));
    }
    post(
        url,
        &headers,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )?;
    let (status, body, _) = post(
        url,
        &headers,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}}),
    )?;
    Ok((status, body))
}

/// One HTTP/1.1 POST of a JSON body: the status, the JSON answer (from a JSON body or the last
/// `data:` line of an event stream) and the headers, by lowercase name.
fn post(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
) -> Result<(u16, Value, HashMap<String, String>), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("only http:// MCP servers are supported")?;
    let (host, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let mut stream = TcpStream::connect(host).map_err(|e| format!("connect {host}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(600)))
        .map_err(|e| e.to_string())?;
    let body = body.to_string();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| e.to_string())?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("not an HTTP answer: {status_line}"))?;
    let mut response_headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            response_headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let mut raw = Vec::new();
    reader.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    if response_headers
        .get("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        raw = dechunk(&raw);
    }
    let text = String::from_utf8_lossy(&raw);
    let json_text = if response_headers
        .get("content-type")
        .is_some_and(|v| v.starts_with("text/event-stream"))
    {
        text.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .next_back()
            .unwrap_or_default()
            .trim()
            .to_owned()
    } else {
        text.trim().to_owned()
    };
    let answer = if json_text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&json_text).unwrap_or(Value::String(json_text))
    };
    Ok((status, answer, response_headers))
}

fn dechunk(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(end) = rest.windows(2).position(|w| w == b"\r\n") {
        let size =
            usize::from_str_radix(String::from_utf8_lossy(&rest[..end]).trim(), 16).unwrap_or(0);
        rest = &rest[end + 2..];
        if size == 0 || rest.len() < size {
            break;
        }
        out.extend_from_slice(&rest[..size]);
        rest = rest.get(size + 2..).unwrap_or_default();
    }
    out
}
