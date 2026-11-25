//! The scenarios of #1330 in the window: Settings of All machines (`docs/remote.md`, "Settings for
//! all machines"). The picker on the title's line offers All machines first; there, About you edits
//! the values every machine uses, signed with the device key; on a machine, About you says once
//! that it follows them, a field changed there is that machine's own (its edge, "Changed on
//! server" and Use all machines' value), and a block of the machine alone says "This machine
//! only.". Without a hub, Settings is as it was. On GPUI's test platform in front of real
//! daemons, each with its own temporary HOME and XDG folders and no keyring, a hub served in the
//! test on 127.0.0.1. No window opens on a screen; the people are invented.

use crate::remote::{Machine, NoKeyring, pc_and_server_on};
use crate::window_any_machine::{click, present, runner, settle, step, until, window};

use flottille_window::device::Device;
use flottille_window::link::Connect;
use gpui_design_system::omarchy::Environment;
use gpui_kit::{px, size};

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::about::{AboutEditor, SHARED_LINE, USE_ALL};
use flottille_window::link::Sign;
use flottille_window::settings::MACHINE_ONLY_LINE;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

const SAM: &str = "Sam";

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

fn about(machine: &Machine) -> Value {
    machine.user.call("about.show", json!({})).unwrap()
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

/// Settings, its General tab.
fn general(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    click(shell, "settings", cx);
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    click(shell, "settings-tabs-general", cx);
}

/// The About you of the page shown.
fn editor(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<AboutEditor> {
    shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .settings_page()
            .expect("the settings page")
            .read(cx)
            .about()
            .clone()
    })
}

/// The picker on `key`: `all`, `here` or a machine's name.
fn choose(shell: &Entity<Shell>, key: &str, cx: &mut VisualTestContext) {
    click(shell, "settings-machine", cx);
    click(shell, &format!("settings-machine-{key}"), cx);
    settle(shell, cx);
}

fn type_name(editor: &Entity<AboutEditor>, text: &str, cx: &mut VisualTestContext) {
    let field = editor.read_with(cx, |e, _| e.name_field().cloned().unwrap());
    cx.update(|window, cx| field.update(cx, |state, cx| state.set_value(text, window, cx)));
}

/// The window of a machine with no hub.
fn window_alone<'a>(
    machine: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: machine.user.path("os-release"),
        state_home: machine.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(machine.user.connector());
    let device = Some(Device::in_folder(machine.folder()));
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    until(&shell, cx, "this machine is read", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).read())
    });
    settle(&shell, cx);
    (shell, cx)
}

fn pc_and_server() -> (crate::remote::TestHub, Machine, Machine) {
    pc_and_server_on(Machine::of(runner()), runner())
}

#[gpui_kit::test]
fn the_picker_offers_all_machines_first_with_only_what_is_shared(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    general(&shell, cx);

    click(&shell, "settings-machine", cx);
    assert_eq!(
        label(cx, "settings-machine-all-label").as_deref(),
        Some("All machines")
    );
    let top = |cx: &mut VisualTestContext, id: &str| {
        let id = id.to_owned();
        cx.update(|window, _| window.try_find(id).map(|found| found.bounds().origin.y))
    };
    assert!(
        top(cx, "settings-machine-all") < top(cx, "settings-machine-here"),
        "All machines comes first"
    );
    click(&shell, "settings-machine-all", cx);
    settle(&shell, cx);

    // The picker says it, on the title's line; only the tab of what is shared, only About you.
    assert_eq!(
        label(cx, "settings-machine-field").as_deref(),
        Some("Settings of")
    );
    assert!(present(cx, "about-title"));
    assert!(
        !present(cx, "settings-tabs-agents"),
        "no tab of a machine's own"
    );
    assert!(
        !present(cx, "appearance-title"),
        "no section of a machine's own"
    );
    assert!(!present(cx, "about-shared"), "All machines follows nothing");
    // Back on this machine: its whole Settings.
    choose(&shell, "here", cx);
    assert!(present(cx, "settings-tabs-agents"));
}

