//! Flottille's Claude bridge, `flottille bridge claude` (`docs/agents.md`): an ACP agent on stdio,
//! which the daemon starts as it starts any agent, in front of the user's own `claude` driven in
//! its stream-json mode. It translates and decides nothing: the bare mode comes from the daemon's
//! options, which it refuses when they are not bare, and Claude's own permission prompts go to the
//! daemon as ACP permission requests.
//!
//! One bridge runs one session. Once the session is open, the end of its `claude` is the end of
//! the bridge, with the same exit code: the daemon sees the agent die.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, ExitStatus, Stdio};
use std::time::Duration;

use flottille_core::paths::{Paths, ensure_private_dir};
use flottille_core::secrets;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::VERSION;
use crate::claude;
use crate::providers;

/// The options of `_meta.claudeCode.options` the bridge takes; any other is refused.
const OPTIONS: [&str; 6] = [
    "tools",
    "allowedTools",
    "settingSources",
    "strictMcpConfig",
    "systemPrompt",
    "env",
];

/// A history past this size keeps its later half: a load replays what fits.
const HISTORY_CAP: u64 = 4 << 20;

const NOT_FOUND: i64 = -32601;
const INVALID: i64 = -32602;
const INTERNAL: i64 = -32603;

/// What Claude is told when one of its own permission prompts is refused.
const REFUSED: &str = "Refused: this agent can use Flottille's tools only.";

/// How Claude Code's own messages for a spent usage start, as the pinned version's Agent SDK
/// lists them (`USAGE_LIMIT_ERROR_PREFIXES`).
const USAGE_LIMIT: [&str; 12] = [
    "You've hit your",
    "You've reached your",
    "You're out of usage credits",
    "Your org is out of usage · add funds to continue",
    "Your org is out of usage · contact your admin",
    "Your seat type doesn't include usage credits",
    "Your seat type doesn't include usage",
    "Your usage allocation has been disabled by your admin",
    "Your group's usage limit is set to $0",
    "Fable 5 requires usage credits",
    "You're out of extra usage",
    "Your seat type doesn't include extra usage",
];

pub fn run(paths: &Paths) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let bridge = Bridge {
        folder: claude::bridge_files(paths),
        paths: paths.clone(),
        env: std::env::vars_os().collect(),
        offers_resume: std::env::var_os("FLOTTILLE_BRIDGE_NO_RESUME").is_none(),
        out: tokio::io::stdout(),
        claude: None,
        from_claude: None,
        waiting: HashMap::new(),
        asked: HashMap::new(),
        next: 0,
        turn: None,
    };
    let code = runtime.block_on(bridge.serve())?;
    // The standard input is read on a thread of the runtime that a dead `claude` leaves blocked.
    runtime.shutdown_background();
    Ok(ExitCode::from(code))
}

struct Bridge {
    /// Where the MCP configuration with the session's token is written, out of the command line.
    folder: PathBuf,
    paths: Paths,
    /// Its environment, whose secrets are masked out of the history.
    env: BTreeMap<OsString, OsString>,
    offers_resume: bool,
    out: tokio::io::Stdout,
    claude: Option<Claude>,
    from_claude: Option<mpsc::UnboundedReceiver<Value>>,
    /// What waits for the answer to one of the bridge's control requests, by its id.
    waiting: HashMap<String, Waiting>,
    /// The permission prompts of `claude` asked of the client, by the ACP request's id: the
    /// control request's id and the tool's input.
    asked: HashMap<String, (String, Value)>,
    next: u64,
    turn: Option<Turn>,
}

/// The user's `claude`, running one session.
struct Claude {
    child: Child,
    stdin: Option<ChildStdin>,
    session: String,
    /// The MCP configuration file, removed with the process.
    config: PathBuf,
    history: PathBuf,
    /// The models its answer to `initialize` listed, each with its effort levels.
    models: Vec<Value>,
    model: String,
    /// The effort level set, `default` for Claude Code's own.
    effort: String,
    open: bool,
    /// The model's context window, once a result named it.
    window: Option<u64>,
}

impl Drop for Claude {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.config);
    }
}

/// The prompt being played.
struct Turn {
    id: Value,
    cancelled: bool,
    /// Whether the text of the message under way came as stream events, which its whole message
    /// then repeats.
    streamed: bool,
    /// The id of the API message being streamed, as its `message_start` named it.
    message: Option<String>,
    /// The context in use, as the last message or compaction said.
    used: Option<u64>,
    /// The provider refused the turn for a limit: a rate limit rejected, an error or a message
    /// of Claude Code's saying so.
    limited: bool,
    /// The kind of error the last message of the turn carried (`authentication_failed`…).
    error: Option<String>,
}

