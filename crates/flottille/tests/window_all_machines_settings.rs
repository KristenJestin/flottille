//! The scenarios of #1461 in the window: the guidance and the rest of "Shared, personal" on All
//! machines and on a machine (`docs/remote.md`, "Settings for all machines"). On All machines the
//! rules for every project, the judge's, the compaction's and Review's guidance, the limits of long
//! tool results, Specs, the automatic subscriptions, Review's passes and the shade are the values
//! every machine uses, a change signed with the device key; on a machine each says once that it
//! follows them, a value changed there is the machine's own (its edge, "Changed on server" and Use
//! all machines' value). Each of these moves. On GPUI's test platform in front of real daemons,
//! each with its own temporary HOME and XDG folders and no keyring, a hub served in the test on
//! 127.0.0.1. No window opens on a screen; the texts are invented.

use crate::remote::{Machine, NoKeyring, pc_and_server_on, signed};
use crate::window_any_machine::{click, present, runner, settle, step, until, window};

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::about::{SHARED_LINE, USE_ALL};
use flottille_window::link::Sign;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, VisualTestContext};
use serde_json::{Value, json};

const RULES: &str = "Small commits, tests first.";

/// How this test's window signs, as the device of `machine`.
fn signing(machine: &Machine) -> Sign {
    let folder = machine.folder();
    Arc::new(move |target, request| {
        let key = DeviceKey::load(&NoKeyring, &folder)
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .ok_or_else(|| std::io::Error::other("no device key"))?;
        let signed = Request::sign(
            &key,
            &request.id,
            target,
            &request.method,
            &request.params,
            SystemTime::now(),
        )
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        serde_json::to_value(signed).map_err(std::io::Error::other)
    })
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn pc_and_server() -> (crate::remote::TestHub, Machine, Machine) {
    // Daemons that cut tool results as they do for everyone: the harness's own file would be
    // each machine's own limit.
    pc_and_server_on(
        Machine::of(runner().with_cut_results()),
        runner().with_cut_results(),
    )
}

/// Settings, the tab `tab`.
fn tab(shell: &Entity<Shell>, tab: &str, cx: &mut VisualTestContext) {
    click(shell, "settings", cx);
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    click(shell, &format!("settings-tabs-{tab}"), cx);
}

/// The picker on `key`: `all`, `here` or a machine's name.
fn choose(shell: &Entity<Shell>, key: &str, cx: &mut VisualTestContext) {
    click(shell, "settings-machine", cx);
    click(shell, &format!("settings-machine-{key}"), cx);
    settle(shell, cx);
}

fn page(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Entity<flottille_window::settings::SettingsPage> {
    shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .settings_page()
            .expect("the settings page")
            .clone()
    })
}

fn type_in(
    field: &Entity<gpui_kit::base::input::TextareaState>,
    text: &str,
    cx: &mut VisualTestContext,
) {
    cx.update(|window, cx| field.update(cx, |state, cx| state.set_value(text, window, cx)));
}

/// Where the element `id` is drawn, from the top.
fn top(cx: &mut VisualTestContext, id: &str) -> Option<f32> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map(|found| f32::from(found.bounds().origin.y))
    })
}

fn show(machine: &Machine, method: &str, params: Value) -> Value {
    machine.user.call(method, params).unwrap()
}

#[gpui_kit::test]
fn all_machines_offers_the_tabs_that_hold_a_shared_setting(cx: &mut gpui_kit::TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    tab(&shell, "general", cx);

    choose(&shell, "all", cx);

    for shown in ["agents", "approvals", "features", "general"] {
        assert!(present(cx, &format!("settings-tabs-{shown}")), "{shown}");
    }
    for own in ["vault", "machines"] {
        assert!(
            !present(cx, &format!("settings-tabs-{own}")),
            "{own} is the machine's own"
        );
    }
    click(&shell, "settings-tabs-agents", cx);
    assert!(present(cx, "rules-text-"), "the rules for every project");
    assert!(present(cx, "tool-results-limit"), "the long tool results");
    assert!(
        !present(cx, "providers-title"),
        "no section of a machine's own"
    );
    click(&shell, "settings-tabs-approvals", cx);
    assert!(present(cx, "judge-guidance-text"));
    click(&shell, "settings-tabs-features", cx);
    for shared in [
        "specs-setting-language-label",
        "autosubscriptions-setting-created_pull-label",
        "review-setting-passes-label",
        "compaction-guidance-text",
    ] {
        assert!(present(cx, shared), "{shared}");
    }
    click(&shell, "settings-tabs-general", cx);
    assert!(present(cx, "appearance-shade"), "the shade");
    assert!(
        !present(cx, "appearance-theme"),
        "the colours are the machine's own"
    );
}

