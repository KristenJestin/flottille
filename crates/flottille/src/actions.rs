//! What an agent does of what the window does (`docs/tools.md`, "The project"): the project's
//! tasks (`tasks`), skills (`skills`), commands (`commands`), setup (`setup`), settings
//! (`project`) and workspaces (`workspace`), always its own project. Reads run at once; every
//! write is a call to judge (a tracker's change is base, a line a task or a step sets is judged as
//! its run, `gate.rs`, and making a workspace ready as each setup step it plays, `mcp.rs`), and
//! its request says what it changes in words. No argument names another project: only the user
//! does that.

use std::collections::BTreeMap;
use std::sync::Arc;

use flottille_core::commands::{self, Command};
use flottille_core::declared::{ProjectTracker, RepositoryForge};
use flottille_core::project::{Project, SetupStep, Skill, StepKind, Task, check_skill, check_task};
use flottille_core::workspace::{Action, State, Step, StepState, Workspace};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome, arguments};
use crate::place::{Place, Resolved};
use crate::projects::{self, Failure};

/// The tools of this module.
pub const TOOLS: [&str; 6] = [
    "tasks",
    "skills",
    "commands",
    "setup",
    "project",
    "workspace",
];

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
struct TasksCall {
    action: String,
    name: Option<String>,
    line: Option<String>,
    /// Left out: unchanged; `null`: at the top of the place.
    #[serde(default, deserialize_with = "present")]
    repository: Option<Option<String>>,
    serve: Option<bool>,
    check: Option<bool>,
    read_only: Option<bool>,
    ask_before_running: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupCall {
    action: String,
    number: Option<usize>,
    to: Option<usize>,
    step: Option<SetupStep>,
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
    /// set_rules: the rules.
    text: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceCall {
    action: String,
    /// The workspace; for all but create, the agent's own when left out.
    name: Option<String>,
    repositories: Option<Vec<String>>,
    /// link_ticket: `null` unlinks.
    #[serde(default, deserialize_with = "present")]
    ticket: Option<Option<String>>,
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
        ("tasks", "add" | "edit" | "remove") => Some("tasks"),
        ("setup", "add" | "edit" | "remove" | "move") => Some("setup"),
        ("skills", "add" | "edit" | "remove") => Some("skills"),
        ("commands", "add" | "edit" | "remove") => Some("commands"),
        ("project", "set_settings") => Some("settings"),
        ("project", "set_rules") => Some("rules"),
        ("workspace", "create" | "prepare") => Some("workspace"),
        ("workspace", "link_ticket") => Some("ticket"),
        _ => None,
    }
}

/// Why the gate holds a write of a family: what it does, as the window would.
pub fn asked(family: &str) -> String {
    match family {
        "tasks" => "it adds, changes or removes a task of the project",
        "setup" => "it changes the setup that prepares the project's new workspaces",
        "skills" => "it adds, changes or removes a skill of the project",
        "commands" => "it adds, changes or removes a command of the project",
        "rules" => "it changes how the project's agents are told to work",
        "workspace" => {
            "it makes a workspace ready, playing setup steps that run without the gate, each \
             judged as its run"
        }
        "ticket" => "it links a workspace to a ticket, which its agents then read",
        _ => "it changes the project's settings",
    }
    .into()
}

/// The command line a write sets, which runs later without the gate: a task's, or a setup step's
/// `run`. For the gate, which judges setting it as running it.
pub fn line_of<'a>(tool: &str, args: &'a Value) -> Option<&'a str> {
    match (tool, args["action"].as_str()?) {
        ("tasks", "add" | "edit") => args["line"].as_str(),
        ("setup", "add" | "edit") => args["step"]["run"].as_str(),
        _ => None,
    }
}

/// What a write of a task or a setup step leaves to run later without the gate, as the run of
/// it will be: the task it runs (its name for a task or a task step, none for a `run` step),
/// with the line and the repository the run takes, and whether it runs in each repository.
pub struct Sets {
    pub name: Option<String>,
    pub task: Task,
    pub each: bool,
}

/// What a write sets to run, `None` for a call that sets nothing or does not fit (the tool
/// refuses it). For the gate, which judges setting it as running it.
pub fn sets(tasks: &BTreeMap<String, Task>, tool: &str, args: &Value) -> Option<Sets> {
    let action = args["action"].as_str()?;
    match (tool, action) {
        ("tasks", "add" | "edit") => {
            let call: TasksCall = serde_json::from_value(args.clone()).ok()?;
            let name = call.name.clone()?;
            let was = match action {
                "add" => Task {
                    line: call.line.clone()?,
                    repository: None,
                    serve: false,
                    check: false,
                    read_only: false,
                    ask_before_running: false,
                    added_by: None,
                },
                _ => tasks.get(&name)?.clone(),
            };
            Some(Sets {
                name: Some(name),
                task: edited(&was, &call),
                each: false,
            })
        }
        ("setup", "add" | "edit") => {
            let step: SetupStep = serde_json::from_value(args["step"].clone()).ok()?;
            let (name, mut task) = match (&step.run, &step.task) {
                (Some(line), _) => (
                    None,
                    Task {
                        line: line.trim().to_owned(),
                        repository: None,
                        serve: false,
                        check: false,
                        read_only: false,
                        ask_before_running: false,
                        added_by: None,
                    },
                ),
                (None, Some(name)) => (Some(name.clone()), tasks.get(name)?.clone()),
                (None, None) => return None,
            };
            // Where the setup plays it (`workspaces.rs`): the step's repository, else the
            // task's.
            if step.repository.is_some() {
                task.repository = step.repository.clone();
            }
            Some(Sets {
                name,
                task,
                each: step.for_each_repository,
            })
        }
        _ => None,
    }
}

