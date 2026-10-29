//! The Auto judge (`docs/design.md` §7): in Auto, a call to judge is put to a small model the user
//! chose, which answers `allow` or `ask`. It is an ACP agent Flottille runs in bare mode with no
//! tool at all, no MCP server and an empty folder: one prompt in, one answer out. Its prompt is
//! built only from the call as it is recorded (cut and masked), the place, the user's last
//! messages and the agent's `why`. Any failure asks.
//!
//! One process per judgement, in a fresh session: no judgement sees another.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, Implementation, InitializeRequest, NewSessionRequest, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionConfigOptionValue, SessionNotification, SetSessionConfigOptionRequest, TextContent,
};
use agent_client_protocol::{Agent as AcpAgent, Client, ConnectionTo, Lines};
use flottille_core::agent::{Agent, AgentState, Choices, GateMode};
use flottille_core::approval::Judgement;
use flottille_core::provider::{Judge, Settings};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};

use crate::VERSION;
use crate::daemon::Daemon;
use crate::mcp::{self, Grant};
use crate::providers;
use crate::tree;

/// The judge's built-in instructions.
pub const INSTRUCTIONS: &str = include_str!("../roles/judge.md");

/// How long a judgement may take before it is taken for a failure, which asks.
pub fn timeout() -> Duration {
    // Tests shorten it, so that a silent judge is seen failing without waiting half a minute.
    std::env::var("FLOTTILLE_JUDGE_TIMEOUT_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(30))
}

/// The lines of a file's content the judge sees.
const CONTENT_LINES: usize = 10;
/// The user's messages the judge sees, the last ones.
const MESSAGES: usize = 3;
/// Each of them, cut.
const MESSAGE_CHARS: usize = 2_000;

/// The judge the user chose, if any.
pub fn chosen(daemon: &Daemon) -> Option<Judge> {
    Settings::load(&daemon.paths).ok()?.judge
}

/// The mode of the agent's session.
pub fn mode(daemon: &Daemon, agent: &str) -> GateMode {
    daemon
        .read(|store| store.agent(agent))
        .ok()
        .flatten()
        .map(|agent| agent.gate_mode)
        // An agent that cannot be read is judged by nobody.
        .unwrap_or(GateMode::Ask)
}

/// The judgement of a call to judge: what the judge said, or why it could not decide (which
/// asks).
pub async fn judge(
    daemon: &Arc<Daemon>,
    judge: &Judge,
    grant: &Grant,
    tool: &str,
    args: &Value,
    reasons: &[String],
) -> Judgement {
    let prompt = prompt(daemon, grant, tool, args, reasons);
    let failed = |why: String| Judgement {
        by: judge.who(),
        verdict: "failed".into(),
        reason: format!("the judge could not decide: {why}"),
    };
    let answer = match tokio::time::timeout(timeout(), ask(daemon, judge, &prompt)).await {
        Err(_) => {
            return failed(format!(
                "it gave no answer within {} seconds",
                timeout().as_secs_f32()
            ));
        }
        Ok(Err(why)) => return failed(why),
        Ok(Ok(answer)) => answer,
    };
    match verdict(&answer) {
        Some((verdict, reason)) => Judgement {
            by: judge.who(),
            verdict,
            reason: mcp::masked(&reason, &grant.place),
        },
        None => failed("its answer was not the JSON verdict it is asked for".into()),
    }
}

/// The verdict and the reason of the judge's answer: one JSON object, `allow` or `ask`, possibly
/// in a code fence. Anything else is no verdict.
pub fn verdict(answer: &str) -> Option<(String, String)> {
    let text = answer.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .and_then(|inner| inner.strip_suffix("```"))
        .unwrap_or(text)
        .trim();
    let value: Value = serde_json::from_str(text).ok()?;
    let verdict = value["verdict"].as_str()?;
    if !["allow", "ask"].contains(&verdict) {
        return None;
    }
    let reason = value["reason"].as_str()?.trim();
    if reason.is_empty() {
        return None;
    }
    let reason: String = reason.chars().take(500).collect();
    Some((verdict.to_owned(), reason))
}

/// The judge's prompt: the call as it is recorded (every text cut and masked, a file's content
/// cut to its first lines), why the rules held it, the place, the user's last messages to the
/// session's root, and the agent's `why`. Nothing else: no file, no credential.
pub fn prompt(
    daemon: &Daemon,
    grant: &Grant,
    tool: &str,
    args: &Value,
    reasons: &[String],
) -> String {
    let mut shown = args.clone();
    for key in ["content", "edits"] {
        if let Some(value) = shown.get_mut(key) {
            *value = first_lines(value);
        }
    }
    let why = shown
        .as_object_mut()
        .and_then(|map| map.remove("why"))
        .and_then(|why| why.as_str().map(str::to_owned));
    let call = mcp::record_of(&shown, &grant.place);
    let place = &grant.place;
    let place_words = match &place.workspace {
        Some(name) => format!(
            "workspace {name} of project {} (the agent's own worktrees, branch {})",
            place.project, place.branch
        ),
        None => format!(
            "the project folder (main) of project {}: the user's own checkout",
            place.project
        ),
    };
    let mut text = format!(
        "Judge this call.\n\n## The call\n\nTool: {tool}\nArguments: {call}\n\n## Why the rules \
         held it\n\n"
    );
    for reason in reasons {
        text.push_str(&format!("- {}\n", mcp::masked(reason, place)));
    }
    text.push_str(&format!(
        "\n## The place\n\n{place_words}\nFolder: {}\n",
        place.root.display()
    ));
    text.push_str("\n## The user's last messages to the session\n\n");
    let messages = last_messages(daemon, &grant.agent);
    if messages.is_empty() {
        text.push_str("(none)\n");
    }
    for message in messages {
        text.push_str(&format!(
            "<message>\n{}\n</message>\n",
            mcp::masked(&message, place)
        ));
    }
    text.push_str("\n## The agent's why\n\n");
    match why {
        Some(why) => text.push_str(&mcp::masked(&mcp::cut(&why, MESSAGE_CHARS), place)),
        None => text.push_str("(none given)"),
    }
    text.push_str("\n\nAnswer with the JSON object only.");
    text
}

/// A file's content, or the edits of `fs_edit`, cut to their first lines.
fn first_lines(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            let lines: Vec<&str> = text.lines().collect();
            let mut kept = lines
                .iter()
                .take(CONTENT_LINES)
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            if lines.len() > CONTENT_LINES {
                kept.push_str(&format!("\n‹+{} lines›", lines.len() - CONTENT_LINES));
            }
            Value::String(kept)
        }
        Value::Array(items) => Value::Array(items.iter().map(first_lines).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), first_lines(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The user's last messages to the root of the agent's tree, oldest first: their intent.
fn last_messages(daemon: &Daemon, agent: &str) -> Vec<String> {
    let root = tree::root_of(daemon, agent);
    let mut messages: Vec<String> = daemon
        .read(|store| store.last_deliveries(&root, 32))
        .unwrap_or_default()
        .into_iter()
        .filter(|delivery| delivery.kind == "user")
        .take(MESSAGES)
        .map(|delivery| mcp::cut(&delivery.body, MESSAGE_CHARS))
        .collect();
    messages.reverse();
    messages
}

/// The judge's answer to one prompt: its provider's ACP agent started in bare mode with no tool,
/// in an empty folder, a new session with no MCP server, the model set, one prompt, and the text
/// it said.
async fn ask(daemon: &Daemon, judge: &Judge, prompt: &str) -> Result<String, String> {
    let provider = providers::get(&judge.provider)
        .ok_or_else(|| format!("there is no provider {}", judge.provider))?;
    let paths = daemon.paths.clone();
    tokio::task::spawn_blocking(move || providers::startable(&paths, provider))
        .await
        .map_err(|e| e.to_string())??;
    let folder = daemon.paths.state.join("judge");
    std::fs::create_dir_all(&folder).map_err(|e| format!("cannot make its folder: {e}"))?;
    let agent = as_agent(&folder);
    let launch = providers::launch(provider, &daemon.paths, &agent)?;
    let mut command = Command::new(&launch.program);
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(provider.withheld) {
            command.env_remove(name);
        }
    }
    let mut child = command
        .envs(launch.env.iter().map(|(name, value)| (name, value)))
        .args(&launch.args)
        .current_dir(&folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", launch.program.display()))?;
    let stdin = child.stdin.take().ok_or("the judge has no stdin")?;
    let stdout = child.stdout.take().ok_or("the judge has no stdout")?;
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
    let said = Arc::new(Mutex::new(String::new()));
    let saying = said.clone();
    let meta = (provider.session_meta)(INSTRUCTIONS, &[])
        .as_object()
        .cloned();
    let result = Client
        .builder()
        .name("flottille-judge")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let Ok(update) = serde_json::to_value(&notification.update)
                    && update["sessionUpdate"] == "agent_message_chunk"
                    && let Some(text) = update["content"]["text"].as_str()
                {
                    saying.lock().unwrap().push_str(text);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            // The judge has no tool: whatever it asks to run is refused.
            async move |_request: RequestPermissionRequest, responder, _cx| {
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async |cx: ConnectionTo<AcpAgent>| {
            Ok(session(cx, &folder, meta, judge.model.as_deref(), prompt).await)
        })
        .await;
    let _ = child.start_kill();
    match result {
        Ok(Ok(())) => Ok(said.lock().unwrap().clone()),
        Ok(Err(why)) => Err(why),
        Err(error) => Err(format!("the connection to the judge failed: {error}")),
    }
}

/// One session of the judge: open, set the model, play the prompt.
async fn session(
    cx: ConnectionTo<AcpAgent>,
    folder: &Path,
    meta: Option<serde_json::Map<String, Value>>,
    model: Option<&str>,
    prompt: &str,
) -> Result<(), String> {
    cx.send_request(
        InitializeRequest::new(ProtocolVersion::V1)
            .client_info(Implementation::new("Flottille", VERSION)),
    )
    .block_task()
    .await
    .map_err(|e| format!("it refused to start: {e}"))?;
    // No MCP server: the judge has nothing to call.
    let opened = cx
        .send_request(
            NewSessionRequest::new(folder)
                .mcp_servers(Vec::new())
                .meta(meta),
        )
        .block_task()
        .await
        .map_err(|e| format!("it could not open a session: {e}"))?;
    let opened = serde_json::to_value(&opened).unwrap_or_default();
    let session = opened["sessionId"]
        .as_str()
        .ok_or("it opened a session without an id")?
        .to_owned();
    if let Some(model) = model {
        let option = opened["configOptions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|option| option["category"] == "model")
            .ok_or_else(|| format!("it has no model option: it cannot take {model}"))?;
        if !providers::values(option).iter().any(|v| v == model) {
            return Err(format!("it does not offer the model {model}"));
        }
        let id = option["id"].as_str().unwrap_or_default().to_owned();
        cx.send_request(SetSessionConfigOptionRequest::new(
            session.clone(),
            id,
            SessionConfigOptionValue::value_id(model.to_owned()),
        ))
        .block_task()
        .await
        .map_err(|e| format!("it refused the model {model}: {e}"))?;
    }
    cx.send_request(PromptRequest::new(
        session,
        vec![ContentBlock::Text(TextContent::new(prompt.to_owned()))],
    ))
    .block_task()
    .await
    .map_err(|e| format!("its turn failed: {e}"))?;
    Ok(())
}

/// The judge as its provider's bare options take an agent: no tool, its instructions, its folder.
fn as_agent(folder: &Path) -> Agent {
    Agent {
        id: "judge".into(),
        provider: String::new(),
        role: "judge".into(),
        parent: None,
        depth: 0,
        epoch: 0,
        title: None,
        brief: None,
        reported_at: None,
        quiet_turns: 0,
        waiting_on_answer: false,
        project: String::new(),
        workspace: None,
        folder: folder.to_owned(),
        state: AgentState::Starting,
        state_reason: None,
        created_at: 0,
        updated_at: 0,
        active_at: None,
        ended_at: None,
        chosen: Choices::default(),
        taken: Choices::default(),
        native_id: None,
        agent_version: None,
        reopens: false,
        images: None,
        instructions: INSTRUCTIONS.into(),
        tools: Vec::new(),
        gate_mode: GateMode::Ask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_verdict_is_one_json_object_allow_or_ask_with_a_reason() {
        assert_eq!(
            verdict(r#"{"verdict": "allow", "reason": "the user asked for it"}"#),
            Some(("allow".into(), "the user asked for it".into()))
        );
        assert_eq!(
            verdict("```json\n{\"verdict\": \"ask\", \"reason\": \"it deletes files\"}\n```"),
            Some(("ask".into(), "it deletes files".into()))
        );
        for garbage in [
            "",
            "allow",
            r#"{"verdict": "deny", "reason": "no"}"#,
            r#"{"verdict": "allow"}"#,
            r#"{"verdict": "allow", "reason": "  "}"#,
            "Sure! {\"verdict\": \"allow\", \"reason\": \"x\"}",
        ] {
            assert_eq!(verdict(garbage), None, "{garbage}");
        }
    }

    #[test]
    fn a_file_s_content_is_cut_to_its_first_lines() {
        let content: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        let cut = first_lines(&json!(content));
        let cut = cut.as_str().unwrap();
        assert!(cut.starts_with("line 1\n") && cut.contains("line 10"));
        assert!(
            !cut.contains("line 11") && cut.ends_with("‹+20 lines›"),
            "{cut}"
        );
    }
}
