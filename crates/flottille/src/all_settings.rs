//! The settings of all machines a machine often overrides (`docs/remote.md`, "Settings for all
//! machines"): new sessions' provider, model and effort, the judge's, compactor's and review's
//! models, Sessions, the workspaces folder when it is a `~/…` template, the check for updates and
//! the `https://` connections. Each is a value of all machines as About you is (`crate::shared`),
//! text under its name, changed by `settings.share`, which the user's device signs, and read by
//! the setting's own loader while this machine chose none. `settings.shared` says what all
//! machines hold, what this machine overrides and what it lacks; `settings.use_all` drops its own.
//!
//! No secret is a value: nothing here takes a vault value, a token, a key or a project's
//! `[variables]`, and a connection that names a vault entry, runs a program or is reached in
//! clear is not shared. Each machine signs in to a shared connection for itself.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use flottille_core::provider::{self, Settings};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::daemon::Daemon;
use crate::extensions;
use crate::projects::Failure;
use crate::{compaction, inactive, providers, updates};

/// The method whose signed request changes these values for all machines.
pub const SHARE: &str = "settings.share";

/// The passes of a review whose model is one of the values, as `review.<pass>_model`.
const PASSES: [&str; 5] = ["spec", "bugs", "standards", "security", "verifier"];

const CONNECTIONS: &str = extensions::SHARED_CONNECTIONS;

/// The names of these values: each is also in `flottille_link::SHARED_NAMES`, which the hub keeps
/// to.
pub const NAMES: [&str; 13] = [
    provider::NEW_SESSION,
    provider::JUDGE,
    compaction::SHARED,
    "review.spec_model",
    "review.bugs_model",
    "review.standards_model",
    "review.security_model",
    "review.verifier_model",
    inactive::SHARED_DAYS,
    inactive::SHARED_RESUME,
    flottille_core::place::SHARED,
    updates::SHARED,
    CONNECTIONS,
];

/// Why `value` cannot be the value `name`. The empty text clears a value, and is always taken.
/// Only what is true on every machine is checked here: what one machine lacks (a model, a
/// provider) is ignored there.
fn check(name: &str, value: &str) -> Result<(), String> {
    if !NAMES.contains(&name) {
        return Err(format!("{name} is no value of all machines"));
    }
    if value.is_empty() {
        return Ok(());
    }
    match name {
        provider::NEW_SESSION => provider::shared_new_session(value).map(drop),
        provider::JUDGE => provider::shared_judge(value).map(drop),
        compaction::SHARED => compaction::shared_compactor(value).map(drop),
        inactive::SHARED_DAYS => inactive::shared_days(value).map(drop),
        inactive::SHARED_RESUME => inactive::shared_resume(value).map(drop),
        updates::SHARED => updates::shared_check(value).map(drop),
        flottille_core::place::SHARED => {
            // A folder this very machine cannot take (inside one of its own) is said where it
            // is read: only the form is every machine's.
            let home = std::path::Path::new("/home/anyone");
            let avoid = flottille_core::place::Avoid {
                config: None,
                data: std::path::Path::new("/data"),
                home: Some(home),
            };
            flottille_core::place::shared(value, &avoid).map(drop)
        }
        CONNECTIONS => extensions::shared_connections(value).map(drop),
        _ => extensions::shared_model(value).map(drop),
    }
    .map_err(|why| format!("{name}: {why}"))
}

/// The value `name` as a machine keeps it from another: none when `settings.share` would refuse
/// it.
pub(crate) fn received(name: &str, value: &str) -> Option<String> {
    check(name, value).ok().map(|()| value.to_owned())
}

/// All machines' values of these settings, by name; none without a hub.
fn all(daemon: &Daemon) -> Option<BTreeMap<String, String>> {
    let link = daemon.link()?.ok()?;
    let values = link.control.get()?.shared()?;
    Some(
        values
            .into_iter()
            .filter(|(name, kept)| NAMES.contains(&name.as_str()) && !kept.value.is_empty())
            .map(|(name, kept)| (name, kept.value))
            .collect(),
    )
}

