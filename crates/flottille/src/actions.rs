//! What an agent does of what the window does (`docs/tools.md`, "The project"): the project's
//! skills (`skills`), its commands (`commands`), its settings (`project`), always its own project.
//! Reads run at once; every write is a call to judge (a tracker's change is base, `gate.rs`), and
//! its request says what it changes in words. No argument names another project: only the user
//! does that.

use std::sync::Arc;

use flottille_core::commands::{self, Command};
use flottille_core::declared::{ProjectTracker, RepositoryForge};
use flottille_core::project::{Project, Skill, check_skill};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome, arguments};
use crate::projects::{self, Failure};

/// The tools of this module.
pub const TOOLS: [&str; 3] = ["skills", "commands", "project"];

/// The actions of a family of the project's entries, `skills` and `commands`.
const ACTIONS: &str = "list, show, add, edit and remove";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillsCall {
    action: String,
    name: Option<String>,
    description: Option<String>,
    path: Option<String>,
    text: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandsCall {
    action: String,
    name: Option<String>,
    description: Option<String>,
    instructions: Option<String>,
    template: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectCall {
    action: String,
    repositories: Option<Vec<RepositorySetting>>,
    /// Left out: unchanged; `null`: removed.
    #[serde(default, deserialize_with = "present")]
    tracker: Option<Option<ProjectTracker>>,
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
        ("skills", "add" | "edit" | "remove") => Some("skills"),
        ("commands", "add" | "edit" | "remove") => Some("commands"),
        ("project", "set_settings") => Some("settings"),
        _ => None,
    }
}

/// Why the gate holds a write of a family: what it does, as the window would.
pub fn asked(family: &str) -> String {
    match family {
        "skills" => "it adds, changes or removes a skill of the project",
        "commands" => "it adds, changes or removes a command of the project",
        _ => "it changes the project's settings",
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
        "skills" => skills_says(daemon, &project, &arguments(tool, args.clone())?),
        "commands" => commands_says(&project, &arguments(tool, args.clone())?),
        _ => {
            let call: ProjectCall = arguments(tool, args.clone())?;
            match call.action.as_str() {
                "settings" => Ok(Vec::new()),
                "set_settings" => settings_says(&project, &call),
                other => Err(unknown("project", other, "settings and set_settings")),
            }
        }
    }
}

/// What a call of `skills` changes, in words, once it can be done.
fn skills_says(
    daemon: &Daemon,
    project: &Project,
    call: &SkillsCall,
) -> Result<Vec<String>, Outcome> {
    let action = call.action.as_str();
    if !["list", "show", "add", "edit", "remove"].contains(&action) {
        return Err(unknown("skills", action, ACTIONS));
    }
    if action == "list" {
        return Ok(Vec::new());
    }
    let name = named(&call.name, "skills", action, "style")?;
    if action != "add" {
        let skill = project.skills.get(name).ok_or_else(|| {
            Outcome::Refused(format!(
                "refused: project {} has no skill named {name}: its skills are {}",
                project.name,
                listed(project.skills.keys())
            ))
        })?;
        return Ok(match action {
            "show" => Vec::new(),
            "remove" => vec![format!("Remove the skill {name} ({})", where_is(skill))],
            _ => {
                let known = text_of(daemon, project, name, skill);
                edit_says(project, name, call, skill, known)?
            }
        });
    }
    if project.skills.contains_key(name) {
        return Err(Outcome::Refused(format!(
            "refused: project {} already has a skill named {name}: edit it",
            project.name
        )));
    }
    let skill = skill_of(
        project,
        name,
        call,
        call.description.as_deref().unwrap_or(""),
    )?
    .ok_or_else(|| {
        Outcome::Refused(
            "refused: add needs name, description and path or text: {\"action\": \"add\", \
                 \"name\": \"style\", \"description\": \"The code style\", \"text\": \"...\"}"
                .into(),
        )
    })?;
    let mut says = vec![format!(
        "Add the skill {name} ({}): {}",
        where_is(&skill),
        skill.description
    )];
    if let Some(text) = &call.text {
        says.push(format!("Its text: {}", text.trim()));
    }
    Ok(says)
}

/// The words of an edit of a skill: each thing it changes.
fn edit_says(
    project: &Project,
    name: &str,
    call: &SkillsCall,
    skill: &Skill,
    known: Option<String>,
) -> Result<Vec<String>, Outcome> {
    let description = call.description.as_deref().unwrap_or(&skill.description);
    let edited = skill_of(project, name, call, description)?.unwrap_or(Skill {
        description: description.trim().to_owned(),
        path: skill.path.clone(),
    });
    check_skill(name, &edited)
        .map_err(|error| Outcome::Refused(format!("refused: skills.{name}{error}")))?;
    let mut says = Vec::new();
    if edited.description != skill.description {
        says.push(format!(
            "Change the description of the skill {name} to: {}",
            edited.description
        ));
    }
    if edited.path != skill.path {
        says.push(format!("Make the skill {name} {}", where_is(&edited)));
    }
    if let Some(text) = &call.text
        && known.as_deref() != Some(text.as_str())
    {
        says.push(format!(
            "Write the text of the skill {name}: {}",
            text.trim()
        ));
    }
    if says.is_empty() {
        return Err(Outcome::Refused(format!(
            "refused: this edit changes nothing of the skill {name}: give description, path or \
             text"
        )));
    }
    // What the text is now is part of what the user allows: written meanwhile, nothing is done.
    if let Some(known) = known
        && call.text.is_some()
    {
        says.push(format!("Its text was: {}", known.trim()));
    }
    Ok(says)
}

/// The skill a call makes of its path or text, checked as the window checks it; `None` when it
/// gives neither.
fn skill_of(
    project: &Project,
    name: &str,
    call: &SkillsCall,
    description: &str,
) -> Result<Option<Skill>, Outcome> {
    let path = match (&call.path, &call.text) {
        (Some(_), Some(_)) => {
            return Err(Outcome::Refused(
                "refused: a skill is a path of the project or a text, not both: give path or text"
                    .into(),
            ));
        }
        (None, None) => return Ok(None),
        (Some(path), None) => Some(path.trim_end_matches('/').to_owned()),
        (None, Some(text)) if text.trim().is_empty() => {
            return Err(Outcome::Refused(format!(
                "refused: skills.{name}.text: must not be empty"
            )));
        }
        (None, Some(_)) => None,
    };
    let skill = Skill {
        description: description.trim().to_owned(),
        path,
    };
    check_skill(name, &skill)
        .map_err(|error| Outcome::Refused(format!("refused: skills.{name}{error}")))?;
    if let Some(path) = &skill.path
        && whole_checkout(project, path)
    {
        return Err(Outcome::Refused(format!(
            "refused: {path} is the project folder or one of its repositories, which an agent \
             never makes a skill: name the folder or file inside it that is worth reading, like \
             docs/STYLE.md, or give its text"
        )));
    }
    Ok(Some(skill))
}

/// What a call of `commands` changes, in words, once it can be done.
fn commands_says(project: &Project, call: &CommandsCall) -> Result<Vec<String>, Outcome> {
    let action = call.action.as_str();
    if !["list", "show", "add", "edit", "remove"].contains(&action) {
        return Err(unknown("commands", action, ACTIONS));
    }
    if action == "list" {
        return Ok(Vec::new());
    }
    if action == "add" {
        let (name, command) = command_of(call)?;
        if project.commands.contains_key(&name) {
            return Err(Outcome::Refused(format!(
                "refused: project {} already has a command named {name}: edit it",
                project.name
            )));
        }
        return Ok(vec![
            format!("Add the command /{name}: {}", command.description),
            format!("Its instructions: {}", command.instructions.trim()),
        ]);
    }
    let name = named(&call.name, "commands", action, "open-prs")?;
    let command = project.commands.get(name).ok_or_else(|| {
        Outcome::Refused(format!(
            "refused: project {} has no command named {name}: its commands are {}",
            project.name,
            listed(project.commands.keys())
        ))
    })?;
    match action {
        "show" => Ok(Vec::new()),
        "remove" => Ok(vec![format!(
            "Remove the command /{name}: {}",
            command.description
        )]),
        _ => {
            let mut edited = command.clone();
            if let Some(description) = &call.description {
                edited.description = description.trim().to_owned();
            }
            if let Some(instructions) = &call.instructions {
                edited.instructions = instructions.clone();
            }
            edited
                .check(name)
                .map_err(|error| Outcome::Refused(format!("refused: {error}")))?;
            let mut says = Vec::new();
            if edited.description != command.description {
                says.push(format!(
                    "Change the description of the command /{name} to: {}",
                    edited.description
                ));
            }
            if edited.instructions != command.instructions {
                says.push(format!(
                    "Change the instructions of the command /{name} to: {}",
                    edited.instructions.trim()
                ));
                // What they were is part of what the user allows: edited meanwhile, nothing is done.
                says.push(format!("They were: {}", command.instructions.trim()));
            }
            if says.is_empty() {
                return Err(Outcome::Refused(format!(
                    "refused: this edit changes nothing of the command {name}: give description \
                     or instructions"
                )));
            }
            Ok(says)
        }
    }
}

/// The command an `add` makes, with its name: its own words, or one of Flottille's templates.
fn command_of(call: &CommandsCall) -> Result<(String, Command), Outcome> {
    let command = match &call.template {
        Some(template) => {
            if call.description.is_some() || call.instructions.is_some() {
                return Err(Outcome::Refused(
                    "refused: a template is added as it is: edit the command afterwards".into(),
                ));
            }
            commands::template(template).ok_or_else(|| {
                Outcome::Refused(format!(
                    "refused: there is no template {template}: the templates are {}",
                    commands::TEMPLATES.join(", ")
                ))
            })?
        }
        None => {
            let (Some(description), Some(instructions)) = (&call.description, &call.instructions)
            else {
                return Err(Outcome::Refused(
                    "refused: add needs name, description and instructions, or a template: \
                     {\"action\": \"add\", \"name\": \"release\", \"description\": \"Cut a \
                     release\", \"instructions\": \"...\"}; {\"action\": \"add\", \"template\": \
                     \"open-prs\"}"
                        .into(),
                ));
            };
            Command::new(description.trim(), instructions.clone())
        }
    };
    let name = match (&call.name, &call.template) {
        (Some(name), _) => name.clone(),
        (None, Some(template)) => template.clone(),
        (None, None) => named(&None, "commands", "add", "release")?.to_owned(),
    };
    command
        .check(&name)
        .map_err(|error| Outcome::Refused(format!("refused: {error}")))?;
    Ok((name, command))
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
    match tool {
        "skills" => {
            let call: SkillsCall = arguments(tool, args)?;
            let mut params = json!({"project": name, "name": call.name});
            for (key, value) in [
                ("description", &call.description),
                ("path", &call.path),
                ("text", &call.text),
            ] {
                if let Some(value) = value {
                    params[key] = value.clone().into();
                }
            }
            let answer = |method: &str, params: Value| {
                crate::skills::call(daemon, method, params).map_err(refusal)
            };
            match call.action.as_str() {
                "list" => Ok(skills_text(&project)),
                "show" => {
                    let shown = answer("skill.show", params)?;
                    Ok(skill_text(&shown))
                }
                "add" => {
                    answer("skill.add", params)?;
                    Ok(format!(
                        "Skill {} added: skill_read reads it now, and the agents started from now \
                         on see it in their instructions.",
                        call.name.unwrap_or_default()
                    ))
                }
                "edit" => {
                    answer("skill.edit", params)?;
                    Ok(format!(
                        "Skill {} changed: skill_read reads it as it is now.",
                        call.name.unwrap_or_default()
                    ))
                }
                "remove" => {
                    answer("skill.remove", params)?;
                    Ok(format!("Skill {} removed.", call.name.unwrap_or_default()))
                }
                other => Err(unknown("skills", other, ACTIONS)),
            }
        }
        "commands" => {
            let call: CommandsCall = arguments(tool, args)?;
            let answer = |method: &str, params: Value| {
                crate::commands::call(daemon, method, params).map_err(refusal)
            };
            match call.action.as_str() {
                "list" => Ok(commands_text(&project)),
                "show" => {
                    let shown =
                        answer("command.show", json!({"project": name, "name": call.name}))?;
                    Ok(command_text(&shown))
                }
                "add" => {
                    let (command, _) = command_of(&call)?;
                    let mut params = json!({"project": name, "name": command});
                    match &call.template {
                        Some(template) => params["template"] = template.clone().into(),
                        None => {
                            params["description"] = call.description.clone().into();
                            params["instructions"] = call.instructions.clone().into();
                        }
                    }
                    answer("command.add", params)?;
                    Ok(format!(
                        "Command {command} added: /{command} at the start of a message sends it."
                    ))
                }
                "edit" => {
                    let mut params = json!({"project": name, "name": call.name});
                    if let Some(description) = &call.description {
                        params["description"] = description.clone().into();
                    }
                    if let Some(instructions) = &call.instructions {
                        params["instructions"] = instructions.clone().into();
                    }
                    answer("command.edit", params)?;
                    Ok(format!(
                        "Command {} changed.",
                        call.name.unwrap_or_default()
                    ))
                }
                "remove" => {
                    answer(
                        "command.remove",
                        json!({"project": name, "name": call.name}),
                    )?;
                    Ok(format!(
                        "Command {} removed.",
                        call.name.unwrap_or_default()
                    ))
                }
                other => Err(unknown("commands", other, ACTIONS)),
            }
        }
        _ => {
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
                other => Err(unknown("project", other, "settings and set_settings")),
            }
        }
    }
}

