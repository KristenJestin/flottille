//! The `project.*` methods: registering folders and keeping each project's configuration
//! (`docs/protocol.md`, `docs/projects.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flottille_core::declared::RepositoryForge;
use flottille_core::detect;
use flottille_core::paths::Paths;
use flottille_core::project::{Project, slug, valid_name};
use flottille_core::protocol::{ProjectFolder, ProjectName, ProjectRepository, ProjectSettings};
use flottille_core::registry::Registry;
use flottille_core::{Error, Result};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::git::{self, Class};

/// An error answer: a code of `docs/protocol.md` and a sentence.
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>) -> Failure {
        Failure {
            code,
            message: message.into(),
        }
    }

    pub fn refused(message: impl Into<String>) -> Failure {
        Failure::new("refused", message)
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Failure {
        match error {
            Error::Refused(sentence) => Failure::refused(sentence),
            Error::Config(error) => Failure::new("invalid-config", error.to_string()),
            error => Failure::new("unavailable", error.to_string()),
        }
    }
}

impl From<serde_json::Error> for Failure {
    fn from(error: serde_json::Error) -> Failure {
        Failure::new("invalid-params", error.to_string())
    }
}

/// The projects' files, and the projects the daemon last saw in them: the difference is what it
/// announces, since the files are the user's to edit.
pub struct Projects {
    registry: Registry,
    data: PathBuf,
    known: BTreeMap<String, Project>,
}

impl Projects {
    /// Takes the projects there are as the starting point: they are not news.
    pub fn open(paths: &Paths) -> Result<Projects> {
        let registry = Registry::new(paths);
        let known = registry
            .load_all()?
            .projects
            .into_iter()
            .map(|project| (project.name.clone(), project))
            .collect();
        Ok(Projects {
            registry,
            data: paths.data.clone(),
            known,
        })
    }
}

