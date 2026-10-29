//! How agents are told to work (`docs/projects.md`, "Rules"): a default text that ships with
//! Flottille, the user's own for all projects (`rules.md` in Flottille's configuration folder) and
//! one per project (the `rules` key of the project's file), the project's replacing the others.
//! What the gate enforces is not here: it is fixed (`roles/base.md`).
//!
//! The text is read when an agent is created and frozen in its instructions, so a change reaches
//! the agents created after it.

use flottille_core::paths::Paths;
use flottille_core::project::Project;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::{self, Failure};

/// The text agents work by until the user writes another.
pub const DEFAULT: &str = include_str!("../roles/rules.md");

/// The longest rules, in characters: they ride in every session's instructions.
pub const LIMIT: usize = 8_000;

/// Where the rules an agent got come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Default,
    Global,
    Project,
}

impl Source {
    pub fn word(self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::Global => "global",
            Source::Project => "project",
        }
    }
}

/// The rules an agent of a project works by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    pub text: String,
    pub source: Source,
}

/// The user's rules for all projects; none while the file is absent.
pub fn global(paths: &Paths) -> Result<Option<String>, Failure> {
    match std::fs::read_to_string(paths.rules()) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Failure::new(
            "unavailable",
            format!("{}: {error}", paths.rules().display()),
        )),
    }
}

/// The rules of a project's agents: its own, else the user's, else the default. A file that
/// cannot be read is the default, said in the daemon's log: no agent is kept from starting.
pub fn effective(paths: &Paths, project: &Project) -> Rules {
    if let Some(text) = &project.rules {
        return Rules {
            text: text.clone(),
            source: Source::Project,
        };
    }
    match global(paths) {
        Ok(Some(text)) => Rules {
            text,
            source: Source::Global,
        },
        Ok(None) => Rules {
            text: DEFAULT.to_owned(),
            source: Source::Default,
        },
        Err(failure) => {
            eprintln!("flottille daemon: rules: {}", failure.message);
            Rules {
                text: DEFAULT.to_owned(),
                source: Source::Default,
            }
        }
    }
}

/// What is wrong with a text offered as rules.
pub fn check(text: &str) -> Result<(), String> {
    let count = text.chars().count();
    if count > LIMIT {
        return Err(format!(
            "the rules are {count} characters long and may hold {LIMIT}: they ride in every \
             session's instructions, so keep what matters"
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Which {
    project: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    project: Option<String>,
    text: String,
}

/// Answers a `rules.*` request: `rules.show`, `rules.set`, `rules.reset`, for the user's rules
/// (no `project`) or one project's.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "rules.show" => {
            let Which { project } = serde_json::from_value(params)?;
            show(daemon, project.as_deref())
        }
        "rules.set" => {
            let Set { project, text } = serde_json::from_value(params)?;
            check(&text).map_err(Failure::refused)?;
            match &project {
                Some(name) => {
                    set_project(daemon, name, Some(&text))?;
                }
                None => {
                    std::fs::create_dir_all(&daemon.paths.config).map_err(|e| {
                        Failure::new(
                            "unavailable",
                            format!("{}: {e}", daemon.paths.config.display()),
                        )
                    })?;
                    crate::providers::write_in_place(&daemon.paths.rules(), &text)
                        .map_err(|e| Failure::new("unavailable", format!("rules.md: {e}")))?;
                    announce(daemon, None)?;
                }
            }
            show(daemon, project.as_deref())
        }
        "rules.reset" => {
            let Which { project } = serde_json::from_value(params)?;
            match &project {
                Some(name) => {
                    set_project(daemon, name, None)?;
                }
                None => {
                    match std::fs::remove_file(daemon.paths.rules()) {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                            return Err(Failure::new("unavailable", format!("rules.md: {e}")));
                        }
                        _ => {}
                    }
                    announce(daemon, None)?;
                }
            }
            show(daemon, project.as_deref())
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// `{"project"?, "text", "source", "default", "global"}`: the text agents get from now on at that
/// level, where it comes from, Flottille's default, and the text a project without rules of its
/// own follows.
fn show(daemon: &Daemon, project: Option<&str>) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    let global = global(paths)?;
    let followed = Rules {
        text: global.clone().unwrap_or_else(|| DEFAULT.to_owned()),
        source: if global.is_some() {
            Source::Global
        } else {
            Source::Default
        },
    };
    let (rules, name) = match project {
        Some(name) => (
            effective(paths, &projects::load_project(daemon, name)?),
            Some(name),
        ),
        None => (followed.clone(), None),
    };
    Ok(
        json!({"project": name, "text": rules.text, "source": rules.source.word(),
        "default": DEFAULT, "global": followed.text}),
    )
}

/// Writes the project's rules in its file, where the key stands, or removes them (`None`): the
/// project follows the user's again. The file is read back as a project before it is written.
pub fn set_project(daemon: &Daemon, name: &str, text: Option<&str>) -> Result<Project, Failure> {
    if let Some(text) = text {
        check(text).map_err(Failure::refused)?;
    }
    projects::rewrite(daemon, name, |_, document| {
        match text {
            Some(text) => document["rules"] = toml_edit::value(text),
            None => {
                document.remove("rules");
            }
        }
        Ok(())
    })
}

fn announce(daemon: &Daemon, project: Option<&str>) -> Result<(), Failure> {
    daemon
        .emit("rules.changed", json!({"project": project}))
        .map(|_| ())
        .map_err(|error| Failure::new("unavailable", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_rules_name_only_what_agents_are_told_to_use() {
        // They are shown to the user in the window, who edits them: keep them plain text.
        assert!(!DEFAULT.trim().is_empty());
        assert!(DEFAULT.chars().count() < LIMIT);
        assert!(
            !DEFAULT.starts_with('#'),
            "the heading is added around them"
        );
    }

    #[test]
    fn rules_longer_than_the_limit_are_refused_with_the_limit() {
        assert!(check(&"é".repeat(LIMIT)).is_ok());
        let why = check(&"é".repeat(LIMIT + 1)).unwrap_err();
        assert!(why.contains("8000"), "{why}");
    }
}
