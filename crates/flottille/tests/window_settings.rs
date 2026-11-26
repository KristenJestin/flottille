//! The redesigned Settings (#702, `docs/design.md` §10, `docs/reference/gpui.md` rules 13 to 15):
//! six tabs by subject (Machines the sixth, #1144), the Features tab a list of rows that open their page, and each link to a
//! former tab opening the new place. On GPUI's test platform in front of a real daemon in
//! temporary folders, its providers the fake ones. No window opens on a screen.

use crate::common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::settings::SettingsPage;
use gpui_design_system::motion::PANEL;
use gpui_design_system::omarchy::Environment;
use gpui_design_system::space;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// The tabs, in order.
const TABS: [&str; 6] = [
    "agents",
    "approvals",
    "features",
    "vault",
    "machines",
    "general",
];

/// The tabs of the window before the redesign: none is left.
const FORMER: [&str; 11] = [
    "judge",
    "specs",
    "review",
    "providers",
    "compaction",
    "rules",
    "workspaces",
    "sessions",
    "appearance",
    "developer",
    "connections",
];

fn user() -> User {
    let user = User::with_fake_providers();
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

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    window_through(user, connect, cx)
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

fn visible(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some_and(|f| f.visible()))
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn click(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
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

fn tab(shell: &Entity<Shell>, cx: &mut VisualTestContext, key: &str) {
    until(shell, cx, "the tab is there", |cx| {
        present(cx, &format!("settings-tabs-{key}"))
    });
    click(cx, &format!("settings-tabs-{key}"));
}

#[gpui_kit::test]
fn settings_has_six_tabs_and_opens_on_agents(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);

    for key in TABS {
        assert!(visible(cx, &format!("settings-tabs-{key}")), "{key}");
    }
    for key in FORMER {
        assert!(!present(cx, &format!("settings-tabs-{key}")), "{key}");
    }
    assert_eq!(page.read_with(cx, |p, _| p.tab()), 0, "it opens on Agents");
    assert_eq!(
        cx.update(|window, _| window.find("settings-tabs-agents").selected()),
        Some(true)
    );
    // Its four sections, in order.
    for id in [
        "providers-title",
        "rules-title",
        "tool-results-title",
        "sessions-title",
    ] {
        assert!(visible(cx, id), "{id}");
    }
}

#[gpui_kit::test]
fn a_provider_s_switch_carries_its_name_and_says_its_state_once(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);

    assert_eq!(
        label(cx, "provider-switch-opencode-label").as_deref(),
        Some("OpenCode"),
        "the switch says what it turns on, never Use"
    );
    assert_eq!(
        label(cx, "provider-state-opencode").as_deref(),
        Some("Ready")
    );
    // Said once: no other line of the row says it, and no second line of sign-in.
    assert_eq!(page.read_with(cx, |p, _| p.said_of("opencode")), ["Ready"]);

    click(cx, "provider-switch-opencode");
    until(&shell, cx, "the switch is off", |cx| {
        label(cx, "provider-state-opencode").as_deref()
            == Some("Off: sessions that run keep running")
    });
    assert_eq!(
        label(cx, "provider-switch-opencode-label").as_deref(),
        Some("OpenCode"),
        "an off switch keeps its name"
    );
}

#[gpui_kit::test]
fn no_login_path_nor_variable_is_shown(cx: &mut TestAppContext) {
    let user = user().with_env("ANTHROPIC_API_KEY", "not-a-real-key");
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);

    let lines: Vec<String> = ["claude", "codex", "opencode"]
        .iter()
        .flat_map(|name| page.read_with(cx, |p, _| p.said_of(name)))
        .collect();
    assert!(!lines.is_empty());
    for line in &lines {
        for word in [
            "providers.toml",
            "enabled =",
            "ANTHROPIC",
            "API_KEY",
            ".json",
            ".credentials",
            "not-a-real-key",
            "~/",
            "/",
        ] {
            assert!(!line.contains(word), "{word} in {line}");
        }
    }
    for word in ["providers.toml", "ANTHROPIC_API_KEY", "login file"] {
        cx.update(|window, _| assert!(window.try_find_by_label(word).is_none(), "{word}"));
    }
}

