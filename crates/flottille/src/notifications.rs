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
    // What the phones were told of it is dismissed.
    crate::phone::check(daemon);
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
        // What is awaited is not held across the `spawn_blocking` below: the guard of the watch
        // is not `Send`.
        let wakes = tokio::select! {
            received = live.recv() => match received {
                Ok(event) => relevant(&event),
                // Events were missed: the store has them.
                Err(broadcast::error::RecvError::Lagged(_)) => true,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = async { let _ = stopping.wait_for(|stopping| *stopping).await; } => return,
        };
        if wakes {
            let kept = daemon.clone();
            let _ = tokio::task::spawn_blocking(move || keep(&kept)).await;
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
/// old. Called when an event that matters comes, and by the sweep: one at a time.
pub fn keep(daemon: &Arc<Daemon>) {
    let _keeping = daemon
        .notifying
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok((cursor, upto, seeded)) = daemon.read(|store| {
        Ok((
            store.notification_cursor()?,
            store.last_seq()?,
            store.notifications_seeded()?,
        ))
    }) else {
        return;
    };
    // The migration set it: the centre starts with the events that come after it, what was told
    // before it existed is in the inbox and the window's rail, not in rows.
    let cursor = cursor.unwrap_or(upto);
    let mut names: Vec<&str> = EVENTS.to_vec();
    names.extend(["agent.tool_served"]);
    let extensions = crate::extensions::needs_you();
    names.extend(extensions.iter().map(|(event, _, _)| *event));
    let mut changed = Vec::new();
    // What waited for the user at the upgrade still does: its rows are made once.
    if !seeded {
        match seed(daemon, cursor, &extensions) {
            Ok(seeded) => changed.extend(seeded),
            Err(_) => return,
        }
        if daemon
            .read(|store| store.set_notifications_seeded())
            .is_err()
        {
            return;
        }
    }
    let Ok(events) = daemon.read(|store| store.events_named(&names, cursor)) else {
        return;
    };
    at(Point::Read);
    // A row the store failed to write stops the keeper at its event: the next keep tries again.
    let mut done = upto;
    for event in events.iter().filter(|event| event.seq <= upto) {
        match note(daemon, event, &extensions) {
            Ok(rows) => changed.extend(rows),
            Err(_) => {
                done = event.seq - 1;
                break;
            }
        }
    }
    let now = now_ms();
    let pruned = daemon
        .read(|store| match store.prunable_notifications(now)? {
            true => store.prune_notifications(now),
            false => Ok(0),
        })
        .is_ok_and(|pruned| pruned > 0);
    // At rest, nothing is written.
    if done > cursor {
        let _ = daemon.read(|store| store.set_notification_cursor(done));
    }
    changed.sort_unstable();
    changed.dedup();
    changed_event(daemon, &changed, pruned, "kept");
    crate::phone::check(daemon);
}

/// The rows of what waits for the user from before the cursor: a request still held for them,
/// a vault entry still asked for, a root's question nothing answered. What was answered makes
/// none.
fn seed(
    daemon: &Arc<Daemon>,
    cursor: u64,
    extensions: &[(&'static str, &'static str, crate::extensions::Says)],
) -> flottille_core::Result<Vec<u64>> {
    let events = daemon.read(|store| {
        store.events_named(
            &[
                "approval.requested",
                "approval.forwarded",
                "vault.asked",
                "vault.ask_answered",
                "agent.asked",
            ],
            0,
        )
    })?;
    let given: Vec<u64> = events
        .iter()
        .filter(|event| event.name == "vault.ask_answered")
        .filter_map(|event| event.data["ask"].as_u64())
        .collect();
    let mut waiting: std::collections::HashMap<String, Vec<u64>> = Default::default();
    let mut changed = Vec::new();
    for event in events.iter().filter(|event| event.seq <= cursor) {
        let still = match event.name.as_str() {
            // `note` keeps one only while it is held for the user.
            "approval.requested" | "approval.forwarded" => true,
            "vault.asked" => !given.contains(&event.seq),
            // An ended session sends no event that would resolve its question later.
            "agent.asked" => match root(daemon, event.data["agent"].as_str())
                .filter(|id| !is_final(daemon, id))
            {
                Some(id) => waiting
                    .entry(id.clone())
                    .or_insert_with(|| {
                        crate::tree::unanswered(daemon, &id)
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|question| question["seq"].as_u64())
                            .collect()
                    })
                    .contains(&event.seq),
                None => false,
            },
            _ => false,
        };
        if still {
            changed.extend(note(daemon, event, extensions)?);
        }
    }
    Ok(changed)
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
) -> flottille_core::Result<Vec<u64>> {
    add_while(daemon, event, kind, source, agent, title, excerpt, |_| {
        Ok(true)
    })
}

/// Keeps one row while `still` holds, read in the same hold of the store as the row is written:
/// nothing changes between them.
#[allow(clippy::too_many_arguments)]
fn add_while(
    daemon: &Daemon,
    event: &Event,
    kind: &str,
    source: String,
    agent: Option<&str>,
    title: impl Into<String>,
    excerpt: impl Into<String>,
    still: impl FnOnce(&flottille_core::store::Store) -> flottille_core::Result<bool>,
) -> flottille_core::Result<Vec<u64>> {
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
    Ok(daemon
        .read(|store| {
            if at(Point::Add) {
                return Err(flottille_core::Error::Refused("the store failed".into()));
            }
            match still(store)? {
                true => store.add_notification(&new),
                false => Ok(None),
            }
        })?
        .into_iter()
        .collect())
}

/// What an answer resolves: the number of the row when it was waiting.
fn resolve(daemon: &Daemon, source: &str, at: u64) -> flottille_core::Result<Vec<u64>> {
    Ok(daemon
        .read(|store| store.resolve_notification(source, at))?
        .into_iter()
        .collect())
}

/// A root agent: the session itself. A child's report, question and failure go to its parent, never
/// to the user.
fn root(daemon: &Daemon, id: Option<&str>) -> Option<String> {
    let id = id?;
    let agent = daemon.read(|store| store.agent(id)).ok().flatten()?;
    agent.parent.is_none().then_some(agent.id)
}

/// Whether the session ended or failed.
fn is_final(daemon: &Daemon, id: &str) -> bool {
    let agent = daemon.read(|store| store.agent(id)).ok().flatten();
    agent.is_some_and(|agent| agent.state.is_final())
}

/// Whether the request waits for the user: held by them, and nothing answered it.
fn waits(request: &approval::Request) -> bool {
    request.held_by == approval::USER && request.state == approval::RequestState::Pending
}

/// The rows an event makes or resolves.
fn note(
    daemon: &Arc<Daemon>,
    event: &Event,
    extensions: &[(&'static str, &'static str, crate::extensions::Says)],
) -> flottille_core::Result<Vec<u64>> {
    let data = &event.data;
    let agent = data["agent"].as_str();
    if let Some((_, kind, text)) = extensions.iter().find(|(name, _, _)| *name == event.name) {
        // An extension says what happened, and no event of its says when it is over: the row
        // tells, it does not wait for the user.
        return add(
            daemon,
            event,
            kind,
            format!("event:{}", event.seq),
            None,
            format!("From {kind}"),
            notification::first_line(&text(data)),
        );
    }
    Ok(match event.name.as_str() {
        "approval.requested" | "approval.forwarded" => {
            let Some(number) = data["number"].as_u64() else {
                return Ok(Vec::new());
            };
            let held = daemon.read(|store| store.request(number))?;
            match held {
                // It waits for the user as long as the user holds it and nothing answered it: one
                // the judge or the user answered before the keeper looked never waited, nor one
                // answered while it looked.
                Some(request) if waits(&request) => {
                    at(Point::SeenPending);
                    add_while(
                        daemon,
                        event,
                        "request",
                        format!("request:{number}"),
                        Some(request.agent.as_str()),
                        format!("Request #{number} waits for you"),
                        notification::excerpt(&notification::asked(&request.tool, &request.shown)),
                        |store| Ok(store.request(number)?.is_some_and(|now| waits(&now))),
                    )?
                }
                _ => Vec::new(),
            }
        }
        "approval.decided" | "approval.cancelled" => match data["number"].as_u64() {
            Some(number) => resolve(daemon, &format!("request:{number}"), event.at)?,
            None => Vec::new(),
        },
        "agent.asked" if data["to"] == "user" => {
            let Some(id) = root(daemon, agent) else {
                return Ok(Vec::new());
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
            )?
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
                )?,
                _ => Vec::new(),
            };
            if let Some(id) = agent {
                changed.extend(reconcile_questions(daemon, id, event.at)?);
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
            )?,
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
        )?,
        "vault.ask_answered" => match data["ask"].as_u64() {
            Some(ask) => resolve(daemon, &format!("vault:{ask}"), event.at)?,
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
        )?,
        "session.following_resumed" => {
            // Every pause of that pair that still waits.
            let prefix = format!(
                "paused:{}:{}:",
                agent.unwrap_or_default(),
                data["follower"].as_str().unwrap_or_default()
            );
            let open = daemon.read(|store| store.unresolved_notifications("paused"))?;
            let mut changed = Vec::new();
            for source in open.iter().filter(|source| source.starts_with(&prefix)) {
                changed.extend(resolve(daemon, source, event.at)?);
            }
            changed
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
        )?,
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
        )?,
        "remote.delivery_dropped" => add(
            daemon,
            event,
            "dropped",
            format!("event:{}", event.seq),
            None,
            "A message for another machine was dropped",
            notification::first_line(&crate::across::dropped_text(data)),
        )?,
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
            )?,
            None => Vec::new(),
        },
        _ => Vec::new(),
    })
}

