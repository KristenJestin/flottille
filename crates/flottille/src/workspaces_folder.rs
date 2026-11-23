//! Where the workspaces go (`docs/projects.md`, "Where the workspaces go"): the user's default for
//! every project (`workspaces.toml` of Flottille's configuration folder) and each project's own
//! (the `workspaces_root` key of its file), set from the window. Only workspaces made after a
//! change follow it: a workspace's record holds its folder, and nothing is ever moved.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use flottille_core::place::{self, Place};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::{self, Failure};
use crate::providers;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Which {
    project: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    project: Option<String>,
    /// The default: `data`, `beside` or `own`.
    place: Option<String>,
    /// The default's folder when `place` is `own`.
    path: Option<String>,
    /// A project's own folder; none removes it, and the project follows the default.
    own: Option<String>,
}

/// Answers a `workspaces_folder.*` request of the user: `workspaces_folder.show`,
/// `workspaces_folder.set`, for the default (no `project`) or one project's. No tool of an agent
/// reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "workspaces_folder.show" => {
            let Which { project } = serde_json::from_value(params)?;
            show(daemon, project.as_deref())
        }
        "workspaces_folder.set" => {
            let set: Set = serde_json::from_value(params)?;
            match set.project.clone() {
                Some(name) => set_project(daemon, &name, set.own)?,
                None => set_default(daemon, set.place, set.path)?,
            }
            announce(daemon, set.project.as_deref())?;
            show(daemon, set.project.as_deref())
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// The default: `{"place", "path", "error", "data"}`, `path` being what the user typed, `error` why
/// the file cannot be followed (the data folder is) and `data` Flottille's data folder. A project: `{"project", "own", "folder",
/// "default_folder", "fallback"}`: its own folder as typed (`null` when it follows the default),
/// the folder its workspaces are made in, the one the default would give it, and why the default
/// is not followed (`null` when it is, or when the project has a folder of its own).
fn show(daemon: &Daemon, project: Option<&str>) -> Result<Value, Failure> {
    let read = daemon.projects.lock().unwrap().registry().place_read();
    let Some(name) = project else {
        let (place, error) = match read {
            Ok(place) => (place, None),
            Err(error) => (Place::Data, Some(error)),
        };
        let path = match &place {
            Place::Own(path) => Some(path.clone()),
            _ => None,
        };
        return Ok(json!({
            "place": place.word(),
            "path": path,
            "error": error,
            "data": daemon.paths.data,
        }));
    };
    let loaded = projects::load_project(daemon, name)?;
    let fallback = read
        .clone()
        .err()
        .filter(|_| loaded.workspaces_root_own.is_none());
    let default_folder = read.unwrap_or_default().resolve(
        name,
        &loaded.folder,
        &daemon.paths.data,
        place::home().as_deref(),
        "path",
    );
    Ok(json!({
        "project": name,
        "own": loaded.workspaces_root_own,
        "folder": loaded.workspaces_root,
        "default_folder": default_folder.ok(),
        "fallback": fallback,
    }))
}

fn set_default(daemon: &Daemon, word: Option<String>, path: Option<String>) -> Result<(), Failure> {
    let place = match word.as_deref() {
        Some("data") => Place::Data,
        Some("beside") => Place::Beside,
        Some("own") => {
            let path = path.ok_or_else(|| {
                Failure::new("invalid-params", "path: a folder of your own needs one")
            })?;
            let invalid = |why| Failure::new("invalid-params", why);
            // Checked as a project would resolve it, before anything is written.
            let home = place::home();
            let avoid = place::Avoid {
                config: Some(&daemon.paths.config),
                data: &daemon.paths.data,
                home: home.as_deref(),
            };
            place::checked_default(&path, &avoid, "path").map_err(invalid)?;
            // The folder each project would get must keep out of that project's repositories.
            let place = Place::Own(path);
            let loaded = daemon.projects.lock().unwrap().registry().load_all()?;
            for project in &loaded.projects {
                let folder = place.resolve(
                    &project.name,
                    &project.folder,
                    avoid.data,
                    home.as_deref(),
                    "path",
                );
                let repositories: Vec<_> = project
                    .repositories
                    .iter()
                    .map(|repository| project.folder.join(&repository.path))
                    .collect();
                let folder = folder
                    .and_then(|folder| {
                        place::allowed(&folder, &avoid, &repositories, "path").map(|_| folder)
                    })
                    .map_err(|why| invalid(format!("{why} (for the project {})", project.name)))?;
                usable(&folder).map_err(Failure::refused)?;
            }
            place
        }
        _ => {
            return Err(Failure::new("invalid-params", "place: data, beside or own"));
        }
    };
    let paths = &daemon.paths;
    std::fs::create_dir_all(&paths.config)
        .map_err(|e| Failure::new("unavailable", format!("{}: {e}", paths.config.display())))?;
    providers::write_in_place(&paths.workspaces_place(), &place::to_toml(&place))
        .map_err(|e| Failure::new("unavailable", format!("workspaces.toml: {e}")))
}

/// Writes the project's own folder in its file, where the key stands, or removes it (`None`): the
/// project follows the default again. The file is read back as a project before it is written, so
/// a folder that is not absolute is refused with its key.
fn set_project(daemon: &Daemon, name: &str, own: Option<String>) -> Result<(), Failure> {
    let follows_default = own.is_none();
    projects::rewrite_checked(
        daemon,
        name,
        |_, document| {
            match own {
                Some(own) => document["workspaces_root"] = toml_edit::value(own),
                None => {
                    document.remove("workspaces_root");
                }
            }
            Ok(())
        },
        // The folder as the project resolves it, before the file is written.
        |edited| match follows_default {
            true => Ok(()),
            false => usable(&edited.workspaces_root).map_err(Failure::refused),
        },
    )
    .map(|_| ())
}

/// Whether the daemon, as its own user, can make `folder` and write in it, found by making a
/// probe folder there and removing it (and the folders made on the way), never by reading
/// permissions: a mount, a read-only file system or an ACL says no all the same. The reason is
/// the sentence a refusal gives: `/srv/nowhere/app: permission denied`.
pub fn usable(folder: &Path) -> Result<(), String> {
    static PROBES: AtomicU64 = AtomicU64::new(0);
    let mut made: Vec<&Path> = Vec::new();
    for ancestor in folder.ancestors() {
        if ancestor.symlink_metadata().is_ok() {
            break;
        }
        made.push(ancestor);
    }
    let probe = folder.join(format!(
        ".flottille-probe-{}-{}",
        std::process::id(),
        PROBES.fetch_add(1, Ordering::Relaxed)
    ));
    let tried = std::fs::create_dir_all(folder)
        .and_then(|()| std::fs::create_dir(&probe))
        .and_then(|()| std::fs::remove_dir(&probe));
    // Deepest first, and only what is empty: a folder that is not ours to remove stays.
    for folder in made {
        let _ = std::fs::remove_dir(folder);
    }
    tried.map_err(|error| format!("{}: {}", folder.display(), reason(&error)))
}

/// An I/O error as a short lowercase reason: `permission denied`, without the `(os error 13)`.
fn reason(error: &std::io::Error) -> String {
    let text = error.to_string();
    let text = text.split(" (os error").next().unwrap_or(&text);
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => text.to_owned(),
    }
}

fn announce(daemon: &Daemon, project: Option<&str>) -> Result<(), Failure> {
    daemon
        .emit("workspaces_folder.changed", json!({"project": project}))
        .map(|_| ())
        .map_err(|error| Failure::new("unavailable", error.to_string()))
}
