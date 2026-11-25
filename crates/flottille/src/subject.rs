//! What a session is about (`docs/design.md` §10, "What a session is about"): derived by the
//! daemon from what the session took in hand, never asked of the forge nor of the agent. The
//! latest ticket it took in hand wins (its workspace's linked ticket, then one it changed or
//! subscribed to); a pull request it opened is the subject when it holds no ticket; a ticket it
//! only read counts while nothing it holds is open; once all it held is done, it is `free`, and
//! says what it last held. A change of the subject is an `agent.updated` event.

use std::sync::Arc;

use flottille_core::agent::Agent;
use flottille_core::item::Item;
use flottille_core::ticket::Key;
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use crate::daemon::Daemon;

/// Where a ticket came from, as the session's page says it (`What it is about, from …`).
const FROM_WORKSPACE: &str = "the workspace's linked ticket";
const FROM_CHANGED: &str = "a ticket it changed";
const FROM_SUBSCRIBED: &str = "a ticket it subscribed to";
const FROM_READ: &str = "a ticket it read";
const FROM_OPENED: &str = "a pull request it opened";
const FROM_FREE: &str = "what it last held";

/// The `tickets` actions that take a ticket in hand: a write on it, or a subscription.
const TAKES: &[&str] = &[
    "comment",
    "edit",
    "close",
    "reopen",
    "status",
    "log_time",
    "subscribe",
];

/// The session's subject now, as `agent.list` and the tree carry it: `null` while it never held
/// anything.
pub fn shown(daemon: &Daemon, agent: &Agent) -> Value {
    daemon
        .read(|store| {
            let (held, _) = store.agent_subject(&agent.id)?;
            let workspace = match &agent.workspace {
                Some(name) => store.workspace(&agent.project, name)?,
                None => None,
            };
            let ticket = |key: &str| {
                let key = Key::parse(key).ok()?;
                let stored = match &key {
                    Key::Issue(number) => number.to_string(),
                    Key::Jira(key) => key.clone(),
                };
                store
                    .item_by_key(&agent.project, "ticket", None, &stored)
                    .ok()
                    .flatten()
            };
            let pull = |held: &Value| -> Option<Item> {
                let branch = held["branch"].as_str()?;
                let repositories: Vec<String> =
                    serde_json::from_value(held["repositories"].clone()).unwrap_or_default();
                store
                    .pull_of_branch(&agent.project, branch, &repositories)
                    .ok()
                    .flatten()
            };
            let held_pull = pull(&held["pull"]);
            // What its row shows of a pull request: the one it opened, else its branch's.
            let branch_pull = workspace.as_ref().and_then(|workspace| {
                pull(&json!({"branch": workspace.branch,
                    "repositories": workspace.repositories.iter().map(|r| &r.path).collect::<Vec<_>>()}))
            });
            let shown_pull = held_pull.as_ref().or(branch_pull.as_ref());
            Ok(derive(
                &held,
                workspace.and_then(|w| w.ticket),
                &ticket,
                held_pull.as_ref(),
                shown_pull,
            ))
        })
        .unwrap_or(Value::Null)
}

/// The subject of what is held, the items being looked up by `ticket`.
fn derive(
    held: &Value,
    linked: Option<String>,
    ticket: &dyn Fn(&str) -> Option<Item>,
    held_pull: Option<&Item>,
    shown_pull: Option<&Item>,
) -> Value {
    let pull_request = shown_pull.map(pull_json);
    // The tickets in hand, the latest first.
    let mut tickets: Vec<(String, &str)> = Vec::new();
    if let Some(key) = held["ticket"]["key"].as_str() {
        tickets.push((
            key.to_owned(),
            held["ticket"]["from"].as_str().unwrap_or(FROM_CHANGED),
        ));
    }
    if let Some(key) = linked
        && !tickets.iter().any(|(held, _)| *held == key)
    {
        tickets.push((key, FROM_WORKSPACE));
    }
    let open = |key: &str| {
        let item = ticket(key);
        let done = item.as_ref().is_some_and(|item| item.state == "closed");
        (!done).then_some(item)
    };
    let of_ticket = |key: &str, item: Option<Item>, from: &str| {
        json!({"kind": "ticket", "key": key, "title": item.map(|item| item.title),
            "from": from, "pull_request": pull_request})
    };
    for (key, from) in &tickets {
        if let Some(item) = open(key) {
            return of_ticket(key, item, from);
        }
    }
    if let Some(pull) = held_pull.filter(|pull| matches!(pull.state.as_str(), "open" | "draft")) {
        return json!({"kind": "pull_request", "key": format!("#{}", pull.key),
            "title": pull.title, "from": FROM_OPENED, "pull_request": pull_request});
    }
    if let Some(key) = held["read"]["key"].as_str()
        && let Some(item) = open(key)
    {
        return of_ticket(key, item, FROM_READ);
    }
    let mut last: Vec<String> = tickets
        .iter()
        .map(|(key, _)| format!("{key} done"))
        .collect();
    if let Some(pull) = held_pull {
        last.push(format!("#{} {}", pull.key, pull.state));
    }
    if last.is_empty() {
        return Value::Null;
    }
    json!({"kind": "free", "key": null, "title": null, "from": FROM_FREE,
        "last": last.join(" · "), "pull_request": pull_request})
}