#[gpui_kit::test]
fn the_judge_s_notice_opens_approvals_with_the_judge_in_view(cx: &mut TestAppContext) {
    let user = User::with_fake_claude();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the judge's notice is shown", |cx| {
        shell.read_with(cx, |s, _| s.judge_notice().is_some())
    });
    settle(cx);
    cx.update(|window, cx| window.click("judge-notice-open", cx));
    until(&shell, cx, "Settings shows the judge", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .settings_page()
                .is_some_and(|page| page.read(cx).tab() == 1)
        })
    });
    settle(cx);
    assert_eq!(
        label(cx, "judge-switch-label").as_deref(),
        Some("Let a model decide in Auto")
    );
    assert!(visible(cx, "judge-switch"));
    assert!(visible(cx, "judge-usage"));
    assert_eq!(
        label(cx, "guidance-title").as_deref(),
        Some("What the judge is told")
    );
    assert!(!present(cx, "judge-how"), "How Auto works is gone");
}

#[gpui_kit::test]
fn a_feature_is_a_row_of_features_and_opens_its_page(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    tab(&shell, cx, "features");
    until(&shell, cx, "the features are listed", |cx| {
        present(cx, "features-specs")
    });
    // The rows replace the skeleton with a fade: they are present before they are all shown.
    settle(cx);

    for key in ["specs", "review", "compaction"] {
        assert!(visible(cx, &format!("features-{key}")), "{key}");
    }
    assert_eq!(label(cx, "features-specs").as_deref(), Some("Specs"));
    assert!(!present(cx, "specs-switch"), "the list holds no page");

    click(cx, "features-review");
    until(&shell, cx, "its page is in", |cx| {
        visible(cx, "review-switch")
    });
    settle(cx);
    assert_eq!(
        label(cx, "features-path").as_deref(),
        Some("Features / Review")
    );
    assert_eq!(
        label(cx, "review-switch-label").as_deref(),
        Some("Review changes")
    );

    // The first word of the path goes back.
    click(cx, "features-back");
    settle(cx);
    assert!(!present(cx, "review-switch"));
    assert!(visible(cx, "features-review"));
}

#[gpui_kit::test]
fn a_feature_off_shows_its_switch_sentence_and_usage_line_only(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    tab(&shell, cx, "features");
    until(&shell, cx, "the features are listed", |cx| {
        present(cx, "features-specs")
    });
    click(cx, "features-specs");
    until(&shell, cx, "its page is in", |cx| {
        visible(cx, "specs-switch")
    });

    assert_eq!(
        label(cx, "specs-switch-label").as_deref(),
        Some("Write specs before building")
    );
    assert!(label(cx, "specs-switch-hint").is_some_and(|hint| !hint.is_empty()));
    // The usage line says where it acts, shown while it is off.
    assert!(
        label(cx, "specs-usage").is_some_and(|usage| usage.contains("Write the spec")),
        "{:?}",
        label(cx, "specs-usage")
    );
    assert!(!present(cx, "specs-setting-language-label"));

    // On, its setting unfolds under it, as "Written in".
    click(cx, "specs-switch");
    until(&shell, cx, "its setting unfolds", |cx| {
        present(cx, "specs-setting-language-label")
    });
    settle(cx);
    assert_eq!(
        label(cx, "specs-setting-language-label").as_deref(),
        Some("Written in")
    );
    assert!(visible(cx, "specs-usage"));
}

#[gpui_kit::test]
fn the_usage_line_says_how_to_choose_another_mode(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    tab(&shell, cx, "features");
    until(&shell, cx, "the features are listed", |cx| {
        present(cx, "features-specs")
    });
    click(cx, "features-specs");
    until(&shell, cx, "its page is in", |cx| {
        visible(cx, "specs-usage")
    });

    assert_eq!(
        label(cx, "specs-usage").as_deref(),
        Some(
            "Write the spec on a ticket starts it at once; Write the spec as… in the ticket's ⋯ \
             menu chooses where one spec lives, and New spec on a project's Specs tab starts one \
             with no ticket."
        )
    );
    // It holds with no radios under it: nothing points below, and it names no chosen mode.
    assert!(!present(cx, "specs-setting-mode-label"));
    let usage = label(cx, "specs-usage").unwrap();
    assert!(!usage.contains("below"), "{usage}");
    assert!(!usage.contains("Kept in Flottille"), "{usage}");
}

