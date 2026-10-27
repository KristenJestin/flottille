//! What the window asks of the daemon, off the UI thread: reading a project page's details,
//! creating, preparing and removing a workspace, starting a session. Each answer comes back to the
//! UI as an [`Update::Done`]; nothing here waits on the UI, and the UI never waits on it.
//!
//! The details are read on a thread and a connection of their own. Every other request has its
//! own thread and connection, so a workspace removed (git, up to half an hour) holds neither a
//! session started meanwhile nor the details. Every answer is waited for a bounded time: past it
//! the daemon is taken as silent, the request answers so (a session not started, a workspace
//! refused, the details not changed) and its connection is dropped.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};

use crate::link::{Conn, Connect, Update};
use crate::session_link::{PullRequest, pull_requests};

/// Where a session is started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The project folder.
    Main,
    /// A workspace there is.
    Workspace(String),
}

/// What a session is started on: each left out takes the daemon's default (the project's, else
/// the provider's).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// What the project page shows beyond the shell's read: branches and lines changed.
    Details(String),
    Start {
        project: String,
        target: Target,
        pick: Pick,
        message: String,
    },
    /// Enables or disables a provider in `providers.toml`.
    SetProvider {
        name: String,
        enabled: bool,
    },
    /// Reads the providers again: `providers.toml` is the user's to edit by hand too.
    ReadProviders,
    CreateWorkspace {
        project: String,
        name: String,
        /// The ticket to link it to: `#123` or a Jira key.
        ticket: Option<String>,
    },
    /// Links a workspace to a ticket, or to none (`ticket` empty of a null).
    LinkTicket {
        project: String,
        name: String,
        ticket: Option<String>,
    },
    PrepareWorkspace {
        project: String,
        name: String,
    },
    RemoveWorkspace {
        project: String,
        name: String,
    },
    /// A message to a session's root agent: the only agent the window writes to.
    Reply {
        session: String,
        message: String,
    },
    /// The user's answer to a request they hold, given on the page of the session of `session`
    /// (its root agent).
    Answer {
        session: String,
        number: u64,
        answer: Answer,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Deny,
    Once,
    /// Allowed for the root's tree.
    Tree,
}

/// What one project page shows beyond the shell's read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub project: String,
    /// Each repository's path and the branch its checkout in the project folder is on.
    pub branches: Vec<(String, Option<String>)>,
    /// The lines changed in each workspace, all its repositories together.
    pub changes: HashMap<String, Changes>,
    /// The pull requests of each workspace's repositories.
    pub pulls: HashMap<String, Vec<PullRequest>>,
}

/// The lines added and removed in a workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: u64,
    pub removed: u64,
    /// The daemon stopped reading new files: there are more lines than said.
    pub capped: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Details(Details),
    /// A session started: its root agent.
    Started {
        project: String,
        agent: String,
    },
    /// No session started, and why, in a sentence.
    NotStarted {
        project: String,
        why: String,
    },
    /// A workspace was created, prepared again or removed as asked.
    WorkspaceDone {
        project: String,
        name: String,
    },
    /// The daemon refused what was asked of a workspace, and why.
    WorkspaceRefused {
        project: String,
        name: String,
        why: String,
    },
    /// A workspace was linked to a ticket (or unlinked) as asked, or the daemon refused, and why.
    TicketLinked {
        project: String,
        name: String,
        refused: Option<String>,
    },
    /// The providers, read again, with the read's number (`providers::next_read`); `None` when
    /// the daemon could not be asked.
    Providers(u64, Option<crate::providers::Providers>),
    /// A provider was enabled or disabled as asked, or the daemon refused it with why.
    ProviderSet {
        name: String,
        refused: Option<String>,
    },
    /// A message to the session's root was taken, or refused with why.
    Replied {
        session: String,
        refused: Option<String>,
    },
    /// A request was decided as asked (`deny`, `once`, `tree`), or the answer refused with why:
    /// for the page of `session` only.
    Answered {
        session: String,
        number: u64,
        decision: Option<String>,
        refused: Option<String>,
    },
}

/// How long an answer is waited for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    /// What answers at once, or after git reads: the details, a session started.
    pub quick: Duration,
    /// What runs git's work (a fetch, a worktree made or removed): its own limit is 30 minutes.
    pub slow: Duration,
}

pub(crate) const BOUNDS: Bounds = Bounds {
    quick: Duration::from_secs(120),
    slow: Duration::from_secs(31 * 60),
};

