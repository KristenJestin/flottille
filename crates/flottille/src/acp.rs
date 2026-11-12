//! The ACP client side of one agent process, its provider's ACP agent (`docs/agents.md`): start
//! it, shake hands, open the session (resume, else load, else new), set the options, then play the
//! prompts it is given until it is released, stopped, or dies.
//!
//! Built on the official `agent-client-protocol` crate, over the process's stdio. Its callbacks
//! run in its dispatch loop, in the order messages arrive: an update is stored before the answer
//! that follows it is seen, which is what flags a load's replay exactly.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, HttpHeader, ImageContent, Implementation,
    InitializeRequest, LoadSessionRequest, McpServer, McpServerHttp, NewSessionRequest,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest,
    SelectedPermissionOutcome, SessionConfigOptionValue, SessionNotification,
    SetSessionConfigOptionRequest, TextContent,
};
use agent_client_protocol::{Agent as AcpAgent, Client, ConnectionTo, ErrorCode, Lines};
use flottille_core::agent::{Agent, Choices, Used};
use flottille_core::provider;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::VERSION;
use crate::agents::{Order, Turns};
use crate::deliveries::{self, Prompt};
use crate::providers::{self, MCP_SERVER, Provider};

/// A prompt as it may be sent: only one that starts with a delivery's marker. An agent may read a
/// prompt starting otherwise as its own: OpenCode runs one whose text starts with `/` as a command,
/// and a command can run a shell outside the gate (`docs/agents.md`, "Bare mode for OpenCode").
fn prompt_text(text: String) -> Result<String, String> {
    if deliveries::is_delivery(&text) {
        Ok(text)
    } else {
        Err(
            "Flottille refused to send a prompt that does not start with a delivery's marker"
                .into(),
        )
    }
}

/// A text kept in an event is cut here.
const TEXT_LIMIT: usize = 64 * 1024;