#[gpui_kit::test]
fn from_a_ticket_offers_two_radios_and_keeps_the_spec_in_flottille_by_default(
    cx: &mut TestAppContext,
) {
    let user = user();
    user.call("extension.set", json!({"name": "specs", "enabled": true}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    tab(&shell, cx, "features");
    until(&shell, cx, "the features are listed", |cx| {
        present(cx, "features-specs")
    });
    click(cx, "features-specs");
    until(&shell, cx, "its setting is in", |cx| {
        present(cx, "specs-setting-mode-label")
    });
    settle(cx);

    assert_eq!(
        label(cx, "specs-setting-mode-label").as_deref(),
        Some("From a ticket")
    );
    assert_eq!(
        label(cx, "specs-setting-mode-internal").as_deref(),
        Some("Kept in Flottille")
    );
    assert_eq!(
        label(cx, "specs-setting-mode-ticket").as_deref(),
        Some("Written to the ticket")
    );
    // Alone is no choice, and the words of the store are not shown.
    assert!(!present(cx, "specs-setting-mode-alone"));
    let chosen = |cx: &mut VisualTestContext, id: &str| {
        cx.update(|window, _| window.find(id.to_owned()).selected() == Some(true))
    };
    assert!(chosen(cx, "specs-setting-mode-internal"));

    click(cx, "specs-setting-mode-ticket");
    until(&shell, cx, "the daemon has the default", |_| {
        let listed = user.call("extension.list", json!({})).unwrap();
        listed["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "specs")
            .is_some_and(|specs| specs["settings"][0]["value"] == "ticket")
    });
    until(&shell, cx, "the choice moves", |cx| {
        settle(cx);
        chosen(cx, "specs-setting-mode-ticket")
    });
}

#[gpui_kit::test]
fn the_compaction_s_rare_settings_are_under_more_settings(cx: &mut TestAppContext) {
    let user = user().with_compactor(json!([{"say": "A summary."}]), "");
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);
    tab(&shell, cx, "features");
    until(&shell, cx, "the features are listed", |cx| {
        present(cx, "features-compaction")
    });
    click(cx, "features-compaction");
    until(&shell, cx, "the compaction is read", |cx| {
        page.read_with(cx, |p, _| p.compaction().is_some())
    });
    settle(cx);
    assert_eq!(
        label(cx, "compaction-switch-label").as_deref(),
        Some("Compact long conversations")
    );
    assert!(visible(cx, "compaction-usage"));
    assert!(visible(cx, "compaction-summary-model"), "it is on");

    // What a user sets: Compact automatically, then its threshold under it.
    for id in ["compaction-automatic", "compaction-threshold"] {
        assert!(visible(cx, id), "{id}");
    }
    // The rest is folded.
    for id in [
        "compaction-maximum",
        "compaction-kept",
        "compaction-before-expiry",
        "compaction-after-expiry",
        "compaction-minimum",
        "compaction-lifetimes",
    ] {
        assert!(!visible(cx, id), "{id} is under More settings");
    }
    click(cx, "compaction-more");
    for id in [
        "compaction-maximum",
        "compaction-kept",
        "compaction-before-expiry",
        "compaction-after-expiry",
        "compaction-minimum",
        "compaction-lifetimes",
    ] {
        assert!(visible(cx, id), "{id}");
    }
    click(cx, "compaction-more");
    assert!(!visible(cx, "compaction-maximum"), "folded again");
}

#[gpui_kit::test]
fn save_appears_only_after_a_change(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);
    until(&shell, cx, "the rules are read", |cx| {
        page.read_with(cx, |p, cx| p.rules().read(cx).shown().is_some())
    });
    settle(cx);

    assert!(visible(cx, "rules-title"));
    assert!(!present(cx, "rules-save-"), "no Save at rest");

    // A change: the editor is typed in.
    let field = page.read_with(cx, |p, cx| p.rules().read(cx).field().cloned().unwrap());
    cx.update(|window, cx| {
        field.update(cx, |state, cx| {
            state.set_value("Always answer in French.", window, cx)
        })
    });
    settle(cx);
    assert!(visible(cx, "rules-save-"), "Save unfolds beside the change");
    // Beside the line and the other button, each at its own width.
    let width = |cx: &mut VisualTestContext, id: &str| {
        let id = id.to_owned();
        cx.update(|window, _| f32::from(window.find(id).bounds().size.width))
    };
    assert!(width(cx, "rules-save-") < 160.0, "Save is a button's width");

    // Set back to what is saved, it folds away.
    let saved = page.read_with(cx, |p, cx| p.rules().read(cx).shown().unwrap().text.clone());
    cx.update(|window, cx| field.update(cx, |state, cx| state.set_value(saved, window, cx)));
    settle(cx);
    assert!(!present(cx, "rules-save-"), "no Save once set back");
}

#[gpui_kit::test]
fn shade_unfolds_only_for_flottille(cx: &mut TestAppContext) {
    let user = user();
    // The desktop is described by temporary files: an Omarchy with an invented theme.
    std::fs::write(user.path("os-release"), "NAME=Test\nID=omarchy\n").unwrap();
    let colors = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    }
    .colors_path();
    std::fs::create_dir_all(colors.parent().unwrap()).unwrap();
    std::fs::write(
        &colors,
        "background = \"#0e1116\"\nforeground = \"#c9d1d9\"\naccent = \"#f0883e\"\n",
    )
    .unwrap();
    user.call(
        "appearance.set",
        json!({"theme": "flottille", "shade": "dark"}),
    )
    .unwrap();
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);
    tab(&shell, cx, "general");
    until(&shell, cx, "the appearance is read", |cx| {
        page.read_with(cx, |p, _| p.appearance_known())
    });
    settle(cx);
    assert!(visible(cx, "appearance-shade"), "Flottille's own colours");
    cx.update(|window, _| {
        assert!(window.try_find_by_label("Colours").is_some());
        assert!(window.try_find_by_label("Theme").is_none());
    });
    for id in ["appearance-title", "workspaces-title", "developer-title"] {
        assert!(visible(cx, id), "{id}");
    }

    click(cx, "appearance-theme-omarchy");
    until(&shell, cx, "the desktop's theme is kept", |_| {
        user.call("appearance.show", json!({})).unwrap()["theme"] == "omarchy"
    });
    settle(cx);
    assert!(
        !present(cx, "appearance-shade"),
        "Shade folds away with the desktop's theme"
    );
}

