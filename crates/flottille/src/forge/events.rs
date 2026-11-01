//! The pull requests as a source of events for the subscriptions (`crate::subscriptions`): what
//! the items' rounds and live reads emit of a pull request (`docs/items.md`, "Events") is offered
//! to the subscriptions to it, as the events they name. A subscription reads nothing of its own:
//! a state already stored when it subscribed is no event, since the rounds emit only changes.

use std::sync::Arc;

use flottille_core::item::Item;
use flottille_core::subscription::Subscription;
use flottille_core::workspace::Workspace;
use serde_json::{Value, json};

use super::{BY_AGENT, Forge, Job, Pull, logs};
use crate::daemon::Daemon;
use crate::subscriptions::{self, By, Event};

/// The events a pull request has, as a subscription names them.
pub const EVENTS: [&str; 5] = ["checks", "review", "comment", "merged", "closed"];

/// Who wrote a comment or a review an `item.*` event says (`items::Said`).
fn by(said: &Value) -> Option<By> {
    Some(By {
        name: said["author"].as_str().unwrap_or_default().to_owned(),
        bot: said["bot"] == true,
        marked: said["by_agent"] == true,
    })
}

/// The newest comments or reviews an event says.
fn said<'a>(data: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    data[key].as_array().into_iter().flatten()
}

/// The checks of `item`, all ended: how many passed, failed and were skipped.
fn checks_ended(item: &Item) -> String {
    let count = |state: &str| item.checks.iter().filter(|c| c.state == state).count();
    let mut text = format!(
        "The checks ended: {} passed, {} failed",
        count("passing"),
        count("failing")
    );
    if count("skipped") > 0 {
        text.push_str(&format!(", {} skipped", count("skipped")));
    }
    text.push('.');
    text
}

