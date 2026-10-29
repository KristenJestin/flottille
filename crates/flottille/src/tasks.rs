//! The `task.*` methods (`docs/protocol.md`, "The task methods"): the project's named command
//! lines, kept in its file under `[tasks.<name>]`, and the user's run of one from the window. The
//! file is edited where it stands, keeping what the user wrote around the entry.

use std::sync::Arc;

use flottille_core::project::{Project, Task};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::{self, Failure};

type Answer = Result<Value, Failure>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OfProject {
    project: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Named {
    project: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Add {
    project: String,
    name: String,
    line: String,
    repository: Option<String>,
    #[serde(default)]
    serve: bool,
    #[serde(default)]
    check: bool,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    ask_before_running: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    project: String,
    name: String,
    line: Option<String>,
    /// Left out: unchanged; `null`: the task runs at the top of its place.
    #[serde(default, deserialize_with = "present")]
    repository: Option<Option<String>>,
    serve: Option<bool>,
    check: Option<bool>,
    read_only: Option<bool>,
    ask_before_running: Option<bool>,
}

/// A field given, `null` included.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

impl Edit {
    /// The task as this edit leaves it.
    pub fn applied(&self, task: &Task) -> Task {
        let mut edited = task.clone();
        if let Some(line) = &self.line {
            edited.line = line.trim().to_owned();
        }
        if let Some(repository) = &self.repository {
            edited.repository = repository.clone().filter(|r| !r.is_empty());
        }
        for (field, value) in [
            (&mut edited.serve, self.serve),
            (&mut edited.check, self.check),
            (&mut edited.read_only, self.read_only),
            (&mut edited.ask_before_running, self.ask_before_running),
        ] {
            if let Some(value) = value {
                *field = value;
            }
        }
        edited
    }
}

/// Answers a `task.*` request but `task.run`, which `run` answers.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Answer {
    match method {
        "task.list" => {
            let asked: OfProject = serde_json::from_value(params)?;
            let project = projects::load_project(daemon, &asked.project)?;
            Ok(
                json!({"project": project.name, "tasks": project.tasks.iter()
                .map(|(name, task)| to_json(name, task))
                .collect::<Vec<_>>()}),
            )
        }
        "task.show" => {
            let asked: Named = serde_json::from_value(params)?;
            let project = projects::load_project(daemon, &asked.project)?;
            Ok(to_json(&asked.name, find(&project, &asked.name)?))
        }
        "task.add" => {
            let asked: Add = serde_json::from_value(params)?;
            let name = asked.name.clone();
            let task = Task {
                line: asked.line.trim().to_owned(),
                repository: asked.repository.filter(|r| !r.is_empty()),
                serve: asked.serve,
                check: asked.check,
                read_only: asked.read_only,
                ask_before_running: asked.ask_before_running,
                added_by: None,
            };
            let project = projects::rewrite(daemon, &asked.project, |project, document| {
                if project.tasks.contains_key(&name) {
                    return Err(Failure::refused(format!(
                        "project {} already has a task named {name}: edit it, or remove it first",
                        project.name
                    )));
                }
                write(document, &project.name, &name, &task)
            })?;
            Ok(to_json(&name, find(&project, &name)?))
        }
        "task.edit" => {
            let asked: Edit = serde_json::from_value(params)?;
            let name = asked.name.clone();
            let project = projects::rewrite(daemon, &asked.project, |project, document| {
                let task = asked.applied(find(project, &name)?);
                write(document, &project.name, &name, &task)
            })?;
            Ok(to_json(&name, find(&project, &name)?))
        }
        "task.remove" => {
            let asked: Named = serde_json::from_value(params)?;
            let name = asked.name.clone();
            projects::rewrite(daemon, &asked.project, |project, document| {
                find(project, &name)?;
                if let Some(at) = project
                    .setup
                    .iter()
                    .position(|step| step.task.as_deref() == Some(name.as_str()))
                {
                    return Err(Failure::refused(format!(
                        "step {} of the setup runs the task {name}: change or remove that step \
                         first",
                        at + 1
                    )));
                }
                let tasks = document
                    .get_mut("tasks")
                    .and_then(toml_edit::Item::as_table_like_mut)
                    .ok_or_else(|| not_a_table(&project.name))?;
                tasks.remove(&name);
                if tasks.is_empty() {
                    document.remove("tasks");
                }
                Ok(())
            })?;
            Ok(json!({"project": asked.project, "removed": asked.name}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// `task.run`: the user runs a task in the project folder; answered with its run at once.
pub fn run(daemon: &Arc<Daemon>, params: Value) -> Answer {
    let asked: Named = serde_json::from_value(params)?;
    let project = projects::load_project(daemon, &asked.project)?;
    find(&project, &asked.name)?;
    let run = crate::runs::run_for_user(daemon, &project, &asked.name).map_err(Failure::refused)?;
    Ok(json!({"project": project.name, "name": asked.name, "run": run}))
}

fn find<'a>(project: &'a Project, name: &str) -> Result<&'a Task, Failure> {
    project.tasks.get(name).ok_or_else(|| {
        Failure::new(
            "not-found",
            format!("project {} has no task named {name}", project.name),
        )
    })
}

fn not_a_table(project: &str) -> Failure {
    Failure::new(
        "invalid-config",
        format!("projects/{project}.toml: tasks: must be a table"),
    )
}

/// Writes the task under `[tasks.<name>]`, in place of what was there: a flag that is off is
/// left out, as the user would write it. A line no run could split is refused, as a run of it
/// would be.
fn write(
    document: &mut toml_edit::DocumentMut,
    project: &str,
    name: &str,
    task: &Task,
) -> Result<(), Failure> {
    flottille_core::command::split_line(&task.line).map_err(Failure::refused)?;
    let tasks = document.entry("tasks").or_insert_with(|| {
        let mut tasks = toml_edit::Table::new();
        // `[tasks.<name>]` alone, without an empty `[tasks]` above it.
        tasks.set_implicit(true);
        toml_edit::Item::Table(tasks)
    });
    let tasks = tasks
        .as_table_like_mut()
        .ok_or_else(|| not_a_table(project))?;
    if tasks.get(name).is_none() {
        tasks.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    // An entry edited keeps the comments the user wrote above it, and its form, inline or not.
    let entry = tasks
        .get_mut(name)
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| {
            Failure::new(
                "invalid-config",
                format!("projects/{project}.toml: tasks.{name}: must be a table"),
            )
        })?;
    entry.insert("line", toml_edit::value(&task.line));
    match &task.repository {
        Some(repository) => {
            entry.insert("repository", toml_edit::value(repository));
        }
        None => {
            entry.remove("repository");
        }
    }
    for (key, set) in [
        ("serve", task.serve),
        ("check", task.check),
        ("read_only", task.read_only),
        ("ask_before_running", task.ask_before_running),
    ] {
        if set {
            entry.insert(key, toml_edit::value(true));
        } else {
            entry.remove(key);
        }
    }
    Ok(())
}

/// A task as the protocol shows it.
pub fn to_json(name: &str, task: &Task) -> Value {
    json!({
        "name": name,
        "line": task.line,
        "repository": task.repository,
        "serve": task.serve,
        "check": task.check,
        "read_only": task.read_only,
        "ask_before_running": task.ask_before_running,
    })
}