/// How the process's session ended, as far as the session loop saw it.
pub enum Exit {
    /// Idle for the release time: let go, to be started again by the next prompt.
    Released,
    /// The daemon stops, or the agent is stopped.
    Stopped,
    /// The process ended by itself.
    Died(ExitStatus),
    /// Its session is to be replaced, for this reason.
    Replace(String),
    /// Its conversation was summarised: a new session goes on from the summary.
    Compact(Box<crate::compaction::Compacted>),
    /// The user reloads its tools: let go between turns, to be started again on its session with
    /// the tools its role has now.
    Reload,
    /// Its turn failed for a reason that leaves its session usable (`turn_failures`), which the
    /// state line says: let go, to be started again on its session by the next prompt.
    TurnFailed(&'static str),
}

/// What the session loop tells the agent's owner as it goes.
pub trait Events: Send + Sync + 'static {
    /// An event of the agent's conversation; `activity` when it is a sign of activity.
    fn conversation(&self, name: &str, data: Value, activity: bool);
    /// What the session offers in its config options: once it is open, and once the choices
    /// are set. `fresh` when it is a new session none was set on yet: its model is the default;
    /// `own_effort` when no effort was set on the new session: its effort is its model's own.
    fn offered(&self, options: &Value, fresh: bool, own_effort: bool);
    /// The session is open: how, its id, the agent's version, what the agent took, and whether
    /// the agent can resume or load a session (`reopens`).
    fn opened(
        &self,
        opened: &str,
        native_id: &str,
        agent_version: &str,
        taken: Choices,
        reopens: bool,
        images: bool,
    );
    /// The model and the effort the user changed to, to set before the next prompt.
    fn pending(&self) -> Option<Choices>;
    /// What setting them came to: `settled` is what of the change is done with (taken, replaced
    /// or refused), `changed` the values the agent has now for what it changed, `taken` all it
    /// has now, `options` its config options, `refused` why it did not take the rest.
    fn options_set(
        &self,
        settled: Choices,
        changed: Choices,
        taken: Choices,
        options: &Value,
        refused: Vec<String>,
    );
    /// The next prompt, taken from what was delivered, when there is one.
    fn pump(&self) -> Option<Prompt>;
    /// The user asked to reload the agent's tools since this process was given them.
    fn reloading(&self) -> bool;
    /// An image a prompt carries, as its MIME type and its bytes in base64.
    fn image(&self, name: &str) -> Option<(&'static str, String)>;
    /// `typed`: the prompt's `Prompt::typed`.
    fn turn_started(&self, text: &str, images: &[String], typed: &Value, deliveries: &[String]);
    /// The end of a turn, with what it used, one entry per model. A turn ended `failed` gives
    /// its deliveries back.
    fn turn_ended(&self, stop_reason: &str, usage: Value, used: Vec<Used>);
    /// The agent's tools as they are now: its role may have gained some since the session began
    /// (`docs/tools.md`, "When the tools change").
    fn tools(&self) -> Vec<String>;
}

/// What a turn used, from the prompt's answer (`docs/reference/acp.md`, "What a turn used"):
/// Flottille's bridges say it per model in `_meta.flottille.usage`, another agent may give ACP's
/// `usage`, and an agent that says nothing gives one entry without counts.
fn used(answer: &Value) -> Vec<Used> {
    let of = |usage: &Value| Used {
        model: usage["model"].as_str().map(str::to_owned),
        input: usage["inputTokens"].as_u64(),
        output: usage["outputTokens"].as_u64(),
        cache_read: usage["cachedReadTokens"].as_u64(),
        cache_write: usage["cachedWriteTokens"].as_u64(),
        reasoning: usage["thoughtTokens"].as_u64(),
        cost_usd: (usage["cost"]["currency"] == "USD")
            .then(|| usage["cost"]["amount"].as_f64())
            .flatten(),
    };
    match answer["_meta"]["flottille"]["usage"].as_array() {
        Some(models) if !models.is_empty() => models.iter().map(of).collect(),
        _ => vec![of(&answer["usage"])],
    }
}

/// Flottille's MCP server as the agent's session gets it: its address and the process's token.
pub struct Server {
    pub url: String,
    pub token: String,
}

/// Runs the session over the process's stdio until it ends. An error is a failure of the
/// session itself (a refused option, an ACP error): the caller stops the process.
#[allow(clippy::too_many_arguments)]
pub async fn session(
    agent: &Agent,
    provider: &'static Provider,
    child: &mut Child,
    events: Arc<dyn Events>,
    orders: &mut UnboundedReceiver<Order>,
    turns: &mut Turns,
    idle: Duration,
    server: Server,
) -> Result<Exit, String> {
    let stdin = child.stdin.take().ok_or("the process has no stdin")?;
    let stdout = child.stdout.take().ok_or("the process has no stdout")?;
    let incoming = futures::stream::unfold(BufReader::new(stdout).lines(), |mut lines| async {
        match lines.next_line().await {
            Ok(Some(line)) => Some((Ok(line), lines)),
            Ok(None) => None,
            Err(error) => Some((Err(error), lines)),
        }
    });
    let outgoing = futures::sink::unfold(stdin, |mut stdin: ChildStdin, line: String| async move {
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok::<_, std::io::Error>(stdin)
    });
    let transport = Lines::new(Box::pin(outgoing), Box::pin(incoming));

    let replay = Arc::new(AtomicBool::new(false));
    let (updates, answers) = (events.clone(), events.clone());
    let replaying = replay.clone();
    let result = Client
        .builder()
        .name("flottille")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                let replay = replaying.load(Ordering::SeqCst);
                if let Ok(update) = serde_json::to_value(&notification.update)
                    && let Some((name, mut data)) = event_of(&update)
                {
                    data.insert("replay".into(), replay.into());
                    updates.conversation(name, Value::Object(data), !replay);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                let (tool, outcome, allowed) =
                    answer_permission(&request, &answers.tools(), provider);
                answers.conversation(
                    "agent.permission_answered",
                    json!({"tool": tool, "allowed": allowed, "replay": false}),
                    true,
                );
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async |cx: ConnectionTo<AcpAgent>| {
            Ok(drive(
                cx, agent, provider, child, &events, &replay, orders, turns, idle, &server,
            )
            .await)
        })
        .await;
    match result {
        Ok(exit) => exit,
        // The connection ended under the loop: the process is gone, or going.
        Err(error) => match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
            Ok(Ok(status)) => Ok(Exit::Died(status)),
            _ => Err(format!("the connection to the agent failed: {error}")),
        },
    }
}

