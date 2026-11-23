//! Where this machine makes the projects that come with another machine's launches
//! (`docs/remote.md`, "Which project"): `$XDG_DATA_HOME/flottille/projects` unless the user chose
//! a folder of their own, kept in `travelled.toml` of Flottille's configuration folder and set
//! from Settings, General (`travelled_folder.show`, `travelled_folder.set`), on this machine or
//! from another's window through "Settings of". Only the projects made after a change go there:
//! the ones made before stay where they are, nothing is moved.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::{self, Failure};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    /// The folder of the user's; none goes back to the default.
    folder: Option<String>,
}

fn file(daemon: &Daemon) -> PathBuf {
    daemon.paths.config.join("travelled.toml")
}

fn default_folder(daemon: &Daemon) -> PathBuf {
    daemon.paths.data.join("projects")
}

/// The folder the user chose, when the file names one; a file that cannot be read is none, and
/// says why.
fn own(daemon: &Daemon) -> Result<Option<PathBuf>, String> {
    let text = match std::fs::read_to_string(file(daemon)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("travelled.toml cannot be read: {error}")),
    };
    let document: toml_edit::DocumentMut =
        text.parse().map_err(|error: toml_edit::TomlError| {
            format!("travelled.toml is not followed: {}", error.message())
        })?;
    let folder = match document.get("folder") {
        None => return Ok(None),
        Some(item) => item
            .as_str()
            .map(PathBuf::from)
            .ok_or("travelled.toml is not followed: its folder is not a text")?,
    };
    if !folder.is_absolute() {
        return Err(format!(
            "travelled.toml is not followed: {} is not an absolute path",
            folder.display()
        ));
    }
    Ok(Some(folder))
}

/// Where this machine makes the next project another machine's launch needs.
pub(crate) fn folder(daemon: &Daemon) -> PathBuf {
    own(daemon)
        .ok()
        .flatten()
        .unwrap_or_else(|| default_folder(daemon))
}

/// Answers `travelled_folder.show` and `travelled_folder.set`.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "travelled_folder.show" => Ok(show(daemon)),
        "travelled_folder.set" => {
            let Set { folder } = serde_json::from_value(params)?;
            match folder.map(|folder| folder.trim().to_owned()) {
                Some(folder) if !folder.is_empty() => {
                    let folder = checked(daemon, &folder)?;
                    let text = format!(
                        "# Where the projects of other machines' launches are made.\nfolder = {}\n",
                        toml_edit::value(folder.to_string_lossy().as_ref())
                    );
                    std::fs::create_dir_all(&daemon.paths.config)
                        .and_then(|()| crate::providers::write_in_place(&file(daemon), &text))
                        .map_err(|e| Failure::new("unavailable", format!("travelled.toml: {e}")))?;
                }
                _ => match std::fs::remove_file(file(daemon)) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(Failure::new(
                            "unavailable",
                            format!("travelled.toml: {error}"),
                        ));
                    }
                    _ => {}
                },
            }
            Ok(show(daemon))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// `{"folder", "own", "default", "error"}`: the folder the next project is made in, the user's as
/// written (`null` when the default is followed), the default, and why the file is not followed.
fn show(daemon: &Daemon) -> Value {
    let (own, error) = match own(daemon) {
        Ok(own) => (own, None),
        Err(why) => (None, Some(why)),
    };
    json!({
        "folder": own.clone().unwrap_or_else(|| default_folder(daemon)),
        "own": own,
        "default": default_folder(daemon),
        "error": error,
    })
}

/// The folder `typed` as a project folder is checked: absolute (`~` is the home), reached through
/// no link, a folder when it exists, no disk's root nor the home itself, outside Flottille's
/// configuration and state, holding none of Flottille's folders, and neither a project's folder
/// nor inside one.
fn checked(daemon: &Daemon, typed: &str) -> Result<PathBuf, Failure> {
    let refused = |why: String| Failure::refused(why);
    let folder = projects::home_expanded(typed).unwrap_or_else(|| PathBuf::from(typed));
    if !folder.is_absolute() {
        return Err(refused(format!(
            "the folder must be an absolute path, not {typed}"
        )));
    }
    if folder
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(refused(format!(
            "{typed} holds `..`: give the folder itself"
        )));
    }
    // The deepest part that exists is where a link would lead elsewhere.
    let mut existing: &Path = &folder;
    while std::fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().unwrap_or(Path::new("/"));
    }
    let real = std::fs::canonicalize(existing)
        .map_err(|error| refused(format!("cannot read {}: {error}", existing.display())))?;
    if real != existing {
        return Err(refused(format!(
            "{} leads elsewhere through a link: give the folder it leads to",
            folder.display()
        )));
    }
    if existing == folder && !real.is_dir() {
        return Err(refused(format!("{} is not a folder", folder.display())));
    }
    // A whole disk or the home would be filled with other machines' projects, and taken for
    // theirs when one is removed.
    use std::os::unix::fs::MetadataExt as _;
    let device = |path: &Path| std::fs::metadata(path).map(|metadata| metadata.dev()).ok();
    let a_root = match folder.parent() {
        None => true,
        Some(parent) => existing == folder && device(&folder) != device(parent),
    };
    if a_root {
        return Err(refused(format!(
            "{} is the root of a disk: give a folder in it",
            folder.display()
        )));
    }
    if is_the_home(projects::home_expanded("~"), &folder) {
        return Err(refused(format!(
            "{} is the home itself: give a folder in it",
            folder.display()
        )));
    }
    // The default is in Flottille's data: a folder in it is fine, the data folder itself is not.
    for (kept, what, inside) in [
        (&daemon.paths.config, "Flottille's configuration", false),
        (&daemon.paths.state, "Flottille's state", false),
        (&daemon.paths.data, "Flottille's data", true),
    ] {
        let kept = std::fs::canonicalize(kept).unwrap_or_else(|_| kept.clone());
        if kept.starts_with(&folder) {
            return Err(refused(format!("{} holds {what}", folder.display())));
        }
        if folder.starts_with(&kept) && !inside {
            return Err(refused(format!("{} is in {what}", folder.display())));
        }
    }
    for project in projects::all(daemon) {
        let of = std::fs::canonicalize(&project.folder).unwrap_or(project.folder.clone());
        if folder.starts_with(&of) {
            return Err(refused(format!(
                "{} is inside the folder of the project {}: projects do not go inside another",
                folder.display(),
                project.name
            )));
        }
    }
    Ok(folder)
}

/// Whether `folder` (a path with no link in it) is the user's home: both sides canonical, so a home
/// that is itself a link to its folder is recognised.
fn is_the_home(home: Option<PathBuf>, folder: &Path) -> bool {
    home.is_some_and(|home| std::fs::canonicalize(&home).unwrap_or(home) == folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_that_is_a_link_is_the_home_by_its_folder_too() {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = real.parent().unwrap().join("home");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(is_the_home(Some(link.clone()), &real));
        assert!(is_the_home(Some(real.clone()), &real));
        assert!(!is_the_home(Some(link), &real.join("sub")));
        assert!(!is_the_home(None, &real));
    }
}
