//! Settings, Vault, a row's Share with… (#832, `docs/remote.md`, "Vault entries"), on GPUI's test
//! platform in front of two real daemons paired through a hub served in the test: the dialog lists
//! the user's machines online, choosing one shares the entry, signed with the device key of the
//! window's machine, and a toast says the result; on the other machine the entry's row says where
//! it was shared from. The keyrings are the daemons' memory; no window opens on a screen.

mod common;
mod remote;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::link::{Connect, Sign};
use flottille_window::vault::{NO_MACHINE, VaultSection};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use remote::{Machine, NoKeyring, pc_and_server};
use serde_json::json;

const SECRET: &str = "w1ndow-sh4re-secret";

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

/// Signs with the device key the clients of `machine` keep, as the binary's window does.
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

#[gpui_kit::test]
fn share_with_lists_the_online_machines_and_says_the_result(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(common::User::new());
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();
    flottille_window::link::sign_with(signing(&pc));
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
fn the_row_of_a_shared_entry_says_where_it_came_from(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(common::User::new());
    // The server's own entry first: sharing pc's over it makes it a copy.
    for user in [&server.user, &pc.user] {
        user.call("vault.set", json!({"name": "tracker", "secret": SECRET}))
            .unwrap();
    }
    let (shell, cx) = window(&server, cx);
    let section = open_vault(&shell, cx);
    assert!(
        !shown("vault-shared-tracker", cx),
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
    // The row's new line unfolds under it.
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    let partway = cx.update(|window, _| {
        window
            .find("vault-shared-block-tracker")
            .bounds()
            .size
            .height
    });
    settle(cx);
    let whole = cx.update(|window, _| {
        window
            .find("vault-shared-block-tracker")
            .bounds()
            .size
            .height
    });
    assert!(
        partway < whole,
        "the line unfolds: {partway:?} then {whole:?}"
    );
    let said = cx.update(|window, _| {
        window
            .find("vault-shared-tracker")
            .label()
            .map(str::to_owned)
    });
    assert_eq!(said.as_deref(), Some("Shared from pc"));
}
