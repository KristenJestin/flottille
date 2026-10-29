//! What an agent does of what the window does (`docs/tools.md`, "The project"): the project's
//! procedures (`procedure`), its settings and references (`project`), always its own project.
//! Reads run at once; every write is a call to judge (a tracker's change is base, `gate.rs`), and
//! its request says what it changes in words. No argument names another project: only the user
//! does that.

use std::sync::Arc;

use flottille_core::declared::{ProjectTracker, RepositoryForge};
use flottille_core::project::{Project, Reference, check_reference};
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
    /// set_rules: the rules.
    text: Option<String>,
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
/// refuses it). For the gate, which judges every write.
pub fn family(tool: &str, args: &Value) -> Option<&'static str> {
    match (tool, args["action"].as_str()?) {
        ("procedure", "start") => Some("procedures"),
        ("project", "set_settings") => Some("settings"),
        ("project", "add_reference" | "remove_reference") => Some("references"),
        ("project", "set_rules") => Some("rules"),
        _ => None,
    }
}

/// Why the gate holds a write of a family: what it does, as the window would.
pub fn asked(family: &str) -> String {
    match family {
        "procedures" => "it starts a procedure, a new session of its own",
        "settings" => "it changes the project's settings",
        "rules" => "it changes how the project's agents are told to work",
        _ => "it adds or removes a reference of the project",
    }
    .into()
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
                    let instructions = project
                        .procedures
                        .get(name)
                        .map_or("", |procedure| procedure.instructions.as_str());
                    Ok(start_says(name, &workspace, instructions, &shown))
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
                    if whole_checkout(&project, &reference.path) {
                        return Err(Outcome::Refused(format!(
                            "refused: {} is the project folder or one of its repositories, which \
                             an agent never makes a reference: name the folder or file inside it \
                             that is worth reading, like docs/STYLE.md",
                            reference.path
                        )));
                    }
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
                "rules" => Ok(Vec::new()),
                "set_rules" => rules_say(&project, &call),
                other => Err(unknown("project", other, PROJECT_ACTIONS)),
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
                if !project.procedures.contains_key(wanted) {
                    return Err(Outcome::Refused(format!(
                        "refused: project {name} has no procedure named {wanted}"
                    )));
                }
                let started = procedures::call(
                    daemon.clone(),
                    "procedure.start",
                    json!({"project": name, "name": wanted, "workspace": workspace}),
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
        "rules" => Ok(rules_text(&project, &daemon.paths)),
        "set_rules" => {
            rules_say(&project, &call)?;
            let text = call.text.as_deref().unwrap_or_default();
            crate::rules::set_project(daemon, &name, Some(text)).map_err(refusal)?;
            Ok(
                "Saved. The new rules reach the agents created from now on; you, and the agents \
                already created, keep the rules you were created with."
                    .to_owned(),
            )
        }
        other => Err(unknown("project", other, PROJECT_ACTIONS)),
    }
}

const PROJECT_ACTIONS: &str =
    "settings, set_settings, add_reference, remove_reference, rules and set_rules";

/// What `set_rules` changes, in words, or the refusal of a call that does not fit.
fn rules_say(project: &Project, call: &ProjectCall) -> Result<Vec<String>, Outcome> {
    let Some(text) = &call.text else {
        return Err(Outcome::Refused(
            "refused: set_rules needs text, the rules whole: {\"action\": \"set_rules\", \
             \"text\": \"Answer in French.\"}; rules reads the current ones"
                .into(),
        ));
    };
    crate::rules::check(text).map_err(|why| Outcome::Refused(format!("refused: {why}")))?;
    let replaced = match project.rules {
        Some(_) => "its own rules",
        None => "the rules it follows",
    };
    Ok(vec![
        format!("Replace {replaced} for project {} by:", project.name),
        text.trim_end().to_owned(),
    ])
}

/// What `rules` answers: the rules the agents created from now on get, and where they come from.
fn rules_text(project: &Project, paths: &flottille_core::paths::Paths) -> String {
    let (rules, cut) = crate::rules::resolved(paths, project);
    let from = match rules.source {
        crate::rules::Source::Project => "this project's own rules",
        crate::rules::Source::Global => "the user's rules for all projects",
        crate::rules::Source::Default => "Flottille's default rules",
    };
    let cut = cut.map_or_else(String::new, |cut| format!(" Cut: {cut}."));
    format!(
        "The rules agents of project {} get from now on are {from}.{cut} Yours are in your \
         instructions; set_rules proposes a change, which only the user can allow.\n\n{}",
        project.name,
        rules.text.trim_end()
    )
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

/// Whether a reference's path is the project folder or a repository's root, on the text or, past
/// a link, on the disk: everything a checkout holds, which no agent makes a reference of.
fn whole_checkout(project: &Project, path: &str) -> bool {
    let real = |path: &str| project.folder.join(path).canonicalize().ok();
    let target = real(path);
    std::iter::once(".")
        .chain(project.repositories.iter().map(|r| r.path.as_str()))
        .any(|root| root == path || (target.is_some() && real(root) == target))
}

/// What starting a procedure does, in words: the user confirms these, as in the window.
fn start_says(name: &str, workspace: &str, instructions: &str, shown: &Value) -> Vec<String> {
    let mut says = vec![format!(
        "Start the procedure {name} in workspace {workspace}: {}",
        shown["description"].as_str().unwrap_or_default()
    )];
    // What its agent is told is part of what the user confirms: edited meanwhile, nothing starts.
    says.push(format!("Its agent is told: {}", instructions.trim()));
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
        text.push_str(&format!("- {name}: {}\n", procedure.description));
    }
    text
}

fn procedure_text(procedure: &Value) -> String {
    let text = |key: &str| procedure[key].as_str().unwrap_or_default().to_owned();
    let mut shown = format!("Procedure {}: {}\n", text("name"), text("description"));
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
    text
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
}