type Turn<'a> =
    Pin<Box<dyn Future<Output = Result<PromptResponse, agent_client_protocol::Error>> + Send + 'a>>;

#[allow(clippy::too_many_arguments)]
async fn drive(
    cx: ConnectionTo<AcpAgent>,
    agent: &Agent,
    provider: &Provider,
    child: &mut Child,
    events: &Arc<dyn Events>,
    replay: &AtomicBool,
    orders: &mut UnboundedReceiver<Order>,
    turns: &mut Turns,
    idle: Duration,
    server: &Server,
) -> Result<Exit, String> {
    let acp = |error: agent_client_protocol::Error| error.to_string();
    let capabilities = ClientCapabilities::new().meta(
        json!({"jetbrains": {"air": {"version": 1, "capabilities": ["recommendedValue", "sessionFailure"]}}})
            .as_object()
            .cloned(),
    );
    let initialized = cx
        .send_request(
            InitializeRequest::new(ProtocolVersion::V1)
                .client_capabilities(capabilities)
                .client_info(Implementation::new("Flottille", VERSION)),
        )
        .block_task()
        .await
        .map_err(|e| format!("the agent refused to start: {e}"))?;
    let initialized = serde_json::to_value(&initialized).unwrap_or_default();
    let loads = initialized["agentCapabilities"]["loadSession"] == true;
    let resumes = !initialized["agentCapabilities"]["sessionCapabilities"]["resume"].is_null();
    let takes_images = initialized["agentCapabilities"]["promptCapabilities"]["image"] == true;
    let agent_version = format!(
        "{} {}",
        initialized["agentInfo"]["name"].as_str().unwrap_or("agent"),
        initialized["agentInfo"]["version"].as_str().unwrap_or("?")
    );

    let meta = (provider.session_meta)(&agent.instructions, &agent.tools)
        .as_object()
        .cloned();
    let servers = vec![mcp_server(server)];
    let folder: &Path = &agent.folder;
    let (opened, native_id, options) = match &agent.native_id {
        Some(native) if resumes => {
            let answer = cx
                .send_request(
                    ResumeSessionRequest::new(native.clone(), folder)
                        .mcp_servers(servers)
                        .meta(meta),
                )
                .block_task()
                .await
                .map_err(|e| format!("the agent could not resume its session: {e}"))?;
            let answer = serde_json::to_value(&answer).unwrap_or_default();
            ("resumed", native.clone(), answer["configOptions"].clone())
        }
        Some(native) if loads => {
            replay.store(true, Ordering::SeqCst);
            let answer = cx
                .send_request(
                    LoadSessionRequest::new(native.clone(), folder)
                        .mcp_servers(servers)
                        .meta(meta),
                )
                .block_task()
                .await;
            replay.store(false, Ordering::SeqCst);
            let answer = serde_json::to_value(
                &answer.map_err(|e| format!("the agent could not load its session: {e}"))?,
            )
            .unwrap_or_default();
            ("loaded", native.clone(), answer["configOptions"].clone())
        }
        _ => {
            let answer = cx
                .send_request(
                    NewSessionRequest::new(folder)
                        .mcp_servers(servers)
                        .meta(meta),
                )
                .block_task()
                .await
                .map_err(|e| format!("the agent could not open a session: {e}"))?;
            let answer = serde_json::to_value(&answer).unwrap_or_default();
            let id = answer["sessionId"]
                .as_str()
                .ok_or("the agent opened a session without an id")?
                .to_owned();
            ("new", id, answer["configOptions"].clone())
        }
    };

    let new = opened == "new";
    events.offered(&options, new, new);
    // The user's choices, in this order, each checked: a model is never silently replaced.
    let mut options = options;
    let chosen = [
        ("model", "model", &agent.chosen.model),
        ("thought_level", "effort", &agent.chosen.effort),
        ("mode", "mode", &agent.chosen.mode),
    ];
    // A model the user changed to since replaces the chosen one, which the agent may no longer
    // offer: it is set first, and the chosen effort only when the change named none and the new
    // model has it.
    let replaced = events.pending().filter(|pending| pending.model.is_some());
    let mut held = false;
    if let Some(pending) = &replaced {
        held = apply_pending(&cx, &native_id, &mut options, events, pending).await;
    }
    // A new session is never left on the provider's own choice: what it stands for is set by
    // its name, unless a change waiting names it.
    let waiting = events.pending().unwrap_or_default();
    let mut set_any = false;
    for (category, word, value) in chosen {
        let own = match (category, value) {
            (_, Some(_)) => None,
            ("model", None) if new && waiting.model.is_none() => own_choice(&options, category),
            // Not only a new session: a process that opens on the agent's own choice again (a
            // resumed session, none having chosen its effort) runs with a named effort too.
            ("thought_level", None) if waiting.effort.is_none() => own_choice(&options, category),
            _ => None,
        };
        let Some(value) = value.as_ref().or(own.as_ref()) else {
            continue;
        };
        set_any = true;
        if let Some(pending) = &replaced {
            let offered = find_option(&options, category)
                .is_some_and(|option| providers::values(&option).contains(value));
            match category {
                "model" => continue,
                "thought_level" if pending.effort.is_some() || !offered => continue,
                _ => {}
            }
        }
        options = match set_option(&cx, &native_id, &options, category, word, value).await {
            Ok((set, None)) => set,
            Ok((_, Some(reason))) | Err(Unset::Refused(reason) | Unset::Broken(reason)) => {
                return Err(reason);
            }
        };
    }
    if set_any {
        // The effort a session keeps once a model is set may be the previous model's: no effort
        // is learned from it.
        events.offered(&options, false, false);
    }
    // A change the user made since the process last ran waits for it, and is not a failure.
    if replaced.is_none()
        && let Some(pending) = events.pending()
    {
        held = apply_pending(&cx, &native_id, &mut options, events, &pending).await;
    }
    events.opened(
        opened,
        &native_id,
        &agent_version,
        taken_of(&options),
        loads || resumes,
        takes_images,
    );

    let mut turn: Option<Turn> = None;
    // A compaction that came while a turn ran: its new session starts once the turn ended.
    let mut compacting: Option<Box<crate::compaction::Compacted>> = None;
    loop {
        if turn.is_none()
            && let Some(compacted) = compacting.take()
        {
            return Ok(Exit::Compact(compacted));
        }
        // A reload never cuts a turn: the process goes once it ended, before the next prompt,
        // which the next process plays.
        if turn.is_none() && events.reloading() {
            return Ok(Exit::Reload);
        }
        // Between turns only: what was chosen during one is set once it ended, before the next
        // prompt goes. A change the agent could not answer is tried again only then.
        if turn.is_none()
            && !held
            && let Some(pending) = events.pending()
        {
            held = apply_pending(&cx, &native_id, &mut options, events, &pending).await;
        }
        if turn.is_none()
            && turns.queued.is_empty()
            && let Some(prompt) = events.pump()
        {
            turns.queued.push_back(prompt);
        }
        if turn.is_none()
            && let Some(prompt) = turns.queued.pop_front()
        {
            if held && let Some(pending) = events.pending() {
                held = apply_pending(&cx, &native_id, &mut options, events, &pending).await;
            }
            let mut text = prompt_text(prompt.text)?;
            // What cannot go to the agent is said in the text, which the event keeps: the
            // user's words are never lost, and the window shows why a picture is not there.
            let mut sent = Vec::new();
            let mut notes = Vec::new();
            if !takes_images && !prompt.images.is_empty() {
                let count = prompt.images.len();
                notes.push(format!(
                    "[{count} image{} not sent: this agent takes no images]",
                    if count == 1 { "" } else { "s" }
                ));
            } else {
                for name in prompt.images {
                    match events.image(&name) {
                        Some((mime, data)) => sent.push((name, mime, data)),
                        None => notes.push(format!("[image {name} is missing]")),
                    }
                }
            }
            for note in notes {
                text.push_str("\n\n");
                text.push_str(&note);
            }
            let names: Vec<String> = sent.iter().map(|(name, ..)| name.clone()).collect();
            events.turn_started(&text, &names, &prompt.typed, &prompt.deliveries);
            turns.running = true;
            let mut blocks = vec![ContentBlock::Text(TextContent::new(text))];
            blocks.extend(
                sent.into_iter()
                    .map(|(_, mime, data)| ContentBlock::Image(ImageContent::new(data, mime))),
            );
            let request = PromptRequest::new(native_id.clone(), blocks);
            turn = Some(Box::pin(cx.send_request(request).block_task()));
        }
        tokio::select! {
            answer = async { turn.as_mut().expect("guarded").as_mut().await }, if turn.is_some() => {
                turn = None;
                let answer = match answer {
                    Ok(answer) => answer,
                    // A process that dies mid-turn fails its prompt first: its end is the news.
                    Err(error) => {
                        return match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
                            Ok(Ok(status)) => Ok(Exit::Died(status)),
                            _ => match crate::turn_failures::recognise(
                                provider.name,
                                error.data.as_ref().and_then(|data| data["code"].as_str()),
                                &error.message,
                            ) {
                                Some(said) => {
                                    turns.running = false;
                                    events.turn_ended("failed", Value::Null, Vec::new());
                                    Ok(Exit::TurnFailed(said))
                                }
                                None => Err(acp(error)),
                            },
                        };
                    }
                };
                turns.running = false;
                let answer = serde_json::to_value(&answer).unwrap_or_default();
                let stop = answer["stopReason"].as_str().unwrap_or("end_turn");
                events.turn_ended(
                    stop,
                    answer.get("usage").cloned().unwrap_or(Value::Null),
                    used(&answer),
                );
            }
            order = orders.recv() => match order {
                None | Some(Order::Stop) => return Ok(Exit::Stopped),
                Some(Order::Replace(reason)) => return Ok(Exit::Replace(reason)),
                // The agent's prompts wait while it is summarised, so no turn should run; one that
                // started before is never cut.
                Some(Order::Compact(compacted)) if turn.is_none() => {
                    return Ok(Exit::Compact(compacted));
                }
                Some(Order::Compact(compacted)) => compacting = Some(compacted),
                // What was delivered is pumped at the top of the loop, once no turn runs.
                Some(Order::Ring) => {}
                Some(Order::Cancel(answer)) => {
                    let running = turn.is_some();
                    if running {
                        cx.send_notification(CancelNotification::new(native_id.clone()))
                            .map_err(acp)?;
                    }
                    let _ = answer.send(running);
                }
            },
            status = child.wait() => {
                return status.map(Exit::Died).map_err(|e| e.to_string());
            }
            _ = tokio::time::sleep(idle), if turn.is_none() && turns.queued.is_empty() => {
                return Ok(Exit::Released);
            }
        }
    }
}

