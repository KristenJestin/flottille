//! What an agent does of what the window does (`docs/tools.md`, "The project"): the project's
//! procedures (`procedure`), its settings and references (`project`), always its own project.
//! Reads run at once; every write is a family (`AgentActions`) the gate holds for the user unless
//! the project's file lets it run on its own, and the request says what it changes in words. No
//! argument names another project, gives a grant or sets what runs on its own: only the user
//! does that.
//!
//! And the catalogue a line run in the project folder is added to when the user answers its
//! request Always (`docs/tools.md`, "The gate").

use std::sync::Arc;

use flottille_core::command::split_line;
use flottille_core::declared::{ProjectTracker, RepositoryForge};
use flottille_core::procedure::Mode;
use flottille_core::project::{AgentActions, Project, Reference, check_reference, slug};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome, arguments};
use crate::procedures;
use crate::projects::{self, Failure};

/// The tools of this module.
pub const TOOLS: [&str; 2] = ["procedure", "project"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcedureCall {
    action: String,
    name: Option<String>,
    workspace: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectCall {
    action: String,
    repositories: Option<Vec<RepositorySetting>>,
    /// Left out: unchanged; `null`: removed.
    #[serde(default, deserialize_with = "present")]
    tracker: Option<Option<ProjectTracker>>,
    name: Option<String>,
    description: Option<String>,
    path: Option<String>,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RepositorySetting {
    path: String,
    /// `null` removes it.
    forge: Option<RepositoryForge>,
}

/// A field given, `null` included.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// The family of writes a call is, `None` for a read or a call that does not fit (the tool
/// refuses it). For the gate, which holds a write unless the project lets its family run on its
/// own.
pub fn family(tool: &str, args: &Value) -> Option<&'static str> {
    match (tool, args["action"].as_str()?) {
        ("procedure", "start") => Some("procedures"),
        ("project", "set_settings") => Some("settings"),
        ("project", "add_reference" | "remove_reference") => Some("references"),
        _ => None,
    }
}

/// Why the gate holds a write of a family.
pub fn asked(family: &str) -> String {
    let what = match family {
        "procedures" => "start a procedure",
        "settings" => "change the project's settings",
        _ => "add or remove a reference",
    };
    format!("the project's agents {what} only once you allow it")
}

/// Whether the project lets a family run on its own.
pub fn on_its_own(actions: &AgentActions, family: &str) -> bool {
    actions.mode(family) == Some(Mode::Allow)
}

/// A call checked before the gate is asked: what a write changes, in words (nothing for a read),
/// or the tool's refusal. A request shows these words, and is done only while they still hold.
pub async fn prepared(
    daemon: &Arc<Daemon>,
    grant: &Grant,
    tool: &str,
    args: &Value,
) -> Result<Vec<String>, Outcome> {
    if !TOOLS.contains(&tool) {
        return Ok(Vec::new());
    }
    let project = own(daemon, grant)?;
    match tool {
        "procedure" => {
            let call: ProcedureCall = arguments(tool, args.clone())?;
            match call.action.as_str() {
                "list" => Ok(Vec::new()),
                "show" => {
                    named(&call.name, "show")?;
                    Ok(Vec::new())
                }
                "start" => {
                    let name = named(&call.name, "start")?;
                    let workspace = workspace_of(grant, &call.workspace)?;
                    let shown = procedures::call(
                        daemon.clone(),
                        "procedure.run",
                        json!({"project": project.name, "name": name, "workspace": workspace}),
                    )
                    .await
                    .map_err(refusal)?;
                    Ok(start_says(name, &workspace, &shown))
                }
                other => Err(unknown("procedure", other, "list, show and start")),
            }
        }
        "project" => {
            let call: ProjectCall = arguments(tool, args.clone())?;
            match call.action.as_str() {
                "settings" => Ok(Vec::new()),
                "set_settings" => settings_says(&project, &call),
                "add_reference" => {
                    let name = named(&call.name, "add_reference")?;
                    let (Some(description), Some(path)) = (&call.description, &call.path) else {
                        return Err(Outcome::Refused(
                            "refused: add_reference needs name, description and path: \
                             {\"action\": \"add_reference\", \"name\": \"style\", \"description\": \
                             \"The code style\", \"path\": \"docs/STYLE.md\"}"
                                .into(),
                        ));
                    };
                    let reference = Reference {
                        description: description.trim().to_owned(),
                        path: path.trim_end_matches('/').to_owned(),
                    };
                    check_reference(name, &reference).map_err(|error| {
                        Outcome::Refused(format!("refused: references.{name}{error}"))
                    })?;
                    if project.references.contains_key(name) {
                        return Err(Outcome::Refused(format!(
                            "refused: project {} already has a reference named {name}",
                            project.name
                        )));
                    }
                    Ok(vec![format!(
                        "Add the reference {name} ({}): {}",
                        reference.path, reference.description
                    )])
                }
                "remove_reference" => {
                    let name = named(&call.name, "remove_reference")?;
                    let reference = project.references.get(name).ok_or_else(|| {
                        Outcome::Refused(format!(
                            "refused: project {} has no reference named {name}: its references \
                             are {}",
                            project.name,
                            listed(project.references.keys())
                        ))
                    })?;
                    Ok(vec![format!(
                        "Remove the reference {name} ({})",
                        reference.path
                    )])
                }
                other => Err(unknown(
                    "project",
                    other,
                    "settings, set_settings, add_reference and remove_reference",
                )),
            }
        }
        _ => Ok(Vec::new()),
    }
}

/// Performs a call the gate let through: on the agent's own project, always.
pub async fn perform(daemon: &Arc<Daemon>, grant: &Grant, tool: &str, args: Value) -> Outcome {
    match performed(daemon, grant, tool, args).await {
        Ok(text) => Outcome::Done(text),
        Err(outcome) => outcome,
    }
}

async fn performed(
    daemon: &Arc<Daemon>,
    grant: &Grant,
    tool: &str,
    args: Value,
) -> Result<String, Outcome> {
    let project = own(daemon, grant)?;
    let name = project.name.clone();
    if tool == "procedure" {
        let call: ProcedureCall = arguments(tool, args)?;
        return match call.action.as_str() {
            "list" => Ok(procedures_text(&project)),
            "show" => {
                let wanted = named(&call.name, "show")?;
                let procedure = procedures::call(
                    daemon.clone(),
                    "procedure.show",
                    json!({"project": name, "name": wanted}),
                )
                .await
                .map_err(refusal)?;
                Ok(procedure_text(&procedure))
            }
            "start" => {
                let wanted = named(&call.name, "start")?;
                let workspace = workspace_of(grant, &call.workspace)?;
                // The grants are the procedure's own, as the file says now: those the request
                // showed the user, since it is done only while its words still hold.
                let procedure = project.procedures.get(wanted).ok_or_else(|| {
                    Outcome::Refused(format!(
                        "refused: project {name} has no procedure named {wanted}"
                    ))
                })?;
                let started = procedures::call(
                    daemon.clone(),
                    "procedure.start",
                    json!({"project": name, "name": wanted, "workspace": workspace,
                        "grants": procedure.grants, "children": procedure.children}),
                )
                .await
                .map_err(refusal)?;
                Ok(format!(
                    "Procedure {wanted} started in workspace {workspace}: its agent is {}, a \
                     session of its own that reports to the user, not one of your children.",
                    started["agent"]["id"].as_str().unwrap_or("?")
                ))
            }
            other => Err(unknown("procedure", other, "list, show and start")),
        };
    }
    let call: ProjectCall = arguments(tool, args)?;
    match call.action.as_str() {
        "settings" => {
            let settings = projects::settings(daemon.clone(), json!({"name": name}))
                .await
                .map_err(refusal)?;
            Ok(settings_text(&project, &settings))
        }
        "set_settings" => {
            settings_says(&project, &call)?;
            let tracker = match call.tracker {
                Some(tracker) => tracker,
                None => project.tracker.clone(),
            };
            let settings = projects::set_settings(
                daemon.clone(),
                json!({"name": name, "repositories": call.repositories.unwrap_or_default(),
                    "tracker": tracker}),
            )
            .await
            .map_err(refusal)?;
            let saved = projects::load_project(daemon, &name).map_err(refusal)?;
            Ok(format!("Saved.\n{}", settings_text(&saved, &settings)))
        }
        "add_reference" => {
            projects::call(
                daemon,
                "reference.add",
                json!({"project": name, "name": call.name, "description": call.description,
                    "path": call.path}),
            )
            .map_err(refusal)?;
            Ok(format!(
                "Reference {} added: reference_read reads it now, and the agents started from now \
                 on see it in their instructions.",
                call.name.unwrap_or_default()
            ))
        }
        "remove_reference" => {
            projects::call(
                daemon,
                "reference.remove",
                json!({"project": name, "name": call.name}),
            )
            .map_err(refusal)?;
            Ok(format!(
                "Reference {} removed.",
                call.name.unwrap_or_default()
            ))
        }
        other => Err(unknown(
            "project",
            other,
            "settings, set_settings, add_reference and remove_reference",
        )),
    }
}

/// The agent's own project, as its file says now.
fn own(daemon: &Daemon, grant: &Grant) -> Result<Project, Outcome> {
    projects::load_project(daemon, &grant.place.project)
        .map_err(|failure| Outcome::Failed(failure.message))
}

fn named<'a>(name: &'a Option<String>, action: &str) -> Result<&'a str, Outcome> {
    name.as_deref().ok_or_else(|| {
        Outcome::Refused(format!(
            "refused: {action} needs name: {{\"action\": \"{action}\", \"name\": \"open-prs\"}}"
        ))
    })
}