#[gpui_kit::test]
fn the_quick_switcher_opens_a_feature_s_page(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    until(&shell, cx, "the window is ready", |cx| {
        present(cx, "settings") && shell.read_with(cx, |s, cx| s.extensions().read(cx).is_known())
    });
    // The window asks the registry for its list once it is up: the actions follow it.
    settle(cx);
    cx.simulate_keystrokes("ctrl-k");
    settle(cx);
    cx.simulate_input("settings review");
    settle(cx);
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "Settings shows Review's page", |cx| {
        visible(cx, "review-switch")
    });
    settle(cx);
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .expect("Settings is open");
    assert_eq!(page.read_with(cx, |p, _| p.tab()), 2, "the Features tab");
    assert_eq!(
        label(cx, "features-path").as_deref(),
        Some("Features / Review")
    );
}

#[gpui_kit::test]
fn check_again_reads_the_providers_anew_from_the_section_s_header(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);
    assert!(visible(cx, "providers-check"));
    assert_eq!(
        label(cx, "provider-state-opencode").as_deref(),
        Some("Ready")
    );

    // The file is the user's to edit by hand: the page follows once it is asked to look again.
    user.write_providers("[opencode]\nenabled = false\n");
    click(cx, "providers-check");
    until(&shell, cx, "the page shows it off", |cx| {
        !page.read_with(cx, |p, _| p.switch_on("opencode"))
    });
    assert_eq!(
        label(cx, "provider-state-opencode").as_deref(),
        Some("Off: sessions that run keep running")
    );
}

