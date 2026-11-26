//! The scenarios of #1461 and #1525 in the window: the guidance and the rest of "Shared,
//! personal" on All machines and on a machine (`docs/remote.md`, "Settings for all machines"). On
//! All machines the rules for every project, the judge's, the compaction's and Review's guidance,
//! the limits of long tool results, the shade, Specs and the automatic subscriptions are values
//! every machine uses, a change signed with the device key; where none is shared yet, All machines
//! shows Flottille's default and says so. On a machine a value changed there is its own ("Changed
//! on server" and Use all machines' value). Each of these moves. On GPUI's test platform in front
//! of real daemons, each with its own temporary HOME and XDG folders and no keyring, a hub served
//! in the test on 127.0.0.1. No window opens on a screen; the texts are invented.

use crate::remote::{Machine, NoKeyring, pc_and_server_on, signed};
use crate::window_any_machine::{click, present, runner, settle, step, until, window};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::about::USE_ALL;
use flottille_window::all_settings::{AllSettings, DEFAULT_SAID, Shown};
use flottille_window::link::Sign;
use gpui_kit::base::input::TextareaState;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

const RULES: &str = "Small commits, tests first.";

/// The sections this part adds to All machines.
const SECTIONS: [&str; 7] = [
    "all-rules",
    "all-guidance",
    "all-review",
    "all-tool-results",
    "all-appearance",
    "all-specs",
    "all-subscriptions",
];

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

fn pc_and_server() -> (crate::remote::TestHub, Machine, Machine) {
    // Daemons that cut tool results as they do for everyone: the harness's own file would be
    // each machine's own limit.
    pc_and_server_on(
        Machine::of(runner().with_cut_results()),
        runner().with_cut_results(),
    )
}

/// `values` shared from `pc` as the user's device signs them there.
fn share(pc: &Machine, values: Value) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let answer = pc.raw(&signed(
        &pc.device(),
        &format!("guidance-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        "pc",
        "settings.share",
        json!({"values": values}),
    ));
    assert!(answer["error"].is_null(), "{answer}");
}

fn call(machine: &Machine, method: &str, params: Value) -> Value {
    machine.user.call(method, params).unwrap()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// How tall the element `id` is drawn.
fn height(cx: &mut VisualTestContext, id: &str) -> Option<f32> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map(|found| f32::from(found.bounds().size.height))
    })
}

/// Settings, its tab `tab`.
fn settings(shell: &Entity<Shell>, tab: &str, cx: &mut VisualTestContext) {
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

/// All machines, once its values are read.
fn all_machines(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    // Tall enough for every section of All machines to be on screen, its last ones clicked.
    cx.simulate_resize(size(px(1280.), px(9000.)));
    settings(shell, "general", cx);
    choose(shell, "all", cx);
    until(shell, cx, "All machines' sections are shown", |cx| {
        present(cx, "all-subscriptions-title")
    });
    let all = all_settings(shell, cx);
    until(shell, cx, "the values are read", |cx| {
        all.read_with(cx, |all, _| {
            all.shown().is_some_and(|shown| shown.all.is_some())
        })
    });
    settle(shell, cx);
}

fn all_settings(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<AllSettings> {
    shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .settings_page()
            .expect("the settings page")
            .read(cx)
            .all_settings()
            .clone()
    })
}

fn text_field(
    shell: &Entity<Shell>,
    name: &str,
    cx: &mut VisualTestContext,
) -> Entity<TextareaState> {
    let all = all_settings(shell, cx);
    all.read_with(cx, |all, _| all.text_field(name).cloned())
        .unwrap_or_else(|| panic!("no field for {name}"))
}

fn put_text(field: &Entity<TextareaState>, text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| field.update(cx, |state, cx| state.set_value(text, window, cx)));
}

fn text_of(field: &Entity<TextareaState>, cx: &mut VisualTestContext) -> String {
    field.read_with(cx, |state, _| state.value().to_string())
}

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| toast.title().to_owned())
            .collect()
    })
}

