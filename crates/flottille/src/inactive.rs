//! Ending inactive sessions (`docs/design.md` §5.7): when the user turned it
//! on, a root with no activity for the days they set is ended as their "End session" ends it
//! (`tree::stop_tree`: its tree, its runs, what was queued; its workspaces stay), with the reason
//! `inactive for N days`. Off unless the user turns it on. The setting is
//! `end_inactive_days` of `sessions.toml` in Flottille's configuration folder, read when a check
//! is due (`docs/protocol.md`, "The sessions methods").
//!
//! A check is due when the daemon starts, then at most once an hour, from the sweep: no timer of
//! its own. Every time is read from the store's clock, which a test moves ahead.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use flottille_core::agent::{Agent, AgentState};
use flottille_core::paths::Paths;
use flottille_core::store::{Store, now_ms};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::providers;
use crate::tree;

/// The fewest and the most days the user can choose.
pub const MIN_DAYS: u64 = 1;
pub const MAX_DAYS: u64 = 90;
/// How long a check waits for the next.
const EVERY_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * 3_600_000;

/// `$XDG_CONFIG_HOME/flottille/sessions.toml`.
fn config(paths: &Paths) -> PathBuf {
    paths.config.join("sessions.toml")
}

/// What `sessions.toml` says: `Some(days)`, `None` while the ending is off, which is the answer
/// when there is no file.
fn read(paths: &Paths) -> Result<Option<u64>, String> {
    let text = match std::fs::read_to_string(config(paths)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("sessions.toml: {error}")),
    };
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| format!("sessions.toml: {}", e.message()))?;
    for (key, _) in document.iter() {
        if key != "end_inactive_days" {
            return Err(format!("sessions.toml: unknown key `{key}`"));
        }
    }
    let Some(item) = document.get("end_inactive_days") else {
        return Ok(None);
    };
    if item.as_str() == Some("off") {
        return Ok(None);
    }
    match item.as_integer() {
        Some(days) if (MIN_DAYS as i64..=MAX_DAYS as i64).contains(&days) => Ok(Some(days as u64)),
        _ => Err(format!(
            "sessions.toml: end_inactive_days is a number of days from {MIN_DAYS} to {MAX_DAYS}, \
             or \"off\""
        )),
    }
}

/// The days as the protocol says them: a number, or `"off"`. Nothing is null: a client leaves
/// null fields out of what it sends.
fn said(days: Option<u64>) -> Value {
    match days {
        Some(days) => json!(days),
        None => json!("off"),
    }
}

/// Answers a `sessions.*` request of the user: `sessions.show`, `sessions.set`. No tool of an
/// agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    match method {
        "sessions.show" => {
            let days = read(paths).map_err(|e| Failure::new("invalid-config", e))?;
            Ok(json!({"days": said(days)}))
        }
        "sessions.set" => {
            let wanted = match params.get("days") {
                Some(Value::String(word)) if word == "off" => None,
                Some(Value::Number(n))
                    if n.as_u64()
                        .is_some_and(|n| (MIN_DAYS..=MAX_DAYS).contains(&n)) =>
                {
                    n.as_u64()
                }
                _ => {
                    return Err(Failure::new(
                        "invalid-params",
                        format!(
                            "days is a number of days from {MIN_DAYS} to {MAX_DAYS}, or \"off\" \
                             for no ending"
                        ),
                    ));
                }
            };
            std::fs::create_dir_all(&paths.config).map_err(|e| {
                Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
            })?;
            let text = match wanted {
                Some(days) => format!("end_inactive_days = {days}\n"),
                None => "end_inactive_days = \"off\"\n".to_owned(),
            };
            providers::write_in_place(&config(paths), &text)
                .map_err(|e| Failure::new("unavailable", format!("sessions.toml: {e}")))?;
            let answer = json!({"days": said(wanted)});
            daemon
                .emit("sessions.changed", answer.clone())
                .map_err(|error| Failure::new("unavailable", error.to_string()))?;
            Ok(answer)
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// From the daemon's start and the sweep: ends the roots inactive for the days the user set, if a
/// check is due. A file that cannot be read ends nothing.
pub fn check(daemon: &Arc<Daemon>) {
    let now = now_ms();
    let last = daemon.inactive_checked.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < EVERY_MS {
        return;
    }
    daemon.inactive_checked.store(now, Ordering::Relaxed);
    let Ok(Some(days)) = read(&daemon.paths) else {
        return;
    };
    let roots = daemon.read(Store::agents).unwrap_or_default();
    for root in roots
        .iter()
        .filter(|a| a.parent.is_none() && a.state == AgentState::Idle)
    {
        let Some(since) = quiet_since(daemon, root) else {
            continue;
        };
        if now.saturating_sub(since) >= days * DAY_MS {
            let reason = match days {
                1 => "inactive for 1 day".to_owned(),
                days => format!("inactive for {days} days"),
            };
            tree::stop_tree(daemon, &root.id, &reason);
        }
    }
}

/// When the root's tree last did anything, `None` while it has work waiting or running (a turn, a
/// message not read, a command running, a request or a question not answered, a subscription or
/// a timer pending) or when it cannot be told.
fn quiet_since(daemon: &Daemon, root: &Agent) -> Option<u64> {
    let ids = tree::ids(daemon, &root.id).ok()?;
    for id in &ids {
        let agent = daemon.read(|store| store.agent(id)).ok()??;
        if matches!(agent.state, AgentState::Starting | AgentState::Working)
            || agent.waiting_on_answer
        {
            return None;
        }
        for state in ["queued", "sending"] {
            if !daemon
                .read(|store| store.deliveries(id, state))
                .ok()?
                .is_empty()
            {
                return None;
            }
        }
        if !daemon
            .read(|store| store.pending_requests_of(id))
            .ok()?
            .is_empty()
        {
            return None;
        }
    }
    if !daemon
        .read(|store| store.running_runs_of(&ids))
        .ok()?
        .is_empty()
    {
        return None;
    }
    if daemon
        .read(Store::subscriptions)
        .ok()?
        .iter()
        .any(|s| ids.contains(&s.agent) && s.source != crate::subscriptions::SESSION)
    {
        return None;
    }
    daemon.read(|store| store.last_activity_of(&ids)).ok()?
}
