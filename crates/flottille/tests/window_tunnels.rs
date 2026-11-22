//! The scenarios of #830: a run or a link of another machine opened through a tunnel
//! (`docs/remote.md`, "Tunnels", "Requests from another machine's clients"). On GPUI's test
//! platform in front of real daemons, each with its own temporary HOME and XDG folders, a fake
//! agent and a fake HTTP server, and a hub served in the test on 127.0.0.1. The window signs with
//! the device key file of its machine's key folder; the system's opener is a fake that only notes
//! what it was given, so that no browser and no link opens. No window opens on a screen.

use crate::common;
use crate::remote::{Machine, NoKeyring, TestHub, pc_and_server_on};

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use common::User;
use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::device::Device;
use flottille_window::link::{Connect, Sign};
use flottille_window::session::SessionPage;
use flottille_window::shell::Selection;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// How long a link, a launch or a read of the machines may take.
const WAIT: Duration = Duration::from_secs(60);

/// Each scenario runs three daemons and a hub: one at a time, so that the timing of the other
/// windows' tests of the group is not tried by their load.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A picture, 1 pixel by 1: what the server's agent shows, to see that the window never reads a
/// file of another machine on its own disk.
const PNG: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A port nothing listens on now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What the server's agent does: it leads, then shows a link of its own machine's dev server, a
/// public one, and a file.
fn script(port: u16) -> Value {
    let link = |url: &str| json!({"mcp": {"tool": "web", "arguments": {"action": "show_link", "url": url}}});
    let file = |path: &str| json!({"mcp": {"tool": "web", "arguments": {"action": "show_link", "path": path}}});
    json!({"turns": [
        {"when": "lead the work", "do": [{"say": "On it."}]},
        {"when": "SHOW LOCAL", "do": [link(&format!("http://localhost:{port}/docs"))]},
        {"when": "SAY LOCAL", "do": [{"say": format!("[app](http://localhost:{port}/docs)")}]},
        {"when": "SHOW NOWHERE", "do": [link("http://localhost:1/nothing")]},
        {"when": "SHOW PUBLIC", "do": [link("https://example.org/t/42")]},
        {"when": "SHOW FILE", "do": [file("shot.png")]},
        {"when": "SHOW CLIP", "do": [file("clip.mp4")]},
        {"when": "SHOW NOTES", "do": [file("notes.md")]}
    ]})
}

/// A throwaway repository with one commit, registered as the project `app` of `user`.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// The fake HTTP server as the task `web` of `app`, which says it answers at `url`, listening on
/// `listen`; run by the user: its run, once it listens.
fn web(user: &User, url: &str, listen: u16) -> String {
    user.call(
        "task.add",
        json!({"project": "app", "name": "web", "check": true, "url": url,
            "line": format!("{} --listen {listen}", common::fake_server().display())}),
    )
    .unwrap();
    let run = user
        .call("task.run", json!({"project": "app", "name": "web"}))
        .unwrap()["run"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the server listens", || {
        user.call("run.output", json!({"run": run})).unwrap()["output"]
            .as_str()
            .is_some_and(|text| text.contains("listening on"))
    });
    run
}

/// Two machines: `pc`, whose window is tried, and `server`, a runner with the project `app` and
/// an agent that leads in it, which shows the links of `script`.
struct Machines {
    _hub: TestHub,
    pc: Machine,
    server: Machine,
    /// The port the server's dev server listens on.
    port: u16,
    /// The agent leading on the server.
    agent: String,
}

