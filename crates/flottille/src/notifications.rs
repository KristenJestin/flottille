//! The notification centre (`docs/protocol.md`, `notifications.list`, `notifications.read`):
//! what the daemon tells the user, one list every client reads and marks (#1001).
//!
//! The centre is made of the stored events, never of what a client says: a keeper follows the
//! events, writes one row for each thing that is told or that waits for the user, and resolves it
//! once it is answered, wherever it was answered. It reads from the last event it made rows of
//! (`notification_cursor`), so that a burst of events, or a daemon that was off, loses none, and
//! a row is written once for what it is about (`source`), so that reading an event twice changes
//! nothing. Every change says `notifications.changed`.

use std::sync::Arc;

use flottille_core::agent::Agent;
use flottille_core::approval;
use flottille_core::notification::{self, New, Which};
use flottille_core::store::now_ms;
use flottille_protocol::Event;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use crate::daemon::Daemon;
use crate::projects::Failure;

/// The rows given at once when the client names no limit.
const LAST: usize = 100;

/// The events the centre makes rows of, or resolves them with: a request held for the user and
/// its answer, a question and what answers it, a report, a failure, a pause, a vault entry asked
/// for, what the daemon says of a run's agent folders and of a message dropped.
const EVENTS: &[&str] = &[
    "approval.requested",
    "approval.forwarded",
    "approval.decided",
    "approval.cancelled",
    "agent.asked",
    "agent.answered",
    "agent.replaced",
    "agent.resumed",
    "agent.ended",
    "agent.failed",
    "agent.reported",
    "vault.asked",
    "vault.ask_answered",
    "session.following_paused",
    "session.following_resumed",
    "run.agent_folders_unchecked",
    "run.agent_folders_put_back",
    "remote.delivery_dropped",
];

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    after: Option<u64>,
    limit: Option<usize>,
    #[serde(default)]
    unread: bool,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    ids: Option<Vec<u64>>,
    session: Option<String>,
    all: Option<bool>,
}

pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "notifications.list" => list(
            daemon,
            match params {
                Value::Null => List::default(),
                params => serde_json::from_value(params)?,
            },
        ),
        "notifications.read" => read(daemon, serde_json::from_value(params)?),
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

fn list(daemon: &Daemon, asked: List) -> Result<Value, Failure> {
    let (rows, (needs_you, new)) = daemon.read(|store| {
        Ok((
            store.notifications(asked.after, asked.limit.unwrap_or(LAST), asked.unread)?,
            store.unread_notifications()?,
        ))
    })?;
    Ok(json!({"notifications": rows, "unread": {"needs_you": needs_you, "new": new}}))
}

fn read(daemon: &Daemon, asked: Read) -> Result<Value, Failure> {
    let which = match (&asked.ids, &asked.session, asked.all) {
        (Some(ids), None, None) => Which::Ids(ids),
        (None, Some(session), None) => Which::Session(session),
        (None, None, Some(true)) => Which::All,
        _ => {
            return Err(Failure::new(
                "invalid-params",
                "name what to mark read, one way: `ids`, `session`, or `all: true`",
            ));
        }
    };
    let changed = daemon.read(|store| store.read_notifications(which, now_ms()))?;
    changed_event(daemon, &changed, false, "read");
    Ok(json!({"read": changed}))
}

/// Tells the clients which notifications were added, resolved or read.
fn changed_event(daemon: &Daemon, ids: &[u64], pruned: bool, how: &str) {
    if !ids.is_empty() || pruned {
        let _ = daemon.emit(
            "notifications.changed",
            json!({"ids": ids, "pruned": pruned, "how": how}),
        );
    }
}