/// The names this machine holds itself among those all machines have a value of: each of its
/// connections of its own that differs from the shared one is `connections.<name>`. A choice
/// Flottille picked is none of the user's.
fn own(daemon: &Daemon, all: &BTreeMap<String, String>) -> Result<Vec<String>, Failure> {
    let paths = &daemon.paths;
    let invalid = |why| Failure::new("invalid-config", why);
    let providers = Settings::own(paths).map_err(invalid)?;
    let compaction = compaction::Settings::own(paths).map_err(invalid)?;
    let sessions = inactive::own(paths).map_err(invalid)?;
    let mut own = Vec::new();
    for name in all.keys() {
        let holds = match name.as_str() {
            provider::NEW_SESSION => providers.new_session.is_some(),
            provider::JUDGE => {
                providers.judge_set && !providers.judge.as_ref().is_some_and(|judge| judge.picked)
            }
            compaction::SHARED => {
                compaction.compactor_set && !compaction.compactor.as_ref().is_some_and(|c| c.picked)
            }
            inactive::SHARED_DAYS => sessions.days.is_some(),
            inactive::SHARED_RESUME => sessions.resume.is_some(),
            flottille_core::place::SHARED => {
                flottille_core::place::says_a_place(&paths.workspaces_place())
            }
            updates::SHARED => updates::own_check(paths).map_err(invalid)?.is_some(),
            CONNECTIONS => {
                own.extend(
                    extensions::overridden_connections(paths)
                        .map_err(invalid)?
                        .into_iter()
                        .map(|connection| format!("{CONNECTIONS}.{connection}")),
                );
                false
            }
            review => review
                .split_once('.')
                .is_some_and(|(extension, key)| extensions::own_setting(paths, extension, key)),
        };
        if holds {
            own.push(name.clone());
        }
    }
    Ok(own)
}

/// The provider and the model a value names, when it names any.
fn named(name: &str, value: &str) -> Option<(String, Option<String>)> {
    match name {
        provider::NEW_SESSION => provider::shared_new_session(value)
            .ok()
            .map(|chosen| (chosen.provider, chosen.model)),
        provider::JUDGE => provider::shared_judge(value)
            .ok()
            .flatten()
            .map(|judge| (judge.provider, judge.model)),
        compaction::SHARED => compaction::shared_compactor(value)
            .ok()
            .flatten()
            .map(|compactor| (compactor.provider, compactor.model)),
        _ if PASSES
            .iter()
            .any(|pass| name == format!("review.{pass}_model")) =>
        {
            let slot = extensions::shared_model(value).ok()?;
            Some((
                slot["provider"].as_str()?.to_owned(),
                slot["model"].as_str().map(str::to_owned),
            ))
        }
        _ => None,
    }
}

/// `{"all", "own", "ignored"}`: all machines' values (`null` without a hub), the names this
/// machine holds itself, and why each value this machine cannot follow is ignored here.
fn shown(daemon: &Daemon) -> Result<Value, Failure> {
    let Some(all) = all(daemon) else {
        return Ok(json!({"all": null, "own": [], "ignored": {}}));
    };
    let own = own(daemon, &all)?;
    let ignored: Map<String, Value> = all
        .iter()
        .filter(|(name, _)| !own.contains(name))
        .filter_map(|(name, value)| {
            let (provider, model) = named(name, value)?;
            let why = providers::lacks(daemon, &provider, model.as_deref())?;
            Some((name.clone(), json!(format!("{why}: it is ignored here"))))
        })
        .collect();
    Ok(json!({"all": all, "own": own, "ignored": ignored}))
}

