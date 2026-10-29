//! The `command.*` methods (`docs/commands.md`): a project's named instructions, kept in its file
//! beside its tasks. `/name` at the start of a message to an agent (the user's to a root, a
//! parent's to its child) sends the command's instructions in its place. A command grants
//! nothing: what its agent does goes through the gate's classes like any call.

use flottille_core::commands::{self, Command};
use flottille_core::project::Project;
use flottille_core::workspace::Workspace;
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
struct Named {
    project: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Add {
    project: String,
    /// The template's name when it is left out.
    name: Option<String>,
    description: Option<String>,
    instructions: Option<String>,
    /// One of Flottille's commands, written as it is (`commands::TEMPLATES`).
    template: Option<String>,
}

impl Add {
    /// The command it adds, with its name.
    fn command(self) -> Result<(String, Command), Failure> {
        let Some(template) = self.template else {
            let (Some(name), Some(description), Some(instructions)) =
                (self.name, self.description, self.instructions)
            else {
                return Err(Failure::refused(
                    "a command is added with its name, its description and its instructions, or \
                     from a template",
                ));
            };
            return Ok((name, Command::new(description.trim(), instructions)));
        };
        if self.description.is_some() || self.instructions.is_some() {
            return Err(Failure::refused(
                "a template is added as it is: edit the command once it is in the project's file",
            ));
        }
        let command = commands::template(&template).ok_or_else(|| {
            Failure::new(
                "not-found",
                format!(
                    "there is no template {template}: the templates are {}",
                    commands::TEMPLATES.join(", ")
                ),
            )
        })?;
        Ok((self.name.unwrap_or(template), command))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    project: String,
    name: String,
    description: Option<String>,
    instructions: Option<String>,
}

/// Answers a `command.*` request.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Answer {
    match method {
        "command.list" => {
            let asked: OfProject = serde_json::from_value(params)?;
            let project = projects::load_project(daemon, &asked.project)?;
            Ok(
                json!({"project": project.name, "commands": project.commands.iter()
                .map(|(name, command)| to_json(name, command, false))
                .collect::<Vec<_>>()}),
            )
        }
        "command.show" => {
            let asked: Named = serde_json::from_value(params)?;
            let project = projects::load_project(daemon, &asked.project)?;
            Ok(to_json(&asked.name, find(&project, &asked.name)?, true))
        }
        "command.add" => {
            let asked: Add = serde_json::from_value(params)?;
            let project_name = asked.project.clone();
            let (name, command) = asked.command()?;
            command.check(&name).map_err(Failure::refused)?;
            let project = projects::rewrite(daemon, &project_name, |project, document| {
                if project.commands.contains_key(&name) {
                    return Err(Failure::refused(format!(
                        "project {} already has a command named {name}: edit it, or remove it \
                         first",
                        project.name
                    )));
                }
                write(document, &project.name, &name, &command)
            })?;
            Ok(to_json(&name, find(&project, &name)?, true))
        }
        "command.edit" => {
            let asked: Edit = serde_json::from_value(params)?;
            let name = asked.name.clone();
            let project = projects::rewrite(daemon, &asked.project, |project, document| {
                let mut command = find(project, &name)?.clone();
                if let Some(description) = asked.description {
                    command.description = description.trim().to_owned();
                }
                if let Some(instructions) = asked.instructions {
                    command.instructions = instructions;
                }
                command.check(&name).map_err(Failure::refused)?;
                write(document, &project.name, &name, &command)
            })?;
            Ok(to_json(&asked.name, find(&project, &asked.name)?, true))
        }
        "command.remove" => {
            let asked: Named = serde_json::from_value(params)?;
            let name = asked.name.clone();
            projects::rewrite(daemon, &asked.project, |project, document| {
                find(project, &name)?;
                let commands = document
                    .get_mut("commands")
                    .and_then(toml_edit::Item::as_table_like_mut)
                    .ok_or_else(|| not_a_table(&project.name))?;
                commands.remove(&name);
                if commands.is_empty() {
                    document.remove("commands");
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

fn find<'a>(project: &'a Project, name: &str) -> Result<&'a Command, Failure> {
    project.commands.get(name).ok_or_else(|| {
        Failure::new(
            "not-found",
            format!("project {} has no command named {name}", project.name),
        )
    })
}

fn not_a_table(project: &str) -> Failure {
    Failure::new(
        "invalid-config",
        format!("projects/{project}.toml: commands: must be a table"),
    )
}

/// Writes the command under `[commands.<name>]`, in place of what was there.
fn write(
    document: &mut toml_edit::DocumentMut,
    project: &str,
    name: &str,
    command: &Command,
) -> Result<(), Failure> {
    let mut table = toml_edit::Table::new();
    table.insert("description", toml_edit::value(&command.description));
    table.insert("instructions", toml_edit::value(&command.instructions));
    let commands = document.entry("commands").or_insert_with(|| {
        let mut commands = toml_edit::Table::new();
        // `[commands.<name>]` alone, without an empty `[commands]` above it.
        commands.set_implicit(true);
        toml_edit::Item::Table(commands)
    });
    let commands = commands
        .as_table_mut()
        .ok_or_else(|| not_a_table(project))?;
    // An entry edited keeps the comments the user wrote above it.
    match commands
        .get_mut(name)
        .and_then(toml_edit::Item::as_table_mut)
    {
        Some(entry) => {
            for (key, value) in table.iter() {
                entry.insert(key, value.clone());
            }
        }
        None => {
            commands.insert(name, toml_edit::Item::Table(table));
        }
    }
    Ok(())
}

/// A command as the protocol shows it; with its instructions when `whole`.
fn to_json(name: &str, command: &Command, whole: bool) -> Value {
    let mut shown = json!({
        "name": name,
        "description": command.description,
    });
    if whole {
        shown["instructions"] = command.instructions.clone().into();
    }
    shown
}

/// The message `text` stands for when it is sent to an agent of `project` in `workspace`:
/// `/name rest`, where name is one of the project's commands, is the command's instructions with
/// the rest after them, preceded by what the command is and, in a workspace, by its repositories,
/// the project's pull requests and the ticket, which the commands Flottille ships read. Any other
/// text is sent as it is.
pub fn message_for(daemon: &Daemon, project: &str, workspace: Option<&str>, text: &str) -> String {
    let Ok(project) = projects::load_project(daemon, project) else {
        return text.to_owned();
    };
    let Some((name, instructions)) = commands::expanded(text, &project.commands) else {
        return text.to_owned();
    };
    let mut message = format!(
        "The command {name} of project {}: {}\n",
        project.name,
        project.commands[&name].description.trim()
    );
    let workspace = workspace.and_then(|workspace| {
        daemon
            .read(|store| store.workspace(&project.name, workspace))
            .ok()
            .flatten()
    });
    if let Some(workspace) = &workspace {
        message.push_str(&format!(
            "\nIt runs in workspace {} (branch {}).\n",
            workspace.name, workspace.branch
        ));
        message.push_str(&repositories_text(&project, workspace));
        message.push_str(&pull_requests_text(&project));
        if let Some(ticket) = &workspace.ticket {
            message.push_str(&format!(
                "\nThe workspace is linked to the ticket {ticket} (ticket_read).\n"
            ));
        }
    }
    message.push_str("\n---\n\n");
    message.push_str(&instructions);
    message
}

/// The workspace's repositories by dependency level, as the project declares them
/// (`depends_on`), each with its base and its remote.
fn repositories_text(project: &Project, workspace: &Workspace) -> String {
    let paths: Vec<String> = workspace
        .repositories
        .iter()
        .map(|r| r.path.clone())
        .collect();
    let mut text = String::from(
        "\nIts repositories, by dependency level (a level is built on the levels before it):\n",
    );
    for (at, level) in project.dependency_levels(&paths).iter().enumerate() {
        let each: Vec<String> = level
            .iter()
            .filter_map(|path| workspace.repositories.iter().find(|r| r.path == *path))
            .map(|r| format!("{} (base {}, on {})", r.path, r.base.branch, r.base.remote))
            .collect();
        text.push_str(&format!("- level {}: {}\n", at + 1, each.join(", ")));
    }
    text
}

/// What the project's file says of its pull requests (`[pull_requests]`).
fn pull_requests_text(project: &Project) -> String {
    let settings = &project.pull_requests;
    let mut text = String::new();
    if let Some(title) = &settings.title {
        text.push_str(&format!(
            "\nA pull request's title follows the project's format: {}\n",
            title.trim()
        ));
    }
    if !settings.reviewers.is_empty() {
        text.push_str(&format!(
            "{}Flottille asks {} to review each pull request it opens.\n",
            if settings.title.is_some() { "" } else { "\n" },
            settings.reviewers.join(", ")
        ));
    }
    text
}
