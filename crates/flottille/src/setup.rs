//! The `setup.*` methods (`docs/protocol.md`, "The setup methods"): the steps that prepare a new
//! workspace, kept in the project's file as `[[setup]]` tables, in order. A step is named by its
//! number, from 1. The file is edited where it stands: a step keeps the comments the user wrote
//! above it, and moves with them.

use flottille_core::project::{Project, SetupStep, StepKind, check_step};
use serde::Deserialize;
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
struct Add {
    project: String,
    step: SetupStep,
    /// Its number once added; the last when left out.
    at: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    project: String,
    number: usize,
    step: SetupStep,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Numbered {
    project: String,
    number: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Move {
    project: String,
    number: usize,
    to: usize,
}

/// Answers a `setup.*` request.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Answer {
    match method {
        "setup.list" => {
            let asked: OfProject = serde_json::from_value(params)?;
            let project = projects::load_project(daemon, &asked.project)?;
            Ok(listed(&project))
        }
        "setup.add" => {
            let asked: Add = serde_json::from_value(params)?;
            let project = projects::rewrite(daemon, &asked.project, |project, document| {
                let len = project.setup.len();
                let at = asked.at.unwrap_or(len + 1);
                if !(1..=len + 1).contains(&at) {
                    return Err(Failure::refused(format!(
                        "at must be 1 to {}: the setup has {len} steps",
                        len + 1
                    )));
                }
                checked(project, &asked.step, at)?;
                let steps = steps_of(document, &project.name)?;
                steps.insert(at - 1, table_of(&asked.step));
                Ok(())
            })?;
            Ok(listed(&project))
        }
        "setup.edit" => {
            let asked: Edit = serde_json::from_value(params)?;
            let project = projects::rewrite(daemon, &asked.project, |project, document| {
                numbered(project, asked.number)?;
                checked(project, &asked.step, asked.number)?;
                let steps = steps_of(document, &project.name)?;
                let table = steps
                    .get_mut(asked.number - 1)
                    .expect("a step of the project is a table of the file");
                // What the user wrote above it stays.
                let keys: Vec<String> = table.iter().map(|(key, _)| key.to_owned()).collect();
                for key in keys {
                    table.remove(&key);
                }
                for (key, value) in table_of(&asked.step).iter() {
                    table.insert(key, value.clone());
                }
                Ok(())
            })?;
            Ok(listed(&project))
        }
        "setup.remove" => {
            let asked: Numbered = serde_json::from_value(params)?;
            let project = projects::rewrite(daemon, &asked.project, |project, document| {
                numbered(project, asked.number)?;
                let steps = steps_of(document, &project.name)?;
                steps.remove(asked.number - 1);
                if steps.is_empty() {
                    document.remove("setup");
                }
                Ok(())
            })?;
            Ok(listed(&project))
        }
        "setup.move" => {
            let asked: Move = serde_json::from_value(params)?;
            let project = projects::rewrite(daemon, &asked.project, |project, document| {
                numbered(project, asked.number)?;
                numbered(project, asked.to)?;
                let steps = steps_of(document, &project.name)?;
                // The places of the steps in the file stay; the steps change places.
                let places: Vec<Option<isize>> = steps.iter().map(|t| t.position()).collect();
                let step = steps.remove(asked.number - 1);
                steps.insert(asked.to - 1, step);
                for (table, place) in steps.iter_mut().zip(places) {
                    table.set_position(place);
                }
                Ok(())
            })?;
            Ok(listed(&project))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// The step of that number, or the refusal that says which there are.
pub fn numbered(project: &Project, number: usize) -> Result<&SetupStep, Failure> {
    number
        .checked_sub(1)
        .and_then(|index| project.setup.get(index))
        .ok_or_else(|| {
            Failure::new(
                "not-found",
                match project.setup.len() {
                    0 => format!("project {} has no setup step", project.name),
                    len => format!(
                        "project {} has no setup step {number}: its steps are 1 to {len}",
                        project.name
                    ),
                },
            )
        })
}

fn listed(project: &Project) -> Value {
    json!({"project": project.name, "steps": project.setup.iter().enumerate()
        .map(|(index, step)| to_json(index + 1, step))
        .collect::<Vec<_>>()})
}

/// A step as the protocol shows it: its number, then its keys as the file has them.
pub fn to_json(number: usize, step: &SetupStep) -> Value {
    let mut shown = serde_json::to_value(step).expect("a step is JSON");
    shown["number"] = number.into();
    shown
}

/// The `[[setup]]` tables of the file, made when there are none.
fn steps_of<'a>(
    document: &'a mut toml_edit::DocumentMut,
    project: &str,
) -> Result<&'a mut toml_edit::ArrayOfTables, Failure> {
    document
        .entry("setup")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| {
            Failure::new(
                "invalid-config",
                format!(
                    "projects/{project}.toml: setup: Flottille edits [[setup]] tables; write the \
                     steps that way to edit them here"
                ),
            )
        })
}

/// The step, refused as the file would be, `setup[i]` being the place it takes.
fn checked(project: &Project, step: &SetupStep, number: usize) -> Result<(), Failure> {
    let known = |path: &str| project.repositories.iter().any(|r| r.path == path);
    check_step(
        &format!("setup[{}]", number - 1),
        step,
        &known,
        &project.tasks,
    )
    .map_err(Failure::refused)
}

/// The keys of a checked step, in the order the user would write them.
fn table_of(step: &SetupStep) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    match step.kind() {
        StepKind::Copy(globs) => {
            table.insert(
                "copy",
                toml_edit::value(globs.iter().collect::<toml_edit::Array>()),
            );
        }
        StepKind::Link(path) => {
            table.insert("link", toml_edit::value(path));
        }
        StepKind::RootEntries => {
            table.insert("root_entries", toml_edit::value(true));
        }
        StepKind::Run(line) => {
            table.insert("run", toml_edit::value(line));
        }
        StepKind::Task(name) => {
            table.insert("task", toml_edit::value(name));
        }
    }
    if let Some(repository) = &step.repository {
        table.insert("repository", toml_edit::value(repository));
    }
    if step.for_each_repository {
        table.insert("for_each_repository", toml_edit::value(true));
    }
    if let Some(limit) = step.time_limit {
        table.insert("time_limit", toml_edit::value(limit as i64));
    }
    table
}
