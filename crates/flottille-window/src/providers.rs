//! The providers as `provider.list` gives them, and what the window says of each: whether it is
//! enabled and ready, why not, how it signs in. A key is never shown, nor held: the daemon gives
//! the name of the variable set, never its value.

use std::sync::atomic::{AtomicU64, Ordering};

use flottille_core::provider;
use flottille_ui::{Chosen, ModelEntry};
use serde_json::Value;

/// The `provider.list` reads, numbered as they are asked, whoever asks: answers come back in any
/// order, and one older than an answer applied is dropped (`Shell`).
static READS: AtomicU64 = AtomicU64::new(0);

/// The number of a read about to be asked.
pub fn next_read() -> u64 {
    READS.fetch_add(1, Ordering::Relaxed) + 1
}

/// What the window says of a model before a session of its provider said the models.
pub const MODEL_UNKNOWN: &str = "model not known yet";

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
    /// The effort it runs with when none is asked, once a session said it.
    pub default_effort: Option<String>,
    /// The model this one, the provider's own `default`, stands for, when the provider says.
    pub stands_for: Option<String>,
}

impl Model {
    /// How the window names it: its provider's name for it.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.id.clone())
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
    /// The daemon is listing its models now: efforts not known yet may come.
    pub models_learning: bool,
    /// Whether its agents take images in a prompt, as the last that started said; `None` before
    /// one did.
    pub images: Option<bool>,
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
        let mut provider = Provider::read(entry);
        provider.unword_own_choice();
        provider
    }

    /// The row of the provider's own choice (`default`) is never shown by a name that says
    /// "default", nor by none: models stored under the adapter's "Default (recommended)" are
    /// shown by the model they stand for or run, else by the provider's, until a session says
    /// them again (the daemon names the row by its model then).
    fn unword_own_choice(&mut self) {
        let Some(models) = self.models.as_mut() else {
            return;
        };
        let Some(row) = models.iter().find(|m| m.id == provider::OWN_CHOICE) else {
            return;
        };
        if row
            .name
            .as_deref()
            .is_some_and(|n| !provider::says_default(n))
        {
            return;
        }
        // An alias is the model it stands for; a row that runs a model of its own, or any other
        // row, is not a model the others tell.
        let named = |id: &str| {
            let model = models.iter().find(|m| m.id == id)?;
            let label = model.label();
            (model.id != provider::OWN_CHOICE && !provider::says_default(&label)).then_some(label)
        };
        let shown = row
            .stands_for
            .as_deref()
            .and_then(named)
            .unwrap_or_else(|| format!("{}'s model", self.title));
        if let Some(row) = models.iter_mut().find(|m| m.id == provider::OWN_CHOICE) {
            row.name = Some(shown);
        }
    }

    fn read(entry: &Value) -> Provider {
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
                        default_effort: text(&model["default_effort"]),
                        stands_for: text(&model["stands_for"]),
                    })
                    .collect()
            }),
            models_outdated: entry["models_outdated"] == true,
            models_learning: entry["models_learning"] == true,
            images: entry["images"].as_bool(),
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

    /// What the settings page says of it under its name: `Not used` when its switch is off (the
    /// switch is right there, and the config file is the CLI's business), else its state.
    pub fn status(&self) -> String {
        if self.enabled {
            self.state()
        } else {
            "Not used".into()
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

    /// The model a new session takes, once a session said the models: the one marked (the
    /// provider's own `default` when it stands for no other), else the one Flottille applies,
    /// what that `default` stands for or the first model.
    pub fn default_model(&self) -> Option<&Model> {
        let models = self.models.as_ref()?;
        if let Some(marked) = models.iter().find(|m| m.default && m.stands_for.is_none()) {
            return Some(marked);
        }
        let stands_for = models
            .iter()
            .find(|m| m.id == provider::OWN_CHOICE)
            .and_then(|m| m.stands_for.as_deref());
        let id = provider::default_model(models.iter().map(|m| m.id.as_str()), stands_for)?;
        models.iter().find(|m| m.id == id)
    }

    /// The model listed under that id, or that id without its date suffix (the daemon takes both).
    pub fn model(&self, id: &str) -> Option<&Model> {
        let models = self.models.as_ref()?;
        let listed = provider::listed_model(models.iter().map(|m| m.id.as_str()), id)?;
        models.iter().find(|m| m.id == listed)
    }
}

impl Providers {
    /// The model a provider's session runs on, as the window names it: the model asked as it is;
    /// none asked, or the provider's own `default`, the model it takes by its name; `None` when no
    /// session said which that is.
    pub fn model_said(&self, provider: &str, model: Option<&str>) -> Option<String> {
        match model {
            Some(model) if model != crate::composer::DEFAULT => Some(model.to_owned()),
            _ => {
                let provider = self.get(provider)?;
                crate::composer::named(Some(provider), provider.default_model()?)
            }
        }
    }

    /// The same as a sentence says it: when no session said the models yet, that it is not known.
    pub fn model_line(&self, provider: &str, model: Option<&str>) -> String {
        self.model_said(provider, model)
            .unwrap_or_else(|| MODEL_UNKNOWN.to_owned())
    }

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

    /// What a `ModelChoice` offers: the models of the providers enabled and ready, as the daemon
    /// knows them; a provider whose models no session said yet, by its default.
    pub fn model_entries(&self) -> Vec<ModelEntry> {
        let mut entries = Vec::new();
        for provider in self.offered() {
            let listed: Vec<&Model> = provider
                .models
                .iter()
                .flatten()
                .filter(|m| m.id != crate::composer::DEFAULT)
                .collect();
            if listed.is_empty() {
                entries.push(ModelEntry::new(
                    provider.name.clone(),
                    provider.title.clone(),
                    None,
                    self.model_said(&provider.name, None)
                        .unwrap_or_else(|| "its default model".to_owned()),
                ));
            }
            for model in listed {
                entries.push(
                    ModelEntry::new(
                        provider.name.clone(),
                        provider.title.clone(),
                        Some(model.id.clone().into()),
                        model.label(),
                    )
                    .efforts(model.efforts.clone().unwrap_or_default()),
                );
            }
        }
        entries
    }

    /// A model chosen as a `ModelChoice` shows it: the provider's default (no model, or its own
    /// `default`) is its default model's entry, once a session said it.
    pub fn chosen(&self, provider: &str, model: Option<&str>) -> Chosen {
        let model = model
            .filter(|m| *m != crate::composer::DEFAULT)
            .map(str::to_owned)
            .or_else(|| {
                self.get(provider)
                    .filter(|p| p.offered())
                    .and_then(|p| p.default_model())
                    .map(|m| m.id.clone())
            });
        Chosen::Model(provider.to_owned().into(), model.map(Into::into))
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
    fn a_model_id_without_its_date_suffix_is_the_listed_model() {
        let mut dated = listed();
        dated["providers"][0]["models"][0]["id"] = json!("sonnet-4-5-20250929");
        let providers = Providers::of(Ok(&dated));
        let claude = providers.get("claude").unwrap();
        assert_eq!(
            claude.model("sonnet-4-5").unwrap().id,
            "sonnet-4-5-20250929"
        );
        assert!(claude.model("sonnet-4-6").is_none());
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
    fn a_model_is_shown_by_its_name_and_the_alias_says_what_it_stands_for() {
        let providers = Providers::of(Ok(&json!({"providers": [
            {"name": "claude", "title": "Claude", "enabled": true, "ready": true,
             "models": [
                {"id": "default", "name": "Default (recommended)", "efforts": null,
                 "stands_for": "fable"},
                {"id": "fable", "name": "Fable", "efforts": ["low"], "default": true,
                 "default_effort": "low"},
                {"id": "haiku", "efforts": null}]}
        ]})));
        let claude = providers.get("claude").unwrap();
        let labels: Vec<String> = claude.models.iter().flatten().map(Model::label).collect();
        assert_eq!(labels, ["Fable", "Fable", "haiku"]);
        assert_eq!(
            claude.model("default").unwrap().stands_for.as_deref(),
            Some("fable")
        );
        let fable = claude.default_model().unwrap();
        assert_eq!(
            (fable.id.as_str(), fable.default_effort.as_deref()),
            ("fable", Some("low"))
        );
    }

    #[test]
    fn a_stored_default_recommended_name_is_not_shown() {
        // What the daemon stored before it named the row by its model: the adapter's wording,
        // or no name at all.
        for name in [Some("Default (recommended)"), None] {
            let mut stored = json!({"id": "default", "efforts": ["low", "high"], "default": true});
            if let Some(name) = name {
                stored["name"] = json!(name);
            }
            let providers = Providers::of(Ok(&json!({"providers": [
                {"name": "claude", "title": "Claude", "enabled": true, "ready": true,
                 "models": [stored, {"id": "opus", "name": "Claude Opus 5.5", "efforts": null}]}
            ]})));
            let pick = crate::actions::Pick {
                provider: Some("claude".into()),
                model: None,
                effort: None,
            };
            let pickers = crate::composer::Pickers {
                providers: &providers,
                pick: &pick,
                project: (None, None),
            };
            let mut said = vec![pickers.model_shown()];
            said.extend(
                pickers
                    .entries(crate::composer::Menu::Model)
                    .into_iter()
                    .map(|e| e.label),
            );
            said.push(providers.model_line("claude", None));
            for line in &said {
                assert!(
                    !line.to_lowercase().contains("default"),
                    "{name:?}: {said:?}"
                );
                assert!(!line.contains("recommended"), "{name:?}: {said:?}");
            }
            assert_eq!(said[0], "Claude's model", "{name:?}");
        }
    }

    #[test]
    fn a_default_with_no_row_of_its_own_is_shown_and_picked_by_its_name() {
        let providers = Providers::of(Ok(&json!({"providers": [
            {"name": "claude", "title": "Claude", "enabled": true, "ready": true,
             "models": [
                {"id": "default", "name": "Opus 5.5 (1M context)", "efforts": ["low", "high"],
                 "default": true},
                {"id": "opus", "name": "Claude Opus 5.5", "efforts": ["low", "high"]}]}
        ]})));
        let claude = providers.get("claude").unwrap();
        assert_eq!(claude.default_model().unwrap().id, "default");
        let pick = crate::actions::Pick {
            provider: Some("claude".into()),
            model: None,
            effort: None,
        };
        let pickers = crate::composer::Pickers {
            providers: &providers,
            pick: &pick,
            project: (None, None),
        };
        assert_eq!(pickers.model_shown(), "Opus 5.5 (1M context)");
        let rows: Vec<(String, bool, Option<String>)> = pickers
            .entries(crate::composer::Menu::Model)
            .into_iter()
            .map(|e| (e.label, e.on, e.note))
            .collect();
        assert_eq!(
            rows,
            [
                ("Opus 5.5 (1M context)".into(), true, Some("default".into())),
                ("Claude Opus 5.5".into(), false, None)
            ]
        );
    }
}
