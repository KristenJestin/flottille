//! Settings, Connections, a connection's Share with… and the Sign in of the copy on the other
//! machine (#833, `docs/remote.md`, "Connections"), on GPUI's test platform in front of two real
//! daemons paired through a hub served in the test. The window signs with the device key of its
//! machine; the browser is played by the test. The keyrings are the daemons' memory; no window
//! opens on a screen.

use crate::common;
use crate::remote;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::remote::{Machine, NoKeyring, pair, pc_and_server};
use common::User;
use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::extensions::connections::ConnectionsSection;
use flottille_window::link::{Connect, Sign};
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const SECRET: &str = "w1ndow-n0tes-k3y-secret";

fn window<'a>(
    machine: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let connect: Connect = Arc::new(machine.user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2000.)));
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
        cx.executor().advance_clock(Duration::from_millis(16));
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
}

fn click(id: &str, cx: &mut VisualTestContext) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn shown(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some_and(|f| f.visible()))
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn height(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .map_or(0., |found| f32::from(found.bounds().size.height))
    })
}

fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .map(|toast| format!("{}: {}", toast.title(), toast.second_line().unwrap_or("")))
            .collect()
    })
}

fn section(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ConnectionsSection> {
    shell.read_with(cx, |s, cx| {
        let page = s.main().read(cx).settings_page().cloned().unwrap();
        page.read(cx).connections().clone()
    })
}

/// Connections' page, open from its row of Settings, Features.
fn open_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click("settings", cx));
    until(shell, cx, "the settings page is shown", |cx| {
        present(cx, "settings-tabs-features")
    });
    settle(cx);
    click("settings-tabs-features", cx);
    until(shell, cx, "Connections is a row", |cx| {
        present(cx, "features-connections")
    });
    settle(cx);
    click("features-connections", cx);
    until(shell, cx, "its page is in", |cx| {
        shown(cx, "connections-switch")
    });
    settle(cx);
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

fn fake_mcp() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(
        path.exists(),
        "{} is missing: `cargo build -p flottille-fake-mcp` first",
        path.display()
    );
    path
}

/// The fake MCP server, protected by its fake authorization server, of `user`: its URL.
fn protected_server(user: &User) -> (common::Spawned, String) {
    let script = json!({
        "server": {"name": "notes", "version": "1.0.0"},
        "tools": [{"name": "search", "description": "Finds notes by words.",
            "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}}],
        "answers": {"search": {"text": "found"}},
        "oauth": {"tokens_file": user.path("notes-tokens")},
    });
    let script_file = user.path("notes-script.json");
    std::fs::write(&script_file, script.to_string()).unwrap();
    let port_file = user.path("notes-port");
    let server = common::Spawned::new({
        let mut command = std::process::Command::new(fake_mcp());
        command.args([
            "--script",
            script_file.to_str().unwrap(),
            "--http",
            port_file.to_str().unwrap(),
        ]);
        command
    });
    remote::eventually("the server listens", || {
        port_file.exists() && !std::fs::read_to_string(&port_file).unwrap().is_empty()
    });
    let port = std::fs::read_to_string(&port_file).unwrap();
    (server, format!("http://127.0.0.1:{port}/mcp"))
}

/// A GET as the browser makes it, following nothing: its status and its `Location`.
fn browse(address: &str) -> (u16, Option<String>) {
    use std::io::{Read, Write};
    let url = url::Url::parse(address).unwrap();
    let mut stream = std::net::TcpStream::connect((
        url.host_str().unwrap(),
        url.port_or_known_default().unwrap(),
    ))
    .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let target = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        url.host_str().unwrap()
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let head = answer.split("\r\n\r\n").next().unwrap_or_default();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let location = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("location")
            .then(|| value.trim().to_owned())
    });
    (status, location)
}