/// Answers a `project.*` request.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> std::result::Result<Value, Failure> {
    let mut projects = daemon.projects.lock().unwrap();
    let projects = &mut *projects;
    let loaded = announce_edits(daemon, projects)?;
    match method {
        "project.list" => Ok(json!({
            "projects": loaded.projects,
            "errors": loaded.invalid.iter().map(|error| json!({
                "name": error.name,
                "file": error.file(),
                "message": error.message,
            })).collect::<Vec<_>>(),
        })),
        "project.show" => {
            let ProjectName { name } = serde_json::from_value(params)?;
            Ok(serde_json::to_value(load(projects, &name)?)?)
        }
        "project.detect" => {
            let plan = plan(projects, serde_json::from_value(params)?)?;
            Ok(json!({
                "name": plan.name,
                "folder": plan.folder,
                "repositories": plan.repositories,
            }))
        }
        "project.add" => {
            let asked: ProjectFolder = serde_json::from_value(params)?;
            let confirmed = asked.repositories.clone();
            let plan = plan(projects, asked)?;
            if let Some(confirmed) = confirmed
                && confirmed != plan.repositories
            {
                return Err(Failure::refused(format!(
                    "the repositories of {} changed since they were shown: run it again",
                    plan.folder.display()
                )));
            }
            let project = Project::new(&plan.name, plan.folder, plan.repositories, &projects.data);
            projects.registry.create(&project)?;
            projects.known.insert(project.name.clone(), project.clone());
            emit(
                daemon,
                "project.added",
                json!({"name": project.name, "folder": project.folder}),
            )?;
            Ok(serde_json::to_value(project)?)
        }
        "project.remove" => {
            let ProjectName { name } = serde_json::from_value(params)?;
            if !projects.registry.remove(&name)? {
                return Err(not_found(&name));
            }
            projects.known.remove(&name);
            emit(daemon, "project.removed", json!({"name": name}))?;
            Ok(json!({"removed": name}))
        }
        "project.check" => {
            let ProjectName { name } = serde_json::from_value(params)?;
            let project = load(projects, &name)?;
            let detected = detect::repositories(&project.folder).map_err(|error| {
                Failure::refused(format!(
                    "the folder {} of project {name} no longer exists or cannot be read: {error}",
                    project.folder.display()
                ))
            })?;
            let configured: Vec<_> = project.repositories.iter().map(|r| &r.path).collect();
            Ok(json!({
                "name": name,
                "folder": project.folder,
                "new": detected.iter().filter(|path| !configured.contains(path)).collect::<Vec<_>>(),
                "missing": configured.iter().filter(|path| !detected.contains(path)).collect::<Vec<_>>(),
            }))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// `project.show`: the project, and the branch each repository's checkout in the project folder
/// is on (`null` when it is on none), read by git outside the projects' lock.
pub async fn show(daemon: Arc<Daemon>, params: Value) -> std::result::Result<Value, Failure> {
    let mut project = call(&daemon, "project.show", params)?;
    let folder = PathBuf::from(project["folder"].as_str().unwrap_or_default());
    for repository in project["repositories"].as_array_mut().into_iter().flatten() {
        let checkout = match repository["path"].as_str().unwrap_or(".") {
            "." => folder.clone(),
            path => folder.join(path),
        };
        // A folder that is not its own repository yet: git would answer for the one around it.
        if !checkout.join(".git").exists() {
            repository["branch"] = Value::Null;
            continue;
        }
        let head = git::run(
            &checkout,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            Class::Read,
        )
        .await;
        repository["branch"] = match head {
            Ok(head) if head.success => head.stdout.trim().into(),
            _ => Value::Null,
        };
    }
    Ok(project)
}

/// The folder of a repository of the project; none when it is not its own repository: git would
/// answer for the one around it.
fn checkout_of(project: &Project, path: &str) -> Option<PathBuf> {
    let folder = match path {
        "." => project.folder.clone(),
        path => project.folder.join(path),
    };
    folder.join(".git").exists().then_some(folder)
}

/// The address a repository's remote leads to, credentials taken out and masked; none when it
/// has no such remote. It is shown, never checked: the check reads the remote itself.
async fn remote_url(project: &Project, path: &str, remote: &str) -> Option<String> {
    let folder = checkout_of(project, path)?;
    git::output(&folder, &["remote", "get-url", remote], Class::Read)
        .await
        .ok()
        .map(|url| crate::forge::masked(&crate::forge::without_credentials(url.trim())))
}

/// Why `forge` is not where the repository's remote leads (`forge::disagreement`), masked; none
/// when it is.
async fn disagreement(
    project: &Project,
    path: &str,
    remote: &str,
    forge: &RepositoryForge,
) -> Option<String> {
    match checkout_of(project, path) {
        None => Some(format!("it has no remote {remote}")),
        Some(folder) => crate::forge::disagreement(forge, &folder, remote).await,
    }
}

/// What the project's settings show (`project.settings`): for each repository its remote's
/// address, the forge declared and the one its address proposes, and the tracker. The proposal is
/// only that: nothing is read nor written for a repository until its forge is declared.
async fn settings_of(project: &Project) -> Value {
    let mut repositories = Vec::new();
    for repository in &project.repositories {
        let url = remote_url(project, &repository.path, &repository.remote).await;
        let proposal = url.as_deref().and_then(RepositoryForge::proposed);
        let disagrees = match &repository.forge {
            Some(forge) => disagreement(project, &repository.path, &repository.remote, forge).await,
            None => None,
        };
        repositories.push(json!({
            "path": repository.path,
            "remote": repository.remote,
            "url": url,
            "forge": repository.forge,
            "proposal": proposal,
            "disagrees": disagrees,
        }));
    }
    json!({
        "name": project.name,
        "folder": project.folder,
        "repositories": repositories,
        "tracker": project.tracker,
        "obsolete": project.obsolete,
    })
}

/// `project.settings`.
pub async fn settings(daemon: Arc<Daemon>, params: Value) -> std::result::Result<Value, Failure> {
    let ProjectName { name } = serde_json::from_value(params)?;
    let project = load_project(&daemon, &name)?;
    Ok(settings_of(&project).await)
}

/// `project.set_settings`: writes the declared forges and tracker in the project's file, what
/// is around them kept, once each forge is checked to be the one its remote leads to.
pub async fn set_settings(
    daemon: Arc<Daemon>,
    params: Value,
) -> std::result::Result<Value, Failure> {
    let asked: ProjectSettings = serde_json::from_value(params)?;
    let project = load_project(&daemon, &asked.name)?;
    // Each forge as checked (its host in lowercase), which is what the file gets.
    let mut checked_forges = BTreeMap::new();
    for setting in &asked.repositories {
        let Some(repository) = project.repositories.iter().find(|r| r.path == setting.path) else {
            return Err(Failure::refused(format!(
                "project {} has no repository {}",
                project.name, setting.path
            )));
        };
        let Some(forge) = &setting.forge else {
            continue;
        };
        let mut checked = forge.clone();
        checked
            .check()
            .map_err(|error| Failure::refused(format!("the forge of {}{error}", setting.path)))?;
        if let Some(why) = disagreement(&project, &setting.path, &repository.remote, &checked).await
        {
            return Err(Failure::refused(format!(
                "{} cannot be declared on {}: {why}; nothing was saved",
                setting.path,
                checked.said(),
            )));
        }
        checked_forges.insert(setting.path.clone(), checked);
    }
    let saved = rewrite(&daemon, &asked.name, |_, document| {
        let not_an_array = || {
            Failure::new(
                "invalid-config",
                format!(
                    "projects/{}.toml: repositories: must be an array of tables",
                    asked.name
                ),
            )
        };
        let entries = document
            .get_mut("repositories")
            .and_then(toml_edit::Item::as_array_of_tables_mut)
            .ok_or_else(not_an_array)?;
        for setting in &asked.repositories {
            for entry in entries.iter_mut() {
                if entry.get("path").and_then(|p| p.as_str()) != Some(setting.path.as_str()) {
                    continue;
                }
                match checked_forges.get(&setting.path) {
                    None => {
                        entry.remove("forge");
                    }
                    Some(forge) => {
                        entry.insert("forge", toml_edit::value(inline(forge)?));
                    }
                }
            }
        }
        match &asked.tracker {
            None => {
                document.remove("tracker");
            }
            Some(tracker) => {
                let mut table = toml_edit::Table::new();
                for (key, value) in inline(tracker)?.iter() {
                    table.insert(key, toml_edit::value(value.clone()));
                }
                document.insert("tracker", toml_edit::Item::Table(table));
            }
        }
        Ok(())
    })?;
    Ok(settings_of(&saved).await)
}

/// `project.repositories`: each repository of the project as its checkout stands (found or not,
/// the branch it is on, where its remote leads, none when it is local only, its forge, its base
/// rule and its local branches), and `new`, the repositories the folder holds that the file does
/// not name, detected again.
pub async fn repositories(
    daemon: Arc<Daemon>,
    params: Value,
) -> std::result::Result<Value, Failure> {
    let ProjectName { name } = serde_json::from_value(params)?;
    let project = load_project(&daemon, &name)?;
    Ok(repositories_of(&project).await)
}

async fn repositories_of(project: &Project) -> Value {
    let mut listed = Vec::new();
    for repository in &project.repositories {
        let checkout = checkout_of(project, &repository.path);
        let (branch, branches) = match &checkout {
            None => (None, Vec::new()),
            Some(folder) => {
                let branch = git::output(
                    folder,
                    &["symbolic-ref", "--quiet", "--short", "HEAD"],
                    Class::Read,
                )
                .await
                .ok()
                .map(|branch| branch.trim().to_owned());
                let branches: Vec<String> = git::output(
                    folder,
                    &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
                    Class::Read,
                )
                .await
                .map(|listed| listed.lines().map(str::to_owned).collect())
                .unwrap_or_default();
                (branch, branches)
            }
        };
        listed.push(json!({
            "path": repository.path,
            "found": checkout.is_some(),
            "branch": branch,
            "remote": repository.remote,
            "url": remote_url(project, &repository.path, &repository.remote).await,
            "forge": repository.forge,
            "base": repository.base,
            "branches": branches,
        }));
    }
    // A folder that cannot be read offers nothing new; its repositories say they are not found.
    let new: Vec<String> = detect::repositories(&project.folder)
        .unwrap_or_default()
        .into_iter()
        .filter(|path| !project.repositories.iter().any(|r| &r.path == path))
        .collect();
    json!({
        "name": project.name,
        "folder": project.folder,
        "repositories": listed,
        "new": new,
    })
}

/// A path typed for a repository, as the file writes it: relative, `/`-separated, `.` for the
/// folder itself; none when it leads out of the folder.
fn repository_path(typed: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in Path::new(typed.trim()).components() {
        match part {
            std::path::Component::Normal(part) => parts.push(part.to_str()?.to_owned()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(match parts.is_empty() {
        true => ".".to_owned(),
        false => parts.join("/"),
    })
}

/// `project.add_repository`: a repository of the project's folder, written in the file where it
/// stands, its comments kept, with the defaults (`origin`, base `main`).
pub async fn add_repository(
    daemon: Arc<Daemon>,
    params: Value,
) -> std::result::Result<Value, Failure> {
    let asked: ProjectRepository = serde_json::from_value(params)?;
    let project = load_project(&daemon, &asked.name)?;
    let Some(path) = repository_path(&asked.path) else {
        return Err(Failure::refused(format!(
            "{} is not a folder of {}",
            asked.path, asked.name
        )));
    };
    if project.repositories.iter().any(|r| r.path == path) {
        return Err(Failure::refused(format!(
            "{path} is already a repository of {}",
            asked.name
        )));
    }
    if checkout_of(&project, &path).is_none() {
        return Err(Failure::refused(format!(
            "{path} is not a Git repository: {} holds no .git",
            project.folder.join(&path).display()
        )));
    }
    let saved = rewrite(&daemon, &asked.name, |_, document| {
        let entries = document
            .entry("repositories")
            .or_insert(toml_edit::Item::ArrayOfTables(Default::default()))
            .as_array_of_tables_mut()
            .ok_or_else(|| {
                Failure::new(
                    "invalid-config",
                    format!(
                        "projects/{}.toml: repositories: must be an array of tables",
                        asked.name
                    ),
                )
            })?;
        let mut table = toml_edit::Table::new();
        table.insert("path", toml_edit::value(path.clone()));
        table.decor_mut().set_prefix("\n");
        entries.push(table);
        Ok(())
    })?;
    Ok(repositories_of(&saved).await)
}

/// `project.remove_repository`: the project forgets one of its repositories, written in the file
/// where it stands, its comments kept. Its folder, its branches and the worktrees of the
/// workspaces that took it are never touched. Refused when the file would no longer be one the
/// daemon can use (a repository depending on it, a task running in it…), with the reason.
pub async fn remove_repository(
    daemon: Arc<Daemon>,
    params: Value,
) -> std::result::Result<Value, Failure> {
    let asked: ProjectRepository = serde_json::from_value(params)?;
    let project = load_project(&daemon, &asked.name)?;
    let path = repository_path(&asked.path).unwrap_or_else(|| asked.path.clone());
    if !project.repositories.iter().any(|r| r.path == path) {
        return Err(Failure::refused(format!(
            "{} has no repository {path}",
            asked.name
        )));
    }
    if project.repositories.len() == 1 {
        return Err(Failure::refused(format!(
            "{path} is the last repository of {}, and a project keeps at least one repository",
            asked.name
        )));
    }
    let saved = rewrite(&daemon, &asked.name, |_, document| {
        let entries = document
            .get_mut("repositories")
            .and_then(toml_edit::Item::as_array_of_tables_mut)
            .ok_or_else(|| {
                Failure::new(
                    "invalid-config",
                    format!(
                        "projects/{}.toml: repositories: must be an array of tables",
                        asked.name
                    ),
                )
            })?;
        let at = entries
            .iter()
            .position(|entry| entry.get("path").and_then(|p| p.as_str()) == Some(path.as_str()));
        if let Some(at) = at {
            entries.remove(at);
        }
        Ok(())
    })
    .map_err(|failure| match failure.code {
        "invalid-config" => Failure::refused(format!(
            "{path} is not removed, nothing was written: {}",
            failure.message
        )),
        _ => failure,
    })?;
    Ok(repositories_of(&saved).await)
}

/// A declaration as the TOML inline table it is written as: its keys hold strings and lists of
/// strings.
fn inline(value: &impl serde::Serialize) -> std::result::Result<toml_edit::InlineTable, Failure> {
    let mut table = toml_edit::InlineTable::new();
    let Value::Object(fields) = serde_json::to_value(value)? else {
        return Err(Failure::refused("a declaration is a table"));
    };
    for (key, field) in fields {
        let value: toml_edit::Value = match field {
            Value::String(text) => text.into(),
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .collect::<toml_edit::Array>()
                .into(),
            _ => continue,
        };
        table.insert(&key, value);
    }
    Ok(table)
}

fn emit(daemon: &Daemon, name: &str, data: Value) -> std::result::Result<(), Failure> {
    daemon
        .emit(name, data)
        .map(|_| ())
        .map_err(|error| Failure::new("unavailable", error.to_string()))
}

fn not_found(name: &str) -> Failure {
    Failure::new("not-found", format!("no project named {name}"))
}

fn load(projects: &Projects, name: &str) -> std::result::Result<Project, Failure> {
    projects.registry.load(name)?.ok_or_else(|| not_found(name))
}

/// Edits the project's file where it stands, keeping what the user wrote around the edit, and
/// writes it only when the result is a configuration the daemon can use. The edit is given the
/// project as the file says now.
pub fn rewrite(
    daemon: &Daemon,
    name: &str,
    edit: impl FnOnce(&Project, &mut toml_edit::DocumentMut) -> std::result::Result<(), Failure>,
) -> std::result::Result<Project, Failure> {
    let mut projects = daemon.projects.lock().unwrap();
    let projects = &mut *projects;
    let project = load(projects, name)?;
    let file = projects.registry.file(name);
    let unavailable = |error: std::io::Error| {
        Failure::new("unavailable", format!("projects/{name}.toml: {error}"))
    };
    let text = std::fs::read_to_string(&file).map_err(unavailable)?;
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            Failure::new(
                "invalid-config",
                format!("projects/{name}.toml: {}", error.message()),
            )
        })?;
    edit(&project, &mut document)?;
    let text = document.to_string();
    let edited = Project::parse(name, &text, &projects.data)
        .map_err(|error| Failure::new("invalid-config", error.to_string()))?;
    crate::providers::write_in_place(&file, &text).map_err(unavailable)?;
    announce_edits(daemon, projects)?;
    Ok(edited)
}

/// At start, every project's file is brought to the keys of this version (`former_keys_renamed`),
/// where it stands: what the user wrote around them, comments included, stays as it is, and the
/// file is replaced at once, never left half written. A file that the daemon could use is
/// written only when it still can afterwards.
pub fn migrate_files(daemon: &Daemon) {
    let names: Vec<String> = {
        let projects = daemon.projects.lock().unwrap();
        let Ok(loaded) = projects.registry.load_all() else {
            return;
        };
        loaded
            .projects
            .iter()
            .map(|project| project.name.clone())
            .chain(loaded.invalid.iter().map(|error| error.name.clone()))
            .collect()
    };
    for name in names {
        let mut projects = daemon.projects.lock().unwrap();
        let file = projects.registry.file(&name);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for task in tasks_twice(&text) {
            eprintln!(
                "flottille daemon: projects/{name}.toml: commands.{task} and tasks.{task} are \
                 both kept: remove the one you do not want"
            );
        }
        let Some(migrated) = migrated(&text) else {
            continue;
        };
        // The file is written only when it reads afterwards: one that worked still does, one that
        // did not now does, or else it stays as the user left it.
        let usable = |text: &str| Project::parse(&name, text, &projects.data).is_ok();
        if !usable(&migrated) {
            if usable(&text) {
                eprintln!(
                    "flottille daemon: projects/{name}.toml: its former keys are kept: the file \
                     would not read the same with their new names"
                );
            }
            continue;
        }
        if let Err(error) = crate::providers::write_in_place(&file, &migrated) {
            eprintln!("flottille daemon: projects/{name}.toml: its former keys are kept: {error}");
            continue;
        }
        let _ = announce_edits(daemon, &mut projects);
    }
}

/// The text of a project's file with its keys brought to this version, `None` when it holds
/// none of the former ones (or is no TOML at all).
pub fn migrated(text: &str) -> Option<String> {
    let mut document: toml_edit::DocumentMut = text.parse().ok()?;
    former_keys_renamed(&mut document).then(|| document.to_string())
}

/// The tasks a file holds twice, under their former key `[commands.<name>]` and under
/// `[tasks.<name>]`: neither is moved, the user settles which one stays.
pub fn tasks_twice(text: &str) -> Vec<String> {
    let Ok(document) = text.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    former_tasks(document.as_table())
        .into_iter()
        .filter(|name| {
            document
                .get("tasks")
                .and_then(toml_edit::Item::as_table_like)
                .is_some_and(|tasks| tasks.contains_key(name))
        })
        .collect()
}

/// The entries of `[commands]` that are tasks, by their `line`.
fn former_tasks(root: &toml_edit::Table) -> Vec<String> {
    root.get("commands")
        .and_then(toml_edit::Item::as_table_like)
        .map(|commands| {
            commands
                .iter()
                .filter(|(_, entry)| {
                    entry
                        .as_table_like()
                        .is_some_and(|e| e.contains_key("line"))
                })
                .map(|(name, _)| name.to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Renames the keys of earlier versions in the document, keeping everything else, the comments
/// above and beside each entry included, and the order of the file:
///
/// - a task, `[commands.<name>]` with a `line`, goes under `[tasks.<name>]`, unless the file has
///   that task there already (`tasks_twice`);
/// - `[procedures.<name>]` becomes `[commands.<name>]`, once the tasks have left `commands`;
/// - `[references.<name>]` becomes `[skills.<name>]`;
/// - `[[recipe]]` becomes `[[setup]]`, and a step's `command` its `task`;
/// - the rights the gate no longer hands out go (`docs/design.md` §7): `[agent_actions]`, a
///   task's `added_by`, a command's `grants` and `children`.
///
/// Whether anything changed.
pub fn former_keys_renamed(document: &mut toml_edit::DocumentMut) -> bool {
    let mut changed = false;
    let root = document.as_table_mut();
    for name in former_tasks(root) {
        let twice = root
            .get("tasks")
            .and_then(toml_edit::Item::as_table_like)
            .is_some_and(|tasks| tasks.contains_key(&name));
        if !twice {
            changed |= moved(root, "commands", &name, "tasks");
        }
    }
    changed |= merged(root, "procedures", "commands");
    changed |= merged(root, "references", "skills");
    if !root.contains_key("setup")
        && let Some((key, recipe)) = root.remove_entry("recipe")
    {
        root.insert_formatted(&renamed(&key, "setup"), recipe);
        changed = true;
    }
    if let Some(steps) = root
        .get_mut("setup")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
    {
        for step in steps.iter_mut() {
            changed |= key_renamed(step, "command", "task");
        }
    }
    changed |= root.remove("agent_actions").is_some();
    for (table, keys) in [
        ("tasks", &["added_by"][..]),
        ("commands", &["grants", "children"][..]),
    ] {
        let Some(entries) = root
            .get_mut(table)
            .and_then(toml_edit::Item::as_table_like_mut)
        else {
            continue;
        };
        for (_, entry) in entries.iter_mut() {
            if let Some(entry) = entry.as_table_like_mut() {
                for key in keys {
                    changed |= entry.remove(key).is_some();
                }
            }
        }
    }
    changed
}

/// The key, named `name`, with the comments and spaces around it kept.
fn renamed(key: &toml_edit::Key, name: &str) -> toml_edit::Key {
    toml_edit::Key::new(name)
        .with_leaf_decor(key.leaf_decor().clone())
        .with_dotted_decor(key.dotted_decor().clone())
}

/// Moves the entry `name` of the table `from` into the table `to`, made when it is missing; `from`
/// goes once it is empty, unless its header holds a comment. Nothing moves when `to` is no table.
fn moved(root: &mut toml_edit::Table, from: &str, name: &str, to: &str) -> bool {
    if root
        .get(to)
        .is_some_and(|target| !target.is_table() && !target.is_inline_table())
    {
        return false;
    }
    let Some(table) = root
        .get_mut(from)
        .and_then(toml_edit::Item::as_table_like_mut)
    else {
        return false;
    };
    let Some(key) = table.key(name).cloned() else {
        return false;
    };
    let Some(entry) = table.remove(name) else {
        return false;
    };
    let commented = |decor: &toml_edit::Decor| {
        [decor.prefix(), decor.suffix()]
            .into_iter()
            .flatten()
            .any(|text| text.as_str().is_some_and(|text| text.contains('#')))
    };
    if root.get(from).is_some_and(|table| match table {
        toml_edit::Item::Table(table) => table.is_empty() && !commented(table.decor()),
        toml_edit::Item::Value(toml_edit::Value::InlineTable(table)) => {
            table.is_empty() && !commented(table.decor())
        }
        _ => false,
    }) {
        root.remove(from);
    }
    let target = root.entry(to).or_insert_with(|| {
        let mut table = toml_edit::Table::new();
        // Only the entries show: `[tasks.name]`, never an empty `[tasks]`.
        table.set_implicit(true);
        toml_edit::Item::Table(table)
    });
    match target {
        toml_edit::Item::Table(table) => {
            table.insert_formatted(&key, entry);
        }
        toml_edit::Item::Value(toml_edit::Value::InlineTable(table)) => {
            if let Ok(value) = entry.into_value() {
                table.insert_formatted(&key, value);
            }
        }
        _ => return false,
    }
    true
}

/// The table `from` renamed `to`, or its entries moved into `to` when the file has both.
fn merged(root: &mut toml_edit::Table, from: &str, to: &str) -> bool {
    if !root.contains_key(to) {
        let Some((key, table)) = root.remove_entry(from) else {
            return false;
        };
        root.insert_formatted(&renamed(&key, to), table);
        return true;
    }
    let names: Vec<String> = root
        .get(from)
        .and_then(toml_edit::Item::as_table_like)
        .map(|table| table.iter().map(|(name, _)| name.to_owned()).collect())
        .unwrap_or_default();
    let mut changed = false;
    for name in names {
        // A name in both is the user's to settle: the file says so when it is read.
        let twice = root
            .get(to)
            .and_then(toml_edit::Item::as_table_like)
            .is_some_and(|table| table.contains_key(&name));
        if !twice {
            changed |= moved(root, from, &name, to);
        }
    }
    changed
}

/// The key `from` of a table renamed `to`, in its place among the others.
fn key_renamed(table: &mut toml_edit::Table, from: &str, to: &str) -> bool {
    if !table.contains_key(from) || table.contains_key(to) {
        return false;
    }
    let entries: Vec<(toml_edit::Key, toml_edit::Item)> = table
        .iter()
        .map(|(name, _)| name.to_owned())
        .collect::<Vec<_>>()
        .into_iter()
        .filter_map(|name| table.remove_entry(&name))
        .collect();
    for (key, item) in entries {
        let key = if key.get() == from {
            renamed(&key, to)
        } else {
            key
        };
        table.insert_formatted(&key, item);
    }
    true
}

/// The project as its file says now.
pub fn load_project(daemon: &Daemon, name: &str) -> std::result::Result<Project, Failure> {
    load(&daemon.projects.lock().unwrap(), name)
}

/// Reads the files and announces what changed in them since the last request: a project that
/// appeared, one that was edited, one that is gone. A file that cannot be used is neither: it is
/// reported by the requests that read it, and the project it was stays known until it is fixed.
fn announce_edits(
    daemon: &Daemon,
    projects: &mut Projects,
) -> std::result::Result<flottille_core::registry::Loaded, Failure> {
    let loaded = projects.registry.load_all()?;
    for project in &loaded.projects {
        let event = match projects.known.get(&project.name) {
            None => "project.added",
            Some(known) if known != project => "project.changed",
            Some(_) => continue,
        };
        emit(
            daemon,
            event,
            json!({"name": project.name, "folder": project.folder}),
        )?;
        projects.known.insert(project.name.clone(), project.clone());
    }
    let gone: Vec<String> = projects
        .known
        .keys()
        .filter(|name| {
            !loaded.projects.iter().any(|project| &project.name == *name)
                && !loaded.invalid.iter().any(|error| &error.name == *name)
        })
        .cloned()
        .collect();
    for name in gone {
        projects.known.remove(&name);
        emit(daemon, "project.removed", json!({"name": name}))?;
    }
    Ok(loaded)
}

/// What registering a folder would register.
struct Plan {
    name: String,
    folder: PathBuf,
    repositories: Vec<String>,
}

fn plan(projects: &Projects, asked: ProjectFolder) -> std::result::Result<Plan, Failure> {
    let typed = Path::new(&asked.folder);
    if !typed.is_absolute() {
        return Err(Failure::refused(format!(
            "the folder must be an absolute path, not {}",
            asked.folder
        )));
    }
    let folder = typed.canonicalize().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => {
            Failure::refused(format!("{} does not exist", asked.folder))
        }
        _ => Failure::refused(format!("cannot read {}: {error}", asked.folder)),
    })?;
    if !folder.is_dir() {
        return Err(Failure::refused(format!(
            "{} is not a folder",
            folder.display()
        )));
    }

    let name = match asked.name {
        Some(name) if valid_name(&name) => name,
        Some(name) => {
            return Err(Failure::refused(format!(
                "{name} is not a name: use 1 to 64 letters, digits, dots, dashes or underscores, \
                 starting with a letter or a digit"
            )));
        }
        None => {
            let base = folder.file_name().and_then(|name| name.to_str());
            Some(slug(base.unwrap_or_default()))
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    Failure::refused(format!(
                        "no name can be made from {}: give one with --name",
                        folder.display()
                    ))
                })?
        }
    };

    let repositories = detect::repositories(&folder)
        .map_err(|error| Failure::refused(format!("cannot read {}: {error}", folder.display())))?;
    if repositories.is_empty() {
        return Err(Failure::refused(format!(
            "no Git repository in {}: it is not one, and none of its folders one or two levels \
             down holds a .git",
            folder.display()
        )));
    }

    // A file that cannot be used is a project all the same: its name is taken. A file that cannot
    // be read is another matter, said as it is.
    let taken = match projects.registry.load(&name) {
        Ok(project) => project.is_some(),
        Err(Error::Config(_)) => true,
        Err(error) => return Err(error.into()),
    };
    if taken || projects.known.contains_key(&name) {
        return Err(Failure::refused(format!(
            "a project named {name} already exists"
        )));
    }
    if let Some(project) = projects.known.values().find(|project| {
        project.folder == folder || project.folder.canonicalize().ok().as_ref() == Some(&folder)
    }) {
        return Err(Failure::refused(format!(
            "{} is already registered as {}",
            folder.display(),
            project.name
        )));
    }
    Ok(Plan {
        name,
        folder,
        repositories,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const FORMER: &str =
        include_str!("../../flottille-core/tests/fixtures/multi-repo-case-former-keys.toml");
    const CURRENT: &str = include_str!("../../flottille-core/tests/fixtures/multi-repo-case.toml");

    fn parse(text: &str) -> Project {
        Project::parse("app", text, Path::new("/data/flottille")).unwrap()
    }

    /// Each task's name and line.
    fn lines(project: &Project) -> Vec<(&str, &str)> {
        project
            .tasks
            .iter()
            .map(|(name, task)| (name.as_str(), task.line.as_str()))
            .collect()
    }

    #[test]
    fn the_former_keys_of_a_file_get_their_new_names_and_its_comments_stay() {
        let migrated = migrated(FORMER).expect("the file holds former keys");

        for comment in FORMER.lines().filter(|line| line.starts_with('#')) {
            assert!(migrated.contains(comment), "{comment}\n---\n{migrated}");
        }
        for former in [
            "[commands.\"api:dev\"]",
            "[procedures",
            "[references",
            "[[recipe]]",
            "[agent_actions]",
            "grants",
            "added_by",
            "command = ",
        ] {
            assert!(!migrated.contains(former), "{former}\n---\n{migrated}");
        }
        // Each entry stays where it was, its comment above it.
        for kept in [
            "# What we do before every review.\n[commands.open-prs]\n",
            "[skills.style] # the team's own words\n",
            "# The test data, once.\n[[setup]]\ntask = \"generate-data\"\n",
            "PATH = \"{workspace.path}/.bin\"\n\n[tasks.\"api:dev\"]\n",
        ] {
            assert!(migrated.contains(kept), "{kept}\n---\n{migrated}");
        }
        let (before, after) = (parse(FORMER), parse(&migrated));
        assert_eq!(lines(&after), lines(&before));
        assert_eq!(format!("{:?}", after.setup), format!("{:?}", before.setup));
        assert_eq!(after.commands.keys().collect::<Vec<_>>(), ["open-prs"]);
        assert_eq!(after.skills.keys().collect::<Vec<_>>(), ["style"]);
        assert_eq!(after.tasks.len(), 5);
        assert_eq!(after.setup.len(), 4);
    }

    #[test]
    fn a_task_under_both_its_former_and_its_new_key_stays_under_both() {
        let text = "folder = \"/work/app\"\n\n\
            # The build we had.\n[commands.build]\nline = \"make\"\n\n\
            # The build we have.\n[tasks.build]\nline = \"make all\"\n\n\
            [references.style]\npath = \"docs/style.md\"\n";

        assert_eq!(tasks_twice(text), ["build"]);
        let migrated = migrated(text).expect("the reference is renamed");
        for kept in [
            "# The build we had.\n[commands.build]\nline = \"make\"\n",
            "# The build we have.\n[tasks.build]\nline = \"make all\"\n",
            "[skills.style]",
        ] {
            assert!(migrated.contains(kept), "{kept}\n---\n{migrated}");
        }
    }

    #[test]
    fn a_tasks_that_is_not_a_table_loses_nothing() {
        let text = "tasks = \"oops\"\n\n# Ours.\n[commands.build]\nline = \"make\"\n";

        assert_eq!(migrated(text), None);
        let mut document: toml_edit::DocumentMut = text.parse().unwrap();
        assert!(!former_keys_renamed(&mut document));
        assert_eq!(document.to_string(), text);
    }

    #[test]
    fn the_comment_of_a_commands_header_stays_when_its_tasks_leave() {
        let text = "# What we run.\n[commands]\n\n[commands.build]\nline = \"make\"\n";

        let migrated = migrated(text).unwrap();
        assert!(
            migrated.contains("# What we run.\n[commands]\n"),
            "{migrated}"
        );
        assert!(migrated.contains("[tasks.build]\n"), "{migrated}");
        assert!(!migrated.contains("[commands.build]"), "{migrated}");
    }

    #[test]
    fn a_file_with_the_new_keys_is_left_as_it_is() {
        assert_eq!(migrated(CURRENT), None);
        let once = migrated(FORMER).unwrap();
        assert_eq!(migrated(&once), None, "a second start changes nothing");
        assert_eq!(migrated("not = [toml"), None);
    }

    #[test]
    fn the_new_fixture_reads_as_the_former_one_brought_to_the_new_keys() {
        let (current, former) = (parse(CURRENT), parse(FORMER));
        assert_eq!(lines(&current), lines(&former));
        assert_eq!(current.setup.len() + 1, former.setup.len());
    }
}
