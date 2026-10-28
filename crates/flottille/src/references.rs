//! The tool `reference_read` (`docs/tools.md`): a project's references, documents an agent reads
//! when they are relevant (`docs/projects.md`, "References").
//!
//! A reference is read from the project folder, whatever the agent's place: it is the project's,
//! not the branch's. Every path is checked against the reference itself, as `fs_read` checks
//! against the place: no `..` and no link leads out of it.

use std::path::{Component, Path, PathBuf};

use flottille_core::project::{Project, Reference};
use flottille_core::secrets;
use serde::Deserialize;
use serde_json::Value;

use crate::files::{self, Context, Range};
use crate::gate;
use crate::mcp::{Outcome, arguments};
use crate::place::Resolved;
use crate::projects;

/// The files a folder reference lists, at most.
const LISTED_MAX: usize = 1000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArguments {
    name: String,
    path: Option<String>,
    range: Option<Range>,
}

pub fn reference_read(cx: &Context, args: Value) -> Outcome {
    let args: ReadArguments = match arguments("reference_read", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    let (offset, length) = match files::page_of(args.range.as_ref()) {
        Ok(page) => page,
        Err(refusal) => return refusal,
    };
    let project = match projects::load_project(cx.daemon, &cx.place.project) {
        Ok(project) => project,
        Err(failure) => return Outcome::Failed(failure.message),
    };
    match read(&project, &args, offset, length) {
        Ok(outcome) | Err(outcome) => outcome,
    }
}

fn read(
    project: &Project,
    args: &ReadArguments,
    offset: u64,
    length: u64,
) -> Result<Outcome, Outcome> {
    let name = args.name.as_str();
    let Some(reference) = project.references.get(name) else {
        return Err(Outcome::Refused(if project.references.is_empty() {
            format!("refused: no reference named {name}: the project has no references")
        } else {
            let names: Vec<&str> = project.references.keys().map(String::as_str).collect();
            format!(
                "refused: no reference named {name}: the project's references are {}",
                names.join(", ")
            )
        }));
    };
    let root = root_of(project, name, reference)?;
    let folder = project.folder.canonicalize().unwrap_or_default();
    let is_dir = root.is_dir();
    let Some(named) = args.path.as_deref() else {
        held_for_the_user(&folder, &root, name)?;
        if !is_dir {
            return read_file(name, name, &root, offset, length);
        }
        return Ok(listing(name, &root, &folder));
    };
    if !is_dir {
        return Err(Outcome::Refused(format!(
            "refused: the reference {name} is a file: it takes no path, read it with just its name"
        )));
    }
    let target = inside(name, &root, named)?;
    let shown = format!("{name}/{}", relative(&root, &target));
    held_for_the_user(&folder, &target, &shown)?;
    if target.is_dir() {
        return Ok(listing(&shown, &target, &folder));
    }
    read_file(name, &shown, &target, offset, length)
}

/// The reference's real path, which must still be inside the project folder.
fn root_of(project: &Project, name: &str, reference: &Reference) -> Result<PathBuf, Outcome> {
    let folder = project
        .folder
        .canonicalize()
        .map_err(|error| Outcome::Failed(format!("the project folder cannot be read: {error}")))?;
    let root = project
        .folder
        .join(&reference.path)
        .canonicalize()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Outcome::Refused(format!(
                "refused: the reference {name} points to {}, which is not there any more: the \
                 user fixes or removes it (flottille reference)",
                reference.path
            )),
            _ => Outcome::Failed(format!("the reference {name}: {error}")),
        })?;
    if !root.starts_with(&folder) {
        return Err(Outcome::Refused(format!(
            "refused: the reference {name} leads outside the project folder: it is not served"
        )));
    }
    Ok(root)
}

