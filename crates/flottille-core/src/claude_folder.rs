//! A project's `.claude` folder in the Anthropic layout (`docs/skills.md`, `docs/commands.md`,
//! "Importing a `.claude` folder"): the skills `.claude/skills/<name>/SKILL.md` and the commands
//! `.claude/commands/<name>.md`, each a Markdown file that may start with YAML front matter.
//!
//! This module only reads: it finds what could be imported, and why a file cannot be. Nothing is
//! ever written in the project's folder, and a file that is not whole (no name, unreadable, out of
//! the folder) is shown with its reason and never imported half.

use std::path::Path;

use serde::Deserialize;

use crate::commands::Command;
use crate::project::{Skill, check_skill, valid_name};

/// The skills of the folder, relative to the project folder.
pub const SKILLS: &str = ".claude/skills";

/// The commands of the folder, relative to the project folder.
pub const COMMANDS: &str = ".claude/commands";

/// The largest file read: a skill or a command is a text for an agent, not a data file.
const FILE_MAX: u64 = 1024 * 1024;

/// The longest description made from a command's first line.
const DESCRIPTION_MAX: usize = 100;

/// One file of the `.claude` folder that holds, or should hold, a skill or a command.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// What the project would call it; none when the file does not say.
    pub name: Option<String>,
    /// The file, relative to the project folder, with `/`.
    pub path: String,
    /// The one line, none when the file cannot be imported for lack of it.
    pub description: Option<String>,
    /// Why the file cannot be imported, none when it can.
    pub problem: Option<String>,
}

impl Entry {
    fn unusable(name: Option<String>, path: String, problem: impl Into<String>) -> Entry {
        Entry {
            name,
            path,
            description: None,
            problem: Some(problem.into()),
        }
    }
}

/// The text of a file before and after its front matter.
#[derive(Debug, PartialEq)]
pub struct Split<'a> {
    /// The YAML between the two `---` lines; none when the text does not start with them.
    pub front: Option<&'a str>,
    /// What follows the front matter, or the whole text without one.
    pub body: &'a str,
}

/// Splits `text` at its front matter: a first line `---`, the YAML, and a line `---` (or `...`)
/// that ends it. Refused when it is opened and never closed.
pub fn split(text: &str) -> Result<Split<'_>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return Ok(Split {
            front: None,
            body: text,
        });
    }
    let start = text.find('\n').map_or(text.len(), |at| at + 1);
    let mut at = start;
    for line in lines {
        if matches!(line.trim_end(), "---" | "...") {
            return Ok(Split {
                front: Some(&text[start..at]),
                body: &text[at + line.len()..],
            });
        }
        at += line.len();
    }
    Err("its front matter is never closed: a line with --- ends it".into())
}

/// The keys of the front matter that Flottille reads; the others (`allowed-tools`, `model`…) are
/// the other tool's own and are left alone.
#[derive(Default, Deserialize)]
struct Fields {
    name: Option<String>,
    description: Option<String>,
}

fn fields(front: &str) -> Result<Fields, String> {
    if front.trim().is_empty() {
        return Ok(Fields::default());
    }
    serde_saphyr::from_str::<Fields>(front).map_err(|error| {
        let error = error.to_string();
        let first = error.lines().next().unwrap_or_default();
        format!("its front matter is not valid YAML: {first}")
    })
}

/// The words of `text` on one line: a description is one.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The file `relative` to `folder`, as text. Every link is followed and the real file must be in
/// the project folder; the reason it cannot be read otherwise.
pub fn read_text(folder: &Path, relative: &str) -> Result<String, String> {
    let root = folder
        .canonicalize()
        .map_err(|error| format!("the project folder cannot be read: {error}"))?;
    let real = folder
        .join(relative)
        .canonicalize()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => "it is not there".to_owned(),
            _ => format!("it cannot be read: {error}"),
        })?;
    if !real.starts_with(&root) {
        return Err("it leads outside the project folder".into());
    }
    let metadata =
        std::fs::metadata(&real).map_err(|error| format!("it cannot be read: {error}"))?;
    if !metadata.is_file() {
        return Err("it is not a file".into());
    }
    if metadata.len() > FILE_MAX {
        return Err("it is larger than 1 MiB".into());
    }
    let bytes = std::fs::read(&real).map_err(|error| format!("it cannot be read: {error}"))?;
    String::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_owned())
}

/// What a command file sends: its text after the front matter, trimmed. This is what a file-backed
/// command reads each time it is used, so that a changed file is read changed.
pub fn command_text(folder: &Path, relative: &str) -> Result<String, String> {
    let text = read_text(folder, relative)?;
    let split = split(&text)?;
    let body = split.body.trim();
    if body.is_empty() {
        return Err("it has no instructions: nothing follows its front matter".into());
    }
    Ok(body.to_owned())
}