/// Flottille's MCP server, the session's only one, with the process's token in a header (which
/// keeps it out of the prompt cache's way).
fn mcp_server(server: &Server) -> McpServer {
    let mut http = McpServerHttp::new(MCP_SERVER, server.url.clone());
    http.headers = vec![HttpHeader::new(
        "Authorization",
        format!("Bearer {}", server.token),
    )];
    McpServer::Http(http)
}

/// What the agent has now, a value named `default` standing for the agent's recommended one when
/// it names it.
fn taken_of(options: &Value) -> Choices {
    let current = |category| {
        let option = find_option(options, category)?;
        let value = option["currentValue"].as_str()?;
        let recommended = option["_meta"]["jetbrains"]["air"]["recommendedValue"].as_str();
        Some(match recommended {
            Some(recommended) if value == "default" => recommended.to_owned(),
            _ => value.to_owned(),
        })
    };
    // An effort is never kept as `default`: it is the one Flottille applies for it, else none.
    let effort = match current("thought_level") {
        Some(effort) if effort == provider::OWN_CHOICE => own_choice(options, "thought_level"),
        effort => effort,
    };
    Choices {
        model: current("model"),
        effort,
        mode: current("mode"),
    }
}

/// What a session left on its provider's own choice (`default`) of a model or an effort runs
/// with, by its name: what the agent says it stands for, else the first model it lists or the
/// middle of the efforts it offers. `None` when the option already names a value.
fn own_choice(options: &Value, category: &str) -> Option<String> {
    let option = find_option(options, category)?;
    if option["currentValue"].as_str()? != provider::OWN_CHOICE {
        return None;
    }
    let offered = providers::values(&option);
    let recommended = option["_meta"]["jetbrains"]["air"]["recommendedValue"].as_str();
    let found = match category {
        "model" => provider::default_model(
            offered.iter().map(String::as_str),
            providers::alias_of(&option),
        ),
        _ => recommended
            .filter(|r| offered.iter().any(|o| o == r))
            .or_else(|| provider::middle_effort(&offered).map(String::as_str)),
    };
    found.map(str::to_owned)
}

