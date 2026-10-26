//! Flottille's Codex bridge, `flottille bridge codex` (`docs/agents.md`): an ACP agent on stdio,
//! which the daemon starts as it starts any agent, in front of the user's own `codex` running its
//! app server on stdio. It starts Codex in bare mode (`codex.rs`), refuses a session whose
//! options are not the daemon's bare ones, and refuses every request of Codex's own: the agent's
//! tools are Flottille's, which its gate judges.
//!
//! Codex has no allowlist of tools: should a version of it start a tool of its own all the same,
//! the bridge interrupts the turn and fails the prompt, which fails the agent.
//!
//! One bridge runs one session. Once the session is open, the end of its `codex` is the end of
//! the bridge, with the same exit code: the daemon sees the agent die.

use std::collections::HashMap;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{ExitCode, ExitStatus, Stdio};
use std::time::Duration;

use flottille_core::paths::Paths;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::providers::{self, MCP_SERVER};
use crate::{VERSION, claude, codex};

const NOT_FOUND: i64 = -32601;
const INVALID: i64 = -32602;
const INTERNAL: i64 = -32603;

/// The items of a bare turn: the prompt, the replies and thoughts, the calls of Flottille's
/// tools and a compaction. Any other is a tool of Codex's own.
const BARE_ITEMS: [&str; 5] = [
    "userMessage",
    "agentMessage",
    "reasoning",
    "mcpToolCall",
    "contextCompaction",
];

pub fn run(paths: &Paths) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let bridge = Bridge {
        paths: paths.clone(),
        out: tokio::io::stdout(),
        codex: None,
        from_codex: None,
        waiting: HashMap::new(),
        next: 0,
        turn: None,
    };
    let code = runtime.block_on(bridge.serve())?;
    // The standard input is read on a thread of the runtime that a dead `codex` leaves blocked.
    runtime.shutdown_background();
    Ok(ExitCode::from(code))
}

struct Bridge {
    paths: Paths,
    out: tokio::io::Stdout,
    codex: Option<Codex>,
    from_codex: Option<mpsc::UnboundedReceiver<Value>>,
    /// What waits for the answer to one of the bridge's requests to `codex`, by its id.
    waiting: HashMap<u64, Waiting>,
    next: u64,
    turn: Option<Turn>,
}

/// The user's `codex`, running one thread.
struct Codex {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Its thread, the ACP session, once started or resumed.
    thread: String,
    /// The models `model/list` gave, but hidden ones.
    models: Vec<Value>,
    model: String,
    effort: String,
    open: bool,
    /// The thread's tokens so far, which a turn's usage counts from.
    total: Value,
}

/// The prompt being played.
struct Turn {
    id: Value,
    /// Codex's id of the turn, once `turn/start` answered.
    turn: Option<String>,
    cancelled: bool,
    /// Why the turn is no bare one: a tool of Codex's own started.
    breach: Option<String>,
    /// The context in use and the model's context window, as Codex last said.
    used: Option<(u64, u64)>,
    total: Value,
}

/// A session being opened, from one answer of `codex` to the next request.
struct Opening {
    id: Value,
    /// The thread to resume; `None` for a new one.
    resume: Option<String>,
    params: Value,
}

enum Waiting {
    Initialize(Opening),
    Account(Opening),
    Models(Opening),
    Thread(Opening),
    Turn,
    Nothing,
}

type Failure = (i64, String);

impl Bridge {
    async fn serve(mut self) -> std::io::Result<u8> {
        let mut input = BufReader::new(tokio::io::stdin()).lines();
        let mut terminate = signal(SignalKind::terminate())?;
        loop {
            tokio::select! {
                line = input.next_line() => match line {
                    Ok(Some(line)) => {
                        if let Ok(message) = serde_json::from_str::<Value>(&line) {
                            self.client_said(message).await;
                        }
                    }
                    _ => break,
                },
                message = next(&mut self.from_codex) => match message {
                    Some(message) => self.codex_said(message).await,
                    None => {
                        if let Some(code) = self.codex_ended().await {
                            return Ok(code);
                        }
                    }
                },
                _ = terminate.recv() => break,
            }
        }
        if let Some(mut codex) = self.codex.take() {
            // Its input closed, `codex` ends by itself; one that does not is killed.
            drop(codex.stdin.take());
            if tokio::time::timeout(Duration::from_secs(2), codex.child.wait())
                .await
                .is_err()
            {
                let _ = codex.child.kill().await;
            }
        }
        Ok(0)
    }

