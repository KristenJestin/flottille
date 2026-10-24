//! The `project.*` methods: registering folders and keeping each project's configuration
//! (`docs/protocol.md`, `docs/projects.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flottille_core::detect;
use flottille_core::paths::Paths;
use flottille_core::project::{Project, slug, valid_name};
use flottille_core::protocol::{ProjectFolder, ProjectName};
use flottille_core::registry::Registry;
use flottille_core::{Error, Result};
use serde_json::{Value, json};

use crate::daemon::Daemon;

/// An error answer: a code of `docs/protocol.md` and a sentence.
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Failure {
        Failure {
            code,
            message: message.into(),
        }
    }

    fn refused(message: impl Into<String>) -> Failure {
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
