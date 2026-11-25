//! A session blocked by someone else (`docs/design.md` §10, "Blocked by others"): at rest, it
//! waits through one of its open subscriptions on a person, a review asked of someone else than
//! the user and not given yet, or a ticket assigned to someone else. Derived from the items the
//! daemon keeps, never set by hand and never asked of the forge: the session's line of the tree
//! says it (`blocked`), and `agent.blocked` says each change.

use std::collections::HashMap;
use std::sync::Arc;

use flottille_core::agent::{Agent, AgentState};
use flottille_core::store::Store;
use flottille_core::subscription::Subscription;
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use crate::daemon::Daemon;
use crate::subscriptions::TICKETS;

/// The events of a pull request a person makes happen: a subscription to the checks alone waits
/// on no one.
const BY_PEOPLE: [&str; 3] = ["review", "approved", "comment"];

/// What the session's line says: `{by, why, since}`, or `null` while it is not blocked.
pub fn of(daemon: &Daemon, agent: &Agent) -> Value {
    if agent.state != AgentState::Idle || agent.waiting_on_answer {
        return Value::Null;
    }
    let asking = daemon
        .read(|store| store.pending_requests_of(&agent.id))
        .is_ok_and(|requests| !requests.is_empty());
    if asking {
        return Value::Null;
    }
    let me = crate::items::me_names(daemon);
    crate::subscriptions::of(daemon, &agent.id)
        .iter()
        .find_map(|subscription| {
            let (by, why) = waits_on(daemon, subscription, &me)?;
            // Since it came to rest with the subscription open.
            let since = agent
                .exchanged_at
                .unwrap_or(agent.updated_at)
                .max(subscription.created_at);
            Some(json!({"by": by, "why": why, "since": since}))
        })
        .unwrap_or(Value::Null)
}

/// The people `subscription` waits on and what for (`review of #12`, `ticket ABC-1`); none when
/// it waits on no one.
fn waits_on(
    daemon: &Daemon,
    subscription: &Subscription,
    me: &[String],
) -> Option<(Vec<String>, String)> {
    let target = &subscription.target;
    let project = target["project"].as_str()?;
    let repository = target["repository"].as_str().unwrap_or_default();
    let (item, why) = match subscription.source.as_str() {
        "forge"
            if subscription
                .events
                .iter()
                .any(|e| BY_PEOPLE.contains(&e.as_str())) =>
        {
            let number = target["number"].as_u64()?;
            let key = number.to_string();
            let item = daemon.read(|store| store.item(project, "pull", repository, &key));
            (item.ok()??, format!("review of #{number}"))
        }
        TICKETS => {
            let key = target["key"].as_str()?;
            let item = daemon.read(|store| store.item(project, "ticket", repository, key));
            let why = format!("ticket {}", crate::tickets::reference_of(target));
            (item.ok()??, why)
        }
        _ => return None,
    };
    if !matches!(item.state.as_str(), "open" | "draft") {
        return None;
    }
    let named: Vec<String> = match item.kind.as_str() {
        "pull" => match item.details["requested"].as_array() {
            Some(requested) => requested
                .iter()
                .filter_map(|who| match who.as_str() {
                    // As a round before the login was kept stored it.
                    Some(name) => Some((name.to_owned(), String::new())),
                    None => Some((
                        who["name"].as_str()?.to_owned(),
                        who["login"].as_str().unwrap_or_default().to_owned(),
                    )),
                })
                .collect(),
            // Bitbucket keeps its reviewers with their verdicts.
            None => item.details["reviewers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|reviewer| reviewer["state"] == "pending")
                .filter_map(|reviewer| {
                    let handle = reviewer["handle"].as_str().unwrap_or_default();
                    Some((reviewer["name"].as_str()?.to_owned(), handle.to_owned()))
                })
                .collect(),
        },
        // One of the user's assigned to them blocks nothing; one they reported, assigned to
        // someone else, does. Stored before Jira was asked which (no `assigned_to_me`), one of
        // theirs blocks nothing.
        _ if item.source == "jira" && item.mine && item.details["assigned_to_me"] != false => {
            Vec::new()
        }
        _ => item
            .assignees
            .iter()
            .map(|name| (name.clone(), String::new()))
            .collect(),
    }
    .into_iter()
    .filter(|(name, handle)| !me.contains(name) && !me.contains(handle))
    .map(|(name, _)| name)
    // A person, by a name: never an e-mail.
    .filter(|name| !name.is_empty() && !name.contains('@'))
    .fold(Vec::new(), |mut names, name| {
        if !names.contains(&name) {
            names.push(name);
        }
        names
    });
    (!named.is_empty()).then_some((named, why))
}

/// Whether an event may change whether a session is blocked: what its items, its subscriptions,
/// its turns and its asks become.
fn moves(name: &str) -> bool {
    name.starts_with("item.")
        || name.starts_with("subscription.")
        || name.starts_with("approval.")
        || matches!(
            name,
            "items.forgotten"
                | "agent.prompted"
                | "agent.started"
                | "agent.turn_ended"
                | "agent.released"
                | "agent.asked"
                | "agent.answered"
                | "agent.resumed"
                | "agent.ended"
                | "agent.failed"
                | "agent.died"
        )
}

/// What each live session's `blocked` is now, by its id.
fn now(daemon: &Daemon) -> HashMap<String, Value> {
    daemon
        .read(Store::agents)
        .unwrap_or_default()
        .iter()
        .filter(|agent| !agent.state.is_final())
        .map(|agent| (agent.id.clone(), of(daemon, agent)))
        .collect()
}

/// Says each change of a session's `blocked` with `agent.blocked`, until the daemon stops. What
/// it is at the start is no change: the line of the tree says it.
pub async fn run(daemon: Arc<Daemon>, mut stopping: watch::Receiver<bool>) {
    let Some(mut live) = daemon.live() else {
        return;
    };
    let first = daemon.clone();
    let Ok(mut known) = tokio::task::spawn_blocking(move || now(&first)).await else {
        return;
    };
    loop {
        let mut moved = tokio::select! {
            received = live.recv() => match received {
                Ok(event) => moves(&event.name),
                Err(broadcast::error::RecvError::Lagged(_)) => true,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            // What a round stores may change no event says: a reviewer asked or withdrawn.
            () = daemon.items.stored.notified() => true,
            _ = async { let _ = stopping.wait_for(|stopping| *stopping).await; } => return,
        };
        // A round says hundreds of `item.*` at once: one reading for all that is queued.
        loop {
            match live.try_recv() {
                Ok(event) => moved |= moves(&event.name),
                Err(broadcast::error::TryRecvError::Lagged(_)) => moved = true,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => return,
            }
        }
        if !moved {
            continue;
        }
        let read = daemon.clone();
        let Ok(found) = tokio::task::spawn_blocking(move || now(&read)).await else {
            return;
        };
        for (agent, blocked) in &found {
            let was = known.get(agent).unwrap_or(&Value::Null);
            if was != blocked {
                let _ = daemon.emit("agent.blocked", json!({"agent": agent, "blocked": blocked}));
            }
        }
        // A session that ended is blocked by no one: its end says it.
        known = found;
    }
}
