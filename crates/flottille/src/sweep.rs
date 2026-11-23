//! The sweep (`docs/reference/hemera.md` §5 "Sweep"): every few seconds, what time alone decides.
//! A note not taken in time goes as a prompt; a child stuck in a turn is replaced; a lead whose
//! children run while nothing happens is told how they are; a child that reported and stays in
//! its turn is stopped; a led session's integration check that had to wait, or found its work
//! not integrated yet, runs again; an ended
//! root's session kept 7 days is forgotten. It never wakes an agent by itself: what it finds is delivered.
//!
//! Every time is read from the store's clock, which a test moves ahead (`test.advance`).

use std::sync::Arc;
use std::time::Duration;

use flottille_core::agent::AgentState;
use flottille_core::approval;
use flottille_core::store::{AgentChange, Store, advance_clock, now_ms};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::agents::{self, Order};
use crate::daemon::Daemon;
use crate::deliveries;
use crate::projects::Failure;
use crate::tree;

const EVERY: Duration = Duration::from_secs(5);
/// A turn with no sign of activity this long, nothing else explaining it, is stuck.
pub const STUCK_MS: u64 = 5 * 60 * 1000;
/// A lead between turns, its children running, with no delivery this long, is told how they are;
/// the interval doubles while nothing else comes, up to the ceiling.
const STATUS_FIRST_MS: u64 = 20 * 60 * 1000;
const STATUS_CEILING_MS: u64 = 160 * 60 * 1000;

/// Sweeps until the daemon stops.
pub async fn run(daemon: Arc<Daemon>, mut stopping: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(EVERY) => sweep(&daemon),
            _ = stopping.wait_for(|stopping| *stopping) => return,
        }
    }
}

pub fn sweep(daemon: &Arc<Daemon>) {
    deliveries::fall_back(daemon);
    crate::across::drop_old(daemon);
    crate::notifications::keep(daemon);
    tree::end_late_reporters(daemon);
    tree::remind_integrated_due(daemon);
    crate::subscriptions::sweep(daemon);
    crate::forge::poll(daemon);
    crate::items::poll(daemon);
    agents::forget_kept_sessions(daemon);
    crate::inactive::check(daemon);
    crate::quota::check(daemon);
    crate::models::check(daemon);
    forget_runs(daemon);
    let agents = daemon.read(Store::live_agents).unwrap_or_default();
    let now = now_ms();
    for agent in &agents {
        match agent.state {
            AgentState::Working if agent.reported_at.is_none() => stuck(daemon, agent, now),
            AgentState::Idle => {
                children_status(daemon, agent, now);
                crate::compaction::at_rest(daemon, agent, now);
            }
            _ => {}
        }
    }
}

/// The runs of a session that ended for good go with it, as its other data does: a root that
/// ended or failed and cannot be resumed, or whose kept session was forgotten, and the tree under
/// it. A run still running is left to end.
fn forget_runs(daemon: &Daemon) {
    let Ok(with_runs) = daemon.read(Store::agents_with_runs) else {
        return;
    };
    if with_runs.is_empty() {
        return;
    }
    // Each agent with runs is looked up, and its parents: the whole history is not read.
    let over: Vec<String> = daemon
        .read(|store| {
            let mut over = Vec::new();
            for id in with_runs {
                let mut at = store.agent(&id)?;
                while let Some(agent) = at {
                    match &agent.parent {
                        Some(parent) => at = store.agent(parent)?,
                        None => {
                            if agent.state.is_final() && !tree::keeps_session(&agent) {
                                over.push(id);
                            }
                            break;
                        }
                    }
                }
            }
            Ok(over)
        })
        .unwrap_or_default();
    if !over.is_empty()
        && let Err(error) = daemon.read(|store| store.forget_runs_of(&over))
    {
        eprintln!("flottille daemon: forgetting the runs of ended sessions: {error}");
    }
}

/// A turn showing no sign for 5 minutes, not waiting on the provider, with no command of the
/// agent's own running or just ended: the agent is stuck, its parent told, and its session
/// replaced.
fn stuck(daemon: &Arc<Daemon>, agent: &flottille_core::agent::Agent, now: u64) {
    let last = agent.active_at.unwrap_or(0).max(agent.updated_at);
    let quiet = now.saturating_sub(last);
    if quiet < STUCK_MS || daemon.waiting_provider.lock().unwrap().contains(&agent.id) {
        return;
    }
    let running = daemon
        .read(Store::running_runs)
        .unwrap_or_default()
        .iter()
        .any(|run| run.agent.as_deref() == Some(agent.id.as_str()));
    let ended = daemon
        .read(|store| store.last_run_end(&agent.id))
        .ok()
        .flatten();
    if running || ended.is_some_and(|at| now.saturating_sub(at) < STUCK_MS) {
        return;
    }
    let minutes = quiet / 60_000;
    // Out of `working` at once, so that the next sweep does not find it again.
    let _ = daemon.write(|store| {
        let (_, event) = store.change_agent(
            &agent.id,
            AgentChange {
                state: Some(AgentState::Starting),
                state_reason: Some("stuck".into()),
                ..AgentChange::default()
            },
            Some(("agent.stuck", json!({"agent": agent.id, "quiet_ms": quiet}))),
        )?;
        Ok(((), event))
    });
    tree::tell_parent(
        daemon,
        agent,
        "child-stuck",
        &format!(
            "was stuck: no sign of activity for {minutes} min in a turn, and no command of its \
             own running. Flottille replaces it with a new session that starts from its first \
             message and what Flottille holds of its conversation."
        ),
        approval::BETWEEN_TURNS,
    );
    agents::tell(
        daemon,
        &agent.id,
        Order::Replace(format!(
            "it was stuck: no sign of activity for {minutes} min"
        )),
    );
}

