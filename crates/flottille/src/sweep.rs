//! The sweep (`docs/reference/hemera.md` §5 "Sweep"): every few seconds, what time alone decides.
//! A note not taken in time goes as a prompt; a child stuck in a turn is replaced; a lead whose
//! children run while nothing happens is told how they are; a child that reported and stays in
//! its turn is stopped; an ended root's session kept 7 days is forgotten. It never wakes an agent by itself: what it finds is delivered.
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
    tree::end_late_reporters(daemon);
    crate::forge::poll(daemon);
    agents::forget_kept_sessions(daemon);
    let agents = daemon.read(Store::agents).unwrap_or_default();
    let now = now_ms();
    for agent in agents.iter().filter(|a| !a.state.is_final()) {
        match agent.state {
            AgentState::Working if agent.reported_at.is_none() => stuck(daemon, agent, now),
            AgentState::Idle => children_status(daemon, agent, now),
            _ => {}
        }
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
        .any(|run| run.agent == agent.id);
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
             own running. Flottille replaces it with a new session that starts again from its \
             brief."
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

/// The `test.*` methods: only when the daemon runs with `FLOTTILLE_TEST_CLOCK` set, so that a
/// test sees minutes pass without waiting for them (`docs/agents.md`).
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
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