#[gpui_kit::test]
fn share_and_sign_in_a_connection_of_another_machine(cx: &mut TestAppContext) {
    let (_hub, pc, server) =
        pc_and_server(User::new().with_env("FLOTTILLE_TEST_SIGN_IN_LOOPBACK", "127.0.0.2"));
    pc.user
        .call(
            "vault.set",
            json!({"name": "notes-key", "secret": SECRET, "url": "https://notes.example.com"}),
        )
        .unwrap();
    // Off, so that nothing reaches its address from here.
    pc.user
        .call(
            "connection.add",
            json!({"name": "notes", "about": "The user's notes.", "enabled": false,
                "url": "https://notes.example.com/mcp",
                "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                    "entry": "notes-key"}]}),
        )
        .unwrap();
    pc.user
        .call(
            "extension.set",
            json!({"name": "connections", "enabled": true}),
        )
        .unwrap();
    let (shell, cx) = window(&pc, cx);
    // This window's own signer: another test of the process may have set the process's.
    shell.update(cx, |s, _| s.sign_with(signing(&pc)));
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let opened = opened.clone();
        shell.update(cx, |s, _| {
            s.set_opener(Arc::new(move |path: &Path| {
                opened.lock().unwrap().push(path.display().to_string());
            }))
        });
    }
    open_page(&shell, cx);
    until(&shell, cx, "the server is listed", |cx| {
        present(cx, "connection-notes")
    });
    let section = section(&shell, cx);

    // Share with…: the dialog rises, lists the runners online, and offers the entries it names.
    click("connection-more-notes", cx);
    assert_eq!(
        label(cx, "share-connection-notes-label").as_deref(),
        Some("Share with…")
    );
    click("share-connection-notes", cx);
    until(&shell, cx, "the machines are listed", |cx| {
        section.read_with(cx, |s, _| s.share_machines().is_some())
    });
    assert!(section.read_with(cx, |s, _| s.is_sharing()));
    assert_eq!(
        section.read_with(cx, |s, _| s.share_machines().cloned()),
        Some(Ok(vec!["server".to_owned()]))
    );
    settle(cx);
    assert!(shown(cx, "connection-share-to-server"));
    assert_eq!(
        label(cx, "connection-share-entries-label").as_deref(),
        Some("Share its vault entries too")
    );
    assert_eq!(
        label(cx, "connection-share-entries-hint").as_deref(),
        Some("notes-key: its secret goes into the keyring of the machine you choose.")
    );
    click("connection-share-entries", cx);
    click("connection-share-to-server", cx);
    until(&shell, cx, "the toast says it is shared", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t.starts_with("Connection notes shared with server"))
    });
    // Under the row, the copy's state on the server unfolds.
    let partway = height(cx, "connection-elsewhere-block-notes");
    settle(cx);
    assert!(!section.read_with(cx, |s, _| s.is_sharing()), "it closed");
    let copy = server.user.call("connection.list", json!({})).unwrap()["connections"][0].clone();
    assert_eq!(copy["name"], "notes", "{copy}");
    let entry = server.user.call("vault.list", json!({})).unwrap()["entries"][0].clone();
    assert_eq!(entry["name"], "notes-key", "{entry}");
    assert_eq!(entry["shared_from"], "pc", "{entry}");

    let whole = height(cx, "connection-elsewhere-block-notes");
    assert!(partway < whole, "the line unfolds: {partway} then {whole}");
    assert_eq!(
        label(cx, "connection-elsewhere-notes").as_deref(),
        Some("On server: not signed in yet")
    );

    // The server's copy reaches the server's own fake (a test's stand-in for its https server).
    let (mut process, url) = protected_server(&server.user);
    server
        .user
        .call(
            "extension.set",
            json!({"name": "connections", "enabled": true}),
        )
        .unwrap();
    server
        .user
        .call(
            "connection.edit",
            json!({"name": "notes", "url": url, "secrets": null, "enabled": true}),
        )
        .unwrap();

    // Sign in on server: the browser opens the server's sign-in, and comes back through pc.
    click("connection-more-notes", cx);
    assert_eq!(
        label(cx, "sign-in-connection-notes-on-server-label").as_deref(),
        Some("Sign in on server")
    );
    click("sign-in-connection-notes-on-server", cx);
    until(&shell, cx, "the browser is opened on the sign-in", |cx| {
        assert!(
            toasts(&shell, cx)
                .iter()
                .all(|t| !t.contains("not signed in")),
            "{:?}",
            toasts(&shell, cx)
        );
        !opened.lock().unwrap().is_empty()
    });
    assert_eq!(
        label(cx, "connection-elsewhere-notes").as_deref(),
        Some("On server: signing in, finish it in your browser")
    );
    let address = opened.lock().unwrap()[0].clone();
    let (status, location) = browse(&address);
    assert_eq!(status, 302);
    let (status, _) = browse(&location.unwrap());
    assert_eq!(status, 200);
    until(&shell, cx, "it says it is signed in", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t.starts_with("Signed in to notes on server"))
    });
    settle(cx);
    assert_eq!(
        label(cx, "connection-elsewhere-notes").as_deref(),
        Some("On server: signed in")
    );
    assert!(
        server.user.call("vault.list", json!({})).unwrap()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "notes.oauth")
    );
    let _ = process.kill();
    let _ = process.wait();
}

/// A window of `machine` that holds its device key and shows `shown`, another of the user's
/// machines: every request of its pages goes there, signed.
fn window_showing<'a>(
    machine: &Machine,
    shown: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    window_showing_after(machine, shown, false, cx)
}

