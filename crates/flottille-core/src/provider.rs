//! The providers an agent runs on (`docs/agents.md`, "Providers"): their names, and the user's
//! choice of those enabled, `providers.toml` in Flottille's configuration folder.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::paths::Paths;

/// Every provider Flottille knows, in the order they are listed and chosen by default.
pub const NAMES: [&str; 2] = ["claude", "opencode"];

/// Enabled when the file says nothing of them: the provider Flottille always had.
const ENABLED_BY_DEFAULT: [&str; 1] = ["claude"];

/// Which providers the user enabled: each known name, with its choice.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    enabled: BTreeMap<&'static str, bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    enabled: Option<bool>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            enabled: NAMES
                .iter()
                .map(|name| (*name, ENABLED_BY_DEFAULT.contains(name)))
                .collect(),
        }
    }
}

impl Settings {
    pub fn enabled(&self, name: &str) -> bool {
        self.enabled.get(name).copied().unwrap_or(false)
    }

    /// Reads `providers.toml`: the defaults when there is none. The error names the file and
    /// the key at fault.
    pub fn load(paths: &Paths) -> Result<Settings, String> {
        match std::fs::read_to_string(file(paths)) {
            Ok(text) => Settings::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(error) => Err(format!("providers.toml: {error}")),
        }
    }

    /// The text of `providers.toml`, checked: a provider Flottille does not know is refused, so
    /// that a typo is never silently ignored.
    pub fn parse(text: &str) -> Result<Settings, String> {
        let refuse = |message: String| format!("providers.toml: {message}");
        let reading = toml::Deserializer::parse(text).map_err(|e| refuse(e.message().into()))?;
        let entries: BTreeMap<String, Entry> = serde_path_to_error::deserialize(reading)
            .map_err(|error| refuse(format!("{}: {}", error.path(), error.inner().message())))?;
        let mut settings = Settings::default();
        for (name, entry) in entries {
            let known = NAMES.iter().find(|known| **known == name).ok_or_else(|| {
                refuse(format!(
                    "{name}: unknown provider: the providers are {}",
                    NAMES.join(", ")
                ))
            })?;
            if let Some(enabled) = entry.enabled {
                settings.enabled.insert(known, enabled);
            }
        }
        Ok(settings)
    }
}

/// `$XDG_CONFIG_HOME/flottille/providers.toml`.
pub fn file(paths: &Paths) -> std::path::PathBuf {
    paths.config.join("providers.toml")
}

/// A provider Flottille knows, by its name.
pub fn known(name: &str) -> bool {
    NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_file_claude_is_enabled_and_opencode_is_not() {
        let settings = Settings::default();
        assert!(settings.enabled("claude"));
        assert!(!settings.enabled("opencode"));
        assert!(!settings.enabled("codex"));
    }

    #[test]
    fn the_file_enables_and_disables_each_provider() {
        let settings =
            Settings::parse("[claude]\nenabled = false\n[opencode]\nenabled = true\n").unwrap();
        assert!(!settings.enabled("claude"));
        assert!(settings.enabled("opencode"));
        assert_eq!(
            Settings::parse("[opencode]\n").unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn an_unknown_provider_or_key_is_refused_with_its_key() {
        let unknown = Settings::parse("[codex]\nenabled = true\n").unwrap_err();
        assert_eq!(
            unknown,
            "providers.toml: codex: unknown provider: the providers are claude, opencode"
        );
        let key = Settings::parse("[claude]\nenable = true\n").unwrap_err();
        assert!(key.starts_with("providers.toml: claude.enable: "), "{key}");
        let kind = Settings::parse("[claude]\nenabled = \"yes\"\n").unwrap_err();
        assert!(
            kind.starts_with("providers.toml: claude.enabled: "),
            "{kind}"
        );
    }
}