#[gpui_kit::test]
fn all_machines_offers_the_guidance_and_the_rest(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);

    all_machines(&shell, cx);

    for section in SECTIONS {
        assert!(present(cx, &format!("{section}-title")), "{section}");
        // Nothing to save until something changes.
        assert!(!present(cx, &format!("{section}-save")), "{section}");
    }
    for control in [
        "all-rules-box",
        "all-judge-guidance-box",
        "all-compaction-guidance-box",
        "all-review-guidance-box",
        "all-review-guidance-bugs-box",
        "all-review-passes-security",
        "all-tool-results-limit-box",
        "all-tool_results-unchanged-true",
        "all-appearance-shade-light",
        "all-specs-mode-ticket",
        "all-specs-language-box",
        "all-autosubscriptions-ticket-closed",
    ] {
        assert!(present(cx, control), "{control}");
    }
}

#[gpui_kit::test]
fn the_rules_saved_on_all_machines_reach_every_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    all_machines(&shell, cx);
    let field = text_field(&shell, "rules", cx);

    put_text(&field, RULES, cx);
    until(&shell, cx, "the Save unfolds", |cx| {
        present(cx, "all-rules-save")
    });
    // Unfolded whole before it is clicked.
    settle(&shell, cx);
    click(&shell, "all-rules-save", cx);

    eventually("the server follows the rules of all machines", || {
        let shown = call(&server, "rules.show", json!({}));
        shown["text"] == RULES && shown["source"] == "all"
    });
    until(&shell, cx, "the Save folds away", |cx| {
        !present(cx, "all-rules-save")
    });
    assert_eq!(call(&pc, "settings.shared", json!({}))["own"], json!([]));
    assert!(toasts(&shell, cx).contains(&"Saved for all machines".to_owned()));
}

#[gpui_kit::test]
fn the_rest_chosen_on_all_machines_reaches_every_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    all_machines(&shell, cx);

    click(&shell, "all-appearance-shade-light", cx);
    click(&shell, "all-appearance-save", cx);
    eventually("the server is light", || {
        call(&server, "appearance.show", json!({}))["shade"] == "light"
    });
    until(&shell, cx, "the Save folds away", |cx| {
        !present(cx, "all-appearance-save")
    });

    click(&shell, "all-review-passes-security", cx);
    let judge = text_field(&shell, "review.guidance.bugs", cx);
    put_text(&judge, "Hunt the off-by-one.", cx);
    settle(&shell, cx);
    click(&shell, "all-review-save", cx);
    eventually(
        "the server reviews security, and hunts the off-by-one",
        || {
            let listed = call(&server, "extension.list", json!({}));
            let review = listed["extensions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|extension| extension["name"] == "review")
                .unwrap()
                .clone();
            let passes = review["settings"]
                .as_array()
                .unwrap()
                .iter()
                .find(|setting| setting["key"] == "passes")
                .unwrap()["value"]
                .clone();
            passes == json!(["spec", "bugs", "standards", "security"])
                && call(&server, "review.show", json!({"part": "bugs"}))["text"]
                    == "Hunt the off-by-one."
        },
    );
}

/// Point 3 of #1525: where no machine shared a value, All machines shows Flottille's default,
/// said as such, never this machine's own.
#[gpui_kit::test]
fn all_machines_shows_the_default_where_nothing_is_shared(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    call(
        &pc,
        "rules.set",
        json!({"text": "Rules of this machine alone."}),
    );
    call(&pc, "judge.set", json!({"text": "Ask about every write."}));
    let default = call(&pc, "rules.show", json!({}))["default"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, cx) = window(&pc, cx);

    all_machines(&shell, cx);

    let rules = text_field(&shell, "rules", cx);
    assert_eq!(text_of(&rules, cx), default);
    let judge = text_field(&shell, "judge.guidance", cx);
    assert!(!text_of(&judge, cx).contains("every write"));
    for name in ["rules", "judge-guidance", "appearance-shade", "specs-mode"] {
        assert_eq!(
            label(cx, &format!("all-{name}-default")).as_deref(),
            Some(DEFAULT_SAID),
            "{name}"
        );
    }
    // Nothing differs from what is shown: nothing to save.
    assert!(!present(cx, "all-rules-save"));
}

