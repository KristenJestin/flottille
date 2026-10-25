//! `inbox.list`: what the root agents sent to the user (`docs/protocol.md`, `docs/cli.md`).
//!
//! The inbox is a view of stored events, not a store of its own: nothing is marked read, and a
//! client reads on from the number of the last entry it saw.

use flottille_core::agent::Agent;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;

/// The entries given at once when the client names no number to read after.
const LAST: usize = 50;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    after: Option<u64>,
    limit: Option<usize>,
}

pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "inbox.list" => list(
            daemon,
            match params {
                Value::Null => List::default(),
                params => serde_json::from_value(params)?,
            },
        ),
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// A root's report, its question to the user, or its failure, oldest first. With `after`, the
/// first `limit` after that number; without, the last `limit` of all.
fn list(daemon: &Daemon, asked: List) -> Result<Value, Failure> {
    let events = daemon.read(|store| {
        store.events_named(
            &["agent.reported", "agent.asked", "agent.failed"],
            asked.after.unwrap_or(0),
        )
    })?;
    let mut entries = Vec::new();
    for event in events {
        let kind = match (event.name.as_str(), &event.data["to"]) {
            ("agent.reported", _) => "report",
            ("agent.asked", to) if to == "user" => "question",
            ("agent.failed", _) => "failure",
            _ => continue,
        };
        let Some(id) = event.data["agent"].as_str() else {
            continue;
        };
        let agent: Option<Agent> = daemon.read(|store| store.agent(id))?;
        let Some(agent) = agent.filter(|agent| agent.parent.is_none()) else {
            continue;
        };
        let text = match kind {
            "report" => &event.data["text"],
            "question" => &event.data["question"],
            _ => &event.data["reason"],
        };
        entries.push(json!({
            "seq": event.seq, "at": event.at, "kind": kind, "agent": agent.id,
            "project": agent.project, "workspace": agent.workspace, "text": text,
        }));
    }
    let limit = asked.limit.unwrap_or(LAST);
    if asked.after.is_some() {
        entries.truncate(limit);
    } else {
        entries = entries.split_off(entries.len().saturating_sub(limit));
    }
    Ok(json!({"entries": entries}))
}
