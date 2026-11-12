//! What a new session starts on in the composer (issue #785): the project's file first, then what
//! the user chose in Settings, then the last pick, then what the provider proposes. Pure: the
//! providers as the daemon lists them, and the pick the composer makes of them.

use flottille_window::actions::Pick;
use flottille_window::composer::first_pick;
use flottille_window::providers::Providers;
use serde_json::{Value, json};

fn providers(new_session: Value) -> Providers {
    Providers::of(Ok(&json!({"providers": [
        {"name": "claude", "title": "Claude", "enabled": true, "ready": true,
         "models": [
            {"id": "opus", "name": "Opus 5.5", "default": true, "default_effort": "high",
             "efforts": ["low", "high"]},
            {"id": "haiku", "name": "Haiku 4.5", "efforts": ["low", "medium"]}]},
        {"name": "codex", "title": "Codex", "enabled": true, "ready": true,
         "models": [{"id": "gpt-5", "name": "GPT-5", "default": true, "efforts": ["low", "high"]},
                    {"id": "gpt-5-mini", "name": "GPT-5 mini", "efforts": ["low", "high"]}]}
    ], "new_session": new_session})))
}

fn pick(provider: &str, model: Option<&str>, effort: Option<&str>) -> Pick {
    Pick {
        provider: Some(provider.into()),
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
    }
}

#[test]
fn by_default_a_new_session_starts_on_what_the_provider_proposes() {
    let providers = providers(Value::Null);
    assert_eq!(
        first_pick(&providers, (None, None), None),
        pick("claude", None, None)
    );
}

#[test]
fn a_chosen_provider_model_and_effort_are_what_a_new_session_starts_on() {
    let providers = providers(json!({"provider": "codex", "model": "gpt-5-mini", "effort": "low"}));
    let last = pick("claude", Some("haiku"), None);
    assert_eq!(
        first_pick(&providers, (None, None), Some(&last)),
        pick("codex", Some("gpt-5-mini"), Some("low")),
        "the setting stands above the last pick"
    );
    // The provider alone leaves its model and effort to it.
    let alone = self::providers(json!({"provider": "codex", "model": null, "effort": null}));
    assert_eq!(
        first_pick(&alone, (None, None), None),
        pick("codex", None, None)
    );
}

#[test]
fn a_project_s_model_wins_over_the_setting() {
    let providers = providers(json!({"provider": "claude", "model": "haiku", "effort": "medium"}));
    // The project's model, and the effort chosen went with another model.
    assert_eq!(
        first_pick(&providers, (Some("claude"), Some("opus")), None),
        pick("claude", Some("opus"), None)
    );
    // A project that gives its provider only leaves the model to the setting.
    assert_eq!(
        first_pick(&providers, (Some("claude"), None), None),
        pick("claude", Some("haiku"), Some("medium"))
    );
    // Another provider than the setting's: the setting is not for it.
    assert_eq!(
        first_pick(&providers, (Some("codex"), None), None),
        pick("codex", None, None)
    );
}

#[test]
fn a_model_no_longer_offered_falls_back_to_what_the_provider_proposes() {
    let providers = providers(json!({"provider": "claude", "model": "retired", "effort": "high"}));
    assert_eq!(
        first_pick(&providers, (None, None), None),
        pick("claude", None, None)
    );
    let providers =
        self::providers(json!({"provider": "claude", "model": "haiku", "effort": "high"}));
    assert_eq!(
        first_pick(&providers, (None, None), None),
        pick("claude", Some("haiku"), None),
        "the effort the model no longer offers goes, the model stays"
    );
}
