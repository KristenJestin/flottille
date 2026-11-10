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

/// The text of the daemon's own word of a run.
fn daemon_text(kind: &str, data: &Value) -> String {
    match kind {
        "put_back" => put_back(data),
        _ => unchecked(data),
    }
}

/// What a `run.agent_folders_put_back` tells the user: what the put-back of a run's place did.
fn put_back(data: &Value) -> String {
    let list = |key: &str| {
        data[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut said = vec![format!(
        "A run in {} wrote in agent folders (.claude, .claude.json, .codex, .mcp.json), whose \
         settings and hooks run programs when you open the place with an agent's own command \
         line; its run {} ended last there",
        data["folder"].as_str().unwrap_or_default(),
        data["run"].as_str().unwrap_or_default()
    )];
    let (put, left) = (list("put_back"), list("left"));
    if !put.is_empty() {
        said.push(format!("Flottille put back as they were: {put}"));
    }
    if !left.is_empty() {
        said.push(format!(
            "Flottille could not put back: {left}, look at them before you open it"
        ));
    }
    if let Some(kept) = data["kept"].as_str() {
        said.push(format!("what it removed or replaced is kept in {kept}"));
    }
    format!("{}.", said.join("; "))
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
    if data["interrupted"] == true {
        said.push(
            "the daemon stopped while it went on, before it put back what was written in its \
             agent folders (.claude, .claude.json, .codex, .mcp.json): nothing there was checked \
             nor put back, and it is now how it was left"
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
    // What an extension says needs the user is no agent's: its own kind, its own text.
    let extensions = crate::extensions::needs_you();
    let mut names = vec![
        "agent.reported",
        "agent.asked",
        "agent.failed",
        "session.following_paused",
        "vault.asked",
        "run.agent_folders_unchecked",
        "run.agent_folders_put_back",
    ];
    names.extend(extensions.iter().map(|(event, _, _)| *event));
    let events = daemon.read(|store| store.events_named(&names, asked.after.unwrap_or(0)))?;
    // A paused following that resumed since says so: the inbox keeps what was sent.
    let resumed = daemon.read(|store| {
        store.events_named(&["session.following_resumed"], asked.after.unwrap_or(0))
    })?;
    // A request for a vault entry the user answered says so too.
    let answered = daemon
        .read(|store| store.events_named(&["vault.ask_answered"], asked.after.unwrap_or(0)))?;
    let mut entries = Vec::new();
    for event in events {
        if let Some((_, kind, text)) = extensions.iter().find(|(name, _, _)| *name == event.name) {
            entries.push(json!({
                "seq": event.seq, "at": event.at, "kind": kind, "agent": null,
                "project": null, "workspace": null, "text": text(&event.data),
            }));
            continue;
        }
        let kind = match (event.name.as_str(), &event.data["to"]) {
            ("agent.reported", _) => "report",
            ("agent.asked", to) if to == "user" => "question",
            ("agent.failed", _) => "failure",
            ("session.following_paused", _) => "paused",
            ("vault.asked", _) => "vault",
            ("run.agent_folders_unchecked", _) => "unchecked",
            ("run.agent_folders_put_back", _) => "put_back",
            _ => continue,
        };
        // The daemon's own word of a run is the user's to hear too: a child's, or one of an
        // agent no longer known (a daemon that stopped during it).
        let daemon_s = matches!(kind, "unchecked" | "put_back");
        let agent: Option<Agent> = match event.data["agent"].as_str() {
            Some(id) => daemon.read(|store| store.agent(id))?,
            None => None,
        };
        let Some(agent) = agent.filter(|agent| agent.parent.is_none() || daemon_s) else {
            if daemon_s {
                entries.push(json!({
                    "seq": event.seq, "at": event.at, "kind": kind,
                    "agent": event.data["agent"], "project": event.data["project"],
                    "workspace": event.data["workspace"], "text": daemon_text(kind, &event.data),
                }));
            }
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
            "unchecked" | "put_back" => json!(daemon_text(kind, &event.data)),
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
        let text = unchecked(
            &json!({"run": "r1", "folder": "/work/app", "too_large": false,
            "interrupted": true}),
        );
        assert!(text.contains("daemon stopped"), "{text}");
    }

    #[test]
    fn a_put_back_entry_says_what_was_put_back_left_and_kept() {
        let text = put_back(&json!({"run": "r1", "folder": "/work/app",
            "put_back": [".claude/settings.json"], "left": [".codex/big.bin"],
            "kept": "/state/agent-folders/r1"}));
        assert!(
            text.contains("put back as they were: .claude/settings.json"),
            "{text}"
        );
        assert!(
            text.contains("could not put back: .codex/big.bin"),
            "{text}"
        );
        assert!(text.contains("/state/agent-folders/r1"), "{text}");
        // One stored before it said why was a place too large.
        let text = unchecked(&json!({"run": "r1", "folder": "/work/app"}));
        assert!(text.contains("too large for Flottille to check"), "{text}");
    }
}