fn machines() -> (std::sync::MutexGuard<'static, ()>, Machines) {
    let turn = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let port = free_port();
    let user = User::with_fake_agent();
    user.fake_script(script(port));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    let (hub, pc, server) = pc_and_server_on(Machine::new(), user);
    std::fs::write(pc.user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    project(&server.user);
    let started = server
        .user
        .call(
            "agent.run",
            json!({"project": "app", "message": "lead the work"}),
        )
        .unwrap();
    let agent = started["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the agent's first turn", || {
        server
            .user
            .call("agent.show", json!({"agent": agent}))
            .unwrap()["agent"]["state"]
            == "idle"
    });
    (
        turn,
        Machines {
            _hub: hub,
            pc,
            server,
            port,
            agent,
        },
    )
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

/// What the window opened: the links it was asked to open, in order.
type Opened = Arc<Mutex<Vec<PathBuf>>>;

/// `pc`'s window, tall enough for the project page's runs, which signs as `pc` and opens nothing
/// but what `opened` notes.
fn window<'a>(
    pc: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext, Opened) {
    let env = Environment {
        os_release: pc.user.path("os-release"),
        state_home: pc.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(pc.user.connector());
    let device = Some(Device::in_folder(pc.folder()));
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    let opened: Opened = Arc::default();
    let given = opened.clone();
    shell.update(cx, |s, _| {
        s.sign_with(signing(pc));
        s.set_opener(Arc::new(move |path: &Path| {
            given.lock().unwrap().push(path.to_owned())
        }))
    });
    (shell, cx, opened)
}

/// One step of the window as the platform would run it.
fn step(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        step(shell, Duration::from_millis(16), cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Lets every motion end.
fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..40 {
        step(shell, Duration::from_millis(16), cx);
    }
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn height(cx: &mut VisualTestContext, id: &str) -> f32 {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .map_or(0., |found| f32::from(found.bounds().size.height))
    })
}

fn click(shell: &Entity<Shell>, cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(shell, cx);
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

/// The window shows the server, and its project `app`'s page.
fn on_the_server(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    until(shell, cx, "the server is read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .and_then(|read| read.machine("server"))
                .is_some_and(|server| server.online())
        })
    });
    settle(shell, cx);
    shell.update(cx, |shell, cx| {
        shell.choose_machine(Some("server".to_owned()), cx)
    });
    until(shell, cx, "the server's project is in the tree", |cx| {
        shell.read_with(cx, |s, _| {
            s.machine_shown().as_deref() == Some("server")
                && s.snapshot().projects.iter().any(|p| p.name == "app")
        })
    });
}

fn project_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
}

/// The tunnels the daemon of `pc` carries.
fn carried(pc: &Machine) -> Vec<Value> {
    pc.user.call("tunnel.list", json!({})).unwrap()["tunnels"]
        .as_array()
        .unwrap()
        .clone()
}