/// The questions of a root that nothing waits on any more are resolved: the store says which are
/// open, whatever event told the keeper to look (`tree::unanswered`).
fn reconcile_questions(
    daemon: &Arc<Daemon>,
    agent: &str,
    at: u64,
) -> flottille_core::Result<Vec<u64>> {
    let prefix = format!("question:{agent}:");
    let open: Vec<String> = daemon
        .read(|store| store.unresolved_notifications("question"))?
        .into_iter()
        .filter(|source| source.starts_with(&prefix))
        .collect();
    if open.is_empty() {
        return Ok(Vec::new());
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
    let mut changed = Vec::new();
    for source in open.iter().filter(|source| {
        let seq = source
            .rsplit(':')
            .next()
            .and_then(|seq| seq.parse::<u64>().ok());
        !seq.is_some_and(|seq| waiting.contains(&seq))
    }) {
        changed.extend(resolve(daemon, source, at)?);
    }
    Ok(changed)
}

/// The points of the keeper a test stops at, or makes fail (`tests::at`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Point {
    /// The events after the cursor are read, none noted yet.
    Read,
    /// A request was seen waiting for the user, its row not kept yet.
    SeenPending,
    /// A row is about to be kept: `true` makes the store fail.
    Add,
}

#[cfg(not(test))]
fn at(_: Point) -> bool {
    false
}