/// Why an option was not set: the agent refused the value, or could not answer (its process or
/// its service failed), which says nothing of the value.
enum Unset {
    Refused(String),
    Broken(String),
}

/// Sets one option as the user chose it: the agent's options as they are then, with why when it
/// took another value than the one asked; or why it could not.
async fn set_option(
    cx: &ConnectionTo<AcpAgent>,
    native_id: &str,
    options: &Value,
    category: &str,
    word: &str,
    value: &str,
) -> Result<(Value, Option<String>), Unset> {
    let option = find_option(options, category).ok_or_else(|| {
        Unset::Refused(format!(
            "the agent has no {word} option: it cannot take {value}"
        ))
    })?;
    let offered = providers::values(&option);
    // A model may be asked without its date suffix: the session's own id is what is set.
    let listed = match category {
        "model" => {
            flottille_core::provider::listed_model(offered.iter().map(String::as_str), value)
        }
        _ => offered
            .iter()
            .map(String::as_str)
            .find(|offered| *offered == value),
    };
    let Some(value) = listed.map(str::to_owned) else {
        return Err(Unset::Refused(format!(
            "the agent does not offer the {word} {value}: it offers {}",
            offered.join(", ")
        )));
    };
    let id = option["id"].as_str().unwrap_or_default().to_owned();
    let answer = cx
        .send_request(SetSessionConfigOptionRequest::new(
            native_id.to_owned(),
            id,
            SessionConfigOptionValue::value_id(value.clone()),
        ))
        .block_task()
        .await
        .map_err(|e| {
            let reason = format!(
                "the agent refused the {word} {value}: {e}; it offers {}",
                offered.join(", ")
            );
            // An agent refuses a value as invalid; any other error is its process's or its
            // service's.
            match e.code {
                ErrorCode::InvalidParams
                | ErrorCode::InvalidRequest
                | ErrorCode::MethodNotFound => Unset::Refused(reason),
                _ => Unset::Broken(format!("the agent could not set the {word} {value}: {e}")),
            }
        })?;
    let options = serde_json::to_value(&answer).unwrap_or_default()["configOptions"].clone();
    let taken = find_option(&options, category).map(|o| o["currentValue"].clone());
    let other = (taken.as_ref().and_then(Value::as_str) != Some(value.as_str())).then(|| {
        format!(
            "the agent took the {word} {} when {value} was asked",
            taken.unwrap_or_default()
        )
    });
    Ok((options, other))
}

