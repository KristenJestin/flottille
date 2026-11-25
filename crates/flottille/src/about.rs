//! What the user says of themselves, which every agent reads (`docs/design.md` §8): their name,
//! how to refer to them, and anything else every agent should know (who they are, the language
//! and tone they want). `about.toml` in Flottille's configuration folder, edited in the window, by
//! hand, and through `about.show`, `about.set` and `about.reset`, as the rules are.
//!
//! Like the rules, it is read when an agent is created and frozen in its instructions. Agents read
//! it and never change it.
//!
//! With a hub (`docs/remote.md`, "Settings for all machines"), each field is read on three levels:
//! the machine's own (`about.toml`), else the value of all machines (`crate::shared`, set with
//! `about.share`, which the user's device signs), else nothing. A field written on a machine is
//! its own; written empty, the machine follows all machines' value again.

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

/// The values of all machines this machine last checked, while it has a hub: none without one.
fn all(daemon: &Daemon) -> Option<About> {
    let link = daemon.link()?.ok()?;
    let values = link.control.get()?.shared()?;
    let text = |name: &str| {
        values
            .get(name)
            .map(|kept| kept.value.clone())
            .unwrap_or_default()
    };
    Some(About {
        name: text("about.name"),
        pronouns: text("about.pronouns"),
        notes: text("about.notes"),
    })
}

/// The machine's own fields, else all machines' values: what agents read.
fn merged(own: &About, all: Option<&About>) -> About {
    let pick = |own: &str, all: Option<&str>| match own.trim().is_empty() {
        false => own.to_owned(),
        true => all.unwrap_or_default().to_owned(),
    };
    About {
        name: pick(&own.name, all.map(|all| all.name.as_str())),
        pronouns: pick(&own.pronouns, all.map(|all| all.pronouns.as_str())),
        notes: pick(&own.notes, all.map(|all| all.notes.as_str())),
    }
}

/// The fields this machine holds itself, by their key.
fn own_fields(own: &About) -> Vec<&'static str> {
    [
        ("name", &own.name),
        ("pronouns", &own.pronouns),
        ("notes", &own.notes),
    ]
    .into_iter()
    .filter(|(_, text)| !text.trim().is_empty())
    .map(|(key, _)| key)
    .collect()
}

/// What agents created now are told: what the user wrote, this machine's own field else all
/// machines' value, cut to the limits. A file that cannot be read is nothing, said in the
/// daemon's log: no agent is kept from starting.
pub fn effective(daemon: &Daemon) -> About {
    let all = all(daemon);
    match read(&daemon.paths) {
        Ok(about) => {
            let about = merged(&about, all.as_ref());
            let (about, cuts) = limited(about);
            for cut in cuts {
                eprintln!("flottille daemon: about: {cut}");
            }
            about
        }
        Err(failure) => {
            eprintln!("flottille daemon: about: {}", failure.message);
            limited(merged(&About::default(), all.as_ref())).0
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

/// The name, how to refer to the user and the notes given, each left out or not.
type Given = (Option<String>, Option<String>, Option<String>);

/// The fields of a `Set`, trimmed where they are one line, once each passes its limits.
fn checked(set: Set) -> Result<Given, Failure> {
    let Set {
        name,
        pronouns,
        notes,
    } = set;
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
    Ok((name, pronouns, notes))
}

/// The field `field` of `about.share` (`name`, `pronouns`, `notes`) as a machine keeps it from
/// another: trimmed where it is one line, none past its limits or on more than one line, as
/// `about.set` would refuse it.
pub(crate) fn received(field: &str, text: &str) -> Option<String> {
    let (what, limit, lined) = match field {
        "name" => ("the name", SHORT_LIMIT, true),
        "pronouns" => ("how to refer to you", SHORT_LIMIT, true),
        "notes" => ("what every agent should know", LIMIT, false),
        _ => return None,
    };
    let text = match lined {
        true => text.trim(),
        false => text,
    };
    check(what, text, limit).ok()?;
    if lined {
        one_line(what, text).ok()?;
    }
    Some(text.to_owned())
}

/// Why `about.share` is refused unless the user's device signed it here.
pub(crate) const NOT_SIGNED: &str = "the values of all machines change only as a request signed by the user's \
                          device: change them in the window, on All machines";

/// Answers an `about.*` request: `about.show`, `about.set`, `about.reset`, `about.share`.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    match method {
        "about.show" => {}
        "about.share" => {
            // Checked as `about.set` is, before anything is sealed: a field of no other name.
            checked(serde_json::from_value(params)?)?;
            let asked = crate::daemon::asked()
                .filter(|asked| {
                    serde_json::from_str::<Value>(&asked.signed).is_ok_and(|request| {
                        request["signed"]["method"] == crate::shared::ABOUT_SHARE
                    })
                })
                .ok_or_else(|| Failure::refused(NOT_SIGNED))?;
            let control = daemon
                .link()
                .and_then(Result::ok)
                .and_then(|link| link.control.get().cloned())
                .ok_or_else(|| {
                    Failure::refused(
                        "this machine has no hub: turn remote on and pair it to share a value \
                         with all machines",
                    )
                })?;
            let delivery = control
                .share(crate::shared::ABOUT_SHARE, &asked.signed)
                .map_err(Failure::refused)?;
            announce(daemon)?;
            let mut shown = show(daemon)?;
            shown["hub"] = json!(delivery);
            return Ok(shown);
        }
        "about.set" => {
            let (name, pronouns, notes) = checked(serde_json::from_value(params)?)?;
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

/// `{"name", "pronouns", "notes", "own", "all", "limit", "cut"}`: what agents read (`""` for a
/// field said nowhere), the fields this machine holds itself, all machines' values (`null`
/// without a hub), the notes' limit in characters, and why a text edited by hand past a limit is
/// cut for the agents (`null` when it is not).
fn show(daemon: &Daemon) -> Result<Value, Failure> {
    let own = read(&daemon.paths)?;
    let all = all(daemon);
    let about = merged(&own, all.as_ref());
    let (_, cuts) = limited(about.clone());
    Ok(
        json!({"name": about.name, "pronouns": about.pronouns, "notes": about.notes,
        "own": own_fields(&own),
        "all": all.map(|all| json!({"name": all.name, "pronouns": all.pronouns, "notes": all.notes})),
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

#[cfg(test)]
mod secret_tests {
    use super::*;

    #[test]
    fn about_you_holding_a_token_is_refused_and_never_received() {
        let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        let notes = format!("Push with {token}.");
        let refused = check("what every agent should know", &notes, LIMIT).unwrap_err();
        assert!(refused.contains("secret"), "{refused}");
        assert!(received("notes", &notes).is_none());
        assert!(received("notes", "I review on Fridays.").is_some());
    }
}