/// The workspace a procedure starts in: the one named, or the agent's own.
fn workspace_of(grant: &Grant, named: &Option<String>) -> Result<String, Outcome> {
    named
        .clone()
        .or_else(|| grant.place.workspace.clone())
        .ok_or_else(|| {
            Outcome::Refused(
                "refused: a procedure runs in a workspace, and this agent works in the project \
                 folder: name one, {\"action\": \"start\", \"name\": \"open-prs\", \"workspace\": \
                 \"login\"}"
                    .into(),
            )
        })
}

fn unknown(tool: &str, action: &str, actions: &str) -> Outcome {
    Outcome::Refused(format!(
        "refused: {tool} has no action {action:?}: its actions are {actions}"
    ))
}

fn refusal(failure: Failure) -> Outcome {
    match failure.code {
        "unavailable" | "invalid-config" => Outcome::Failed(failure.message),
        _ => Outcome::Refused(format!("refused: {}", failure.message)),
    }
}

fn listed<'a>(names: impl Iterator<Item = &'a String>) -> String {
    let names: Vec<&str> = names.map(String::as_str).collect();
    match names.is_empty() {
        true => "none".into(),
        false => names.join(", "),
    }
}

/// What starting a procedure does, in words: the user confirms these, as in the window.
fn start_says(name: &str, workspace: &str, shown: &Value) -> Vec<String> {
    let mut says = vec![format!(
        "Start the procedure {name} in workspace {workspace}: {}",
        shown["description"].as_str().unwrap_or_default()
    )];
    let grants = shown["says"].as_array().map_or(&[][..], Vec::as_slice);
    for grant in grants {
        let how = match grant["mode"].as_str() {
            Some("allow") => "on their own",
            _ => "after asking you, each time",
        };
        says.push(format!(
            "Its agents may {}, {how}",
            grant["says"].as_str().unwrap_or_default()
        ));
    }
    says.push(
        match (grants.is_empty(), shown["children"] == true) {
            (true, _) => "It grants nothing beyond what agents may always do",
            (false, true) => "The agents the run starts may do the same",
            (false, false) => "The agents the run starts may do none of it",
        }
        .into(),
    );
    says
}

