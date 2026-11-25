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

/// What the window shows: the theme and the shade, as this machine reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
struct Appearance {
    theme: Theme,
    shade: Shade,
}

/// What `appearance.toml` says, key by key: a key it leaves out follows all machines' value, else
/// the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Own {
    theme: Option<Theme>,
    shade: Option<Shade>,
}

/// `$XDG_CONFIG_HOME/flottille/appearance.toml`.
fn config(paths: &Paths) -> PathBuf {
    paths.config.join("appearance.toml")
}

/// What `appearance.toml` says: nothing for a key it leaves out, or when there is no file.
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
    serde_json::from_value(Value::Object(table)).map_err(|e| format!("appearance.toml: {e}"))
}

/// The shade of all machines, when this machine has a hub that says one.
fn all(paths: &Paths) -> Option<Shade> {
    let word = crate::settings_share::said_of(paths, SHADE_NAME)?;
    serde_json::from_value(json!(word)).ok()
}

/// The name of the shade among the values of all machines.
const SHADE_NAME: &str = "appearance.shade";

/// What this machine shows: its own theme and shade, the shade of all machines, the default.
fn read(paths: &Paths) -> Result<Appearance, String> {
    let own = own(paths)?;
    Ok(Appearance {
        theme: own.theme.unwrap_or_default(),
        shade: own.shade.or_else(|| all(paths)).unwrap_or_default(),
    })
}

/// What the window is told: the theme and the shade it shows, the keys this machine holds itself
/// (`own`), and the values of all machines (`all`, `null` while it has no hub).
fn said(paths: &Paths, appearance: Appearance) -> Value {
    let own = own(paths).unwrap_or_default();
    let held: Vec<&str> = [
        ("theme", own.theme.is_some()),
        ("shade", own.shade.is_some()),
    ]
    .into_iter()
    .filter(|(_, held)| *held)
    .map(|(key, _)| key)
    .collect();
    let all = crate::shared::at(paths).map(|_| match all(paths) {
        Some(shade) => json!({"shade": shade}),
        None => json!({}),
    });
    json!({"theme": appearance.theme, "shade": appearance.shade, "own": held, "all": all})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    theme: Option<Theme>,
    shade: Option<Shade>,
    /// The keys to drop: they follow all machines again (or the default).
    #[serde(default)]
    reset: Vec<String>,
}

/// Answers an `appearance.*` request of the user: `appearance.show`, `appearance.set`. No tool of
/// an agent reaches them.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    let appearance = match method {
        "appearance.show" => read(paths).map_err(|e| Failure::new("invalid-config", e))?,
        "appearance.set" => {
            let Set {
                theme,
                shade,
                reset,
            } = serde_json::from_value(params)?;
            let mut wanted = own(paths).unwrap_or_default();
            wanted.theme = theme.or(wanted.theme);
            wanted.shade = shade.or(wanted.shade);
            for key in &reset {
                match key.as_str() {
                    "theme" => wanted.theme = None,
                    "shade" => wanted.shade = None,
                    other => {
                        return Err(Failure::new(
                            "invalid-params",
                            format!("{other} is not one of theme, shade"),
                        ));
                    }
                }
            }
            let mut text = String::new();
            if let Some(theme) = wanted.theme {
                text.push_str(&format!("theme = {}\n", json!(theme)));
            }
            if let Some(shade) = wanted.shade {
                text.push_str(&format!("shade = {}\n", json!(shade)));
            }
            if text.is_empty() {
                match std::fs::remove_file(config(paths)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        return Err(Failure::new("unavailable", format!("appearance.toml: {e}")));
                    }
                    _ => {}
                }
            } else {
                std::fs::create_dir_all(&paths.config).map_err(|e| {
                    Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
                })?;
                providers::write_in_place(&config(paths), &text)
                    .map_err(|e| Failure::new("unavailable", format!("appearance.toml: {e}")))?;
            }
            let appearance = read(paths).map_err(|e| Failure::new("invalid-config", e))?;
            daemon
                .emit("appearance.changed", said(paths, appearance))
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
    Ok(said(paths, appearance))
}