/// `window_showing`, with the window's own machine's extensions read before it switches when
/// `own_read_first` (otherwise that read races the switch, and goes to either machine).
fn window_showing_after<'a>(
    machine: &Machine,
    shown: &str,
    own_read_first: bool,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    std::fs::write(machine.user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let env = Environment {
        os_release: machine.user.path("os-release"),
        state_home: machine.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(machine.user.connector());
    let device = Some(Device::in_folder(machine.folder()));
    let (shell, cx) = cx.add_window_view(|window, cx| {
        Shell::new_polled_with(connect.clone(), device.clone(), window, cx)
    });
    cx.simulate_resize(size(px(1280.), px(2000.)));
    shell.update(cx, |s, _| s.sign_with(signing(machine)));
    until(&shell, cx, "this machine's tree is read", |cx| {
        shell.read_with(cx, |s, cx| s.machines().read(cx).machines().is_some())
    });
    if own_read_first {
        until(&shell, cx, "this machine's extensions are read", |cx| {
            shell.read_with(cx, |s, cx| s.extensions().read(cx).is_known())
        });
        // And its connections: the page lists none, as this machine has none.
        open_page(&shell, cx);
        let own = section(&shell, cx);
        until(&shell, cx, "this machine's connections are read", |cx| {
            own.read_with(cx, |s, _| s.servers().is_some())
        });
    }
    let name = shown.to_owned();
    shell.update_in(cx, |s, window, cx| s.choose_machine(Some(name), window, cx));
    until(&shell, cx, "the machine is shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some(shown)
            && !present(cx, "sidebar-loading")
    });
    settle(cx);
    (shell, cx)
}

/// The connection `notes` of `machine`, off, and the vault entry it names.
fn notes_on(machine: &Machine) {
    machine
        .user
        .call(
            "vault.set",
            json!({"name": "notes-key", "secret": SECRET, "url": "https://notes.example.com"}),
        )
        .unwrap();
    machine
        .user
        .call(
            "connection.add",
            json!({"name": "notes", "about": "The user's notes.", "enabled": false,
                "url": "https://notes.example.com/mcp",
                "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                    "entry": "notes-key"}]}),
        )
        .unwrap();
    machine
        .user
        .call(
            "extension.set",
            json!({"name": "connections", "enabled": true}),
        )
        .unwrap();
}

#[gpui_kit::test]
fn share_a_connection_of_the_machine_shown(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    notes_on(&server);
    let (shell, cx) = window_showing(&pc, "server", cx);
    open_page(&shell, cx);
    until(&shell, cx, "the server's connection is listed", |cx| {
        present(cx, "connection-notes")
    });
    let section = section(&shell, cx);

    click("connection-more-notes", cx);
    click("share-connection-notes", cx);
    until(&shell, cx, "the machines are listed", |cx| {
        section.read_with(cx, |s, _| s.share_machines().is_some())
    });
    // The shown machine's runners: this one.
    assert_eq!(
        section.read_with(cx, |s, _| s.share_machines().cloned()),
        Some(Ok(vec!["pc".to_owned()]))
    );
    settle(cx);
    click("connection-share-entries", cx);
    click("connection-share-to-pc", cx);
    until(&shell, cx, "the toast says what came of it", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t.starts_with("Connection notes"))
    });
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|t| t.starts_with("Connection notes shared with pc")),
        "{:?}",
        toasts(&shell, cx)
    );
    let copy = pc.user.call("connection.list", json!({})).unwrap()["connections"][0].clone();
    assert_eq!(copy["name"], "notes", "{copy}");
    let entry = pc.user.call("vault.list", json!({})).unwrap()["entries"][0].clone();
    assert_eq!(entry["name"], "notes-key", "{entry}");
    assert_eq!(entry["shared_from"], "server", "{entry}");
}

#[gpui_kit::test]
fn what_the_machine_shown_lists_is_read_when_the_window_switches_to_it(cx: &mut TestAppContext) {
    // Connections is on at the server and off here: what the window lists must be the server's.
    let (_hub, pc, server) = pc_and_server(User::new());
    notes_on(&server);
    let (shell, cx) = window_showing_after(&pc, "server", true, cx);
    until(&shell, cx, "the server's extensions are listed", |cx| {
        shell.read_with(cx, |s, cx| {
            s.extensions()
                .read(cx)
                .listed("connections")
                .is_some_and(|listed| listed.enabled)
        })
    });
    let page = shell.read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned());
    if page.is_none() {
        open_page(&shell, cx);
    }
    let shown = section(&shell, cx);
    until(&shell, cx, "the server's connections are listed", |cx| {
        shown.read_with(cx, |s, _| s.servers().map(<[_]>::len) == Some(1))
    });
}

