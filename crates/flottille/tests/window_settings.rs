//! The redesigned Settings (#702, `docs/design.md` §10, `docs/reference/gpui.md` rules 13 to 15):
//! five tabs by subject, the Features tab a list of rows that open their page, and each link to a
//! former tab opening the new place. On GPUI's test platform in front of a real daemon in
//! temporary folders, its providers the fake ones. No window opens on a screen.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::settings::SettingsPage;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

/// The tabs, in order.
const TABS: [&str; 5] = ["agents", "approvals", "features", "vault", "general"];

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
fn settings_has_five_tabs_and_opens_on_agents(cx: &mut TestAppContext) {
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
    assert!(width(cx, "rules-reach-") > 100.0, "the line keeps its room");
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
    assert!(visible(cx, "judge-switch"), "the judge replaces its skeleton");
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
    assert!(!present(cx, "provider-sign-in-claude"), "nothing to sign in");
    click(cx, "provider-install-claude");
    let page = label(cx, "provider-install-claude").unwrap();
    assert!(page.starts_with("https://"), "{page}");
    assert_eq!(*opened.lock().unwrap(), [page], "the system opens it");
}
