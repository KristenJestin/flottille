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

/// What `sessions.toml` says: the days after which an inactive session is ended, `None` while the
/// ending is off, and whether a session is resumed when its quota resets (`docs/design.md` §5.9).
/// Both are off when there is no file.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    pub days: Option<u64>,
    pub resume: bool,
}

pub fn read(paths: &Paths) -> Result<Settings, String> {
    let text = match std::fs::read_to_string(config(paths)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => return Err(format!("sessions.toml: {error}")),
    };
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| format!("sessions.toml: {}", e.message()))?;
    for (key, _) in document.iter() {
        if key != "end_inactive_days" && key != RESUME_KEY {
            return Err(format!("sessions.toml: unknown key `{key}`"));
        }
    }
    let resume = match document.get(RESUME_KEY) {
        None => false,
        Some(item) => item
            .as_bool()
            .ok_or_else(|| format!("sessions.toml: {RESUME_KEY} is true or false"))?,
    };
    let days = match document.get("end_inactive_days") {
        None => None,
        Some(item) if item.as_str() == Some("off") => None,
        Some(item) => match item.as_integer() {
            Some(days) if (MIN_DAYS as i64..=MAX_DAYS as i64).contains(&days) => Some(days as u64),
            _ => {
                return Err(format!(
                    "sessions.toml: end_inactive_days is a number of days from {MIN_DAYS} to \
                     {MAX_DAYS}, or \"off\""
                ));
            }
        },
    };
    Ok(Settings { days, resume })
}

/// The key of resuming a session at its quota's reset.
const RESUME_KEY: &str = "resume_at_reset";

/// The days as the protocol says them: a number, or `"off"`. Nothing is null: a client leaves
/// null fields out of what it sends.
fn said(days: Option<u64>) -> Value {
    match days {
        Some(days) => json!(days),
        None => json!("off"),
    }
}

fn shown(settings: Settings) -> Value {
    json!({"days": said(settings.days), "resume": settings.resume})
}

/// Answers a `sessions.*` request of the user: `sessions.show`, `sessions.set`. No tool of an
/// agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    match method {
        "sessions.show" => {
            let settings = read(paths).map_err(|e| Failure::new("invalid-config", e))?;
            Ok(shown(settings))
        }
        "sessions.set" => {
            let days = match params.get("days") {
                None => None,
                Some(Value::String(word)) if word == "off" => Some(None),
                Some(Value::Number(n))
                    if n.as_u64()
                        .is_some_and(|n| (MIN_DAYS..=MAX_DAYS).contains(&n)) =>
                {
                    Some(n.as_u64())
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
            let resume = match params.get("resume") {
                None => None,
                Some(Value::Bool(resume)) => Some(*resume),
                Some(_) => {
                    return Err(Failure::new("invalid-params", "resume is true or false"));
                }
            };
            if days.is_none() && resume.is_none() {
                return Err(Failure::new(
                    "invalid-params",
                    format!(
                        "days is a number of days from {MIN_DAYS} to {MAX_DAYS}, or \"off\" for \
                         no ending; resume is true or false"
                    ),
                ));
            }
            // What is not asked stays as the file says; a file that cannot be read is written
            // again whole.
            let before = read(paths).unwrap_or_default();
            let wanted = Settings {
                days: days.unwrap_or(before.days),
                resume: resume.unwrap_or(before.resume),
            };
            std::fs::create_dir_all(&paths.config).map_err(|e| {
                Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
            })?;
            let mut text = match wanted.days {
                Some(days) => format!("end_inactive_days = {days}\n"),
                None => "end_inactive_days = \"off\"\n".to_owned(),
            };
            if wanted.resume {
                text.push_str(&format!("{RESUME_KEY} = true\n"));
            }
            providers::write_in_place(&config(paths), &text)
                .map_err(|e| Failure::new("unavailable", format!("sessions.toml: {e}")))?;
            let answer = shown(wanted);
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
    let Ok(Settings {
        days: Some(days), ..
    }) = read(&daemon.paths)
    else {
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