/// Why `settings.share` is refused unless the user's device signed it here.
const NOT_SIGNED: &str = "the values of all machines change only as a request signed by the user's \
                          device: change them in the window, on All machines";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Share {
    values: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UseAll {
    names: Vec<String>,
}

/// Answers a `settings.*` request of the user: `settings.shared`, `settings.share`,
/// `settings.use_all`. No tool of an agent reaches them.
pub fn call(daemon: &Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "settings.shared" => shown(daemon),
        SHARE => {
            let Share { values } = serde_json::from_value(params)?;
            if values.is_empty() {
                return Err(Failure::new("invalid-params", "values: none to change"));
            }
            for (name, value) in &values {
                check(name, value).map_err(Failure::refused)?;
            }
            let asked = crate::daemon::asked()
                .filter(|asked| {
                    serde_json::from_str::<Value>(&asked.signed)
                        .is_ok_and(|request| request["signed"]["method"] == SHARE)
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
            let delivery = control.share(&asked.signed).map_err(Failure::refused)?;
            applied(daemon, &values.into_keys().collect::<Vec<_>>());
            let mut answer = shown(daemon)?;
            answer["hub"] = json!(delivery);
            Ok(answer)
        }
        "settings.use_all" => {
            let UseAll { names } = serde_json::from_value(params)?;
            for name in &names {
                use_all(daemon, name)?;
            }
            let changed: Vec<String> = names
                .iter()
                .map(|name| match name.split_once('.') {
                    Some((CONNECTIONS, _)) => CONNECTIONS.to_owned(),
                    _ => name.clone(),
                })
                .collect();
            applied(daemon, &changed);
            shown(daemon)
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

/// Drops this machine's own choice of `name`: it follows all machines again.
fn use_all(daemon: &Arc<Daemon>, name: &str) -> Result<(), Failure> {
    let paths = &daemon.paths;
    let unavailable = |why| Failure::new("unavailable", why);
    if let Some((CONNECTIONS, connection)) = name.split_once('.') {
        return extensions::drop_own_connection(daemon, connection);
    }
    if !NAMES.contains(&name) || name == CONNECTIONS {
        return Err(Failure::new(
            "invalid-params",
            format!("{name} is no setting this machine overrides"),
        ));
    }
    match name {
        provider::NEW_SESSION => providers::forget_new_session(paths),
        provider::JUDGE => providers::forget_judge(paths),
        compaction::SHARED => compaction::forget_compactor(paths),
        inactive::SHARED_DAYS | inactive::SHARED_RESUME => {
            let mut own = inactive::own(paths).map_err(unavailable)?;
            match name {
                inactive::SHARED_DAYS => own.days = None,
                _ => own.resume = None,
            }
            inactive::write(paths, own).map_err(unavailable)
        }
        flottille_core::place::SHARED => match std::fs::remove_file(paths.workspaces_place()) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(unavailable(format!("workspaces.toml: {error}")))
            }
            _ => Ok(()),
        },
        updates::SHARED => updates::forget_check(paths).map_err(unavailable),
        review => match review.split_once('.') {
            Some((extension, key)) => extensions::forget_setting(daemon, extension, key),
            None => Ok(()),
        },
    }
}

/// Says what changed of the values of all machines, `settings.shared_changed` and each setting
/// as its own change is said, and gives the connections of all machines to agents: after a change
/// of all machines made here or taken from the hub, or a machine's own choice dropped.
///
/// The judge is not said as `provider.judge_changed`: that one is what this machine chose, which
/// is remembered (`providers::pick_judge`).
pub fn applied(daemon: &Arc<Daemon>, names: &[String]) {
    if names.is_empty() {
        return;
    }
    if let Err(error) = daemon.emit("settings.shared_changed", json!({"names": names})) {
        eprintln!("flottille daemon: settings.shared_changed was not stored: {error}");
    }
    let mut said = BTreeSet::new();
    for name in names {
        let event = match name.as_str() {
            provider::NEW_SESSION => "provider.new_session_changed",
            compaction::SHARED => "compaction",
            inactive::SHARED_DAYS | inactive::SHARED_RESUME => "sessions.changed",
            flottille_core::place::SHARED => "workspaces_folder.changed",
            updates::SHARED => "update",
            CONNECTIONS => "connections",
            name if name.starts_with("review.") => "extension.changed",
            name if name.starts_with("about.") => "about.changed",
            _ => continue,
        };
        said.insert(event);
    }
    for event in said {
        if let Err(error) = say(daemon, event) {
            eprintln!(
                "flottille daemon: a change of all machines could not be said: {}",
                error.message
            );
        }
    }
}

fn say(daemon: &Arc<Daemon>, event: &str) -> Result<(), Failure> {
    let emit = |name: &str, data: Value| {
        daemon
            .emit(name, data)
            .map(drop)
            .map_err(|error| Failure::new("unavailable", error.to_string()))
    };
    let invalid = |why| Failure::new("invalid-config", why);
    match event {
        "provider.new_session_changed" => {
            let settings = Settings::load(&daemon.paths).map_err(invalid)?;
            emit(event, json!({"new_session": settings.new_session}))
        }
        "compaction" => compaction::announce(daemon),
        "sessions.changed" => {
            let shown = inactive::call(daemon, "sessions.show", json!({}))?;
            emit(event, shown)
        }
        "workspaces_folder.changed" => emit(event, json!({"project": null})),
        "update" => {
            updates::changed(daemon);
            Ok(())
        }
        "connections" => {
            extensions::shared_connections_changed(daemon);
            Ok(())
        }
        "extension.changed" => emit(event, json!({"name": "review", "project": null})),
        event => emit(event, json!({})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hub_keeps_every_name_of_these_values() {
        for name in NAMES {
            assert!(
                flottille_link::SHARED_NAMES.contains(&name),
                "{name} is not in SHARED_NAMES"
            );
        }
    }
}
