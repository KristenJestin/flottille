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

/// Where the global rules come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    #[default]
    Default,
    Global,
}

impl Source {
    pub fn word(self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::Global => "global",
        }
    }
}

/// The rules an agent of a project works by: the global ones, then the project's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// The user's rules for all projects, else the default: what every project shares. Empty
    /// when the project has `rules_alone`.
    pub shared: String,
    /// Where `shared` comes from (also when the project leaves it out).
    pub source: Source,
    /// The project's own rules, without the part that repeats `shared`.
    pub own: String,
    /// The project gets its own rules alone.
    pub alone: bool,
    /// The project's text starts with the global one, which is left out of `own`.
    pub repeats: bool,
}

/// Why a text was cut to the limit, for each of the two.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cuts {
    pub global: Option<String>,
    pub own: Option<String>,
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

/// The rules of a project's agents: the user's, else the default, then its own. A file that
/// cannot be read is the default, said in the daemon's log: no agent is kept from starting. Rules
/// edited by hand beyond the limit are cut to it, which the daemon's log says.
pub fn effective(paths: &Paths, project: &Project) -> Rules {
    let (rules, cuts) = resolved(paths, project);
    for cut in [cuts.global, cuts.own].into_iter().flatten() {
        eprintln!("flottille daemon: rules: {cut}");
    }
    rules
}

/// `effective`, with why each text was cut when it was.
pub fn resolved(paths: &Paths, project: &Project) -> (Rules, Cuts) {
    let (global, source, global_cut) = users(paths);
    let (stored, own_cut) = own(project);
    let alone = project.rules_alone;
    let rest = if alone {
        None
    } else {
        without(&global, &stored)
    };
    let repeats = rest.is_some();
    let rules = Rules {
        shared: if alone { String::new() } else { global },
        source,
        own: rest.unwrap_or(stored),
        alone,
        repeats,
    };
    (
        rules,
        Cuts {
            global: global_cut,
            own: own_cut,
        },
    )
}

/// The project's own rules as stored, cut to the limit; empty when it has none.
pub fn own(project: &Project) -> (String, Option<String>) {
    match &project.rules {
        Some(text) => limited(
            text,
            &format!(
                "the rules of project {} (the rules key of its file)",
                project.name
            ),
        ),
        None => (String::new(), None),
    }
}

/// What the project's text adds to the global one when it starts with it (a copy made by hand,
/// as the doc once advised); none when it does not.
fn without(global: &str, own: &str) -> Option<String> {
    let global = global.trim();
    let rest = own.trim_start().strip_prefix(global)?;
    (!global.is_empty() && rest.chars().next().is_none_or(char::is_whitespace))
        .then(|| rest.trim().to_owned())
}

/// The rules a project without its own follows: the user's, else the default.
fn users(paths: &Paths) -> (String, Source, Option<String>) {
    match global(paths) {
        Ok(Some(text)) => {
            let (text, cut) = limited(
                &text,
                &format!("your rules for all projects ({})", paths.rules().display()),
            );
            (text, Source::Global, cut)
        }
        Ok(None) => (DEFAULT.to_owned(), Source::Default, None),
        Err(failure) => {
            eprintln!("flottille daemon: rules: {}", failure.message);
            (DEFAULT.to_owned(), Source::Default, None)
        }
    }
}

