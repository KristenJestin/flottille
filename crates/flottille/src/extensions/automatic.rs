//! Automatic subscriptions (`docs/extensions.md`, "Automatic subscriptions", `docs/design.md`
//! §5.6): the subjects a session is subscribed to without asking, set in Settings and per project,
//! each with the events it waits for, from people only. A pull request the session opens
//! (`forge create`, `forge::created`), the session's own pull request opened another way (seen
//! by the rounds of the items as `item.opened` on its workspace's branch), and the ticket it works
//! on (its workspace's, linked before it starts or while it runs). Only a session gets them: a root,
//! led or handed, never a child nor an extension's agents.

use std::sync::Arc;

use flottille_core::agent::Agent;
use flottille_core::store::now_ms;
use flottille_core::subscription::Subscription;
use serde_json::{Value, json};

use super::{Answered, Extension, Kind, Methods, Place, Scope, Setting};
use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::subscriptions;

/// A pull request the session opens with `forge create`.
pub const CREATED_PULL: &str = "created_pull";
/// The session's own pull request, opened on its workspace's branch another way.
pub const BRANCH_PULL: &str = "branch_pull";
/// The ticket the session works on: its workspace's.
pub const TICKET: &str = "ticket";

pub const EXTENSION: Extension = Extension {
    name: "autosubscriptions",
    title: "Automatic subscriptions",
    sentence: "Sessions hear of their pull request and their ticket without asking: people's \
               comments, reviews, the end of the checks wake them. It reads nothing more.",
    scope: Scope::Project,
    on_by_default: true,
    switch: "Subscribe sessions automatically",
    usage: "A session is subscribed when it opens a pull request, when one is opened on its \
            branch, and to its workspace's ticket, until they end. It says so in the \
            conversation, and **Cancel** on the session's list of subscriptions ends one.",
    place: Place::Row {
        icon: "bell",
        line: "What wakes a session, without it asking.",
        order: 5,
    },
    settings: &[
        Setting {
            key: CREATED_PULL,
            label: "A pull request the session opens",
            hint: "Until it is merged or closed",
            kind: Kind::Subset {
                of: &crate::forge::EVENTS,
                default: &crate::forge::EVENTS,
            },
            per_project: true,
        },
        Setting {
            key: BRANCH_PULL,
            label: "The session's pull request opened another way",
            hint: "Opened on its branch outside Flottille, until it is merged or closed",
            kind: Kind::Subset {
                of: &crate::forge::EVENTS,
                default: &crate::forge::EVENTS,
            },
            per_project: true,
        },
        Setting {
            key: TICKET,
            label: "The ticket the session works on",
            hint: "Its workspace's ticket, until it is closed",
            kind: Kind::Subset {
                of: &crate::tickets::EVENTS,
                default: &[],
            },
            per_project: true,
        },
    ],
    migrations: &[],
    methods: Methods {
        prefix: "autosubscriptions",
        call,
    },
    actions: &[],
    tools: &[],
    started: starts_no_session,
    heard: &[
        ("item.opened", pull_opened),
        ("agent.created", session_created),
        ("workspace.ticket", ticket_linked),
    ],
    fact: no_fact,
    launched: launches_none,
    commands: &[],
    guidance: None,
    declared: None,
    dynamic: None,
    needs_you: &[],
};

fn starts_no_session(_: &Daemon, _: &str) -> bool {
    false
}

fn no_fact(_: &Daemon, _: &str) -> Option<Value> {
    None
}

fn launches_none(_: &Daemon, _: &str) -> Option<&'static str> {
    None
}

/// `autosubscriptions.events {project}`: the events each subject subscribes a session of the
/// project to now, as Settings and the project set them (none while it is off there).
fn call(daemon: Arc<Daemon>, method: String, params: Value) -> Answered {
    Box::pin(async move {
        match method.as_str() {
            "autosubscriptions.events" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Asked {
                    project: String,
                }
                let Asked { project } = serde_json::from_value(params)?;
                crate::projects::load_project(&daemon, &project)?;
                let subjects: serde_json::Map<String, Value> = [CREATED_PULL, BRANCH_PULL, TICKET]
                    .iter()
                    .map(|subject| {
                        (
                            (*subject).to_owned(),
                            json!(events(&daemon, &project, subject)),
                        )
                    })
                    .collect();
                Ok(json!({"project": project, "subjects": subjects}))
            }
            method => Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            )),
        }
    })
}

