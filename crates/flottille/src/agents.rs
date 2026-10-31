//! The `agent.*` methods (`docs/protocol.md`) and the life of each agent (`docs/agents.md`).
//!
//! An agent is owned by one task of its own, which starts its process (its provider's ACP agent) when a
//! delivery comes, keeps it while it works, lets it go after the idle release time, notices when it dies
//! and replaces its session then. The methods only write the store and hand the task its orders:
//! none of them waits on the agent. Every prompt an agent gets is a delivery (`deliveries`).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use flottille_core::agent::{Agent, AgentState, Choices, GateMode, new_id};
use flottille_core::approval;
use flottille_core::paths::ensure_private_dir;
use flottille_core::project::Project;
use flottille_core::protocol::{
    AgentImage, AgentName, AgentRun, AgentSend, AgentTrees, ImageUpload,
};
use flottille_core::secrets::{self, Secrets};
use flottille_core::store::{AgentChange, Store, now_ms};
use flottille_core::workspace::State;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::acp::{self, Exit};
use crate::daemon::Daemon;
use crate::deliveries;
use crate::mcp;
use crate::projects::{self, Failure};
use crate::providers::{self, Provider};
use crate::roles::{self, Role};
use crate::tree;

type Answer = Result<Value, Failure>;

/// A process idle this long is let go (`docs/reference/hemera.md` §1); the next prompt starts it
/// again. `FLOTTILLE_IDLE_RELEASE_MS` changes it, for tests and development.
const IDLE_RELEASE: Duration = Duration::from_secs(5 * 60);
/// How long a process asked to stop has before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// The events `agent.show` gives at once.
const PAGE: usize = 1000;

/// `agent.unqueue`'s and `agent.send_now`'s parameters: a message that waits for the root.
/// `images` asks for its pictures back.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentUnqueue {
    agent: String,
    delivery: String,
    #[serde(default)]
    images: bool,
}

/// `agent.show`'s parameters: a page of the agent's events after `after`, or the last before
/// `before`.
#[derive(serde::Deserialize)]
struct AgentShow {
    agent: String,
    #[serde(default)]
    after: Option<u64>,
    #[serde(default)]
    before: Option<u64>,
}

/// A depth an agent may have, at most: a root is at 0, and a child, at 1, starts none
/// (`docs/design.md` §5.1).
pub const DEPTH_LIMIT: u32 = 1;
/// Replacements an agent may have in this time before it fails instead (`docs/design.md` §5.5).
const REPLACEMENTS_MAX: usize = 2;
const REPLACEMENT_WINDOW_MS: u64 = 30 * 60 * 1000;

/// What an agent's task is asked.
pub enum Order {
    /// Something was delivered: pump it when no turn runs.
    Ring,
    /// Answered whether a turn was running.
    Cancel(oneshot::Sender<bool>),
    Stop,
    /// Its session is let go and a new one replaces it, for this reason. The parent was told.
    Replace(String),
}

/// The prompt taken for the agent, kept across its processes, and whether one is being played.
pub struct Turns {
    pub queued: VecDeque<deliveries::Prompt>,
    pub running: bool,
}

/// An agent's task, and where its orders go.
type Task = (mpsc::UnboundedSender<Order>, JoinHandle<()>);

/// The tasks of the agents that have one, by agent id.
#[derive(Default)]
pub struct Live {
    tasks: Mutex<HashMap<String, Task>>,
    /// The processes this daemon started that have not ended, with their start times: the only
    /// ones it kills when it stops, whatever the shared table holds.
    processes: Mutex<HashMap<u32, u64>>,
}

