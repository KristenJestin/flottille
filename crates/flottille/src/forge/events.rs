//! The forge as a source of events for the subscriptions (`crate::subscriptions`): a read of a
//! watched workspace is compared with what each subscription to one of its pull requests saw
//! last, and what changed becomes events. A state already there when it was seen is no event.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use flottille_core::subscription::Subscription;
use flottille_core::workspace::Workspace;
use serde_json::{Value, json};

use super::{BY_AGENT, Comment, Pull, Read, logs};
use crate::daemon::Daemon;
use crate::subscriptions::{self, By, Event};

/// The events a pull request has, as a subscription names them.
pub const EVENTS: [&str; 5] = ["checks", "review", "comment", "merged", "closed"];
/// What a delivery shows of a comment or a review at most.
const EXCERPT_CHARS: usize = 300;

/// What a subscription keeps of a pull request to compare the next read with.
pub(super) fn seen_of(pull: &Pull) -> Value {
    let reviewers: HashMap<&str, &str> = pull
        .reviewers
        .iter()
        .map(|(name, state)| (name.as_str(), *state))
        .collect();
    json!({
        "state": pull.state,
        "pending": pull.checks.iter().any(|(_, state)| *state == "pending"),
        "head": pull.head_commit,
        "comments": hashes(&pull.comments),
        "reviews": hashes(&pull.reviews),
        "reviewers": reviewers,
    })
}

/// Each comment as a short hash: what was seen is known without keeping its text.
fn hashes(comments: &[Comment]) -> Vec<String> {
    comments.iter().map(hash).collect()
}

fn hash(comment: &Comment) -> String {
    let mut hasher = DefaultHasher::new();
    (&comment.author, &comment.state, &comment.body).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The comments of `now` that `seen` did not hold, as many times as they are new.
fn new_ones<'a>(seen: &Value, now: &'a [Comment]) -> Vec<&'a Comment> {
    let mut left: HashMap<&str, usize> = HashMap::new();
    for hash in seen
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        *left.entry(hash).or_default() += 1;
    }
    let hashes = hashes(now);
    now.iter()
        .zip(&hashes)
        .filter(|(_, hash)| match left.get_mut(hash.as_str()) {
            Some(count) if *count > 0 => {
                *count -= 1;
                false
            }
            _ => true,
        })
        .map(|(comment, _)| comment)
        .collect()
}

fn excerpt(text: &str) -> String {
    let text = text.trim();
    let mut cut: String = text.chars().take(EXCERPT_CHARS).collect();
    if cut.len() < text.len() {
        cut.push('…');
    }
    cut.replace('\n', "\n  ")
}

