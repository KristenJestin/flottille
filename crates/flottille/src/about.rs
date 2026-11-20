//! What the user says of themselves, which every agent reads (`docs/design.md` §8): their name,
//! how to refer to them, and anything else every agent should know (who they are, the language
//! and tone they want). `about.toml` in Flottille's configuration folder, edited in the window, by
//! hand, and through `about.show`, `about.set` and `about.reset`, as the rules are.
//!
//! Like the rules, it is read when an agent is created and frozen in its instructions. Agents read
//! it and never change it.

use flottille_core::paths::Paths;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;

/// The longest notes, in characters: they ride in every session's instructions, as the rules do.
pub const LIMIT: usize = 8_000;

/// The longest name, or way to refer to the user, in characters.
pub const SHORT_LIMIT: usize = 100;

/// What the user says of themselves; every field may be empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct About {
    /// What agents call the user.
    pub name: String,
    /// How to refer to the user, on one line ("he/him", "she/her", "they").
    pub pronouns: String,
    /// Anything every agent should know, in free text.
    pub notes: String,
}

impl About {
    /// Nothing is said: agents are told nothing of the user.
    pub fn is_empty(&self) -> bool {
        self.name.trim().is_empty()
            && self.pronouns.trim().is_empty()
            && self.notes.trim().is_empty()
    }
}

/// What the user wrote, or nothing while there is no file. A file that is not TOML, or whose keys
/// are not text, is an error that says so.
pub fn read(paths: &Paths) -> Result<About, Failure> {
    let file = paths.about();
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(About::default());
        }
        Err(error) => {
            return Err(Failure::new("unavailable", format!("about.toml: {error}")));
        }
    };
    let invalid = |why: String| Failure::new("invalid-config", format!("about.toml: {why}"));
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| invalid(e.message().to_owned()))?;
    let key = |name: &str| -> Result<String, Failure> {
        match document.get(name) {
            None => Ok(String::new()),
            Some(value) => value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(format!("{name}: it is text"))),
        }
    };
    Ok(About {
        name: key("name")?,
        pronouns: key("pronouns")?,
        notes: key("notes")?,
    })
}

/// What agents created now are told: what the user wrote, cut to the limits. A file that cannot be
/// read is nothing, said in the daemon's log: no agent is kept from starting.
pub fn effective(paths: &Paths) -> About {
    match read(paths) {
        Ok(about) => {
            let (about, cuts) = limited(about);
            for cut in cuts {
                eprintln!("flottille daemon: about: {cut}");
            }
            about
        }
        Err(failure) => {
            eprintln!("flottille daemon: about: {}", failure.message);
            About::default()
        }
    }
}

/// The text cut to `limit` characters, and why when it was.
fn cut(text: String, limit: usize, what: &str) -> (String, Option<String>) {
    let count = text.chars().count();
    if count <= limit {
        return (text, None);
    }
    (
        text.chars().take(limit).collect(),
        Some(format!(
            "{what} is {count} characters long and may hold {limit}: agents get the first {limit} \
             only; shorten it"
        )),
    )
}

/// `about` cut to the limits, as a hand edit may have gone past them, and why each cut was made.
fn limited(about: About) -> (About, Vec<String>) {
    let mut cuts = Vec::new();
    let mut field = |text: String, limit: usize, what: &str| {
        let (text, why) = cut(text, limit, what);
        cuts.extend(why);
        text
    };
    let about = About {
        name: field(about.name, SHORT_LIMIT, "the name"),
        pronouns: field(about.pronouns, SHORT_LIMIT, "how to refer to you"),
        notes: field(about.notes, LIMIT, "what every agent should know"),
    };
    (about, cuts)
}

/// What is wrong with a field offered: too long, or a name that is not one line.
fn check(name: &str, text: &str, limit: usize) -> Result<(), String> {
    let count = text.chars().count();
    if count > limit {
        return Err(format!(
            "{name} is {count} characters long and may hold {limit}: it rides in every session's \
             instructions, so keep what matters"
        ));
    }
    Ok(())
}