/// The rules as stored, cut to the limit when a hand edit went past it: `rules.set` refuses such
/// a text, a file edited by hand does not go through it.
fn limited(text: &str, whose: &str) -> (String, Option<String>) {
    let count = text.chars().count();
    if count <= LIMIT {
        return (text.to_owned(), None);
    }
    let cut = format!(
        "{whose} are {count} characters long and may hold {LIMIT}: agents get the first {LIMIT} \
         only, since the rules ride in every agent's instructions; shorten them"
    );
    (text.chars().take(LIMIT).collect(), Some(cut))
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
    /// Left out on a project: its text is kept.
    text: Option<String>,
    /// Whether the project gets its own rules alone; left out: unchanged.
    alone: Option<bool>,
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
            let Set {
                project,
                text,
                alone,
            } = serde_json::from_value(params)?;
            if let Some(text) = &text {
                check(text).map_err(Failure::refused)?;
            }
            match (&project, text) {
                (Some(name), text) => {
                    if text.is_none() && alone.is_none() {
                        return Err(Failure::refused("rules.set needs text or alone"));
                    }
                    set_project(daemon, name, text.as_deref(), alone)?;
                }
                (None, Some(text)) => {
                    if alone.is_some() {
                        return Err(Failure::refused(
                            "alone is a project's: the rules for all projects have no such switch",
                        ));
                    }
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
                (None, None) => return Err(Failure::refused("rules.set needs text")),
            }
            show(daemon, project.as_deref())
        }
        "rules.reset" => {
            let Which { project } = serde_json::from_value(params)?;
            match &project {
                Some(name) => {
                    reset_project(daemon, name)?;
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

/// `{"project"?, "text", "source", "cut", "default", "global", "global_source", "global_cut",
/// "alone", "repeats"}`. For the user's rules, `text` is what every project follows and `source`
/// where it comes from (`default` or `global`). For a project, `text` is its own rules (empty
/// when it has none), `source` is `project` when it has some and else the global source, `cut`
/// says why its text was cut to the limit (`null` when it was not), `alone` that it leaves the
/// global rules out and `repeats` that its text starts with the global one, which is then left out
/// of its agents'. `default` is Flottille's text, `global` the text every project shares.
fn show(daemon: &Daemon, project: Option<&str>) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    // A file that cannot be read is said to the user, not taken for the default.
    global(paths)?;
    let (followed, source, global_cut) = users(paths);
    let mut shown = json!({"project": project, "text": followed, "source": source.word(),
        "cut": global_cut, "default": DEFAULT, "global": followed,
        "global_source": source.word(), "global_cut": global_cut, "alone": false,
        "repeats": false});
    if let Some(name) = project {
        let project = projects::load_project(daemon, name)?;
        let (rules, cuts) = resolved(paths, &project);
        let (stored, _) = own(&project);
        shown["text"] = stored.into();
        shown["source"] = if project.rules.is_some() {
            "project"
        } else {
            source.word()
        }
        .into();
        shown["cut"] = cuts.own.into();
        shown["alone"] = rules.alone.into();
        shown["repeats"] = rules.repeats.into();
    }
    Ok(shown)
}

/// Writes the project's rules in its file, where the keys stand: the text and whether it is
/// alone, each only when given. The file is read back as a project before it is written.
pub fn set_project(
    daemon: &Daemon,
    name: &str,
    text: Option<&str>,
    alone: Option<bool>,
) -> Result<Project, Failure> {
    if let Some(text) = text {
        check(text).map_err(Failure::refused)?;
    }
    projects::rewrite(daemon, name, |_, document| {
        if let Some(text) = text {
            document["rules"] = toml_edit::value(text);
        }
        match alone {
            Some(true) => document["rules_alone"] = toml_edit::value(true),
            Some(false) => {
                document.remove("rules_alone");
            }
            None => {}
        }
        Ok(())
    })
}

/// Removes the project's rules and its `rules_alone`: it gets the global rules again.
pub fn reset_project(daemon: &Daemon, name: &str) -> Result<Project, Failure> {
    projects::rewrite(daemon, name, |_, document| {
        document.remove("rules");
        document.remove("rules_alone");
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

    #[test]
    fn only_a_copy_of_the_whole_global_text_is_left_out() {
        assert_eq!(
            without("Be brief.\n", "  Be brief.\n\nName branches."),
            Some("Name branches.".to_owned())
        );
        assert_eq!(without("Be brief.", "Be brief.\n"), Some(String::new()));
        // A project's sentence that merely begins like the global one is its own.
        assert_eq!(without("Be brief", "Be briefer than that."), None);
        assert_eq!(without("Be brief.", "Name branches."), None);
        assert_eq!(without("  \n", "Name branches."), None);
    }
}