/// The folders a run of what a write sets would start in, resolved in this place: a workspace
/// created later is laid out as the agent's own, or the project folder in `main`. None when it
/// names no repository of the project (the tool refuses it).
pub fn folders(place: &Place, sets: &Sets) -> Vec<Resolved> {
    if sets.each {
        return place
            .repositories
            .iter()
            .map(|(_, folder)| place.resolve(folder, "."))
            .collect();
    }
    place
        .base(sets.task.repository.as_deref())
        .map(|(_, base)| vec![place.resolve(base, ".")])
        .unwrap_or_default()
}

/// A call checked before the gate is asked: what a write changes, in words (nothing for a read),
/// or the tool's refusal. A request shows these words, and is done only while they still hold.
pub async fn prepared(
    daemon: &Arc<Daemon>,
    grant: &Grant,
    tool: &str,
    args: &Value,
) -> Result<Vec<String>, Outcome> {
    if tool == "agent_launch" {
        return launch_says(daemon, grant, args);
    }
    if tool == "session_send" {
        return session_says(daemon, grant, args);
    }
    if tool == "tickets" {
        let project = own(daemon, grant)?;
        return match crate::tickets::unwritable(&project, args) {
            Some(why) => Err(Outcome::Refused(why)),
            None => Ok(posted_says(args)),
        };
    }
    if tool == "forge" {
        return Ok(posted_says(args));
    }
    if !TOOLS.contains(&tool) {
        return Ok(Vec::new());
    }
    let project = own(daemon, grant)?;
    match tool {
        "tasks" => {
            let call: TasksCall = arguments(tool, args.clone())?;
            if family(tool, args).is_some() && !is_root(daemon, grant) {
                return Err(Outcome::Refused(
                    "refused: a child reads the project's tasks and runs them; the agent the user \
                     talks to changes them: ask your parent (ask_parent)"
                        .into(),
                ));
            }
            tasks_says(&project, &call)
        }
        "setup" => setup_says(&project, &arguments(tool, args.clone())?),
        "skills" => skills_says(daemon, &project, &arguments(tool, args.clone())?),
        "commands" => commands_says(&project, &arguments(tool, args.clone())?),
        "workspace" => {
            // In `main` a workspace removed is named: one a child does not work in.
            if args["action"] == "remove"
                && grant.place.workspace.is_none()
                && !is_root(daemon, grant)
            {
                return Err(Outcome::Refused(CHILD_MAIN_REMOVAL.into()));
            }
            let mine = matches!(args["action"].as_str(), Some("list" | "show" | "remove"));
            if !mine && !is_root(daemon, grant) {
                return Err(Outcome::Refused(CHILD_WORKSPACES.into()));
            }
            workspace_says(daemon, grant, &project, &arguments(tool, args.clone())?)
        }
        _ => {
            let call: ProjectCall = arguments(tool, args.clone())?;
            match call.action.as_str() {
                "settings" => Ok(Vec::new()),
                "set_settings" => settings_says(&project, &call),
                "rules" => Ok(Vec::new()),
                "set_rules" => rules_say(&project, &call),
                other => Err(unknown("project", other, PROJECT_ACTIONS)),
            }
        }
    }
}

/// Why a child in `main` cannot remove a workspace.
const CHILD_MAIN_REMOVAL: &str = "refused: a child removes only its own workspace, and this one \
    works in main, the project folder: ask your parent (ask_parent)";

/// What a write of `forge` or `tickets` posts, whole: the user reads on the card what the forge
/// or the tracker will show (the arguments recorded are cut). Nothing for a read.
fn posted_says(args: &Value) -> Vec<String> {
    if matches!(
        args["action"].as_str(),
        Some("list" | "search" | "show") | None
    ) {
        return Vec::new();
    }
    [("title", "Its title"), ("body", "Its body")]
        .into_iter()
        .filter_map(|(key, said)| Some(format!("{said}: {}", args[key].as_str()?)))
        .collect()
}

/// Whether the agent is a root, the agent the user talks to.
fn is_root(daemon: &Daemon, grant: &Grant) -> bool {
    daemon
        .read(|store| store.agent(&grant.agent))
        .ok()
        .flatten()
        .is_some_and(|agent| agent.parent.is_none())
}

/// A task in words: its line, where it runs and what it is.
fn task_said(task: &Task) -> String {
    let mut notes = Vec::new();
    if let Some(repository) = &task.repository {
        notes.push(format!("in {repository}"));
    }
    for (set, note) in [
        (task.serve, "keeps running"),
        (task.check, "a check"),
        (task.read_only, "changes nothing"),
        (task.ask_before_running, "judged each time it runs"),
    ] {
        if set {
            notes.push(note.to_owned());
        }
    }
    match notes.is_empty() {
        true => format!("`{}`", task.line),
        false => format!("`{}` ({})", task.line, notes.join(", ")),
    }
}

