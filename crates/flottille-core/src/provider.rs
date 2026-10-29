//! The providers an agent runs on (`docs/agents.md`, "Providers"): their names, and the user's
//! choice of those enabled, `providers.toml` in Flottille's configuration folder.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::paths::Paths;

/// Every provider Flottille knows, in the order they are listed and chosen by default.
pub const NAMES: [&str; 3] = ["claude", "opencode", "codex"];

/// Enabled when the file says nothing of them: the provider Flottille always had.
const ENABLED_BY_DEFAULT: [&str; 1] = ["claude"];

/// The providers whose bare mode holds only on the version it was qualified on: another one is
/// refused unless the user allows it (`allow_unqualified_version`).
pub const QUALIFIED_ONLY: [&str; 2] = ["opencode", "codex"];

/// Which providers the user enabled: each known name, with its choice.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    enabled: BTreeMap<&'static str, bool>,
    unqualified: Vec<&'static str>,
    /// The Auto judge the user chose (`[judge]`): none until they choose one.
    pub judge: Option<Judge>,
}

/// The model that judges the calls to judge in Auto (`docs/design.md` §7), as `[judge]` holds it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Judge {
    pub provider: String,
    /// The provider's default model when none is named.
    #[serde(default)]
    pub model: Option<String>,
}

impl Judge {
    /// Who judged, as a verdict records it: `claude haiku`.
    pub fn who(&self) -> String {
        match &self.model {
            Some(model) => format!("{} {model}", self.provider),
            None => self.provider.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    enabled: Option<bool>,
    allow_unqualified_version: Option<bool>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            enabled: NAMES
                .iter()
                .map(|name| (*name, ENABLED_BY_DEFAULT.contains(name)))
                .collect(),
            unqualified: Vec::new(),
            judge: None,
        }
    }
}

impl Settings {
    pub fn enabled(&self, name: &str) -> bool {
        self.enabled.get(name).copied().unwrap_or(false)
    }

    /// Whether the user allows the provider's agents on a version other than the qualified one.
    pub fn allows_unqualified_version(&self, name: &str) -> bool {
        self.unqualified.contains(&name)
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
        let mut table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| refuse(e.message().into()))?;
        let judge = table.remove("judge");
        let entries: BTreeMap<String, Entry> = serde_path_to_error::deserialize(table)
            .map_err(|error| refuse(format!("{}: {}", error.path(), error.inner().message())))?;
        let mut settings = Settings::default();
        if let Some(judge) = judge {
            let judge: Judge = serde_path_to_error::deserialize(judge).map_err(|error| {
                refuse(format!(
                    "judge.{}: {}",
                    error.path(),
                    error.inner().message()
                ))
            })?;
            if !known(&judge.provider) {
                return Err(refuse(format!(
                    "judge.provider: unknown provider {}: the providers are {}",
                    judge.provider,
                    NAMES.join(", ")
                )));
            }
            settings.judge = Some(judge);
        }
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
            if let Some(allow) = entry.allow_unqualified_version {
                if !QUALIFIED_ONLY.contains(known) {
                    let whose: Vec<String> = QUALIFIED_ONLY
                        .iter()
                        .map(|name| format!("{name}'s"))
                        .collect();
                    return Err(refuse(format!(
                        "{name}.allow_unqualified_version: only {} versions are checked before \
                         their agents start",
                        whose.join(" and ")
                    )));
                }
                if allow {
                    settings.unqualified.push(known);
                }
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
    fn the_auto_judge_is_read_from_its_table_and_none_is_chosen_by_default() {
        assert_eq!(Settings::default().judge, None);
        let settings = Settings::parse(
            "[claude]\nenabled = true\n[judge]\nprovider = \"claude\"\nmodel = \"haiku\"\n",
        )
        .unwrap();
        assert_eq!(
            settings.judge,
            Some(Judge {
                provider: "claude".into(),
                model: Some("haiku".into())
            })
        );
        assert!(settings.enabled("claude"));
        let unknown = Settings::parse("[judge]\nprovider = \"gemini\"\n").unwrap_err();
        assert!(
            unknown.starts_with("providers.toml: judge.provider"),
            "{unknown}"
        );
        let key = Settings::parse("[judge]\nprovider = \"claude\"\nwho = 1\n").unwrap_err();
        assert!(key.contains("judge"), "{key}");
    }

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
    fn opencode_s_unqualified_versions_are_allowed_only_when_the_file_says_so() {
        assert!(!Settings::default().allows_unqualified_version("opencode"));
        let settings =
            Settings::parse("[opencode]\nenabled = true\nallow_unqualified_version = true\n")
                .unwrap();
        assert!(settings.allows_unqualified_version("opencode"));
        let claude = Settings::parse("[claude]\nallow_unqualified_version = true\n").unwrap_err();
        assert_eq!(
            claude,
            "providers.toml: claude.allow_unqualified_version: only opencode's and codex's \
             versions are checked before their agents start"
        );
    }

    #[test]
    fn an_unknown_provider_or_key_is_refused_with_its_key() {
        let unknown = Settings::parse("[gemini]\nenabled = true\n").unwrap_err();
        assert_eq!(
            unknown,
            "providers.toml: gemini: unknown provider: the providers are claude, opencode, codex"
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