/// The details are read at most once in this time: a burst of changes asks for them once.
const DETAILS_EVERY: Duration = Duration::from_secs(1);

/// The way to the actions.
#[derive(Clone)]
pub struct Actions {
    details: Sender<String>,
    connect: Connect,
    updates: UnboundedSender<Update>,
    bounds: Bounds,
}

impl Actions {
    /// Starts the details' thread; it ends when every `Actions` is dropped.
    pub fn spawn(connect: Connect, updates: UnboundedSender<Update>) -> Actions {
        Self::spawn_bounded(connect, updates, BOUNDS)
    }

    pub(crate) fn spawn_bounded(
        connect: Connect,
        updates: UnboundedSender<Update>,
        bounds: Bounds,
    ) -> Actions {
        let (details, asked) = channel();
        let (to, updates_to) = (connect.clone(), updates.clone());
        std::thread::Builder::new()
            .name("flottille-details".into())
            .spawn(move || read_details(to, asked, updates_to, bounds))
            .expect("a thread for the project page's details");
        Actions {
            details,
            connect,
            updates,
            bounds,
        }
    }

    pub fn send(&self, command: Command) {
        if let Command::Details(project) = command {
            let _ = self.details.send(project);
            return;
        }
        let (connect, updates, bounds) = (self.connect.clone(), self.updates.clone(), self.bounds);
        std::thread::Builder::new()
            .name("flottille-action".into())
            .spawn(move || {
                let outcome = perform(&connect, command, bounds);
                let _ = updates.unbounded_send(Update::Done(outcome));
            })
            .expect("a thread for a window's action");
    }
}

fn read_details(
    connect: Connect,
    asked: Receiver<String>,
    updates: UnboundedSender<Update>,
    bounds: Bounds,
) {
    let mut conn: Option<Conn> = None;
    while let Ok(first) = asked.recv() {
        let began = Instant::now();
        // Details asked again before they were read are read once.
        let mut projects = vec![first];
        for project in asked.try_iter() {
            if !projects.contains(&project) {
                projects.push(project);
            }
        }
        for project in projects {
            if let Some(details) = details(&connect, &mut conn, &project, bounds.quick)
                && updates
                    .unbounded_send(Update::Done(Outcome::Details(details)))
                    .is_err()
            {
                return;
            }
        }
        std::thread::sleep(DETAILS_EVERY.saturating_sub(began.elapsed()));
    }
}

/// The daemon's answer on a kept connection, made again when it broke or went silent.
fn ask(
    connect: &Connect,
    conn: &mut Option<Conn>,
    method: &str,
    params: Value,
    bound: Duration,
) -> Result<Result<Value, String>, String> {
    if conn.is_none() {
        *conn = Some(connected(connect)?);
    }
    let answer = conn
        .as_mut()
        .expect("connected")
        .ask_within(method, params, bound);
    if answer.is_err() {
        *conn = None;
    }
    answer
}

fn connected(connect: &Connect) -> Result<Conn, String> {
    let stream = connect().map_err(|e| format!("cannot reach the daemon: {e}"))?;
    Conn::new(stream).map_err(|e| e.to_string())
}

/// One request on a connection of its own.
fn ask_alone(
    connect: &Connect,
    method: &str,
    params: Value,
    bound: Duration,
) -> Result<Result<Value, String>, String> {
    connected(connect)?.ask_within(method, params, bound)
}