/// What happened to `pull` since `seen`, and the event that ends it (merged or closed); nothing
/// when nothing was seen yet: this read is the first.
fn events_between(seen: &Value, pull: &Pull, named: &str) -> (Vec<Event>, Option<Event>) {
    if !seen.is_object() {
        return (Vec::new(), None);
    }
    let mut events = Vec::new();
    let pending = pull.checks.iter().any(|(_, state)| *state == "pending");
    let new_head = seen["head"].is_string()
        && pull.head_commit.is_some()
        && seen["head"].as_str() != pull.head_commit.as_deref();
    if !pull.checks.is_empty() && !pending && (seen["pending"] == true || new_head) {
        let count = |state| pull.checks.iter().filter(|(_, s)| *s == state).count();
        let mut text = format!(
            "The checks ended: {} passed, {} failed",
            count("passing"),
            count("failing")
        );
        if count("skipped") > 0 {
            text.push_str(&format!(", {} skipped", count("skipped")));
        }
        text.push_str(".\n");
        text.push_str(&logs::text(&pull.jobs));
        events.push(Event {
            kind: "checks",
            by: None,
            text: text.trim_end().to_owned(),
        });
    }
    for review in new_ones(&seen["reviews"], &pull.reviews) {
        let state = review.state.as_deref().unwrap_or("commented");
        if state == "pending" {
            continue;
        }
        events.push(Event {
            kind: "review",
            by: Some(By {
                name: review.author.clone(),
                bot: review.bot,
                marked: false,
            }),
            text: format!(
                "A review by {}, {state}: {}",
                review.author,
                excerpt(&review.body)
            ),
        });
    }
    // Bitbucket says a review as a reviewer's verdict.
    for (name, state) in &pull.reviewers {
        if *state != "pending" && seen["reviewers"][name.as_str()].as_str() != Some(*state) {
            events.push(Event {
                kind: "review",
                by: Some(By {
                    name: name.clone(),
                    bot: false,
                    marked: false,
                }),
                text: format!("A review by {name}: {state}"),
            });
        }
    }
    for comment in new_ones(&seen["comments"], &pull.comments) {
        let (body, marked) = comment.unmarked();
        let agent = if marked {
            format!(" ({BY_AGENT})")
        } else {
            String::new()
        };
        events.push(Event {
            kind: "comment",
            by: Some(By {
                name: comment.author.clone(),
                bot: comment.bot,
                marked,
            }),
            text: format!("A comment by {}{agent}: {}", comment.author, excerpt(body)),
        });
    }
    let ends = match (seen["state"].as_str(), pull.state) {
        (Some(was), now @ ("merged" | "closed")) if was != now => Some(Event {
            kind: now,
            by: None,
            text: format!("The {named} was {now}"),
        }),
        _ => None,
    };
    events.extend(ends.clone());
    (events, ends)
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

/// The subscriptions to the pull requests of the workspace.
fn on(daemon: &Daemon, workspace: &Workspace) -> Vec<Subscription> {
    subscriptions::all(daemon)
        .into_iter()
        .filter(|s| {
            s.source == "forge"
                && s.target["project"] == workspace.project.as_str()
                && s.target["workspace"] == workspace.name.as_str()
        })
        .collect()
}

/// Offers what a read of the workspace found to the subscriptions to its pull requests. The logs
/// of the failed jobs are read only when a subscription is told the checks ended.
pub(super) async fn offer(daemon: &Arc<Daemon>, workspace: &Workspace, read: &Read) {
    let subscriptions = on(daemon, workspace);
    if subscriptions.is_empty() {
        return;
    }
    for repository in &read.repositories {
        let (Some(forge), Some(pull)) = (&repository.forge, &repository.pull) else {
            continue;
        };
        let mine: Vec<&Subscription> = subscriptions
            .iter()
            .filter(|s| {
                s.target["repository"] == repository.path.as_str()
                    && s.target["number"].as_u64() == Some(pull.number)
            })
            .collect();
        if mine.is_empty() {
            continue;
        }
        let named = pull_named(&workspace.project, &repository.path, pull.number);
        let events = |pull: &Pull, seen: &Value| events_between(seen, pull, &named);
        let mut pull = pull.clone();
        let told_checks = mine.iter().any(|s| {
            s.events.iter().any(|e| e == "checks")
                && events(&pull, &s.seen).0.iter().any(|e| e.kind == "checks")
        });
        if told_checks {
            logs::attach(forge, &mut pull).await;
        }
        let seen = seen_of(&pull);
        for subscription in mine {
            let (found, ends) = events(&pull, &subscription.seen);
            subscriptions::offer(
                daemon,
                subscription,
                &target_named(&subscription.target),
                &found,
                ends.as_ref(),
                &seen,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull(state: &'static str, checks: &[&'static str], comments: &[(&str, &str)]) -> Pull {
        Pull {
            number: 12,
            title: String::new(),
            state,
            head: "app/login".into(),
            head_commit: Some("aaaaaaa".into()),
            base: "main".into(),
            url: String::new(),
            description: String::new(),
            mergeable: None,
            review: None,
            checks: checks.iter().map(|s| ("ci".to_owned(), *s)).collect(),
            comments: comments
                .iter()
                .map(|(author, body)| Comment {
                    author: (*author).into(),
                    state: None,
                    body: (*body).into(),
                    bot: false,
                })
                .collect(),
            reviews: Vec::new(),
            reviewers: Vec::new(),
            jobs: Vec::new(),
            ci: None,
        }
    }

    fn kinds(seen: &Pull, now: &Pull) -> Vec<&'static str> {
        let (events, _) = events_between(&seen_of(seen), now, "pull request #12 of api");
        events.iter().map(|e| e.kind).collect()
    }

    #[test]
    fn a_change_is_an_event_and_a_state_already_seen_is_not() {
        let running = pull("open", &["pending"], &[]);
        let ended = pull("open", &["failing"], &[]);
        assert_eq!(kinds(&running, &ended), ["checks"]);
        assert!(kinds(&ended, &ended).is_empty());
        assert!(
            events_between(&Value::Null, &ended, "x").0.is_empty(),
            "the first read is no event"
        );
        let commented = pull("open", &["failing"], &[("alice", "a"), ("bob", "b")]);
        let (events, _) = events_between(&seen_of(&ended), &commented, "x");
        assert_eq!(events.len(), 2);
        // A comment inserted among the others, as a Bitbucket reply is, is the only new one.
        let replied = pull(
            "open",
            &["failing"],
            &[("alice", "a"), ("carol", "c"), ("bob", "b")],
        );
        let (events, _) = events_between(&seen_of(&commented), &replied, "x");
        assert_eq!(events.len(), 1);
        assert!(events[0].text.contains("carol"), "{:?}", events);
        let merged = pull("merged", &["failing"], &[]);
        let (events, ends) = events_between(&seen_of(&ended), &merged, "x");
        assert_eq!(ends.map(|e| e.kind), Some("merged"));
        assert_eq!(events.len(), 1);
    }
}
