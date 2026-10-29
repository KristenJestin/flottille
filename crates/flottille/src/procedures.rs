//! The `procedure.*` methods and the runs of procedures (`docs/procedures.md`): a project's named
//! actions, kept in its file beside its recipe, each run by a root agent in a workspace as plain
//! instructions. A run grants nothing: its agents' calls go through the gate's classes.

use std::sync::Arc;

use flottille_core::approval;
use flottille_core::procedure::{self, Procedure};
use flottille_core::project::Project;
use flottille_core::store::{ProcedureRun, Store, now_ms};
use flottille_core::workspace::Workspace;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agents::{self, New};
use crate::daemon::Daemon;
use crate::deliveries;
use crate::projects::{self, Failure};
use crate::roles;

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
    /// One of Flottille's procedures, written as it is (`procedure::TEMPLATES`).
    template: Option<String>,
}

impl Add {
    /// The procedure it adds, with its name.
    fn procedure(self) -> Result<(String, Procedure), Failure> {
        let Some(template) = self.template else {
            let (Some(name), Some(description), Some(instructions)) =
                (self.name, self.description, self.instructions)
            else {
                return Err(Failure::refused(
                    "a procedure is added with its name, its description and its instructions, \
                     or from a template",
                ));
            };
            return Ok((
                name,
                Procedure {
                    description,
                    instructions,
                    grants: None,
                    children: None,
                },
            ));
        };
        if self.description.is_some() || self.instructions.is_some() {
            return Err(Failure::refused(
                "a template is added as it is: edit the procedure once it is in the project's \
                 file",
            ));
        }
        let procedure = procedure::template(&template).ok_or_else(|| {
            Failure::new(
                "not-found",
                format!(
                    "there is no template {template}: the templates are {}",
                    procedure::TEMPLATES.join(", ")
                ),
            )
        })?;
        Ok((self.name.unwrap_or(template), procedure))
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OfAgent {
    agent: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    project: String,
    name: String,
    workspace: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    project: String,
    name: String,
    workspace: String,
}

/// Answers a `procedure.*` request.
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Answer {
    match method {
        "procedure.list" => {
            let asked: OfProject = serde_json::from_value(params)?;
            let project = projects::load_project(&daemon, &asked.project)?;
            Ok(
                json!({"project": project.name, "procedures": project.procedures.iter()
                .map(|(name, procedure)| to_json(name, procedure, false))
                .collect::<Vec<_>>()}),
            )
        }
        "procedure.show" => {
            let asked: Named = serde_json::from_value(params)?;
            let project = projects::load_project(&daemon, &asked.project)?;
            let procedure = find(&project, &asked.name)?;
            Ok(to_json(&asked.name, procedure, true))
        }
        "procedure.add" => {
            let asked: Add = serde_json::from_value(params)?;
            let project_name = asked.project.clone();
            let (name, procedure) = asked.procedure()?;
            procedure.check(&name).map_err(Failure::refused)?;
            let project = projects::rewrite(&daemon, &project_name, |project, document| {
                if project.procedures.contains_key(&name) {
                    return Err(Failure::refused(format!(
                        "project {} already has a procedure named {name}: edit it, or remove it \
                         first",
                        project.name
                    )));
                }
                write(document, &project.name, &name, &procedure)
            })?;
            Ok(to_json(&name, find(&project, &name)?, true))
        }
        "procedure.edit" => {
            let asked: Edit = serde_json::from_value(params)?;
            let name = asked.name.clone();
            let project = projects::rewrite(&daemon, &asked.project, |project, document| {
                let mut procedure = find(project, &name)?.clone();
                if let Some(description) = asked.description {
                    procedure.description = description;
                }
                if let Some(instructions) = asked.instructions {
                    procedure.instructions = instructions;
                }
                procedure.check(&name).map_err(Failure::refused)?;
                write(document, &project.name, &name, &procedure)
            })?;
            Ok(to_json(&asked.name, find(&project, &asked.name)?, true))
        }
        "procedure.remove" => {
            let asked: Named = serde_json::from_value(params)?;
            let name = asked.name.clone();
            projects::rewrite(&daemon, &asked.project, |project, document| {
                find(project, &name)?;
                let procedures = document
                    .get_mut("procedures")
                    .and_then(toml_edit::Item::as_table_like_mut)
                    .ok_or_else(|| not_a_table(&project.name))?;
                procedures.remove(&name);
                if procedures.is_empty() {
                    document.remove("procedures");
                }
                Ok(())
            })?;
            Ok(json!({"project": asked.project, "removed": asked.name}))
        }
        "procedure.run" => {
            let asked: Run = serde_json::from_value(params)?;
            let (project, procedure) = runnable(&daemon, &asked.project, &asked.name)?;
            workspace_of(&daemon, &project, &asked.workspace)?;
            let mut shown = to_json(&asked.name, &procedure, false);
            shown["project"] = project.name.clone().into();
            shown["workspace"] = asked.workspace.into();
            shown["started"] = false.into();
            Ok(shown)
        }
        "procedure.start" => start(&daemon, serde_json::from_value(params)?),
        "procedure.of" => {
            let asked: OfAgent = serde_json::from_value(params)?;
            let run = daemon.read(|store| store.procedure_run(&asked.agent))?;
            Ok(json!({"agent": asked.agent, "run": run.map(|run| json!({
                "procedure": run.procedure, "project": run.project,
                "workspace": run.workspace, "ended": run.ended_at.is_some(),
            }))}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

fn find<'a>(project: &'a Project, name: &str) -> Result<&'a Procedure, Failure> {
    project.procedures.get(name).ok_or_else(|| {
        Failure::new(
            "not-found",
            format!("project {} has no procedure named {name}", project.name),
        )
    })
}

fn runnable(daemon: &Daemon, project: &str, name: &str) -> Result<(Project, Procedure), Failure> {
    let project = projects::load_project(daemon, project)?;
    let procedure = find(&project, name)?.clone();
    Ok((project, procedure))
}

fn workspace_of(daemon: &Daemon, project: &Project, name: &str) -> Result<Workspace, Failure> {
    daemon
        .read(|store| store.workspace(&project.name, name))?
        .ok_or_else(|| {
            Failure::new(
                "not-found",
                format!("project {} has no workspace named {name}", project.name),
            )
        })
}

fn not_a_table(project: &str) -> Failure {
    Failure::new(
        "invalid-config",
        format!("projects/{project}.toml: procedures: must be a table"),
    )
}

/// Writes the procedure under `[procedures.<name>]`, in place of what was there.
fn write(
    document: &mut toml_edit::DocumentMut,
    project: &str,
    name: &str,
    procedure: &Procedure,
) -> Result<(), Failure> {
    let mut table = toml_edit::Table::new();
    table.insert("description", toml_edit::value(&procedure.description));
    table.insert("instructions", toml_edit::value(&procedure.instructions));
    let procedures = document.entry("procedures").or_insert_with(|| {
        let mut procedures = toml_edit::Table::new();
        // `[procedures.<name>]` alone, without an empty `[procedures]` above it.
        procedures.set_implicit(true);
        toml_edit::Item::Table(procedures)
    });
    procedures
        .as_table_mut()
        .ok_or_else(|| not_a_table(project))?
        .insert(name, toml_edit::Item::Table(table));
    Ok(())
}

/// A procedure as the protocol shows it; with its instructions when `whole`.
fn to_json(name: &str, procedure: &Procedure, whole: bool) -> Value {
    let mut shown = json!({
        "name": name,
        "description": procedure.description,
    });
    if whole {
        shown["instructions"] = procedure.instructions.clone().into();
    }
    shown
}

/// `procedure.start`: a root agent starts in the workspace with the procedure's instructions as
/// its first message, with no grant: its calls go through the gate's classes like any agent's.
fn start(daemon: &Arc<Daemon>, asked: Start) -> Answer {
    let (project, procedure) = runnable(daemon, &asked.project, &asked.name)?;
    let workspace = workspace_of(daemon, &project, &asked.workspace)?;
    let after = daemon.read(Store::last_seq)?;
    let brief = brief(&project, &workspace, &asked.name, &procedure);
    let agent = agents::create(
        daemon,
        &project,
        New {
            workspace: Some(asked.workspace.clone()),
            preparing: false,
            role: roles::role(roles::LEAD).expect("the lead role"),
            parent: None,
            title: None,
            brief: brief.clone(),
            provider: None,
            model: None,
            effort: None,
            gate_mode: None,
        },
    )?;
    let run = ProcedureRun {
        agent: agent.id.clone(),
        project: project.name.clone(),
        workspace: asked.workspace,
        procedure: asked.name,
        started_at: now_ms(),
        ended_at: None,
    };
    daemon.write(|store| Ok(((), Some(store.start_procedure_run(&run)?))))?;
    // As any root's brief: its first message, delivered again by a replacement.
    deliveries::deliver(
        daemon,
        &agent.id,
        "user",
        Some(format!("brief-{}-0", agent.id)),
        &brief,
        approval::BETWEEN_TURNS,
    );
    Ok(json!({"agent": agent.to_json(), "after": after, "procedure": run.procedure}))
}

/// The first message of a run's agent: what it runs, the workspace's repositories by dependency
/// level and what the project says of its pull requests, then the instructions.
fn brief(project: &Project, workspace: &Workspace, name: &str, procedure: &Procedure) -> String {
    let mut text = format!(
        "You run the procedure {name} of project {}, in workspace {} (branch {}): {}\n\n",
        project.name,
        workspace.name,
        workspace.branch,
        procedure.description.trim()
    );
    text.push_str(&repositories_text(project, workspace));
    text.push_str(&pull_requests_text(project));
    if let Some(ticket) = &workspace.ticket {
        text.push_str(&format!(
            "\nThe workspace is linked to the ticket {ticket} (ticket_read).\n"
        ));
    }
    text.push_str("\n---\n\n");
    text.push_str(procedure.instructions.trim());
    text
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

/// Ends the run whose root is that agent, if it has one going.
pub fn end(daemon: &Daemon, agent: &str, reason: &str) {
    if let Err(error) = daemon.write(|store| Ok(((), store.end_procedure_run(agent, reason)?))) {
        eprintln!("flottille daemon: agent {agent}: its procedure run cannot end: {error}");
    }
}