#[gpui_kit::test]
fn sign_in_on_a_machine_while_another_is_shown(cx: &mut TestAppContext) {
    let (hub, pc, server) = pc_and_server(User::new());
    // A third machine, a runner, whose copy of the server's connection is signed in.
    let laptop = Machine::of(User::new().with_env("FLOTTILLE_TEST_SIGN_IN_LOOPBACK", "127.0.0.2"));
    pair(&hub, &laptop, "laptop", "runner");
    server.wait_for_member("laptop", true);
    pc.wait_for("laptop", "online");
    server.wait_for("laptop", "online");
    notes_on(&server);
    let shared = server.user.run(&[
        "connection",
        "share",
        "notes",
        "--machine",
        "laptop",
        "--entry",
        "notes-key",
    ]);
    assert!(
        shared.status.success(),
        "{}",
        String::from_utf8_lossy(&shared.stderr)
    );
    let (mut process, url) = protected_server(&laptop.user);
    laptop
        .user
        .call(
            "extension.set",
            json!({"name": "connections", "enabled": true}),
        )
        .unwrap();
    laptop
        .user
        .call(
            "connection.edit",
            json!({"name": "notes", "url": url, "secrets": null, "enabled": true}),
        )
        .unwrap();

    let (shell, cx) = window_showing(&pc, "server", cx);
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let opened = opened.clone();
        shell.update(cx, |s, _| {
            s.set_opener(Arc::new(move |path: &Path| {
                opened.lock().unwrap().push(path.display().to_string());
            }))
        });
    }
    open_page(&shell, cx);
    until(&shell, cx, "the server's connection is listed", |cx| {
        present(cx, "connection-notes")
    });
    let section = section(&shell, cx);

    // The sign-in of the laptop's copy: its redirect comes back to this machine's 127.0.0.1,
    // whichever machine the window shows.
    section.update(cx, |s, cx| s.sign_in_on("notes", "laptop", cx));
    until(&shell, cx, "the browser is opened on the sign-in", |cx| {
        assert!(
            toasts(&shell, cx)
                .iter()
                .all(|t| !t.contains("not signed in")),
            "{:?}",
            toasts(&shell, cx)
        );
        !opened.lock().unwrap().is_empty()
    });
    let address = opened.lock().unwrap()[0].clone();
    let (status, location) = browse(&address);
    assert_eq!(status, 302);
    let (status, _) = browse(&location.unwrap());
    assert_eq!(status, 200);
    until(&shell, cx, "it says it is signed in", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t.starts_with("Signed in to notes on laptop"))
    });
    let _ = process.kill();
    let _ = process.wait();
}

#[gpui_kit::test]
fn the_sign_in_toast_s_action_after_switching_goes_to_the_machine_that_shared(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server(User::new());
    notes_on(&server);
    let (shell, cx) = window_showing(&pc, "server", cx);
    open_page(&shell, cx);
    until(&shell, cx, "the server's connection is listed", |cx| {
        present(cx, "connection-notes")
    });
    let section = section(&shell, cx);
    click("connection-more-notes", cx);
    click("share-connection-notes", cx);
    until(&shell, cx, "the machines are listed", |cx| {
        section.read_with(cx, |s, _| s.share_machines().is_some())
    });
    settle(cx);
    click("connection-share-entries", cx);
    click("connection-share-to-pc", cx);
    until(&shell, cx, "the toast offers the sign-in", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t.starts_with("Connection notes shared with pc"))
    });
    // pc's copy reaches nothing: its sign-in fails at once, on this machine's own loopback.
    pc.user
        .call(
            "extension.set",
            json!({"name": "connections", "enabled": true}),
        )
        .unwrap();
    pc.user
        .call(
            "connection.edit",
            json!({"name": "notes", "url": "http://127.0.0.1:9/mcp", "secrets": null,
                "enabled": true}),
        )
        .unwrap();

    // This machine's Settings in front, then the server's toast's action.
    shell.update_in(cx, |s, window, cx| s.choose_machine(None, window, cx));
    until(&shell, cx, "this machine is shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).is_none()
    });
    settle(cx);
    let action = (1..16)
        .map(|n| format!("toast-{n}-action"))
        .find(|id| label(cx, id).as_deref() == Some("Sign in on pc"))
        .expect("the toast's action");
    click(&action, cx);
    until(
        &shell,
        cx,
        "the server's section hears how the sign-in went",
        |cx| {
            section.read_with(cx, |s, _| s.elsewhere_said("notes"))
                == Some("On pc: not signed in yet".to_owned())
        },
    );
}
