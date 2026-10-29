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
use agent_client_protocol::{Agent as AcpAgent, Client, ConnectionTo, Lines};
use flottille_core::agent::{Agent, Choices};
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
}

/// What the session loop tells the agent's owner as it goes.
pub trait Events: Send + Sync + 'static {
    /// An event of the agent's conversation; `activity` when it is a sign of activity.
    fn conversation(&self, name: &str, data: Value, activity: bool);
    /// What the session offers in its config options: once it is open, and once the choices
    /// are set. `fresh` when it is a new session none was set on yet: its model is the default.
    fn offered(&self, options: &Value, fresh: bool);
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
    /// The next prompt, taken from what was delivered, when there is one.
    fn pump(&self) -> Option<Prompt>;
    /// An image a prompt carries, as its MIME type and its bytes in base64.
    fn image(&self, name: &str) -> Option<(&'static str, String)>;
    fn turn_started(&self, text: &str, images: &[String]);
    fn turn_ended(&self, stop_reason: &str, usage: Value);
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
    let tools = agent.tools.clone();
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
                let (tool, outcome, allowed) = answer_permission(&request, &tools, provider);
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
    let images = initialized["agentCapabilities"]["promptCapabilities"]["image"] == true;
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

    events.offered(&options, opened == "new");
    // The user's choices, in this order, each checked: a model is never silently replaced.
    let mut options = options;
    let chosen = [
        ("model", "model", &agent.chosen.model),
        ("thought_level", "effort", &agent.chosen.effort),
        ("mode", "mode", &agent.chosen.mode),
    ];
    for (category, word, value) in chosen {
        let Some(value) = value else { continue };
        let option = find_option(&options, category)
            .ok_or_else(|| format!("the agent has no {word} option: it cannot take {value}"))?;
        let offered = providers::values(&option);
        if !offered.iter().any(|v| v == value) {
            return Err(format!(
                "the agent does not offer the {word} {value}: it offers {}",
                offered.join(", ")
            ));
        }
        let id = option["id"].as_str().unwrap_or_default().to_owned();
        let answer = cx
            .send_request(SetSessionConfigOptionRequest::new(
                native_id.clone(),
                id,
                SessionConfigOptionValue::value_id(value.clone()),
            ))
            .block_task()
            .await
            .map_err(|e| format!("the agent refused the {word} {value}: {e}"))?;
        options = serde_json::to_value(&answer).unwrap_or_default()["configOptions"].clone();
        let taken = find_option(&options, category).map(|o| o["currentValue"].clone());
        if taken.as_ref().and_then(Value::as_str) != Some(value) {
            return Err(format!(
                "the agent took the {word} {} when {value} was asked",
                taken.unwrap_or_default()
            ));
        }
    }
    if chosen.iter().any(|(_, _, value)| value.is_some()) {
        events.offered(&options, false);
    }
    // A value named `default` stands for the agent's recommended one, when it names it.
    let current = |category| {
        let option = find_option(&options, category)?;
        let value = option["currentValue"].as_str()?;
        let recommended = option["_meta"]["jetbrains"]["air"]["recommendedValue"].as_str();
        Some(match recommended {
            Some(recommended) if value == "default" => recommended.to_owned(),
            _ => value.to_owned(),
        })
    };
    events.opened(
        opened,
        &native_id,
        &agent_version,
        Choices {
            model: current("model"),
            effort: current("thought_level"),
            mode: current("mode"),
        },
        loads || resumes,
        images,
    );

    let mut turn: Option<Turn> = None;
    loop {
        if turn.is_none()
            && turns.queued.is_empty()
            && let Some(prompt) = events.pump()
        {
            turns.queued.push_back(prompt);
        }
        if turn.is_none()
            && let Some(prompt) = turns.queued.pop_front()
        {
            let text = prompt_text(prompt.text)?;
            events.turn_started(&text, &prompt.images);
            turns.running = true;
            let mut blocks = vec![ContentBlock::Text(TextContent::new(text))];
            blocks.extend(
                prompt
                    .images
                    .iter()
                    .filter_map(|name| events.image(name))
                    .map(|(mime, data)| ContentBlock::Image(ImageContent::new(data, mime))),
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
                            _ => Err(acp(error)),
                        };
                    }
                };
                turns.running = false;
                let answer = serde_json::to_value(&answer).unwrap_or_default();
                let stop = answer["stopReason"].as_str().unwrap_or("end_turn");
                events.turn_ended(stop, answer.get("usage").cloned().unwrap_or(Value::Null));
            }
            order = orders.recv() => match order {
                None | Some(Order::Stop) => return Ok(Exit::Stopped),
                Some(Order::Replace(reason)) => return Ok(Exit::Replace(reason)),
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
            json!({"used": update["used"], "size": update["size"], "cost": update["cost"]}),
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