#[cfg(test)]
fn at(point: Point) -> bool {
    tests::at(point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flottille_core::approval::{Decision, Kind, Request, RequestState};
    use serde_json::json;
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::time::Duration;

    type Hook = Box<dyn FnMut() -> bool>;

    thread_local! {
        /// What the keeper does at each point, on the thread that keeps: each test sets its own.
        static SEAMS: RefCell<Vec<(Point, Hook)>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn at(point: Point) -> bool {
        SEAMS.with(|seams| {
            let mut seams = seams.borrow_mut();
            let mut fails = false;
            for (at, hook) in seams.iter_mut() {
                if *at == point {
                    fails |= hook();
                }
            }
            fails
        })
    }

    fn on(point: Point, hook: impl FnMut() -> bool + 'static) {
        SEAMS.with(|seams| seams.borrow_mut().push((point, Box::new(hook))));
    }

    /// A daemon on a throwaway store with the root agent `a`, no bus.
    fn a_daemon(dir: &std::path::Path) -> Arc<Daemon> {
        let paths = flottille_core::paths::Paths {
            runtime: dir.join("runtime"),
            state: dir.join("state"),
            config: dir.join("config"),
            data: dir.join("data"),
        };
        // The keyring is the daemon's memory, never the user's.
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("keyring.toml"), "backend = \"memory\"\n").unwrap();
        let mut store = flottille_core::store::Store::open_in_memory().unwrap();
        store
            .create_agent(
                &crate::daemon::tests::an_agent("a"),
                ("agent.created", json!({})),
            )
            .unwrap();
        let projects = crate::projects::Projects::open(&paths).unwrap();
        Arc::new(Daemon::new(&paths, store, projects, 0, None))
    }

    /// A request of `a` held for the user: its number.
    fn held(daemon: &Daemon) -> u64 {
        daemon
            .write(|store| {
                let (request, event) = store.ask(&Request {
                    number: 0,
                    agent: "a".into(),
                    call_key: None,
                    tool: "fs_read".into(),
                    arguments: json!({"path": ".env"}),
                    shown: json!({"path": ".env"}),
                    reasons: Vec::new(),
                    base: true,
                    kind: Kind::Call,
                    says: Vec::new(),
                    judgement: None,
                    held_by: approval::USER.into(),
                    forwards: Vec::new(),
                    state: RequestState::Pending,
                    decision: None,
                    created_at: 0,
                    decided_at: None,
                    place: None,
                    digest: None,
                })?;
                Ok((request.number, event))
            })
            .unwrap()
    }

    fn deny(daemon: &Daemon, number: u64) {
        daemon
            .write(|store| {
                let (_, event) = store
                    .decide(number, Decision::Deny, approval::USER)?
                    .unwrap();
                Ok(((), Some(event)))
            })
            .unwrap();
    }

    fn cursor(daemon: &Daemon) -> u64 {
        daemon
            .read(|store| store.notification_cursor())
            .unwrap()
            .unwrap()
    }

    fn last(daemon: &Daemon) -> u64 {
        daemon.read(|store| store.last_seq()).unwrap()
    }

    fn rows(daemon: &Daemon) -> Vec<notification::Notification> {
        daemon
            .read(|store| store.notifications(None, 100, false))
            .unwrap()
    }

    /// A keep on a thread of its own that stops at `point` until it is let go: the keep, the
    /// word that it stopped, and what lets it go.
    fn stopped_at(
        daemon: &Arc<Daemon>,
        point: Point,
    ) -> (std::thread::JoinHandle<()>, mpsc::Sender<()>) {
        let (stopped, stop) = mpsc::channel();
        let (go, going) = mpsc::channel::<()>();
        let kept = daemon.clone();
        let keeping = std::thread::spawn(move || {
            let mut once = Some((stopped, going));
            on(point, move || {
                if let Some((stopped, going)) = once.take() {
                    stopped.send(()).unwrap();
                    let _ = going.recv();
                }
                false
            });
            keep(&kept);
        });
        stop.recv_timeout(Duration::from_secs(10))
            .expect("the keep stopped where it was asked to");
        (keeping, go)
    }

    /// Another keep while the first is stopped: it is given time to run, or to wait for the
    /// first.
    fn another_keep(daemon: &Arc<Daemon>) -> (std::thread::JoinHandle<()>, mpsc::Receiver<()>) {
        let (done, finished) = mpsc::channel();
        let kept = daemon.clone();
        let keeping = std::thread::spawn(move || {
            keep(&kept);
            let _ = done.send(());
        });
        let _ = finished.recv_timeout(Duration::from_millis(300));
        (keeping, finished)
    }

    #[test]
    fn two_keeps_at_once_never_take_the_cursor_back() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        keep(&daemon);
        daemon.emit("remote.delivery_dropped", json!({})).unwrap();
        let (first, go) = stopped_at(&daemon, Point::Read);

        let later = daemon.emit("remote.delivery_dropped", json!({})).unwrap();
        let (second, _) = another_keep(&daemon);
        go.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();

        assert!(cursor(&daemon) >= later.seq, "the cursor went back");
        assert_eq!(rows(&daemon).len(), 2);
    }

    #[test]
    fn a_request_decided_while_the_keeper_looks_waits_for_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        keep(&daemon);
        let number = held(&daemon);
        let (first, go) = stopped_at(&daemon, Point::SeenPending);

        // Answered between the keeper's look and its row.
        deny(&daemon, number);
        let (second, _) = another_keep(&daemon);
        go.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();

        assert!(
            daemon
                .read(|store| store.unresolved_notifications("request"))
                .unwrap()
                .is_empty(),
            "a row waits for an answered request: {:?}",
            rows(&daemon)
        );
    }

    #[test]
    fn a_keep_with_nothing_new_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        daemon.emit("remote.delivery_dropped", json!({})).unwrap();
        keep(&daemon);
        // The keep after a change passes the keeper's own `notifications.changed`.
        keep(&daemon);
        let before = daemon.read(|store| Ok(store.changes_made())).unwrap();

        keep(&daemon);

        assert_eq!(
            daemon.read(|store| Ok(store.changes_made())).unwrap(),
            before
        );
    }

    #[test]
    fn what_waits_at_the_upgrade_is_in_the_centre_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let waiting = held(&daemon);
        let answered = held(&daemon);
        deny(&daemon, answered);
        let asked = daemon
            .emit(
                "vault.asked",
                json!({"agent": "a", "project": "app", "name": "DEPLOY_TOKEN"}),
            )
            .unwrap();
        let given = daemon
            .emit(
                "vault.asked",
                json!({"agent": "a", "project": "app", "name": "OTHER_TOKEN"}),
            )
            .unwrap();
        daemon
            .emit("vault.ask_answered", json!({"ask": given.seq}))
            .unwrap();
        daemon
            .write(|store| {
                let event = store.append_agent_event(
                    "a",
                    "agent.asked",
                    json!({"agent": "a", "to": "user", "question": "Which branch?"}),
                    false,
                )?;
                Ok(((), Some(event)))
            })
            .unwrap();
        // As the migration leaves it: at the last event there was.
        let upto = last(&daemon);
        daemon
            .read(|store| store.set_notification_cursor(upto))
            .unwrap();

        keep(&daemon);

        let mut kept: Vec<(String, String, bool)> = rows(&daemon)
            .into_iter()
            .map(|row| (row.kind, row.title, row.resolved))
            .collect();
        kept.sort();
        assert_eq!(
            kept,
            [
                (
                    "question".to_owned(),
                    "Asks you a question".to_owned(),
                    false
                ),
                (
                    "request".to_owned(),
                    format!("Request #{waiting} waits for you"),
                    false
                ),
                (
                    "vault".to_owned(),
                    "Asks you for a vault entry".to_owned(),
                    false
                ),
            ],
            "{asked:?}"
        );
    }

    #[test]
    fn a_question_of_a_session_that_ended_before_the_upgrade_is_not_in_the_centre() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        let mut gone = crate::daemon::tests::an_agent("gone");
        gone.state = flottille_core::agent::AgentState::Ended;
        daemon
            .write(|store| {
                let event = store.create_agent(&gone, ("agent.created", json!({})))?;
                Ok(((), Some(event)))
            })
            .unwrap();
        daemon
            .write(|store| {
                let event = store.append_agent_event(
                    "gone",
                    "agent.asked",
                    json!({"agent": "gone", "to": "user", "question": "Which branch?"}),
                    false,
                )?;
                Ok(((), Some(event)))
            })
            .unwrap();
        let upto = last(&daemon);
        daemon
            .read(|store| store.set_notification_cursor(upto))
            .unwrap();

        keep(&daemon);

        assert!(
            rows(&daemon).is_empty(),
            "a question of an ended session waits in the centre: {:?}",
            rows(&daemon)
        );
    }

    #[test]
    fn a_failed_write_keeps_its_event_for_the_next_keep() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        keep(&daemon);
        let dropped = daemon.emit("remote.delivery_dropped", json!({})).unwrap();
        on(Point::Add, || true);
        keep(&daemon);
        SEAMS.with(|seams| seams.borrow_mut().clear());
        assert!(rows(&daemon).is_empty());
        assert!(cursor(&daemon) < dropped.seq, "the cursor passed the event");

        keep(&daemon);

        assert_eq!(rows(&daemon).len(), 1);
    }

    #[test]
    fn what_an_extension_says_does_not_claim_to_need_the_user() {
        let dir = tempfile::tempdir().unwrap();
        let daemon = a_daemon(dir.path());
        keep(&daemon);
        daemon
            .emit(
                "connection.signed_out",
                json!({"name": "tracker", "why": "its token expired"}),
            )
            .unwrap();

        keep(&daemon);

        let kept = rows(&daemon);
        assert_eq!(kept.len(), 1);
        assert_ne!(kept[0].title, "Needs you");
        assert!(!kept[0].needs_you);
        assert_eq!(
            daemon.read(|store| store.unread_notifications()).unwrap(),
            (0, 1)
        );
    }

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
