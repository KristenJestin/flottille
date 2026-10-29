//! A project's skills (`docs/skills.md`): texts an agent reads when they are relevant, with the
//! tool `skill_read`, and the `skill.*` methods that edit them.
//!
//! A skill is either a path of the project, read from the project folder whatever the agent's
//! place (it is the project's, not the branch's), or a text Flottille keeps in its data folder,
//! which an agent can write without writing in the project. Every path is checked against the
//! skill itself, as `fs_read` checks against the place: no `..` and no link leads out of it.

use std::path::{Component, Path, PathBuf};

use flottille_core::project::{Project, Skill, check_skill};
use flottille_core::secrets;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::files::{self, Context, Range};
use crate::gate;
use crate::mcp::{Outcome, arguments};
use crate::place::Resolved;
use crate::projects::{self, Failure};

/// The files a folder skill lists, at most.
const LISTED_MAX: usize = 1000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArguments {
    name: String,
    path: Option<String>,
    range: Option<Range>,
}

pub fn skill_read(cx: &Context, args: Value) -> Outcome {
    let args: ReadArguments = match arguments("skill_read", args) {
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
    match read(&cx.daemon.paths.data, &project, &args, offset, length) {
        Ok(outcome) | Err(outcome) => outcome,
    }
}

fn read(
    data: &Path,
    project: &Project,
    args: &ReadArguments,
    offset: u64,
    length: u64,
) -> Result<Outcome, Outcome> {
    let name = args.name.as_str();
    let Some(skill) = project.skills.get(name) else {
        return Err(Outcome::Refused(if project.skills.is_empty() {
            format!("refused: no skill named {name}: the project has no skills")
        } else {
            let names: Vec<&str> = project.skills.keys().map(String::as_str).collect();
            format!(
                "refused: no skill named {name}: the project's skills are {}",
                names.join(", ")
            )
        }));
    };
    let Some(path) = &skill.path else {
        if args.path.is_some() {
            return Err(Outcome::Refused(format!(
                "refused: the skill {name} is a text: it takes no path, read it with just its name"
            )));
        }
        // The user's own file, which may be a link: read where it leads, which the file tools
        // open without following a link.
        let file = Skill::text_file(data, &project.name, name);
        let file = Resolved::real_inside(name.to_owned(), file.canonicalize().unwrap_or(file));
        return files::read_page(None, &file, offset, length).or_else(|error| {
            Ok(match error.kind() {
                std::io::ErrorKind::NotFound => Outcome::Refused(format!(
                    "refused: the text of the skill {name} is not there any more: the user \
                     writes it again or removes the skill"
                )),
                _ => files::failed(&file, error),
            })
        });
    };
    let root = root_of(project, name, path)?;
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
            "refused: the skill {name} is a file: it takes no path, read it with just its name"
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

/// The skill's real path, which must still be inside the project folder.
fn root_of(project: &Project, name: &str, path: &str) -> Result<PathBuf, Outcome> {
    let folder = project
        .folder
        .canonicalize()
        .map_err(|error| Outcome::Failed(format!("the project folder cannot be read: {error}")))?;
    let root = project
        .folder
        .join(path)
        .canonicalize()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Outcome::Refused(format!(
                "refused: the skill {name} points to {path}, which is not there any more: the \
                 user fixes or removes it (flottille skill)"
            )),
            _ => Outcome::Failed(format!("the skill {name}: {error}")),
        })?;
    if !root.starts_with(&folder) {
        return Err(Outcome::Refused(format!(
            "refused: the skill {name} leads outside the project folder: it is not served"
        )));
    }
    Ok(root)
}

/// `named` inside the skill `root`: taken on the text first (a step out of the skill is
/// outside, the disk is not looked at), then on the disk, every link followed.
fn inside(name: &str, root: &Path, named: &str) -> Result<PathBuf, Outcome> {
    let outside = || {
        Outcome::Refused(format!(
            "refused: {named} is outside the skill {name}: it serves its own files only"
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
                "refused: {named} does not exist in the skill {name}"
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
                "refused: {named} goes through a .git folder, which a skill never serves"
            )));
        }
        if secrets::secret_file(&part) {
            return Err(Outcome::Refused(format!(
                "refused: {named} is a file of secrets, which a skill never serves"
            )));
        }
    }
    Ok(real)
}

/// Whether `fs_read` would hold the real path `target` for the user, by the gate's own rule of
/// sensitive places, taken from the project folder: a skill never serves it, asked or not.
fn sensitive(folder: &Path, target: &Path) -> Option<String> {
    let path = Resolved::real_inside(relative(folder, target), target.to_owned());
    gate::sensitive(&[], &path)
}