/// The agent's own project, as its file says now.
fn own(daemon: &Daemon, grant: &Grant) -> Result<Project, Outcome> {
    projects::load_project(daemon, &grant.place.project)
        .map_err(|failure| Outcome::Failed(failure.message))
}

fn named<'a>(
    name: &'a Option<String>,
    tool: &str,
    action: &str,
    example: &str,
) -> Result<&'a str, Outcome> {
    name.as_deref().ok_or_else(|| {
        Outcome::Refused(format!(
            "refused: {tool} {action} needs name: {{\"action\": \"{action}\", \"name\": \
             \"{example}\"}}"
        ))
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

/// Whether a skill's path is the project folder or a repository's root, on the text or, past a
/// link, on the disk: everything a checkout holds, which no agent makes a skill of.
fn whole_checkout(project: &Project, path: &str) -> bool {
    let real = |path: &str| project.folder.join(path).canonicalize().ok();
    let target = real(path);
    std::iter::once(".")
        .chain(project.repositories.iter().map(|r| r.path.as_str()))
        .any(|root| root == path || (target.is_some() && real(root) == target))
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

/// Where a skill is: its path, or Flottille's data folder for a text.
fn where_is(skill: &Skill) -> String {
    match &skill.path {
        Some(path) => path.clone(),
        None => "a text Flottille keeps".into(),
    }
}

/// The text of a text skill as it is now; `None` for a path skill or a text not written.
fn text_of(daemon: &Daemon, project: &Project, name: &str, skill: &Skill) -> Option<String> {
    skill.path.is_none().then(|| {
        std::fs::read_to_string(Skill::text_file(&daemon.paths.data, &project.name, name))
            .unwrap_or_default()
    })
}

fn skills_text(project: &Project) -> String {
    if project.skills.is_empty() {
        return format!(
            "Project {} has no skill: skills {{\"action\": \"add\"}} adds one, a path of the \
             project or a text.",
            project.name
        );
    }
    let mut text = format!(
        "The skills of project {} (skill_read reads one; skills {{\"action\": \"show\", \"name\": \
         …}} gives its definition):\n",
        project.name
    );
    for (name, skill) in &project.skills {
        text.push_str(&format!(
            "- {name} ({}): {}\n",
            where_is(skill),
            skill.description
        ));
    }
    text
}

fn skill_text(skill: &Value) -> String {
    let text = |key: &str| skill[key].as_str().unwrap_or_default().to_owned();
    let mut shown = format!("Skill {}: {}\n", text("name"), text("description"));
    match skill["path"].as_str() {
        Some(path) => shown.push_str(&format!(
            "Path: {path}, in the project folder: read it with skill_read.\n"
        )),
        None => shown.push_str(&format!("A text Flottille keeps:\n{}\n", text("text"))),
    }
    shown
}

fn commands_text(project: &Project) -> String {
    if project.commands.is_empty() {
        return format!(
            "Project {} has no command: commands {{\"action\": \"add\"}} adds one, or one of \
             Flottille's templates ({}).",
            project.name,
            commands::TEMPLATES.join(", ")
        );
    }
    let mut text = format!(
        "The commands of project {} (commands {{\"action\": \"show\", \"name\": …}} gives one \
         whole; /name at the start of agent_send's message sends one to a child):\n",
        project.name
    );
    for (name, command) in &project.commands {
        text.push_str(&format!("- /{name}: {}\n", command.description));
    }
    text
}

fn command_text(command: &Value) -> String {
    let text = |key: &str| command[key].as_str().unwrap_or_default().to_owned();
    let mut shown = format!("Command /{}: {}\n", text("name"), text("description"));
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
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_writes_are_a_family_the_gate_holds() {
        let call = |tool: &str, action: &str| family(tool, &json!({"action": action}));
        for action in ["add", "edit", "remove"] {
            assert_eq!(call("skills", action), Some("skills"));
            assert_eq!(call("commands", action), Some("commands"));
        }
        assert_eq!(call("project", "set_settings"), Some("settings"));
        for (tool, action) in [
            ("skills", "list"),
            ("skills", "show"),
            ("commands", "list"),
            ("commands", "show"),
            ("project", "settings"),
        ] {
            assert_eq!(call(tool, action), None, "{tool} {action}");
        }
        assert_eq!(family("commands", &json!({})), None);
    }
}