#[gpui_kit::test]
fn the_log_path_unfolds_only_with_developer_mode(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);
    tab(&shell, cx, "general");
    until(&shell, cx, "the developer mode is read", |cx| {
        page.read_with(cx, |p, _| p.developer().is_some())
    });
    settle(cx);
    assert!(visible(cx, "developer-switch"));
    assert!(!present(cx, "developer-file"), "off: no path");

    click(cx, "developer-switch");
    until(&shell, cx, "the daemon says it is on", |cx| {
        page.read_with(cx, |p, _| p.developer().is_some_and(|d| d.log))
    });
    settle(cx);
    assert!(visible(cx, "developer-file"), "on: the path unfolds");
    // The path is `space::MD` under the switch, not against it.
    let (switch, line) = cx.update(|window, _| {
        (
            window.find("developer-switch").bounds(),
            window.find("developer-file-line").bounds(),
        )
    });
    assert_eq!(line.top() - switch.bottom(), space::MD);
    assert_eq!(
        label(cx, "developer-file").as_deref(),
        Some(user.path("state/flottille/developer.log").to_str().unwrap())
    );

    click(cx, "developer-switch");
    until(&shell, cx, "the daemon says it is off", |cx| {
        page.read_with(cx, |p, _| p.developer().is_some_and(|d| !d.log))
    });
    settle(cx);
    assert!(!present(cx, "developer-file"), "off again: it folds away");
}

/// A third of a panel's motion later: a motion that moves is neither at its start nor at its end.
fn a_third_later(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.executor().advance_clock(PANEL / 3);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

/// How far the log line's block is under the switch of its card.
fn log_line_under_the_switch(cx: &mut VisualTestContext) -> gpui_kit::Pixels {
    cx.update(|window, _| {
        window.find("developer-file-line").bounds().top()
            - window.find("developer-switch").bounds().bottom()
    })
}

#[gpui_kit::test]
fn the_log_line_brings_its_gap_as_it_unfolds_and_takes_it_as_it_folds(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);
    tab(&shell, cx, "general");
    until(&shell, cx, "the developer mode is read", |cx| {
        page.read_with(cx, |p, _| p.developer().is_some())
    });
    settle(cx);
    let one_frame = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    };

    // Turned on: the card's gap is not there at once, it unfolds with the line. (`click` settles
    // every motion, and this test looks at them: the daemon's answer starts the unfolding.)
    cx.update(|window, cx| window.click("developer-switch", cx));
    until(&shell, cx, "the daemon says it is on", |cx| {
        page.read_with(cx, |p, _| p.developer().is_some_and(|d| d.log))
    });
    one_frame(cx);
    let start = log_line_under_the_switch(cx);
    assert!(
        start < space::MD,
        "the gap is there before the line is: {start:?}"
    );
    a_third_later(cx);
    let part = log_line_under_the_switch(cx);
    assert!(part > px(0.) && part < space::MD, "unfolding: {part:?}");
    settle(cx);
    assert_eq!(log_line_under_the_switch(cx), space::MD);

    // Turned off: the gap folds with the line, and the card does not jump when it ends.
    cx.update(|window, cx| window.click("developer-switch", cx));
    until(&shell, cx, "the daemon says it is off", |cx| {
        page.read_with(cx, |p, _| p.developer().is_some_and(|d| !d.log))
    });
    one_frame(cx);
    let start = log_line_under_the_switch(cx);
    assert!(
        start > space::MD / 2.,
        "the gap goes only as the line does: {start:?}"
    );
    a_third_later(cx);
    let part = log_line_under_the_switch(cx);
    assert!(part > px(0.) && part < space::MD, "folding: {part:?}");
    settle(cx);
    assert!(!present(cx, "developer-file-line"), "folded away");
}

/// `connect` as a window's connection, except that a request for the providers waits for `open`:
/// the daemon answers it late, as a slow machine does.
fn holding_providers(
    connect: impl Fn() -> std::io::Result<UnixStream> + Send + Sync + 'static,
    open: Arc<AtomicBool>,
) -> flottille_window::link::Connect {
    Arc::new(move || {
        let (window, relay) = UnixStream::pair()?;
        let daemon = connect()?;
        let (mut to_window, mut from_daemon) = (relay.try_clone()?, daemon.try_clone()?);
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut from_daemon, &mut to_window);
        });
        let open = open.clone();
        std::thread::spawn(move || {
            let mut to_daemon = daemon;
            for line in BufReader::new(relay).lines().map_while(Result::ok) {
                let held = serde_json::from_str::<serde_json::Value>(&line)
                    .is_ok_and(|request| request["method"] == "provider.list");
                while held && !open.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if writeln!(to_daemon, "{line}").is_err() {
                    break;
                }
            }
        });
        Ok(window)
    })
}