    async fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        // A client gone is the end of the input, which the loop sees.
        let _ = self.out.write_all(line.as_bytes()).await;
        let _ = self.out.flush().await;
    }

    async fn answer(&mut self, id: Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await;
    }

    async fn fail(&mut self, id: Value, (code, message): Failure) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
            .await;
    }

    async fn update(&mut self, update: Value) {
        let Some(codex) = &self.codex else { return };
        let session = codex.thread.clone();
        self.send(json!({"jsonrpc": "2.0", "method": "session/update",
            "params": {"sessionId": session, "update": update}}))
            .await;
    }

    async fn write_codex(&mut self, message: Value) {
        let Some(stdin) = self.codex.as_mut().and_then(|c| c.stdin.as_mut()) else {
            return;
        };
        let mut line = message.to_string();
        line.push('\n');
        // A `codex` gone shows as the end of its output.
        let _ = stdin.write_all(line.as_bytes()).await;
        let _ = stdin.flush().await;
    }

    async fn request(&mut self, method: &str, params: Value, waiting: Waiting) {
        self.next += 1;
        self.waiting.insert(self.next, waiting);
        self.write_codex(json!({"id": self.next, "method": method, "params": params}))
            .await;
    }

    async fn client_said(&mut self, message: Value) {
        let id = message.get("id").cloned().filter(|id| !id.is_null());
        let Some(method) = message["method"].as_str() else {
            return;
        };
        let params = &message["params"];
        let Some(id) = id else {
            if method == "session/cancel" {
                self.cancel().await;
            }
            return;
        };
        let answered = match method {
            "initialize" => Ok(Some(initialize())),
            "session/new" | "session/resume" => self.open(&id, method, params).await,
            "session/set_config_option" => self.set_option(params),
            "session/prompt" => self.prompt(&id, params).await,
            _ => Err((
                NOT_FOUND,
                format!("the Codex bridge does not know {method}"),
            )),
        };
        match answered {
            Ok(Some(result)) => self.answer(id, result).await,
            Ok(None) => {}
            Err(failure) => self.fail(id, failure).await,
        }
    }

    /// Starts `codex` on the session: a new thread, or a resumed one.
    async fn open(
        &mut self,
        id: &Value,
        method: &str,
        params: &Value,
    ) -> Result<Option<Value>, Failure> {
        let invalid = |message: String| (INVALID, message);
        if self.codex.is_some() {
            return Err(invalid("this bridge runs a session already".into()));
        }
        let (instructions, tools) = bare_options(&params["_meta"]).map_err(invalid)?;
        let server = mcp_server(&params["mcpServers"], &tools).map_err(invalid)?;
        let cwd = params["cwd"]
            .as_str()
            .filter(|cwd| PathBuf::from(cwd).is_absolute())
            .ok_or_else(|| invalid("the session needs an absolute cwd".into()))?;
        let resume = if method == "session/resume" {
            let thread = params["sessionId"]
                .as_str()
                .filter(|id| claude::is_session_id(id))
                .ok_or_else(|| invalid("not a session id of the Codex bridge".into()))?;
            Some(thread.to_owned())
        } else {
            None
        };
        let program = providers::find_program("codex").ok_or_else(|| {
            (
                INTERNAL,
                "`codex` is not on PATH: install Codex and sign in with it first".into(),
            )
        })?;
        let mut child = Command::new(&program)
            .args(codex::arguments())
            .envs(codex::environment(&self.paths))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| (INTERNAL, format!("cannot start {}: {e}", program.display())))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (sender, receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            let mut line = Vec::new();
            // Bytes, not text: a line that is not UTF-8 is noise to skip, not the end.
            while let Ok(read) = stdout.read_until(b'\n', &mut line).await {
                if read == 0 {
                    break;
                }
                if let Ok(message) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&line))
                    && sender.send(message).is_err()
                {
                    break;
                }
                line.clear();
            }
        });
        self.codex = Some(Codex {
            child,
            stdin: Some(stdin),
            thread: String::new(),
            models: Vec::new(),
            model: String::new(),
            effort: String::new(),
            open: false,
            total: Value::Null,
        });
        self.from_codex = Some(receiver);
        let opening = Opening {
            id: id.clone(),
            resume,
            params: codex::thread(cwd, &instructions, server),
        };
        self.request(
            "initialize",
            json!({"clientInfo": {"name": "flottille", "title": "Flottille", "version": VERSION},
                "capabilities": {"experimentalApi": true}}),
            Waiting::Initialize(opening),
        )
        .await;
        Ok(None)
    }

    fn set_option(&mut self, params: &Value) -> Result<Option<Value>, Failure> {
        let Some(codex) = self.codex.as_mut().filter(|c| c.open) else {
            return Err((INVALID, "no session is open".into()));
        };
        let option = params["configId"].as_str().unwrap_or_default();
        let value = params["value"].as_str().unwrap_or_default().to_owned();
        let offered = match option {
            "model" => model_values(&codex.models),
            "effort" => efforts(&codex.models, &codex.model),
            _ => return Err((INVALID, format!("no option {option}"))),
        };
        if !offered.contains(&value) {
            return Err((
                INVALID,
                format!("{value} is not a {option} offered: {}", offered.join(", ")),
            ));
        }
        // Both are given with the next turn, which keeps them.
        if option == "model" {
            codex.model = value;
            if !efforts(&codex.models, &codex.model).contains(&codex.effort) {
                codex.effort = default_effort(&codex.models, &codex.model);
            }
        } else {
            codex.effort = value;
        }
        Ok(Some(json!({"configOptions": config_options(codex)})))
    }

    async fn prompt(&mut self, id: &Value, params: &Value) -> Result<Option<Value>, Failure> {
        let Some(codex) = self.codex.as_ref().filter(|c| c.open) else {
            return Err((INVALID, "no session is open".into()));
        };
        if self.turn.is_some() {
            return Err((INVALID, "a prompt is playing already".into()));
        }
        let mut texts = Vec::new();
        for block in params["prompt"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => texts.push(block["text"].as_str().unwrap_or_default()),
                other => {
                    return Err((
                        INVALID,
                        format!("the Codex bridge takes no {} block", other.unwrap_or("?")),
                    ));
                }
            }
        }
        let turn = json!({
            "threadId": codex.thread,
            "input": [{"type": "text", "text": texts.join("\n\n"), "text_elements": []}],
            "model": codex.model,
            "effort": codex.effort,
            // Every turn again: no environment, so no command, file or image tool.
            "environments": [],
        });
        self.turn = Some(Turn {
            id: id.clone(),
            turn: None,
            cancelled: false,
            breach: None,
            used: None,
            total: codex.total.clone(),
        });
        self.request("turn/start", turn, Waiting::Turn).await;
        Ok(None)
    }

    async fn cancel(&mut self) {
        let Some(turn) = self.turn.as_mut().filter(|t| !t.cancelled) else {
            return;
        };
        turn.cancelled = true;
        // A turn Codex has not named yet is interrupted once it has.
        if let Some(started) = turn.turn.clone() {
            self.interrupt(started).await;
        }
    }

    async fn interrupt(&mut self, turn: String) {
        let Some(codex) = &self.codex else { return };
        let thread = codex.thread.clone();
        self.request(
            "turn/interrupt",
            json!({"threadId": thread, "turnId": turn}),
            Waiting::Nothing,
        )
        .await;
    }

    /// `codex`'s output ended: its process is gone. The bridge's exit code once the session is
    /// open; before, the open fails and the bridge waits for the next.
    async fn codex_ended(&mut self) -> Option<u8> {
        self.from_codex = None;
        let mut codex = self.codex.take()?;
        let status = codex.child.wait().await.ok();
        if codex.open {
            return Some(status.map_or(1, exit_code));
        }
        let ended = match status {
            Some(status) => format!("codex ended as the session opened ({status})"),
            None => "codex ended as the session opened".to_owned(),
        };
        let opening: Vec<Value> = self
            .waiting
            .drain()
            .filter_map(|(_, waiting)| match waiting {
                Waiting::Initialize(o)
                | Waiting::Account(o)
                | Waiting::Models(o)
                | Waiting::Thread(o) => Some(o.id),
                _ => None,
            })
            .collect();
        for id in opening {
            self.fail(id, (INTERNAL, ended.clone())).await;
        }
        None
    }

    async fn codex_said(&mut self, message: Value) {
        let Some(method) = message["method"].as_str() else {
            if let Some(id) = message["id"].as_u64() {
                self.answered(id, &message).await;
            }
            return;
        };
        if !message["id"].is_null() {
            // Codex asks for an approval, an answer of the user's, or a tool of the client's:
            // bare mode has none to give.
            return self
                .write_codex(json!({"id": message["id"], "error": {"code": NOT_FOUND,
                    "message": "Flottille takes no request of Codex's: its agent has \
                        Flottille's tools only"}}))
                .await;
        }
        let params = &message["params"];
        match method {
            "item/agentMessage/delta" => self.chunk("agent_message_chunk", params).await,
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                self.chunk("agent_thought_chunk", params).await;
            }
            "item/started" => self.item_started(&params["item"]).await,
            "item/completed" => self.item_completed(&params["item"]).await,
            "thread/tokenUsage/updated" => {
                let usage = &params["tokenUsage"];
                if let Some(turn) = &mut self.turn {
                    turn.used = usage["last"]["totalTokens"]
                        .as_u64()
                        .zip(usage["modelContextWindow"].as_u64());
                }
                if let Some(codex) = &mut self.codex {
                    codex.total = usage["total"].clone();
                }
            }
            "error" if params["willRetry"] == true => {
                let text = format!(
                    "the provider is retrying: {}",
                    params["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown error")
                );
                self.update(
                    json!({"sessionUpdate": "session_info_update", "_meta": {"jetbrains":
                    {"air": {"sessionFailure": {"severity": "warning", "message": text}}}}}),
                )
                .await;
            }
            "turn/completed" => self.turn_completed(&params["turn"]).await,
            _ => {}
        }
    }

    async fn chunk(&mut self, kind: &str, params: &Value) {
        let Some(text) = params["delta"].as_str() else {
            return;
        };
        let text = text.to_owned();
        self.update(json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}))
            .await;
    }

    async fn item_started(&mut self, item: &Value) {
        let kind = item["type"].as_str().unwrap_or("?");
        let bare =
            BARE_ITEMS.contains(&kind) && (kind != "mcpToolCall" || item["server"] == MCP_SERVER);
        if !bare {
            return self.breached(item).await;
        }
        if kind == "mcpToolCall" {
            let tool = format!(
                "{}{}",
                codex::PROVIDER.tool_prefix,
                item["tool"].as_str().unwrap_or_default()
            );
            self.update(
                json!({"sessionUpdate": "tool_call", "toolCallId": item["id"],
                "title": tool, "kind": "other", "status": "in_progress",
                "rawInput": item["arguments"]}),
            )
            .await;
        }
    }

    /// A tool of Codex's own started: the turn is interrupted, and its prompt fails.
    async fn breached(&mut self, item: &Value) {
        let Some(turn) = self.turn.as_mut().filter(|t| t.breach.is_none()) else {
            return;
        };
        let what = match item["type"].as_str() {
            Some("mcpToolCall") => format!(
                "a tool of the MCP server {}",
                item["server"].as_str().unwrap_or("?")
            ),
            Some(kind) => format!("its own {kind}"),
            None => "an item it did not name".to_owned(),
        };
        turn.breach = Some(format!(
            "Codex started {what}, which its bare mode shuts off: the turn was interrupted, and \
             this Codex cannot run Flottille's agents"
        ));
        if let Some(started) = turn.turn.clone() {
            self.interrupt(started).await;
        }
    }

    async fn item_completed(&mut self, item: &Value) {
        match item["type"].as_str() {
            Some("mcpToolCall") if item["server"] == MCP_SERVER => {
                let failed = item["status"] == "failed";
                let output = if failed {
                    item["error"].clone()
                } else {
                    item["result"]["content"].clone()
                };
                let status = if failed { "failed" } else { "completed" };
                self.update(json!({"sessionUpdate": "tool_call_update",
                    "toolCallId": item["id"], "status": status, "rawOutput": output}))
                    .await;
            }
            Some("contextCompaction") => {
                self.update(
                    json!({"sessionUpdate": "tool_call", "toolCallId": item["id"],
                    "title": "Compacting the conversation", "kind": "other", "status": "completed",
                    "_meta": {"contextCompaction": {"trigger": "auto"}}}),
                )
                .await;
            }
            _ => {}
        }
    }

    /// The end of a turn: the context in use, then the prompt's answer.
    async fn turn_completed(&mut self, ended: &Value) {
        if self.turn.as_ref().and_then(|t| t.turn.as_deref()) != ended["id"].as_str() {
            return;
        }
        let Some(turn) = self.turn.take() else { return };
        if let Some(breach) = turn.breach {
            return self.fail(turn.id, (INTERNAL, breach)).await;
        }
        if let Some((used, size)) = turn.used {
            self.update(json!({"sessionUpdate": "usage_update", "used": used, "size": size}))
                .await;
        }
        let stop_reason = match ended["status"].as_str() {
            Some("interrupted") => "cancelled",
            Some("failed") => {
                let error = &ended["error"];
                let text = error["message"]
                    .as_str()
                    .unwrap_or("the turn failed")
                    .to_owned();
                let info = &error["codexErrorInfo"];
                // Signed out: the agent cannot go on until the user signs in again.
                if info == "unauthorized" {
                    return self.fail(turn.id, (INTERNAL, text)).await;
                }
                let limit = info == "usageLimitExceeded" || info == "rateLimitExceeded";
                let category = if limit { "limit" } else { "service" };
                self.update(
                    json!({"sessionUpdate": "session_info_update", "_meta": {"jetbrains":
                    {"air": {"sessionFailure": {"severity": "error", "category": category,
                        "message": text}}}}}),
                )
                .await;
                "end_turn"
            }
            _ => "end_turn",
        };
        let now = self.codex.as_ref().map_or(Value::Null, |c| c.total.clone());
        let count = |key: &str| {
            now[key]
                .as_u64()
                .unwrap_or(0)
                .saturating_sub(turn.total[key].as_u64().unwrap_or(0))
        };
        let (cached, input, output) = (
            count("cachedInputTokens"),
            count("inputTokens"),
            count("outputTokens"),
        );
        self.answer(
            turn.id,
            json!({"stopReason": stop_reason, "usage": {"inputTokens": input.saturating_sub(cached),
                "outputTokens": output, "cachedReadTokens": cached,
                "totalTokens": input + output}}),
        )
        .await;
    }

    /// An answer of `codex` to one of the bridge's requests.
    async fn answered(&mut self, id: u64, message: &Value) {
        let Some(waiting) = self.waiting.remove(&id) else {
            return;
        };
        let result = &message["result"];
        let error = message["error"]["message"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| (!message["error"].is_null()).then(|| "codex refused".to_owned()));
        if let Some(error) = error {
            return match waiting {
                Waiting::Initialize(o)
                | Waiting::Account(o)
                | Waiting::Models(o)
                | Waiting::Thread(o) => self.refuse_open(o.id, error).await,
                Waiting::Turn => {
                    if let Some(turn) = self.turn.take() {
                        self.fail(turn.id, (INTERNAL, error)).await;
                    }
                }
                Waiting::Nothing => {}
            };
        }
        match waiting {
            Waiting::Initialize(opening) => {
                self.write_codex(json!({"method": "initialized"})).await;
                self.request("account/read", json!({}), Waiting::Account(opening))
                    .await;
            }
            Waiting::Account(opening) => {
                let account = &result["account"];
                if account.is_null() && result["requiresOpenaiAuth"] == true {
                    return self
                        .refuse_open(
                            opening.id,
                            format!("Codex is not signed in: {}", codex::PROVIDER.sign_in),
                        )
                        .await;
                }
                if let Some(why) = codex::refused_account(account) {
                    return self.refuse_open(opening.id, why).await;
                }
                self.request("model/list", json!({}), Waiting::Models(opening))
                    .await;
            }
            Waiting::Models(opening) => {
                let models: Vec<Value> = result["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|m| m["hidden"] != true && m["model"].is_string())
                    .cloned()
                    .collect();
                if let Some(codex) = &mut self.codex {
                    codex.models = models;
                }
                let (method, params) = match &opening.resume {
                    Some(thread) => {
                        let mut params = opening.params.clone();
                        params["threadId"] = thread.clone().into();
                        // A resumed thread takes its environments from each turn.
                        params.as_object_mut().map(|p| p.remove("environments"));
                        ("thread/resume", params)
                    }
                    None => ("thread/start", opening.params.clone()),
                };
                self.request(method, params, Waiting::Thread(opening)).await;
            }
            Waiting::Thread(opening) => {
                let Some(codex) = &mut self.codex else { return };
                codex.thread = result["thread"]["id"].as_str().unwrap_or_default().into();
                let model = result["model"].as_str().unwrap_or_default();
                codex.model = if model_values(&codex.models).iter().any(|m| m == model) {
                    model.to_owned()
                } else {
                    codex
                        .models
                        .iter()
                        .find(|m| m["isDefault"] == true)
                        .or(codex.models.first())
                        .and_then(|m| m["model"].as_str())
                        .unwrap_or(model)
                        .to_owned()
                };
                codex.effort = match result["reasoningEffort"].as_str() {
                    Some(effort)
                        if efforts(&codex.models, &codex.model)
                            .iter()
                            .any(|e| e == effort) =>
                    {
                        effort.to_owned()
                    }
                    _ => default_effort(&codex.models, &codex.model),
                };
                codex.open = true;
                let mut answer = json!({"configOptions": config_options(codex)});
                if opening.resume.is_none() {
                    answer["sessionId"] = codex.thread.clone().into();
                }
                self.answer(opening.id, answer).await;
            }
            Waiting::Turn => {
                let Some(turn) = &mut self.turn else { return };
                let started = result["turn"]["id"].as_str().unwrap_or_default().to_owned();
                turn.turn = Some(started.clone());
                if turn.cancelled || turn.breach.is_some() {
                    self.interrupt(started).await;
                }
            }
            Waiting::Nothing => {}
        }
    }

    /// The session does not open: `codex` goes, and the bridge waits for the next.
    async fn refuse_open(&mut self, id: Value, why: String) {
        self.from_codex = None;
        self.waiting.clear();
        if let Some(mut codex) = self.codex.take() {
            let _ = codex.child.kill().await;
        }
        self.fail(id, (INTERNAL, why)).await;
    }
}