/// What `port` of this machine answers to a request.
fn answer_on(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    let _ = stream.read_to_string(&mut text);
    text
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn open_session(shell: &Entity<Shell>, agent: &str, cx: &mut VisualTestContext) {
    until(shell, cx, "the server's session is in the tree", |cx| {
        cx.update(|window, _| window.try_find(session_id(agent)).is_some())
    });
    settle(shell, cx);
    cx.update(|window, cx| window.click(session_id(agent), cx));
    until(shell, cx, "the session's page shows", |cx| {
        session_page(shell, cx).read_with(cx, |page, cx| {
            page.id() == agent && !page.conversation().read(cx).rows().is_empty()
        })
    });
    settle(shell, cx);
}

/// The row of the card of links, once there is one.
fn card_at(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<usize> {
    page.read_with(cx, |page, cx| {
        page.conversation()
            .read(cx)
            .blocks()
            .iter()
            .position(|block| matches!(block, Block::Links { .. }))
    })
}

fn tell(machine: &Machine, agent: &str, message: &str) {
    machine
        .user
        .call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
}

/// The server's session open in the window and its agent asked to show what `message` names:
/// the row of the card.
fn card_of(
    shell: &Entity<Shell>,
    machines: &Machines,
    message: &str,
    cx: &mut VisualTestContext,
) -> (Entity<SessionPage>, usize) {
    open_session(shell, &machines.agent, cx);
    let page = session_page(shell, cx);
    tell(&machines.server, &machines.agent, message);
    until(shell, cx, "the card is in the conversation", |cx| {
        card_at(&page, cx).is_some()
    });
    settle(shell, cx);
    let at = card_at(&page, cx).unwrap();
    (page, at)
}

#[gpui_kit::test]
fn opening_a_run_of_another_machine_opens_a_tunnel_then_the_local_address(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let url = format!("http://127.0.0.1:{}", machines.port);
    let run = web(&machines.server.user, &url, machines.port);
    let (shell, cx, opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    project_page(&shell, cx);
    let link = format!("place-run-link-{run}");
    until(
        &shell,
        cx,
        "the project page shows the run's address",
        |cx| label(cx, &link).is_some(),
    );
    settle(&shell, cx);
    assert_eq!(
        label(cx, &link).as_deref(),
        Some(format!("127.0.0.1:{}", machines.port).as_str())
    );
    assert!(carried(&machines.pc).is_empty(), "no tunnel before a click");
    assert!(opened.lock().unwrap().is_empty());

    click(&shell, cx, &link);
    until(&shell, cx, "the window opened the local address", |_| {
        !opened.lock().unwrap().is_empty()
    });

    let tunnels = carried(&machines.pc);
    assert_eq!(tunnels.len(), 1, "{tunnels:?}");
    let local = tunnels[0]["local_port"].as_u64().unwrap();
    assert_eq!(tunnels[0]["machine"], "server");
    assert_eq!(tunnels[0]["run"], run.as_str());
    assert_eq!(tunnels[0]["port"], machines.port);
    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from(format!("http://localhost:{local}"))],
        "the system's handler is given the local address, not the server's"
    );
    // The address is the server's page: the tunnel carries it.
    let page = answer_on(local as u16);
    assert!(page.starts_with("HTTP/1.1 204"), "{page}");
    // Another click on the same address opens the same tunnel again, not a second one.
    click(&shell, cx, &link);
    assert_eq!(carried(&machines.pc).len(), 1);
    assert_eq!(opened.lock().unwrap().len(), 2);
}

#[gpui_kit::test]
fn a_localhost_link_of_another_machine_opens_through_a_tunnel(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let url = format!("http://127.0.0.1:{}", machines.port);
    let run = web(&machines.server.user, &url, machines.port);
    let (shell, cx, opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    let (_page, at) = card_of(&shell, &machines, "SHOW LOCAL", cx);

    // Nothing opens by itself.
    assert!(opened.lock().unwrap().is_empty());
    assert!(carried(&machines.pc).is_empty());
    click(&shell, cx, &format!("row-{at}-link-0-open"));
    until(&shell, cx, "the window opened the local address", |_| {
        !opened.lock().unwrap().is_empty()
    });

    let tunnels = carried(&machines.pc);
    assert_eq!(tunnels.len(), 1, "{tunnels:?}");
    assert_eq!(tunnels[0]["run"], run.as_str());
    assert_eq!(tunnels[0]["port"], machines.port);
    let local = tunnels[0]["local_port"].as_u64().unwrap();
    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from(format!("http://localhost:{local}/docs"))],
        "the path of the link is kept, its host and port are the tunnel's"
    );
}

#[gpui_kit::test]
fn a_localhost_link_written_in_markdown_opens_through_a_tunnel(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let url = format!("http://127.0.0.1:{}", machines.port);
    let run = web(&machines.server.user, &url, machines.port);
    let (shell, cx, opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    open_session(&shell, &machines.agent, cx);
    let page = session_page(&shell, cx);
    tell(&machines.server, &machines.agent, "SAY LOCAL");
    let reply = |block: &Block| matches!(block, Block::Text { text } if text.starts_with('['));
    let at = |cx: &mut VisualTestContext| {
        page.read_with(cx, |page, cx| {
            page.conversation().read(cx).blocks().iter().position(reply)
        })
    };
    until(&shell, cx, "the reply is in the conversation", |cx| {
        at(cx).is_some()
    });
    settle(&shell, cx);
    let at = at(cx).unwrap();

    cx.update(|window, cx| {
        window.click_at(
            format!("row-{at}-text-region"),
            gpui_kit::point(px(4.), px(4.)),
            cx,
        )
    });
    until(&shell, cx, "the window opened the local address", |_| {
        !opened.lock().unwrap().is_empty()
    });

    let tunnels = carried(&machines.pc);
    assert_eq!(tunnels.len(), 1, "{tunnels:?}");
    assert_eq!(tunnels[0]["run"], run.as_str());
    let local = tunnels[0]["local_port"].as_u64().unwrap();
    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from(format!("http://localhost:{local}/docs"))],
        "the machine's own address is opened through its tunnel, not as written"
    );
}