/// What the events of one pull request say, as its subscriptions name them, and the one that
/// ends it (merged or closed).
fn events_of(
    named: &str,
    item: Option<&Item>,
    events: &[&(&'static str, Value)],
) -> (Vec<Event>, Option<Event>) {
    let mut found = Vec::new();
    let mut ends = None;
    for (name, data) in events {
        match *name {
            "item.checks" => found.extend(item.map(|item| Event {
                kind: "checks",
                by: None,
                text: checks_ended(item),
            })),
            "item.reviewed" => {
                for review in said(data, "reviews") {
                    let state = review["state"].as_str().unwrap_or("commented");
                    if state == "pending" {
                        continue;
                    }
                    let author = review["author"].as_str().unwrap_or_default();
                    found.push(Event {
                        kind: "review",
                        by: by(review),
                        text: format!(
                            "A review by {author}, {state}: {}",
                            review["excerpt"].as_str().unwrap_or_default()
                        ),
                    });
                }
                // Bitbucket says a review as a reviewer's verdict.
                for verdict in said(data, "verdicts") {
                    let name = verdict["name"].as_str().unwrap_or_default();
                    found.push(Event {
                        kind: "review",
                        by: Some(By {
                            name: name.to_owned(),
                            bot: false,
                            marked: false,
                        }),
                        text: format!(
                            "A review by {name}: {}",
                            verdict["state"].as_str().unwrap_or_default()
                        ),
                    });
                }
            }
            "item.commented" => {
                for comment in said(data, "comments") {
                    let agent = match comment["by_agent"] == true {
                        true => format!(" ({BY_AGENT})"),
                        false => String::new(),
                    };
                    found.push(Event {
                        kind: "comment",
                        by: by(comment),
                        text: format!(
                            "A comment by {}{agent}: {}",
                            comment["author"].as_str().unwrap_or_default(),
                            comment["excerpt"].as_str().unwrap_or_default()
                        ),
                    });
                }
            }
            "item.merged" | "item.closed" => {
                let now = match *name {
                    "item.merged" => "merged",
                    _ => "closed",
                };
                let end = Event {
                    kind: now,
                    by: None,
                    text: format!("The {named} was {now}"),
                };
                found.push(end.clone());
                ends = Some(end);
            }
            _ => {}
        }
    }
    (found, ends)
}

/// The failing checks of `item` as jobs whose log can be read.
fn jobs_of(item: &Item) -> Vec<Job> {
    item.checks
        .iter()
        .filter(|check| check.state == "failing")
        .map(|check| Job {
            name: check.name.clone(),
            details: check.link.clone(),
            log: None,
        })
        .collect()
}

/// The forge the project declares for the repository.
fn forge_of(daemon: &Daemon, project: &str, repository: &str) -> Option<Forge> {
    let project = crate::projects::load_project(daemon, project).ok()?;
    let declared = project
        .repositories
        .iter()
        .find(|r| r.path == repository)?
        .forge
        .as_ref()?;
    Some(Forge::from(declared))
}

/// Offers what the items stored to a subscription to a pull request, when some of the events are
/// of its pull request. The logs of the failed jobs are read only when it is told the checks
/// ended, after the round, which does not wait for them.
pub(crate) fn offer(
    daemon: &Arc<Daemon>,
    subscription: &Subscription,
    items: &[Item],
    events: &[(&'static str, Value)],
) {
    let target = &subscription.target;
    let project = target["project"].as_str().unwrap_or_default();
    let repository = target["repository"].as_str().unwrap_or_default();
    let key = target["number"].as_u64().unwrap_or_default().to_string();
    let of = |p: &str, kind: &str, r: &str, k: &str| {
        p == project && kind == "pull" && r == repository && k == key
    };
    let mine: Vec<&(&'static str, Value)> = events
        .iter()
        .filter(|(_, data)| {
            let text = |key: &str| data[key].as_str().unwrap_or_default();
            of(
                text("project"),
                text("kind"),
                text("repository"),
                text("key"),
            )
        })
        .collect();
    if mine.is_empty() {
        return;
    }
    let item = items
        .iter()
        .find(|i| of(&i.project, &i.kind, &i.repository, &i.key));
    let named = pull_named(project, repository, key.parse().unwrap_or_default());
    let (found, ends) = events_of(&named, item, &mine);
    let named = target_named(target);
    let jobs = item.map(jobs_of).unwrap_or_default();
    let told_checks = subscription.events.iter().any(|e| e == "checks")
        && found.iter().any(|e| e.kind == "checks");
    let forge = forge_of(daemon, project, repository);
    let (Some(forge), true, false) = (forge, told_checks, jobs.is_empty()) else {
        subscriptions::offer(daemon, subscription, &named, &found, ends.as_ref());
        return;
    };
    let (daemon, subscription) = (daemon.clone(), subscription.clone());
    tokio::spawn(async move {
        let mut pull = Pull {
            number: 0,
            title: String::new(),
            state: "open",
            head: String::new(),
            head_commit: None,
            base: String::new(),
            url: String::new(),
            mergeable: None,
            review: None,
            checks: Vec::new(),
            comments: Vec::new(),
            reviews: Vec::new(),
            reviewers: Vec::new(),
            jobs,
            ci: None,
        };
        logs::attach(&forge, &mut pull).await;
        let logs = logs::text(&pull.jobs);
        let found: Vec<Event> = found
            .into_iter()
            .map(|mut event| {
                if event.kind == "checks" {
                    event.text = format!("{}\n{logs}", event.text).trim_end().to_owned();
                }
                event
            })
            .collect();
        subscriptions::offer(&daemon, &subscription, &named, &found, ends.as_ref());
    });
}

/// A repository that is the project's folder is named by the project, as the window does.
fn pull_named(project: &str, repository: &str, number: u64) -> String {
    let repository = if repository == "." {
        project
    } else {
        repository
    };
    format!("pull request #{number} of {repository}")
}

/// What a subscription of the forge watches: a pull request of a workspace's repository.
pub(super) fn target(workspace: &Workspace, repository: &str, number: u64) -> Value {
    json!({"project": workspace.project, "workspace": workspace.name, "repository": repository,
        "number": number})
}

/// A subscription's target, as a delivery and `forge show` say it.
pub fn target_named(target: &Value) -> String {
    format!(
        "{}, workspace {}",
        pull_named(
            target["project"].as_str().unwrap_or_default(),
            target["repository"].as_str().unwrap_or_default(),
            target["number"].as_u64().unwrap_or_default()
        ),
        target["workspace"].as_str().unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &'static str, data: Value) -> (&'static str, Value) {
        (name, data)
    }

    #[test]
    fn the_events_of_the_items_are_those_a_subscription_names() {
        let events = [
            event(
                "item.commented",
                json!({"comments": [
                    {"author": "alice", "bot": false, "by_agent": false, "excerpt": "Why?"},
                    {"author": "kris", "bot": false, "by_agent": true, "excerpt": "Rebased"}
                ]}),
            ),
            event(
                "item.reviewed",
                json!({"reviews": [
                    {"author": "bob", "bot": false, "state": "changes requested", "excerpt": "Rename"},
                    {"author": "eve", "bot": false, "state": "pending", "excerpt": ""}
                ], "verdicts": [{"name": "carol", "state": "approved"}]}),
            ),
            event("item.merged", json!({})),
        ];
        let events: Vec<&(&'static str, Value)> = events.iter().collect();
        let (found, ends) = events_of("pull request #12 of api", None, &events);
        let texts: Vec<&str> = found.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "A comment by alice: Why?",
                "A comment by kris (posted by an agent): Rebased",
                "A review by bob, changes requested: Rename",
                "A review by carol: approved",
                "The pull request #12 of api was merged",
            ]
        );
        assert!(found[1].by.as_ref().is_some_and(|by| by.marked));
        assert_eq!(ends.map(|e| e.kind), Some("merged"));
    }
}