enum Waiting {
    /// The session opens once `claude` took its `initialize`.
    Open {
        id: Value,
        replay: bool,
        new: bool,
    },
    Model {
        id: Value,
        model: String,
    },
    Effort {
        id: Value,
        effort: String,
    },
    /// A model change, answered once the level it does not have went back to Claude Code's own.
    Reset {
        id: Value,
        options: Value,
    },
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
                message = next(&mut self.from_claude) => match message {
                    Some(message) => self.claude_said(message).await,
                    None => {
                        if let Some(code) = self.claude_ended().await {
                            return Ok(code);
                        }
                    }
                },
                _ = terminate.recv() => break,
            }
        }
        if let Some(mut claude) = self.claude.take() {
            // Its input closed, `claude` ends by itself; one that does not is killed.
            drop(claude.stdin.take());
            if tokio::time::timeout(Duration::from_secs(2), claude.child.wait())
                .await
                .is_err()
            {
                let _ = claude.child.kill().await;
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
        let Some(claude) = &self.claude else { return };
        let session = claude.session.clone();
        self.send(json!({"jsonrpc": "2.0", "method": "session/update",
            "params": {"sessionId": session, "update": update}}))
            .await;
    }

    async fn write_claude(&mut self, message: Value) {
        let Some(stdin) = self.claude.as_mut().and_then(|c| c.stdin.as_mut()) else {
            return;
        };
        let mut line = message.to_string();
        line.push('\n');
        // A `claude` gone shows as the end of its output.
        let _ = stdin.write_all(line.as_bytes()).await;
        let _ = stdin.flush().await;
    }

    async fn control(&mut self, request: Value, waiting: Waiting) {
        self.next += 1;
        let id = format!("flottille-{}", self.next);
        self.waiting.insert(id.clone(), waiting);
        self.write_claude(json!({"type": "control_request", "request_id": id, "request": request}))
            .await;
    }

    async fn client_said(&mut self, message: Value) {
        let id = message.get("id").cloned().filter(|id| !id.is_null());
        let Some(method) = message["method"].as_str() else {
            if let Some(id) = id {
                self.permission_answered(&id, &message["result"]).await;
            }
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
            "initialize" => Ok(Some(self.initialize())),
            "session/new" | "session/resume" | "session/load" => {
                self.open(&id, method, params).await
            }
            "session/set_config_option" => self.set_option(&id, params).await,
            "session/prompt" => self.prompt(&id, params).await,
            _ => Err((
                NOT_FOUND,
                format!("the Claude bridge does not know {method}"),
            )),
        };
        match answered {
            Ok(Some(result)) => self.answer(id, result).await,
            Ok(None) => {}
            Err(failure) => self.fail(id, failure).await,
        }
    }

    fn initialize(&self) -> Value {
        let mut sessions = json!({});
        if self.offers_resume {
            sessions["resume"] = json!({});
        }
        json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {"image": false, "audio": false, "embeddedContext": false},
                "mcpCapabilities": {"http": true, "sse": false},
                "sessionCapabilities": sessions,
            },
            "agentInfo": {"name": "flottille-claude-bridge", "version": VERSION},
            "authMethods": [],
        })
    }

    /// Starts `claude` on the session: new, resumed, or loaded (resumed, its history replayed).
    async fn open(
        &mut self,
        id: &Value,
        method: &str,
        params: &Value,
    ) -> Result<Option<Value>, Failure> {
        let invalid = |message: String| (INVALID, message);
        if self.claude.is_some() {
            return Err(invalid("this bridge runs a session already".into()));
        }
        let options = bare_options(&params["_meta"]["claudeCode"]["options"]).map_err(invalid)?;
        let servers = mcp_servers(&params["mcpServers"]).map_err(invalid)?;
        let cwd = params["cwd"]
            .as_str()
            .map(PathBuf::from)
            .filter(|cwd| cwd.is_absolute())
            .ok_or_else(|| invalid("the session needs an absolute cwd".into()))?;
        let new = method == "session/new";
        let session = if new {
            new_session_id().map_err(|e| (INTERNAL, e.to_string()))?
        } else {
            params["sessionId"]
                .as_str()
                .filter(|id| claude::is_session_id(id))
                .ok_or_else(|| invalid("not a session id of the Claude bridge".into()))?
                .to_owned()
        };
        let program = providers::find_program("claude").ok_or_else(|| {
            (
                INTERNAL,
                "`claude` is not on PATH: install Claude Code and sign in with it first".into(),
            )
        })?;
        let internal = |e: &dyn std::fmt::Display| (INTERNAL, e.to_string());
        ensure_private_dir(&self.folder).map_err(|e| internal(&e))?;
        ensure_private_dir(&claude::histories(&self.paths)).map_err(|e| internal(&e))?;
        let config = self.folder.join(format!("{}.mcp.json", std::process::id()));
        write_private(&config, &servers.to_string()).map_err(|e| internal(&e))?;
        let mut child = Command::new(&program)
            .args(arguments(&options, &config, new, &session))
            .envs(options.env.iter().map(|(k, v)| (k, v)))
            .env("CLAUDE_CODE_ENTRYPOINT", "sdk-rs")
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                let _ = std::fs::remove_file(&config);
                (INTERNAL, format!("cannot start {}: {e}", program.display()))
            })?;
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
                // stdout is the protocol; anything else there is noise.
                if let Ok(message) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&line))
                    && sender.send(message).is_err()
                {
                    break;
                }
                line.clear();
            }
        });
        let history = claude::history(&self.paths, &session).expect("a session id names a file");
        self.claude = Some(Claude {
            child,
            stdin: Some(stdin),
            session: session.clone(),
            config,
            history,
            models: Vec::new(),
            model: "default".into(),
            effort: "default".into(),
            open: false,
            window: None,
        });
        self.from_claude = Some(receiver);
        self.control(
            json!({"subtype": "initialize", "systemPrompt": options.prompt,
                "systemPromptSnapshot": options.snapshot}),
            Waiting::Open {
                id: id.clone(),
                replay: method == "session/load",
                new,
            },
        )
        .await;
        Ok(None)
    }

    async fn set_option(&mut self, id: &Value, params: &Value) -> Result<Option<Value>, Failure> {
        let Some(claude) = self.claude.as_ref().filter(|c| c.open) else {
            return Err((INVALID, "no session is open".into()));
        };
        let option = params["configId"].as_str().unwrap_or_default();
        let value = params["value"].as_str().unwrap_or_default().to_owned();
        let offered = match option {
            "model" => model_values(&claude.models),
            "effort" => effort_levels(&claude.models, &claude.model)
                .ok_or((INVALID, format!("the model {} has no effort", claude.model)))?,
            _ => return Err((INVALID, format!("no option {option}"))),
        };
        if !offered.contains(&value) {
            return Err((
                INVALID,
                format!("{value} is not a {option} offered: {}", offered.join(", ")),
            ));
        }
        let id = id.clone();
        let (request, waiting) = if option == "model" {
            // Without a model, `claude` goes back to its own choice.
            let mut request = json!({"subtype": "set_model"});
            if value != "default" {
                request["model"] = value.clone().into();
            }
            (request, Waiting::Model { id, model: value })
        } else {
            (
                effort_request(&value),
                Waiting::Effort { id, effort: value },
            )
        };
        self.control(request, waiting).await;
        Ok(None)
    }

    async fn prompt(&mut self, id: &Value, params: &Value) -> Result<Option<Value>, Failure> {
        let Some(claude) = self.claude.as_ref().filter(|c| c.open) else {
            return Err((INVALID, "no session is open".into()));
        };
        if self.turn.is_some() {
            return Err((INVALID, "a prompt is playing already".into()));
        }
        // Text only, as `initialize` said (`embeddedContext: false`).
        let mut texts = Vec::new();
        for block in params["prompt"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => texts.push(block["text"].as_str().unwrap_or_default()),
                other => {
                    return Err((
                        INVALID,
                        format!("the Claude bridge takes no {} block", other.unwrap_or("?")),
                    ));
                }
            }
        }
        let text = texts.join("\n\n");
        remember(&claude.history, "user", &text, None, &self.env);
        let session = claude.session.clone();
        self.turn = Some(Turn {
            id: id.clone(),
            cancelled: false,
            streamed: false,
            message: None,
            used: None,
            limited: false,
            error: None,
        });
        self.write_claude(
            json!({"type": "user", "session_id": session, "parent_tool_use_id": null,
            "message": {"role": "user", "content": [{"type": "text", "text": text}]}}),
        )
        .await;
        Ok(None)
    }

    async fn cancel(&mut self) {
        let Some(turn) = self.turn.as_mut().filter(|t| !t.cancelled) else {
            return;
        };
        turn.cancelled = true;
        self.control(json!({"subtype": "interrupt"}), Waiting::Nothing)
            .await;
    }

    async fn permission_answered(&mut self, id: &Value, result: &Value) {
        let key = id.as_str().map_or_else(|| id.to_string(), str::to_owned);
        let Some((request, input)) = self.asked.remove(&key) else {
            return;
        };
        let outcome = &result["outcome"];
        let allowed = outcome["outcome"] == "selected" && outcome["optionId"] == "allow";
        let decision = if allowed {
            json!({"behavior": "allow", "updatedInput": input})
        } else {
            json!({"behavior": "deny", "message": REFUSED})
        };
        self.write_claude(json!({"type": "control_response", "response": {
            "subtype": "success", "request_id": request, "response": decision}}))
            .await;
    }

    /// `claude`'s output ended: its process is gone. The bridge's exit code once the session is
    /// open; before, the open fails and the bridge waits for the next.
    async fn claude_ended(&mut self) -> Option<u8> {
        self.from_claude = None;
        let mut claude = self.claude.take()?;
        let status = claude.child.wait().await.ok();
        if claude.open {
            return Some(status.map_or(1, exit_code));
        }
        let ended = match status {
            Some(status) => format!("claude ended as the session opened ({status})"),
            None => "claude ended as the session opened".to_owned(),
        };
        let opening: Vec<Value> = self
            .waiting
            .drain()
            .filter_map(|(_, waiting)| match waiting {
                Waiting::Open { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        for id in opening {
            self.fail(id, (INTERNAL, ended.clone())).await;
        }
        None
    }

    async fn claude_said(&mut self, message: Value) {
        // A sub-agent's messages: there is none in bare mode, and they are not the session's.
        if !message["parent_tool_use_id"].is_null() {
            return;
        }
        match message["type"].as_str() {
            Some("control_response") => self.control_answered(&message["response"]).await,
            Some("control_request") => self.control_asked(&message).await,
            Some("stream_event") => self.streamed(&message["event"]).await,
            Some("assistant") => self.assistant(&message).await,
            Some("user") => self.tool_results(&message["message"]).await,
            Some("system") => self.system(&message).await,
            Some("result") => self.result(&message).await,
            Some("rate_limit_event") => {
                let info = &message["rate_limit_info"];
                // Past its own limit, an account with extra usage goes on on that.
                let overage = matches!(
                    info["overageStatus"].as_str(),
                    Some("allowed" | "allowed_warning")
                );
                if let Some(turn) = &mut self.turn
                    && info["status"] == "rejected"
                    && !overage
                {
                    turn.limited = true;
                }
            }
            _ => {}
        }
    }

    async fn control_answered(&mut self, response: &Value) {
        let id = response["request_id"].as_str().unwrap_or_default();
        let Some(waiting) = self.waiting.remove(id) else {
            return;
        };
        let refused = (response["subtype"] != "success").then(|| {
            response["error"]
                .as_str()
                .unwrap_or("claude refused the request")
                .to_owned()
        });
        match waiting {
            Waiting::Open { id, replay, new } => {
                if let Some(error) = refused {
                    self.claude = None;
                    self.from_claude = None;
                    return self.fail(id, (INTERNAL, error)).await;
                }
                if replay {
                    self.replay().await;
                }
                let Some(claude) = &mut self.claude else {
                    return;
                };
                claude.open = true;
                claude.models = response["response"]["models"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let mut answer = json!({"configOptions": config_options(claude)});
                if new {
                    answer["sessionId"] = claude.session.clone().into();
                }
                self.answer(id, answer).await;
            }
            Waiting::Model { id, model } => {
                if let Some(error) = refused {
                    return self.fail(id, (INVALID, error)).await;
                }
                let Some(claude) = &mut self.claude else {
                    return;
                };
                claude.model = model;
                // A level the new model does not have goes back to Claude Code's own.
                let kept = effort_levels(&claude.models, &claude.model)
                    .is_some_and(|levels| levels.contains(&claude.effort));
                let reset = !kept && claude.effort != "default";
                if !kept {
                    claude.effort = "default".into();
                }
                let options = config_options(claude);
                if reset {
                    return self
                        .control(effort_request("default"), Waiting::Reset { id, options })
                        .await;
                }
                self.answer(id, json!({"configOptions": options})).await;
            }
            Waiting::Reset { id, options } => {
                if let Some(error) = refused {
                    return self.fail(id, (INVALID, error)).await;
                }
                self.answer(id, json!({"configOptions": options})).await;
            }
            Waiting::Effort { id, effort } => {
                if let Some(error) = refused {
                    return self.fail(id, (INVALID, error)).await;
                }
                let Some(claude) = &mut self.claude else {
                    return;
                };
                claude.effort = effort;
                let options = config_options(claude);
                self.answer(id, json!({"configOptions": options})).await;
            }
            Waiting::Nothing => {}
        }
    }

    /// The history of a loaded session, as `session/update`s before the answer.
    async fn replay(&mut self) {
        let Some(claude) = &self.claude else { return };
        let Ok(history) = std::fs::read_to_string(&claude.history) else {
            return;
        };
        for line in history.lines() {
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let (kind, text) = if let Some(text) = entry["user"].as_str() {
                ("user_message_chunk", text)
            } else if let Some(text) = entry["agent"].as_str() {
                ("agent_message_chunk", text)
            } else if let Some(text) = entry["thought"].as_str()
                && !text.is_empty()
            {
                ("agent_thought_chunk", text)
            } else {
                continue;
            };
            self.chunk(kind, text, entry["id"].as_str()).await;
        }
    }

    async fn control_asked(&mut self, message: &Value) {
        let request = &message["request"];
        let request_id = message["request_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if request["subtype"] != "can_use_tool" {
            return self
                .write_claude(json!({"type": "control_response", "response": {"subtype": "error",
                    "request_id": request_id, "error": "the Claude bridge does not take this request"}}))
                .await;
        }
        let Some(claude) = &self.claude else { return };
        let session = claude.session.clone();
        self.next += 1;
        let id = format!("permission-{}", self.next);
        let tool = request["tool_name"].as_str().unwrap_or_default();
        self.asked
            .insert(id.clone(), (request_id, request["input"].clone()));
        self.send(
            json!({"jsonrpc": "2.0", "id": id, "method": "session/request_permission",
            "params": {
                "sessionId": session,
                "toolCall": {"toolCallId": request["tool_use_id"], "title": tool,
                    "rawInput": request["input"], "_meta": {"claudeCode": {"toolName": tool}}},
                "options": [
                    {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                    {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
                ],
            }}),
        )
        .await;
    }

    /// A piece of text, of the API message `message` when it is known: the pieces that share an
    /// id are one message to the readers.
    async fn chunk(&mut self, kind: &str, text: &str, message: Option<&str>) {
        let mut update = json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}});
        if let Some(message) = message {
            update["messageId"] = message.into();
        }
        self.update(update).await;
    }

    async fn streamed(&mut self, event: &Value) {
        if event["type"] == "message_start" {
            if let Some(turn) = &mut self.turn {
                turn.message = event["message"]["id"].as_str().map(str::to_owned);
            }
            return;
        }
        if event["type"] != "content_block_delta" {
            return;
        }
        let delta = &event["delta"];
        let (kind, text) = match delta["type"].as_str() {
            Some("text_delta") => ("agent_message_chunk", &delta["text"]),
            Some("thinking_delta") => ("agent_thought_chunk", &delta["thinking"]),
            _ => return,
        };
        let Some(text) = text.as_str() else { return };
        let mut message = None;
        if let Some(turn) = &mut self.turn {
            turn.streamed = true;
            message = turn.message.clone();
        }
        self.chunk(kind, text, message.as_deref()).await;
    }

    async fn assistant(&mut self, whole: &Value) {
        let message = &whole["message"];
        if let Some(turn) = &mut self.turn {
            turn.error = whole["error"].as_str().map(str::to_owned);
            let text = message["content"][0]["text"].as_str().unwrap_or_default();
            if message["model"] == "<synthetic>" && USAGE_LIMIT.iter().any(|p| text.starts_with(p))
            {
                turn.limited = true;
            }
        }
        let streamed = self.turn.as_ref().is_some_and(|t| t.streamed);
        // Absent for a message `claude` wrote itself, which no API message is behind.
        let id = message["id"].as_str();
        for block in message["content"].as_array().into_iter().flatten() {
            let (kind, key, text) = match block["type"].as_str() {
                Some("text") => ("agent_message_chunk", "agent", &block["text"]),
                Some("thinking") => ("agent_thought_chunk", "thought", &block["thinking"]),
                Some("tool_use") => {
                    let tool = block["name"].as_str().unwrap_or_default();
                    // The call's id is the tool use's, which `claude` hands Flottille's MCP server
                    // as `claudecode/toolUseId`.
                    self.update(
                        json!({"sessionUpdate": "tool_call", "toolCallId": block["id"],
                        "title": tool, "kind": "other", "status": "in_progress",
                        "rawInput": block["input"], "_meta": {"claudeCode": {"toolName": tool}}}),
                    )
                    .await;
                    continue;
                }
                _ => continue,
            };
            let text = text.as_str().unwrap_or_default();
            // A thought whose display Claude omitted has its signature and no text: nothing to
            // show or keep.
            if kind == "agent_thought_chunk" && text.is_empty() {
                continue;
            }
            // The history keeps the whole message, one line, never its stream's pieces.
            if let Some(claude) = &self.claude {
                remember(&claude.history, key, text, id, &self.env);
            }
            if !streamed {
                self.chunk(kind, text, id).await;
            }
        }
        if let Some(turn) = &mut self.turn {
            turn.streamed = false;
            let usage = &message["usage"];
            if usage.is_object() {
                turn.used = Some(
                    [
                        "input_tokens",
                        "output_tokens",
                        "cache_read_input_tokens",
                        "cache_creation_input_tokens",
                    ]
                    .iter()
                    .map(|key| usage[key].as_u64().unwrap_or(0))
                    .sum(),
                );
            }
        }
    }

    async fn tool_results(&mut self, message: &Value) {
        for block in message["content"].as_array().into_iter().flatten() {
            if block["type"] != "tool_result" {
                continue;
            }
            let status = if block["is_error"] == true {
                "failed"
            } else {
                "completed"
            };
            self.update(json!({"sessionUpdate": "tool_call_update",
                "toolCallId": block["tool_use_id"], "status": status, "rawOutput": block["content"]}))
                .await;
        }
    }

    async fn system(&mut self, message: &Value) {
        match message["subtype"].as_str() {
            Some("compact_boundary") => {
                let metadata = &message["compact_metadata"];
                self.next += 1;
                let id = format!("compaction-{}", self.next);
                self.update(json!({"sessionUpdate": "tool_call", "toolCallId": id,
                    "title": "Compacting the conversation", "kind": "other", "status": "completed",
                    "_meta": {"contextCompaction": {"trigger": metadata["trigger"],
                        "preTokens": metadata["pre_tokens"], "postTokens": metadata["post_tokens"]}}}))
                    .await;
                if let (Some(turn), Some(used)) = (&mut self.turn, metadata["post_tokens"].as_u64())
                {
                    turn.used = Some(used);
                }
            }
            Some("api_retry") => {
                let text = format!(
                    "the provider is retrying (attempt {} of {}): {}",
                    message["attempt"],
                    message["max_retries"],
                    message["error"].as_str().unwrap_or("unknown error")
                );
                self.update(
                    json!({"sessionUpdate": "session_info_update", "_meta": {"jetbrains":
                    {"air": {"sessionFailure": {"severity": "warning", "message": text}}}}}),
                )
                .await;
            }
            _ => {}
        }
    }

    /// The end of a turn: the context in use, then the prompt's answer.
    async fn result(&mut self, message: &Value) {
        let Some(turn) = self.turn.take() else { return };
        let window = message["modelUsage"]
            .as_object()
            .into_iter()
            .flat_map(|models| models.values())
            .filter_map(|model| model["contextWindow"].as_u64())
            .max();
        if let Some(claude) = &mut self.claude {
            claude.window = window.or(claude.window);
        }
        let window = self.claude.as_ref().and_then(|c| c.window);
        if let (Some(used), Some(size)) = (turn.used, window) {
            let mut update = json!({"sessionUpdate": "usage_update", "used": used, "size": size});
            if let Some(cost) = message["total_cost_usd"].as_f64() {
                update["cost"] = json!({"amount": cost, "currency": "USD"});
            }
            self.update(update).await;
        }
        let stop = message["stop_reason"].as_str();
        let stop_reason = if turn.cancelled {
            "cancelled"
        } else if stop == Some("refusal") {
            "refusal"
        } else if stop == Some("max_tokens") {
            "max_tokens"
        } else if matches!(
            message["subtype"].as_str(),
            Some(
                "error_max_turns" | "error_max_budget_usd" | "error_max_structured_output_retries"
            )
        ) {
            "max_turn_requests"
        } else if message["is_error"] == true {
            let errors: Vec<&str> = message["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let text = match message["result"].as_str() {
                Some(text) if !text.is_empty() => text.to_owned(),
                _ if !errors.is_empty() => errors.join(", "),
                _ => message["subtype"].as_str().unwrap_or("error").to_owned(),
            };
            let error = turn.error.as_deref();
            // Signed out: the agent cannot go on until the user signs in again.
            if text.contains("Please run /login")
                || matches!(
                    error,
                    Some("authentication_failed" | "oauth_org_not_allowed")
                )
            {
                return self.fail(turn.id, (INTERNAL, text)).await;
            }
            // Any other error ends the turn, the session alive, as Claude Code's own adapter
            // ends it for a client that takes session failures.
            let limit = turn.limited
                || matches!(
                    error,
                    Some("rate_limit" | "billing_error" | "account_on_hold")
                );
            let category = if limit { "limit" } else { "service" };
            self.update(
                json!({"sessionUpdate": "session_info_update", "_meta": {"jetbrains":
                {"air": {"sessionFailure": {"severity": "error", "category": category,
                    "message": text}}}}}),
            )
            .await;
            "end_turn"
        } else {
            "end_turn"
        };
        let usage = &message["usage"];
        let count = |key: &str| usage[key].as_u64().unwrap_or(0);
        let (input, output, read, write) = (
            count("input_tokens"),
            count("output_tokens"),
            count("cache_read_input_tokens"),
            count("cache_creation_input_tokens"),
        );
        self.answer(
            turn.id,
            json!({"stopReason": stop_reason, "usage": {"inputTokens": input,
                "outputTokens": output, "cachedReadTokens": read, "cachedWriteTokens": write,
                "totalTokens": input + output + read + write}}),
        )
        .await;
    }
}

async fn next(receiver: &mut Option<mpsc::UnboundedReceiver<Value>>) -> Option<Value> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// The options of a bare session: what the daemon set, checked to be bare.
#[derive(Debug, PartialEq)]
struct Options {
    allowed: Vec<String>,
    prompt: Vec<String>,
    snapshot: bool,
    env: Vec<(String, String)>,
}

fn bare_options(options: &Value) -> Result<Options, String> {
    let options = options
        .as_object()
        .ok_or("the session has no Claude options: bare mode needs them")?;
    if let Some(key) = options.keys().find(|key| !OPTIONS.contains(&key.as_str())) {
        return Err(format!("the Claude bridge does not take the option {key}"));
    }
    let empty = |key: &str| options.get(key) == Some(&json!([]));
    if !empty("tools")
        || !empty("settingSources")
        || options.get("strictMcpConfig") != Some(&json!(true))
    {
        return Err(
            "not bare: a session needs `tools` and `settingSources` empty and `strictMcpConfig`"
                .into(),
        );
    }
    let strings = |value: Option<&Value>| -> Option<Vec<String>> {
        value?
            .as_array()?
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    let allowed = strings(options.get("allowedTools").or(Some(&json!([]))))
        .filter(|tools| tools.iter().all(|t| !t.is_empty() && !t.contains(',')))
        .ok_or("`allowedTools` must be a list of tool names")?;
    let system = options
        .get("systemPrompt")
        .filter(|p| p["type"] == "custom")
        .ok_or("bare mode needs Flottille's own system prompt")?;
    let prompt = match &system["prompt"] {
        Value::String(text) => vec![text.clone()],
        prompt => strings(Some(prompt)).ok_or("the system prompt is text or a list of texts")?,
    };
    let env = match options.get("env") {
        None => Vec::new(),
        Some(env) => env
            .as_object()
            .ok_or("`env` must be an object of texts")?
            .iter()
            .map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
            .collect::<Option<_>>()
            .ok_or("`env` must be an object of texts")?,
    };
    Ok(Options {
        allowed,
        prompt,
        snapshot: system["snapshot"] != false,
        env,
    })
}

/// The ACP MCP servers as `claude --mcp-config` takes them; HTTP only, as Flottille's is.
fn mcp_servers(servers: &Value) -> Result<Value, String> {
    let mut config = serde_json::Map::new();
    for server in servers.as_array().into_iter().flatten() {
        let name = server["name"].as_str().filter(|n| !n.is_empty());
        let (Some(name), Some(url)) = (name, server["url"].as_str()) else {
            return Err("an MCP server needs a name and a url".into());
        };
        if server["type"] != "http" {
            return Err(format!("the MCP server {name} is not an HTTP one"));
        }
        let headers: serde_json::Map<String, Value> = server["headers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| Some((h["name"].as_str()?.to_owned(), h["value"].clone())))
            .collect();
        config.insert(
            name.to_owned(),
            json!({"type": "http", "url": url, "headers": headers}),
        );
    }
    Ok(json!({"mcpServers": config}))
}

/// `claude`'s arguments, as the version of `claude::CLAUDE_VERSION` takes them: stream-json both
/// ways, no built-in tool, no setting, no MCP server but the configuration's, and Claude's own
/// permission prompts sent to the bridge.
fn arguments(options: &Options, config: &Path, new: bool, session: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
        "--include-partial-messages",
        // Without it, Claude Code in this mode leaves the API's default, which omits the text of
        // the thoughts of recent models: each would come as a signature with no text.
        "--thinking-display=summarized",
        "--permission-prompt-tool=stdio",
        "--tools=",
        "--setting-sources=",
        "--strict-mcp-config",
    ]
    .map(str::to_owned)
    .to_vec();
    if !options.allowed.is_empty() {
        args.push(format!("--allowedTools={}", options.allowed.join(",")));
    }
    args.push("--mcp-config".into());
    args.push(config.display().to_string());
    args.push(if new {
        format!("--session-id={session}")
    } else {
        format!("--resume={session}")
    });
    args
}

/// The session's options: the models `claude` listed, and the effort levels of the one set when
/// it has any.
fn config_options(claude: &Claude) -> Value {
    let mut models: Vec<Value> = claude
        .models
        .iter()
        .filter_map(|model| {
            let value = model["value"].as_str()?;
            let name = model["displayName"].as_str().unwrap_or(value);
            Some(json!({"value": value, "name": name}))
        })
        .collect();
    if !models.iter().any(|m| m["value"] == "default") {
        models.insert(0, json!({"value": "default", "name": "Default"}));
    }
    let mut options = vec![
        json!({"id": "model", "name": "Model", "category": "model", "type": "select",
        "currentValue": claude.model, "options": models}),
    ];
    if let Some(levels) = effort_levels(&claude.models, &claude.model) {
        let levels: Vec<Value> = levels
            .iter()
            .map(|level| json!({"value": level, "name": level}))
            .collect();
        options.push(
            json!({"id": "effort", "name": "Effort", "category": "thought_level",
            "type": "select", "currentValue": claude.effort, "options": levels}),
        );
    }
    Value::Array(options)
}

/// The models offered, `default` first when `claude` did not list it.
fn model_values(models: &[Value]) -> Vec<String> {
    let mut values: Vec<String> = models
        .iter()
        .filter_map(|m| m["value"].as_str().map(str::to_owned))
        .collect();
    if !values.iter().any(|v| v == "default") {
        values.insert(0, "default".into());
    }
    values
}

/// The effort levels of a model, `default` first; `None` for a model without any.
fn effort_levels(models: &[Value], model: &str) -> Option<Vec<String>> {
    let model = models.iter().find(|m| m["value"] == model)?;
    let levels: Vec<String> = model["supportedEffortLevels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l.as_str().map(str::to_owned))
        .collect();
    (model["supportsEffort"] == true && !levels.is_empty()).then(|| {
        std::iter::once("default".to_owned())
            .chain(levels)
            .collect()
    })
}

/// Sets the session's effort on `claude`'s flag layer, as `--effort` would; `null` goes back to
/// its own.
fn effort_request(effort: &str) -> Value {
    let level = if effort == "default" {
        Value::Null
    } else {
        effort.into()
    };
    json!({"subtype": "apply_flag_settings", "settings": {"effortLevel": level}})
}

/// A file only the user can read, for the token it holds.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())
}

/// One message of the session's history, its secrets masked, with the id of the API message it
/// comes from when it has one; a history that cannot be written only loses its replay.
fn remember(
    history: &Path,
    key: &str,
    text: &str,
    id: Option<&str>,
    env: &BTreeMap<OsString, OsString>,
) {
    let mut entry = json!({key: secrets::mask(text, env)});
    if let Some(id) = id {
        entry["id"] = id.into();
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(history)
    else {
        return;
    };
    let _ = file.write_all(format!("{entry}\n").as_bytes());
    if file.metadata().is_ok_and(|m| m.len() > HISTORY_CAP) {
        let _ = keep_later_half(history, HISTORY_CAP);
    }
}

/// The history cut to the lines of its last `cap / 2` bytes.
fn keep_later_half(history: &Path, cap: u64) -> std::io::Result<()> {
    let text = std::fs::read(history)?;
    let from = text.len().saturating_sub((cap / 2) as usize);
    let start = text[from..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(text.len(), |at| from + at + 1);
    let kept = history.with_extension("jsonl.kept");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&kept)?;
    file.write_all(&text[start..])?;
    std::fs::rename(kept, history)
}

/// A new session id, a random UUID as `claude --session-id` takes it.
fn new_session_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
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
    fn a_session_id_is_a_version_4_uuid() {
        let id = new_session_id().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(claude::is_session_id(&id));
        assert_ne!(id, new_session_id().unwrap());
        assert!(!claude::is_session_id("../../etc"));
        assert!(!claude::is_session_id(""));
    }

    #[test]
    fn a_text_system_prompt_and_missing_tools_are_taken_as_bare() {
        let options = bare_options(&json!({"tools": [], "settingSources": [],
            "strictMcpConfig": true, "systemPrompt": {"type": "custom", "prompt": "plain"}}))
        .unwrap();
        assert_eq!(options.prompt, ["plain"]);
        assert!(options.allowed.is_empty());
        assert!(options.snapshot);
        assert!(
            bare_options(
                &json!({"tools": [], "settingSources": [], "strictMcpConfig": true,
            "systemPrompt": {"type": "custom", "prompt": "p"}, "allowedTools": ["a,b"]})
            )
            .is_err()
        );
    }

    #[test]
    fn a_history_past_its_cap_keeps_its_later_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let history = dir.path().join("s.jsonl");
        for n in 0..50 {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&history)
                .unwrap()
                .write_all(format!("{}\n", json!({"agent": format!("line {n}")})).as_bytes())
                .unwrap();
        }

        keep_later_half(&history, 200).unwrap();

        let text = std::fs::read_to_string(&history).unwrap();
        assert!(text.len() <= 100, "{}", text.len());
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.last().unwrap()["agent"], "line 49");
        assert!(lines.len() > 1);
    }

    #[test]
    fn only_http_mcp_servers_are_taken() {
        assert!(
            mcp_servers(&json!([{"type": "stdio", "name": "x", "url": "u", "command": "sh"}]))
                .is_err()
        );
        assert_eq!(mcp_servers(&json!([])).unwrap(), json!({"mcpServers": {}}));
    }
}
