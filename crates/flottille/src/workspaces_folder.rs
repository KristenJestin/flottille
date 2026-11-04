//! Where the workspaces go (`docs/projects.md`, "Where the workspaces go"): the user's default for
//! every project (`workspaces.toml` of Flottille's configuration folder) and each project's own
//! (the `workspaces_root` key of its file), set from the window. Only workspaces made after a
//! change follow it: a workspace's record holds its folder, and nothing is ever moved.

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

/// The default: `{"place", "path", "error"}`, `path` being what the user typed and `error` why the
/// file cannot be followed (the data folder is). A project: `{"project", "own", "folder",
/// "default_folder"}`: its own folder as typed (`null` when it follows the default), the folder
/// its workspaces are made in, and the one the default would give it.
fn show(daemon: &Daemon, project: Option<&str>) -> Result<Value, Failure> {
    let file = daemon.paths.workspaces_place();
    let read = place::read(&file);
    let Some(name) = project else {
        let (place, error) = match read {
            Ok(place) => (place, None),
            Err(error) => (Place::Data, Some(error)),
        };
        let path = match &place {
            Place::Own(path) => Some(path.clone()),
            _ => None,
        };
        return Ok(json!({"place": place.word(), "path": path, "error": error}));
    };
    let loaded = projects::load_project(daemon, name)?;
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
            // Checked as a project would resolve it, before anything is written.
            place::typed(&path, "project", place::home().as_deref(), "path")
                .map_err(|why| Failure::new("invalid-params", why))?;
            Place::Own(path)
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
    projects::rewrite(daemon, name, |_, document| {
        match own {
            Some(own) => document["workspaces_root"] = toml_edit::value(own),
            None => {
                document.remove("workspaces_root");
            }
        }
        Ok(())
    })
    .map(|_| ())
}

fn announce(daemon: &Daemon, project: Option<&str>) -> Result<(), Failure> {
    daemon
        .emit("workspaces_folder.changed", json!({"project": project}))
        .map(|_| ())
        .map_err(|error| Failure::new("unavailable", error.to_string()))
}