/// A pull request as a subject carries it: its number, title, state and one mark of its checks.
fn pull_json(pull: &Item) -> Value {
    let any = |state: &str| pull.checks.iter().any(|check| check.state == state);
    let checks = if any("failing") {
        Some("failing")
    } else if any("pending") {
        Some("pending")
    } else if any("passing") {
        Some("passing")
    } else {
        None
    };
    json!({"key": format!("#{}", pull.key), "repository": pull.repository,
        "title": pull.title, "state": pull.state, "url": pull.url, "checks": checks})
}

/// A tool call was served: what it took in hand is kept, and a change of the subject told.
pub fn served(daemon: &Daemon, agent: &str, tool: &str, args: &Value, done: bool) {
    if !done || !matches!(tool, "tickets" | "forge") {
        return;
    }
    let Ok(Some(found)) = daemon.read(|store| store.agent(agent)) else {
        return;
    };
    let action = args["action"].as_str().unwrap_or_default();
    let workspace = || {
        let name = args["workspace"]
            .as_str()
            .map(str::to_owned)
            .or(found.workspace.clone())?;
        daemon
            .read(|store| store.workspace(&found.project, &name))
            .ok()
            .flatten()
    };
    let (mut held, _) = daemon
        .read(|store| store.agent_subject(agent))
        .unwrap_or((Value::Null, Value::Null));
    if !held.is_object() {
        held = json!({});
    }
    match tool {
        "tickets" if TAKES.contains(&action) || action == "show" => {
            let key = match args["ticket"].as_str() {
                Some(text) => Key::parse(text).ok().map(|key| key.text()),
                None => workspace().and_then(|workspace| workspace.ticket),
            };
            let Some(key) = key else { return };
            if action == "show" {
                held["read"] = json!({"key": key});
            } else {
                // Another ticket than the one in hand: the pull request it held was that one's.
                let before = held["ticket"]["key"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| workspace()?.ticket);
                if before.as_deref() != Some(key.as_str()) {
                    held["pull"] = Value::Null;
                }
                let from = if action == "subscribe" {
                    FROM_SUBSCRIBED
                } else {
                    FROM_CHANGED
                };
                held["ticket"] = json!({"key": key, "from": from});
            }
        }
        "forge" if action == "create" => {
            let Some(workspace) = workspace() else { return };
            let repositories: Vec<String> = match args["repository"].as_str() {
                Some(repository) => vec![repository.to_owned()],
                None => workspace
                    .repositories
                    .iter()
                    .map(|r| r.path.clone())
                    .collect(),
            };
            held["pull"] = json!({"branch": workspace.branch, "repositories": repositories});
        }
        _ => return,
    }
    if let Err(error) = daemon.write(|store| {
        store.set_agent_held(agent, &held)?;
        Ok(((), None::<flottille_core::protocol::Event>))
    }) {
        eprintln!("flottille daemon: agent {agent}: its subject was not kept: {error}");
    }
    refresh(daemon, &found);
}

/// One refresh at a time: a call served and a round of the items can both see the same change,
/// which is told once.
static REFRESHING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Tells `agent.updated` when the subject is no longer what was last said.
pub fn refresh(daemon: &Daemon, agent: &Agent) {
    let _one = REFRESHING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = shown(daemon, agent);
    let Ok((_, said)) = daemon.read(|store| store.agent_subject(&agent.id)) else {
        return;
    };
    if said == now {
        return;
    }
    if let Err(error) = daemon.write(|store| {
        let event = store.set_agent_subject_shown(&agent.id, &now)?;
        Ok(((), Some(event)))
    }) {
        eprintln!("flottille daemon: agent {}: {error}", agent.id);
    }
}

