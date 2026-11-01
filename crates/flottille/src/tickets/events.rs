//! The tickets as a source of events for the subscriptions (`crate::subscriptions`): what the
//! items' rounds and live reads emit of a ticket (`docs/items.md`, "Events") is offered to the
//! subscriptions to it, as the events they name, and `tickets subscribe` and `unsubscribe`, the
//! same shape as `forge`'s. A subscription reads nothing of its own.

use std::sync::Arc;

use flottille_core::declared::ProjectTracker;
use flottille_core::item::Item;
use flottille_core::project::Project;
use flottille_core::store::now_ms;
use flottille_core::subscription::Subscription;
use flottille_core::ticket::Key;
use serde_json::{Value, json};

use super::{Ticket, TicketsCall};
use crate::daemon::Daemon;
use crate::forge::BY_AGENT;
use crate::mcp::Outcome;
use crate::place::Place;
use crate::subscriptions::{self, By, Event, TICKETS};

/// The events a ticket has, as a subscription names them.
pub const EVENTS: [&str; 4] = ["commented", "status", "assigned", "closed"];

/// The ticket a subscription waits on, as the agent writes it: `#5`, `ABC-1`.
pub fn reference_of(target: &Value) -> String {
    let key = target["key"].as_str().unwrap_or_default();
    match target["repository"].as_str().unwrap_or_default() {
        "" => key.to_owned(),
        _ => format!("#{key}"),
    }
}

/// A subscription's target, as a delivery and the window say it: `ticket #5 of api` (the project
/// for a repository that is its folder), `ticket ABC-1`.
pub fn target_named(target: &Value) -> String {
    let reference = reference_of(target);
    match target["repository"].as_str().unwrap_or_default() {
        "" => format!("ticket {reference}"),
        "." => format!(
            "ticket {reference} of {}",
            target["project"].as_str().unwrap_or_default()
        ),
        repository => format!("ticket {reference} of {repository}"),
    }
}

/// What the item of `key` is in the project's tracker: its repository (none for Jira) and key.
fn item_of(project: &Project, key: &Key) -> Option<(String, String)> {
    match (project.tracker.as_ref()?, key) {
        (ProjectTracker::Github { repository }, Key::Issue(number)) => {
            Some((repository.clone(), number.to_string()))
        }
        (ProjectTracker::Jira { .. }, Key::Jira(key)) => Some((String::new(), key.clone())),
        _ => None,
    }
}

/// A ticket read live, as an item: its state is said for GitHub alone (a Jira status is in its
/// details, its state is kept), its comments counted for GitHub alone, as the rounds count them.
fn fetched_of(project: &Project, key: &Key, ticket: &Ticket) -> Option<crate::items::Fetched> {
    let (repository, key) = item_of(project, key)?;
    let jira = repository.is_empty();
    let details = match jira {
        true => json!({"status": ticket.state}),
        false => json!({"comments": ticket.comments.len() + ticket.older}),
    };
    let comments = ticket
        .comments
        .iter()
        .map(|comment| crate::items::Said {
            author: comment.author.clone(),
            bot: crate::forge::is_bot(&json!({"login": comment.author})),
            by_agent: comment.agent,
            state: None,
            excerpt: crate::items::excerpt(&comment.body),
            body: comment.body.clone(),
        })
        .collect();
    Some(crate::items::Fetched {
        item: Item {
            project: project.name.clone(),
            kind: "ticket".into(),
            source: match jira {
                true => "jira".into(),
                false => "github".into(),
            },
            repository,
            key,
            title: ticket.title.clone(),
            state: match jira {
                true => String::new(),
                false => ticket.state.clone(),
            },
            author: None,
            assignees: Vec::new(),
            labels: Vec::new(),
            url: ticket.url.clone(),
            updated_at: None,
            seen_at: 0,
            mine: false,
            details,
            checks: Vec::new(),
        },
        comments,
        reviews: Vec::new(),
        created_at: None,
        detailed: true,
    })
}

/// What a live read found of a ticket, stored over what the items keep of it.
pub(super) fn remember(daemon: &Arc<Daemon>, project: &Project, key: &Key, ticket: &Ticket) {
    if let Some(fetched) = fetched_of(project, key, ticket) {
        crate::items::put_live(daemon, fetched);
    }
}

/// A list as a delivery says it.
fn names(value: &Value) -> String {
    let names: Vec<&str> = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    match names.is_empty() {
        true => "nobody".into(),
        false => names.join(", "),
    }
}

