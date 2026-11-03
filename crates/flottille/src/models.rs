//! Reading a provider's models as soon as it is ready (`docs/agents.md`, "The models"): its ACP
//! agent started bare as the judge's is, in an empty folder, a new session with no MCP server, the
//! options it offers kept, and no prompt: each program lists its models before any request to a
//! model, and the session is not asked for anything else. It runs in the background, once for
//! each state of the provider (its program, its API keys, its sign-in file), with a time limit,
//! and what goes wrong is said in the developer log and changes nothing.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    Implementation, InitializeRequest, NewSessionRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse,
};
use agent_client_protocol::{Agent as AcpAgent, Client, ConnectionTo, Lines};
use flottille_core::agent::{Agent, AgentState, Choices, GateMode};
use flottille_core::provider::Settings;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};

use crate::VERSION;
use crate::daemon::Daemon;
use crate::providers::{self, Check, Provider, Version};

/// A program that has not listed its models by then is let go: the listing never holds anything.
const LIMIT: Duration = Duration::from_secs(60);

/// The state of each provider a listing was made for, under which it is not made again: its
/// program, its API keys and whether its sign-in file is there. A failed listing is asked again
/// only when one of them changes, or the provider is enabled again.
static TRIED: Mutex<Option<HashMap<&'static str, String>>> = Mutex::new(None);
/// The providers a listing is running for.
static RUNNING: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// A test's daemon, one with any `FLOTTILLE_TEST_` variable set, lists the models only where the
/// test asks for it: the fakes of the other tests see no start of their own, and the programs of
/// the machine on a test's `PATH` are never started.
fn silenced() -> bool {
    std::env::var_os("FLOTTILLE_TEST_LIST_MODELS").is_none()
        && std::env::vars_os().any(|(name, _)| {
            name.to_str()
                .is_some_and(|name| name.starts_with("FLOTTILLE_TEST_"))
        })
}

/// The provider is enabled again: its models are asked for once more, even from a state a
/// listing already failed in.
pub fn enabled(daemon: &Arc<Daemon>, provider: &'static Provider) {
    if let Some(tried) = TRIED.lock().unwrap().as_mut() {
        tried.remove(provider.name);
    }
    check(daemon);
}

/// Starts the listing of every enabled provider whose models are not read from the program and
/// the keys there are now. Looks at files only, so that the sweep can call it: the listing itself
/// runs in the background.
pub fn check(daemon: &Arc<Daemon>) {
    if silenced() {
        return;
    }
    let Ok(settings) = Settings::load(&daemon.paths) else {
        return;
    };
    for provider in providers::PROVIDERS {
        let check = Check::of(provider, &settings, Version::Skip);
        if !check.enabled || (check.program.is_none() && check.overridden.is_none()) {
            continue;
        }
        if providers::last_models(daemon, &check).is_some_and(|(_, current)| current) {
            continue;
        }
        let state = format!(
            "{:?}; sign-in file {}",
            providers::models_version(&check),
            check.login_present
        );
        {
            let mut tried = TRIED.lock().unwrap();
            let tried = tried.get_or_insert_with(HashMap::new);
            if tried.get(provider.name) == Some(&state) {
                continue;
            }
            let mut running = RUNNING.lock().unwrap();
            if !running
                .get_or_insert_with(HashSet::new)
                .insert(provider.name)
            {
                continue;
            }
            tried.insert(provider.name, state);
        }
        let daemon = daemon.clone();
        tokio::spawn(async move {
            let outcome = tokio::time::timeout(LIMIT, read(&daemon, provider))
                .await
                .unwrap_or_else(|_| Err(format!("it listed nothing in {} s", LIMIT.as_secs())));
            if let Some(running) = RUNNING.lock().unwrap().as_mut() {
                running.remove(provider.name);
            }
            report(&daemon, provider, outcome);
        });
    }
}

fn report(daemon: &Daemon, provider: &'static Provider, outcome: Result<usize, String>) {
    let fields = match &outcome {
        Ok(count) => {
            json!({"agent": null, "provider": provider.name, "outcome": "read", "models": count})
        }
        Err(error) => {
            json!({"agent": null, "provider": provider.name, "outcome": "failed", "error": error})
        }
    };
    daemon.developer.write("models", fields, &[]);
    if outcome.is_ok() {
        // The window reads the providers again at any `provider.` event.
        let _ = daemon.emit("provider.models_read", json!({"name": provider.name}));
    }
}

/// The models the provider's agent offers to a session that is given nothing to do, kept as a
/// session's are; how many.
async fn read(daemon: &Arc<Daemon>, provider: &'static Provider) -> Result<usize, String> {
    let paths = daemon.paths.clone();
    tokio::task::spawn_blocking(move || providers::startable(&paths, provider))
        .await
        .map_err(|e| e.to_string())??;
    let folder = daemon.paths.state.join("models");
    std::fs::create_dir_all(&folder).map_err(|e| format!("cannot make its folder: {e}"))?;
    let agent = as_agent(provider, &folder);
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
    let _group = child.id().map(Group);
    let stdin = child.stdin.take().ok_or("it has no stdin")?;
    let stdout = child.stdout.take().ok_or("it has no stdout")?;
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
    let meta = (provider.session_meta)("", &[]).as_object().cloned();
    let opened = Client
        .builder()
        .name("flottille-models")
        .on_receive_request(
            // Nothing is asked of it, so it has nothing to run.
            async move |_request: RequestPermissionRequest, responder, _cx| {
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, async |cx: ConnectionTo<AcpAgent>| {
            Ok(session(cx, &folder, meta).await)
        })
        .await;
    let _ = child.start_kill();
    let (session, options) = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(why)) => return Err(why),
        Err(error) => return Err(format!("the connection failed: {error}")),
    };
    (provider.forget_session)(&daemon.paths, &session);
    (provider.forget_agent)(&daemon.paths, &agent.id);
    let count = options
        .as_array()
        .into_iter()
        .flatten()
        .find(|option| option["category"] == "model")
        .map(|model| providers::values(model).len())
        .filter(|&count| count > 0)
        .ok_or("it offers no model")?;
    providers::learn(daemon, provider, &options, true, true);
    Ok(count)
}

/// One session, no prompt: its id and the options it offers.
async fn session(
    cx: ConnectionTo<AcpAgent>,
    folder: &Path,
    meta: Option<serde_json::Map<String, Value>>,
) -> Result<(String, Value), String> {
    cx.send_request(
        InitializeRequest::new(ProtocolVersion::V1)
            .client_info(Implementation::new("Flottille", VERSION)),
    )
    .block_task()
    .await
    .map_err(|e| format!("it refused to start: {e}"))?;
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
    Ok((session, opened["configOptions"].clone()))
}

/// The process group, killed whole once the options are read or the time is up.
struct Group(u32);

impl Drop for Group {
    fn drop(&mut self) {
        crate::agents::signal_group(self.0, libc::SIGKILL);
    }
}

/// The listing as its provider's bare options take an agent: no tool, no instructions.
fn as_agent(provider: &Provider, folder: &Path) -> Agent {
    Agent {
        id: format!("models-{}", provider.name),
        provider: provider.name.into(),
        role: "models".into(),
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
        instructions: String::new(),
        tools: Vec::new(),
        gate_mode: GateMode::Ask,
        launched_by: None,
    }
}