/// Sets the model and the effort the user changed to, the model first. A value the agent refuses
/// never fails it: it goes on with its previous options, and the refusal is said; one it replaced
/// with another is what it has now. Whether a value could not be answered at all: it stays
/// pending, for the next prompt.
async fn apply_pending(
    cx: &ConnectionTo<AcpAgent>,
    native_id: &str,
    options: &mut Value,
    events: &Arc<dyn Events>,
    pending: &Choices,
) -> bool {
    let mut settled = Choices::default();
    let mut changed = Choices::default();
    let mut refused = Vec::new();
    let mut held = false;
    for (category, word, value, settled, changed) in [
        (
            "model",
            "model",
            &pending.model,
            &mut settled.model,
            &mut changed.model,
        ),
        (
            "thought_level",
            "effort",
            &pending.effort,
            &mut settled.effort,
            &mut changed.effort,
        ),
    ] {
        let Some(value) = value else { continue };
        match set_option(cx, native_id, options, category, word, value).await {
            Ok((set, _)) => {
                *options = set;
                *settled = Some(value.clone());
                *changed = find_option(options, category)
                    .and_then(|option| option["currentValue"].as_str().map(str::to_owned));
            }
            Err(Unset::Refused(reason)) => {
                *settled = Some(value.clone());
                refused.push(reason);
            }
            Err(Unset::Broken(reason)) => {
                eprintln!("flottille daemon: session {native_id}: {reason}; kept for later");
                held = true;
            }
        }
    }
    // A model the agent set back on its own effort (the one before is not the new model's) runs
    // with the effort Flottille names for it, as a new session does.
    if pending.effort.is_none()
        && !held
        && let Some(own) = own_choice(options, "thought_level")
    {
        // Nothing waits for it: an agent that does not take it is shown by the name anyway.
        if let Ok((set, _)) =
            set_option(cx, native_id, options, "thought_level", "effort", &own).await
        {
            *options = set;
        }
    }
    events.options_set(settled, changed, taken_of(options), options, refused);
    held
}

