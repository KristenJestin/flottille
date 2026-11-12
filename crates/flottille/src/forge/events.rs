//! The pull requests as a source of events for the subscriptions (`crate::subscriptions`): what
//! the items' rounds and live reads emit of a pull request (`docs/items.md`, "Events") is offered
//! to the subscriptions to it, as the events they name. A subscription reads nothing of its own:
//! a state already stored when it subscribed is no event, since the rounds emit only changes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use flottille_core::item::Item;
use flottille_core::subscription::Subscription;
use serde_json::{Value, json};

use super::ending::{self, Ending};
use super::{BY_AGENT, Forge, Job, Log, Pull, logs};
use crate::daemon::Daemon;
use crate::subscriptions::{self, By, Event};

/// The events a pull request has, as a subscription names them.
pub const EVENTS: [&str; 6] = [
    "checks", "review", "approved", "comment", "merged", "closed",
];

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

/// The checks of `item`, all ended: how many passed, failed (the code's, and the jobs that timed
/// out), failed of the CI's own, were stopped and were skipped. `refined` is what the logs read
/// since say of how the first failing checks ended, in place of what their status says.
fn checks_ended(item: &Item, refined: &[Ending]) -> String {
    let count = |state: &str| item.checks.iter().filter(|c| c.state == state).count();
    let endings: Vec<Ending> = item
        .checks
        .iter()
        .filter(|check| check.state == "failing")
        .enumerate()
        .map(|(at, check)| match refined.get(at) {
            Some(ending) => ending.clone(),
            None => Ending::of(check.outcome.as_deref()),
        })
        .collect();
    let (code, ci, stopped) = ending::counted(&endings);
    let mut text = format!(
        "The checks ended: {} passed, {code} failed",
        count("passing")
    );
    if ci > 0 {
        let s = if ci == 1 { "" } else { "s" };
        text.push_str(&format!(", {ci} CI failure{s}"));
    }
    if stopped > 0 {
        text.push_str(&format!(", {stopped} stopped"));
    }
    if count("skipped") > 0 {
        text.push_str(&format!(", {} skipped", count("skipped")));
    }
    text.push('.');
    text
}

