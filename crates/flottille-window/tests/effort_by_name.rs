//! An effort is shown by its name, never as `default` (issue #784): a `default` that a daemon
//! reported or a store kept is resolved by the window to the effort the model runs with, as the
//! model's label is, and no list of efforts offers it as a choice.

use flottille_window::actions::Pick;
use flottille_window::composer::{Menu, Pickers};
use flottille_window::providers::Providers;
use gpui_design_system::ModelEntry;
use serde_json::json;

fn providers() -> Providers {
    Providers::of(Ok(&json!({"providers": [
        {"name": "claude", "title": "Claude", "enabled": true, "ready": true, "why": null,
         "warnings": [], "program": {"name": "claude", "present": true, "path": "/usr/bin/claude",
         "version": "2.1.295", "checked": "2.1.295"},
         "login": {"present": true, "path": "/home/someone/.claude/.credentials.json",
         "api_key": null},
         "override": null,
         "models": [{"id": "opus", "name": "Claude Opus 5.5", "default": true,
                     "efforts": ["default", "low", "medium", "high", "xhigh"]}]}
    ]})))
}

fn pick(effort: Option<&str>) -> Pick {
    Pick {
        provider: Some("claude".into()),
        model: Some("opus".into()),
        effort: effort.map(str::to_owned),
    }
}

#[test]
fn a_stray_default_effort_is_shown_by_the_name_the_model_runs_with() {
    let providers = providers();
    let pick = pick(Some("default"));
    let pickers = Pickers {
        providers: &providers,
        pick: &pick,
        project: (None, None),
    };

    assert_eq!(pickers.effort_shown(), "high");
    let rows = pickers.entries(Menu::Effort);
    let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(labels, ["low", "medium", "high", "xhigh"]);
    let on: Vec<&str> = rows
        .iter()
        .filter(|row| row.on)
        .map(|row| row.label.as_str())
        .collect();
    assert_eq!(
        on,
        ["high"],
        "the effort the model runs with is the selected row"
    );
}

#[test]
fn no_effort_label_in_the_window_reads_default() {
    let providers = providers();
    for picked in [None, Some("default"), Some("low"), Some("xhigh")] {
        let pick = pick(picked);
        let pickers = Pickers {
            providers: &providers,
            pick: &pick,
            project: (None, None),
        };
        let mut said = vec![pickers.effort_shown()];
        said.extend(
            pickers
                .entries(Menu::Effort)
                .into_iter()
                .map(|row| row.label),
        );
        for said in said {
            assert!(
                !said.to_lowercase().contains("default"),
                "{picked:?}: {said:?}"
            );
        }
    }
    // The efforts a settings picker (the compactor's, the judge's, Review's) offers beside a
    // model: the provider's own choice is not one of them.
    let entry = ModelEntry::new("claude", "Claude", Some("opus".into()), "Claude Opus 5.5");
    assert_eq!(
        providers.model_entries(),
        [entry
            .efforts(["low", "medium", "high", "xhigh"])
            .recommended()
            .proposes("high")]
    );
}