/// Answers an `agent.*` request.
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Answer {
    let daemon = &daemon;
    match method {
        // `agent.run` is the name it had before the pilot door; both stay.
        "agent.start" | "agent.run" => start(daemon, serde_json::from_value(params)?),
        "agent.send" => {
            let asked: AgentSend = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            refuse_child(daemon, &agent, "writes to it")?;
            refuse_final(&agent)?;
            if asked.images.is_empty() {
                refuse_empty(&asked.message)?;
            }
            let after = daemon.read(Store::last_seq)?;
            // `/name` sends the project's command of that name. Its images are kept only once
            // it can be sent: a command whose file cannot be read leaves none behind.
            let message = crate::commands::message_for(
                daemon,
                &agent.project,
                agent.workspace.as_deref(),
                &asked.message,
            )?;
            let images = keep_images(daemon, &agent, &asked.images)?;
            let delivery = deliveries::deliver_with_images(
                daemon,
                &agent.id,
                "user",
                None,
                &message,
                approval::BETWEEN_TURNS,
                images.clone(),
            )
            .ok_or_else(|| {
                deliveries::discard_images(&daemon.paths, &agent.id, &images);
                Failure::new("unavailable", "the message could not be stored")
            })?;
            Ok(json!({"agent": agent.to_json(), "after": after, "delivery": delivery}))
        }
        "agent.unqueue" => {
            let asked: AgentUnqueue = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            refuse_child(daemon, &agent, "takes back a message")?;
            deliveries::withdraw(daemon, &agent.id, &asked.delivery, asked.images)
        }
        "agent.send_now" => {
            let asked: AgentUnqueue = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            refuse_child(daemon, &agent, "sends a message now")?;
            deliveries::send_now(daemon, &agent.id, &asked.delivery)
        }
        "agent.image" => {
            let asked: AgentImage = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            let (mime, data) = deliveries::read_image(daemon, &agent.id, &asked.name)
                .ok_or_else(|| Failure::new("not-found", format!("no image {}", asked.name)))?;
            Ok(json!({"mime": mime, "data": data}))
        }
        "agent.stop" => {
            let asked: AgentName = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            refuse_child(daemon, &agent, "stops it")?;
            refuse_final(&agent)?;
            tree::stop_tree(daemon, &agent.id, "stopped by the user");
            let agent = find(daemon, &agent.id)?;
            Ok(json!({"agent": agent.to_json()}))
        }
        "agent.resume" => {
            let asked: AgentName = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            refuse_child(daemon, &agent, "resumes it")?;
            if !agent.state.is_final() {
                return Err(Failure::refused(format!(
                    "agent {} has not ended: it is {}",
                    agent.id,
                    agent.state.as_str()
                )));
            }
            if let Some(why) = tree::resume_refusal(daemon, &agent) {
                return Err(Failure::refused(format!(
                    "agent {} cannot resume: {why}",
                    agent.id
                )));
            }
            // Its last process would revoke the new one's token as it goes.
            if stopping(daemon, &agent.id) {
                return Err(Failure::refused(format!(
                    "agent {} is still stopping, try again in a moment",
                    agent.id
                )));
            }
            let ended_at = agent.ended_at.unwrap_or(agent.updated_at);
            let resumed = daemon.write(|store| match store.resume_agent(&agent.id)? {
                Some((agent, event)) => Ok((Some(agent), Some(event))),
                None => Ok((None, None)),
            })?;
            let agent = resumed.ok_or_else(|| {
                Failure::refused(format!("agent {} was resumed already", agent.id))
            })?;
            tell_resumed(daemon, &agent, ended_at);
            Ok(json!({"agent": agent.to_json()}))
        }
        "agent.mode" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Mode {
                agent: String,
                mode: String,
            }
            let asked: Mode = serde_json::from_value(params)?;
            let mode = GateMode::parse(&asked.mode).ok_or_else(|| {
                Failure::new(
                    "invalid-params",
                    format!(
                        "there is no mode {}: the modes are ask, auto and full",
                        asked.mode
                    ),
                )
            })?;
            // A tree has one mode, its root's.
            let agent = find(daemon, &tree::root_of(daemon, &asked.agent))?;
            // Only the user's door reaches this method: no tool of an agent changes a mode.
            daemon.write(|store| {
                let (_, event) = store.change_agent(
                    &agent.id,
                    flottille_core::store::AgentChange {
                        gate_mode: Some(mode),
                        ..Default::default()
                    },
                    Some((
                        "agent.gate_mode_changed",
                        json!({"agent": agent.id, "mode": mode.as_str()}),
                    )),
                )?;
                Ok(((), event))
            })?;
            let agent = find(daemon, &agent.id)?;
            Ok(json!({"agent": agent.to_json()}))
        }
        "agent.tree" => {
            let asked: AgentName = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            Ok(json!({"agents": tree::tree(daemon, &agent)?}))
        }
        "agent.trees" => {
            let asked: AgentTrees = match params {
                Value::Null => AgentTrees::default(),
                params => serde_json::from_value(params)?,
            };
            Ok(tree::trees(daemon, &asked)?)
        }
        "agent.list" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct List {
                #[serde(default)]
                project: Option<String>,
            }
            let List { project } = match params {
                Value::Null => List { project: None },
                params => serde_json::from_value(params)?,
            };
            let agents = daemon.read(Store::agents)?;
            Ok(json!({"agents": agents
                .iter()
                .filter(|a| project.as_ref().is_none_or(|p| &a.project == p))
                .map(|agent| tree::summary(daemon, agent))
                .collect::<Vec<_>>()}))
        }
        "agent.read" => {
            let asked: AgentName = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            let page = tree::conversation(daemon, &agent, asked.after.unwrap_or(0))?;
            Ok(
                json!({"agent": agent.id, "name": tree::named(&agent), "lines": page.lines,
                "after": page.after, "more": page.more}),
            )
        }
        "agent.status" => {
            let asked: AgentName = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            Ok(tree::status_of(daemon, &agent)?)
        }
        "agent.cancel" => {
            let asked: AgentName = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            refuse_child(daemon, &agent, "cancels its turn")?;
            let (answer, answered) = oneshot::channel();
            let task = daemon
                .agents
                .tasks
                .lock()
                .unwrap()
                .get(&agent.id)
                .map(|(sender, _)| sender.clone());
            let sent = task.is_some_and(|task| task.send(Order::Cancel(answer)).is_ok());
            // Answered by the task at once: it never waits on the agent to do so.
            let cancelled = sent && answered.await.unwrap_or(false);
            Ok(json!({"agent": agent.to_json(), "cancelled": cancelled}))
        }
        "agent.show" => {
            let asked: AgentShow = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            // One more than a page is read, to say whether more remain.
            let mut events = daemon.read(|store| match asked.before {
                Some(before) => store.agent_events_before(&agent.id, before, PAGE + 1),
                None => store.agent_events(&agent.id, asked.after.unwrap_or(0), PAGE + 1),
            })?;
            let more = events.len() > PAGE;
            if more && asked.before.is_some() {
                events.remove(0);
            }
            events.truncate(PAGE);
            Ok(json!({"agent": agent.to_json(), "events": events, "more": more}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

fn find(daemon: &Daemon, id: &str) -> Result<Agent, Failure> {
    let mut agent = daemon
        .read(|store| store.agent(id))?
        .ok_or_else(|| Failure::new("not-found", format!("there is no agent {id}")))?;
    // A child's mode is its root's: what it stored is not used.
    if agent.parent.is_some() {
        agent.gate_mode = crate::judge::mode(daemon, id);
    }
    Ok(agent)
}

/// A child belongs to its parent: only its parent writes to it, and it stops with its root.
fn refuse_child(daemon: &Daemon, agent: &Agent, does: &str) -> Result<(), Failure> {
    let Some(parent) = &agent.parent else {
        return Ok(());
    };
    let root = tree::root_of(daemon, &agent.id);
    Err(Failure::refused(format!(
        "agent {} is a child of agent {parent}: only its parent {does}; the user writes to and \
         stops its root, agent {root}",
        agent.id
    )))
}

fn refuse_final(agent: &Agent) -> Result<(), Failure> {
    if agent.state.is_final() {
        return Err(Failure::refused(format!(
            "agent {} is {}{}: it takes no more messages",
            agent.id,
            agent.state.as_str(),
            agent
                .state_reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        )));
    }
    Ok(())
}

use deliveries::{MAX_IMAGE_BYTES, MAX_IMAGES};

/// The images of a message, checked and written to the agent's folder of images in the data
/// folder (never the project's): the names of the files.
fn keep_images(
    daemon: &Daemon,
    agent: &Agent,
    images: &[ImageUpload],
) -> Result<Vec<String>, Failure> {
    use base64::Engine as _;
    if images.is_empty() {
        return Ok(Vec::new());
    }
    if agent.images == Some(false) {
        return Err(Failure::refused(format!(
            "the agent of {} takes no images",
            agent.id
        )));
    }
    if images.len() > MAX_IMAGES {
        return Err(Failure::refused(format!(
            "a message carries {MAX_IMAGES} images at most"
        )));
    }
    let mut checked = Vec::new();
    for image in images {
        let (_, extension) = deliveries::IMAGE_TYPES
            .iter()
            .find(|(mime, _)| *mime == image.mime)
            .ok_or_else(|| {
                Failure::refused(format!(
                    "an image of type {} is not taken: png, jpeg, gif and webp are",
                    image.mime
                ))
            })?;
        // Judged before it is decoded: a huge request is not turned into bytes to be refused.
        if image.data.len() > MAX_IMAGE_BYTES * 4 / 3 + 4 {
            return Err(Failure::refused(format!(
                "an image is empty or over {} MiB",
                MAX_IMAGE_BYTES / 1024 / 1024
            )));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|_| Failure::refused("an image is not valid base64"))?;
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            return Err(Failure::refused(format!(
                "an image is empty or over {} MiB",
                MAX_IMAGE_BYTES / 1024 / 1024
            )));
        }
        checked.push((extension, bytes));
    }
    let folder = daemon.paths.images(&agent.id);
    ensure_private_dir(&folder).map_err(|e| Failure::new("unavailable", e.to_string()))?;
    // Without a stamp two messages could share their names: none is better than a collision.
    let stamp = new_id().map_err(|e| Failure::new("unavailable", format!("no stamp: {e}")))?;
    let mut names = Vec::new();
    for (at, (extension, bytes)) in checked.into_iter().enumerate() {
        let name = format!("{stamp}-{at}.{extension}");
        if let Err(error) = std::fs::write(folder.join(&name), bytes) {
            deliveries::discard_images(&daemon.paths, &agent.id, &names);
            return Err(Failure::new(
                "unavailable",
                format!("an image cannot be stored: {error}"),
            ));
        }
        names.push(name);
    }
    Ok(names)
}

fn refuse_empty(message: &str) -> Result<(), Failure> {
    if message.trim().is_empty() {
        return Err(Failure::refused("the message is empty"));
    }
    Ok(())
}

fn start(daemon: &Arc<Daemon>, asked: AgentRun) -> Answer {
    let project = projects::load_project(daemon, &asked.project)?;
    refuse_empty(&asked.message)?;
    // An agent's own permission modes decide which of its calls run unasked: in bare mode every
    // call goes through Flottille's gate, which no mode may loosen.
    if let Some(mode) = &asked.mode {
        return Err(Failure::refused(format!(
            "an agent takes no mode ({mode} was asked): Flottille's gate decides every tool call, \
             and a permission mode of the agent's could only loosen it"
        )));
    }
    let role = roles::role(asked.role.as_deref().unwrap_or(roles::LEAD)).ok_or_else(|| {
        Failure::refused(format!(
            "there is no role {}: the roles are {}",
            asked.role.as_deref().unwrap_or_default(),
            roles::ROLES.map(|role| role.name).join(", ")
        ))
    })?;
    let after = daemon.read(Store::last_seq)?;
    // `/name` starts the session with the project's command of that name.
    let message = crate::commands::message_for(
        daemon,
        &project.name,
        asked.workspace.as_deref(),
        &asked.message,
    )?;
    let agent = create(
        daemon,
        &project,
        New {
            workspace: asked.workspace,
            preparing: false,
            role,
            parent: None,
            title: None,
            brief: message.clone(),
            provider: asked.provider,
            model: asked.model,
            effort: asked.effort,
            gate_mode: None,
            launched_by: None,
        },
    )?;
    // The root's brief is its first message: a replacement supersedes it by its id, as any
    // brief, and delivers the brief again.
    deliveries::deliver(
        daemon,
        &agent.id,
        "user",
        Some(format!("brief-{}-0", agent.id)),
        &message,
        approval::BETWEEN_TURNS,
    );
    Ok(json!({"agent": agent.to_json(), "after": after}))
}

/// What a new agent is made of.
pub struct New {
    pub workspace: Option<String>,
    /// Its workspace may still be preparing: the agent is sent nothing until it is ready.
    pub preparing: bool,
    pub role: &'static Role,
    /// Its parent, with the parent's depth; `None` for a root.
    pub parent: Option<(String, u32)>,
    pub title: Option<String>,
    pub brief: String,
    /// Its provider, model and effort as asked: each checked, and the defaults taken for those
    /// left out (`providers::choose`).
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Ask or Auto, never Full: a child's is its parent's; `None` for a new root, which starts in
    /// Auto.
    pub gate_mode: Option<GateMode>,
    /// The agent that starts this root for the user; `None` for a child or the user's own root.
    pub launched_by: Option<String>,
}

/// Writes a new agent, its instructions and tools frozen in it; nothing is sent to it yet.
pub fn create(daemon: &Arc<Daemon>, project: &Project, new: New) -> Result<Agent, Failure> {
    let (place, folder, repositories) = match &new.workspace {
        None => {
            let repositories: Vec<_> = project
                .repositories
                .iter()
                .map(|r| {
                    let folder = match r.path.as_str() {
                        "." => project.folder.clone(),
                        path => project.folder.join(path),
                    };
                    (r.path.clone(), folder.clone(), folder)
                })
                .collect();
            ("main".to_owned(), project.folder.clone(), repositories)
        }
        Some(name) => {
            let workspace = daemon
                .read(|store| store.workspace(&project.name, name))?
                .ok_or_else(|| {
                    Failure::new(
                        "not-found",
                        format!("project {} has no workspace named {name}", project.name),
                    )
                })?;
            if workspace.state() != State::Ready
                && !(new.preparing && workspace.state() == State::Preparing)
            {
                return Err(Failure::refused(format!(
                    "workspace {name} is not ready ({}): see `flottille workspace show {} {name}`",
                    serde_json::to_value(workspace.state())
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                    project.name
                )));
            }
            let repositories: Vec<_> = workspace
                .repositories
                .iter()
                .map(|r| {
                    let original = project.folder.join(&r.path);
                    (r.path.clone(), workspace.folder_of(&r.path), original)
                })
                .collect();
            (format!("workspace {name}"), workspace.folder, repositories)
        }
    };
    // Refused now rather than failed later: the user can act on it before anything is written.
    let (provider, chosen) = providers::choose(
        daemon,
        project,
        providers::Asked {
            provider: new.provider,
            model: new.model,
            effort: new.effort,
        },
    )?;
    let now = now_ms();
    let depth = new.parent.as_ref().map_or(0, |(_, depth)| depth + 1);
    let at_depth_limit = depth >= DEPTH_LIMIT;
    let forge = project.repositories.iter().any(|r| r.forge.is_some());
    let tools = provider.tools(
        new.parent.is_none(),
        at_depth_limit,
        forge,
        project.tracker.is_some(),
    );
    let files = roles::project_files(&project.folder, &repositories);
    let skills: Vec<(String, String)> = project
        .skills
        .iter()
        .map(|(name, skill)| (name.clone(), skill.description.clone()))
        .collect();
    let rules = crate::rules::effective(&daemon.paths, project);
    let instructions = roles::instructions(
        new.role,
        &roles::Session {
            project: &project.name,
            place: &place,
            folder: &folder,
            title: new.title.as_deref(),
            at_depth_limit,
            skills: &skills,
            rules: &rules,
        },
        &files,
    );
    let agent = Agent {
        id: new_id().map_err(|e| Failure::new("unavailable", e.to_string()))?,
        provider: provider.name.into(),
        role: new.role.name.into(),
        project: project.name.clone(),
        workspace: new.workspace.clone(),
        instructions,
        folder,
        state: AgentState::Starting,
        state_reason: None,
        created_at: now,
        updated_at: now,
        active_at: None,
        ended_at: None,
        chosen,
        taken: Choices::default(),
        native_id: None,
        agent_version: None,
        tools,
        parent: new.parent.map(|(parent, _)| parent),
        depth,
        epoch: 0,
        title: new.title,
        brief: Some(new.brief),
        reported_at: None,
        quiet_turns: 0,
        waiting_on_answer: false,
        reopens: false,
        images: None,
        gate_mode: new.gate_mode.unwrap_or_default(),
        launched_by: new.launched_by,
    };
    daemon.write(|store| {
        let event = store.create_agent(
            &agent,
            (
                "agent.created",
                json!({"agent": agent.id, "project": agent.project, "workspace": agent.workspace,
                    "folder": agent.folder, "provider": agent.provider, "model": agent.chosen.model,
                    "role": agent.role, "parent": agent.parent, "depth": agent.depth,
                    "title": agent.title}),
            ),
        )?;
        Ok(((), event))
    })?;
    Ok(agent)
}

/// Asks the agent's turn to stop, when one runs.
pub fn cancel(daemon: &Daemon, id: &str) {
    let task = daemon
        .agents
        .tasks
        .lock()
        .unwrap()
        .get(id)
        .map(|(sender, _)| sender.clone());
    if let Some(task) = task {
        let (answer, _) = oneshot::channel();
        let _ = task.send(Order::Cancel(answer));
    }
}

/// Hands an order to the agent's task, if it has one: an agent without a task has no process.
pub fn tell(daemon: &Daemon, id: &str, order: Order) {
    let task = daemon
        .agents
        .tasks
        .lock()
        .unwrap()
        .get(id)
        .map(|(sender, _)| sender.clone());
    if let Some(task) = task {
        let _ = task.send(order);
    }
}

/// Whether the agent's last process is still going: its task has not let its orders go.
pub fn stopping(daemon: &Daemon, id: &str) -> bool {
    daemon
        .agents
        .tasks
        .lock()
        .unwrap()
        .get(id)
        .is_some_and(|(sender, _)| !sender.is_closed())
}

/// What a resumed root reads first in its next prompt, without being woken for it: that the user
/// stopped it, and which of its requests were cancelled then, never to be executed.
fn tell_resumed(daemon: &Arc<Daemon>, agent: &Agent, ended_at: u64) {
    let cancelled = daemon
        .read(|store| store.cancelled_requests_of(&agent.id, ended_at))
        .unwrap_or_default();
    let mut body = "The user stopped you, then resumed you on this session: what you were doing \
                    was cut. Check the real state of your place before you go on."
        .to_owned();
    if !cancelled.is_empty() {
        let named: Vec<String> = cancelled
            .iter()
            .map(|request| format!("#{} ({})", request.number, request.tool))
            .collect();
        body.push_str(&format!(
            " Cancelled when you stopped, never executed: {}.",
            named.join(", ")
        ));
    }
    deliveries::queue(
        daemon,
        &agent.id,
        "resume",
        Some(format!("resumed-{}-{ended_at}", agent.id)),
        &body,
    );
}

/// Hands an order to the agent's task, starting the task when it has none.
pub fn order(daemon: &Arc<Daemon>, id: &str, order: Order) {
    let mut tasks = daemon.agents.tasks.lock().unwrap();
    let sender = match tasks.get(id) {
        Some((sender, _)) if !sender.is_closed() => sender.clone(),
        _ => {
            let (sender, orders) = mpsc::unbounded_channel();
            let task = tokio::spawn(agent_task(daemon.clone(), id.to_owned(), orders));
            tasks.insert(id.to_owned(), (sender.clone(), task));
            sender
        }
    };
    let _ = sender.send(order);
}

/// Stops every agent's process, the daemon stopping: each is let go as an idle one is. A task that
/// has not answered in time (stuck while its agent opens a session, say) is left no process: its
/// group is killed, so that the daemon goes with nothing of its own running.
pub async fn stop_all(daemon: &Daemon) {
    let tasks: Vec<_> = daemon.agents.tasks.lock().unwrap().drain().collect();
    let mut stuck = Vec::new();
    for (_, (sender, mut task)) in tasks {
        let _ = sender.send(Order::Stop);
        if tokio::time::timeout(STOP_GRACE * 2, &mut task)
            .await
            .is_err()
        {
            stuck.push(task);
        }
    }
    if !stuck.is_empty() {
        // The processes of the tasks that finished are gone: those left are the stuck ones',
        // killed while their leader is alive (the task is aborted after, which kills it too).
        let left: Vec<_> = daemon.agents.processes.lock().unwrap().drain().collect();
        let _ = daemon.read(|store| {
            for (pid, started) in left {
                kill_left(store, pid, started);
            }
            Ok(())
        });
        for task in stuck {
            task.abort();
        }
    }
}

fn idle_release() -> Duration {
    std::env::var("FLOTTILLE_IDLE_RELEASE_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(IDLE_RELEASE, Duration::from_millis)
}

/// The agent's task: its life, then, once it ended, its session's history goes, unless it is a
/// root that can be resumed on it.
async fn agent_task(daemon: Arc<Daemon>, id: String, orders: mpsc::UnboundedReceiver<Order>) {
    agent_life(daemon.clone(), id.clone(), orders).await;
    let Ok(Some(agent)) = daemon.read(|store| store.agent(&id)) else {
        return;
    };
    if !agent.state.is_final() {
        return;
    }
    if tree::keeps_session(&agent) {
        // Said once its process is gone: it can be resumed now (`stopping`).
        record(
            &daemon,
            &id,
            "agent.released",
            json!({"reason": "stopped"}),
            false,
        );
        return;
    }
    if let Ok(provider) = providers::of(&agent) {
        if let Some(native) = &agent.native_id {
            (provider.forget_session)(&daemon.paths, native);
        }
        (provider.forget_agent)(&daemon.paths, &agent.id);
    }
    deliveries::forget_images(&daemon.paths, &id);
}

/// Forgets the sessions of the ended roots kept longer than `tree::KEPT_MS`: they are resumed no
/// more, and what their tools printed goes with them.
pub fn forget_kept_sessions(daemon: &Daemon) {
    let now = now_ms();
    for agent in daemon.read(Store::agents).unwrap_or_default() {
        let Some(ended_at) = agent.ended_at else {
            continue;
        };
        if !tree::keeps_session(&agent)
            || now.saturating_sub(ended_at) < tree::KEPT_MS
            || stopping(daemon, &agent.id)
        {
            continue;
        }
        // Written first, so that the next sweep does not find it again.
        let forgotten = daemon.write(|store| {
            let change = AgentChange {
                reopens: Some(false),
                ..AgentChange::default()
            };
            let (_, event) = store.change_agent(&agent.id, change, None)?;
            Ok(((), event))
        });
        let (Ok(()), Ok(provider), Some(native)) =
            (forgotten, providers::of(&agent), agent.native_id.clone())
        else {
            continue;
        };
        let paths = daemon.paths.clone();
        tokio::task::spawn_blocking(move || {
            (provider.forget_session)(&paths, &native);
            (provider.forget_agent)(&paths, &agent.id);
            deliveries::forget_images(&paths, &agent.id);
        });
    }
}

/// The life of one agent: a process per stretch of work, until it stops or fails.
async fn agent_life(daemon: Arc<Daemon>, id: String, mut orders: mpsc::UnboundedReceiver<Order>) {
    let mut turns = Turns {
        queued: VecDeque::new(),
        running: false,
    };
    loop {
        if turns.queued.is_empty() {
            match deliveries::pump(&daemon, &id) {
                Some(prompt) => turns.queued.push_back(prompt),
                None => match orders.recv().await {
                    None | Some(Order::Stop) => return,
                    Some(Order::Cancel(answer)) => {
                        let _ = answer.send(false);
                        continue;
                    }
                    Some(Order::Ring) => continue,
                    Some(Order::Replace(reason)) => {
                        if !replace(&daemon, &id, &reason, true, false) {
                            return;
                        }
                        continue;
                    }
                },
            }
        }
        let Ok(Some(agent)) = daemon.read(|store| store.agent(&id)) else {
            return;
        };
        if agent.state.is_final() {
            return;
        }
        match process(&daemon, &agent, &mut orders, &mut turns).await {
            Ok(true) => continue,
            Ok(false) => return,
            Err(reason) => {
                fail(&daemon, &id, &reason);
                return;
            }
        }
    }
}

/// One process of the agent, from its start to its end: whether the agent goes on (released) or
/// not (stopped, dead).
async fn process(
    daemon: &Arc<Daemon>,
    agent: &Agent,
    orders: &mut mpsc::UnboundedReceiver<Order>,
    turns: &mut Turns,
) -> Result<bool, String> {
    change(daemon, &agent.id, AgentState::Starting, None, None);
    let provider = providers::of(agent)?;
    // Checked at every start, a replacement's included: the user may have disabled the provider
    // since the agent was made.
    // Its version may be asked, which can take seconds: off the daemon's thread.
    let paths = daemon.paths.clone();
    tokio::task::spawn_blocking(move || providers::startable(&paths, provider))
        .await
        .map_err(|e| e.to_string())??;
    let launch = providers::launch(provider, &daemon.paths, agent)?;
    // A token of this process's own, which revokes the last one.
    let grant = mcp::grant_of(daemon, agent)?;
    let token = daemon
        .mcp
        .mint(grant)
        .map_err(|e| format!("cannot make the agent's token: {e}"))?;
    let mut command = Command::new(&launch.program);
    // The daemon's environment, less what the agent has no business with; what Flottille sets
    // comes after, so that it is not among the removed.
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(provider.withheld) {
            command.env_remove(name);
        }
    }
    command
        .envs(launch.env.iter().map(|(name, value)| (name, value)))
        .args(&launch.args)
        .current_dir(&agent.folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own process group: the agent, what it runs (the bridge's `claude`) and their
        // children go together.
        .process_group(0)
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", launch.program.display()))?;
    let pid = child.id().ok_or("the agent's process ended at once")?;
    if let Some(started) = start_time(pid) {
        daemon.agents.processes.lock().unwrap().insert(pid, started);
        let _ = daemon.read(|store| store.add_process(pid, started, &agent.id));
    }
    // What Flottille set (a password) is masked too, wherever the agent would say it.
    let env: BTreeMap<OsString, OsString> = std::env::vars_os()
        .chain(launch.env.iter().cloned())
        .collect();
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(log_stderr(agent.id.clone(), stderr, env.clone()));
    }

    let recorder = Arc::new(Recorder {
        daemon: daemon.clone(),
        agent: agent.id.clone(),
        provider,
        pid,
        speech: Speech::new(Secrets::new(&env, &[])),
        env,
    });
    let events = recorder.clone();
    let server = acp::Server {
        url: daemon.mcp.url.clone(),
        token,
    };
    let exit = acp::session(
        agent,
        provider,
        &mut child,
        events,
        orders,
        turns,
        idle_release(),
        server,
    )
    .await;
    recorder.said();
    if !matches!(exit, Ok(Exit::Died(_) | Exit::Replace(_))) {
        daemon.mcp.revoke(&agent.id);
    }
    daemon.waiting_provider.lock().unwrap().remove(&agent.id);
    let goes_on = match exit {
        Ok(Exit::Released) => {
            stop(&mut child, pid).await;
            change(
                daemon,
                &agent.id,
                AgentState::Idle,
                None,
                Some(("agent.released", json!({"reason": "idle"}))),
            );
            Ok(true)
        }
        Ok(Exit::Stopped) => {
            stop(&mut child, pid).await;
            Ok(false)
        }
        Ok(Exit::Died(status)) => {
            // The agent is gone, not what it started: a `claude` left in the bridge's group would
            // run on as an orphan, with its tools.
            signal_group(pid, libc::SIGKILL);
            daemon.mcp.retire(&agent.id);
            Ok(died(daemon, &agent.id, status, turns))
        }
        Ok(Exit::Replace(reason)) => {
            stop(&mut child, pid).await;
            daemon.mcp.retire(&agent.id);
            turns.running = false;
            turns.queued.clear();
            Ok(replace(daemon, &agent.id, &reason, true, false))
        }
        Err(reason) => {
            stop(&mut child, pid).await;
            Err(reason)
        }
    };
    daemon.agents.processes.lock().unwrap().remove(&pid);
    let _ = daemon.read(|store| store.remove_process(pid));
    (provider.forget_process)(&daemon.paths, pid);
    goes_on
}

/// The process died: published, then the agent's session is replaced. Whether it goes on.
fn died(daemon: &Arc<Daemon>, id: &str, status: ExitStatus, turns: &mut Turns) -> bool {
    use std::os::unix::process::ExitStatusExt;
    let during_turn = std::mem::replace(&mut turns.running, false);
    turns.queued.clear();
    if during_turn {
        record(
            daemon,
            id,
            "agent.turn_ended",
            json!({"stop_reason": "interrupted", "usage": null}),
            false,
        );
    }
    let reason = match (status.code(), status.signal()) {
        (Some(code), _) => format!("its process died (exit code {code})"),
        (_, Some(signal)) => format!("its process died (signal {signal})"),
        _ => "its process died".to_owned(),
    };
    record(
        daemon,
        id,
        "agent.died",
        json!({"code": status.code(), "signal": status.signal(), "during_turn": during_turn}),
        false,
    );
    replace(daemon, id, &reason, true, true)
}

/// A new session replaces the agent's (`docs/reference/hemera.md` §5 "Replacement"): what its
/// last turn did not take is given back, its brief is delivered again with a word on why, and its
/// parent is told when `tell` (a stuck agent's was told already). Past the replacement limit it
/// fails instead. Whether it goes on.
pub fn replace(daemon: &Arc<Daemon>, id: &str, reason: &str, counted: bool, tell: bool) -> bool {
    let reason = secrets::mask(reason, &std::env::vars_os().collect());
    let since = now_ms().saturating_sub(REPLACEMENT_WINDOW_MS);
    let replaced = daemon
        .read(|store| store.replacements_since(id, since))
        .unwrap_or(0);
    if counted && replaced >= REPLACEMENTS_MAX {
        tree::fail_tree(
            daemon,
            id,
            &format!("it keeps failing: {reason}, after {replaced} replacements in 30 minutes"),
        );
        return false;
    }
    let _ = daemon.write(|store| Ok(((), store.give_back(id)?)));
    // The new session gets one brief and one word on why: those its old sessions were given, or
    // were still to be given, go.
    let _ = daemon.write(|store| {
        Ok((
            (),
            store.supersede_like(id, &[&format!("brief-{id}-"), &format!("resume-{id}-")])?,
        ))
    });
    let replaced = daemon
        .read(|store| store.agent(id))
        .ok()
        .flatten()
        .and_then(|agent| Some((providers::of(&agent).ok()?, agent.native_id?)));
    let agent = match daemon.write(|store| store.replace_agent(id, &reason, counted)) {
        Ok(agent) if !agent.state.is_final() => agent,
        Ok(_) => return false,
        Err(error) => {
            eprintln!("flottille daemon: agent {id}: {error}");
            return false;
        }
    };
    // No process runs here: the old session is over, and nothing will load it.
    if let Some((provider, native)) = replaced {
        (provider.forget_session)(&daemon.paths, &native);
    }
    if agent.brief.is_some() {
        let (kind, brief) = crate::tree::first_message(daemon, &agent);
        deliveries::deliver(
            daemon,
            id,
            kind,
            Some(format!("brief-{id}-{}", agent.epoch)),
            &brief,
            approval::BETWEEN_TURNS,
        );
    }
    deliveries::deliver(
        daemon,
        id,
        "resume",
        Some(format!("resume-{id}-{}", agent.epoch)),
        &format!(
            "A previous session of yours stopped: {reason}. You start again from your brief: \
             check the real state of your place first, what it did may be there already."
        ),
        approval::BETWEEN_TURNS,
    );
    if tell {
        tree::tell_parent(
            daemon,
            &agent,
            "child-replaced",
            &format!(
                "its session was replaced: {reason}. A new session starts again from its brief."
            ),
            approval::BETWEEN_TURNS,
        );
    }
    true
}

/// The agent failed: it and its tree end, and its parent is told.
fn fail(daemon: &Arc<Daemon>, id: &str, reason: &str) {
    let _ = daemon.write(|store| Ok(((), store.give_back(id)?)));
    tree::fail_tree(daemon, id, reason);
}

/// Changes the agent's state, with its event when one is given.
pub fn change(
    daemon: &Daemon,
    id: &str,
    state: AgentState,
    reason: Option<String>,
    event: Option<(&str, Value)>,
) {
    let result = daemon.write(|store| {
        let event = event.map(|(name, mut data)| {
            data["agent"] = id.into();
            (name, data)
        });
        let (_, event) = store.change_agent(
            id,
            AgentChange {
                state: Some(state),
                state_reason: reason,
                ..AgentChange::default()
            },
            event,
        )?;
        Ok(((), event))
    });
    if let Err(error) = result {
        eprintln!("flottille daemon: agent {id}: {error}");
    }
}

pub fn record(daemon: &Daemon, id: &str, name: &str, mut data: Value, activity: bool) {
    data["agent"] = id.into();
    if let Err(error) = daemon.write(|store| {
        let event = store.append_agent_event(id, name, data, activity)?;
        Ok(((), Some(event)))
    }) {
        eprintln!("flottille daemon: agent {id}: {error}");
    }
}

/// What the session loop reports, written to the store and the event stream.
struct Recorder {
    daemon: Arc<Daemon>,
    agent: String,
    provider: &'static Provider,
    pid: u32,
    /// Secrets of the daemon's environment are masked out of what the agent says and does.
    env: BTreeMap<OsString, OsString>,
    /// What the agent is saying, held back until it is settled.
    speech: Speech,
}

/// Events of the session that say nothing of the conversation: they may come between two pieces
/// of one message without ending it.
const BOOKKEEPING: [&str; 4] = [
    "agent.usage",
    "agent.mode_changed",
    "agent.options_changed",
    "agent.provider_wait",
];

/// Events that stream a text in pieces: a secret their pieces split is whole only in the text.
const STREAMED: [&str; 3] = ["agent.message", "agent.thought", "agent.user_message"];

/// What the agent says, thinks or is told, in pieces, masked whole before it is recorded.
struct Speech {
    /// The secret values of the daemon's environment, found once for every piece.
    secrets: Secrets,
    /// The text being streamed, while it is.
    saying: Mutex<Option<Saying>>,
}

/// A text still streaming: its event, its message id when its source gives one, its last part,
/// held back until it is settled (`Secrets::settled`), and the bookkeeping kept after it.
struct Saying {
    name: &'static str,
    id: Option<String>,
    held: String,
    after: Vec<(String, Value, bool)>,
}

/// Where `Speech` records an event: its name, its data and whether it is activity.
type Record<'a> = &'a mut dyn FnMut(&str, Value, bool);

impl Speech {
    fn new(secrets: Secrets) -> Self {
        Speech {
            secrets,
            saying: Mutex::new(None),
        }
    }

    /// The text being streamed. A panic while one piece was handled leaves the held text as it
    /// was: the pieces after it are still kept, masked, rather than each panicking in turn.
    fn saying(&self) -> MutexGuard<'_, Option<Saying>> {
        self.saying.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A piece of the `name` text. What of it is settled is recorded, masked, and the rest held
    /// back until the next piece or its end; a piece of another text ends the one being streamed.
    fn say(&self, name: &'static str, data: &Value, record: Record) {
        let mut saying = self.saying();
        let id = data["message_id"].as_str().map(str::to_owned);
        if saying
            .as_ref()
            .is_some_and(|said| said.name != name || said.id != id)
        {
            self.end(saying.take(), record);
        }
        let said = saying.get_or_insert(Saying {
            name,
            id,
            held: String::new(),
            after: Vec::new(),
        });
        said.held
            .push_str(data["text"].as_str().unwrap_or_default());
        let settled = said.held.drain(..self.secrets.settled(&said.held));
        self.record_said(name, said.id.as_deref(), settled.as_str(), record);
    }

    /// Keeps a bookkeeping event after the text being streamed, which stays one; gives it back
    /// when none is.
    fn keep(&self, name: &str, data: Value, activity: bool) -> Result<(), Value> {
        match self.saying().as_mut() {
            Some(said) => {
                said.after.push((name.to_owned(), data, activity));
                Ok(())
            }
            None => Err(data),
        }
    }

    /// The text being streamed is over: what was held back is recorded.
    fn said(&self, record: Record) {
        let mut saying = self.saying();
        self.end(saying.take(), record);
    }

    fn end(&self, saying: Option<Saying>, record: Record) {
        if let Some(said) = saying {
            self.record_said(said.name, said.id.as_deref(), &said.held, record);
            for (name, data, activity) in said.after {
                record(&name, data, activity);
            }
        }
    }

    /// Records `text` of the `name` text `id`, masked, in pieces no line of a reader cuts.
    fn record_said(&self, name: &str, id: Option<&str>, text: &str, record: Record) {
        for piece in secrets::pieces(&self.secrets.mask(text), tree::LINE_CHARS) {
            let mut data = json!({"text": piece, "replay": false});
            if let Some(id) = id {
                data["message_id"] = self.secrets.mask(id).into();
            }
            record(name, data, true);
        }
    }
}

/// A piece the session handled after its end (a process that died as it spoke) is kept too.
impl Drop for Recorder {
    fn drop(&mut self) {
        self.said();
    }
}

impl Recorder {
    fn record(&self, name: &str, data: Value, activity: bool) {
        record(&self.daemon, &self.agent, name, data, activity);
    }

    /// The text the agent was streaming is over: what was held back is recorded.
    fn said(&self) {
        self.speech
            .said(&mut |name, data, activity| self.record(name, data, activity));
    }
}

impl acp::Events for Recorder {
    fn conversation(&self, name: &str, data: Value, activity: bool) {
        // The history a load replays is held already, as it happened: never recorded twice
        // (`docs/reference/hemera.md` §8.7).
        if data["replay"] == true {
            return;
        }
        {
            let mut waiting = self.daemon.waiting_provider.lock().unwrap();
            if name == "agent.provider_wait" {
                waiting.insert(self.agent.clone());
            } else if activity {
                waiting.remove(&self.agent);
            }
        }
        if let Some(&streamed) = STREAMED.iter().find(|&&streamed| streamed == name) {
            self.speech
                .say(streamed, &data, &mut |name, data, activity| {
                    self.record(name, data, activity)
                });
            return;
        }
        let mut data = mask(data, &self.env);
        // Bookkeeping between two pieces of a message does not end it: it is kept after the
        // message, which stays one. Anything else the agent does (a tool call, a compaction…)
        // ends it.
        if BOOKKEEPING.contains(&name) {
            match self.speech.keep(name, data, activity) {
                Ok(()) => return,
                Err(not_kept) => data = not_kept,
            }
        } else {
            self.said();
        }
        self.record(name, data, activity);
        if name == "agent.compacted" {
            tree::compacted(&self.daemon, &self.agent);
        }
    }

    fn pump(&self) -> Option<deliveries::Prompt> {
        deliveries::pump(&self.daemon, &self.agent)
    }

    fn image(&self, name: &str) -> Option<(&'static str, String)> {
        deliveries::read_image(&self.daemon, &self.agent, name)
    }

    fn offered(&self, options: &Value, fresh: bool) {
        providers::learn(&self.daemon, self.provider, options, fresh);
    }

    fn opened(
        &self,
        opened: &str,
        native_id: &str,
        agent_version: &str,
        taken: Choices,
        reopens: bool,
        images: bool,
    ) {
        let result = self.daemon.write(|store| {
            let data = json!({"agent": self.agent, "pid": self.pid, "opened": opened,
                "native_id": native_id, "agent_version": agent_version, "taken": taken});
            let (_, event) = store.change_agent(
                &self.agent,
                AgentChange {
                    state: Some(AgentState::Idle),
                    native_id: Some(native_id.to_owned()),
                    agent_version: Some(agent_version.to_owned()),
                    reopens: Some(reopens),
                    images: Some(images),
                    taken: Some(taken),
                    ..AgentChange::default()
                },
                Some(("agent.started", data)),
            )?;
            Ok(((), event))
        });
        if let Err(error) = result {
            eprintln!("flottille daemon: agent {}: {error}", self.agent);
        }
    }

    fn turn_started(&self, text: &str, images: &[String]) {
        let mut data = json!({"text": text});
        if !images.is_empty() {
            data["images"] = json!(images);
        }
        change(
            &self.daemon,
            &self.agent,
            AgentState::Working,
            None,
            Some(("agent.prompted", data)),
        );
    }

    fn turn_ended(&self, stop_reason: &str, usage: Value) {
        self.said();
        let _ = self
            .daemon
            .read(|store| store.deliveries_taken(&self.agent));
        change(
            &self.daemon,
            &self.agent,
            AgentState::Idle,
            None,
            Some((
                "agent.turn_ended",
                json!({"stop_reason": stop_reason, "usage": usage}),
            )),
        );
        tree::after_turn(&self.daemon, &self.agent, stop_reason);
    }
}

/// Every text of the value masked.
fn mask(value: Value, env: &BTreeMap<OsString, OsString>) -> Value {
    match value {
        Value::String(text) => Value::String(secrets::mask(&text, env)),
        Value::Array(items) => Value::Array(items.into_iter().map(|v| mask(v, env)).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, mask(v, env))).collect())
        }
        other => other,
    }
}

