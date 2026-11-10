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

/// What a `run.agent_folders_unchecked` tells the user: why the agent folders of a run's place
/// were not all checked after it.
fn unchecked(data: &Value) -> String {
    let (run, folder) = (
        data["run"].as_str().unwrap_or_default(),
        data["folder"].as_str().unwrap_or_default(),
    );
    let mut said = Vec::new();
    // An event of before `too_large` was one.
    if data["too_large"].as_bool().unwrap_or(true) {
        said.push(
            "it is too large for Flottille to check all its agent folders (.claude, .claude.json, \
             .codex, .mcp.json) after it: those right in the place and in its repositories were \
             checked and put back, a change in one further down was neither seen nor put back"
                .to_owned(),
        );
    }
    if data["missing"] == true {
        said.push(
            "Flottille could not note its agent folders before it: what it changed there was \
             neither known nor put back"
                .to_owned(),
        );
    }
    let unreached: Vec<&str> = data["unreached"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !unreached.is_empty() {
        said.push(format!(
            "the agent folders of {}, a repository of the place whose folder is outside it \
             (reached through a link), were not checked: a change there was neither seen nor put \
             back",
            unreached.join(", ")
        ));
    }
    format!(
        "Its run {run} in {folder} ended: {}. Look there before you open it with an agent's own \
         command line.",
        said.join("; ")
    )
}

/// A root's report, its question to the user, or its failure, oldest first. With `after`, the
/// first `limit` after that number; without, the last `limit` of all.
fn list(daemon: &Daemon, asked: List) -> Result<Value, Failure> {
    let events = daemon.read(|store| {
        store.events_named(
            &[
                "agent.reported",
                "agent.asked",
                "agent.failed",
                "session.following_paused",
                "vault.asked",
                "run.agent_folders_unchecked",
            ],
            asked.after.unwrap_or(0),
        )
    })?;
    // A paused following that resumed since says so: the inbox keeps what was sent.
    let resumed = daemon.read(|store| {
        store.events_named(&["session.following_resumed"], asked.after.unwrap_or(0))
    })?;
    // A request for a vault entry the user answered says so too.
    let answered = daemon
        .read(|store| store.events_named(&["vault.ask_answered"], asked.after.unwrap_or(0)))?;
    let mut entries = Vec::new();
    for event in events {
        let kind = match (event.name.as_str(), &event.data["to"]) {
            ("agent.reported", _) => "report",
            ("agent.asked", to) if to == "user" => "question",
            ("agent.failed", _) => "failure",
            ("session.following_paused", _) => "paused",
            ("vault.asked", _) => "vault",
            ("run.agent_folders_unchecked", _) => "unchecked",
            _ => continue,
        };
        let Some(id) = event.data["agent"].as_str() else {
            continue;
        };
        let agent: Option<Agent> = daemon.read(|store| store.agent(id))?;
        // The daemon's own word of a child's run is the user's to hear too.
        let Some(agent) = agent.filter(|agent| agent.parent.is_none() || kind == "unchecked")
        else {
            continue;
        };
        let resolved = (kind == "paused"
            && resumed.iter().any(|later| {
                later.seq > event.seq
                    && later.data["agent"] == event.data["agent"]
                    && later.data["follower"] == event.data["follower"]
            }))
            || (kind == "vault" && answered.iter().any(|later| later.data["ask"] == event.seq));
        let paused = format!(
            "The session {} and the agent {} that started it have written to each other {} times \
             without you: the agent no longer wakes at the end of this session's turns. Write a \
             message to either of them to resume it.{}",
            agent.id,
            event.data["follower"].as_str().unwrap_or_default(),
            event.data["exchanges"],
            if resolved {
                " It has resumed since."
            } else {
                ""
            }
        );
        let text = match kind {
            "paused" => json!(paused),
            "report" => event.data["text"].clone(),
            "question" => event.data["question"].clone(),
            "unchecked" => json!(unchecked(&event.data)),
            "vault" => json!(format!(
                "It asks you to set the vault entry {} of {}: {}",
                event.data["name"].as_str().unwrap_or_default(),
                event.data["project"].as_str().unwrap_or_default(),
                event.data["why"].as_str().unwrap_or_default()
            )),
            _ => event.data["reason"].clone(),
        };
        entries.push(json!({
            "seq": event.seq, "at": event.at, "kind": kind, "agent": agent.id,
            "project": agent.project, "workspace": agent.workspace, "text": text,
        }));
        if kind == "question" {
            entries.last_mut().unwrap()["questions"] =
                json!(flottille_core::question::of(&event.data));
        }
        if resolved {
            entries.last_mut().unwrap()["resolved"] = json!(true);
        }
    }
    let limit = asked.limit.unwrap_or(LAST);
    if asked.after.is_some() {
        entries.truncate(limit);
    } else {
        entries = entries.split_off(entries.len().saturating_sub(limit));
    }
    Ok(json!({"entries": entries}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchecked_entry_names_a_repository_outside_the_place() {
        let text = unchecked(
            &json!({"run": "r1", "folder": "/work/app", "too_large": false,
            "unreached": ["/work/app/linked"]}),
        );
        assert!(text.contains("/work/app/linked"), "{text}");
        assert!(text.contains("outside it"), "{text}");
        assert!(!text.contains("too large"), "{text}");
        let text = unchecked(
            &json!({"run": "r1", "folder": "/work/app", "too_large": false,
            "missing": true}),
        );
        assert!(text.contains("could not note its agent folders"), "{text}");
        // One stored before it said why was a place too large.
        let text = unchecked(&json!({"run": "r1", "folder": "/work/app"}));
        assert!(text.contains("too large for Flottille to check"), "{text}");
    }
}