/// The checks of `item` in one line, as an `approved` delivery says them: a failure first, then
/// a check that still runs.
fn checks_line(item: &Item) -> String {
    let count = |state: &str| item.checks.iter().filter(|c| c.state == state).count();
    match (count("failing"), count("pending"), item.checks.len()) {
        (0, 0, 0) => "no checks".to_owned(),
        (0, 0, _) => match (count("passing"), count("skipped")) {
            (passed, 0) => format!("checks: {passed} passed"),
            (0, skipped) => format!("checks: {skipped} skipped"),
            (passed, skipped) => format!("checks: {passed} passed, {skipped} skipped"),
        },
        (0, _, _) => "checks still running".to_owned(),
        (failed, _, _) => format!("checks: {failed} failed"),
    }
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
                text: checks_ended(item, &[]),
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
            "item.approved" => {
                let by: Vec<&str> = said(data, "by").filter_map(Value::as_str).collect();
                let by = match by.is_empty() {
                    true => String::new(),
                    false => format!(" by {}", by.join(", ")),
                };
                let checks = item
                    .map(|item| format!("; {}", checks_line(item)))
                    .unwrap_or_default();
                found.push(Event {
                    kind: "approved",
                    // The review as a whole, not one person's: `from` leaves it alone.
                    by: None,
                    text: format!("The {named} is approved{by}{checks}"),
                });
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

/// How many failed jobs a delivery lists: the others are counted, so that a wake weighs the same
/// whatever the number of jobs that failed.
const LISTED: usize = 3;

/// The failing checks of `item` as jobs whose log can be read, `LISTED` at most, and how many
/// more failed.
fn jobs_of(item: &Item) -> (Vec<Job>, usize) {
    let failing: Vec<_> = item
        .checks
        .iter()
        .filter(|check| check.state == "failing")
        .collect();
    let jobs = failing.iter().take(LISTED).map(|check| Job::of(check));
    (jobs.collect(), failing.len().saturating_sub(LISTED))
}

/// A duration as a wake says it: `45 s`, `2 min 5 s`, `1 h 2 min`.
pub(super) fn lasted(seconds: u64) -> String {
    match (seconds / 3600, seconds / 60 % 60, seconds % 60) {
        (0, 0, s) => format!("{s} s"),
        (0, m, 0) => format!("{m} min"),
        (0, m, s) => format!("{m} min {s} s"),
        (h, 0, _) => format!("{h} h"),
        (h, m, _) => format!("{h} h {m} min"),
    }
}

/// What a wake says of the checks that ended: `head`, then for each failed job its name, how long
/// it took and the line that says why it failed, the others counted, and where the end of
/// every log is. The wake weighs the agent's context at every turn; `forge show` holds the logs.
fn summary(head: &str, jobs: &[Job], more: usize, repository: &str) -> String {
    let mut text = head.to_owned();
    for job in jobs {
        let took = job
            .took
            .map(|s| format!(" ({})", lasted(s)))
            .unwrap_or_default();
        // How it ended is said before any reason: the code's failure by its error line, the
        // others by what they are.
        let why = match (job.said(repository), &job.log) {
            (Some(said), _) => Some(said),
            (None, Some(Ok(log))) => logs::why(log),
            (None, Some(Err(why))) => Some(format!("no log: {why}")),
            (None, None) => None,
        };
        text.push_str(&format!("\n- {}{took}", job.name));
        if let Some(why) = why {
            text.push_str(&format!(": {why}"));
        }
        if let (
            Ending::Code | Ending::Ci(_),
            Some(Ok(Log {
                whole: Some(whole), ..
            })),
        ) = (&job.ending, &job.log)
        {
            text.push_str(&format!("\n  {}", logs::read_whole(repository, whole)));
        }
    }
    if more > 0 {
        text.push_str(&format!("\nand {more} more failed, not listed."));
    }
    text.push_str("\nThe end of each failed job's log is in `forge` `show` of the pull request.");
    text
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

/// What a read found for a subscription: its events, and the one that ends it.
type Found = (Vec<Event>, Option<Event>);

/// The subscriptions told their checks ended, while the logs of the failed jobs are read, with
/// what came for them meanwhile: delivered with the checks, after them.
static READING: Mutex<BTreeMap<String, Vec<Found>>> = Mutex::new(BTreeMap::new());

/// Offers what the items stored to a subscription to a pull request, when some of the events are
/// of its pull request; whether a delivery was queued, for the caller to ring the agent. The logs
/// of the failed jobs are read only when it is told the checks ended, after the round, which does
/// not wait for them: that delivery rings the agent itself, and the events that come for it
/// meanwhile are kept and delivered with it, after the checks.
pub(crate) fn offer(
    daemon: &Arc<Daemon>,
    subscription: &Subscription,
    items: &[Item],
    events: &[(&'static str, Value)],
) -> bool {
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
        return false;
    }
    let item = items
        .iter()
        .find(|i| of(&i.project, &i.kind, &i.repository, &i.key));
    let named = pull_named(project, repository, key.parse().unwrap_or_default());
    let (found, ends) = events_of(&named, item, &mine);
    if let Some(kept) = READING.lock().unwrap().get_mut(&subscription.id) {
        kept.push((found, ends));
        return false;
    }
    let named = target_named(target);
    let (jobs, more) = item.map(jobs_of).unwrap_or_default();
    let told_checks = subscription.events.iter().any(|e| e == "checks")
        && found.iter().any(|e| e.kind == "checks");
    let forge = forge_of(daemon, project, repository);
    let (Some(forge), true, false) = (forge, told_checks, jobs.is_empty()) else {
        return subscriptions::offer(daemon, subscription, &named, &found, ends.as_ref(), false);
    };
    let item = item.cloned();
    let (daemon, subscription) = (daemon.clone(), subscription.clone());
    let repository = repository.to_owned();
    READING
        .lock()
        .unwrap()
        .insert(subscription.id.clone(), Vec::new());
    tokio::spawn(async move {
        let mut pull = Pull {
            number: 0,
            title: String::new(),
            state: "open",
            head: String::new(),
            head_commit: None,
            base: String::new(),
            url: String::new(),
            description: String::new(),
            mergeable: None,
            review: None,
            checks: Vec::new(),
            not_started: None,
            comments: Vec::new(),
            reviews: Vec::new(),
            reviewers: Vec::new(),
            jobs,
            ci: None,
            closing: None,
            ended: None,
        };
        logs::attach(&forge, &mut pull.jobs).await;
        // The logs may say a failed check was the CI's: the head counts it so.
        let endings: Vec<Ending> = pull.jobs.iter().map(|job| job.ending.clone()).collect();
        let mut found: Vec<Event> = found
            .into_iter()
            .map(|mut event| {
                if event.kind == "checks" {
                    let head = match &item {
                        Some(item) => checks_ended(item, &endings),
                        None => event.text.clone(),
                    };
                    event.text = summary(&head, &pull.jobs, more, &repository);
                }
                event
            })
            .collect();
        let mut ends = ends;
        // Held until the delivery is queued: what a round finds meanwhile waits for it, and comes
        // after it, never between the entry's removal and the delivery.
        let mut reading = READING.lock().unwrap();
        for (later, ended) in reading.remove(&subscription.id).unwrap_or_default() {
            found.extend(later);
            ends = ends.or(ended);
        }
        subscriptions::offer(&daemon, &subscription, &named, &found, ends.as_ref(), true);
        drop(reading);
    });
    false
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

/// What a subscription of the forge watches: a pull request of a repository, found by a
/// workspace's branch, or by its number alone.
pub(super) fn target(
    project: &str,
    workspace: Option<&str>,
    repository: &str,
    number: u64,
) -> Value {
    let mut target = json!({"project": project, "repository": repository, "number": number});
    if let Some(workspace) = workspace {
        target["workspace"] = workspace.into();
    }
    target
}

/// Whether two targets are the same pull request, whichever way each was found.
pub(super) fn same_pull(one: &Value, other: &Value) -> bool {
    ["project", "repository", "number"]
        .iter()
        .all(|key| one[key] == other[key])
        && one["number"].is_u64()
}

/// A subscription's target, as a delivery and `forge show` say it.
pub fn target_named(target: &Value) -> String {
    let pull = pull_named(
        target["project"].as_str().unwrap_or_default(),
        target["repository"].as_str().unwrap_or_default(),
        target["number"].as_u64().unwrap_or_default(),
    );
    match target["workspace"].as_str() {
        Some(workspace) => format!("{pull}, workspace {workspace}"),
        None => pull,
    }
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

    #[test]
    fn an_approval_says_who_approved_and_the_checks_in_one_line() {
        let check = |state: &str| flottille_core::item::Check {
            name: "test".into(),
            state: state.into(),
            started_at: None,
            ended_at: None,
            link: None,
            outcome: None,
        };
        let item = |states: &[&str]| Item {
            project: "app".into(),
            kind: "pull".into(),
            source: "github".into(),
            repository: "api".into(),
            key: "12".into(),
            title: String::new(),
            state: "open".into(),
            author: None,
            assignees: Vec::new(),
            labels: Vec::new(),
            url: None,
            updated_at: None,
            seen_at: 1,
            mine: false,
            details: json!({}),
            checks: states.iter().map(|s| check(s)).collect(),
        };
        let events = [event("item.approved", json!({"by": ["bob", "carol"]}))];
        let events: Vec<&(&'static str, Value)> = events.iter().collect();
        for (checks, said) in [
            (vec!["passing"; 12], "checks: 12 passed"),
            (vec!["passing", "pending"], "checks still running"),
            (vec!["failing", "failing", "passing"], "checks: 2 failed"),
            (vec!["passing", "skipped"], "checks: 1 passed, 1 skipped"),
            (vec!["skipped", "skipped"], "checks: 2 skipped"),
            (vec![], "no checks"),
        ] {
            let item = item(&checks);
            let (found, ends) = events_of("pull request #12 of api", Some(&item), &events);
            assert_eq!(
                found[0].text,
                format!("The pull request #12 of api is approved by bob, carol; {said}")
            );
            assert_eq!(found[0].kind, "approved");
            assert!(found[0].by.is_none() && ends.is_none());
        }
    }

    #[test]
    fn details_changed_alone_are_no_event_of_a_pull_request() {
        let events = [event(
            "item.changed",
            json!({"changes": {}, "fields": ["description", "discussion", "checks"]}),
        )];
        let events: Vec<&(&'static str, Value)> = events.iter().collect();
        let (found, ends) = events_of("pull request #12 of api", None, &events);
        assert!(found.is_empty(), "{found:?}");
        assert!(ends.is_none());
    }

    #[test]
    fn a_duration_is_said_in_its_two_largest_units() {
        for (seconds, said) in [
            (45, "45 s"),
            (120, "2 min"),
            (125, "2 min 5 s"),
            (3600, "1 h"),
            (3725, "1 h 2 min"),
        ] {
            assert_eq!(lasted(seconds), said);
        }
    }

    #[test]
    fn the_failed_check_wake_says_how_to_read_the_whole_log() {
        let job = Job {
            name: "test".into(),
            details: None,
            log: Some(Ok(Log {
                text: "error: test failed".into(),
                shown: 1,
                total: 80,
                part: true,
                whole: Some((900, "456".into())),
            })),
            ending: Ending::Code,
            took: None,
            rerun: None,
        };

        let text = summary("The checks ended.", &[job], 0, "api");

        assert!(
            text.contains(
                "- test: error: test failed\n  Read the whole log with forge {\"action\": \"run_logs\", \"repository\": \"api\", \"run\": 900, \"step\": \"456\"}."
            ),
            "{text}"
        );
    }

    #[test]
    fn a_job_with_no_log_says_why_whole() {
        let why = format!(
            "it is not a GitHub Actions job, see https://ci.example.com/{}",
            "b".repeat(300)
        );
        let job = Job {
            name: "lint".into(),
            details: None,
            log: Some(Err(why.clone())),
            ending: Ending::Code,
            took: Some(14),
            rerun: None,
        };

        let text = summary("The checks ended.", &[job], 0, "api");

        assert!(
            text.contains(&format!("- lint (14 s): no log: {why}")),
            "{text}"
        );
    }
}
