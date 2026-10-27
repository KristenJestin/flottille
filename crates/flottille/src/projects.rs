//! The `project.*` methods: registering folders and keeping each project's configuration
//! (`docs/protocol.md`, `docs/projects.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flottille_core::detect;
use flottille_core::paths::Paths;
use flottille_core::project::{Project, Reference, check_reference, slug, valid_name};
use flottille_core::protocol::{
    ProjectFolder, ProjectName, ReferenceAdd, ReferenceList, ReferenceRemove,
};
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
        "reference.list" => {
            let ReferenceList { project } = serde_json::from_value(params)?;
            let project = load(projects, &project)?;
            Ok(
                json!({"project": project.name, "references": project.references.iter()
                .map(|(name, r)| json!({"name": name, "description": r.description, "path": r.path}))
                .collect::<Vec<_>>()}),
            )
        }
        "reference.add" => {
            let asked: ReferenceAdd = serde_json::from_value(params)?;
            let project = load(projects, &asked.project)?;
            let path = match asked.path.trim_end_matches('/') {
                "" => asked.path.as_str(),
                trimmed => trimmed,
            };
            let reference = Reference {
                description: asked.description.trim().to_owned(),
                path: path.to_owned(),
            };
            check_reference(&asked.name, &reference)
                .map_err(|error| Failure::refused(format!("references.{}{error}", asked.name)))?;
            if project.references.contains_key(&asked.name) {
                return Err(Failure::refused(format!(
                    "project {} already has a reference named {}: remove it first",
                    project.name, asked.name
                )));
            }
            check_reachable(&project, &reference)?;
            edit_references(daemon, projects, &project, |table| {
                let mut entry = toml_edit::Table::new();
                entry.insert(
                    "description",
                    toml_edit::value(reference.description.as_str()),
                );
                entry.insert("path", toml_edit::value(reference.path.as_str()));
                table.insert(&asked.name, toml_edit::Item::Table(entry));
            })?;
            Ok(json!({"project": project.name, "name": asked.name,
                "description": reference.description, "path": reference.path}))
        }
        "reference.remove" => {
            let ReferenceRemove { project, name } = serde_json::from_value(params)?;
            let project = load(projects, &project)?;
            if !project.references.contains_key(&name) {
                return Err(Failure::new(
                    "not-found",
                    format!("project {} has no reference named {name}", project.name),
                ));
            }
            edit_references(daemon, projects, &project, |table| {
                table.remove(&name);
            })?;
            Ok(json!({"project": project.name, "removed": name}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// A reference is a file or a folder that is there now, and that stays in the project folder
/// once the links are followed.
fn check_reachable(project: &Project, reference: &Reference) -> std::result::Result<(), Failure> {
    let inside = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::NotFound => Failure::refused(format!(
            "{} does not exist in the project folder",
            reference.path
        )),
        _ => Failure::refused(format!("cannot read {}: {error}", reference.path)),
    };
    let folder = project.folder.canonicalize().map_err(|error| {
        Failure::refused(format!(
            "the folder {} of project {} cannot be read: {error}",
            project.folder.display(),
            project.name
        ))
    })?;
    let real = project
        .folder
        .join(&reference.path)
        .canonicalize()
        .map_err(inside)?;
    if !real.starts_with(&folder) {
        return Err(Failure::refused(format!(
            "{} leads outside the project folder: a reference stays inside it",
            reference.path
        )));
    }
    Ok(())
}

/// Changes the `references` table of the project's file and nothing else in it, comments
/// included, then announces the change. The new text is read back as a project before it is
/// written.
fn edit_references(
    daemon: &Daemon,
    projects: &mut Projects,
    project: &Project,
    change: impl FnOnce(&mut toml_edit::Table),
) -> std::result::Result<(), Failure> {
    let file = projects.registry.file(&project.name);
    let unavailable = |error: std::io::Error| {
        Failure::new(
            "unavailable",
            format!("projects/{}.toml: {error}", project.name),
        )
    };
    let text = std::fs::read_to_string(&file).map_err(unavailable)?;
    let mut document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            Failure::new(
                "invalid-config",
                format!("projects/{}.toml: {}", project.name, error.message()),
            )
        })?;
    let not_a_table = || {
        Failure::new(
            "invalid-config",
            format!("projects/{}.toml: references: not a table", project.name),
        )
    };
    if !document.contains_key("references") {
        let mut table = toml_edit::Table::new();
        // Only the entries show: `[references.name]`, never an empty `[references]`.
        table.set_implicit(true);
        document.insert("references", toml_edit::Item::Table(table));
    }
    let table = document["references"]
        .as_table_mut()
        .ok_or_else(not_a_table)?;
    change(table);
    if table.is_empty() {
        document.remove("references");
    }
    let text = document.to_string();
    let edited = Project::parse(&project.name, &text, &projects.data)
        .map_err(|error| Failure::new("invalid-config", error.to_string()))?;
    crate::providers::write_in_place(&file, &text).map_err(unavailable)?;
    projects.known.insert(edited.name.clone(), edited.clone());
    emit(
        daemon,
        "project.changed",
        json!({"name": edited.name, "folder": edited.folder}),
    )
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