#[gpui_kit::test]
fn a_value_saved_on_all_machines_reaches_every_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    general(&shell, cx);
    choose(&shell, "all", cx);
    let editor = editor(&shell, cx);
    until(&shell, cx, "the values of all machines are read", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some_and(|s| s.all.is_some()))
    });

    type_name(&editor, SAM, cx);
    settle(&shell, cx);
    click(&shell, "about-save", cx);

    eventually("the server has the name of all machines", || {
        about(&server)["name"] == SAM
    });
    assert_eq!(about(&pc)["own"], json!([]));
    assert_eq!(about(&server)["own"], json!([]));
}

#[gpui_kit::test]
fn a_machine_says_once_it_follows_all_machines_and_what_is_its_own(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    general(&shell, cx);
    until(&shell, cx, "About you is read", |cx| {
        present(cx, "about-shared")
    });

    assert_eq!(label(cx, "about-shared").as_deref(), Some(SHARED_LINE));
    assert_eq!(
        label(cx, "settings-only-developer").as_deref(),
        Some(MACHINE_ONLY_LINE)
    );
    // What every machine may share says nothing of the kind.
    assert!(!present(cx, "settings-only-about"));
}

#[gpui_kit::test]
fn a_field_changed_on_a_machine_is_its_own_until_it_uses_all_machines_value(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server();
    let (shell, cx) = window(&pc, cx);
    general(&shell, cx);
    choose(&shell, "server", cx);
    let editor = editor(&shell, cx);
    until(&shell, cx, "the server's About you is read", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some_and(|s| s.all.is_some()))
    });
    assert!(!present(cx, "about-name-own-set"), "nothing of its own yet");

    type_name(&editor, "Samuel", cx);
    settle(&shell, cx);
    click(&shell, "about-save", cx);
    until(&shell, cx, "the name is the server's own", |cx| {
        present(cx, "about-name-own-set")
    });
    settle(&shell, cx);

    assert_eq!(
        label(cx, "about-name-own-set").as_deref(),
        Some("Changed on server")
    );
    assert_eq!(label(cx, "about-name-own-reset").as_deref(), Some(USE_ALL));
    assert_eq!(about(&server)["own"], json!(["name"]));
    // Only the field changed is the server's own.
    assert!(!present(cx, "about-pronouns-own-set"));
    let marked = top(cx, "about-pronouns-label").expect("the next field");

    cx.update(|window, cx| window.click("about-name-own-reset".to_owned(), cx));
    // The line folds away with its gap: the next field comes up over several frames, not at once.
    let mut tops = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while present(cx, "about-name-own-set") {
        assert!(Instant::now() < deadline, "the mark never went");
        step(&shell, cx);
        tops.extend(top(cx, "about-pronouns-label"));
        std::thread::sleep(Duration::from_millis(25));
    }
    settle(&shell, cx);
    let folded = top(cx, "about-pronouns-label").expect("the next field");
    assert!(folded < marked, "{folded} < {marked}");
    assert!(
        tops.iter().any(|at| *at < marked && *at > folded),
        "no frame between {marked} and {folded}: {tops:?}"
    );
    assert_eq!(about(&server)["own"], json!([]));
    assert_eq!(about(&server)["name"], about(&server)["all"]["name"]);
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

#[gpui_kit::test]
fn without_a_hub_settings_is_as_it_was(cx: &mut TestAppContext) {
    let user = runner();
    let machine = Machine { user };
    let (shell, cx) = window_alone(&machine, cx);
    general(&shell, cx);
    let editor = editor(&shell, cx);
    until(&shell, cx, "About you is read", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    settle(&shell, cx);

    assert!(!present(cx, "about-shared"));
    assert!(!present(cx, "settings-only-developer"));
    assert!(!present(cx, "settings-machine"), "no picker");
}