/// Follows the events until the daemon stops.
pub async fn run(daemon: Arc<Daemon>, mut stopping: watch::Receiver<bool>) {
    let Some(mut live) = daemon.live() else {
        return;
    };
    let kept = daemon.clone();
    let _ = tokio::task::spawn_blocking(move || keep(&kept)).await;
    loop {
        tokio::select! {
            received = live.recv() => {
                let wakes = match received {
                    Ok(event) => relevant(&event),
                    // Events were missed: the store has them.
                    Err(broadcast::error::RecvError::Lagged(_)) => true,
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if wakes {
                    let kept = daemon.clone();
                    let _ = tokio::task::spawn_blocking(move || keep(&kept)).await;
                }
            }
            _ = stopping.wait_for(|stopping| *stopping) => return,
        }
    }
}

/// Whether an event is one the centre makes rows of, or resolves them with. Every message piece
/// of every session goes by: only what matters wakes the keeper.
fn relevant(event: &Event) -> bool {
    EVENTS.contains(&event.name.as_str())
        || notification::forged(event).is_some()
        || crate::extensions::needs_you()
            .iter()
            .any(|(name, _, _)| *name == event.name)
}

/// Makes rows of the events since the last it saw, resolves what they answer, and forgets the
/// old. Called when an event that matters comes, and by the sweep.
pub fn keep(daemon: &Arc<Daemon>) {
    let Ok((cursor, upto)) =
        daemon.read(|store| Ok((store.notification_cursor()?, store.last_seq()?)))
    else {
        return;
    };
    // The migration set it: the centre starts with the events that come after it, what was told
    // before it existed is in the inbox and the window's rail, not in rows.
    let cursor = cursor.unwrap_or(upto);
    let mut names: Vec<&str> = EVENTS.to_vec();
    names.extend(["agent.tool_served"]);
    let extensions = crate::extensions::needs_you();
    names.extend(extensions.iter().map(|(event, _, _)| *event));
    let Ok(events) = daemon.read(|store| store.events_named(&names, cursor)) else {
        return;
    };
    let mut changed = Vec::new();
    for event in events.iter().filter(|event| event.seq <= upto) {
        changed.extend(note(daemon, event, &extensions));
    }
    let pruned = daemon
        .read(|store| store.prune_notifications(now_ms()))
        .is_ok_and(|pruned| pruned > 0);
    let _ = daemon.read(|store| store.set_notification_cursor(upto.max(cursor)));
    changed.sort_unstable();
    changed.dedup();
    changed_event(daemon, &changed, pruned, "kept");
}

/// The root of the agent's tree, its project and its workspace: the session a row belongs to.
fn session_of(daemon: &Daemon, agent: &str) -> (Option<String>, Option<String>, Option<String>) {
    let root = crate::tree::root_of(daemon, agent);
    match daemon.read(|store| store.agent(&root)).ok().flatten() {
        Some(Agent {
            id,
            project,
            workspace,
            ..
        }) => (Some(id), Some(project), workspace),
        None => (Some(root), None, None),
    }
}

/// Keeps one row: its number when it is new.
fn add(
    daemon: &Daemon,
    event: &Event,
    kind: &str,
    source: String,
    agent: Option<&str>,
    title: impl Into<String>,
    excerpt: impl Into<String>,
) -> Vec<u64> {
    let (session, project, workspace) = match agent {
        Some(agent) => {
            let (session, project, workspace) = session_of(daemon, agent);
            // An agent the store no longer knows: the event says where it ran.
            (
                session,
                project.or_else(|| event.data["project"].as_str().map(str::to_owned)),
                workspace.or_else(|| event.data["workspace"].as_str().map(str::to_owned)),
            )
        }
        None => (
            None,
            event.data["project"].as_str().map(str::to_owned),
            event.data["workspace"].as_str().map(str::to_owned),
        ),
    };
    let new = New {
        at: event.at,
        kind: kind.to_owned(),
        source,
        session,
        project,
        workspace,
        title: title.into(),
        excerpt: excerpt.into(),
    };
    daemon
        .read(|store| store.add_notification(&new))
        .ok()
        .flatten()
        .into_iter()
        .collect()
}

/// What an answer resolves: the number of the row when it was waiting.
fn resolve(daemon: &Daemon, source: &str, at: u64) -> Vec<u64> {
    daemon
        .read(|store| store.resolve_notification(source, at))
        .ok()
        .flatten()
        .into_iter()
        .collect()
}

/// A root agent: the session itself. A child's report, question and failure go to its parent, never
/// to the user.
fn root(daemon: &Daemon, id: Option<&str>) -> Option<String> {
    let id = id?;
    let agent = daemon.read(|store| store.agent(id)).ok().flatten()?;
    agent.parent.is_none().then_some(agent.id)
}

/// The rows an event makes or resolves.
fn note(
    daemon: &Arc<Daemon>,
    event: &Event,
    extensions: &[(&'static str, &'static str, crate::extensions::Says)],
) -> Vec<u64> {
    let data = &event.data;
    let agent = data["agent"].as_str();
    if let Some((_, kind, text)) = extensions.iter().find(|(name, _, _)| *name == event.name) {
        return add(
            daemon,
            event,
            kind,
            format!("event:{}", event.seq),
            None,
            "Needs you",
            notification::first_line(&text(data)),
        );
    }
    match event.name.as_str() {
        "approval.requested" | "approval.forwarded" => {
            let Some(number) = data["number"].as_u64() else {
                return Vec::new();
            };
            let held = daemon.read(|store| store.request(number)).ok().flatten();
            match held {
                // It waits for the user as long as the user holds it and nothing answered it: one
                // the judge or the user answered before the keeper looked never waited.
                Some(request)
                    if request.held_by == approval::USER
                        && request.state == approval::RequestState::Pending =>
                {
                    add(
                        daemon,
                        event,
                        "request",
                        format!("request:{number}"),
                        Some(request.agent.as_str()),
                        format!("Request #{number} waits for you"),
                        notification::excerpt(&notification::asked(&request.tool, &request.shown)),
                    )
                }
                _ => Vec::new(),
            }
        }
        "approval.decided" | "approval.cancelled" => match data["number"].as_u64() {
            Some(number) => resolve(daemon, &format!("request:{number}"), event.at),
            None => Vec::new(),
        },
        "agent.asked" if data["to"] == "user" => {
            let Some(id) = root(daemon, agent) else {
                return Vec::new();
            };
            let questions = flottille_core::question::of(data);
            let asked = questions
                .first()
                .map(|q| q.question.as_str())
                .unwrap_or_default();
            add(
                daemon,
                event,
                "question",
                format!("question:{id}:{}", event.seq),
                Some(id.as_str()),
                "Asks you a question",
                notification::first_line(asked),
            )
        }
        "agent.answered" | "agent.replaced" | "agent.resumed" | "agent.ended" | "agent.failed" => {
            let mut changed = match (event.name.as_str(), root(daemon, agent)) {
                ("agent.failed", Some(id)) => add(
                    daemon,
                    event,
                    "failure",
                    format!("event:{}", event.seq),
                    Some(id.as_str()),
                    "Failed",
                    notification::first_line(data["reason"].as_str().unwrap_or_default()),
                ),
                _ => Vec::new(),
            };
            if let Some(id) = agent {
                changed.extend(reconcile_questions(daemon, id, event.at));
            }
            changed
        }
        "agent.reported" => match root(daemon, agent) {
            Some(id) => add(
                daemon,
                event,
                "report",
                format!("event:{}", event.seq),
                Some(id.as_str()),
                "Reported",
                notification::first_line(data["text"].as_str().unwrap_or_default()),
            ),
            None => Vec::new(),
        },
        "vault.asked" => add(
            daemon,
            event,
            "vault",
            format!("vault:{}", event.seq),
            agent,
            "Asks you for a vault entry",
            notification::excerpt(data["name"].as_str().unwrap_or_default()),
        ),
        "vault.ask_answered" => match data["ask"].as_u64() {
            Some(ask) => resolve(daemon, &format!("vault:{ask}"), event.at),
            None => Vec::new(),
        },
        "session.following_paused" => add(
            daemon,
            event,
            "paused",
            format!(
                "paused:{}:{}:{}",
                agent.unwrap_or_default(),
                data["follower"].as_str().unwrap_or_default(),
                event.seq
            ),
            agent,
            "A session stopped waking",
            "It and the agent that started it talk without you",
        ),
        "session.following_resumed" => {
            // Every pause of that pair that still waits.
            let prefix = format!(
                "paused:{}:{}:",
                agent.unwrap_or_default(),
                data["follower"].as_str().unwrap_or_default()
            );
            let open = daemon
                .read(|store| store.unresolved_notifications("paused"))
                .unwrap_or_default();
            open.iter()
                .filter(|source| source.starts_with(&prefix))
                .flat_map(|source| resolve(daemon, source, event.at))
                .collect()
        }
        "run.agent_folders_unchecked" => add(
            daemon,
            event,
            "unchecked",
            format!("event:{}", event.seq),
            agent,
            "Agent folders were not all checked",
            notification::first_line(&format!(
                "Its run in {} ended",
                data["folder"].as_str().unwrap_or_default()
            )),
        ),
        "run.agent_folders_put_back" => add(
            daemon,
            event,
            "put_back",
            format!("event:{}", event.seq),
            agent,
            "Agent folders were put back",
            notification::first_line(&format!(
                "A run in {} wrote in them",
                data["folder"].as_str().unwrap_or_default()
            )),
        ),
        "remote.delivery_dropped" => add(
            daemon,
            event,
            "dropped",
            format!("event:{}", event.seq),
            None,
            "A message for another machine was dropped",
            notification::first_line(&crate::across::dropped_text(data)),
        ),
        "agent.tool_served" => match notification::forged(event) {
            Some(forged) => add(
                daemon,
                event,
                if forged.ticket {
                    "ticket"
                } else {
                    "pull_request"
                },
                format!("event:{}", event.seq),
                Some(forged.agent.as_str()),
                forged.said,
                forged.excerpt,
            ),
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// The questions of a root that nothing waits on any more are resolved: the store says which are
/// open, whatever event told the keeper to look (`tree::unanswered`).
fn reconcile_questions(daemon: &Arc<Daemon>, agent: &str, at: u64) -> Vec<u64> {
    let prefix = format!("question:{agent}:");
    let open: Vec<String> = daemon
        .read(|store| store.unresolved_notifications("question"))
        .unwrap_or_default()
        .into_iter()
        .filter(|source| source.starts_with(&prefix))
        .collect();
    if open.is_empty() {
        return Vec::new();
    }
    let final_agent = daemon
        .read(|store| store.agent(agent))
        .ok()
        .flatten()
        .is_none_or(|agent| agent.state.is_final());
    let waiting: Vec<u64> = match final_agent {
        true => Vec::new(),
        false => crate::tree::unanswered(daemon, agent)
            .unwrap_or_default()
            .iter()
            .filter_map(|question| question["seq"].as_u64())
            .collect(),
    };
    open.iter()
        .filter(|source| {
            let seq = source
                .rsplit(':')
                .next()
                .and_then(|seq| seq.parse::<u64>().ok());
            !seq.is_some_and(|seq| waiting.contains(&seq))
        })
        .flat_map(|source| resolve(daemon, source, at))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_session_message_piece_wakes_nothing_and_a_request_does() {
        let piece = Event {
            seq: 1,
            at: 1,
            name: "agent.message".into(),
            data: json!({"agent": "a1", "text": "hello"}),
        };
        assert!(!relevant(&piece));
        let request = Event {
            name: "approval.requested".into(),
            ..piece.clone()
        };
        assert!(relevant(&request));
        let served = Event {
            name: "agent.tool_served".into(),
            data: json!({"agent": "a1", "tool": "forge", "outcome": "ok",
                "arguments": {"action": "create", "title": "x"}}),
            ..piece.clone()
        };
        assert!(relevant(&served));
        let read = Event {
            data: json!({"agent": "a1", "tool": "forge", "outcome": "ok",
                "arguments": {"action": "list"}}),
            ..served
        };
        assert!(!relevant(&read));
    }
}