/// What a call of `tasks` changes, in words, once it can be done.
fn tasks_says(project: &Project, call: &TasksCall) -> Result<Vec<String>, Outcome> {
    let action = call.action.as_str();
    if !["list", "show", "add", "edit", "remove"].contains(&action) {
        return Err(unknown("tasks", action, ACTIONS));
    }
    if action == "list" {
        return Ok(Vec::new());
    }
    let name = named(&call.name, "tasks", action, "test")?;
    let known = |path: &str| project.repositories.iter().any(|r| r.path == path);
    let check = |task: &Task| {
        check_task(name, task, &known)
            .map_err(|error| Outcome::Refused(format!("refused: tasks.{name}{error}")))?;
        flottille_core::command::split_line(&task.line).map_err(Outcome::Refused)?;
        Ok::<(), Outcome>(())
    };
    if action == "add" {
        if project.tasks.contains_key(name) {
            return Err(Outcome::Refused(format!(
                "refused: project {} already has a task named {name}: edit it",
                project.name
            )));
        }
        let Some(line) = &call.line else {
            return Err(Outcome::Refused(
                "refused: add needs name and line: {\"action\": \"add\", \"name\": \"test\", \
                 \"line\": \"cargo test\"}"
                    .into(),
            ));
        };
        let task = edited(
            &Task {
                line: line.trim().to_owned(),
                repository: None,
                serve: false,
                check: false,
                read_only: false,
                ask_before_running: false,
                added_by: None,
            },
            call,
        );
        check(&task)?;
        return Ok(vec![format!("Add the task {name}: {}", task_said(&task))]);
    }
    let task = project.tasks.get(name).ok_or_else(|| {
        Outcome::Refused(format!(
            "refused: project {} has no task named {name}: its tasks are {}",
            project.name,
            listed(project.tasks.keys())
        ))
    })?;
    match action {
        "show" => Ok(Vec::new()),
        "remove" => {
            if let Some(at) = project
                .setup
                .iter()
                .position(|step| step.task.as_deref() == Some(name))
            {
                return Err(Outcome::Refused(format!(
                    "refused: step {} of the setup runs the task {name}: change or remove that \
                     step first (setup)",
                    at + 1
                )));
            }
            Ok(vec![format!("Remove the task {name}: {}", task_said(task))])
        }
        _ => {
            let changed = edited(task, call);
            check(&changed)?;
            if changed == *task {
                return Err(Outcome::Refused(format!(
                    "refused: this edit changes nothing of the task {name}: give line, \
                     repository or a flag"
                )));
            }
            // What it was is part of what the user allows: edited meanwhile, nothing is done.
            Ok(vec![
                format!("Change the task {name} to: {}", task_said(&changed)),
                format!("It was: {}", task_said(task)),
            ])
        }
    }
}

/// The task as a call of `tasks` leaves it.
fn edited(task: &Task, call: &TasksCall) -> Task {
    let mut task = task.clone();
    if let Some(line) = &call.line {
        task.line = line.trim().to_owned();
    }
    if let Some(repository) = &call.repository {
        task.repository = repository.clone().filter(|r| !r.is_empty());
    }
    for (field, value) in [
        (&mut task.serve, call.serve),
        (&mut task.check, call.check),
        (&mut task.read_only, call.read_only),
        (&mut task.ask_before_running, call.ask_before_running),
    ] {
        if let Some(value) = value {
            *field = value;
        }
    }
    task
}

const SETUP_ACTIONS: &str = "list, add, edit, remove and move";

/// A setup step in words: what it does and where.
pub fn step_said(step: &SetupStep) -> String {
    let what = match step.kind() {
        StepKind::Copy(globs) => format!("copy {}", globs.join(", ")),
        StepKind::Link(path) => format!("link {path}"),
        StepKind::RootEntries => "link the project folder's other entries".to_owned(),
        StepKind::Run(line) => format!("run `{line}`"),
        StepKind::Task(name) => format!("run the task {name}"),
    };
    let place = match (&step.repository, step.for_each_repository, step.kind()) {
        (_, _, StepKind::RootEntries) => String::new(),
        (Some(repository), _, _) => format!(" in {repository}"),
        (None, true, _) => " in each repository, leaves first".to_owned(),
        (None, false, StepKind::Task(_)) => " where the task says".to_owned(),
        (None, false, _) => " in the workspace folder".to_owned(),
    };
    let limit = step
        .time_limit
        .map_or_else(String::new, |limit| format!(", stopped after {limit} s"));
    format!("{what}{place}{limit}")
}