fn initialize() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": false,
            "promptCapabilities": {"image": false, "audio": false, "embeddedContext": false},
            "mcpCapabilities": {"http": true, "sse": false},
            "sessionCapabilities": {"resume": {}},
        },
        "agentInfo": {"name": "flottille-codex-bridge", "version": VERSION},
        "authMethods": [],
    })
}

async fn next(receiver: &mut Option<mpsc::UnboundedReceiver<Value>>) -> Option<Value> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// The agent's instructions and tools, from `_meta.codex` as the daemon sets it; anything else
/// is refused.
fn bare_options(meta: &Value) -> Result<(String, Vec<String>), String> {
    let options = meta["codex"]
        .as_object()
        .ok_or("the session has no Codex options: bare mode needs them")?;
    if let Some(key) = options
        .keys()
        .find(|key| !["instructions", "tools"].contains(&key.as_str()))
    {
        return Err(format!("the Codex bridge does not take the option {key}"));
    }
    let instructions = options
        .get("instructions")
        .and_then(Value::as_str)
        .ok_or("bare mode needs Flottille's own instructions")?;
    let tools = options
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| {
            tools
                .iter()
                .map(|t| {
                    t.as_str()
                        .filter(|t| t.starts_with(codex::PROVIDER.tool_prefix))
                        .map(str::to_owned)
                })
                .collect::<Option<Vec<_>>>()
        })
        .ok_or("`tools` must be a list of Flottille's tools, as Codex names them")?;
    Ok((instructions.to_owned(), tools))
}