fn one_line(name: &str, text: &str) -> Result<(), String> {
    if text.contains(['\n', '\r']) {
        return Err(format!("{name} is one line"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    /// A field left out is kept; an empty one is cleared.
    name: Option<String>,
    pronouns: Option<String>,
    notes: Option<String>,
}

/// Answers an `about.*` request: `about.show`, `about.set`, `about.reset`.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    match method {
        "about.show" => {}
        "about.set" => {
            let Set {
                name,
                pronouns,
                notes,
            } = serde_json::from_value(params)?;
            let name = name.map(|text| text.trim().to_owned());
            let pronouns = pronouns.map(|text| text.trim().to_owned());
            for (field, text, limit, lined) in [
                ("the name", &name, SHORT_LIMIT, true),
                ("how to refer to you", &pronouns, SHORT_LIMIT, true),
                ("what every agent should know", &notes, LIMIT, false),
            ] {
                let Some(text) = text else { continue };
                check(field, text, limit).map_err(Failure::refused)?;
                if lined {
                    one_line(field, text).map_err(Failure::refused)?;
                }
            }
            // What is around the keys the user wrote by hand (their comments) is kept.
            let mut document = match std::fs::read_to_string(paths.about()) {
                Ok(text) => text.parse::<toml_edit::DocumentMut>().map_err(|e| {
                    Failure::new("invalid-config", format!("about.toml: {}", e.message()))
                })?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
                Err(e) => return Err(Failure::new("unavailable", format!("about.toml: {e}"))),
            };
            for (key, text) in [("name", name), ("pronouns", pronouns), ("notes", notes)] {
                match text {
                    Some(text) if !text.trim().is_empty() => document[key] = toml_edit::value(text),
                    Some(_) => {
                        document.remove(key);
                    }
                    None => {}
                }
            }
            if ["name", "pronouns", "notes"]
                .iter()
                .all(|key| document.get(key).is_none())
            {
                forget(daemon)?;
            } else {
                std::fs::create_dir_all(&paths.config).map_err(|e| {
                    Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
                })?;
                crate::providers::write_in_place(&paths.about(), &document.to_string())
                    .map_err(|e| Failure::new("unavailable", format!("about.toml: {e}")))?;
            }
            announce(daemon)?;
        }
        "about.reset" => {
            forget(daemon)?;
            announce(daemon)?;
        }
        method => {
            return Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            ));
        }
    }
    show(daemon)
}

/// Removes `about.toml`.
fn forget(daemon: &Daemon) -> Result<(), Failure> {
    match std::fs::remove_file(daemon.paths.about()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(Failure::new("unavailable", format!("about.toml: {e}")))
        }
        _ => Ok(()),
    }
}

fn announce(daemon: &Daemon) -> Result<(), Failure> {
    daemon
        .emit("about.changed", json!({}))
        .map(|_| ())
        .map_err(|error| Failure::new("unavailable", error.to_string()))
}

/// `{"name", "pronouns", "notes", "limit", "cut"}`: what the user wrote (`""` for a field they
/// did not), the notes' limit in characters, and why a text edited by hand past a limit is cut
/// for the agents (`null` when it is not).
fn show(daemon: &Daemon) -> Result<Value, Failure> {
    let about = read(&daemon.paths)?;
    let (_, cuts) = limited(about.clone());
    Ok(
        json!({"name": about.name, "pronouns": about.pronouns, "notes": about.notes,
        "limit": LIMIT, "cut": (!cuts.is_empty()).then(|| cuts.join("; "))}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanks_say_nothing() {
        assert!(About::default().is_empty());
        assert!(
            About {
                name: " ".into(),
                pronouns: String::new(),
                notes: "\n".into()
            }
            .is_empty()
        );
        assert!(
            !About {
                pronouns: "they".into(),
                ..About::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn a_text_past_its_limit_is_cut_and_says_why() {
        let (about, cuts) = limited(About {
            name: "é".repeat(SHORT_LIMIT + 1),
            notes: "é".repeat(LIMIT),
            ..About::default()
        });
        assert_eq!(about.name.chars().count(), SHORT_LIMIT);
        assert_eq!(about.notes.chars().count(), LIMIT);
        assert_eq!(cuts.len(), 1, "{cuts:?}");
    }
}