/// Follows the events that can change a subject without a call of its session (a round of the
/// items, a ticket linked to a workspace, a session created), until the daemon stops.
pub async fn run(daemon: Arc<Daemon>, mut stopping: watch::Receiver<bool>) {
    let Some(mut live) = daemon.live() else {
        return;
    };
    loop {
        let event = tokio::select! {
            received = live.recv() => match received {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = async { let _ = stopping.wait_for(|stopping| *stopping).await; } => return,
        };
        let concerns = event.name.starts_with("item.")
            || matches!(
                event.name.as_str(),
                "workspace.ticket" | "agent.created" | "agent.moved"
            );
        if !concerns {
            continue;
        }
        let daemon = daemon.clone();
        let _ = tokio::task::spawn_blocking(move || settle(&daemon, &[event])).await;
    }
}

/// Refreshes the sessions `events` can concern; how many refreshes that took.
fn settle(daemon: &Daemon, events: &[flottille_core::protocol::Event]) -> usize {
    let Ok(agents) = daemon.read(|store| store.agents()) else {
        return 0;
    };
    let mut refreshed = 0;
    for event in events {
        let name = &event.name;
        let data = &event.data;
        for agent in agents.iter().filter(|agent| !agent.state.is_final()) {
            let concerned = match data["agent"].as_str() {
                Some(id) if name.starts_with("agent.") => agent.id == id,
                _ => data["project"] == agent.project.as_str(),
            };
            if concerned {
                refresh(daemon, agent);
                refreshed += 1;
            }
        }
    }
    refreshed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, key: &str, title: &str, state: &str) -> Item {
        Item {
            project: "app".into(),
            kind: kind.into(),
            source: "github".into(),
            repository: "api".into(),
            key: key.into(),
            title: title.into(),
            state: state.into(),
            author: None,
            assignees: Vec::new(),
            labels: Vec::new(),
            url: None,
            updated_at: None,
            seen_at: 0,
            mine: false,
            details: json!({}),
            checks: Vec::new(),
        }
    }

    fn event(name: &str, data: Value) -> flottille_core::protocol::Event {
        flottille_core::protocol::Event {
            seq: 0,
            at: 0,
            name: name.into(),
            data,
        }
    }

    #[tokio::test]
    async fn the_list_says_the_subject_kept_without_deriving_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::daemon::tests::an_agent("c1");
        let daemon = crate::tree::tests::a_daemon_with(dir.path(), &agent);
        // Kept as last said; what it holds would derive nothing.
        let kept = json!({"kind": "ticket", "key": "#7", "title": "Kept", "from": FROM_CHANGED,
            "pull_request": null});
        daemon
            .write(|store| Ok(((), Some(store.set_agent_subject_shown("c1", &kept)?))))
            .unwrap();
        assert_eq!(crate::tree::summary(&daemon, &agent)["subject"], kept);
    }

    #[tokio::test]
    async fn a_burst_of_item_events_refreshes_each_session_once() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::daemon::tests::an_agent("c1");
        let daemon = crate::tree::tests::a_daemon_with(dir.path(), &agent);
        let burst: Vec<_> = (0..40)
            .map(|at| {
                event(
                    if at % 2 == 0 {
                        "item.checks"
                    } else {
                        "item.changed"
                    },
                    json!({"project": "app", "kind": if at % 3 == 0 { "ticket" } else { "pull" },
                        "key": at.to_string()}),
                )
            })
            .collect();
        assert_eq!(settle(&daemon, &burst), 1);
        // Another project's items, or an item that is neither a ticket nor a pull request.
        let others = [
            event(
                "item.changed",
                json!({"project": "web", "kind": "ticket", "key": "1"}),
            ),
            event(
                "item.changed",
                json!({"project": "app", "kind": "review", "key": "2"}),
            ),
        ];
        assert_eq!(settle(&daemon, &others), 0);
    }

    #[tokio::test]
    async fn two_calls_of_one_session_served_at_once_both_keep_what_they_took() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::daemon::tests::an_agent("c1");
        let daemon = crate::tree::tests::a_daemon_with(dir.path(), &agent);
        for round in 0..300 {
            daemon
                .write(|store| {
                    store.set_agent_held("c1", &json!({}))?;
                    Ok(((), None::<flottille_core::protocol::Event>))
                })
                .unwrap();
            let start = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                for (action, ticket) in [("show", "#3"), ("subscribe", "#7")] {
                    let (start, daemon) = (&start, &daemon);
                    scope.spawn(move || {
                        start.wait();
                        let args = json!({"action": action, "ticket": ticket});
                        served(daemon, "c1", "tickets", &args, true);
                    });
                }
            });
            let (held, _) = daemon.read(|store| store.agent_subject("c1")).unwrap();
            assert_eq!(held["read"]["key"], "#3", "round {round}: {held}");
            assert_eq!(held["ticket"]["key"], "#7", "round {round}: {held}");
        }
    }

    #[test]
    fn a_session_that_never_held_anything_has_no_subject() {
        let none = |_: &str| None;
        assert_eq!(derive(&json!({}), None, &none, None, None), Value::Null);
    }

    #[test]
    fn a_held_ticket_wins_over_the_workspace_s_until_it_is_done() {
        let closed = std::cell::Cell::new(false);
        let ticket = |key: &str| {
            let state = if key == "#7" && closed.get() {
                "closed"
            } else {
                "open"
            };
            Some(item(
                "ticket",
                key.trim_start_matches('#'),
                "A ticket",
                state,
            ))
        };
        let held = json!({"ticket": {"key": "#7", "from": FROM_CHANGED}});
        let subject = derive(&held, Some("#5".into()), &ticket, None, None);
        assert_eq!(subject["key"], "#7");
        closed.set(true);
        let subject = derive(&held, Some("#5".into()), &ticket, None, None);
        assert_eq!(subject["key"], "#5");
        assert_eq!(subject["from"], FROM_WORKSPACE);
    }
}