#[gpui_kit::test]
fn the_rules_saved_on_all_machines_reach_every_machine(cx: &mut gpui_kit::TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    tab(&shell, "agents", cx);
    choose(&shell, "all", cx);
    let rules = page(&shell, cx).read_with(cx, |page, _| page.rules().clone());
    until(&shell, cx, "the rules are read", |cx| {
        rules.read_with(cx, |e, _| e.shown().is_some_and(|s| s.all.is_some()))
    });
    let field = rules.read_with(cx, |e, _| e.field().cloned().expect("the field"));

    type_in(&field, RULES, cx);
    settle(&shell, cx);
    click(&shell, "rules-save-", cx);

    eventually("the server follows the rules of all machines", || {
        let shown = show(&server, "rules.show", json!({}));
        shown["text"] == RULES && shown["source"] == "all"
    });
    assert_eq!(show(&pc, "rules.show", json!({}))["own"], false);
}

#[gpui_kit::test]
fn a_machine_follows_the_rules_of_all_machines_until_it_writes_its_own(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    let shared = pc.raw(&signed(
        &pc.device(),
        "share-rules",
        "pc",
        "settings.share",
        json!({"name": "rules", "value": RULES}),
    ));
    assert!(shared["error"].is_null(), "{shared}");
    tab(&shell, "agents", cx);
    choose(&shell, "server", cx);
    let rules = page(&shell, cx).read_with(cx, |page, _| page.rules().clone());
    until(&shell, cx, "the server's rules are all machines'", |cx| {
        rules.read_with(cx, |e, _| e.shown().is_some_and(|s| s.text == RULES))
    });
    settle(&shell, cx);
    assert_eq!(label(cx, "rules-shared").as_deref(), Some(SHARED_LINE));
    assert!(!present(cx, "rules-own-set"), "nothing of its own yet");

    let field = rules.read_with(cx, |e, _| e.field().cloned().expect("the field"));
    type_in(&field, "Rules of the server alone.", cx);
    settle(&shell, cx);
    click(&shell, "rules-save-", cx);
    until(&shell, cx, "the rules are the server's own", |cx| {
        present(cx, "rules-own-set")
    });
    settle(&shell, cx);

    assert_eq!(
        label(cx, "rules-own-set").as_deref(),
        Some("Changed on server")
    );
    assert_eq!(label(cx, "rules-own-reset").as_deref(), Some(USE_ALL));
    assert_eq!(show(&server, "rules.show", json!({}))["source"], "global");
    // pc keeps the rules of all machines.
    assert_eq!(show(&pc, "rules.show", json!({}))["text"], RULES);

    cx.update(|window, cx| window.click("rules-own-reset".to_owned(), cx));
    // The mark folds away with its gap over several frames, not at once.
    let marked = top(cx, "rules-text-").expect("the field");
    let mut tops = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while present(cx, "rules-own-set") {
        assert!(Instant::now() < deadline, "the mark never went");
        step(&shell, cx);
        tops.extend(top(cx, "rules-text-"));
        std::thread::sleep(Duration::from_millis(25));
    }
    settle(&shell, cx);
    assert_eq!(show(&server, "rules.show", json!({}))["source"], "all");
    assert_eq!(show(&server, "rules.show", json!({}))["text"], RULES);
    let folded = top(cx, "rules-text-").expect("the field");
    assert!(
        tops.iter().all(|at| *at <= marked) && folded <= marked,
        "{folded} {marked} {tops:?}"
    );
}

#[gpui_kit::test]
fn the_judge_and_the_compaction_guidance_are_saved_for_all_machines(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    tab(&shell, "approvals", cx);
    choose(&shell, "all", cx);
    let judge = page(&shell, cx).read_with(cx, |page, _| page.judge_guidance().clone());
    until(&shell, cx, "the judge's guidance is read", |cx| {
        judge.read_with(cx, |e, _| e.shown().is_some_and(|s| s.all.is_some()))
    });
    let field = judge.read_with(cx, |e, _| e.field().cloned().expect("the field"));
    type_in(&field, "Let a read of the project through.", cx);
    settle(&shell, cx);
    click(&shell, "judge-guidance-save", cx);
    eventually("the server judges by the guidance of all machines", || {
        let shown = show(&server, "judge.show", json!({}));
        shown["text"] == "Let a read of the project through." && shown["source"] == "all"
    });

    click(&shell, "settings-tabs-features", cx);
    let compaction = page(&shell, cx).read_with(cx, |page, _| page.compaction_guidance().clone());
    until(&shell, cx, "the compaction's guidance is read", |cx| {
        compaction.read_with(cx, |e, _| e.shown().is_some_and(|s| s.all.is_some()))
    });
    let field = compaction.read_with(cx, |e, _| e.field().cloned().expect("the field"));
    type_in(&field, "Keep the decisions.", cx);
    settle(&shell, cx);
    click(&shell, "compaction-guidance-save", cx);
    eventually(
        "the server compacts by the guidance of all machines",
        || {
            let shown = show(&server, "compaction.show", json!({}));
            shown["guidance"]["text"] == "Keep the decisions."
                && shown["guidance"]["source"] == "all"
        },
    );
}

