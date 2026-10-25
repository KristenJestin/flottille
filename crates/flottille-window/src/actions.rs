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

/// Where a session is started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The project folder.
    Main,
    /// A workspace there is.
    Workspace(String),
    /// A workspace made for it, named here: it is created and prepared first.
    New(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// What the project page shows beyond the shell's read: branches and lines changed.
    Details(String),
    Start {
        project: String,
        target: Target,
        message: String,
    },
    CreateWorkspace {
        project: String,
        name: String,
    },
    PrepareWorkspace {
        project: String,
        name: String,
    },
    RemoveWorkspace {
        project: String,
        name: String,
    },
}

/// What one project page shows beyond the shell's read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub project: String,
    /// Each repository's path and the branch its checkout in the project folder is on.
    pub branches: Vec<(String, Option<String>)>,
    /// The lines added and removed in each workspace, all its repositories together.
    pub changes: HashMap<String, (u64, u64)>,
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
}

/// How long an answer is waited for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    /// What answers at once, or after git reads: the details, a session started.
    pub quick: Duration,
    /// What runs git's work (a fetch, a worktree made or removed): its own limit is 30 minutes.
    pub slow: Duration,
    /// While a new workspace is prepared, how long without an event before the daemon is asked
    /// whether it is still there.
    pub quiet: Duration,
}

const BOUNDS: Bounds = Bounds {
    quick: Duration::from_secs(120),
    slow: Duration::from_secs(31 * 60),
    quiet: Duration::from_secs(30),
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
            target: Target::New(name),
            message,
        } => start_in_new(connect, &project, &name, &message, bounds),
        Command::Start {
            project,
            target,
            message,
        } => {
            let workspace = match target {
                Target::Main => None,
                Target::Workspace(name) | Target::New(name) => Some(name),
            };
            match ask_alone(
                connect,
                "agent.start",
                start_params(&project, workspace.as_deref(), &message),
                bounds.quick,
            ) {
                Ok(Ok(started)) => started_outcome(&project, &started),
                Ok(Err(why)) | Err(why) => Outcome::NotStarted { project, why },
            }
        }
        Command::CreateWorkspace { project, name } => workspace_outcome(
            ask_alone(
                connect,
                "workspace.create",
                json!({"project": project, "name": name}),
                bounds.slow,
            ),
            project,
            name,
        ),
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

fn start_params(project: &str, workspace: Option<&str>, message: &str) -> Value {
    // No model: the agent's default, the only one known before its session opens.
    let mut params = json!({"project": project, "message": message});
    if let Some(workspace) = workspace {
        params["workspace"] = json!(workspace);
    }
    params
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
        let mut total = (0, 0);
        for repository in shown["repositories"].as_array().into_iter().flatten() {
            total.0 += repository["changes"]["added"].as_u64().unwrap_or(0);
            total.1 += repository["changes"]["removed"].as_u64().unwrap_or(0);
        }
        changes.insert(name.to_owned(), total);
    }
    Some(Details {
        project: project.to_owned(),
        branches,
        changes,
    })
}

/// Creates a workspace, waits for its preparation, then starts the session in it. A preparation
/// that fails starts nothing: its output is on the workspace's card. A preparation takes as long
/// as its recipe: while no event comes, the daemon is asked now and then whether it is still there.
fn start_in_new(
    connect: &Connect,
    project: &str,
    name: &str,
    message: &str,
    bounds: Bounds,
) -> Outcome {
    let not_started = |why: String| Outcome::NotStarted {
        project: project.to_owned(),
        why,
    };
    let mut conn = match connected(connect) {
        Ok(conn) => conn,
        Err(why) => return not_started(why),
    };
    // Subscribed first, so that no step of the preparation goes unseen.
    if let Err(why) = conn
        .ask_within("events.watch", json!({}), bounds.quick)
        .and_then(|a| a)
    {
        return not_started(why);
    }
    let created = match conn.ask_within(
        "workspace.create",
        json!({"project": project, "name": name}),
        bounds.slow,
    ) {
        Ok(Ok(created)) => created,
        Ok(Err(why)) | Err(why) => return not_started(why),
    };
    let mut state = created["state"].as_str().unwrap_or("preparing").to_owned();
    while state == "preparing" {
        let event = match conn.next_event(Some(bounds.quiet)) {
            Ok(Some(event)) => event,
            Ok(None) => {
                state = match still_preparing(&mut conn, project, name, bounds.quick) {
                    Ok(state) => state,
                    Err(why) => return not_started(why),
                };
                continue;
            }
            Err(why) => return not_started(why),
        };
        let ours = event.data["project"] == project && event.data["workspace"] == name;
        state = match (ours, event.name.as_str()) {
            (true, "workspace.ready") => "ready".into(),
            (true, "workspace.step_failed") => "failed".into(),
            (true, "workspace.removed") => "removed".into(),
            _ => state,
        };
    }
    if state != "ready" {
        return not_started(format!(
            "the preparation of workspace {name} failed: no session started"
        ));
    }
    match conn.ask_within(
        "agent.start",
        start_params(project, Some(name), message),
        bounds.quick,
    ) {
        Ok(Ok(started)) => started_outcome(project, &started),
        Ok(Err(why)) | Err(why) => not_started(why),
    }
}

/// The workspace's state as the daemon says it now (a listing: it runs no git); `removed` when it
/// is gone.
fn still_preparing(
    conn: &mut Conn,
    project: &str,
    name: &str,
    bound: Duration,
) -> Result<String, String> {
    let listed = conn.ask_within("workspace.list", json!({"project": project}), bound)??;
    Ok(listed["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|workspace| workspace["name"] == name)
        .map_or("removed", |workspace| {
            workspace["state"].as_str().unwrap_or("preparing")
        })
        .to_owned())
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