/// What a call of `setup` changes, in words, once it can be done.
fn setup_says(project: &Project, call: &SetupCall) -> Result<Vec<String>, Outcome> {
    let action = call.action.as_str();
    if !["list", "add", "edit", "remove", "move"].contains(&action) {
        return Err(unknown("setup", action, SETUP_ACTIONS));
    }
    if action == "list" {
        return Ok(Vec::new());
    }
    let len = project.setup.len();
    let step_at = |number: Option<usize>| -> Result<(usize, &SetupStep), Outcome> {
        let number = number.ok_or_else(|| {
            Outcome::Refused(format!(
                "refused: setup {action} needs number, the step's: {{\"action\": \"{action}\", \
                 \"number\": 1}}; list numbers them"
            ))
        })?;
        crate::setup::numbered(project, number)
            .map(|step| (number, step))
            .map_err(|failure| Outcome::Refused(format!("refused: {}", failure.message)))
    };
    let checked = |number: usize| -> Result<&SetupStep, Outcome> {
        let step = call.step.as_ref().ok_or_else(|| {
            Outcome::Refused(
                "refused: give step, the step whole: {\"copy\": [\".env\"], \
                 \"for_each_repository\": true}, {\"run\": \"npm install\", \"repository\": \
                 \"web\"} or {\"root_entries\": true}"
                    .into(),
            )
        })?;
        let known = |path: &str| project.repositories.iter().any(|r| r.path == path);
        flottille_core::project::check_step(
            &format!("setup[{}]", number - 1),
            step,
            &known,
            &project.tasks,
        )
        .map_err(|error| Outcome::Refused(format!("refused: {error}")))?;
        Ok(step)
    };
    match action {
        "add" => {
            let number = call.number.unwrap_or(len + 1);
            if !(1..=len + 1).contains(&number) {
                return Err(Outcome::Refused(format!(
                    "refused: number must be 1 to {}: the setup has {len} steps",
                    len + 1
                )));
            }
            let step = checked(number)?;
            Ok(vec![format!(
                "Add setup step {number}: {}",
                step_said(step)
            )])
        }
        "edit" => {
            let (number, was) = step_at(call.number)?;
            let step = checked(number)?;
            if step == was {
                return Err(Outcome::Refused(format!(
                    "refused: this edit changes nothing of setup step {number}"
                )));
            }
            Ok(vec![
                format!("Change setup step {number} to: {}", step_said(step)),
                format!("It was: {}", step_said(was)),
            ])
        }
        "remove" => {
            let (number, was) = step_at(call.number)?;
            Ok(vec![format!(
                "Remove setup step {number}: {}",
                step_said(was)
            )])
        }
        _ => {
            let (number, was) = step_at(call.number)?;
            let to = call.to.ok_or_else(|| {
                Outcome::Refused(
                    "refused: move needs to, the step's new number: {\"action\": \"move\", \
                     \"number\": 3, \"to\": 1}"
                        .into(),
                )
            })?;
            step_at(Some(to))?;
            if to == number {
                return Err(Outcome::Refused(format!(
                    "refused: setup step {number} is already there"
                )));
            }
            Ok(vec![format!(
                "Move setup step {number} ({}) to {to}",
                step_said(was)
            )])
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
        "tasks" => {
            let call: TasksCall = arguments(tool, args)?;
            let answer = |method: &str, params: Value| {
                crate::tasks::call(daemon, method, params).map_err(refusal)
            };
            let task = call.name.clone().unwrap_or_default();
            match call.action.as_str() {
                "list" => Ok(tasks_text(&project)),
                "show" => {
                    let shown = project.tasks.get(&task).ok_or_else(|| {
                        Outcome::Refused(format!(
                            "refused: project {name} has no task named {task}: its tasks are {}",
                            listed(project.tasks.keys())
                        ))
                    })?;
                    Ok(format!(
                        "Task {task}: {}\nRun it with run {{\"task\": \"{task}\"}}.",
                        task_said(shown)
                    ))
                }
                action @ ("add" | "edit") => {
                    let mut params = json!({"project": name, "name": task});
                    if let Some(line) = &call.line {
                        params["line"] = line.clone().into();
                    }
                    if let Some(repository) = &call.repository {
                        params["repository"] = repository.clone().into();
                    }
                    for (key, value) in [
                        ("serve", call.serve),
                        ("check", call.check),
                        ("read_only", call.read_only),
                        ("ask_before_running", call.ask_before_running),
                    ] {
                        if let Some(value) = value {
                            params[key] = value.into();
                        }
                    }
                    answer(&format!("task.{action}"), params)?;
                    Ok(match action {
                        "add" => format!(
                            "Task {task} added: run {{\"task\": \"{task}\"}} runs it, and the \
                             user's window has its Run button."
                        ),
                        _ => format!("Task {task} changed."),
                    })
                }
                "remove" => {
                    answer("task.remove", json!({"project": name, "name": task}))?;
                    Ok(format!("Task {task} removed."))
                }
                other => Err(unknown("tasks", other, ACTIONS)),
            }
        }
        "setup" => {
            let call: SetupCall = arguments(tool, args)?;
            let answer = |method: &str, params: Value| {
                crate::setup::call(daemon, method, params).map_err(refusal)
            };
            let number = call.number;
            match call.action.as_str() {
                "list" => return Ok(setup_text(&project)),
                "add" => answer(
                    "setup.add",
                    json!({"project": name, "step": call.step, "at": number}),
                )?,
                "edit" => answer(
                    "setup.edit",
                    json!({"project": name, "number": number, "step": call.step}),
                )?,
                "remove" => answer("setup.remove", json!({"project": name, "number": number}))?,
                "move" => answer(
                    "setup.move",
                    json!({"project": name, "number": number, "to": call.to}),
                )?,
                other => return Err(unknown("setup", other, SETUP_ACTIONS)),
            };
            let saved = projects::load_project(daemon, &name).map_err(refusal)?;
            Ok(format!(
                "Saved: the workspaces created from now on are prepared by it; those that exist \
                 keep theirs.\n{}",
                setup_text(&saved)
            ))
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
        "workspace" => {
            let call: WorkspaceCall = arguments(tool, args)?;
            let answer = |method: &'static str, params: Value| async move {
                crate::workspaces::call(daemon.clone(), method, params)
                    .await
                    .map_err(refusal)
            };
            match call.action.as_str() {
                "list" => workspaces_text(daemon, &project),
                "show" => Ok(workspace_text(&named_workspace(
                    daemon,
                    grant,
                    call.name.as_deref(),
                )?)),
                "create" => {
                    let created = answer(
                        "workspace.create",
                        json!({"project": name, "name": call.name,
                            "repositories": call.repositories, "ticket": call.ticket.flatten()}),
                    )
                    .await?;
                    let made = created["name"].as_str().unwrap_or_default();
                    Ok(format!(
                        "Workspace {made} created in {}: its worktrees are made, and its setup \
                         goes on without you; workspace {{\"action\": \"show\", \"name\": \
                         \"{made}\"}} follows it, agent_launch {{\"workspace\": \"{made}\"}} \
                         starts a child there.{}",
                        created["folder"].as_str().unwrap_or_default(),
                        crate::workspaces::notes(&created)
                    ))
                }
                "prepare" => {
                    let workspace = named_workspace(daemon, grant, call.name.as_deref())?;
                    answer(
                        "workspace.prepare",
                        json!({"project": name, "name": workspace.name}),
                    )
                    .await?;
                    Ok(format!(
                        "The preparation of workspace {} goes on: workspace {{\"action\": \
                         \"show\", \"name\": \"{}\"}} follows it.",
                        workspace.name, workspace.name
                    ))
                }
                "link_ticket" => {
                    let workspace = named_workspace(daemon, grant, call.name.as_deref())?;
                    answer(
                        "workspace.link_ticket",
                        json!({"project": name, "name": workspace.name, "ticket": call.ticket}),
                    )
                    .await?;
                    Ok(match call.ticket.flatten() {
                        Some(ticket) => format!(
                            "Workspace {} is linked to the ticket {ticket}: tickets {{\"action\": \
                             \"show\"}} reads it.",
                            workspace.name
                        ),
                        None => format!("Workspace {} is linked to no ticket.", workspace.name),
                    })
                }
                "remove" => {
                    let removed = crate::workspaces::workspace_remove(
                        daemon,
                        &grant.agent,
                        &grant.place,
                        call.name.as_deref(),
                    )
                    .await;
                    match removed {
                        Outcome::Done(text) => Ok(text),
                        other => Err(other),
                    }
                }
                other => Err(unknown("workspace", other, WORKSPACE_ACTIONS)),
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
                "rules" => Ok(rules_text(&project, &daemon.paths)),
                "set_rules" => {
                    rules_say(&project, &call)?;
                    let text = call.text.as_deref().unwrap_or_default();
                    crate::rules::set_project(daemon, &name, Some(text)).map_err(refusal)?;
                    Ok(
                        "Saved. The new rules reach the agents created from now on; you, and the \
                         agents already created, keep the rules you were created with."
                            .to_owned(),
                    )
                }
                other => Err(unknown("project", other, PROJECT_ACTIONS)),
            }
        }
    }
}

// The workspaces.

const WORKSPACE_ACTIONS: &str = "list, show, create, prepare, link_ticket and remove";

/// What a child is told of the writes of the `workspace` family but `remove`, which are not its
/// own.
pub const CHILD_WORKSPACES: &str = "refused: the agent the user talks to manages the project's \
    workspaces; you work in the place you were given, and may only remove it once done: ask your \
    parent (ask_parent)";

/// The setup steps a call plays without the gate, as the gate judges them: creating a workspace
/// (`workspace create`, `agent_launch` with `new_workspace`) plays the project's setup, resuming
/// one (`workspace prepare`) the steps it does again. `None` for any other call, and for one that
/// does not fit (the tool refuses it).
pub fn plays(daemon: &Daemon, grant: &Grant, tool: &str, args: &Value) -> Option<Vec<SetupStep>> {
    let creates = match tool {
        "workspace" => match args["action"].as_str()? {
            "create" => true,
            "prepare" => false,
            _ => return None,
        },
        // A launch that also names an existing workspace makes none: the tool refuses it.
        "agent_launch" if args["new_workspace"].is_string() && args["workspace"].is_null() => true,
        _ => return None,
    };
    let project = own(daemon, grant).ok()?;
    if creates {
        return Some(project.setup);
    }
    let workspace = named_workspace(daemon, grant, args["name"].as_str()).ok()?;
    Some(again(&workspace).into_iter().filter_map(step_of).collect())
}

/// The steps a call plays, in words, numbered: what the judge reads of them.
pub fn plays_said(daemon: &Daemon, grant: &Grant, tool: &str, args: &Value) -> Vec<String> {
    plays(daemon, grant, tool, args)
        .map(|steps| numbered(&steps))
        .unwrap_or_default()
}

fn numbered(steps: &[SetupStep]) -> Vec<String> {
    steps
        .iter()
        .enumerate()
        .map(|(at, step)| format!("{}. {}", at + 1, step_said(step)))
        .collect()
}

/// The steps of a workspace that resuming it does again: those not done, and the done ones whose
/// result is gone. Its worktrees are left out: they are no step of the setup.
fn again(workspace: &Workspace) -> Vec<&Step> {
    let redone = crate::workspaces::done_but_missing(workspace);
    workspace
        .steps
        .iter()
        .filter(|step| step.action != Action::Worktree)
        .filter(|step| {
            matches!(
                step.state,
                StepState::Failed | StepState::Running | StepState::Pending
            ) || redone.contains(&step.position)
        })
        .collect()
}

/// A step of a workspace as the setup step it plays: a task's step as the line it froze.
fn step_of(step: &Step) -> Option<SetupStep> {
    let mut set = SetupStep {
        repository: step.repository.clone(),
        ..SetupStep::default()
    };
    match &step.action {
        Action::Worktree => return None,
        Action::Copy { globs } => set.copy = Some(globs.clone()),
        Action::Link { path } => set.link = Some(path.clone()),
        Action::RootEntries { .. } => {
            set.root_entries = Some(true);
            set.repository = None;
        }
        Action::Run { line, .. } => set.run = Some(line.clone()),
    }
    Some(set)
}

/// The workspace a call names, or the agent's own when it names none.
fn named_workspace(
    daemon: &Daemon,
    grant: &Grant,
    name: Option<&str>,
) -> Result<Workspace, Outcome> {
    let project = &grant.place.project;
    let Some(name) = name.or(grant.place.workspace.as_deref()) else {
        return Err(Outcome::Refused(format!(
            "refused: you work in main, which is no workspace: name one, {{\"name\": \"login\"}}; \
             {}",
            names_of(daemon, project)
        )));
    };
    match daemon.read(|store| store.workspace(project, name)) {
        Ok(Some(workspace)) => Ok(workspace),
        Ok(None) => Err(Outcome::Refused(format!(
            "refused: project {project} has no workspace named {name}: {}",
            names_of(daemon, project)
        ))),
        Err(error) => Err(Outcome::Failed(error.to_string())),
    }
}

fn names_of(daemon: &Daemon, project: &str) -> String {
    let names: Vec<String> = daemon
        .read(|store| store.workspaces(Some(project)))
        .map(|all| all.into_iter().map(|w| w.name).collect())
        .unwrap_or_default();
    match names.is_empty() {
        true => "it has none".into(),
        false => format!("its workspaces are {}", names.join(", ")),
    }
}

/// What a call of `workspace` changes, in words, once it can be done.
fn workspace_says(
    daemon: &Daemon,
    grant: &Grant,
    project: &Project,
    call: &WorkspaceCall,
) -> Result<Vec<String>, Outcome> {
    let action = call.action.as_str();
    if !["list", "show", "create", "prepare", "link_ticket", "remove"].contains(&action) {
        return Err(unknown("workspace", action, WORKSPACE_ACTIONS));
    }
    match action {
        "list" => Ok(Vec::new()),
        "create" => {
            let name = named(&call.name, "workspace", action, "login")?;
            let mut says = create_says(daemon, project, name, &call.repositories)?;
            if let Some(Some(ticket)) = &call.ticket {
                let key = crate::tickets::accepted(project, ticket)
                    .map_err(|why| Outcome::Refused(format!("refused: {why}")))?;
                says[0].push_str(&format!(", linked to the ticket {}", key.text()));
            }
            Ok(says)
        }
        _ => {
            let workspace = named_workspace(daemon, grant, call.name.as_deref())?;
            let name = &workspace.name;
            match action {
                "show" => Ok(Vec::new()),
                "prepare" => {
                    if daemon.preparing.lock().unwrap().contains(&workspace.id) {
                        return Err(Outcome::Refused(format!(
                            "refused: workspace {name} is being prepared: workspace show follows \
                             it"
                        )));
                    }
                    let again: Vec<SetupStep> =
                        again(&workspace).into_iter().filter_map(step_of).collect();
                    let worktrees = workspace.steps.iter().any(|step| {
                        step.action == Action::Worktree && step.state != StepState::Done
                    });
                    if again.is_empty() && !worktrees {
                        return Err(Outcome::Refused(format!(
                            "refused: workspace {name} has nothing to resume: its setup is done"
                        )));
                    }
                    let mut says = vec![format!(
                        "Resume the preparation of the workspace {name}, from where it stopped"
                    )];
                    if !again.is_empty() {
                        says.push("These steps of its setup run again, without the gate:".into());
                        says.extend(numbered(&again));
                    }
                    Ok(says)
                }
                // Judged as before #141, as a call: what it removes is said by the gate.
                // An agent in a workspace removes only its own.
                "remove" => crate::mcp::workspace_named(
                    daemon,
                    &grant.place,
                    call.name.as_deref(),
                    "workspace remove",
                )
                .map(|_| Vec::new()),
                _ => {
                    let ticket = match &call.ticket {
                        Some(Some(ticket)) => ticket,
                        Some(None) => {
                            let Some(was) = &workspace.ticket else {
                                return Err(Outcome::Refused(format!(
                                    "refused: workspace {name} is linked to no ticket"
                                )));
                            };
                            return Ok(vec![format!(
                                "Unlink the workspace {name} from the ticket {was}"
                            )]);
                        }
                        None => {
                            return Err(Outcome::Refused(
                                "refused: link_ticket needs ticket (null unlinks): \
                                 {\"action\": \"link_ticket\", \"name\": \"login\", \
                                 \"ticket\": \"#123\"}"
                                    .into(),
                            ));
                        }
                    };
                    let key = crate::tickets::accepted(project, ticket)
                        .map_err(|why| Outcome::Refused(format!("refused: {why}")))?;
                    let was = workspace
                        .ticket
                        .as_ref()
                        .map_or(String::new(), |was| format!(" (it was {was})"));
                    Ok(vec![format!(
                        "Link the workspace {name} to the ticket {}{was}",
                        key.text()
                    )])
                }
            }
        }
    }
}

/// What creating a workspace does, in words, or the refusal of one that cannot be made: the
/// workspace, then each step of the setup it plays.
fn create_says(
    daemon: &Daemon,
    project: &Project,
    name: &str,
    repositories: &Option<Vec<String>>,
) -> Result<Vec<String>, Outcome> {
    if !flottille_core::project::valid_name(name) {
        return Err(Outcome::Refused(format!(
            "refused: {name} is not a name for a workspace: use 1 to 64 letters, digits, dots, \
             dashes or underscores, starting with a letter or a digit"
        )));
    }
    if let Ok(Some(_)) = daemon.read(|store| store.workspace(&project.name, name)) {
        return Err(Outcome::Refused(format!(
            "refused: project {} already has a workspace named {name}: choose another name",
            project.name
        )));
    }
    let known = |path: &String| project.repositories.iter().any(|r| &r.path == path);
    if let Some(unknown) = repositories.iter().flatten().find(|path| !known(path)) {
        return Err(Outcome::Refused(format!(
            "refused: {unknown} is not a repository of project {}: its repositories are {}",
            project.name,
            listed(project.repositories.iter().map(|r| &r.path))
        )));
    }
    let taken = match repositories {
        Some(paths) => format!("with the repositories {}", paths.join(", ")),
        None => "with the project's default repositories".into(),
    };
    let mut says = vec![format!(
        "Create the workspace {name} on the branch {}/{name}, {taken}",
        project.branch_prefix
    )];
    match project.setup.is_empty() {
        true => says.push("The project has no setup: nothing else runs.".into()),
        false => {
            says.push("Its setup then runs, without the gate:".into());
            says.extend(numbered(&project.setup));
        }
    }
    Ok(says)
}

/// What a child is told of `agent_launch` with `root`.
pub const CHILD_ROOTS: &str = "refused: only the agent the user talks to starts a session for \
    the user; a child's work stays in its tree, under its parent: ask your parent (ask_parent)";

/// What `agent_launch` does, in words, when it is judged: with `new_workspace`, as `workspace
/// create` says it, since it creates the workspace the same way; with `root`, the session it
/// starts for the user and its first message whole. Nothing for any other launch. Only a root
/// starts a root: a child doing so would leave its tree, its parent and its parent's sight.
fn launch_says(daemon: &Daemon, grant: &Grant, args: &Value) -> Result<Vec<String>, Outcome> {
    let root = args["root"] == true;
    if root && !is_root(daemon, grant) {
        return Err(Outcome::Refused(CHILD_ROOTS.into()));
    }
    crate::tree::launch_refusal(grant.place.workspace.as_deref(), args)?;
    let title = args["title"].as_str().unwrap_or_default();
    let mut says = match (args["new_workspace"].as_str(), args["workspace"].is_null()) {
        (Some(name), true) => {
            let project = own(daemon, grant)?;
            let repositories: Option<Vec<String>> =
                serde_json::from_value(args["repositories"].clone()).unwrap_or_default();
            let mut says = create_says(daemon, &project, name, &repositories)?;
            let whose = match root {
                true => "a session of the user's",
                false => "a child",
            };
            says[0].push_str(&format!(", for {whose}: \"{title}\""));
            says
        }
        _ if root => {
            let place = match args["workspace"]
                .as_str()
                .or(grant.place.workspace.as_deref())
            {
                Some(name) => format!("the workspace {name}"),
                None => "main, the project folder".to_owned(),
            };
            vec![format!(
                "Start a session for the user in {place}: \"{title}\", theirs, not a child of \
                 this agent"
            )]
        }
        _ => return Ok(Vec::new()),
    };
    if root {
        says.push(format!(
            "Its first message: {}",
            args["brief"].as_str().unwrap_or_default()
        ));
    }
    Ok(says)
}

/// What `session_send` does, in words: the session written to and the text whole, or the
/// refusal of a session the caller may not write to, before anything is asked.
fn session_says(daemon: &Daemon, grant: &Grant, args: &Value) -> Result<Vec<String>, Outcome> {
    let asked: crate::tree::SessionSend = arguments("session_send", args.clone())?;
    let caller = daemon
        .read(|store| store.agent(&grant.agent))
        .map_err(|error| Outcome::Failed(error.to_string()))?
        .ok_or_else(|| Outcome::Failed(format!("there is no agent {}", grant.agent)))?;
    let (session, started) = crate::tree::session_of(daemon, &caller, &asked)?;
    let whose = match started {
        true => "which you started",
        false => "the agent that started this session",
    };
    Ok(vec![format!(
        "Send to the session {}, {whose}: {}",
        crate::tree::named(&session),
        asked.text
    )])
}

/// The project's workspaces, a line each.
fn workspaces_text(daemon: &Daemon, project: &Project) -> Result<String, Outcome> {
    let all = daemon
        .read(|store| store.workspaces(Some(&project.name)))
        .map_err(|error| Outcome::Failed(error.to_string()))?;
    if all.is_empty() {
        return Ok(format!(
            "Project {} has no workspace yet: workspace {{\"action\": \"create\", \"name\": \
             \"login\"}} makes one.",
            project.name
        ));
    }
    let mut text = format!("The workspaces of project {}:\n", project.name);
    for workspace in &all {
        text.push_str(&format!("- {}\n", workspace_line(workspace)));
    }
    Ok(text)
}

/// A workspace in a line: its state, branch, repositories and ticket.
fn workspace_line(workspace: &Workspace) -> String {
    let state = match workspace.state() {
        State::Preparing => "preparing",
        State::Failed => "its preparation failed",
        State::Ready => "ready",
    };
    let repositories: Vec<&str> = workspace
        .repositories
        .iter()
        .map(|r| r.path.as_str())
        .collect();
    let ticket = workspace
        .ticket
        .as_ref()
        .map_or(String::new(), |ticket| format!(", ticket {ticket}"));
    format!(
        "{}: {state}, branch {}, repositories {}{ticket}",
        workspace.name,
        workspace.branch,
        repositories.join(", ")
    )
}

/// A workspace whole: its line, its folder, and each step of its preparation with its state.
fn workspace_text(workspace: &Workspace) -> String {
    let mut text = format!(
        "Workspace {}\nFolder: {}\nIts preparation:\n",
        workspace_line(workspace),
        workspace.folder.display()
    );
    for (at, step) in workspace.steps.iter().enumerate() {
        let what = match step_of(step) {
            Some(set) => step_said(&set),
            None => format!(
                "the worktree of {}",
                step.repository.as_deref().unwrap_or(".")
            ),
        };
        text.push_str(&format!("{}. {what}: {}\n", at + 1, step.state.as_str()));
        if let Some(failure) = &step.failure {
            text.push_str(&format!(
                "   {}: {}\n",
                failure.doing,
                failure.output.trim_end()
            ));
        }
    }
    if workspace.state() == State::Failed {
        text.push_str(&format!(
            "workspace {{\"action\": \"prepare\", \"name\": \"{}\"}} resumes it once the cause \
             is fixed.\n",
            workspace.name
        ));
    }
    text
}

const PROJECT_ACTIONS: &str = "settings, set_settings, rules and set_rules";

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

fn tasks_text(project: &Project) -> String {
    if project.tasks.is_empty() {
        return format!(
            "Project {} has no task: run a line with run {{\"line\": …}}; tasks {{\"action\": \
             \"add\"}} adds one.",
            project.name
        );
    }
    let mut text = format!(
        "The tasks of project {} (run {{\"task\": …}} runs one):\n",
        project.name
    );
    for (name, task) in &project.tasks {
        text.push_str(&format!("- {name}: {}\n", task_said(task)));
    }
    text
}

fn setup_text(project: &Project) -> String {
    if project.setup.is_empty() {
        return format!(
            "Project {} has no setup: a new workspace is its worktrees alone. setup {{\"action\": \
             \"add\", \"step\": …}} adds a step.",
            project.name
        );
    }
    let mut text = format!(
        "The setup of project {}, done in order for each new workspace:\n",
        project.name
    );
    for (index, step) in project.setup.iter().enumerate() {
        text.push_str(&format!("{}. {}\n", index + 1, step_said(step)));
    }
    text
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
            assert_eq!(call("tasks", action), Some("tasks"));
            assert_eq!(call("skills", action), Some("skills"));
            assert_eq!(call("commands", action), Some("commands"));
        }
        for action in ["add", "edit", "remove", "move"] {
            assert_eq!(call("setup", action), Some("setup"));
        }
        assert_eq!(call("project", "set_settings"), Some("settings"));
        for (tool, action) in [
            ("tasks", "list"),
            ("tasks", "show"),
            ("setup", "list"),
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

    #[test]
    fn the_line_a_task_or_a_run_step_sets_is_what_the_gate_judges_as_a_run() {
        let line = |tool: &str, args: Value| line_of(tool, &args).map(str::to_owned);
        assert_eq!(
            line(
                "tasks",
                json!({"action": "add", "name": "t", "line": "make"})
            ),
            Some("make".into())
        );
        assert_eq!(
            line(
                "tasks",
                json!({"action": "edit", "name": "t", "check": true})
            ),
            None
        );
        assert_eq!(
            line(
                "setup",
                json!({"action": "edit", "number": 1, "step": {"run": "npm ci"}})
            ),
            Some("npm ci".into())
        );
        assert_eq!(
            line(
                "setup",
                json!({"action": "add", "step": {"copy": [".env"]}})
            ),
            None
        );
        assert_eq!(
            line("tasks", json!({"action": "show", "name": "t", "line": "x"})),
            None
        );
    }

    #[test]
    fn a_step_is_said_with_what_it_does_and_where() {
        let step = |value: Value| serde_json::from_value::<SetupStep>(value).unwrap();
        assert_eq!(
            step_said(&step(
                json!({"copy": [".env", ".env.*"], "for_each_repository": true})
            )),
            "copy .env, .env.* in each repository, leaves first"
        );
        assert_eq!(
            step_said(&step(
                json!({"run": "npm ci", "repository": "web", "time_limit": 60})
            )),
            "run `npm ci` in web, stopped after 60 s"
        );
        assert_eq!(
            step_said(&step(json!({"root_entries": true}))),
            "link the project folder's other entries"
        );
        assert_eq!(
            step_said(&step(json!({"task": "build"}))),
            "run the task build where the task says"
        );
        assert_eq!(
            step_said(&step(json!({"link": "node_modules"}))),
            "link node_modules in the workspace folder"
        );
    }
}