/// The events `subject` subscribes a session of `project` to: none while it is off there, or when
/// its file cannot be read.
pub fn events(daemon: &Daemon, project: &str, subject: &str) -> Vec<String> {
    let Ok(project) = super::active(daemon, &EXTENSION, project) else {
        return Vec::new();
    };
    super::value(daemon, &EXTENSION, &project, subject)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// Whether the agent is one a subject subscribes: a session (a root, led or handed), never a
/// child nor an extension's agent, which have a parent.
fn is_session(agent: &Agent) -> bool {
    agent.parent.is_none() && !agent.state.is_final()
}

/// The sessions working in the workspace `workspace` of `project`.
fn sessions_in(daemon: &Daemon, project: &str, workspace: &str) -> Vec<Agent> {
    daemon
        .read(|store| store.agents())
        .unwrap_or_default()
        .into_iter()
        .filter(|agent| {
            agent.project == project
                && agent.workspace.as_deref() == Some(workspace)
                && is_session(agent)
        })
        .collect()
}

/// The events as the agent reads them: `checks, approvals, reviews, people's comments`. The end of
/// the target is said by `until`, unless it is all the subscription waits for.
pub fn said(events: &[String]) -> String {
    const WORDS: [(&str, &str); 6] = [
        ("checks", "checks"),
        ("approved", "approvals"),
        ("review", "reviews"),
        ("comment", "people's comments"),
        ("commented", "people's comments"),
        ("status", "status changes"),
    ];
    const MORE: [(&str, &str); 1] = [("assigned", "assignments")];
    const ENDS: [(&str, &str); 2] = [("merged", "its merge"), ("closed", "its closing")];
    let words = |table: &[(&str, &'static str)]| -> Vec<&'static str> {
        table
            .iter()
            .filter(|(event, _)| events.iter().any(|e| e == event))
            .map(|(_, word)| *word)
            .collect()
    };
    let mut found = words(&WORDS);
    found.extend(words(&MORE));
    if found.is_empty() {
        found = words(&ENDS);
    }
    found.join(", ")
}

/// What an automatic subscription to a pull request says, where it starts.
pub fn pull_sentence(events: &[String], tool: &str) -> String {
    format!(
        "You are subscribed to it until it is merged or closed: {}. {tool}unsubscribe to stop.",
        said(events)
    )
}

/// The Core event trigger on `item.opened`: a pull request opened on the branch of a session's
/// workspace another way than its `forge create` subscribes the sessions there, and tells them.
fn pull_opened(daemon: &Arc<Daemon>, data: &Value) {
    if data["kind"] != "pull" {
        return;
    }
    let (Some(project), Some(key), Some(repository)) = (
        data["project"].as_str(),
        data["key"].as_str(),
        data["repository"].as_str(),
    ) else {
        return;
    };
    let filter = flottille_core::item::Filter {
        kind: Some("pull".to_owned()),
        ..Default::default()
    };
    let Some(pull) = daemon
        .read(|store| store.items(project, &filter))
        .ok()
        .into_iter()
        .flatten()
        .find(|pull| pull.key == key && pull.repository == repository)
    else {
        return;
    };
    let head = pull.details["head"].as_str().unwrap_or_default().to_owned();
    // One an agent opens, or reads to subscribe to, is its own call's to subscribe.
    if head.is_empty() || crate::forge::owned(daemon, project, repository, &head) {
        return;
    }
    let Ok(number) = key.parse::<u64>() else {
        return;
    };
    let events = events(daemon, project, BRANCH_PULL);
    if events.is_empty() || matches!(pull.state.as_str(), "merged" | "closed") {
        return;
    }
    let Ok(workspaces) = daemon.read(|store| store.workspaces(Some(project))) else {
        return;
    };
    for workspace in crate::forge::workspaces_of(&pull, workspaces) {
        let target = json!({"project": project, "repository": repository, "number": number,
            "head": head, "base": pull.details["base"], "workspace": workspace.name});
        for session in sessions_in(daemon, project, &workspace.name) {
            let theirs = subscriptions::of(daemon, &session.id);
            if theirs
                .iter()
                .any(|s| s.source == "forge" && crate::forge::same_pull(&s.target, &target))
            {
                continue;
            }
            let subscription = new(&session.id, "forge", target.clone(), &events);
            if subscriptions::subscribe(daemon, &subscription).is_err() {
                continue;
            }
            super::tell(
                daemon,
                &EXTENSION,
                &session.id,
                &format!(
                    "The {} was opened on your branch {head} outside Flottille. {}",
                    crate::forge::target_named(&target),
                    pull_sentence(&events, "forge ")
                ),
            );
        }
    }
}

/// Whether the subscription is one of these (`new`), not one the agent made itself.
fn is_automatic(subscription: &Subscription) -> bool {
    subscription.seen["automatic"] == true
}

/// A subscription of `agent` to `target`, from people, until the target ends. It says it is
/// automatic in `seen`, which no source writes any more, so that what ends it later leaves the
/// agent's own subscriptions alone.
fn new(agent: &str, source: &str, target: Value, events: &[String]) -> Subscription {
    Subscription {
        id: subscriptions::new_subscription_id(),
        agent: agent.to_owned(),
        source: source.into(),
        target,
        events: events.to_vec(),
        from: "people".into(),
        seen: json!({"automatic": true}),
        created_at: now_ms(),
        expires_at: subscriptions::NO_DEADLINE,
    }
}

/// The Core event trigger on `agent.created`: a session that starts in a workspace linked to a
/// ticket is subscribed to it, told with its first prompt.
fn session_created(daemon: &Arc<Daemon>, data: &Value) {
    let (Some(agent), Some(project), Some(workspace)) = (
        data["agent"].as_str(),
        data["project"].as_str(),
        data["workspace"].as_str(),
    ) else {
        return;
    };
    if !data["parent"].is_null() {
        return;
    }
    let Ok(Some(workspace)) = daemon.read(|store| store.workspace(project, workspace)) else {
        return;
    };
    let Some(ticket) = workspace.ticket else {
        return;
    };
    let Ok(Some(session)) = daemon.read(|store| store.agent(agent)) else {
        return;
    };
    if let Some(told) = subscribe_ticket(daemon, &session, &ticket) {
        // Not rung: its process is not started yet, and the delivery goes with its first prompt.
        crate::deliveries::queue(daemon, &session.id, EXTENSION.name, None, &told);
    }
}

/// The Core event trigger on `workspace.ticket`: the sessions of a workspace linked to a ticket
/// are subscribed to it, and told; when the ticket changes or is unlinked, what these subscribed
/// them to of the ticket before ends (the agent's own subscriptions stay).
fn ticket_linked(daemon: &Arc<Daemon>, data: &Value) {
    let (Some(project), Some(workspace)) = (data["project"].as_str(), data["workspace"].as_str())
    else {
        return;
    };
    let ticket = data["ticket"].as_str();
    let now = ticket.and_then(|ticket| {
        let project = crate::projects::load_project(daemon, project).ok()?;
        ticket_target(&project, ticket)
    });
    for session in sessions_in(daemon, project, workspace) {
        for old in subscriptions::of(daemon, &session.id) {
            if old.source == subscriptions::TICKETS
                && is_automatic(&old)
                && !now
                    .as_ref()
                    .is_some_and(|now| same_ticket(&old.target, now))
            {
                subscriptions::end(daemon, &old, "cancelled");
            }
        }
        if let Some(ticket) = ticket
            && let Some(told) = subscribe_ticket(daemon, &session, ticket)
        {
            super::tell(daemon, &EXTENSION, &session.id, &told);
        }
    }
}

/// The target of a subscription to the ticket `ticket` (`#5`, `ABC-1`): none for one of no tracker
/// of the project's.
fn ticket_target(project: &flottille_core::project::Project, ticket: &str) -> Option<Value> {
    let key = crate::tickets::accepted(project, ticket).ok()?;
    let (repository, item) = crate::tickets::item_of(project, &key)?;
    Some(json!({"project": project.name, "repository": repository, "key": item}))
}

/// Whether two targets are the same ticket.
fn same_ticket(one: &Value, other: &Value) -> bool {
    ["project", "repository", "key"]
        .iter()
        .all(|key| one[key] == other[key])
}

/// Subscribes the session to the ticket `ticket` (`#5`, `ABC-1`) with the project's events: what it
/// is told, or none when nothing was subscribed (no event set, a ticket of no tracker of the
/// project's, one already closed, one it is subscribed to already: its own or this one's).
fn subscribe_ticket(daemon: &Daemon, session: &Agent, ticket: &str) -> Option<String> {
    let events = events(daemon, &session.project, TICKET);
    if events.is_empty() {
        return None;
    }
    let project = crate::projects::load_project(daemon, &session.project).ok()?;
    let target = ticket_target(&project, ticket)?;
    let (repository, item) = (
        target["repository"].as_str().unwrap_or_default(),
        target["key"].as_str().unwrap_or_default(),
    );
    let filter = flottille_core::item::Filter {
        kind: Some("ticket".to_owned()),
        ..Default::default()
    };
    let closed = daemon
        .read(|store| store.items(&project.name, &filter))
        .ok()
        .into_iter()
        .flatten()
        .any(|stored| {
            stored.repository == repository && stored.key == item && stored.state == "closed"
        });
    if closed {
        return None;
    }
    // What the agent subscribed to itself stays as it made it, and what is so already says nothing
    // again.
    if subscriptions::of(daemon, &session.id)
        .iter()
        .any(|old| old.source == subscriptions::TICKETS && same_ticket(&old.target, &target))
    {
        return None;
    }
    let subscription = new(&session.id, subscriptions::TICKETS, target, &events);
    subscriptions::subscribe(daemon, &subscription).ok()?;
    Some(format!(
        "You are subscribed to the {} you work on until it is closed: {}. tickets unsubscribe to \
         stop.",
        crate::tickets::target_named(&subscription.target),
        said(&events)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(events: &[&str]) -> String {
        said(&events.iter().map(|e| (*e).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn a_child_of_another_machine_s_agent_is_no_session_to_subscribe() {
        assert!(!is_session(
            &crate::tree::tests::a_child_of_another_machine()
        ));
        assert!(is_session(&crate::daemon::tests::an_agent("a1")));
    }

    #[test]
    fn the_events_are_said_in_words_the_end_left_to_until() {
        assert_eq!(
            words(&crate::forge::EVENTS),
            "checks, approvals, reviews, people's comments"
        );
        assert_eq!(words(&["merged"]), "its merge");
        assert_eq!(
            words(&["commented", "status", "assigned", "closed"]),
            "people's comments, status changes, assignments"
        );
    }
}
