//! Importing a project's `.claude` folder (`docs/skills.md`, `docs/commands.md`): the methods
//! `skill.detect`, `skill.import`, `command.detect` and `command.import`.
//!
//! An import writes entries in the project's file that point at the files of the folder, never a
//! copy of them, so that a file changed is read changed at its next use. Nothing is written in the
//! project's folder, and a file that cannot be imported whole is said with why and left out.

use flottille_core::claude_folder::{self, Entry};
use flottille_core::commands::Command;
use flottille_core::project::{Project, Skill};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::{self, Failure};

/// What is imported: both come from a folder of `.claude`, and are told apart only here.
#[derive(Clone, Copy)]
pub enum Kind {
    Skill,
    Command,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Command => "command",
        }
    }

    fn folder(self) -> &'static str {
        match self {
            Kind::Skill => claude_folder::SKILLS,
            Kind::Command => claude_folder::COMMANDS,
        }
    }

    fn entries(self, project: &Project) -> Vec<Entry> {
        match self {
            Kind::Skill => claude_folder::skills(&project.folder),
            Kind::Command => claude_folder::commands(&project.folder),
        }
    }

    /// What the project has under that name: the file it points at, or none for one that does not
    /// point at a file. `None` when the project has nothing of that name.
    fn known(self, project: &Project, name: &str) -> Option<Option<String>> {
        match self {
            Kind::Skill => project.skills.get(name).map(|skill| skill.path.clone()),
            Kind::Command => project
                .commands
                .get(name)
                .map(|command| command.path.clone()),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Detect {
    project: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    project: String,
    /// The names to import; all that can be, when left out.
    names: Option<Vec<String>>,
}

/// What became of a file in the project, as it is now.
enum State {
    /// Can be imported.
    New,
    /// The project already has it, pointing at that file.
    Imported,
    /// The project has something else under that name.
    Taken(String),
    /// The file cannot be imported, for that reason.
    Unusable(String),
}

impl State {
    fn word(&self) -> &'static str {
        match self {
            State::New => "new",
            State::Imported => "imported",
            State::Taken(_) => "taken",
            State::Unusable(_) => "unusable",
        }
    }

    fn problem(&self) -> Option<&str> {
        match self {
            State::Taken(why) | State::Unusable(why) => Some(why),
            State::New | State::Imported => None,
        }
    }
}

fn state(kind: Kind, project: &Project, entry: &Entry) -> State {
    if let Some(problem) = &entry.problem {
        return State::Unusable(problem.clone());
    }
    let name = entry.name.as_deref().unwrap_or_default();
    match kind.known(project, name) {
        None => State::New,
        Some(path) if path.as_deref() == Some(entry.path.as_str()) => State::Imported,
        Some(_) => State::Taken(format!(
            "project {} already has a {} named {name}: remove it first to import this one",
            project.name,
            kind.word()
        )),
    }
}

/// `skill.detect` and `command.detect`: what the project's `.claude` folder holds, each file with
/// the entry it would make, or why it would not make one.
pub fn detect(daemon: &Daemon, kind: Kind, params: Value) -> Result<Value, Failure> {
    let asked: Detect = serde_json::from_value(params)?;
    let project = projects::load_project(daemon, &asked.project)?;
    let found: Vec<Value> = kind
        .entries(&project)
        .iter()
        .map(|entry| {
            let state = state(kind, &project, entry);
            let mut shown = json!({"name": entry.name, "path": entry.path, "state": state.word()});
            if let Some(description) = &entry.description {
                shown["description"] = description.clone().into();
            }
            if let Some(problem) = state.problem() {
                shown["problem"] = problem.into();
            }
            shown
        })
        .collect();
    Ok(json!({"project": project.name, "folder": kind.folder(), "found": found}))
}

/// What an import does: the files that become entries, and the ones left out with why.
struct Plan {
    import: Vec<Entry>,
    skipped: Vec<Value>,
}

/// Which files are imported, as the project is now. Without names, all that can be, the others
/// said; with names, those, and a name that cannot be imported refuses the whole call so that
/// nothing is half done.
fn plan(kind: Kind, project: &Project, names: Option<&[String]>) -> Result<Plan, Failure> {
    let entries = kind.entries(project);
    let mut plan = Plan {
        import: Vec::new(),
        skipped: Vec::new(),
    };
    let Some(names) = names else {
        for entry in entries {
            match state(kind, project, &entry) {
                State::New => plan.import.push(entry),
                State::Imported => {}
                State::Taken(why) | State::Unusable(why) => plan
                    .skipped
                    .push(json!({"name": entry.name, "path": entry.path, "why": why})),
            }
        }
        return Ok(plan);
    };
    for name in names {
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.name.as_deref() == Some(name.as_str()))
        else {
            return Err(Failure::new(
                "not-found",
                format!(
                    "no {} named {name} is found in {} of project {}",
                    kind.word(),
                    kind.folder(),
                    project.name
                ),
            ));
        };
        match state(kind, project, entry) {
            State::New => {
                if !plan.import.iter().any(|known| known.name == entry.name) {
                    plan.import.push(entry.clone());
                }
            }
            State::Imported => {
                return Err(Failure::refused(format!(
                    "the {} {name} is already imported from {}",
                    kind.word(),
                    entry.path
                )));
            }
            State::Taken(why) => return Err(Failure::refused(why)),
            State::Unusable(why) => {
                return Err(Failure::refused(format!("{}: {why}", entry.path)));
            }
        }
    }
    Ok(plan)
}

/// `skill.import` and `command.import`: the files named, or all that can be, become entries of
/// the project's file that point at them. The file is written once, and only when there is
/// something to write.
pub fn import(daemon: &Daemon, kind: Kind, params: Value) -> Result<Value, Failure> {
    let asked: Import = serde_json::from_value(params)?;
    let names = asked.names.as_deref();
    let known = projects::load_project(daemon, &asked.project)?;
    let first = plan(kind, &known, names)?;
    if first.import.is_empty() {
        return Ok(answer(&known.name, &[], first.skipped));
    }
    let mut done = Vec::new();
    let mut skipped = Vec::new();
    projects::rewrite(daemon, &asked.project, |project, document| {
        // The project may have changed since it was read: the plan is made again on it.
        let plan = plan(kind, project, names)?;
        for entry in &plan.import {
            let name = entry.name.as_deref().unwrap_or_default();
            let description = entry.description.clone().unwrap_or_default();
            done.push(match kind {
                Kind::Skill => {
                    let skill = Skill {
                        description,
                        path: Some(entry.path.clone()),
                    };
                    crate::skills::put(document, &project.name, name, &skill)?;
                    crate::skills::to_json(name, &skill)
                }
                Kind::Command => {
                    let command = Command::file(description, &entry.path);
                    crate::commands::put(document, &project.name, name, &command)?;
                    crate::commands::to_json(&project.folder, name, &command, false)
                }
            });
        }
        skipped = plan.skipped;
        Ok(())
    })?;
    Ok(answer(&known.name, &done, skipped))
}

fn answer(project: &str, imported: &[Value], skipped: Vec<Value>) -> Value {
    json!({"project": project, "imported": imported, "skipped": skipped})
}
