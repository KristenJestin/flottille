//! The scenarios of #1462 in the window: the settings of all machines a machine often overrides
//! (`docs/remote.md`, "Settings for all machines"). On All machines, a section for each chooses
//! the value every machine follows, signed with the device key; on a machine, a section it changed
//! is marked ("Changed on server" and Use all machines' value), a value it cannot follow is said,
//! and Appearance and Version say what stays the machine's. On GPUI's test platform in front of
//! real daemons, each with its own temporary HOME and XDG folders and no keyring, a hub served in
//! the test on 127.0.0.1. No window opens on a screen; the servers are invented.

use crate::remote::{Machine, NoKeyring, pc_and_server_on, signed};
use crate::window_any_machine::{click, present, runner, settle, step, until, window};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::about::USE_ALL;
use flottille_window::link::Sign;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

const ALMANAC: &str = "https://almanac.invalid/mcp";

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
    pc_and_server_on(Machine::of(runner()), runner())
}

/// `values` shared from `pc` as the user's device signs them there.
fn share(pc: &Machine, values: Value) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let answer = pc.raw(&signed(
        &pc.device(),
        &format!("window-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        "pc",
        "settings.share",
        json!({"values": values}),
    ));
    assert!(answer["error"].is_null(), "{answer}");
}

fn call(machine: &Machine, method: &str) -> Value {
    machine.user.call(method, json!({})).unwrap()
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

fn top(cx: &mut VisualTestContext, id: &str) -> Option<f32> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map(|found| f32::from(found.bounds().origin.y))
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
    settings(shell, "general", cx);
    choose(shell, "all", cx);
    until(shell, cx, "All machines' sections are shown", |cx| {
        present(cx, "all-connections-title")
    });
    settle(shell, cx);
}

/// `text` typed in the field of the box `id`.
fn type_in(shell: &Entity<Shell>, id: &str, text: &str, cx: &mut VisualTestContext) {
    click(shell, id, cx);
    cx.simulate_input(text);
    settle(shell, cx);
}

#[gpui_kit::test]
fn all_machines_offers_each_setting_a_machine_often_overrides(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);

    all_machines(&shell, cx);

    for section in [
        "all-new-sessions",
        "all-models",
        "all-sessions",
        "all-workspaces",
        "all-updates",
        "all-connections",
    ] {
        assert!(present(cx, &format!("{section}-title")), "{section}");
        // Nothing to save until something changes.
        assert!(!present(cx, &format!("{section}-save")), "{section}");
    }
}

#[gpui_kit::test]
fn a_value_chosen_on_all_machines_reaches_every_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    all_machines(&shell, cx);

    type_in(&shell, "all-days-box", "30", cx);
    click(&shell, "all-sessions-save", cx);
    eventually("the server ends sessions after 30 days", || {
        call(&server, "sessions.show")["days"] == 30
    });
    until(&shell, cx, "the Save folds away", |cx| {
        !present(cx, "all-sessions-save")
    });

    click(&shell, "all-new_session-model", cx);
    click(&shell, "all-new_session-model-claude-recommended", cx);
    click(&shell, "all-new-sessions-save", cx);
    eventually("the server starts new sessions on Claude", || {
        call(&server, "provider.list")["new_session"]["provider"] == "claude"
    });
    // The window takes the answer before the next choice: its controls wait for it.
    until(&shell, cx, "the Save folds away", |cx| {
        !present(cx, "all-new-sessions-save")
    });

    click(&shell, "all-judge-model", cx);
    click(&shell, "all-judge-model-none", cx);
    click(&shell, "all-models-save", cx);
    eventually("the server has no judge", || {
        call(&server, "settings.shared")["all"]["judge"] == "none"
    });
    assert_eq!(call(&server, "settings.shared")["own"], json!([]));
}

#[gpui_kit::test]
fn all_machines_shares_a_connection_with_every_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    pc.user
        .call(
            "connection.add",
            json!({"name": "almanac", "about": "Notes of the team.", "url": ALMANAC}),
        )
        .unwrap();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    all_machines(&shell, cx);
    until(&shell, cx, "the connection is offered", |cx| {
        present(cx, "all-connection-almanac")
    });

    click(&shell, "all-connection-almanac", cx);
    click(&shell, "all-connections-save", cx);

    eventually("the server declares the connection", || {
        call(&server, "connection.list")["connections"]
            .as_array()
            .is_some_and(|all| all.iter().any(|c| c["url"] == ALMANAC))
    });
}

#[gpui_kit::test]
fn a_section_changed_on_a_machine_is_its_own_until_it_uses_all_machines_value(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    share(&pc, json!({"sessions.end_inactive_days": "30"}));
    eventually("the server follows", || {
        call(&server, "sessions.show")["days"] == 30
    });
    server
        .user
        .call("sessions.set", json!({"days": 3}))
        .unwrap();
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "general", cx);
    choose(&shell, "server", cx);
    click(&shell, "settings-tabs-agents", cx);

    until(&shell, cx, "the section is the server's own", |cx| {
        present(cx, "shared-sessions-set")
    });
    settle(&shell, cx);
    assert_eq!(
        label(cx, "shared-sessions-set").as_deref(),
        Some("Changed on server")
    );
    assert_eq!(label(cx, "shared-sessions-reset").as_deref(), Some(USE_ALL));
    // Another section follows all machines: it says nothing.
    assert!(!present(cx, "shared-new-sessions-set"));
    let marked = top(cx, "shared-sessions-set").expect("the mark");

    cx.update(|window, cx| window.click("shared-sessions-reset".to_owned(), cx));
    // The mark folds away over several frames.
    let mut tops = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while present(cx, "shared-sessions-set") {
        assert!(Instant::now() < deadline, "the mark never went");
        step(&shell, cx);
        tops.extend(top(cx, "shared-sessions-set"));
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!tops.is_empty(), "the mark went at once: {marked}");
    eventually("the server follows all machines again", || {
        call(&server, "sessions.show")["days"] == 30
    });
}

#[gpui_kit::test]
fn a_value_a_machine_lacks_is_said_on_its_page(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    // Codex is enabled on no machine of the test.
    share(&pc, json!({"new_session": r#"{"provider":"codex"}"#}));
    eventually("the server says it ignores it", || {
        call(&server, "settings.shared")["ignored"]["new_session"].is_string()
    });
    let (shell, cx) = window(&pc, cx);
    settings(&shell, "general", cx);
    choose(&shell, "server", cx);
    click(&shell, "settings-tabs-agents", cx);

    until(&shell, cx, "the ignored value is said", |cx| {
        present(cx, "shared-new-sessions-ignored")
    });

    let said = label(cx, "shared-new-sessions-ignored").unwrap();
    assert!(said.contains("codex"), "{said}");
    assert!(said.contains("ignored here"), "{said}");
}

#[gpui_kit::test]
fn appearance_and_version_say_what_stays_the_machine_s(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);

    settings(&shell, "general", cx);
    until(&shell, cx, "what stays is said", |cx| {
        present(cx, "settings-stays-appearance")
    });

    let appearance = label(cx, "settings-stays-appearance").unwrap();
    assert!(appearance.contains("Omarchy"), "{appearance}");
    let version = label(cx, "settings-stays-version").unwrap();
    assert!(version.contains("update channel"), "{version}");
}