fn find_option(options: &Value, category: &str) -> Option<Value> {
    options
        .as_array()?
        .iter()
        .find(|o| o["category"] == category)
        .cloned()
}

/// The agent's own permission prompt, answered at once: one of the session's own tools is
/// allowed, anything else rejected. The tool's name, the outcome, and whether it was allowed.
fn answer_permission(
    request: &RequestPermissionRequest,
    tools: &[String],
    provider: &Provider,
) -> (String, RequestPermissionOutcome, bool) {
    let call = serde_json::to_value(&request.tool_call).unwrap_or_default();
    // Only the tool name the agent states counts: a title is text the model can shape.
    let named = (provider.stated_tool)(&call);
    let tool = named
        .or(call["title"].as_str())
        .unwrap_or_default()
        .to_owned();
    let allowed = named.is_some_and(|name| tools.iter().any(|tool| tool == name));
    let kinds: &[PermissionOptionKind] = if allowed {
        &[PermissionOptionKind::AllowOnce]
    } else {
        &[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ]
    };
    let option = kinds
        .iter()
        .find_map(|kind| request.options.iter().find(|o| &o.kind == kind));
    let outcome = match option {
        Some(option) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
            option.option_id.clone(),
        )),
        None => RequestPermissionOutcome::Cancelled,
    };
    (tool, outcome, allowed && option.is_some())
}

/// The event of an ACP session update, without its `agent` and `replay`; `None` for an update
/// Flottille does not keep.
fn event_of(update: &Value) -> Option<(&'static str, Map<String, Value>)> {
    let text = || {
        let content = &update["content"];
        if content["type"] != "text" {
            return None;
        }
        Some(json!({"text": cut(content["text"].as_str()?)}))
    };
    let (name, data) = match update["sessionUpdate"].as_str()? {
        "user_message_chunk" => ("agent.user_message", text()?),
        "agent_message_chunk" => {
            let mut data = text()?;
            // The pieces that share an id are one message; another id starts another.
            if let Some(id) = update["messageId"].as_str() {
                data["message_id"] = id.into();
            }
            ("agent.message", data)
        }
        "agent_thought_chunk" => ("agent.thought", text()?),
        kind @ ("tool_call" | "tool_call_update") => {
            let meta = &update["_meta"];
            let compaction = !meta["contextCompaction"].is_null()
                || !meta["jetbrains"]["air"]["contextCompaction"].is_null();
            if compaction && update["status"] == "completed" {
                ("agent.compacted", json!({}))
            } else {
                let mut call =
                    json!({"id": update["toolCallId"], "update": kind == "tool_call_update"});
                for (from, to) in [
                    ("title", "title"),
                    ("kind", "kind"),
                    ("status", "status"),
                    ("locations", "locations"),
                    ("content", "content"),
                    ("rawInput", "raw_input"),
                    ("rawOutput", "raw_output"),
                ] {
                    if let Some(value) = update.get(from) {
                        call[to] = value.clone();
                    }
                }
                ("agent.tool_call", call)
            }
        }
        "plan" => ("agent.plan", json!({"entries": update["entries"]})),
        "usage_update" => (
            "agent.usage",
            // A size of 0 is how an agent that does not know its window yet says so (ACP wants
            // a number): the event says `null`.
            json!({"used": update["used"], "size": update["size"].as_u64().filter(|s| *s > 0),
                "cost": update["cost"]}),
        ),
        "current_mode_update" => (
            "agent.mode_changed",
            json!({"mode": update["currentModeId"]}),
        ),
        "config_option_update" => (
            "agent.options_changed",
            json!({"options": update["configOptions"]}),
        ),
        "session_info_update" => {
            let failure = &update["_meta"]["jetbrains"]["air"]["sessionFailure"];
            let message = json!({"message": failure["message"]});
            match (failure["severity"].as_str(), failure["category"].as_str()) {
                (Some("warning"), _) => ("agent.provider_wait", message),
                (Some("error"), Some("limit")) => ("agent.provider_limit", message),
                (Some("error"), _) => ("agent.provider_error", message),
                _ => return None,
            }
        }
        _ => return None,
    };
    match data {
        Value::Object(map) => Some((name, map)),
        _ => None,
    }
}

