//! The "New sessions" section of Settings (#785, `docs/design.md` §10): the provider, model and
//! effort a new session starts on, from the same model choice as everywhere, on GPUI's test
//! platform in front of a real daemon in temporary folders, its providers the fake ones. No window
//! opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::settings::SettingsPage;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn user() -> User {
    let user = User::with_fake_providers().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.write_providers("[opencode]\nenabled = true\n");
    user.provider_script(
        "claude",
        json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}),
    );
    user.provider_script(
        "opencode",
        json!({"models": ["open/small", "open/medium"], "efforts": ["low", "high"]}),
    );
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A window whose every connection to the daemon is made through `connect`.
fn window_through<'a>(
    user: &User,
    connect: flottille_window::link::Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    (shell, cx)
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// Waits for the element to be drawn, then clicks it: a click on an element the window has not
/// drawn yet is lost, or panics.
fn click(shell: &Entity<Shell>, cx: &mut VisualTestContext, id: &str) {
    until(shell, cx, &format!("{id} is drawn"), |cx| present(cx, id));
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

/// Waits until the element says `text`.
fn shows(shell: &Entity<Shell>, cx: &mut VisualTestContext, id: &str, text: &str) {
    until(shell, cx, &format!("{id} says {text}"), |cx| {
        label(cx, id).as_deref() == Some(text)
    });
}

fn open_settings(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SettingsPage> {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap();
    until(shell, cx, "the providers are listed", |cx| {
        page.read_with(cx, |p, _| p.providers().known)
    });
    settle(cx);
    page
}

/// The options of a model select and an effort select, opened: their labels.
fn rows(shell: &Entity<Shell>, cx: &mut VisualTestContext, id: &str, keys: &[&str]) -> Vec<String> {
    click(shell, cx, id);
    let rows = keys
        .iter()
        .map(|key| {
            let option = format!("{id}-{key}");
            until(shell, cx, &format!("{option} is drawn"), |cx| {
                present(cx, &option)
            });
            label(cx, &option).unwrap()
        })
        .collect();
    click(shell, cx, id);
    until(shell, cx, &format!("{id} is closed"), |cx| {
        keys.iter().all(|key| !present(cx, &format!("{id}-{key}")))
    });
    rows
}

/// What the daemon says new sessions start on.
fn chosen(user: &User) -> serde_json::Value {
    user.call("provider.list", serde_json::Value::Null).unwrap()["new_session"].clone()
}

fn settings_page<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<SettingsPage>,
    &'a mut VisualTestContext,
) {
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = window_through(user, connect, cx);
    let page = open_settings(&shell, cx);
    until(&shell, cx, "the models are known", |cx| {
        page.read_with(cx, |p, _| {
            p.providers()
                .get("claude")
                .is_some_and(|c| c.models.is_some())
        })
    });
    settle(cx);
    (shell, page, cx)
}

#[gpui_kit::test]
fn the_setting_s_pickers_name_the_real_value_never_default(cx: &mut TestAppContext) {
    let user = user();
    let (shell, _page, cx) = settings_page(&user, cx);

    // Left alone, the first provider's recommended model and its effort are selected and named.
    shows(
        &shell,
        cx,
        "new-sessions-on-model",
        "Claude · fake-small · recommended",
    );
    until(&shell, cx, "the effort is said", |cx| {
        present(cx, "new-sessions-on-effort")
    });
    let effort = label(cx, "new-sessions-on-effort").unwrap();
    assert!(effort.ends_with(" · recommended"), "{effort}");
    let mut said = vec![label(cx, "new-sessions-on-model").unwrap(), effort];
    said.extend(rows(
        &shell,
        cx,
        "new-sessions-on-model",
        &[
            "claude-fake-small",
            "claude-fake-large",
            "opencode-open-small",
            "opencode-open-medium",
        ],
    ));
    said.extend(rows(&shell, cx, "new-sessions-on-effort", &["low", "high"]));
    assert_eq!(said.len(), 8, "{said:?}");
    for said in &said {
        assert!(!said.to_lowercase().contains("default"), "{said}");
    }
    assert!(!present(cx, "new-sessions-on-model-first"));
    assert!(!present(cx, "new-sessions-on-effort-default"));
    assert_eq!(chosen(&user), serde_json::Value::Null, "nothing was chosen");
}

#[gpui_kit::test]
fn choosing_the_recommended_model_back_clears_the_choice(cx: &mut TestAppContext) {
    let user = user();
    let (shell, _page, cx) = settings_page(&user, cx);

    // OpenCode's recommended model: its provider chosen, its model left to it. Its efforts are
    // known, and its effort goes beside it.
    click(&shell, cx, "new-sessions-on-model");
    click(&shell, cx, "new-sessions-on-model-opencode-open-small");
    until(&shell, cx, "the choice is stored", |_| {
        chosen(&user) == json!({"provider": "opencode"})
    });
    until(&shell, cx, "the model is shown", |cx| {
        label(cx, "new-sessions-on-model").as_deref() == Some("OpenCode · open/small · recommended")
            && present(cx, "new-sessions-on-effort")
    });
    settle(cx);
    click(&shell, cx, "new-sessions-on-effort");
    click(&shell, cx, "new-sessions-on-effort-high");
    until(&shell, cx, "the effort is stored", |_| {
        chosen(&user) == json!({"provider": "opencode", "effort": "high"})
    });
    shows(&shell, cx, "new-sessions-on-effort", "high");
    // Another model of it, then back on its recommended one.
    click(&shell, cx, "new-sessions-on-model");
    click(&shell, cx, "new-sessions-on-model-opencode-open-medium");
    until(&shell, cx, "the model is stored", |_| {
        chosen(&user) == json!({"provider": "opencode", "model": "open/medium"})
    });
    shows(
        &shell,
        cx,
        "new-sessions-on-model",
        "OpenCode · open/medium",
    );
    // The first provider's recommended model, back: no choice at all, as before.
    click(&shell, cx, "new-sessions-on-model");
    click(&shell, cx, "new-sessions-on-model-claude-fake-small");
    until(&shell, cx, "the choice is cleared", |cx| {
        chosen(&user) == serde_json::Value::Null
            && label(cx, "new-sessions-on-model").as_deref()
                == Some("Claude · fake-small · recommended")
    });
    let text = std::fs::read_to_string(user.path("config/flottille/providers.toml")).unwrap();
    assert!(!text.contains("new_session"), "{text}");
}

#[gpui_kit::test]
fn a_model_no_longer_offered_falls_back_to_what_the_provider_proposes(cx: &mut TestAppContext) {
    let user = user();
    user.write_providers(
        "[opencode]\nenabled = true\n[new_session]\nprovider = \"claude\"\nmodel = \"retired\"\n",
    );
    let (shell, _page, cx) = settings_page(&user, cx);

    // The page says so once, and the pickers show what the session starts on.
    shows(
        &shell,
        cx,
        "new-sessions-fallback",
        "Claude no longer offers retired: new sessions start on what it proposes.",
    );
    shows(
        &shell,
        cx,
        "new-sessions-on-model",
        "Claude · fake-small · recommended",
    );
    // Once another model is chosen, it says nothing.
    click(&shell, cx, "new-sessions-on-model");
    click(&shell, cx, "new-sessions-on-model-claude-fake-large");
    // The notice folds away: its animation runs on the clock, which only `settle` advances.
    until(&shell, cx, "the notice is gone", |cx| {
        settle(cx);
        !present(cx, "new-sessions-fallback")
    });
    until(&shell, cx, "the choice is stored", |_| {
        chosen(&user) == json!({"provider": "claude", "model": "fake-large"})
    });
}

/// Every label a model select and an effort select say, closed and opened: the model's select
/// by its id and the models it offers, the effort's by its levels.
fn said(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    id: &str,
    models: &[&str],
    efforts: &[&str],
) -> Vec<String> {
    let mut said = vec![label(cx, &format!("{id}-model")).unwrap()];
    said.extend(rows(shell, cx, &format!("{id}-model"), models));
    if let Some(effort) = label(cx, &format!("{id}-effort")) {
        said.push(effort);
        said.extend(rows(shell, cx, &format!("{id}-effort"), efforts));
    }
    assert!(
        !present(cx, &format!("{id}-model-first")),
        "{id}: a row of the rule"
    );
    said
}

#[gpui_kit::test]
fn no_select_of_the_window_has_a_default_row(cx: &mut TestAppContext) {
    let user = user();
    user.write_providers("[opencode]\nenabled = true\n[judge]\nprovider = \"claude\"\n");
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "[compactor]\nprovider = \"claude\"\n",
    )
    .unwrap();
    let (shell, _page, cx) = settings_page(&user, cx);
    let models = [
        "claude-fake-small",
        "claude-fake-large",
        "opencode-open-small",
        "opencode-open-medium",
    ];
    let mut every = Vec::new();

    // The new sessions' pickers, the judge's, and the compactor's: each opens on what applies,
    // selected and marked, and none has a row named default.
    every.extend(said(
        &shell,
        cx,
        "new-sessions-on",
        &models,
        &["low", "high"],
    ));
    click(&shell, cx, "settings-tabs-approvals");
    until(&shell, cx, "the judge's select", |cx| {
        present(cx, "judge-model")
    });
    settle(cx);
    every.extend(said(&shell, cx, "judge", &models, &[]));
    assert!(
        !present(cx, "judge-effort"),
        "the judge keeps its model's own effort"
    );
    click(&shell, cx, "settings-tabs-features");
    until(&shell, cx, "the compaction row", |cx| {
        present(cx, "features-compaction")
    });
    click(&shell, cx, "features-compaction");
    until(&shell, cx, "the compactor's select", |cx| {
        present(cx, "compaction-summary-model")
    });
    settle(cx);
    every.extend(said(
        &shell,
        cx,
        "compaction-summary",
        &models,
        &["low", "high"],
    ));

    assert!(every.len() > 12, "{every:?}");
    for said in &every {
        assert!(!said.to_lowercase().contains("default"), "{said}");
    }
    assert!(
        every.iter().any(|said| said.ends_with(" · recommended")),
        "the recommended value is marked: {every:?}"
    );
}

