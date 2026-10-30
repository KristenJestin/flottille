//! The providers as `provider.list` gives them, and what the window says of each: whether it is
//! enabled and ready, why not, how it signs in. A key is never shown, nor held: the daemon gives
//! the name of the variable set, never its value.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

/// The `provider.list` reads, numbered as they are asked, whoever asks: answers come back in any
/// order, and one older than an answer applied is dropped (`Shell`).
static READS: AtomicU64 = AtomicU64::new(0);

/// The number of a read about to be asked.
pub fn next_read() -> u64 {
    READS.fetch_add(1, Ordering::Relaxed) + 1
}

/// A model a provider's sessions offered.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub id: String,
    /// The name its provider shows it by, once a session said it.
    pub name: Option<String>,
    /// `None` until a session had this model.
    pub efforts: Option<Vec<String>>,
    /// The model a new session takes when none is asked.
    pub default: bool,
}

impl Model {
    /// How the window names it: its provider's name for it, and "default model" for the
    /// provider's own `default` entry, so that it is not said twice.
    pub fn label(&self) -> String {
        match &self.name {
            _ if self.id == "default" => "default model".to_owned(),
            Some(name) => name.clone(),
            None => self.id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Provider {
    pub name: String,
    pub title: String,
    pub enabled: bool,
    pub ready: bool,
    /// A clause after its name saying why its agents cannot run, as the daemon says it.
    pub why: Option<String>,
    pub warnings: Vec<String>,
    /// Its program's name, and where and at which version it was found.
    pub program: String,
    pub program_path: Option<String>,
    pub version: Option<String>,
    /// Its login file, found or not: checked for presence only, by the daemon.
    pub login: Option<String>,
    pub login_present: bool,
    /// The name of the API key variable found set.
    pub api_key: Option<String>,
    /// A test program stands for it.
    pub overridden: bool,
    /// `None` until a session of its agent said them.
    pub models: Option<Vec<Model>>,
    /// The models were said by another version of its program: shown, but they refuse nothing.
    pub models_outdated: bool,
}

/// Every provider, in order, or why the daemon could not list them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Providers {
    pub list: Vec<Provider>,
    /// The daemon's sentence when it could not list them (`providers.toml` cannot be read).
    pub error: Option<String>,
    /// Whether the daemon answered yet.
    pub known: bool,
    /// The Auto judge the user chose: its provider and its model (the provider's default when
    /// none).
    pub judge: Option<(String, Option<String>)>,
    /// Flottille picked the judge, and the user has not seen the notice that says so yet.
    pub judge_picked: bool,
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

impl Provider {
    pub fn of(entry: &Value) -> Provider {
        Provider {
            name: text(&entry["name"]).unwrap_or_default(),
            title: text(&entry["title"]).unwrap_or_default(),
            enabled: entry["enabled"] == true,
            ready: entry["ready"] == true,
            why: text(&entry["why"]),
            warnings: entry["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(text)
                .collect(),
            program: text(&entry["program"]["name"]).unwrap_or_default(),
            program_path: text(&entry["program"]["path"]),
            version: text(&entry["program"]["version"]),
            login: text(&entry["login"]["path"]),
            login_present: entry["login"]["present"] == true,
            api_key: text(&entry["login"]["api_key"]),
            overridden: !entry["override"].is_null(),
            models: entry["models"].as_array().map(|models| {
                models
                    .iter()
                    .map(|model| Model {
                        id: text(&model["id"]).unwrap_or_default(),
                        name: text(&model["name"]),
                        efforts: model["efforts"]
                            .as_array()
                            .map(|efforts| efforts.iter().filter_map(text).collect()),
                        default: model["default"] == true,
                    })
                    .collect()
            }),
            models_outdated: entry["models_outdated"] == true,
        }
    }

    /// Whether a session can start on it: the composer offers only these.
    pub fn offered(&self) -> bool {
        self.enabled && self.ready
    }

    /// Its state in a sentence: `Ready`, or why not as the daemon says it.
    pub fn state(&self) -> String {
        match &self.why {
            None if self.ready => "Ready".into(),
            None => format!("{} is not ready", self.name),
            Some(why) => format!("{} {why}", self.name),
        }
    }

    /// How it signs in, as far as the daemon can see: never a key's value.
    pub fn sign_in(&self) -> String {
        match (&self.login, self.login_present, &self.api_key) {
            (Some(login), true, _) => format!("Signs in with its CLI's login: {login}"),
            (_, _, Some(key)) => format!("Signs in with the API key in {key}"),
            _ if self.overridden => "A test program stands for it and its sign-in".into(),
            _ => "No sign-in Flottille can see: its CLI's login or an API key in the environment"
                .into(),
        }
    }

    /// Its program as the daemon found it.
    pub fn program_line(&self) -> String {
        match (&self.program_path, &self.version) {
            _ if self.overridden => format!("{} · a test program stands for it", self.program),
            (Some(path), Some(version)) => format!("{} {version} · {path}", self.program),
            (Some(path), None) => format!("{} · {path}", self.program),
            (None, _) => format!("{} is not on PATH", self.program),
        }
    }

    /// The model a new session takes, once a session said it.
    pub fn default_model(&self) -> Option<&Model> {
        self.models.as_ref()?.iter().find(|m| m.default)
    }

    pub fn model(&self, id: &str) -> Option<&Model> {
        self.models.as_ref()?.iter().find(|m| m.id == id)
    }
}

impl Providers {
    pub fn of(answer: Result<&Value, String>) -> Providers {
        match answer {
            Ok(answer) => Providers {
                list: answer["providers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(Provider::of)
                    .collect(),
                error: None,
                known: true,
                judge: text(&answer["judge"]["provider"])
                    .map(|provider| (provider, text(&answer["judge"]["model"]))),
                judge_picked: answer["judge"]["picked"] == true,
            },
            Err(error) => Providers {
                list: Vec::new(),
                error: Some(error),
                known: true,
                judge: None,
                judge_picked: false,
            },
        }
    }

    pub fn get(&self, name: &str) -> Option<&Provider> {
        self.list.iter().find(|p| p.name == name)
    }

    /// The providers a session can start on, in order.
    pub fn offered(&self) -> impl Iterator<Item = &Provider> {
        self.list.iter().filter(|p| p.offered())
    }

    /// Why no session can start, one sentence per provider; empty when one can, or while the
    /// daemon has not answered.
    pub fn why_none(&self) -> Vec<String> {
        if !self.known || self.offered().next().is_some() {
            return Vec::new();
        }
        if let Some(error) = &self.error {
            return vec![error.clone()];
        }
        if self.list.is_empty() {
            return vec!["The daemon lists no provider".into()];
        }
        self.list.iter().map(Provider::state).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn listed() -> Value {
        json!({"providers": [
            {"name": "claude", "title": "Claude", "enabled": true, "ready": true, "why": null,
             "warnings": [], "program": {"name": "claude", "present": true, "path": "/usr/bin/claude",
             "version": "2.1.295", "checked": "2.1.295"},
             "login": {"present": true, "path": "/home/someone/.claude/.credentials.json", "api_key": null},
             "override": null,
             "models": [{"id": "sonnet", "efforts": ["low", "high"], "default": true}, {"id": "opus", "efforts": null}]},
            {"name": "opencode", "title": "OpenCode", "enabled": true, "ready": false,
             "why": "is not ready: `opencode` is not on PATH (install it)", "warnings": [],
             "program": {"name": "opencode", "present": false, "path": null, "version": null, "checked": "1.19.0"},
             "login": {"present": false, "path": "/home/someone/.local/share/opencode/auth.json", "api_key": "OPENAI_API_KEY"},
             "override": null, "models": null}
        ]})
    }

    #[test]
    fn only_the_enabled_and_ready_providers_are_offered() {
        let providers = Providers::of(Ok(&listed()));
        let offered: Vec<&str> = providers.offered().map(|p| p.name.as_str()).collect();
        assert_eq!(offered, ["claude"]);
        assert!(providers.why_none().is_empty());
        let claude = providers.get("claude").unwrap();
        assert_eq!(claude.default_model().unwrap().id, "sonnet");
        assert_eq!(claude.model("opus").unwrap().efforts, None);
    }

    #[test]
    fn a_provider_says_its_state_and_its_sign_in_and_never_a_key_s_value() {
        let providers = Providers::of(Ok(&listed()));
        let claude = providers.get("claude").unwrap();
        assert_eq!(claude.state(), "Ready");
        assert_eq!(
            claude.sign_in(),
            "Signs in with its CLI's login: /home/someone/.claude/.credentials.json"
        );
        assert_eq!(claude.program_line(), "claude 2.1.295 · /usr/bin/claude");
        let opencode = providers.get("opencode").unwrap();
        assert_eq!(
            opencode.state(),
            "opencode is not ready: `opencode` is not on PATH (install it)"
        );
        assert_eq!(
            opencode.sign_in(),
            "Signs in with the API key in OPENAI_API_KEY"
        );
        assert_eq!(opencode.program_line(), "opencode is not on PATH");
    }

    #[test]
    fn when_none_is_offered_each_says_why() {
        let mut answer = listed();
        answer["providers"][0]["enabled"] = json!(false);
        answer["providers"][0]["ready"] = json!(false);
        answer["providers"][0]["why"] = json!("is not enabled (here is how)");
        let providers = Providers::of(Ok(&answer));
        assert_eq!(
            providers.why_none(),
            [
                "claude is not enabled (here is how)",
                "opencode is not ready: `opencode` is not on PATH (install it)"
            ]
        );
        let broken = Providers::of(Err("providers.toml: claude.enabled: invalid type".into()));
        assert_eq!(
            broken.why_none(),
            ["providers.toml: claude.enabled: invalid type"]
        );
        assert!(
            Providers::default().why_none().is_empty(),
            "not before the daemon answered"
        );
    }

    #[test]
    fn a_model_is_shown_by_its_name_and_the_provider_s_default_entry_as_default_model() {
        let providers = Providers::of(Ok(&json!({"providers": [
            {"name": "claude", "title": "Claude", "enabled": true, "ready": true,
             "models": [
                {"id": "default", "name": "Default (recommended)", "efforts": null, "default": true},
                {"id": "fable", "name": "Fable", "efforts": null},
                {"id": "haiku", "efforts": null}]}
        ]})));
        let claude = providers.get("claude").unwrap();
        let labels: Vec<String> = claude.models.iter().flatten().map(Model::label).collect();
        assert_eq!(labels, ["default model", "Fable", "haiku"]);
    }
}