/// A text cut at the limit, saying how much was left out.
fn cut(text: &str) -> String {
    if text.len() <= TEXT_LIMIT {
        return text.to_owned();
    }
    let mut end = TEXT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}‹+{} chars›", &text[..end], text[end..].chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_not_starting_with_flottille_s_marker_is_never_sent() {
        for text in [
            "/init",
            " /review src",
            "\n/compact",
            "@README.md",
            "!`id`",
            "",
            "hello",
        ] {
            assert!(prompt_text(text.to_owned()).is_err(), "{text:?}");
        }
        let delivery = "[flottille:brief]\n/init and @README.md and !`id`".to_owned();
        assert_eq!(prompt_text(delivery.clone()), Ok(delivery));
    }

    #[test]
    fn a_message_chunk_keeps_the_id_of_its_message_when_it_has_one() {
        let chunk = |extra: Value| {
            let mut update = json!({"sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "hi"}});
            update
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            event_of(&update).unwrap().1
        };
        assert_eq!(chunk(json!({"messageId": "m1"}))["message_id"], "m1");
        assert!(!chunk(json!({})).contains_key("message_id"));
    }

    #[test]
    fn updates_become_events_and_a_compaction_is_its_own() {
        let message = json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}});
        assert_eq!(event_of(&message).unwrap().0, "agent.message");
        let image = json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "image"}});
        assert!(event_of(&image).is_none());
        let compaction = json!({"sessionUpdate": "tool_call", "toolCallId": "c", "status": "completed",
            "_meta": {"jetbrains": {"air": {"contextCompaction": {"version": 1}}}}});
        assert_eq!(event_of(&compaction).unwrap().0, "agent.compacted");
        let wait = json!({"sessionUpdate": "session_info_update",
            "_meta": {"jetbrains": {"air": {"sessionFailure": {"severity": "warning", "message": "overloaded"}}}}});
        assert_eq!(event_of(&wait).unwrap().0, "agent.provider_wait");
        let limit = json!({"sessionUpdate": "session_info_update",
            "_meta": {"jetbrains": {"air": {"sessionFailure": {"severity": "error", "category": "limit"}}}}});
        assert_eq!(event_of(&limit).unwrap().0, "agent.provider_limit");
        let error = json!({"sessionUpdate": "session_info_update",
            "_meta": {"jetbrains": {"air": {"sessionFailure": {"severity": "error", "category": "service"}}}}});
        assert_eq!(event_of(&error).unwrap().0, "agent.provider_error");
        let commands =
            json!({"sessionUpdate": "available_commands_update", "availableCommands": []});
        assert!(event_of(&commands).is_none());
    }

    #[test]
    fn a_long_text_is_cut_and_says_how_much() {
        let text = "é".repeat(TEXT_LIMIT);
        let kept = cut(&text);
        assert!(kept.ends_with(&format!("‹+{} chars›", TEXT_LIMIT / 2)));
        assert_eq!(cut("short"), "short");
    }
}