#[gpui_kit::test]
fn a_public_link_of_another_machine_opens_as_it_is(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let (shell, cx, opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    let (_page, at) = card_of(&shell, &machines, "SHOW PUBLIC", cx);

    click(&shell, cx, &format!("row-{at}-link-0-open"));

    assert_eq!(
        *opened.lock().unwrap(),
        [PathBuf::from("https://example.org/t/42")]
    );
    assert!(carried(&machines.pc).is_empty(), "no tunnel for the web");
}

#[gpui_kit::test]
fn the_open_tunnel_shows_in_its_row_and_closes(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let url = format!("http://127.0.0.1:{}", machines.port);
    let run = web(&machines.server.user, &url, machines.port);
    let (shell, cx, _opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    project_page(&shell, cx);
    let link = format!("place-run-link-{run}");
    until(
        &shell,
        cx,
        "the project page shows the run's address",
        |cx| label(cx, &link).is_some(),
    );
    settle(&shell, cx);
    click(&shell, cx, &link);
    until(&shell, cx, "the tunnel is open", |_| {
        !carried(&machines.pc).is_empty()
    });
    let tunnel = carried(&machines.pc)[0].clone();
    let (id, local) = (
        tunnel["tunnel"].as_str().unwrap().to_owned(),
        tunnel["local_port"].as_u64().unwrap(),
    );
    until(&shell, cx, "the row shows its tunnel", |cx| {
        present(cx, &format!("tunnel-{id}"))
    });
    settle(&shell, cx);

    assert_eq!(
        label(cx, &format!("tunnel-{id}")).as_deref(),
        Some(format!("localhost:{local} → server").as_str())
    );
    assert_eq!(
        label(cx, &format!("tunnel-{id}-close")).as_deref(),
        Some("Close")
    );

    click(&shell, cx, &format!("tunnel-{id}-close"));
    until(&shell, cx, "the daemon closed the tunnel", |_| {
        carried(&machines.pc).is_empty()
    });
    settle(&shell, cx);

    assert!(!present(cx, &format!("tunnel-{id}")), "its line is gone");
    assert!(
        TcpStream::connect(("127.0.0.1", local as u16)).is_err(),
        "nothing listens on the local port any more"
    );
    // The run is untouched: a tunnel is a door, not the run.
    assert!(present(cx, &link));
}

#[gpui_kit::test]
fn the_tunnel_s_line_unfolds_and_folds_away(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let url = format!("http://127.0.0.1:{}", machines.port);
    let run = web(&machines.server.user, &url, machines.port);
    let (shell, cx, _opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    project_page(&shell, cx);
    let link = format!("place-run-link-{run}");
    until(
        &shell,
        cx,
        "the project page shows the run's address",
        |cx| label(cx, &link).is_some(),
    );
    settle(&shell, cx);
    // A click and no settling: the line is looked at from its first frame.
    cx.update(|window, cx| window.click(link.clone(), cx));
    until(&shell, cx, "the tunnel is open", |_| {
        !carried(&machines.pc).is_empty()
    });
    let id = carried(&machines.pc)[0]["tunnel"]
        .as_str()
        .unwrap()
        .to_owned();
    let item = format!("tunnel-item-{id}");
    // It unfolds: a first frame of it is shorter than the line at rest.
    until(&shell, cx, "the line comes", |cx| present(cx, &item));
    let arriving = height(cx, &item);
    settle(&shell, cx);
    let rest = height(cx, &item);
    assert!(rest > 8., "the line at rest has a height: {rest}");
    assert!(arriving < rest, "it unfolds: {arriving} then {rest}");

    // It folds away: between the click and the end it is there, smaller.
    cx.update(|window, cx| window.click(format!("tunnel-{id}-close"), cx));
    until(&shell, cx, "the daemon closed the tunnel", |_| {
        carried(&machines.pc).is_empty()
    });
    step(&shell, Duration::from_millis(80), cx);
    let folding = height(cx, &item);
    assert!(
        folding > 0. && folding < rest,
        "it folds away, drawn until it has: {folding} of {rest}"
    );
    settle(&shell, cx);
    assert!(!present(cx, &item));
}

#[gpui_kit::test]
fn a_tunnel_that_cannot_open_is_a_toast_with_its_reason(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let url = format!("http://127.0.0.1:{}", machines.port);
    let run = web(&machines.server.user, &url, machines.port);
    let (shell, cx, opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    // The link names a port the only running run does not listen on: the machine refuses.
    let (_page, at) = card_of(&shell, &machines, "SHOW NOWHERE", cx);

    click(&shell, cx, &format!("row-{at}-link-0-open"));
    until(&shell, cx, "a toast says why", |cx| {
        !toasts(&shell, cx).is_empty()
    });

    let said = toasts(&shell, cx);
    assert!(
        said.iter().any(|title| title.starts_with("Not opened")
            && title.contains(&format!("port 1 is none of run {run}'s"))),
        "{said:?}"
    );
    assert!(opened.lock().unwrap().is_empty(), "nothing opened");
    assert!(carried(&machines.pc).is_empty());
}

#[gpui_kit::test]
fn a_link_no_run_of_the_machine_serves_is_a_toast_with_its_reason(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let (shell, cx, opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    let (_page, at) = card_of(&shell, &machines, "SHOW NOWHERE", cx);

    click(&shell, cx, &format!("row-{at}-link-0-open"));
    until(&shell, cx, "a toast says why", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|title| title.starts_with("Not opened"))
    });

    let said = toasts(&shell, cx);
    assert!(
        said.iter()
            .any(|title| title.contains("no run of server serves port 1")),
        "{said:?}"
    );
    assert!(opened.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn a_file_of_another_machine_that_is_no_picture_says_where_it_is(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    std::fs::write(machines.server.user.path("work/app/notes.md"), "# notes\n").unwrap();
    let (shell, cx, opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    let (_page, at) = card_of(&shell, &machines, "SHOW NOTES", cx);
    let path = std::fs::canonicalize(machines.server.user.path("work/app/notes.md")).unwrap();

    let said = label(cx, &format!("row-{at}-link-0")).expect("the line is drawn");
    assert!(said.contains("server"), "{said}");
    assert!(said.contains(path.to_str().unwrap()), "{said}");
    assert!(
        !present(cx, &format!("row-{at}-link-0-open")),
        "Open is not drawn: the file is not on this machine, and only pictures and videos travel"
    );
    assert!(!present(cx, &format!("row-{at}-link-0-thumb")));
    assert!(opened.lock().unwrap().is_empty());
}

/// What the window opened, once it opened something.
fn opened_file(shell: &Entity<Shell>, opened: &Opened, cx: &mut VisualTestContext) -> PathBuf {
    until(shell, cx, "the window opens a file", |_| {
        !opened.lock().unwrap().is_empty()
    });
    opened.lock().unwrap()[0].clone()
}

/// Frames the window asks for with nothing moving: none at rest.
fn pending_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

#[gpui_kit::test]
fn a_picture_an_agent_showed_on_another_machine_is_displayed(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    std::fs::write(machines.server.user.path("work/app/shot.png"), PNG).unwrap();
    let (shell, cx, opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    let (_page, at) = card_of(&shell, &machines, "SHOW FILE", cx);
    let thumb = format!("row-{at}-link-0-thumb");

    // The server sends it: the placeholder fades into the picture.
    until(&shell, cx, "the picture is drawn", |cx| present(cx, &thumb));
    settle(&shell, cx);
    assert!(height(cx, &thumb) > 0., "the picture takes its room");
    let said = label(cx, &format!("row-{at}-link-0")).expect("the line is drawn");
    assert!(said.contains("server"), "{said}");
    let path = std::fs::canonicalize(machines.server.user.path("work/app/shot.png")).unwrap();
    assert!(
        !said.contains(path.to_str().unwrap()),
        "the picture replaces the server's path: {said}"
    );
    assert_eq!(pending_frames(cx), 0, "at rest once the picture is in");

    // Open hands the user's viewer a copy the server sent, on this machine.
    click(&shell, cx, &format!("row-{at}-link-0-open"));
    let copy = opened_file(&shell, &opened, cx);
    assert_ne!(copy, path);
    assert!(copy.to_str().unwrap().ends_with("shot.png"), "{copy:?}");
    assert_eq!(std::fs::read(&copy).unwrap(), PNG);
}

#[gpui_kit::test]
fn a_video_of_another_machine_plays_in_the_user_s_player(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let video: Vec<u8> = (0..3 * 1024 * 1024 + 7).map(|n| (n % 253) as u8).collect();
    std::fs::write(machines.server.user.path("work/app/clip.mp4"), &video).unwrap();
    let (shell, cx, opened) = window(&machines.pc, cx);
    on_the_server(&shell, cx);
    let (_page, at) = card_of(&shell, &machines, "SHOW CLIP", cx);
    let play = format!("row-{at}-link-0-open");
    assert_eq!(label(cx, &play).as_deref(), Some("Play"));

    // The player is given a file of this machine as soon as its first chunk is in, and the rest
    // arrives into it.
    click(&shell, cx, &play);
    let copy = opened_file(&shell, &opened, cx);
    assert!(copy.to_str().unwrap().ends_with("clip.mp4"), "{copy:?}");
    eventually("the whole video arrives", || {
        std::fs::read(&copy).is_ok_and(|bytes| bytes == video)
    });
}

/// A tunnel to the server's `run` that the command line opened and holds: not the window's own.
/// Its answer, and the process that keeps it open.
fn opened_by_the_command_line(pc: &Machine, run: &str) -> (common::Spawned, Value) {
    use std::io::BufRead;
    let mut command = pc
        .user
        .command(&["--machine", "server", "tunnel", "open", run, "--json"]);
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = common::Spawned::new(command);
    let stdout = child.stdout.take().unwrap();
    let mut first = String::new();
    std::io::BufReader::new(stdout)
        .read_line(&mut first)
        .unwrap();
    let answer = serde_json::from_str(&first).unwrap_or_else(|_| panic!("not JSON: {first}"));
    (child, answer)
}

#[gpui_kit::test]
fn the_tunnels_of_another_machine_are_listed(cx: &mut TestAppContext) {
    let (_turn, machines) = machines();
    let url = format!("http://127.0.0.1:{}", machines.port);
    let run = web(&machines.server.user, &url, machines.port);
    // A tunnel the window did not open: it is the daemon that knows it.
    let (_held, answer) = opened_by_the_command_line(&machines.pc, &run);
    let id = answer["tunnel"].as_str().unwrap().to_owned();
    let local = answer["local_port"].as_u64().unwrap();

    let (shell, cx, _opened) = window(&machines.pc, cx);
    shell.update_in(cx, |s, window, cx| s.open(Selection::Services, window, cx));
    // They are listed by machine, each with the address it opens on, and Close.
    until(&shell, cx, "the tunnels are listed", |cx| {
        present(cx, &format!("tunnels-row-{id}"))
    });
    settle(&shell, cx);
    assert_eq!(
        label(cx, "tunnels-machine-server").as_deref(),
        Some("server")
    );
    assert_eq!(
        label(cx, &format!("tunnels-row-{id}")).as_deref(),
        Some(format!("localhost:{local} → server").as_str())
    );
    assert_eq!(
        label(cx, &format!("tunnels-row-{id}-close")).as_deref(),
        Some("Close")
    );

    // Close ends it on the daemon, whoever opened it, and its row folds away.
    click(&shell, cx, &format!("tunnels-row-{id}-close"));
    until(&shell, cx, "the daemon closed the tunnel", |_| {
        carried(&machines.pc).is_empty()
    });
    until(&shell, cx, "its row folds away", |cx| {
        !present(cx, &format!("tunnels-row-{id}"))
    });
    settle(&shell, cx);
    assert!(
        !present(cx, "tunnels-machine-server"),
        "nothing left to list"
    );
}