/// Point 2 of #1525: a hub of a version that keeps no value of all machines leaves the change on
/// this machine, and the window says so.
#[gpui_kit::test]
fn a_save_to_a_hub_that_keeps_none_says_it_stayed_here(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    all_machines(&shell, cx);
    let field = text_field(&shell, "rules", cx);
    put_text(&field, RULES, cx);
    settle(&shell, cx);
    assert!(present(cx, "all-rules-save"));
    let all = all_settings(&shell, cx);
    let mut kept = all.read_with(cx, |all, _| all.shown().cloned().unwrap());
    kept.all
        .as_mut()
        .unwrap()
        .insert("rules".into(), RULES.into());
    kept.hub = Some("old-hub".into());

    // The answer of `settings.share` from a hub of before the values of all machines.
    cx.update(|window, cx| {
        all.update(cx, |all, cx| {
            all.answered(Ok::<Shown, String>(kept), Some(true), window, cx)
        })
    });
    settle(&shell, cx);

    let said = toasts(&shell, cx);
    assert!(
        said.contains(&"Saved on this machine only".to_owned()),
        "{said:?}"
    );
    assert!(
        !said.contains(&"Saved for all machines".to_owned()),
        "{said:?}"
    );
}

/// Point 4 of #1525: the line that says a value is the default folds away once a machine shares
/// one, over several frames, its gap with it.
#[gpui_kit::test]
fn the_default_line_folds_away_once_a_value_is_shared(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    all_machines(&shell, cx);
    assert!(present(cx, "all-rules-default"));
    let tall = height(cx, "all-rules-default-block").expect("the line's block");

    share(&pc, json!({"rules": RULES}));
    // The window reads the values again at the news of the change.
    let mut heights = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while present(cx, "all-rules-default") {
        assert!(Instant::now() < deadline, "the line never went");
        step(&shell, cx);
        heights.extend(height(cx, "all-rules-default-block"));
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        heights.iter().any(|h| *h > 0.5 && *h < tall - 0.5),
        "the line went at once: {tall} then {heights:?}"
    );
    let rules = text_field(&shell, "rules", cx);
    assert_eq!(text_of(&rules, cx), RULES);
}

/// Point 4 of #1525: the Save of a section unfolds once its text differs, and folds away when it
/// is the same again.
#[gpui_kit::test]
fn the_save_of_a_guidance_unfolds_and_folds_away(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    all_machines(&shell, cx);
    let field = text_field(&shell, "judge.guidance", cx);
    let before = text_of(&field, cx);

    put_text(&field, "Let a read of the project through.", cx);
    step(&shell, cx);
    let opening = height(cx, "all-guidance-save-block");
    settle(&shell, cx);
    let open = height(cx, "all-guidance-save-block").expect("the Save");

    assert!(
        opening.is_some_and(|h| h < open),
        "the Save came at once: {opening:?} then {open}"
    );
    put_text(&field, &before, cx);
    until(&shell, cx, "the Save folds away", |cx| {
        !present(cx, "all-guidance-save")
    });
}