/// Flottille's MCP server, the only one bare mode takes, as the thread's configuration names it.
fn mcp_server(servers: &Value, tools: &[String]) -> Result<Value, String> {
    let [server] = servers.as_array().map(Vec::as_slice).unwrap_or_default() else {
        return Err("not bare: a session needs Flottille's MCP server, and no other".into());
    };
    let url = server["url"].as_str();
    if server["name"] != MCP_SERVER || server["type"] != "http" || url.is_none() {
        return Err("not bare: a session needs Flottille's MCP server, and no other".into());
    }
    let headers: serde_json::Map<String, Value> = server["headers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| Some((h["name"].as_str()?.to_owned(), h["value"].clone())))
        .collect();
    Ok(codex::server(
        url.unwrap_or_default(),
        &Value::Object(headers),
        tools,
    ))
}

/// The session's options: the models Codex listed, and the efforts of the one set.
fn config_options(codex: &Codex) -> Value {
    let models: Vec<Value> = codex
        .models
        .iter()
        .filter_map(|model| {
            let value = model["model"].as_str()?;
            let name = model["displayName"].as_str().unwrap_or(value);
            Some(json!({"value": value, "name": name}))
        })
        .collect();
    let levels: Vec<Value> = efforts(&codex.models, &codex.model)
        .iter()
        .map(|level| json!({"value": level, "name": level}))
        .collect();
    let mut options = vec![
        json!({"id": "model", "name": "Model", "category": "model", "type": "select",
        "currentValue": codex.model, "options": models}),
    ];
    if !levels.is_empty() {
        options.push(
            json!({"id": "effort", "name": "Effort", "category": "thought_level",
            "type": "select", "currentValue": codex.effort, "options": levels}),
        );
    }
    Value::Array(options)
}

fn model_values(models: &[Value]) -> Vec<String> {
    models
        .iter()
        .filter_map(|m| m["model"].as_str().map(str::to_owned))
        .collect()
}

/// The reasoning efforts a model offers.
fn efforts(models: &[Value], model: &str) -> Vec<String> {
    models
        .iter()
        .find(|m| m["model"] == model)
        .and_then(|m| m["supportedReasoningEfforts"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|e| e["reasoningEffort"].as_str().map(str::to_owned))
        .collect()
}

fn default_effort(models: &[Value], model: &str) -> String {
    models
        .iter()
        .find(|m| m["model"] == model)
        .and_then(|m| m["defaultReasoningEffort"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// A process's end as an exit code: its own, or 128 and the signal that ended it.
fn exit_code(status: ExitStatus) -> u8 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code as u8,
        (_, Some(signal)) => 128u8.saturating_add(signal as u8),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_flottille_s_http_server_and_its_tools_are_taken() {
        let ours = json!([{"type": "http", "name": "flottille", "url": "http://127.0.0.1:1/mcp",
            "headers": [{"name": "Authorization", "value": "Bearer t"}]}]);
        let tools = ["mcp__flottille__report".to_owned()];
        let server = mcp_server(&ours, &tools).unwrap();
        assert_eq!(server["flottille"]["enabled_tools"], json!(["report"]));
        assert_eq!(
            server["flottille"]["http_headers"],
            json!({"Authorization": "Bearer t"})
        );
        for servers in [
            json!([]),
            json!([{"type": "http", "name": "other", "url": "u"}]),
            json!([{"type": "stdio", "name": "flottille", "command": "sh"}]),
            json!([ours[0], {"type": "http", "name": "more", "url": "u"}]),
        ] {
            assert!(mcp_server(&servers, &tools).is_err(), "{servers}");
        }
    }

    #[test]
    fn the_options_are_the_instructions_and_flottille_s_tools_only() {
        let meta = json!({"codex": {"instructions": "Be brief.",
            "tools": ["mcp__flottille__report"]}});
        assert_eq!(
            bare_options(&meta).unwrap(),
            (
                "Be brief.".to_owned(),
                vec!["mcp__flottille__report".to_owned()]
            )
        );
        for meta in [
            json!({}),
            json!({"codex": {"tools": []}}),
            json!({"codex": {"instructions": "x", "tools": ["shell"]}}),
            json!({"codex": {"instructions": "x", "tools": [], "sandbox": "danger-full-access"}}),
        ] {
            assert!(bare_options(&meta).is_err(), "{meta}");
        }
    }
}
