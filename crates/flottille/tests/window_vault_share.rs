//! Settings, Vault, a row's Share with… (#832, `docs/remote.md`, "Vault entries"), on GPUI's test
//! platform in front of two real daemons paired through a hub served in the test: the dialog lists
//! the user's machines online, choosing one shares the entry, signed with the device key of the
//! window's machine, and a toast says the result; on the other machine the entry's row says where
//! it was shared from. The keyrings are the daemons' memory; no window opens on a screen.

use crate::common;
use crate::remote;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::{Connect, Sign};
use flottille_window::tree::SWITCHER;
use flottille_window::vault::{NO_MACHINE, VaultSection};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use remote::{Machine, NoKeyring, pair, pc_and_server};
use serde_json::json;

const SECRET: &str = "w1ndow-sh4re-secret";
/// The secret the server sets by hand over its copy.
const SECOND: &str = "s3rver-0wn-secret";

fn window<'a>(
    machine: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let connect: Connect = Arc::new(machine.user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1600.)));
    (shell, cx)
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(40);
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
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn click(id: &str, cx: &mut VisualTestContext) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
}

fn shown(id: &str, cx: &mut VisualTestContext) -> bool {
    let id = id.to_owned();
    cx.update(|window, _| window.try_find(id).is_some_and(|found| found.visible()))
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

fn open_vault(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<VaultSection> {
    click("settings", cx);
    until(shell, cx, "the settings page is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().is_some())
    });
    settle(cx);
    click("settings-tabs-vault", cx);
    let section = shell
        .read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .settings_page()
                .map(|page| page.read(cx).vault().clone())
        })
        .unwrap();
    until(shell, cx, "the entries are read", |cx| {
        section.read_with(cx, |s, _| s.entries().is_some_and(|e| !e.is_empty()))
    });
    settle(cx);
    section
}

/// The key folder of the machine whose test signs now: the window's signer is given once per
/// process, so the tests that share take turns and say whose folder it reads.
static SIGNING_FOR: Mutex<Option<PathBuf>> = Mutex::new(None);
static TURN: Mutex<()> = Mutex::new(());

/// Signs with the device key the clients of `machine` keep, as the binary's window does, until the
/// returned turn is dropped.
fn signing(machine: &Machine) -> MutexGuard<'static, ()> {
    let turn = TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *SIGNING_FOR.lock().unwrap() = Some(machine.folder());
    let sign: Sign = Arc::new(move |target, request| {
        let folder = SIGNING_FOR
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| std::io::Error::other("no machine signs"))?;
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
    });
    flottille_window::link::sign_with(sign);
    turn
}

#[gpui_kit::test]
fn share_with_lists_the_online_machines_and_says_the_result(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(common::User::new());
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();
    let _turn = signing(&pc);
    let (shell, cx) = window(&pc, cx);
    let section = open_vault(&shell, cx);

    click("vault-more-tracker", cx);
    settle(cx);
    click("share-vault-tracker", cx);
    // The dialog rises over the page and reads the machines online: the server, not this one.
    until(&shell, cx, "the machines online are listed", |cx| {
        section.read_with(cx, |s, _| s.machines().is_some())
    });
    assert!(section.read_with(cx, |s, _| s.is_sharing()));
    assert_eq!(
        section.read_with(cx, |s, _| s.machines().cloned()),
        Some(Ok(vec!["server".to_owned()]))
    );
    // Its row unfolds into the dialog.
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    let partway = cx.update(|window, _| window.find("share-item-server").bounds().size.height);
    settle(cx);
    let whole = cx.update(|window, _| window.find("share-item-server").bounds().size.height);
    assert!(
        partway < whole,
        "the row unfolds: {partway:?} then {whole:?}"
    );
    assert!(shown("share-to-server", cx));
    assert!(!shown("vault-share-none", cx), "{NO_MACHINE} is not said");

    click("share-to-server", cx);
    until(&shell, cx, "the toast says it is shared", |cx| {
        toasts(&shell, cx).contains(&"Entry tracker shared with server".to_owned())
    });
    settle(cx);
    assert!(
        !section.read_with(cx, |s, _| s.is_sharing()),
        "the dialog closed"
    );
    let copy = server.user.call("vault.list", json!({})).unwrap()["entries"][0].clone();
    assert_eq!(copy["name"], "tracker", "{copy}");
    assert_eq!(copy["shared_from"], "pc", "{copy}");
    // This machine's own row says nothing of where it went.
    assert!(!shown("vault-shared-tracker", cx));

    // A share the server refuses is said too, and the dialog closes all the same: the server
    // stopped, it is no longer offered, and a share asked meanwhile is refused.
    click("vault-more-tracker", cx);
    settle(cx);
    click("share-vault-tracker", cx);
    until(&shell, cx, "the machines are read again", |cx| {
        section.read_with(cx, |s, _| s.machines().is_some())
    });
    settle(cx);
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    click("share-to-server", cx);
    until(&shell, cx, "the toast says it was not shared", |cx| {
        toasts(&shell, cx).contains(&"Entry tracker was not shared".to_owned())
    });
    settle(cx);
    assert!(!section.read_with(cx, |s, _| s.is_sharing()));
}