/// Point 4 of #1525: a guidance changed on a machine is its own there, its mark unfolds, and Use
/// all machines' value folds it away over several frames.
#[gpui_kit::test]
fn rules_changed_on_a_machine_are_its_own_until_it_uses_all_machines(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    share(&pc, json!({"rules": RULES}));
    eventually("the server follows", || {
        call(&server, "rules.show", json!({}))["source"] == "all"
    });
    call(
        &server,
        "rules.set",
        json!({"text": "Rules of the server."}),
    );
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "general", cx);
    choose(&shell, "server", cx);
    click(&shell, "settings-tabs-agents", cx);

    until(&shell, cx, "the rules are the server's own", |cx| {
        present(cx, "shared-rules-set")
    });
    settle(&shell, cx);
    assert_eq!(
        label(cx, "shared-rules-set").as_deref(),
        Some("Changed on server")
    );
    assert_eq!(label(cx, "shared-rules-reset").as_deref(), Some(USE_ALL));
    assert!(!present(cx, "shared-tool-results-set"));

    let tall = height(cx, "shared-rules-block").expect("the mark's block");
    cx.update(|window, cx| window.click("shared-rules-reset".to_owned(), cx));
    let mut heights = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while present(cx, "shared-rules-set") {
        assert!(Instant::now() < deadline, "the mark never went");
        step(&shell, cx);
        heights.extend(height(cx, "shared-rules-block"));
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        heights.iter().any(|h| *h > 0.5 && *h < tall - 0.5),
        "the mark went at once: {tall} then {heights:?}"
    );
    eventually("the server follows all machines again", || {
        call(&server, "rules.show", json!({}))["text"] == RULES
    });
}

/// Point 1 of #1525 in the window: a shade a version of before wrote is no choice of the server's,
/// and its Appearance is not marked.
#[gpui_kit::test]
fn a_shade_written_before_is_not_marked_as_the_machine_s(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    std::fs::write(
        server.user.path("config/flottille/appearance.toml"),
        "theme = \"omarchy\"\nshade = \"dark\"\n",
    )
    .unwrap();
    share(&pc, json!({"appearance.shade": "light"}));
    eventually("the server is light", || {
        call(&server, "appearance.show", json!({}))["shade"] == "light"
    });
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "general", cx);
    choose(&shell, "server", cx);
    until(&shell, cx, "what stays is said", |cx| {
        present(cx, "settings-stays-appearance")
    });
    settle(&shell, cx);

    assert!(!present(cx, "shared-appearance-set"));
}