/// `named` inside the reference `root`: taken on the text first (a step out of the reference is
/// outside, the disk is not looked at), then on the disk, every link followed.
fn inside(name: &str, root: &Path, named: &str) -> Result<PathBuf, Outcome> {
    let outside = || {
        Outcome::Refused(format!(
            "refused: {named} is outside the reference {name}: it serves its own files only"
        ))
    };
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in Path::new(named).components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop().ok_or_else(outside)?;
            }
            Component::RootDir | Component::Prefix(_) => return Err(outside()),
        }
    }
    if named.starts_with('~') {
        return Err(outside());
    }
    let written: PathBuf = parts.iter().collect();
    let real = root
        .join(&written)
        .canonicalize()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Outcome::Refused(format!(
                "refused: {named} does not exist in the reference {name}"
            )),
            _ => Outcome::Failed(format!("{named}: {error}")),
        })?;
    let Ok(below) = real.strip_prefix(root) else {
        return Err(outside());
    };
    for part in written.components().chain(below.components()) {
        let part = part.as_os_str().to_string_lossy();
        if part == ".git" {
            return Err(Outcome::Refused(format!(
                "refused: {named} goes through a .git folder, which a reference never serves"
            )));
        }
        if secrets::secret_file(&part) {
            return Err(Outcome::Refused(format!(
                "refused: {named} is a file of secrets, which a reference never serves"
            )));
        }
    }
    Ok(real)
}

/// Whether `fs_read` would hold the real path `target` for the user, by the gate's own rule of
/// sensitive places, taken from the project folder: a reference never serves it, asked or not.
fn sensitive(folder: &Path, target: &Path) -> Option<String> {
    let path = Resolved {
        shown: relative(folder, target),
        path: target.to_owned(),
        inside: true,
    };
    gate::sensitive(&[], &path)
}

fn held_for_the_user(folder: &Path, target: &Path, named: &str) -> Result<(), Outcome> {
    match sensitive(folder, target) {
        Some(place) => Err(Outcome::Refused(format!(
            "refused: {named} is in a sensitive place ({place}), which a reference never serves: \
             read it with fs_read, which asks the user first"
        ))),
        None => Ok(()),
    }
}

fn relative(root: &Path, target: &Path) -> String {
    target
        .strip_prefix(root)
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn read_file(
    name: &str,
    shown: &str,
    path: &Path,
    offset: u64,
    length: u64,
) -> Result<Outcome, Outcome> {
    let file = Resolved {
        shown: shown.to_owned(),
        path: path.to_owned(),
        inside: true,
    };
    if path
        .file_name()
        .is_some_and(|file| secrets::secret_file(&file.to_string_lossy()))
    {
        return Err(Outcome::Refused(format!(
            "refused: the reference {name} is a file of secrets, which a reference never serves"
        )));
    }
    files::read_page(None, &file, offset, length).or_else(|error| Ok(files::failed(&file, error)))
}

/// The regular files of a folder, sorted, with their sizes: what to name in `path`. Links, `.git`,
/// files of secrets and sensitive places are left out.
fn listing(shown: &str, folder: &Path, project: &Path) -> Outcome {
    let mut found = Vec::new();
    let truncated = collect(folder, Path::new(""), project, &mut found);
    found.sort();
    let mut text = format!("{shown}: {} file(s)\n", found.len());
    for (path, size) in &found {
        text.push_str(&format!("{path}  {size}\n"));
    }
    if truncated {
        text.push_str(&format!(
            "… stopped at {LISTED_MAX} files: read a folder below\n"
        ));
    }
    text.push_str("Read one with reference_read {name, path}.");
    Outcome::Done(text)
}

/// Whether it stopped early.
fn collect(folder: &Path, relative: &Path, project: &Path, found: &mut Vec<(String, u64)>) -> bool {
    let Ok(entries) = std::fs::read_dir(folder.join(relative)) else {
        return false;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        if name == ".git" || secrets::secret_file(&name) {
            continue;
        }
        let path = relative.join(&name);
        if sensitive(project, &folder.join(&path)).is_some() {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(folder.join(&path)) else {
            continue;
        };
        if metadata.is_dir() {
            if collect(folder, &path, project, found) {
                return true;
            }
        } else if metadata.is_file() {
            if found.len() == LISTED_MAX {
                return true;
            }
            found.push((path.display().to_string(), metadata.len()));
        }
    }
    false
}
