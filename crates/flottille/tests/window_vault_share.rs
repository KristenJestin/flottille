//! Settings, Vault, a row's Share with… (#832, `docs/remote.md`, "Vault entries"), on GPUI's test
//! platform in front of two real daemons paired through a hub served in the test: the dialog lists
//! the user's machines online, choosing one shares the entry, signed with the device key of the
//! window's machine, and a toast says the result; on the other machine the entry's row says where
//! it was shared from. The keyrings are the daemons' memory; no window opens on a screen.

use crate::common;
use crate::remote;

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use common::mcp::{Mcp, server_of};
use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::{Connect, Sign};
use flottille_window::tree::SWITCHER;
use flottille_window::vault::{NO_MACHINE, VaultSection};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use remote::{Machine, NoKeyring, pair, pc_and_server, pc_and_server_on};
use serde_json::json;

const SECRET: &str = "w1ndow-sh4re-secret";
/// The server's own secret: set by hand over its copy, or an entry only it holds.
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
    let (shell, cx) = window(&pc, cx);
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
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

/// A local API that answers every request, the `Authorization` each one carried kept in order:
/// what tells which secret a machine's tools hold, without reading a keyring.
struct Site {
    addr: SocketAddr,
    authorizations: Arc<Mutex<Vec<String>>>,
}

impl Site {
    fn start() -> Site {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let authorizations = Arc::new(Mutex::new(Vec::new()));
        let kept = authorizations.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut authorization = String::new();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = header.trim_end().split_once(':')
                        && name.eq_ignore_ascii_case("authorization")
                    {
                        authorization = value.trim().to_owned();
                    }
                }
                kept.lock().unwrap().push(authorization);
                let body = "{\"fields\":[]}";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                     connection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Site {
            addr,
            authorizations,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

/// The project `name` on `user`'s machine, a throwaway repository.
fn project(user: &common::User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

#[gpui_kit::test]
fn share_with_shares_the_entry_of_the_machine_shown(cx: &mut TestAppContext) {
    let site = Site::start();
    let pc = Machine::of(
        common::User::with_fake_agent().with_env("FLOTTILLE_TEST_WEB_OPEN", &site.addr.to_string()),
    );
    let (_hub, pc, server) = pc_and_server_on(pc, common::User::new());
    project(&pc.user, "app");
    project(&server.user, "app");
    // An entry only the server holds: the row the user clicks while the server is shown.
    server
        .user
        .call(
            "vault.set",
            json!({"name": "tracker", "secret": SECOND, "description": "The server's own",
                "url": site.url("/"), "project": "app"}),
        )
        .unwrap();
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
    // The shown machine's runners: its vault is what is shared.
    assert_eq!(
        section.read_with(cx, |s, _| s.machines().cloned()),
        Some(Ok(vec!["pc".to_owned()]))
    );
    settle(cx);
    click("share-to-pc", cx);
    until(&shell, cx, "the toast says it is shared", |cx| {
        toasts(&shell, cx).contains(&"Entry tracker shared with pc".to_owned())
    });
    assert!(
        !toasts(&shell, cx).contains(&"Entry tracker was not shared".to_owned()),
        "{:?}",
        toasts(&shell, cx)
    );
    let copy = pc.user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "tracker")
        .cloned()
        .expect("the copy on this machine");
    assert_eq!(copy["shared_from"], "server", "{copy}");
    assert_eq!(copy["description"], "The server's own", "{copy}");

    // The server's secret is what this machine's tools now sign in with.
    let fields = site.url("/rest/api/3/field");
    let opened = pc.user.received_requests("session/new").len();
    pc.user
        .call(
            "agent.run",
            json!({"project": "app", "message": format!("read the tracker's fields: {fields}")}),
        )
        .unwrap();
    let (url, authorization) = server_of(&pc.user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    let answer = mcp.call(
        "web",
        json!({"action": "read", "url": fields, "entry": "tracker"}),
    );
    assert!(!answer.error, "{}", answer.text);
    let sent = site.authorizations.lock().unwrap().clone();
    assert!(
        sent == [format!("Bearer {SECOND}")],
        "the server's secret did not reach the API ({} requests)",
        sent.len()
    );
}
