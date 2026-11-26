//! The window's appearance, kept by the daemon so that every window and a restart show the same
//! (`docs/protocol.md`, "The appearance methods"): where its colours come from, the desktop's
//! Omarchy theme or Flottille's own palette, and Flottille's shade. `appearance.toml` in
//! Flottille's configuration folder; absent, Omarchy and dark, which the window turns into
//! Flottille Dark off an Omarchy desktop.

use std::path::PathBuf;

use flottille_core::paths::Paths;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::providers;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Theme {
    #[default]
    Omarchy,
    Flottille,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Shade {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Appearance {
    theme: Theme,
    shade: Shade,
}

/// What `appearance.toml` says this machine chose: a key it leaves out is none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Own {
    theme: Option<Theme>,
    shade: Option<Shade>,
}

/// The name of the shade as a value of all machines (`crate::all_settings`): the theme's colours
/// stay each machine's.
pub const SHARED_SHADE: &str = "appearance.shade";

/// The first line of an `appearance.toml` this version writes: what it holds is what the user
/// chose here. A file of before held the shade whatever the user chose, so its default there is
/// no choice, and all machines' shade applies over it.
const OWN_MARK: &str = "# This machine's own choices: a key left out follows all machines.\n";

/// `$XDG_CONFIG_HOME/flottille/appearance.toml`.
fn config(paths: &Paths) -> PathBuf {
    paths.config.join("appearance.toml")
}

/// All machines' shade as its value says it: `light` or `dark`.
pub fn shared_shade(value: &str) -> Result<(), String> {
    serde_json::from_value::<Shade>(json!(value))
        .map(drop)
        .map_err(|_| "light or dark".to_owned())
}

/// What `appearance.toml` says this machine chose; nothing when there is no file.
fn own(paths: &Paths) -> Result<Own, String> {
    let text = match std::fs::read_to_string(config(paths)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Own::default());
        }
        Err(error) => return Err(format!("appearance.toml: {error}")),
    };
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| format!("appearance.toml: {}", e.message()))?;
    // Each key read as the request would give it, so the file takes the same words.
    let mut table = serde_json::Map::new();
    for key in ["theme", "shade"] {
        if let Some(item) = document.get(key) {
            let word = item
                .as_str()
                .ok_or_else(|| format!("appearance.toml: {key}: it is a word"))?;
            table.insert(key.into(), word.into());
        }
    }
    let mut own: Own = serde_json::from_value(Value::Object(table))
        .map_err(|e| format!("appearance.toml: {e}"))?;
    if !text.starts_with(OWN_MARK) && own.shade == Some(Shade::default()) {
        own.shade = None;
    }
    Ok(own)
}

/// The shade all machines chose, when they chose one.
fn all_shade(paths: &Paths) -> Option<Shade> {
    let value = flottille_core::all_machines::value(paths, SHARED_SHADE)?;
    serde_json::from_value(json!(value)).ok()
}

/// The appearance now: this machine's choice, else all machines' shade, else the default.
fn read(paths: &Paths) -> Result<Appearance, String> {
    let own = own(paths)?;
    Ok(Appearance {
        theme: own.theme.unwrap_or_default(),
        shade: own.shade.or_else(|| all_shade(paths)).unwrap_or_default(),
    })
}

/// Whether this machine chose its own shade.
pub fn own_shade(paths: &Paths) -> Result<bool, String> {
    Ok(own(paths)?.shade.is_some())
}

fn write(paths: &Paths, own: &Own) -> Result<(), String> {
    std::fs::create_dir_all(&paths.config)
        .map_err(|e| format!("{}: {e}", paths.config.display()))?;
    let mut text = OWN_MARK.to_owned();
    if let Some(theme) = own.theme {
        text.push_str(&format!("theme = {}\n", json!(theme)));
    }
    if let Some(shade) = own.shade {
        text.push_str(&format!("shade = {}\n", json!(shade)));
    }
    providers::write_in_place(&config(paths), &text).map_err(|e| format!("appearance.toml: {e}"))
}

/// Drops this machine's own shade: all machines' applies again.
pub fn forget_shade(paths: &Paths) -> Result<(), String> {
    let mut own = own(paths)?;
    if own.shade.is_none() {
        return Ok(());
    }
    own.shade = None;
    write(paths, &own)
}

/// Says the appearance as it is now, `appearance.changed`: after a change of all machines.
pub fn announce(daemon: &Daemon) -> Result<(), String> {
    let appearance = read(&daemon.paths)?;
    daemon
        .emit("appearance.changed", json!(appearance))
        .map(drop)
        .map_err(|error| error.to_string())
}

/// Answers an `appearance.*` request of the user: `appearance.show`, `appearance.set`. No tool of
/// an agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    let appearance = match method {
        "appearance.show" => read(paths).map_err(|e| Failure::new("invalid-config", e))?,
        "appearance.set" => {
            let asked: Own = serde_json::from_value(params)?;
            let invalid = |e| Failure::new("invalid-config", e);
            // A file that cannot be read is written again from the choice, as nothing chosen.
            let mut own = own(paths).unwrap_or_default();
            // What is given is chosen here, even the same as all machines' (#1462): a client
            // sends only what the user changed.
            if asked.theme.is_some() {
                own.theme = asked.theme;
            }
            if asked.shade.is_some() {
                own.shade = asked.shade;
            }
            write(paths, &own).map_err(|e| Failure::new("unavailable", e))?;
            let appearance = read(paths).map_err(invalid)?;
            daemon
                .emit("appearance.changed", json!(appearance))
                .map_err(|error| Failure::new("unavailable", error.to_string()))?;
            appearance
        }
        method => {
            return Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            ));
        }
    };
    Ok(json!(appearance))
}
