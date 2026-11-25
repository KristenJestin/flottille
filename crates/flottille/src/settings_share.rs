//! The settings of all machines that are not About you (#1461, `docs/remote.md`, "Settings for
//! all machines"): the rules for every project, the guidance of the judge, of the compaction and
//! of Review, the limits of long tool results, Specs' language and mode, the automatic
//! subscriptions, Review's passes and the shade. They go the way About you does (`crate::shared`):
//! the user's device signs `settings.share {name, value}` for one machine, whose daemon seals it
//! for every machine and gives the hub the sealed copy.
//!
//! A value is a text. A guidance is its text, `tool_results.limit` a number of characters or
//! `off`, `appearance.shade` a word; the setting of an extension is the JSON text of its value
//! (`"fr-FR"`, `["merged"]`), as `extension.set` takes it. An empty value says nothing: every
//! machine falls back to its default. One value a request, so that a copy holds the text of its
//! own name alone.
//!
//! Each reader of a setting reads three levels, in this order: the project's where one exists,
//! the machine's own (its file), all machines' (`all`), the default. No secret is a setting of
//! all machines: no vault value, token, key or project `[variables]`, and no name that is not
//! listed here is taken.

use flottille_core::paths::Paths;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;

/// The values this method carries by name, besides those of an extension's settings and Review's
/// parts. One a line: the settings added later join in their own lines.
pub const SETTINGS: &[&str] = &[
    "rules",
    "judge.guidance",
    "compaction.guidance",
    "review.guidance",
    "tool_results.limit",
    "tool_results.unchanged",
    "appearance.shade",
];

/// The settings of an extension that are shared: their name, the extension and its key.
pub const EXTENSION_SETTINGS: &[(&str, &str, &str)] = &[
    ("specs.language", "specs", "language"),
    ("specs.mode", "specs", "mode"),
    (
        "autosubscriptions.created_pull",
        "autosubscriptions",
        "created_pull",
    ),
    (
        "autosubscriptions.branch_pull",
        "autosubscriptions",
        "branch_pull",
    ),
    ("autosubscriptions.ticket", "autosubscriptions", "ticket"),
    ("review.passes", "review", "passes"),
];

/// What the guidance of a part of Review is named: `review.guidance.<part>`.
const REVIEW_PART: &str = "review.guidance.";

/// The extension and the key of the shared setting `name`, when it is an extension's.
pub fn extension_of(name: &str) -> Option<(&'static str, &'static str)> {
    EXTENSION_SETTINGS
        .iter()
        .find(|(known, _, _)| *known == name)
        .map(|(_, extension, key)| (*extension, *key))
}

/// The name under which the setting `key` of `extension` is shared, when it is.
pub fn name_of(extension: &str, key: &str) -> Option<&'static str> {
    EXTENSION_SETTINGS
        .iter()
        .find(|(_, own, setting)| *own == extension && *setting == key)
        .map(|(name, _, _)| *name)
}

/// The guidance Review gives its checks, of which each part is shared.
fn review() -> Option<&'static crate::extensions::Guidance> {
    crate::extensions::ALL
        .iter()
        .find(|extension| extension.name == "review")
        .and_then(|extension| extension.guidance.as_ref())
}

/// Whether `name` is a setting of all machines.
pub fn known(name: &str) -> bool {
    SETTINGS.contains(&name)
        || extension_of(name).is_some()
        || name
            .strip_prefix(REVIEW_PART)
            .is_some_and(|part| review().is_some_and(|guidance| guidance.parts.contains(&part)))
}

/// Why `value` is not one the setting `name` takes, as the machine's own setting would refuse it.
pub fn check(name: &str, value: &str) -> Result<(), String> {
    if !known(name) {
        return Err(format!("{name} is not a setting of all machines"));
    }
    // Nothing said: every machine falls back to its default.
    if value.trim().is_empty() {
        return Ok(());
    }
    let words = |wanted: &[&str]| {
        wanted
            .contains(&value)
            .then_some(())
            .ok_or_else(|| format!("{name} is one of {}", wanted.join(", ")))
    };
    match name {
        "rules" => crate::rules::check(value),
        "judge.guidance" => crate::judge::check(value),
        "compaction.guidance" => crate::compaction::check_guidance(value),
        "review.guidance" => text_of_review(value, None),
        "tool_results.limit" => crate::results::check_limit(value),
        "tool_results.unchanged" => words(&["true", "false"]),
        "appearance.shade" => words(&["light", "dark"]),
        _ if name.starts_with(REVIEW_PART) => text_of_review(value, name.strip_prefix(REVIEW_PART)),
        _ => {
            let (extension, key) = extension_of(name).expect("known");
            let value: Value = serde_json::from_str(value)
                .map_err(|_| format!("{name} is the JSON text of its value, such as [\"a\"]"))?;
            crate::extensions::check_setting(extension, key, &value)
        }
    }
}

