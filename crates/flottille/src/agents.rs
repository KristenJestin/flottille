//! The `agent.*` methods (`docs/protocol.md`) and the life of each agent (`docs/agents.md`).
//!
//! An agent is owned by one task of its own, which starts its adapter process when a prompt comes,
//! keeps it while it works, lets it go after the idle release time, and notices when it dies. The
//! methods only write the store and hand the task its orders: none of them waits on the agent.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_core::agent::{Agent, AgentState, Choices, new_id};
use flottille_core::protocol::{AgentName, AgentRun, AgentSend};
use flottille_core::secrets;
use flottille_core::store::{AgentChange, Store, now_ms};
use flottille_core::workspace::State;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::acp::{self, Exit};
use crate::claude;
use crate::daemon::Daemon;
use crate::projects::{self, Failure};

type Answer = Result<Value, Failure>;

/// A process idle this long is let go (`docs/reference/hemera.md` §1); the next prompt starts it
/// again. `FLOTTILLE_IDLE_RELEASE_MS` changes it, for tests and development.
const IDLE_RELEASE: Duration = Duration::from_secs(5 * 60);
/// How long a process asked to stop has before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// The events `agent.show` gives at once.
const PAGE: usize = 1000;

/// What an agent's task is asked.
pub enum Order {
    Prompt(String),
    /// Answered whether a turn was running.
    Cancel(oneshot::Sender<bool>),
    Stop,
}

/// The prompts waiting for the agent, kept across its processes, and whether one is being played.
pub struct Turns {
    pub queued: VecDeque<String>,
    pub running: bool,
}

/// An agent's task, and where its orders go.
type Task = (mpsc::UnboundedSender<Order>, JoinHandle<()>);

/// The tasks of the agents that have one, by agent id.
#[derive(Default)]
pub struct Live {
    tasks: Mutex<HashMap<String, Task>>,
}