#[gpui_kit::test]
fn the_auto_judge_waits_as_a_skeleton_until_the_providers_are_known(cx: &mut TestAppContext) {
    let user = user();
    let open = Arc::new(AtomicBool::new(false));
    let connect = holding_providers(user.connector(), open.clone());
    let (shell, cx) = window_through(&user, connect, cx);
    cx.update(|window, cx| window.click("settings", cx));
    until(&shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    let page = shell
        .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
        .unwrap();
    tab(&shell, cx, "approvals");
    assert!(!page.read_with(cx, |p, _| p.providers().known));
    assert!(present(cx, "judge-skeleton"), "the judge waits");
    assert!(!present(cx, "judge-switch"), "its card is not drawn yet");
    assert!(visible(cx, "judge-title"), "its title stays");

    open.store(true, Ordering::SeqCst);
    until(&shell, cx, "the providers are listed", |cx| {
        page.read_with(cx, |p, _| p.providers().known)
    });
    settle(cx);
    assert!(
        visible(cx, "judge-switch"),
        "the judge replaces its skeleton"
    );
    assert!(!present(cx, "judge-skeleton"));
}

fn copied(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

#[gpui_kit::test]
fn not_signed_in_shows_the_command_to_copy(cx: &mut TestAppContext) {
    let user = User::with_fake_claude();
    std::fs::remove_file(user.path("home/.claude/.credentials.json")).unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    assert_eq!(
        label(cx, "provider-state-claude").as_deref(),
        Some("Not signed in: run")
    );
    assert_eq!(
        label(cx, "provider-sign-in-claude").as_deref(),
        Some("claude login"),
        "the command, in a block"
    );
    assert_eq!(
        label(cx, "provider-sign-in-claude-copy").as_deref(),
        Some("Copy"),
        "with a labelled button"
    );
    click(cx, "provider-sign-in-claude-copy");
    assert_eq!(copied(cx).as_deref(), Some("claude login"));
    assert!(!present(cx, "provider-install-claude"), "it was found");
}

#[gpui_kit::test]
fn not_found_links_to_how_to_install(cx: &mut TestAppContext) {
    // A `PATH` that holds no program: no agent is found on it.
    let user = User::new().with_env("PATH", "/nonexistent-flottille-test-path");
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (shell, cx) = window(&user, cx);
    let opened = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = opened.clone();
    shell.update(cx, |shell, _| {
        shell.set_opener(Arc::new(move |path: &std::path::Path| {
            seen.lock().unwrap().push(path.display().to_string());
        }))
    });
    open_settings(&shell, cx);

    assert_eq!(
        label(cx, "provider-state-claude").as_deref(),
        Some("Not found on this machine")
    );
    assert!(visible(cx, "provider-install-claude"));
    assert!(
        !present(cx, "provider-sign-in-claude"),
        "nothing to sign in"
    );
    click(cx, "provider-install-claude");
    let page = label(cx, "provider-install-claude").unwrap();
    assert!(page.starts_with("https://"), "{page}");
    assert_eq!(*opened.lock().unwrap(), [page], "the system opens it");
}

/// What `extension.list` says of the automatic subscriptions.
fn subscriptions_listed(user: &User) -> serde_json::Value {
    user.call("extension.list", json!({})).unwrap()["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "autosubscriptions")
        .cloned()
        .unwrap()
}

/// The subjects of the automatic subscriptions: the key of each, its label, its events and those
/// on by default.
const SUBJECTS: [(&str, &str, &[&str], &[&str]); 3] = [
    (
        "created_pull",
        "A pull request the session opens",
        &[
            "checks", "review", "approved", "comment", "merged", "closed",
        ],
        &[
            "checks", "review", "approved", "comment", "merged", "closed",
        ],
    ),
    (
        "branch_pull",
        "The session's pull request opened another way",
        &[
            "checks", "review", "approved", "comment", "merged", "closed",
        ],
        &[
            "checks", "review", "approved", "comment", "merged", "closed",
        ],
    ),
    (
        "ticket",
        "The ticket the session works on",
        &["commented", "status", "assigned", "closed"],
        &[],
    ),
];

#[gpui_kit::test]
fn the_settings_page_has_the_subjects_with_their_events(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);
    tab(&shell, cx, "features");
    until(&shell, cx, "the row is offered", |cx| {
        present(cx, "features-autosubscriptions")
    });
    assert_eq!(
        label(cx, "features-autosubscriptions").as_deref(),
        Some("Automatic subscriptions")
    );
    settle(cx);
    click(cx, "features-autosubscriptions");
    until(&shell, cx, "its page is in", |cx| {
        visible(cx, "autosubscriptions-switch")
    });
    until(&shell, cx, "its subjects are in", |cx| {
        visible(cx, "autosubscriptions-setting-ticket-commented")
    });
    settle(cx);

    // On by default: its switch, then a field per subject, a switch per event, on as by default.
    assert_eq!(
        label(cx, "autosubscriptions-switch-label").as_deref(),
        Some("Subscribe sessions automatically")
    );
    let knob_right = |cx: &mut VisualTestContext, id: &str| {
        let knob = cx.update(|window, _| window.find(format!("{id}-knob")).bounds());
        let track = cx.update(|window, _| window.find(format!("{id}-track")).bounds());
        knob.left() - track.left() > track.size.width / 3.
    };
    assert!(knob_right(cx, "autosubscriptions-switch"));
    for (key, name, events, on) in SUBJECTS {
        let id = format!("autosubscriptions-setting-{key}");
        assert_eq!(label(cx, &format!("{id}-label")).as_deref(), Some(name));
        for event in events {
            let switch = format!("{id}-{event}");
            assert!(visible(cx, &switch), "{switch}");
            assert_eq!(knob_right(cx, &switch), on.contains(event), "{switch}");
        }
    }

    // An event switched on is the daemon's; its switch slides.
    cx.update(|window, cx| window.click("autosubscriptions-setting-ticket-commented", cx));
    until(&shell, cx, "the ticket's events are kept", |_| {
        subscriptions_listed(&user)["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["key"] == "ticket" && s["value"] == json!(["commented"]))
    });
    cx.executor().advance_clock(Duration::from_millis(16));
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "the switch moves"
    );
    // The window takes the daemon's answer at a poll, then the knob slides as time passes.
    until(&shell, cx, "the switch is on", |cx| {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        knob_right(cx, "autosubscriptions-setting-ticket-commented")
    });
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

/// Types `text` in a one-line field, as the user would have.
fn type_line(
    field: &Entity<gpui_kit::base::input::InputState>,
    text: &str,
    cx: &mut VisualTestContext,
) {
    cx.update(|window, cx| field.update(cx, |state, cx| state.set_value(text, window, cx)));
    settle(cx);
}

/// The same, in a text area.
fn type_area(
    field: &Entity<gpui_kit::base::input::TextareaState>,
    text: &str,
    cx: &mut VisualTestContext,
) {
    cx.update(|window, cx| field.update(cx, |state, cx| state.set_value(text, window, cx)));
    settle(cx);
}

#[gpui_kit::test]
fn about_you_is_edited_in_settings(cx: &mut TestAppContext) {
    let user = user();
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, cx);
    tab(&shell, cx, "general");
    let editor = page.read_with(cx, |p, _| p.about().clone());
    until(&shell, cx, "what the user said is read", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    settle(cx);

    // Three labelled fields, nothing saved, so nothing to save and nothing to reset.
    assert!(visible(cx, "about-title"));
    for (field, said) in [
        ("about-name-label", "Name"),
        ("about-pronouns-label", "How to refer to you"),
        ("about-notes-label", "Anything every agent should know"),
    ] {
        assert_eq!(label(cx, field).as_deref(), Some(said), "{field}");
    }
    assert!(!present(cx, "about-save"), "no Save at rest");
    assert!(editor.read_with(cx, |e, _| !e.is_set()));

    // Typed, saved: Save unfolds, the daemon keeps it, a toast says so.
    let (name, pronouns, notes) = editor.read_with(cx, |e, _| {
        (
            e.name_field().cloned().unwrap(),
            e.pronouns_field().cloned().unwrap(),
            e.notes_field().cloned().unwrap(),
        )
    });
    type_line(&name, "Sam", cx);
    type_line(&pronouns, "he/him", cx);
    type_area(&notes, "Backend developer, new to Rust.", cx);
    assert!(editor.read_with(cx, |e, cx| e.is_edited(cx)));
    assert!(visible(cx, "about-save"), "Save unfolds beside the change");
    click(cx, "about-save");
    until(&shell, cx, "what was typed is saved", |cx| {
        editor.read_with(cx, |e, _| e.is_set())
    });
    settle(cx);
    assert!(!present(cx, "about-save"), "no Save once saved");
    let said = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .find(|toast| toast.title() == "About you saved")
            .map(|toast| toast.second_line().map(str::to_owned))
    });
    assert_eq!(
        said,
        Some(Some("Agents started from now on read it.".to_owned()))
    );

    // The value read back: from the daemon, and from the file in the configuration folder.
    let shown = user.call("about.show", json!({})).unwrap();
    assert_eq!(shown["name"], "Sam");
    assert_eq!(shown["pronouns"], "he/him");
    assert_eq!(shown["notes"], "Backend developer, new to Rust.");
    let file = std::fs::read_to_string(user.path("config/flottille/about.toml")).unwrap();
    assert!(file.contains("Sam") && file.contains("he/him"), "{file}");
    assert_eq!(
        editor.read_with(cx, |e, cx| e.typed(cx)),
        Some((
            "Sam".to_owned(),
            "he/him".to_owned(),
            "Backend developer, new to Rust.".to_owned()
        ))
    );

    // Changed from another client, the fields follow.
    user.call("about.set", json!({"name": "Samira"})).unwrap();
    until(&shell, cx, "the change of another client is shown", |cx| {
        editor.read_with(cx, |e, cx| {
            e.typed(cx).is_some_and(|(name, _, _)| name == "Samira")
        })
    });

    // Reset forgets all three.
    click(cx, "about-reset");
    until(&shell, cx, "the fields are reset", |cx| {
        editor.read_with(cx, |e, _| !e.is_set())
    });
    assert_eq!(
        editor.read_with(cx, |e, cx| e.typed(cx)),
        Some((String::new(), String::new(), String::new()))
    );
    assert!(!user.path("config/flottille/about.toml").exists());
}

#[gpui_kit::test]
fn a_provider_whose_models_asked_for_a_sign_in_says_it_on_its_row(cx: &mut TestAppContext) {
    // Its login file is there, and its account is gone: the row would say "signed in".
    let user = User::with_fake_codex().with_env("FLOTTILLE_TEST_LIST_MODELS", "1");
    user.write_providers("[claude]\nenabled = false\n[codex]\nenabled = true\n");
    user.codex_script(json!({"account": serde_json::Value::Null}));
    std::fs::create_dir_all(user.path("home/.codex")).unwrap();
    std::fs::write(user.path("home/.codex/auth.json"), "{}").unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    until(&shell, cx, "the sign-in line is said", |cx| {
        label(cx, "provider-warning-codex")
            .is_some_and(|said| said.contains("sign in again (run codex login)"))
    });
}

#[gpui_kit::test]
fn settings_still_say_the_version_drift_of_a_provider(cx: &mut TestAppContext) {
    let user = User::with_fake_codex();
    user.write_providers(
        "[claude]\nenabled = false\n[codex]\nenabled = true\nallow_unqualified_version = true\n",
    );
    user.codex_of_version("0.163.0");
    std::fs::create_dir_all(user.path("home/.codex")).unwrap();
    std::fs::write(user.path("home/.codex/auth.json"), "{}").unwrap();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, cx);

    until(&shell, cx, "the version drift is said", |cx| {
        label(cx, "provider-warning-codex")
            .is_some_and(|said| said.contains("0.163.0") && said.contains("was checked against"))
    });
}