fn text_of_review(value: &str, part: Option<&str>) -> Result<(), String> {
    let guidance = review().ok_or("Review has no guidance")?;
    let count = value.chars().count();
    match count > guidance.limit {
        true => Err(format!(
            "{} {}is {count} characters long and may hold {}: keep what matters",
            guidance.named,
            part.map(|part| format!("({part}) ")).unwrap_or_default(),
            guidance.limit
        )),
        false => Ok(()),
    }
}

/// The value of all machines `name` this machine last checked: none while it has no hub, empty
/// while nothing is said.
pub fn all(paths: &Paths, name: &str) -> Option<String> {
    if !known(name) {
        return None;
    }
    let values = crate::shared::at(paths)?;
    Some(
        values
            .get(name)
            .map(|kept| kept.value.clone())
            .unwrap_or_default(),
    )
}

/// The value of all machines `name` when it says something, a text that is not blank.
pub fn said_of(paths: &Paths, name: &str) -> Option<String> {
    all(paths, name).filter(|text| !text.trim().is_empty())
}

/// The guidance of all machines `name` a machine with none of its own follows: what is shared,
/// when it is one the machine's own would be taken for.
pub fn guidance(paths: &Paths, name: &str) -> Option<String> {
    said_of(paths, name).filter(|text| check(name, text).is_ok())
}

/// The value of all machines of the setting `key` of `extension`, as its value, when it says one
/// that the setting takes.
pub fn of_extension(paths: &Paths, extension: &str, key: &str) -> Option<Value> {
    let name = name_of(extension, key)?;
    let text = said_of(paths, name)?;
    serde_json::from_str(&text).ok()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Share {
    name: String,
    value: String,
}

/// Answers `settings.share {name, value}`: the user's change of a setting for all machines, saying
/// where it went (`hub`, as `about.share` does). Only as a request the user's device signed (`docs/remote.md`, "Requests from another machine's
/// clients"), on a machine that has a hub.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    if method != "settings.share" {
        return Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        ));
    }
    let Share { name, value } = serde_json::from_value(params)?;
    check(&name, &value).map_err(Failure::refused)?;
    let asked = crate::daemon::asked()
        .filter(|asked| {
            serde_json::from_str::<Value>(&asked.signed)
                .is_ok_and(|request| request["signed"]["method"] == crate::shared::SETTINGS_SHARE)
        })
        .ok_or_else(|| Failure::refused(crate::about::NOT_SIGNED))?;
    let control = daemon
        .link()
        .and_then(Result::ok)
        .and_then(|link| link.control.get().cloned())
        .ok_or_else(|| {
            Failure::refused(
                "this machine has no hub: turn remote on and pair it to share a setting with all \
                 machines",
            )
        })?;
    let delivery = control
        .share(crate::shared::SETTINGS_SHARE, &asked.signed)
        .map_err(Failure::refused)?;
    daemon
        .emit("settings.changed", json!({"names": [name]}))
        .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    Ok(json!({"name": name, "value": value, "hub": delivery}))
}

/// What a change of values of all machines taken from the hub says: `about.changed` for About
/// you's, `settings.changed {names}` for the others', so that a client reads them again.
pub fn said(emit: &crate::link::Emit, changed: &[String]) {
    let (about, settings): (Vec<&String>, Vec<&String>) =
        changed.iter().partition(|name| name.starts_with("about."));
    if !about.is_empty() {
        emit("about.changed", json!({}));
    }
    if !settings.is_empty() {
        emit("settings.changed", json!({"names": settings}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_holding_a_token_is_refused() {
        let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        let refused = check("rules", &format!("Push with {token}.")).unwrap_err();
        assert!(refused.contains("secret"), "{refused}");
        assert!(check("rules", "Push with the vault's entry.").is_ok());
    }

    #[test]
    fn the_hub_keeps_every_setting_this_machine_shares_and_no_other_of_them() {
        let parts = review().expect("Review has a guidance").parts;
        let known_here: Vec<String> = SETTINGS
            .iter()
            .map(|name| name.to_string())
            .chain(
                EXTENSION_SETTINGS
                    .iter()
                    .map(|(name, _, _)| name.to_string()),
            )
            .chain(parts.iter().map(|part| format!("{REVIEW_PART}{part}")))
            .collect();
        for name in &known_here {
            assert!(
                flottille_link::SHARED_NAMES.contains(&name.as_str()),
                "the hub refuses {name}"
            );
        }
        for name in flottille_link::SHARED_NAMES {
            assert!(
                name.starts_with("about.") || known(name),
                "{name} is no setting of this machine"
            );
        }
        assert!(flottille_link::SHARED_NAMES.len() <= flottille_link::MOST_VALUES);
    }
}