/// Answers an `agent.*` request.
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Answer {
    let daemon = &daemon;
    match method {
        "agent.run" => run(daemon, serde_json::from_value(params)?),
        "agent.send" => {
            let asked: AgentSend = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            refuse_final(&agent)?;
            refuse_empty(&asked.message)?;
            let after = daemon.read(Store::last_seq)?;
            order(daemon, &agent.id, Order::Prompt(asked.message))?;
            Ok(json!({"agent": agent.to_json(), "after": after}))
        }
        "agent.cancel" => {
            let asked: AgentName = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
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
            let asked: AgentName = serde_json::from_value(params)?;
            let agent = find(daemon, &asked.agent)?;
            let events = daemon
                .read(|store| store.agent_events(&agent.id, asked.after.unwrap_or(0), PAGE))?;
            Ok(json!({"agent": agent.to_json(), "events": events}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

fn find(daemon: &Daemon, id: &str) -> Result<Agent, Failure> {
    daemon
        .read(|store| store.agent(id))?
        .ok_or_else(|| Failure::new("not-found", format!("there is no agent {id}")))
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

fn refuse_empty(message: &str) -> Result<(), Failure> {
    if message.trim().is_empty() {
        return Err(Failure::refused("the message is empty"));
    }
    Ok(())
}

fn run(daemon: &Arc<Daemon>, asked: AgentRun) -> Answer {
    let project = projects::load_project(daemon, &asked.project)?;
    refuse_empty(&asked.message)?;
    let (place, folder) = match &asked.workspace {
        None => ("main".to_owned(), project.folder.clone()),
        Some(name) => {
            let workspace = daemon
                .read(|store| store.workspace(&project.name, name))?
                .ok_or_else(|| {
                    Failure::new(
                        "not-found",
                        format!("project {} has no workspace named {name}", project.name),
                    )
                })?;
            if workspace.state() != State::Ready {
                return Err(Failure::refused(format!(
                    "workspace {name} is not ready ({}): see `flottille workspace show {} {name}`",
                    serde_json::to_value(workspace.state())?
                        .as_str()
                        .unwrap_or_default(),
                    project.name
                )));
            }
            (format!("workspace {name}"), workspace.folder)
        }
    };
    // Refused now rather than failed later: the user can act on it before anything is written.
    claude::launch(&daemon.paths).map_err(Failure::refused)?;
    let now = now_ms();
    // Roots are the only agents until the tree comes: the tools of one that can have children.
    let tools = claude::tools(true, false);
    let agent = Agent {
        id: new_id().map_err(|e| Failure::new("unavailable", e.to_string()))?,
        provider: "claude".into(),
        role: "lead".into(),
        project: project.name.clone(),
        workspace: asked.workspace.clone(),
        instructions: claude::instructions(&project.name, &place, &folder),
        folder,
        state: AgentState::Starting,
        state_reason: None,
        created_at: now,
        updated_at: now,
        active_at: None,
        ended_at: None,
        chosen: Choices {
            model: asked.model,
            effort: asked.effort,
            mode: asked.mode,
        },
        taken: Choices::default(),
        native_id: None,
        agent_version: None,
        tools,
    };
    let after = daemon.read(Store::last_seq)?;
    daemon.write(|store| {
        let event = store.create_agent(
            &agent,
            (
                "agent.created",
                json!({"agent": agent.id, "project": agent.project, "workspace": agent.workspace,
                    "folder": agent.folder, "provider": agent.provider, "model": agent.chosen.model}),
            ),
        )?;
        Ok(((), event))
    })?;
    order(daemon, &agent.id, Order::Prompt(asked.message))?;
    Ok(json!({"agent": agent.to_json(), "after": after}))
}

/// Hands an order to the agent's task, starting the task when it has none.
fn order(daemon: &Arc<Daemon>, id: &str, order: Order) -> Result<(), Failure> {
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
    sender
        .send(order)
        .map_err(|_| Failure::refused(format!("agent {id} has stopped")))
}

/// Stops every agent's process, the daemon stopping: each is let go as an idle one is.
pub async fn stop_all(daemon: &Daemon) {
    let tasks: Vec<_> = daemon.agents.tasks.lock().unwrap().drain().collect();
    for (_, (sender, task)) in tasks {
        let _ = sender.send(Order::Stop);
        let _ = tokio::time::timeout(STOP_GRACE * 2, task).await;
    }
}

fn idle_release() -> Duration {
    std::env::var("FLOTTILLE_IDLE_RELEASE_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(IDLE_RELEASE, Duration::from_millis)
}

/// The life of one agent: a process per stretch of work, until it stops or fails.
async fn agent_task(daemon: Arc<Daemon>, id: String, mut orders: mpsc::UnboundedReceiver<Order>) {
    let mut turns = Turns {
        queued: VecDeque::new(),
        running: false,
    };
    loop {
        if turns.queued.is_empty() {
            match orders.recv().await {
                None | Some(Order::Stop) => return,
                Some(Order::Cancel(answer)) => {
                    let _ = answer.send(false);
                    continue;
                }
                Some(Order::Prompt(text)) => turns.queued.push_back(text),
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
    let launch = claude::launch(&daemon.paths)?;
    let mut command = Command::new(&launch.program);
    // The daemon's environment, less what the agent has no business with; what Flottille sets
    // comes after, so that it is not among the removed.
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(claude::withheld) {
            command.env_remove(name);
        }
    }
    command
        .args(&launch.args)
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&agent.folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own process group: the adapter, the CLI it runs and their children go together.
        .process_group(0)
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", launch.program.display()))?;
    let pid = child.id().ok_or("the agent's process ended at once")?;
    if let Some(started) = start_time(pid) {
        let _ = daemon.read(|store| store.add_process(pid, started, &agent.id));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(log_stderr(agent.id.clone(), stderr));
    }

    let events = Arc::new(Recorder {
        daemon: daemon.clone(),
        agent: agent.id.clone(),
        pid,
        env: std::env::vars_os().collect(),
    });
    let exit = acp::session(agent, &mut child, events, orders, turns, idle_release()).await;
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
            died(daemon, &agent.id, status, turns);
            Ok(false)
        }
        Err(reason) => {
            stop(&mut child, pid).await;
            Err(reason)
        }
    };
    let _ = daemon.read(|store| store.remove_process(pid));
    goes_on
}

fn died(daemon: &Daemon, id: &str, status: ExitStatus, turns: &mut Turns) {
    use std::os::unix::process::ExitStatusExt;
    let during_turn = std::mem::replace(&mut turns.running, false);
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
    change(
        daemon,
        id,
        AgentState::Failed,
        Some(reason),
        Some((
            "agent.died",
            json!({"code": status.code(), "signal": status.signal(), "during_turn": during_turn}),
        )),
    );
}

fn fail(daemon: &Daemon, id: &str, reason: &str) {
    let reason = secrets::mask(reason, &std::env::vars_os().collect());
    change(
        daemon,
        id,
        AgentState::Failed,
        Some(reason.clone()),
        Some(("agent.failed", json!({"reason": reason}))),
    );
}

/// Changes the agent's state, with its event when one is given.
fn change(
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

fn record(daemon: &Daemon, id: &str, name: &str, mut data: Value, activity: bool) {
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
    pid: u32,
    /// Secrets of the daemon's environment are masked out of what the agent says and does.
    env: BTreeMap<OsString, OsString>,
}

impl acp::Events for Recorder {
    fn conversation(&self, name: &str, data: Value, activity: bool) {
        record(
            &self.daemon,
            &self.agent,
            name,
            mask(data, &self.env),
            activity,
        );
    }

    fn opened(&self, opened: &str, native_id: &str, agent_version: &str, taken: Choices) {
        let result = self.daemon.write(|store| {
            let data = json!({"agent": self.agent, "pid": self.pid, "opened": opened,
                "native_id": native_id, "agent_version": agent_version, "taken": taken});
            let (_, event) = store.change_agent(
                &self.agent,
                AgentChange {
                    state: Some(AgentState::Idle),
                    native_id: Some(native_id.to_owned()),
                    agent_version: Some(agent_version.to_owned()),
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

    fn turn_started(&self, text: &str) {
        change(
            &self.daemon,
            &self.agent,
            AgentState::Working,
            None,
            Some(("agent.prompted", json!({"text": text}))),
        );
    }

    fn turn_ended(&self, stop_reason: &str, usage: Value) {
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

async fn log_stderr(agent: String, stderr: tokio::process::ChildStderr) {
    let env = std::env::vars_os().collect();
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

fn signal_group(pid: u32, signal: i32) {
    // SAFETY: killpg has no memory precondition; the group is the process's own (`process_group(0)`).
    unsafe {
        libc::killpg(pid as i32, signal);
    }
}

/// When the process started, in clock ticks after boot (`/proc/<pid>/stat`, field 22): with its
/// pid, what tells it from a process that took the pid since.
fn start_time(pid: u32) -> Option<u64> {
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
        if start_time(pid) == Some(started) {
            signal_group(pid, libc::SIGKILL);
            eprintln!("flottille daemon: killed the agent process {pid} a previous daemon left");
        }
        let _ = store.remove_process(pid);
    }
}

/// At daemon start: an agent that was starting or working has lost its process and its turn.
pub fn settle_at_start(daemon: &Daemon) {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