/// What a change of the settings does, in words, once each repository is the project's and each
/// declaration can be used.
fn settings_says(project: &Project, call: &ProjectCall) -> Result<Vec<String>, Outcome> {
    let mut says = Vec::new();
    for setting in call.repositories.iter().flatten() {
        if !project.repositories.iter().any(|r| r.path == setting.path) {
            return Err(Outcome::Refused(format!(
                "refused: project {} has no repository {}: it has {}",
                project.name,
                setting.path,
                listed(project.repositories.iter().map(|r| &r.path))
            )));
        }
        says.push(match &setting.forge {
            Some(forge) => {
                let mut checked = forge.clone();
                checked.check().map_err(|error| {
                    Outcome::Refused(format!("refused: the forge of {}{error}", setting.path))
                })?;
                format!("Declare {} on {}", setting.path, checked.said())
            }
            None => format!("Remove the forge of {}", setting.path),
        });
    }
    match &call.tracker {
        Some(Some(tracker)) => {
            let mut checked = tracker.clone();
            checked
                .check()
                .map_err(|error| Outcome::Refused(format!("refused: the tracker{error}")))?;
            says.push(format!("Set the tracker to {}", checked.said()));
        }
        Some(None) => says.push("Remove the tracker".into()),
        None => {}
    }
    if says.is_empty() {
        return Err(Outcome::Refused(
            "refused: set_settings changes nothing: give repositories, tracker or both, \
             {\"action\": \"set_settings\", \"repositories\": [{\"path\": \"api\", \"forge\": \
             {\"kind\": \"github\", \"repository\": \"acme/api\"}}]}"
                .into(),
        ));
    }
    Ok(says)
}

