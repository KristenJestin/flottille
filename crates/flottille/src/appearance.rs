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

/// `$XDG_CONFIG_HOME/flottille/appearance.toml`.
fn config(paths: &Paths) -> PathBuf {
    paths.config.join("appearance.toml")
}

/// What `appearance.toml` says: the default when there is no file.
fn read(paths: &Paths) -> Result<Appearance, String> {
    let text = match std::fs::read_to_string(config(paths)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Appearance::default());
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
    serde_json::from_value(Value::Object(table)).map_err(|e| format!("appearance.toml: {e}"))
}

/// Answers an `appearance.*` request of the user: `appearance.show`, `appearance.set`. No tool of
/// an agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    let appearance = match method {
        "appearance.show" => read(paths).map_err(|e| Failure::new("invalid-config", e))?,
        "appearance.set" => {
            let appearance: Appearance = serde_json::from_value(params)?;
            std::fs::create_dir_all(&paths.config).map_err(|e| {
                Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
            })?;
            let text = format!(
                "theme = {}\nshade = {}\n",
                json!(appearance.theme),
                json!(appearance.shade)
            );
            providers::write_in_place(&config(paths), &text)
                .map_err(|e| Failure::new("unavailable", format!("appearance.toml: {e}")))?;
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