#[gpui_kit::test]
fn the_limits_of_tool_results_are_shared_and_a_machine_may_keep_its_own(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    tab(&shell, "agents", cx);
    choose(&shell, "all", cx);
    let settings = page(&shell, cx);
    until(&shell, cx, "the limits are read", |cx| {
        settings.read_with(cx, |page, _| page.tool_results_known())
    });
    settle(&shell, cx);

    click(&shell, "tool-results-limit", cx);
    click(&shell, "tool-results-limit-16000", cx);
    eventually("the server cuts at the limit of all machines", || {
        show(&server, "tool_results.show", json!({}))["limit"] == 16_000
    });
    click(&shell, "tool-results-unchanged-switch", cx);
    eventually("the server says a file is unchanged", || {
        show(&server, "tool_results.show", json!({}))["unchanged"] == true
    });
    assert_eq!(
        show(&server, "tool_results.show", json!({}))["own"],
        json!([])
    );

    // On the server the limit is its own once it changes it, and the way back restores all
    // machines'.
    choose(&shell, "server", cx);
    until(&shell, cx, "the server's limits are read", |cx| {
        settings.read_with(cx, |page, _| page.tool_results_limit() == Some(16_000))
    });
    settle(&shell, cx);
    assert_eq!(
        label(cx, "tool-results-shared").as_deref(),
        Some(SHARED_LINE)
    );
    click(&shell, "tool-results-limit", cx);
    click(&shell, "tool-results-limit-4000", cx);
    until(&shell, cx, "the limit is the server's own", |cx| {
        present(cx, "tool-results-limit-own-set")
    });
    assert_eq!(
        label(cx, "tool-results-limit-own-set").as_deref(),
        Some("Changed on server")
    );
    assert!(!present(cx, "tool-results-unchanged-own-set"));
    assert_eq!(show(&pc, "tool_results.show", json!({}))["limit"], 16_000);
    click(&shell, "tool-results-limit-own-reset", cx);
    eventually("the server follows all machines again", || {
        show(&server, "tool_results.show", json!({}))["limit"] == 16_000
    });
}

#[gpui_kit::test]
fn the_shade_is_shared_and_a_machine_may_keep_its_own(cx: &mut gpui_kit::TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    tab(&shell, "general", cx);
    choose(&shell, "all", cx);
    click(&shell, "appearance-shade-light", cx);
    eventually("the server follows the shade of all machines", || {
        show(&server, "appearance.show", json!({}))["shade"] == "light"
    });
    assert_eq!(
        show(&server, "appearance.show", json!({}))["own"],
        json!([])
    );

    // The server's own shade wins there only.
    show(&server, "appearance.set", json!({"shade": "dark"}));
    choose(&shell, "server", cx);
    until(&shell, cx, "the shade is the server's own", |cx| {
        present(cx, "appearance-shade-own-set")
    });
    assert_eq!(
        label(cx, "appearance-shade-own-set").as_deref(),
        Some("Changed on server")
    );
    assert_eq!(show(&pc, "appearance.show", json!({}))["shade"], "light");
    click(&shell, "appearance-shade-own-reset", cx);
    eventually("the server follows all machines again", || {
        show(&server, "appearance.show", json!({}))["shade"] == "light"
    });
}

#[gpui_kit::test]
fn specs_the_subscriptions_and_review_are_shared_and_overridden_by_a_machine(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    tab(&shell, "features", cx);
    choose(&shell, "all", cx);
    until(&shell, cx, "the settings of Specs are listed", |cx| {
        present(cx, "specs-setting-mode")
    });
    settle(&shell, cx);

    click(&shell, "specs-setting-mode-ticket", cx);
    eventually("the server writes specs to the ticket", || {
        let listed = show(&server, "extension.list", json!({}));
        let specs = listed["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|extension| extension["name"] == "specs")
            .unwrap();
        specs["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|setting| setting["key"] == "mode" && setting["value"] == "ticket")
    });

    // On the server a setting it sets itself says so, and the way back restores all machines'.
    choose(&shell, "server", cx);
    until(&shell, cx, "the server's page follows all machines", |cx| {
        present(cx, "specs-settings-shared")
    });
    click(&shell, "specs-setting-mode-internal", cx);
    until(&shell, cx, "the mode is the server's own", |cx| {
        present(cx, "specs-setting-mode-own-set")
    });
    assert_eq!(
        label(cx, "specs-setting-mode-own-set").as_deref(),
        Some("Changed on server")
    );
    click(&shell, "specs-setting-mode-own-reset", cx);
    eventually("the server follows all machines again", || {
        let listed = show(&server, "extension.list", json!({}));
        listed["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|extension| extension["name"] == "specs")
            .unwrap()["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|setting| {
                setting["key"] == "mode" && setting["value"] == "ticket" && setting["set"] == false
            })
    });
}

#[gpui_kit::test]
fn without_a_hub_no_section_says_it_follows_anything(cx: &mut gpui_kit::TestAppContext) {
    let machine = Machine { user: runner() };
    let (shell, cx) = crate::window_all_machines::window_alone(&machine, cx);
    tab(&shell, "agents", cx);
    settle(&shell, cx);

    assert!(!present(cx, "rules-shared"));
    assert!(!present(cx, "tool-results-shared"));
    assert!(!present(cx, "settings-machine"), "no picker");
    tab(&shell, "general", cx);
    assert!(!present(cx, "appearance-shared"));
}