fn perform(connect: &Connect, command: Command, bounds: Bounds) -> Outcome {
    match command {
        Command::Details(_) => unreachable!("the details have their own thread"),
        Command::Start {
            project,
            target,
            pick,
            message,
        } => {
            let workspace = match target {
                Target::Main => None,
                Target::Workspace(name) => Some(name),
            };
            match ask_alone(
                connect,
                "agent.start",
                start_params(&project, workspace.as_deref(), &pick, &message),
                bounds.quick,
            ) {
                Ok(Ok(started)) => started_outcome(&project, &started),
                Ok(Err(why)) | Err(why) => Outcome::NotStarted { project, why },
            }
        }
        Command::SetProvider { name, enabled } => {
            let method = if enabled {
                "provider.enable"
            } else {
                "provider.disable"
            };
            let refused = match ask_alone(connect, method, json!({"name": name}), bounds.quick) {
                Ok(Ok(_)) => None,
                Ok(Err(why)) | Err(why) => Some(why),
            };
            Outcome::ProviderSet { name, refused }
        }
        Command::ReadProviders => {
            let read = crate::providers::next_read();
            Outcome::Providers(read, read_providers(connect, bounds.quick))
        }
        Command::CreateWorkspace {
            project,
            name,
            ticket,
        } => workspace_outcome(
            ask_alone(
                connect,
                "workspace.create",
                json!({"project": project, "name": name, "ticket": ticket}),
                bounds.slow,
            ),
            project,
            name,
        ),
        Command::LinkTicket {
            project,
            name,
            ticket,
        } => match ask_alone(
            connect,
            "workspace.link_ticket",
            json!({"project": project, "name": name, "ticket": ticket}),
            bounds.slow,
        ) {
            Ok(Ok(_)) => Outcome::TicketLinked {
                project,
                name,
                refused: None,
            },
            Ok(Err(why)) | Err(why) => Outcome::TicketLinked {
                project,
                name,
                refused: Some(why),
            },
        },
        Command::PrepareWorkspace { project, name } => workspace_outcome(
            ask_alone(
                connect,
                "workspace.prepare",
                json!({"project": project, "name": name}),
                bounds.slow,
            ),
            project,
            name,
        ),
        Command::Reply { session, message } => match ask_alone(
            connect,
            "agent.send",
            json!({"agent": session, "message": message}),
            bounds.quick,
        ) {
            Ok(Ok(_)) => Outcome::Replied {
                session,
                refused: None,
            },
            Ok(Err(why)) | Err(why) => Outcome::Replied {
                session,
                refused: Some(why),
            },
        },
        Command::Answer {
            session,
            number,
            answer,
        } => {
            let (method, params) = match answer {
                Answer::Deny => ("approval.deny", json!({"number": number})),
                Answer::Once => ("approval.approve", json!({"number": number})),
                Answer::Tree => (
                    "approval.approve",
                    json!({"number": number, "for_tree": true}),
                ),
            };
            match ask_alone(connect, method, params, bounds.quick) {
                Ok(Ok(answered)) => Outcome::Answered {
                    session,
                    number,
                    decision: answered["request"]["decision"].as_str().map(str::to_owned),
                    refused: None,
                },
                Ok(Err(why)) | Err(why) => Outcome::Answered {
                    session,
                    number,
                    decision: None,
                    refused: Some(why),
                },
            }
        }
        Command::RemoveWorkspace { project, name } => workspace_outcome(
            ask_alone(
                connect,
                "workspace.remove",
                json!({"project": project, "name": name}),
                bounds.slow,
            ),
            project,
            name,
        ),
    }
}

/// The providers on a connection of their own, or the daemon's sentence when it cannot list them.
/// `None` when the daemon did not answer: that changes nothing, and the link reads them again once
/// it is back.
pub(crate) fn read_providers(
    connect: &Connect,
    bound: Duration,
) -> Option<crate::providers::Providers> {
    match ask_alone(connect, "provider.list", json!({}), bound) {
        Ok(listed) => Some(crate::providers::Providers::of(
            listed.as_ref().map_err(Clone::clone),
        )),
        Err(_) => None,
    }
}

fn workspace_outcome(
    answer: Result<Result<Value, String>, String>,
    project: String,
    name: String,
) -> Outcome {
    match answer {
        Ok(Ok(_)) => Outcome::WorkspaceDone { project, name },
        Ok(Err(why)) | Err(why) => Outcome::WorkspaceRefused { project, name, why },
    }
}

fn start_params(project: &str, workspace: Option<&str>, pick: &Pick, message: &str) -> Value {
    // What is left out is sent as a null, which the daemon reads as absent: its to choose.
    json!({
        "project": project,
        "workspace": workspace,
        "provider": pick.provider,
        "model": pick.model,
        "effort": pick.effort,
        "message": message,
    })
}

fn started_outcome(project: &str, started: &Value) -> Outcome {
    match started["agent"]["id"].as_str() {
        Some(agent) => Outcome::Started {
            project: project.to_owned(),
            agent: agent.to_owned(),
        },
        None => Outcome::NotStarted {
            project: project.to_owned(),
            why: "the daemon did not say which agent it started".into(),
        },
    }
}