#[gpui_kit::test]
fn with_the_first_provider_s_models_unknown_the_select_opens_on_it(cx: &mut TestAppContext) {
    // Claude, the provider new sessions start on, has said no model yet; OpenCode has.
    let user = User::with_fake_providers().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.write_providers("[opencode]\nenabled = true\n");
    user.provider_script("claude", json!({"die_on_open": 3}));
    user.provider_script(
        "opencode",
        json!({"models": ["open/small", "open/medium"], "efforts": ["low", "high"]}),
    );
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = window_through(&user, connect, cx);
    let page = open_settings(&shell, cx);
    until(&shell, cx, "OpenCode's models are known", |cx| {
        page.read_with(cx, |p, _| {
            p.providers()
                .get("opencode")
                .is_some_and(|o| o.models.is_some())
        })
    });
    settle(cx);
    assert!(page.read_with(cx, |p, _| {
        p.providers()
            .get("claude")
            .is_some_and(|c| c.models.is_none())
    }));

    // The value that applies is Claude's, its models learnt at its first session: never
    // OpenCode's recommended model.
    shows(
        &shell,
        cx,
        "new-sessions-on-model",
        "Claude · models learnt at its first session · recommended",
    );
    assert_eq!(
        rows(
            &shell,
            cx,
            "new-sessions-on-model",
            &["opencode-open-small"]
        ),
        vec!["OpenCode · open/small · recommended".to_owned()]
    );
    // Chosen, OpenCode's is a choice; Claude's back clears it.
    click(&shell, cx, "new-sessions-on-model");
    click(&shell, cx, "new-sessions-on-model-opencode-open-small");
    until(&shell, cx, "the choice is stored", |_| {
        chosen(&user) == json!({"provider": "opencode"})
    });
    // The window shows the choice before Claude's row is clicked: until then, picking it would
    // change nothing from the window's point of view.
    shows(
        &shell,
        cx,
        "new-sessions-on-model",
        "OpenCode · open/small · recommended",
    );
    click(&shell, cx, "new-sessions-on-model");
    click(&shell, cx, "new-sessions-on-model-claude-recommended");
    until(&shell, cx, "the choice is cleared", |cx| {
        chosen(&user) == serde_json::Value::Null
            && label(cx, "new-sessions-on-model").as_deref()
                == Some("Claude · models learnt at its first session · recommended")
    });
}