/// A lead between turns with children running and no delivery for its interval gets one line per
/// child.
fn children_status(daemon: &Arc<Daemon>, agent: &flottille_core::agent::Agent, now: u64) {
    let children: Vec<_> = daemon
        .read(|store| store.children(&agent.id))
        .unwrap_or_default()
        .into_iter()
        .filter(|child| !child.state.is_final())
        .filter(|child| !crate::extensions::hidden(daemon, &agent.id, &child.id))
        .collect();
    if children.is_empty() {
        return;
    }
    let last = daemon
        .read(|store| store.last_deliveries(&agent.id, 16))
        .unwrap_or_default();
    let statuses = last
        .iter()
        .take_while(|d| d.kind == "children-status")
        .count() as u32;
    let interval = STATUS_FIRST_MS
        .saturating_mul(1 << statuses.min(16))
        .min(STATUS_CEILING_MS);
    let since = last.first().map_or(agent.created_at, |d| d.created_at);
    if now.saturating_sub(since) < interval {
        return;
    }
    let mut body = format!(
        "Nothing happened for {} min. Your children:",
        now.saturating_sub(since) / 60_000
    );
    for child in &children {
        body.push_str(&format!("\n- {}", tree::line(daemon, child)));
    }
    body.push_str(
        "\n\nDecide whether that is normal (a long command) or not (a child going in circles): \
         agent_send, agent_stop or a new launch; or nothing.",
    );
    deliveries::deliver(
        daemon,
        &agent.id,
        "children-status",
        None,
        &body,
        approval::BETWEEN_TURNS,
    );
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Advance {
    ms: u64,
}

/// The `test.*` methods: `test.advance` only when the daemon runs with `FLOTTILLE_TEST_CLOCK`
/// set, so that a test sees minutes pass without waiting for them (`docs/agents.md`); `test.hold`
/// only in a test build run with `FLOTTILLE_TEST_HOLD` set, for the tests of a daemon that does
/// not answer (`docs/reference/daemon.md`).
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    if method == "test.hold"
        && cfg!(debug_assertions)
        && std::env::var_os("FLOTTILLE_TEST_HOLD").is_some()
    {
        let asked: Advance = serde_json::from_value(params)?;
        // Said before it holds, with no await between: once a test reads it, the runtime runs
        // nothing else, not even an accept.
        eprintln!("flottille daemon: test.hold: the runtime is held");
        daemon.hold_store(Duration::from_millis(asked.ms));
        return Ok(json!({}));
    }
    // A request that waits without holding the runtime, as one on outside work does.
    if method == "test.wait"
        && cfg!(debug_assertions)
        && std::env::var_os("FLOTTILLE_TEST_HOLD").is_some()
    {
        let asked: Advance = serde_json::from_value(params)?;
        tokio::time::sleep(Duration::from_millis(asked.ms)).await;
        return Ok(json!({}));
    }
    // What `agent_launch` will start on another machine (#816), in a test build only.
    if matches!(method, "test.travel" | "test.travel_back") && crate::travel::on() {
        return travel(&daemon, method, params).await;
    }
    if std::env::var_os("FLOTTILLE_TEST_CLOCK").is_none() || method != "test.advance" {
        return Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        ));
    }
    let asked: Advance = serde_json::from_value(params)?;
    advance_clock(asked.ms);
    sweep(&daemon);
    Ok(json!({"now": now_ms()}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Travel {
    #[serde(default)]
    machine: String,
    #[serde(default)]
    project: String,
    launch: String,
    /// A workspace of the project; the project's own folder when left out.
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    corrupt: bool,
    /// How long each bundle stays offered before the other machine is told of it.
    #[serde(default)]
    hold_ms: u64,
}

/// `test.travel`: the code of a place sent to a machine; `test.travel_back`: the new commits of a
/// launch received sent back.
async fn travel(daemon: &Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    let asked: Travel = serde_json::from_value(params)?;
    if method == "test.travel_back" {
        return crate::travel::send_back(daemon, &asked.launch)
            .await
            .map_err(Failure::refused);
    }
    let project = crate::projects::load_project(daemon, &asked.project)?;
    let place: Vec<(String, std::path::PathBuf)> = match &asked.workspace {
        Some(name) => {
            let workspace = daemon
                .read(|store| store.workspace(&project.name, name))?
                .ok_or_else(|| Failure::refused(format!("no workspace {name}")))?;
            workspace
                .repositories
                .iter()
                .map(|worktree| (worktree.path.clone(), workspace.folder_of(&worktree.path)))
                .collect()
        }
        None => project
            .repositories
            .iter()
            .map(|repository| {
                let folder = match repository.path.as_str() {
                    "." => project.folder.clone(),
                    path => project.folder.join(path),
                };
                (repository.path.clone(), folder)
            })
            .collect(),
    };
    crate::travel::send(
        daemon,
        &asked.machine,
        &asked.launch,
        &project.name,
        &place,
        crate::travel::Testing {
            corrupt: asked.corrupt,
            hold: std::time::Duration::from_millis(asked.hold_ms),
        },
    )
    .await
    .map_err(Failure::refused)
}