fn procedures_text(project: &Project) -> String {
    if project.procedures.is_empty() {
        return format!(
            "Project {} has no procedure: the user adds them in the window or with flottille \
             procedure add.",
            project.name
        );
    }
    let mut text = format!(
        "The procedures of project {} (procedure {{\"action\": \"show\", \"name\": …}} gives one \
         whole):\n",
        project.name
    );
    for (name, procedure) in &project.procedures {
        text.push_str(&format!(
            "- {name}: {} Grants: {}.\n",
            procedure.description,
            flottille_core::procedure::shown(&procedure.grants)
        ));
    }
    text
}

fn procedure_text(procedure: &Value) -> String {
    let text = |key: &str| procedure[key].as_str().unwrap_or_default().to_owned();
    let mut shown = format!("Procedure {}: {}\n", text("name"), text("description"));
    let grants = procedure["says"].as_array().map_or(&[][..], Vec::as_slice);
    if grants.is_empty() {
        shown.push_str("It grants nothing.\n");
    }
    for grant in grants {
        shown.push_str(&format!(
            "- {} ({}): {}\n",
            grant["grant"].as_str().unwrap_or_default(),
            grant["mode"].as_str().unwrap_or_default(),
            grant["says"].as_str().unwrap_or_default()
        ));
    }
    shown.push_str(match procedure["children"] == true {
        true => "Its run's children get its grants.\n",
        false => "Its run's children get none of its grants.\n",
    });
    shown.push_str(&format!("Instructions:\n{}\n", text("instructions")));
    shown
}

fn settings_text(project: &Project, settings: &Value) -> String {
    let mut text = format!("Project {}.\nRepositories:\n", project.name);
    for repository in settings["repositories"].as_array().into_iter().flatten() {
        let forge = |key: &str| {
            serde_json::from_value::<RepositoryForge>(repository[key].clone())
                .ok()
                .map(|forge| forge.said())
        };
        let mut line = format!(
            "- {} (remote {}): ",
            repository["path"].as_str().unwrap_or_default(),
            repository["remote"].as_str().unwrap_or_default()
        );
        match (forge("forge"), forge("proposal")) {
            (Some(forge), _) => line.push_str(&format!("on {forge}")),
            (None, Some(proposal)) => line.push_str(&format!(
                "no forge declared; its remote proposes {proposal}"
            )),
            (None, None) => line.push_str("no forge declared"),
        }
        if let Some(why) = repository["disagrees"].as_str() {
            line.push_str(&format!("; it disagrees with its remote: {why}"));
        }
        text.push_str(&line);
        text.push('\n');
    }
    text.push_str(&format!(
        "Tracker: {}\n",
        project
            .tracker
            .as_ref()
            .map_or("none".into(), ProjectTracker::said)
    ));
    text.push_str("References:\n");
    if project.references.is_empty() {
        text.push_str("- none\n");
    }
    for (name, reference) in &project.references {
        text.push_str(&format!(
            "- {name} ({}): {}\n",
            reference.path, reference.description
        ));
    }
    let modes: Vec<String> = AgentActions::FAMILIES
        .iter()
        .map(|family| {
            let mode = project.agent_actions.mode(family).unwrap_or(Mode::Ask);
            format!("{family} {}", mode.word())
        })
        .collect();
    text.push_str(&format!(
        "What agents do on their own: {} (ask: each write waits for the user; only the user \
         changes this).\n",
        modes.join(", ")
    ));
    text
}