#[gpui_kit::test]
fn the_judge_and_the_compactor_mark_the_model_flottille_applies_for_them(cx: &mut TestAppContext) {
    // Claude proposes fake-large; Flottille's judge and compactor run on its smallest, haiku.
    let user = User::with_fake_providers().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.write_providers("[opencode]\nenabled = true\n[judge]\nprovider = \"claude\"\n");
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "[compactor]\nprovider = \"claude\"\n",
    )
    .unwrap();
    user.provider_script(
        "claude",
        json!({"models": ["fake-large", "haiku"], "efforts": ["low", "high"]}),
    );
    user.provider_script("opencode", json!({"models": ["open/small"]}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (shell, _page, cx) = settings_page(&user, cx);

    click(&shell, cx, "settings-tabs-approvals");
    until(&shell, cx, "the judge's select", |cx| {
        present(cx, "judge-model")
    });
    settle(cx);
    // A judge without a model runs on the provider's: shown as it is, not as the recommended.
    shows(&shell, cx, "judge-model", "Claude · fake-large");
    assert_eq!(
        rows(&shell, cx, "judge-model", &["claude-haiku"]),
        vec!["Claude · haiku · recommended".to_owned()]
    );
    click(&shell, cx, "judge-model");
    click(&shell, cx, "judge-model-claude-haiku");
    until(&shell, cx, "the judge is haiku", |_| {
        let listed = user.call("provider.list", serde_json::Value::Null).unwrap();
        listed["judge"]["provider"] == "claude" && listed["judge"]["model"] == "haiku"
    });

    click(&shell, cx, "settings-tabs-features");
    until(&shell, cx, "the compaction row", |cx| {
        present(cx, "features-compaction")
    });
    click(&shell, cx, "features-compaction");
    until(&shell, cx, "the compactor's select", |cx| {
        present(cx, "compaction-summary-model")
    });
    settle(cx);
    shows(
        &shell,
        cx,
        "compaction-summary-model",
        "Claude · fake-large",
    );
    click(&shell, cx, "compaction-summary-model");
    until(&shell, cx, "the haiku row is drawn", |cx| {
        present(cx, "compaction-summary-model-claude-haiku")
    });
    shows(
        &shell,
        cx,
        "compaction-summary-model-claude-haiku",
        "Claude · haiku · recommended",
    );
    click(&shell, cx, "compaction-summary-model-claude-haiku");
    until(&shell, cx, "the compactor is haiku", |_| {
        let shown = user.call("compaction.show", json!({})).unwrap();
        shown["compactor"]["provider"] == "claude" && shown["compactor"]["model"] == "haiku"
    });
}