/// Reads the branches of the project's repositories and the lines changed in its workspaces.
/// `None` when the daemon cannot be asked: the page keeps what it showed.
fn details(
    connect: &Connect,
    conn: &mut Option<Conn>,
    project: &str,
    bound: Duration,
) -> Option<Details> {
    let shown = ask(
        connect,
        conn,
        "project.show",
        json!({"name": project}),
        bound,
    )
    .ok()?
    .ok()?;
    let branches = shown["repositories"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|repository| {
            (
                repository["path"].as_str().unwrap_or_default().to_owned(),
                repository["branch"].as_str().map(str::to_owned),
            )
        })
        .collect();
    let listed = ask(
        connect,
        conn,
        "workspace.list",
        json!({"project": project}),
        bound,
    )
    .ok()?
    .ok()?;
    let mut changes = HashMap::new();
    let mut pulls = HashMap::new();
    for workspace in listed["workspaces"].as_array().into_iter().flatten() {
        let name = workspace["name"].as_str().unwrap_or_default();
        let Ok(Ok(shown)) = ask(
            connect,
            conn,
            "workspace.show",
            json!({"project": project, "name": name}),
            bound,
        ) else {
            continue;
        };
        let mut total = Changes::default();
        for repository in shown["repositories"].as_array().into_iter().flatten() {
            let changes = &repository["changes"];
            total.added += changes["added"].as_u64().unwrap_or(0);
            total.removed += changes["removed"].as_u64().unwrap_or(0);
            total.capped |= changes["capped"] == true;
        }
        changes.insert(name.to_owned(), total);
        if let Ok(Ok(forge)) = ask(
            connect,
            conn,
            "workspace.forge",
            json!({"project": project, "name": name}),
            bound,
        ) {
            pulls.insert(name.to_owned(), pull_requests(&forge).0);
        }
    }
    Some(Details {
        project: project.to_owned(),
        branches,
        changes,
        pulls,
    })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    use futures::channel::mpsc::{UnboundedReceiver, unbounded};

    use super::*;

    /// A daemon on socket pairs that answers `agent.start` at once and never answers what is in
    /// `silent`.
    fn daemon(silent: &'static [&'static str]) -> Connect {
        Arc::new(move || {
            let (window, daemon) = UnixStream::pair()?;
            std::thread::spawn(move || {
                let mut writer = daemon.try_clone().unwrap();
                for line in BufReader::new(daemon).lines() {
                    let Ok(line) = line else { return };
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let method = request["method"].as_str().unwrap();
                    if silent.contains(&method) {
                        continue;
                    }
                    let result = match method {
                        "agent.start" => json!({"agent": {"id": "made-up-agent"}}),
                        _ => json!({}),
                    };
                    let answer = json!({"id": request["id"], "result": result});
                    writeln!(writer, "{answer}").unwrap();
                }
            });
            Ok(window)
        })
    }

    fn next(updates: &mut UnboundedReceiver<Update>, within: Duration) -> Option<Outcome> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Ok(Update::Done(outcome)) = updates.try_recv() {
                return Some(outcome);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn start_in_main() -> Command {
        Command::Start {
            project: "atlas".into(),
            target: Target::Main,
            pick: Pick::default(),
            message: "Fix the timeout".into(),
        }
    }

    #[test]
    fn a_session_started_while_a_workspace_is_removed_does_not_wait_for_it() {
        let (sender, mut updates) = unbounded();
        let actions = Actions::spawn(daemon(&["workspace.remove"]), sender);
        actions.send(Command::RemoveWorkspace {
            project: "atlas".into(),
            name: "login".into(),
        });
        actions.send(start_in_main());

        assert_eq!(
            next(&mut updates, Duration::from_secs(5)),
            Some(Outcome::Started {
                project: "atlas".into(),
                agent: "made-up-agent".into()
            })
        );
    }

    #[test]
    fn a_daemon_that_does_not_answer_is_said_after_a_while() {
        let (sender, mut updates) = unbounded();
        let bounds = Bounds {
            quick: Duration::from_millis(200),
            ..BOUNDS
        };
        let actions = Actions::spawn_bounded(daemon(&["agent.start"]), sender, bounds);
        actions.send(start_in_main());

        let Some(Outcome::NotStarted { project, why }) = next(&mut updates, Duration::from_secs(5))
        else {
            panic!("no answer of the window's own");
        };
        assert_eq!(project, "atlas");
        assert_eq!(why, "the daemon did not answer agent.start within 200 ms");
    }
}