/// The name a line takes in the catalogue: its words as a slug, made unique.
pub fn catalogue_name(project: &Project, words: &[String]) -> String {
    let program = |word: &String| word.rsplit('/').next().unwrap_or(word).to_owned();
    let mut base = slug(
        &words
            .iter()
            .enumerate()
            .map(|(i, word)| if i == 0 { program(word) } else { word.clone() })
            .collect::<Vec<_>>()
            .join(" "),
    );
    if base.len() > 48 {
        base.truncate(48);
        base = base.trim_end_matches('-').to_owned();
    }
    if base.is_empty() {
        base = "command".into();
    }
    let mut name = base.clone();
    let mut n = 2;
    while project.commands.contains_key(&name) {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

/// What a line outside the catalogue, run in the project folder, does, and what Always adds.
pub fn catalogue_says(daemon: &Daemon, grant: &Grant, args: &Value) -> Vec<String> {
    let Some(line) = args["line"].as_str() else {
        return Vec::new();
    };
    let repository = args["repository"].as_str().unwrap_or(".");
    let mut says = vec![match repository {
        "." => format!("Run {line} in the project folder"),
        repository => format!("Run {line} in {repository}, in the project folder"),
    }];
    if let (Ok(project), Ok(words)) = (own(daemon, grant), split_line(line)) {
        says.push(format!(
            "Always adds it to the project's catalogue as {}, a read-only check its agents run \
             there without asking",
            catalogue_name(&project, &words)
        ));
    }
    says
}

/// Adds the line of a `commands_run` call to the project's catalogue as a read-only check, so
/// that it runs in the project folder without asking from now on: its name. A line the
/// catalogue already holds there is not added again.
pub fn add_to_catalogue(daemon: &Daemon, grant: &Grant, args: &Value) -> Result<String, String> {
    let line = args["line"]
        .as_str()
        .ok_or("the request runs no line")?
        .to_owned();
    let words = split_line(&line)?;
    let repository = args["repository"]
        .as_str()
        .filter(|path| *path != ".")
        .map(str::to_owned);
    let mut added = String::new();
    projects::rewrite(daemon, &grant.place.project, |project, document| {
        if let Some((name, _)) = project.commands.iter().find(|(_, command)| {
            command.check
                && command.read_only
                && command.repository.as_deref().filter(|path| *path != ".")
                    == repository.as_deref()
                && split_line(&command.line).ok().as_ref() == Some(&words)
        }) {
            added = name.clone();
            return Ok(());
        }
        added = catalogue_name(project, &words);
        let mut entry = toml_edit::Table::new();
        entry.insert("line", toml_edit::value(line.as_str()));
        if let Some(repository) = &repository {
            entry.insert("repository", toml_edit::value(repository.as_str()));
        }
        entry.insert("check", toml_edit::value(true));
        entry.insert("read_only", toml_edit::value(true));
        let commands = document.entry("commands").or_insert_with(|| {
            let mut commands = toml_edit::Table::new();
            // `[commands.<name>]` alone, without an empty `[commands]` above it.
            commands.set_implicit(true);
            toml_edit::Item::Table(commands)
        });
        commands
            .as_table_mut()
            .ok_or_else(|| {
                Failure::new(
                    "invalid-config",
                    format!("projects/{}.toml: commands: must be a table", project.name),
                )
            })?
            .insert(&added, toml_edit::Item::Table(entry));
        Ok(())
    })
    .map_err(|failure| failure.message)?;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_writes_are_a_family_the_gate_holds() {
        let call = |tool: &str, action: &str| family(tool, &json!({"action": action}));
        assert_eq!(call("procedure", "start"), Some("procedures"));
        assert_eq!(call("project", "set_settings"), Some("settings"));
        assert_eq!(call("project", "add_reference"), Some("references"));
        assert_eq!(call("project", "remove_reference"), Some("references"));
        for (tool, action) in [
            ("procedure", "list"),
            ("procedure", "show"),
            ("project", "settings"),
        ] {
            assert_eq!(call(tool, action), None, "{tool} {action}");
        }
        assert_eq!(family("procedure", &json!({})), None);
    }

    #[test]
    fn a_line_takes_the_slug_of_its_words_as_its_name_once() {
        let mut project = Project::new(
            "app",
            "/work/app".into(),
            vec![".".into()],
            std::path::Path::new("/data"),
        );
        let words: Vec<String> = ["/usr/bin/cargo", "test", "-p", "core"]
            .map(String::from)
            .to_vec();
        assert_eq!(catalogue_name(&project, &words), "cargo-test-p-core");
        project.commands.insert(
            "cargo-test-p-core".into(),
            serde_json::from_value(json!({"line": "x"})).unwrap(),
        );
        assert_eq!(catalogue_name(&project, &words), "cargo-test-p-core-2");
    }
}