/// Point 5 of #1525's review: a section whose second value would be refused shares none of its
/// values, and says why.
#[gpui_kit::test]
fn a_section_with_a_refused_value_shares_none_of_them(cx: &mut TestAppContext) {
    let (hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    all_machines(&shell, cx);
    let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
    let judge = text_field(&shell, "judge.guidance", cx);
    put_text(&judge, "Let a read of the project through.", cx);
    let compaction = text_field(&shell, "compaction.guidance", cx);
    put_text(&compaction, &format!("Keep {token} for later."), cx);
    until(&shell, cx, "the Save unfolds", |cx| {
        present(cx, "all-guidance-save")
    });
    settle(&shell, cx);

    click(&shell, "all-guidance-save", cx);

    until(&shell, cx, "the refusal is said", |cx| {
        toasts(&shell, cx).contains(&"Not saved".to_owned())
    });
    let all = call(&pc, "settings.shared", json!({}))["all"].clone();
    assert!(all.get("judge.guidance").is_none(), "{all}");
    let kept = hub.with_hub(|hub| serde_json::to_string(hub.shared()).unwrap());
    assert!(!kept.contains("judge.guidance"), "{kept}");
    // What was typed stays, to be corrected.
    assert_eq!(text_of(&judge, cx), "Let a read of the project through.");
}

/// Point 3 of #1525's review (#1462): a shade chosen on a machine stays its own, even the same as
/// all machines', and its page marks it once all machines change theirs.
#[gpui_kit::test]
fn a_shade_chosen_on_a_machine_stays_marked_when_all_machines_change(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    share(&pc, json!({"appearance.shade": "light"}));
    eventually("the server is light", || {
        call(&server, "appearance.show", json!({}))["shade"] == "light"
    });
    call(&server, "appearance.set", json!({"shade": "light"}));
    share(&pc, json!({"appearance.shade": "dark"}));
    eventually("all machines are dark", || {
        call(&server, "settings.shared", json!({}))["all"]["appearance.shade"] == "dark"
    });
    let (shell, cx) = window(&pc, cx);

    settings(&shell, "general", cx);
    choose(&shell, "server", cx);

    until(&shell, cx, "the server's shade is marked", |cx| {
        present(cx, "shared-appearance-set")
    });
    assert_eq!(
        call(&server, "appearance.show", json!({}))["shade"],
        "light"
    );
}

/// Point 3 of #1525's review: colours chosen in the window leave the shade to all machines: the
/// window sends only what the user changed.
#[gpui_kit::test]
fn colours_chosen_in_the_window_leave_the_shade_to_all_machines(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    // An Omarchy desktop, whose theme the window follows first: Flottille's colours are a choice.
    std::fs::write(pc.user.path("os-release"), "NAME=Test\nID=omarchy\n").unwrap();
    let colours = gpui_design_system::omarchy::Environment {
        os_release: pc.user.path("os-release"),
        state_home: pc.user.path("state"),
    }
    .colors_path();
    std::fs::create_dir_all(colours.parent().unwrap()).unwrap();
    std::fs::write(
        &colours,
        "background = \"#0e1116\"\nforeground = \"#c9d1d9\"\naccent = \"#f0883e\"\n",
    )
    .unwrap();
    share(&pc, json!({"appearance.shade": "light"}));
    eventually("this machine is light", || {
        call(&pc, "appearance.show", json!({}))["shade"] == "light"
    });
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "general", cx);
    until(&shell, cx, "the appearance is read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .settings_page()
                .is_some_and(|page| page.read(cx).appearance_known())
        })
    });
    settle(&shell, cx);

    click(&shell, "appearance-theme-flottille", cx);
    eventually("the colours are Flottille's", || {
        call(&pc, "appearance.show", json!({}))["theme"] == "flottille"
    });
    share(&pc, json!({"appearance.shade": "dark"}));

    eventually("this machine follows all machines' dark", || {
        call(&pc, "appearance.show", json!({}))["shade"] == "dark"
    });
    assert_eq!(call(&pc, "settings.shared", json!({}))["own"], json!([]));
}

/// The colour of the edge of the box `id`, as last painted.
fn edge(cx: &mut VisualTestContext, id: &str) -> Option<gpui_kit::Hsla> {
    cx.update(|window, _| {
        let found = window.try_find(id.to_owned())?;
        let bounds = found.bounds().scale(window.scale_factor());
        window
            .painted_quads()
            .iter()
            .find(|quad| {
                (quad.bounds.origin.x.0 - bounds.origin.x.0).abs() < 1.0
                    && (quad.bounds.origin.y.0 - bounds.origin.y.0).abs() < 1.0
                    && (quad.bounds.size.width.0 - bounds.size.width.0).abs() < 1.0
                    && quad.border_color.a > 0.0
            })
            .map(|quad| quad.border_color)
    })
}

/// Point 9 of #1525's review: the edge of a guidance's box moves to the focus colour as the field
/// takes the focus, over several frames.
#[gpui_kit::test]
fn the_edge_of_a_guidance_moves_to_the_focus_colour(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    all_machines(&shell, cx);
    let field = text_field(&shell, "rules", cx);
    let rest = edge(cx, "all-rules-box").expect("the box's edge");

    field.update_in(cx, |field, window, cx| field.focus(window, cx));
    let mut edges = Vec::new();
    for _ in 0..30 {
        step(&shell, cx);
        edges.extend(edge(cx, "all-rules-box"));
    }

    let focused = *edges.last().expect("the box's edge");
    assert!(focused != rest, "the edge stays at rest: {rest:?}");
    assert!(
        edges.iter().any(|edge| *edge != rest && *edge != focused),
        "the edge moved at once: {rest:?} then {edges:?}"
    );
}