async fn log_stderr(
    agent: String,
    stderr: tokio::process::ChildStderr,
    env: BTreeMap<OsString, OsString>,
) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        eprintln!("agent {agent}: {}", secrets::mask(&line, &env));
    }
}

/// Asks the process group to stop, then kills it after the grace time.
async fn stop(child: &mut Child, pid: u32) {
    signal_group(pid, libc::SIGTERM);
    if tokio::time::timeout(STOP_GRACE, child.wait())
        .await
        .is_err()
    {
        signal_group(pid, libc::SIGKILL);
        let _ = child.wait().await;
    }
}

pub fn signal_group(pid: u32, signal: i32) {
    // SAFETY: killpg has no memory precondition; the group is the process's own (`process_group(0)`).
    unsafe {
        libc::killpg(pid as i32, signal);
    }
}

/// When the process started, in clock ticks after boot (`/proc/<pid>/stat`, field 22): with its
/// pid, what tells it from a process that took the pid since.
pub fn start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name, field 2, is in parentheses and may hold spaces.
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// At daemon start: kills the process groups a previous daemon left running, each only when the
/// process is still the one it started, then forgets them all.
pub fn kill_orphans(store: &Store) {
    let processes = match store.processes() {
        Ok(processes) => processes,
        Err(error) => {
            eprintln!("flottille daemon: reading the agents' processes: {error}");
            return;
        }
    };
    for (pid, started) in processes {
        kill_left(store, pid, started);
    }
}