/// The names of a folder's entries, sorted; none when it is not there. A name that is not text
/// is left out.
fn names(folder: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// The skills of `.claude/skills`: a folder each, with a `SKILL.md` whose front matter gives the
/// skill's `name` and `description`. By path.
pub fn skills(folder: &Path) -> Vec<Entry> {
    let mut found = Vec::new();
    for directory in names(&folder.join(SKILLS)) {
        if !folder.join(SKILLS).join(&directory).is_dir() {
            continue;
        }
        let path = format!("{SKILLS}/{directory}/SKILL.md");
        found.push(skill(folder, path));
    }
    named_once(&mut found, "skill");
    found
}

fn skill(folder: &Path, path: String) -> Entry {
    if !folder.join(&path).exists() && std::fs::symlink_metadata(folder.join(&path)).is_err() {
        return Entry::unusable(None, path, "the folder has no SKILL.md");
    }
    let text = match read_text(folder, &path) {
        Ok(text) => text,
        Err(why) => return Entry::unusable(None, path, why),
    };
    let split = match split(&text) {
        Ok(split) => split,
        Err(why) => return Entry::unusable(None, path, why),
    };
    let Some(front) = split.front else {
        return Entry::unusable(
            None,
            path,
            "it has no front matter: a skill file starts with --- lines giving its name and \
             description",
        );
    };
    let fields = match fields(front) {
        Ok(fields) => fields,
        Err(why) => return Entry::unusable(None, path, why),
    };
    let name = fields.name.map(|name| name.trim().to_owned());
    let Some(name) = name.filter(|name| !name.is_empty()) else {
        return Entry::unusable(None, path, "its front matter has no name");
    };
    if !valid_name(&name) {
        return Entry::unusable(
            Some(name.clone()),
            path,
            format!(
                "its name “{name}” cannot be used: 1 to 64 letters, digits, dots, dashes or \
                 underscores, starting with a letter or a digit"
            ),
        );
    }
    let description = one_line(&fields.description.unwrap_or_default());
    if description.is_empty() {
        return Entry::unusable(Some(name), path, "its front matter has no description");
    }
    let candidate = Skill {
        description: description.clone(),
        path: Some(path.clone()),
    };
    if let Err(why) = check_skill(&name, &candidate) {
        return Entry::unusable(Some(name), path, format!("it cannot be used{why}"));
    }
    Entry {
        name: Some(name),
        path,
        description: Some(description),
        problem: None,
    }
}

/// The commands of `.claude/commands`: a Markdown file each, named after the file, whose text
/// after the optional front matter is the instructions. By path.
pub fn commands(folder: &Path) -> Vec<Entry> {
    let mut found = Vec::new();
    for file in names(&folder.join(COMMANDS)) {
        let Some(stem) = file.strip_suffix(".md") else {
            continue;
        };
        if !folder.join(COMMANDS).join(&file).is_file() {
            continue;
        }
        let path = format!("{COMMANDS}/{file}");
        found.push(command(folder, stem, path));
    }
    named_once(&mut found, "command");
    found
}

fn command(folder: &Path, name: &str, path: String) -> Entry {
    if !valid_name(name) {
        return Entry::unusable(
            None,
            path,
            format!(
                "the name “{name}” of the file cannot be used: 1 to 64 letters, digits, dots, \
                 dashes or underscores, starting with a letter or a digit"
            ),
        );
    }
    let text = match read_text(folder, &path) {
        Ok(text) => text,
        Err(why) => return Entry::unusable(Some(name.to_owned()), path, why),
    };
    let split = match split(&text) {
        Ok(split) => split,
        Err(why) => return Entry::unusable(Some(name.to_owned()), path, why),
    };
    let described = match split.front.map(fields).transpose() {
        Ok(fields) => fields.and_then(|fields| fields.description),
        Err(why) => return Entry::unusable(Some(name.to_owned()), path, why),
    };
    let body = split.body.trim();
    if body.is_empty() {
        return Entry::unusable(
            Some(name.to_owned()),
            path,
            "it has no instructions: nothing follows its front matter",
        );
    }
    let description = described
        .map(|description| one_line(&description))
        .filter(|description| !description.is_empty())
        .unwrap_or_else(|| first_line(body));
    let candidate = Command::file(&description, &path);
    if let Err(why) = candidate.check(name) {
        return Entry::unusable(
            Some(name.to_owned()),
            path,
            format!("it cannot be used: {why}"),
        );
    }
    Entry {
        name: Some(name.to_owned()),
        path,
        description: Some(description),
        problem: None,
    }
}

/// A command with no description is told by its first line, a heading's marks dropped.
fn first_line(body: &str) -> String {
    let line = body
        .lines()
        .map(|line| one_line(line.trim_start_matches(['#', ' ', '\t'])))
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= DESCRIPTION_MAX {
        return line;
    }
    let cut: String = line.chars().take(DESCRIPTION_MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

/// A name two files give is given to the first only: the others say so.
fn named_once(found: &mut [Entry], kind: &str) {
    let mut seen: Vec<String> = Vec::new();
    for entry in found.iter_mut() {
        let (Some(name), None) = (&entry.name, &entry.problem) else {
            continue;
        };
        if seen.contains(name) {
            entry.problem = Some(format!("another {kind} file already has the name {name}"));
            entry.description = None;
        } else {
            seen.push(name.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write(folder: &Path, relative: &str, text: &str) {
        let file = folder.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    #[test]
    fn a_text_without_front_matter_is_all_body() {
        assert_eq!(
            split("# Title\n---\nnot front matter\n").unwrap(),
            Split {
                front: None,
                body: "# Title\n---\nnot front matter\n"
            }
        );
        assert_eq!(split("").unwrap().front, None);
    }

    #[test]
    fn front_matter_ends_at_the_next_dashes_and_the_rest_is_the_body() {
        let text = "---\nname: x\ndescription: y\n---\nBody\n---\nmore\n";
        let split = split(text).unwrap();
        assert_eq!(split.front, Some("name: x\ndescription: y\n"));
        assert_eq!(split.body, "Body\n---\nmore\n");
        let text = "---\r\nname: x\r\n...\r\nBody\r\n";
        let split = super::split(text).unwrap();
        assert_eq!(split.front, Some("name: x\r\n"));
        assert_eq!(split.body, "Body\r\n");
        assert_eq!(super::split("---\n---\nB").unwrap().front, Some(""));
        assert_eq!(super::split("\u{feff}---\na: b\n---\nB").unwrap().body, "B");
    }

    #[test]
    fn front_matter_that_is_never_closed_is_refused() {
        assert!(
            split("---\nname: x\nBody\n")
                .unwrap_err()
                .contains("never closed")
        );
        assert!(split("---").unwrap_err().contains("never closed"));
    }

    #[test]
    fn a_description_over_several_lines_is_one_and_other_keys_are_left_alone() {
        let folder = temp();
        write(
            folder.path(),
            ".claude/skills/react/SKILL.md",
            "---\nname: react\ndescription: >\n  How we write\n  React components\n\
             allowed-tools: [Read, Grep]\nmetadata:\n  version: 2\n---\nText\n",
        );
        let found = skills(folder.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].problem, None);
        assert_eq!(
            found[0].description.as_deref(),
            Some("How we write React components")
        );
    }

    #[test]
    fn a_name_given_by_two_files_goes_to_the_first_by_path() {
        let folder = temp();
        for directory in ["a", "b"] {
            write(
                folder.path(),
                &format!(".claude/skills/{directory}/SKILL.md"),
                "---\nname: same\ndescription: d\n---\nText\n",
            );
        }
        let found = skills(folder.path());
        assert_eq!(found[0].problem, None);
        assert!(
            found[1]
                .problem
                .as_deref()
                .unwrap()
                .contains("already has the name same")
        );
    }

    #[test]
    fn a_long_first_line_is_cut_for_the_description_of_a_command() {
        let folder = temp();
        write(
            folder.path(),
            ".claude/commands/long.md",
            &format!("## {}\nmore\n", "word ".repeat(40)),
        );
        let found = commands(folder.path());
        let description = found[0].description.as_deref().unwrap();
        assert_eq!(description.chars().count(), DESCRIPTION_MAX);
        assert!(description.ends_with('…'));
        assert!(description.starts_with("word word"));
    }

    #[test]
    fn a_command_text_is_what_follows_the_front_matter_read_now() {
        let folder = temp();
        write(
            folder.path(),
            ".claude/commands/go.md",
            "---\ndescription: d\n---\n\nDo it.\n\n",
        );
        assert_eq!(
            command_text(folder.path(), ".claude/commands/go.md").unwrap(),
            "Do it."
        );
        write(
            folder.path(),
            ".claude/commands/go.md",
            "Do it differently.\n",
        );
        assert_eq!(
            command_text(folder.path(), ".claude/commands/go.md").unwrap(),
            "Do it differently."
        );
        std::fs::remove_file(folder.path().join(".claude/commands/go.md")).unwrap();
        assert_eq!(
            command_text(folder.path(), ".claude/commands/go.md").unwrap_err(),
            "it is not there"
        );
    }
}
