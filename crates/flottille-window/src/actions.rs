//! What the window asks of the daemon, off the UI thread: reading a project page's details,
//! creating, preparing and removing a workspace, starting a session. Each answer comes back to the
//! UI as an [`Update::Done`]; nothing here waits on the UI, and the UI never waits on it.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

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

/// The way to the actions' thread.
#[derive(Clone)]
pub struct Actions {
    commands: Sender<Command>,
}

impl Actions {
    /// Starts the thread; it ends when every `Actions` is dropped.
    pub fn spawn(connect: Connect, updates: UnboundedSender<Update>) -> Actions {
        let (commands, received) = channel();
        std::thread::Builder::new()
            .name("flottille-actions".into())
            .spawn(move || run(connect, received, updates))
            .expect("a thread for the window's actions");
        Actions { commands }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

fn run(connect: Connect, commands: Receiver<Command>, updates: UnboundedSender<Update>) {
    let mut conn: Option<Conn> = None;
    while let Ok(first) = commands.recv() {
        // Details asked again before they were read are read once.
        let mut batch = vec![first];
        batch.extend(commands.try_iter());
        let batch: Vec<Command> = batch
            .iter()
            .enumerate()
            .filter(|(at, command)| match command {
                Command::Details(_) => !batch[at + 1..].contains(command),
                _ => true,
            })
            .map(|(_, command)| command.clone())
            .collect();
        for command in batch {
            let outcome = match command {
                // It waits on the preparation: on a thread and a connection of its own, so that
                // nothing else waits for it.
                Command::Start {
                    project,
                    target: Target::New(name),
                    message,
                } => {
                    let (connect, updates) = (connect.clone(), updates.clone());
                    std::thread::spawn(move || {
                        let outcome = start_in_new(&connect, &project, &name, &message);
                        let _ = updates.unbounded_send(Update::Done(outcome));
                    });
                    continue;
                }
                command => perform(&connect, &mut conn, command),
            };
            if let Some(outcome) = outcome
                && updates.unbounded_send(Update::Done(outcome)).is_err()
            {
                return;
            }
        }
    }
}

/// The daemon's answer on the thread's connection, made again when it broke.
fn ask(
    connect: &Connect,
    conn: &mut Option<Conn>,
    method: &str,
    params: Value,
) -> Result<Result<Value, String>, String> {
    if conn.is_none() {
        let stream = connect().map_err(|e| format!("cannot reach the daemon: {e}"))?;
        *conn = Some(Conn::new(stream).map_err(|e| e.to_string())?);
    }
    let answer = conn.as_mut().expect("connected").ask(method, params);
    if answer.is_err() {
        *conn = None;
    }
    answer
}

fn perform(connect: &Connect, conn: &mut Option<Conn>, command: Command) -> Option<Outcome> {
    match command {
        Command::Details(project) => details(connect, conn, &project).map(Outcome::Details),
        Command::Start {
            project,
            target,
            message,
        } => {
            let workspace = match target {
                Target::Main => None,
                Target::Workspace(name) | Target::New(name) => Some(name),
            };
            Some(
                match ask(
                    connect,
                    conn,
                    "agent.start",
                    start_params(&project, workspace.as_deref(), &message),
                ) {
                    Ok(Ok(started)) => started_outcome(&project, &started),
                    Ok(Err(why)) | Err(why) => Outcome::NotStarted { project, why },
                },
            )
        }
        Command::CreateWorkspace { project, name } => Some(workspace_outcome(
            ask(
                connect,
                conn,
                "workspace.create",
                json!({"project": project, "name": name}),
            ),
            project,
            name,
        )),
        Command::PrepareWorkspace { project, name } => Some(workspace_outcome(
            ask(
                connect,
                conn,
                "workspace.prepare",
                json!({"project": project, "name": name}),
            ),
            project,
            name,
        )),
        Command::RemoveWorkspace { project, name } => Some(workspace_outcome(
            ask(
                connect,
                conn,
                "workspace.remove",
                json!({"project": project, "name": name}),
            ),
            project,
            name,
        )),
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
fn details(connect: &Connect, conn: &mut Option<Conn>, project: &str) -> Option<Details> {
    let shown = ask(connect, conn, "project.show", json!({"name": project}))
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
    let listed = ask(connect, conn, "workspace.list", json!({"project": project}))
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
/// that fails starts nothing: its output is on the workspace's card.
fn start_in_new(connect: &Connect, project: &str, name: &str, message: &str) -> Outcome {
    let not_started = |why: String| Outcome::NotStarted {
        project: project.to_owned(),
        why,
    };
    let mut conn = match connect()
        .map_err(|e| format!("cannot reach the daemon: {e}"))
        .and_then(|stream| Conn::new(stream).map_err(|e| e.to_string()))
    {
        Ok(conn) => conn,
        Err(why) => return not_started(why),
    };
    // Subscribed first, so that no step of the preparation goes unseen.
    if let Err(why) = conn.ask("events.watch", json!({})).and_then(|a| a) {
        return not_started(why);
    }
    let created = match conn.ask(
        "workspace.create",
        json!({"project": project, "name": name}),
    ) {
        Ok(Ok(created)) => created,
        Ok(Err(why)) | Err(why) => return not_started(why),
    };
    let mut state = created["state"].as_str().unwrap_or("preparing").to_owned();
    while state == "preparing" {
        let event = match conn.next_event(None) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
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
    match conn.ask("agent.start", start_params(project, Some(name), message)) {
        Ok(Ok(started)) => started_outcome(project, &started),
        Ok(Err(why)) | Err(why) => not_started(why),
    }
}