/// Kills the process group of `pid` when it is still the process started at `started`, then
/// forgets it.
fn kill_left(store: &Store, pid: u32, started: u64) {
    if start_time(pid) == Some(started) {
        signal_group(pid, libc::SIGKILL);
        eprintln!("flottille daemon: killed the agent process {pid} that was left running");
    }
    let _ = store.remove_process(pid);
}

/// At daemon start: an agent that was starting or working has lost its process and its turn. Its
/// session is kept (the next process resumes it, which keeps what it knew), and every agent with
/// something queued is rung. A working agent's prompt was sent (it is working once its prompt is
/// asked): it is in the session's history, so it is taken as sent, never sent again, and the
/// agent is told its turn was cut. A starting agent's was not sent yet: it is given back.
pub fn settle_at_start(daemon: &Arc<Daemon>) {
    let agents = match daemon.read(Store::agents) {
        Ok(agents) => agents,
        Err(error) => {
            eprintln!("flottille daemon: reading agents: {error}");
            return;
        }
    };
    for agent in agents {
        if matches!(agent.state, AgentState::Starting | AgentState::Working) {
            change(
                daemon,
                &agent.id,
                AgentState::Idle,
                None,
                Some(("agent.released", json!({"reason": "restart"}))),
            );
            if agent.state == AgentState::Starting {
                let _ = daemon.write(|store| Ok(((), store.give_back(&agent.id)?)));
            } else {
                let _ = daemon.read(|store| store.deliveries_taken(&agent.id));
                deliveries::deliver(
                    daemon,
                    &agent.id,
                    "resume",
                    None,
                    "Flottille restarted while you worked, and your last turn was cut: check the \
                     real state of your place, then go on.",
                    approval::BETWEEN_TURNS,
                );
            }
        }
        if !agent.state.is_final()
            && daemon
                .read(|store| store.deliveries(&agent.id, "queued"))
                .is_ok_and(|queued| !queued.is_empty())
        {
            deliveries::ring(daemon, &agent.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poisoned_saying_lock_still_keeps_the_pieces_after_it_masked() {
        let env = BTreeMap::from([("API_TOKEN".into(), "s3cr3t-token-value".into())]);
        let speech = Speech::new(Secrets::new(&env, &[]));
        let piece = |text: &str| json!({"text": text});
        // A recording that panics while the lock is held poisons it.
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            speech.say(
                "agent.message",
                &piece("plain words that settle at once and more "),
                &mut |_, _, _| panic!("the store is gone"),
            );
        }));
        assert!(panicked.is_err() && speech.saying.is_poisoned());

        let mut recorded = String::new();
        let mut record = |_: &str, data: Value, _| {
            recorded.push_str(data["text"].as_str().unwrap());
        };
        speech.say("agent.message", &piece("the token is s3cr3t-"), &mut record);
        speech.say(
            "agent.message",
            &piece("token-value, keep it quiet"),
            &mut record,
        );
        speech.said(&mut record);

        assert!(
            !recorded.contains("s3cr3t-"),
            "{}",
            recorded.replace("s3cr3t-", "<clear>")
        );
        assert!(
            recorded.ends_with("the token is [masked], keep it quiet"),
            "{recorded}"
        );
    }

    fn alive(pid: u32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat
                .rsplit_once(')')
                .unwrap()
                .1
                .trim_start()
                .starts_with('Z')
        })
    }

    fn sleeper() -> std::process::Child {
        use std::os::unix::process::CommandExt;
        std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap()
    }

    #[test]
    fn an_orphan_is_killed_and_a_process_that_took_its_pid_since_is_not() {
        let store = Store::open_in_memory().unwrap();
        let mut orphan = sleeper();
        let mut other = sleeper();
        store
            .add_process(orphan.id(), start_time(orphan.id()).unwrap(), "a")
            .unwrap();
        // The pid of a process that started at another time: not the one Flottille started.
        store
            .add_process(other.id(), start_time(other.id()).unwrap() + 1, "b")
            .unwrap();

        kill_orphans(&store);

        let _ = orphan.wait();
        assert!(!alive(orphan.id()));
        assert!(
            alive(other.id()),
            "a process that merely reused the pid is spared"
        );
        assert!(store.processes().unwrap().is_empty());
        let _ = other.kill();
        let _ = other.wait();
    }

    #[test]
    fn a_start_time_is_read_for_a_command_with_spaces_in_its_name() {
        assert!(start_time(std::process::id()).is_some());
        assert_eq!(start_time(u32::MAX), None);
    }

    #[test]
    fn every_text_is_masked() {
        let env = [(OsString::from("API_TOKEN"), OsString::from("s3cr3t-value"))].into();
        let masked = mask(json!({"a": ["the s3cr3t-value"], "n": 1}), &env);
        assert_eq!(masked, json!({"a": ["the [masked]"], "n": 1}));
    }
}