/// What the events of one ticket say, as its subscriptions name them, and the one that ends it.
fn events_of(named: &str, events: &[&(&'static str, Value)]) -> (Vec<Event>, Option<Event>) {
    let mut found = Vec::new();
    let mut ends = None;
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    for (name, data) in events {
        match *name {
            "item.commented" => {
                for comment in data["comments"].as_array().into_iter().flatten() {
                    let agent = match comment["by_agent"] == true {
                        true => format!(" ({BY_AGENT})"),
                        false => String::new(),
                    };
                    found.push(Event {
                        kind: "commented",
                        by: Some(By {
                            name: text(&comment["author"]),
                            bot: comment["bot"] == true,
                            marked: comment["by_agent"] == true,
                        }),
                        text: format!(
                            "A comment by {}{agent}: {}",
                            text(&comment["author"]),
                            text(&comment["excerpt"])
                        ),
                    });
                }
            }
            "item.changed" => {
                let changes = &data["changes"];
                for (key, said) in [("status", "status"), ("state", "state")] {
                    let change = &changes[key];
                    if change.is_object() {
                        found.push(Event {
                            kind: "status",
                            by: None,
                            text: format!(
                                "Its {said} changed from {} to {}.",
                                text(&change["from"]),
                                text(&change["to"])
                            ),
                        });
                    }
                }
                let assignees = &changes["assignees"];
                if assignees.is_object() {
                    found.push(Event {
                        kind: "assigned",
                        by: None,
                        text: format!(
                            "Its assignees changed from {} to {}.",
                            names(&assignees["from"]),
                            names(&assignees["to"])
                        ),
                    });
                }
            }
            "item.closed" => {
                let end = Event {
                    kind: "closed",
                    by: None,
                    text: format!("The {named} was closed"),
                };
                found.push(end.clone());
                ends = Some(end);
            }
            _ => {}
        }
    }
    (found, ends)
}

/// Offers what the items stored to a subscription to a ticket, when some of the events are of its
/// ticket.
pub fn offer(
    daemon: &Arc<Daemon>,
    subscription: &Subscription,
    _items: &[Item],
    events: &[(&'static str, Value)],
) {
    let target = &subscription.target;
    let mine: Vec<&(&'static str, Value)> = events
        .iter()
        .filter(|(_, data)| {
            data["kind"] == "ticket"
                && data["project"] == target["project"]
                && data["repository"] == target["repository"]
                && data["key"] == target["key"]
        })
        .collect();
    if mine.is_empty() {
        return;
    }
    let named = target_named(target);
    let (found, ends) = events_of(&named, &mine);
    subscriptions::offer(daemon, subscription, &named, &found, ends.as_ref());
}

/// `tickets subscribe`: the agent waits, once, for the next of the events it names of a ticket
/// (the one named, or its workspace's); the answer comes at once, the event later as a delivery,
/// from the next rounds of the items. What is so now is no event: this read is stored first.
pub(super) async fn subscribe(
    daemon: &Arc<Daemon>,
    agent: &str,
    place: &Place,
    project: &Project,
    asked: &TicketsCall,
) -> Outcome {
    let all = EVENTS.join(", ");
    let events = asked.events.clone().unwrap_or_default();
    if events.is_empty() {
        return Outcome::Refused(format!(
            "refused: tickets subscribe needs events, among {all}"
        ));
    }
    if let Some(other) = events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
        return Outcome::Refused(format!(
            "refused: a ticket has no event {other:?}: its events are {all}"
        ));
    }
    let from = asked.from.as_deref().unwrap_or("anyone");
    if !subscriptions::FROM.contains(&from) {
        return Outcome::Refused(format!(
            "refused: from is anyone (the default) or people (no bot, not your own), not {from:?}"
        ));
    }
    let longest = subscriptions::LONGEST_MINUTES;
    let minutes = asked.minutes.unwrap_or(subscriptions::DEFAULT_MINUTES);
    if !(1..=longest).contains(&minutes) {
        return Outcome::Refused(format!(
            "refused: minutes is from 1 to {longest}, 60 by default"
        ));
    }
    let key = match super::key_of(daemon, place, project, asked) {
        Ok(key) => key,
        Err(refusal) => return refusal,
    };
    if matches!(key, Key::Jira(_)) && events.iter().any(|e| e == "commented") {
        return Outcome::Refused(
            "refused: the comments of a Jira work item are not read yet: subscribe to status, \
             assigned or closed"
                .into(),
        );
    }
    // The read now is stored before the subscription opens, unless a round wrote meanwhile.
    let before = crate::items::before_live(daemon, &project.name);
    let ticket = match super::read(project, &key).await {
        Ok(ticket) => ticket,
        Err(problem) => {
            return Outcome::Failed(format!(
                "the ticket {} could not be read: {problem}",
                key.text()
            ));
        }
    };
    let Some((repository, item)) = item_of(project, &key) else {
        return Outcome::Failed(format!("{} is not of this tracker", key.text()));
    };
    let target = json!({"project": project.name, "repository": repository, "key": item});
    let held = crate::items::held_after(daemon, &project.name, before).await;
    if held.is_some() {
        remember(daemon, project, &key, &ticket);
    }
    let unseen = crate::items::unseen(daemon, &project.name, "ticket", &repository, &item);
    for old in subscriptions::of(daemon, agent) {
        if old.source == TICKETS && old.target == target {
            subscriptions::end(daemon, &old, "cancelled");
        }
    }
    let now = now_ms();
    let subscription = Subscription {
        id: subscriptions::new_subscription_id(),
        agent: agent.to_owned(),
        source: TICKETS.into(),
        target,
        events,
        from: from.to_owned(),
        seen: json!({}),
        created_at: now,
        expires_at: now + minutes * 60_000,
    };
    let subscribed = subscriptions::subscribe(daemon, &subscription);
    drop(held);
    if let Err(error) = subscribed {
        return Outcome::Failed(error);
    }
    let from = match from {
        "people" => ", from people",
        _ => "",
    };
    let mut text = format!(
        "Subscribed: the next {}{from} of the {} wakes you once, as a delivery, within {minutes} \
         min. What is so now is no event: tickets show reads it.",
        subscription.events.join(" or "),
        target_named(&subscription.target)
    );
    match unseen {
        Some(crate::items::Unseen::Source) => text.push_str(
            "\nFlottille's rounds have not read the tracker once yet: what happens before their \
             first complete round is not seen.",
        ),
        Some(crate::items::Unseen::Item) => text.push_str(
            "\nFlottille's rounds have not stored this ticket yet: what happens to it before \
             their next round is not seen.",
        ),
        None => {}
    }
    Outcome::Done(text)
}

/// `tickets unsubscribe`: the agent's subscriptions to the ticket it names, every one when it
/// names none.
pub(super) fn unsubscribe(
    daemon: &Daemon,
    agent: &str,
    project: &Project,
    asked: &TicketsCall,
) -> Outcome {
    let named = match &asked.ticket {
        None => None,
        Some(text) => match super::accepted(project, text) {
            Ok(key) => item_of(project, &key),
            Err(why) => return Outcome::Refused(format!("refused: {why}")),
        },
    };
    let cancelled: Vec<Subscription> = subscriptions::of(daemon, agent)
        .into_iter()
        .filter(|s| s.source == TICKETS)
        .filter(|s| {
            named.as_ref().is_none_or(|(repository, key)| {
                s.target["repository"] == repository.as_str() && s.target["key"] == key.as_str()
            })
        })
        .collect();
    if cancelled.is_empty() {
        return Outcome::Refused("refused: you have no such subscription".into());
    }
    let mut text = "Cancelled your subscriptions:\n".to_owned();
    for subscription in cancelled {
        subscriptions::end(daemon, &subscription, "cancelled");
        text.push_str(&format!("- {}\n", target_named(&subscription.target)));
    }
    Outcome::Done(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_an_assignment_a_comment_and_a_close_are_the_events_of_a_ticket() {
        let events = [
            (
                "item.changed",
                json!({"changes": {"status": {"from": "To Do", "to": "In Progress"},
                    "assignees": {"from": [], "to": ["alice"]}}}),
            ),
            (
                "item.commented",
                json!({"comments": [{"author": "bob", "bot": false, "by_agent": false,
                    "excerpt": "On it"}]}),
            ),
            ("item.closed", json!({})),
        ];
        let events: Vec<&(&'static str, Value)> = events.iter().collect();
        let (found, ends) = events_of("ticket ABC-1", &events);
        let said: Vec<(&str, &str)> = found.iter().map(|e| (e.kind, e.text.as_str())).collect();
        assert_eq!(
            said,
            [
                ("status", "Its status changed from To Do to In Progress."),
                ("assigned", "Its assignees changed from nobody to alice."),
                ("commented", "A comment by bob: On it"),
                ("closed", "The ticket ABC-1 was closed"),
            ]
        );
        assert_eq!(ends.map(|e| e.kind), Some("closed"));
    }
}