#[gpui_kit::test]
fn share_with_lists_only_the_runners(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(common::User::new());
    // A laptop that drives without running agents, online too: a share there would be refused.
    let laptop = Machine::new();
    pair(&pc, &laptop, "laptop", "controller");
    server.wait_for_member("laptop", true);
    pc.wait_for("laptop", "online");
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();
    let (shell, cx) = window(&pc, cx);
    let section = open_vault(&shell, cx);

    click("vault-more-tracker", cx);
    settle(cx);
    click("share-vault-tracker", cx);
    until(&shell, cx, "the machines are listed", |cx| {
        section.read_with(cx, |s, _| s.machines().is_some())
    });
    settle(cx);

    assert_eq!(
        section.read_with(cx, |s, _| s.machines().cloned()),
        Some(Ok(vec!["server".to_owned()]))
    );
    assert!(shown("share-to-server", cx));
    assert!(!shown("share-to-laptop", cx));
}

#[gpui_kit::test]
fn the_row_of_a_shared_entry_says_where_it_came_from(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(common::User::new());
    // The server's own entry says nothing; pc's, shared, comes as a copy.
    server
        .user
        .call("vault.set", json!({"name": "notes", "secret": SECRET}))
        .unwrap();
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();
    let (shell, cx) = window(&server, cx);
    let section = open_vault(&shell, cx);
    until(&shell, cx, "the server's own entry is read", |cx| {
        section.read_with(cx, |s, _| {
            s.entries()
                .is_some_and(|e| e.iter().any(|e| e.name == "notes"))
        })
    });
    settle(cx);
    assert!(
        !shown("vault-shared-notes", cx),
        "its own entry says nothing"
    );

    let output = pc
        .user
        .run(&["vault", "share", "tracker", "--machine", "server"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    until(&shell, cx, "the entry is read as shared", |cx| {
        section.read_with(cx, |s, _| {
            s.entries()
                .is_some_and(|e| e.iter().any(|e| e.shared_from.as_deref() == Some("pc")))
        })
    });
    settle(cx);
    let said = cx.update(|window, _| {
        window
            .find("vault-shared-tracker")
            .label()
            .map(str::to_owned)
    });
    assert_eq!(said.as_deref(), Some("Shared from pc"));
    let height = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            window
                .find("vault-shared-block-tracker")
                .bounds()
                .size
                .height
        })
    };
    let whole = height(cx);

    // A secret set here by hand makes it the server's own: the line folds away.
    server
        .user
        .call("vault.set", json!({"name": "tracker", "secret": SECOND}))
        .unwrap();
    until(&shell, cx, "the entry is read as the server's own", |cx| {
        section.read_with(cx, |s, _| {
            s.entries().is_some_and(|e| {
                e.iter()
                    .any(|e| e.name == "tracker" && e.shared_from.is_none())
            })
        })
    });
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    let partway = height(cx);
    assert!(
        partway < whole,
        "the line folds away: {whole:?} then {partway:?}"
    );
    settle(cx);
    assert!(
        !shown("vault-shared-tracker", cx),
        "its own entry says nothing"
    );
}

#[gpui_kit::test]
fn share_with_asks_this_machine_while_another_is_shown(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server(common::User::new());
    // The server holds a copy of pc's entry, so that a share of it again is no refusal.
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();
    let shared = pc
        .user
        .run(&["vault", "share", "tracker", "--machine", "server"]);
    assert!(
        shared.status.success(),
        "{}",
        String::from_utf8_lossy(&shared.stderr)
    );
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECOND}))
        .unwrap();
    let _turn = signing(&pc);
    std::fs::write(pc.user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let env = Environment {
        os_release: pc.user.path("os-release"),
        state_home: pc.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(pc.user.connector());
    let device = Some(Device::in_folder(pc.folder()));
    let (shell, cx) = cx.add_window_view(|window, cx| {
        Shell::new_polled_with(connect.clone(), device.clone(), window, cx)
    });
    cx.simulate_resize(size(px(1280.), px(1600.)));
    // Its block unfolds as the clock moves.
    until(&shell, cx, "the switcher shows", |cx| {
        cx.executor().advance_clock(Duration::from_millis(16));
        shown(SWITCHER, cx)
    });
    settle(cx);
    click(SWITCHER, cx);
    settle(cx);
    click(&format!("{SWITCHER}-server"), cx);
    until(&shell, cx, "the server is shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()) == Some("server".to_owned())
    });
    let section = open_vault(&shell, cx);

    click("vault-more-tracker", cx);
    settle(cx);
    click("share-vault-tracker", cx);
    until(&shell, cx, "the machines online are listed", |cx| {
        section.read_with(cx, |s, _| s.machines().is_some())
    });
    // This machine's runners, not the shown machine's: its vault is what is shared.
    assert_eq!(
        section.read_with(cx, |s, _| s.machines().cloned()),
        Some(Ok(vec!["server".to_owned()]))
    );
    settle(cx);
    click("share-to-server", cx);
    until(&shell, cx, "the toast says it is shared", |cx| {
        toasts(&shell, cx).contains(&"Entry tracker shared with server".to_owned())
    });
    assert!(
        !toasts(&shell, cx).contains(&"Entry tracker was not shared".to_owned()),
        "{:?}",
        toasts(&shell, cx)
    );
}