fn held_for_the_user(folder: &Path, target: &Path, named: &str) -> Result<(), Outcome> {
    match sensitive(folder, target) {
        Some(place) => Err(Outcome::Refused(format!(
            "refused: {named} is in a sensitive place ({place}), which a skill never serves: \
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
    let file = Resolved::real_inside(shown.to_owned(), path.to_owned());
    if path
        .file_name()
        .is_some_and(|file| secrets::secret_file(&file.to_string_lossy()))
    {
        return Err(Outcome::Refused(format!(
            "refused: the skill {name} is a file of secrets, which a skill never serves"
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
    text.push_str("Read one with skill_read {name, path}.");
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OfProject {
    project: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Named {
    project: String,
    name: String,
}

/// `skill.add` and `skill.edit`: a path of the project, or a text Flottille keeps, never both.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    project: String,
    name: String,
    description: Option<String>,
    path: Option<String>,
    text: Option<String>,
}

/// Answers a `skill.*` request.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "skill.list" => {
            let asked: OfProject = serde_json::from_value(params)?;
            let project = projects::load_project(daemon, &asked.project)?;
            Ok(
                json!({"project": project.name, "skills": project.skills.iter()
                .map(|(name, skill)| to_json(name, skill))
                .collect::<Vec<_>>()}),
            )
        }
        "skill.show" => {
            let asked: Named = serde_json::from_value(params)?;
            let project = projects::load_project(daemon, &asked.project)?;
            let skill = find(&project, &asked.name)?;
            let mut shown = to_json(&asked.name, skill);
            if skill.path.is_none() {
                let file = Skill::text_file(&daemon.paths.data, &project.name, &asked.name);
                shown["text"] = std::fs::read_to_string(&file).unwrap_or_default().into();
            }
            Ok(shown)
        }
        "skill.add" => {
            let asked: Written = serde_json::from_value(params)?;
            let description = asked.description.clone().unwrap_or_default();
            let skill =
                asked_skill(&asked, description.trim())?.ok_or_else(|| Failure::refused(EITHER))?;
            let (name, text) = (asked.name.clone(), asked.text.clone());
            write_skill(daemon, &asked.project, &name, &skill, text, |project| {
                if project.skills.contains_key(&name) {
                    return Err(Failure::refused(format!(
                        "project {} already has a skill named {name}: edit it, or remove it first",
                        project.name
                    )));
                }
                Ok(())
            })?;
            Ok(to_json(&name, &skill))
        }
        "skill.edit" => {
            let asked: Written = serde_json::from_value(params)?;
            let project = projects::load_project(daemon, &asked.project)?;
            let known = find(&project, &asked.name)?.clone();
            let description = asked
                .description
                .clone()
                .unwrap_or_else(|| known.description.clone());
            let skill = asked_skill(&asked, description.trim())?.unwrap_or(Skill {
                description: description.trim().to_owned(),
                path: known.path.clone(),
            });
            let (name, text) = (asked.name.clone(), asked.text.clone());
            write_skill(daemon, &asked.project, &name, &skill, text, |project| {
                find(project, &name).map(|_| ())
            })?;
            // A text that became a path leaves nothing behind.
            if known.path.is_none() && skill.path.is_some() {
                let _ = std::fs::remove_file(Skill::text_file(
                    &daemon.paths.data,
                    &asked.project,
                    &name,
                ));
            }
            Ok(to_json(&name, &skill))
        }
        "skill.remove" => {
            let asked: Named = serde_json::from_value(params)?;
            let name = asked.name.clone();
            let mut was_text = false;
            projects::rewrite(daemon, &asked.project, |project, document| {
                was_text = find(project, &name)?.path.is_none();
                let skills = document
                    .get_mut("skills")
                    .and_then(toml_edit::Item::as_table_like_mut)
                    .ok_or_else(|| not_a_table(&project.name))?;
                skills.remove(&name);
                if skills.is_empty() {
                    document.remove("skills");
                }
                Ok(())
            })?;
            if was_text {
                let _ = std::fs::remove_file(Skill::text_file(
                    &daemon.paths.data,
                    &asked.project,
                    &name,
                ));
            }
            Ok(json!({"project": asked.project, "removed": asked.name}))
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

const EITHER: &str = "a skill is a path of the project or a text, one of them: give path or text";

/// The skill a call writes, checked; `None` when it names neither a path nor a text (an edit of
/// the description alone).
fn asked_skill(asked: &Written, description: &str) -> Result<Option<Skill>, Failure> {
    let path = match (&asked.path, &asked.text) {
        (Some(_), Some(_)) => return Err(Failure::refused(EITHER)),
        (None, None) => return Ok(None),
        (Some(path), None) => Some(match path.trim_end_matches('/') {
            "" => path.clone(),
            trimmed => trimmed.to_owned(),
        }),
        (None, Some(text)) => {
            if text.trim().is_empty() {
                return Err(Failure::refused(format!(
                    "skills.{}.text: must not be empty",
                    asked.name
                )));
            }
            None
        }
    };
    let skill = Skill {
        description: description.to_owned(),
        path,
    };
    check_skill(&asked.name, &skill)
        .map_err(|error| Failure::refused(format!("skills.{}{error}", asked.name)))?;
    Ok(Some(skill))
}

/// Writes the skill under `[skills.<name>]`, its text first when it has one, once `fits` accepts
/// the project as its file is now; the text it had is put back when the file is not written. A
/// path must be there, inside the project folder.
fn write_skill(
    daemon: &Daemon,
    project: &str,
    name: &str,
    skill: &Skill,
    text: Option<String>,
    fits: impl FnOnce(&Project) -> Result<(), Failure>,
) -> Result<(), Failure> {
    let file = Skill::text_file(&daemon.paths.data, project, name);
    let former = std::fs::read_to_string(&file).ok();
    let mut written_text = false;
    let written = projects::rewrite(daemon, project, |project, document| {
        fits(project)?;
        if let Some(path) = &skill.path {
            check_reachable(project, path)?;
        }
        if let Some(text) = &text {
            let unavailable = |error: std::io::Error| {
                Failure::new("unavailable", format!("the text of skill {name}: {error}"))
            };
            if let Some(folder) = file.parent() {
                std::fs::create_dir_all(folder).map_err(unavailable)?;
            }
            crate::providers::write_in_place(&file, text).map_err(unavailable)?;
            written_text = true;
        }
        let mut entry = toml_edit::Table::new();
        entry.insert("description", toml_edit::value(&skill.description));
        if let Some(path) = &skill.path {
            entry.insert("path", toml_edit::value(path));
        }
        let skills = document.entry("skills").or_insert_with(|| {
            let mut skills = toml_edit::Table::new();
            // `[skills.<name>]` alone, without an empty `[skills]` above it.
            skills.set_implicit(true);
            toml_edit::Item::Table(skills)
        });
        let skills = skills
            .as_table_like_mut()
            .ok_or_else(|| not_a_table(&project.name))?;
        // An entry edited keeps the comments the user wrote above it, and its form, inline or not.
        match skills
            .get_mut(name)
            .and_then(toml_edit::Item::as_table_like_mut)
        {
            Some(known) => {
                known.remove("path");
                for (key, value) in entry.iter() {
                    known.insert(key, value.clone());
                }
            }
            None => {
                skills.insert(name, toml_edit::Item::Table(entry));
            }
        }
        Ok(())
    });
    // A text written for a skill the file then refused is not left behind, nor in place of the
    // one the skill had.
    if written.is_err() && written_text {
        match &former {
            Some(former) => {
                let _ = crate::providers::write_in_place(&file, former);
            }
            None => {
                let _ = std::fs::remove_file(&file);
            }
        }
    }
    written.map(|_| ())
}

/// A path skill is a file or a folder that is there now, and that stays in the project folder
/// once the links are followed.
fn check_reachable(project: &Project, path: &str) -> Result<(), Failure> {
    let inside = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::NotFound => {
            Failure::refused(format!("{path} does not exist in the project folder"))
        }
        _ => Failure::refused(format!("cannot read {path}: {error}")),
    };
    let folder = project.folder.canonicalize().map_err(|error| {
        Failure::refused(format!(
            "the folder {} of project {} cannot be read: {error}",
            project.folder.display(),
            project.name
        ))
    })?;
    let real = project.folder.join(path).canonicalize().map_err(inside)?;
    if !real.starts_with(&folder) {
        return Err(Failure::refused(format!(
            "{path} leads outside the project folder: a skill stays inside it"
        )));
    }
    Ok(())
}

fn find<'a>(project: &'a Project, name: &str) -> Result<&'a Skill, Failure> {
    project.skills.get(name).ok_or_else(|| {
        Failure::new(
            "not-found",
            format!("project {} has no skill named {name}", project.name),
        )
    })
}

fn not_a_table(project: &str) -> Failure {
    Failure::new(
        "invalid-config",
        format!("projects/{project}.toml: skills: must be a table"),
    )
}

/// A skill as the protocol shows it: its path, or `text` for one Flottille keeps.
fn to_json(name: &str, skill: &Skill) -> Value {
    let kind = if skill.path.is_some() { "path" } else { "text" };
    let mut shown = json!({"name": name, "description": skill.description, "kind": kind});
    if let Some(path) = &skill.path {
        shown["path"] = path.clone().into();
    }
    shown
}
