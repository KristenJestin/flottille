//! The scenarios of #1271: the window holds every machine at once, and nothing switches
//! (`docs/remote.md`, "Requests from another machine's clients"). One connection per machine,
//! kept open; what is read is kept per machine; a page reads only what it shows. On GPUI's test
//! platform in front of real daemons, each with its own temporary HOME and XDG folders, fake agents
//! and no keyring, and a hub served in the test on 127.0.0.1. A relay in front of the window's
//! connections counts what is asked of the other machine, and can delay or swallow it. Throwaway
//! repositories. No window opens on a screen.

use crate::common;
use crate::remote::{Machine, TestHub, pc_and_server_on};

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::model::elsewhere_id;
use flottille_window::tree::{project_id, session_id};
use gpui_design_system::omarchy::Environment;
use gpui_design_system::toggle_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// How long a link, a session or a read of the machines may take.
const WAIT: Duration = Duration::from_secs(60);

/// What the server's agent answers.
const REPLY: &str = "The notes are ready.";

/// What the window asks to read the whole of a machine: none of it is a session's page.
const WHOLE_MACHINE: [&str; 7] = [
    "project.list",
    "workspace.list",
    "agent.list",
    "approval.list",
    "vault.asks",
    "notifications.list",
    "agent.trees",
];

/// What stands between the window and this machine's daemon: each request signed for `server`
/// is counted, then held `delay` (a link of that many milliseconds a round trip), or never sent
/// on while `silent`.
#[derive(Clone, Default)]
struct Relay {
    asked: Arc<Mutex<Vec<(String, Value)>>>,
    delay: Arc<AtomicU64>,
    silent: Arc<AtomicBool>,
    /// The connections the window opened, and how many of them are closed.
    opened: Arc<AtomicU64>,
    closed: Arc<AtomicU64>,
}

impl Relay {
    fn connect(&self, local: Connect) -> Connect {
        let relay = self.clone();
        Arc::new(move || {
            let daemon = local()?;
            let (window, ours) = UnixStream::pair()?;
            relay.opened.fetch_add(1, Ordering::SeqCst);
            let (to, back) = (ours.try_clone()?, daemon.try_clone()?);
            let (to_daemon, from_window) = (daemon, ours);
            let carried = relay.clone();
            std::thread::spawn(move || {
                carried.carry(&from_window, &to_daemon);
                carried.closed.fetch_add(1, Ordering::SeqCst);
                let _ = from_window.shutdown(std::net::Shutdown::Both);
                let _ = to_daemon.shutdown(std::net::Shutdown::Both);
            });
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut &back, &mut &to);
                let _ = to.shutdown(std::net::Shutdown::Both);
                let _ = back.shutdown(std::net::Shutdown::Both);
            });
            Ok(window)
        })
    }

    fn carry(&self, window: &UnixStream, daemon: &UnixStream) {
        let mut lines = BufReader::new(window);
        let mut line = String::new();
        loop {
            line.clear();
            match lines.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let request: Value = serde_json::from_str(line.trim_end()).unwrap_or_default();
            if request["machine"] == "server" {
                let method = request["signed"]["method"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let params = request["signed"]["params"]
                    .as_str()
                    .and_then(|params| serde_json::from_str(params).ok())
                    .unwrap_or_default();
                self.asked.lock().unwrap().push((method, params));
                if self.silent.load(Ordering::SeqCst) {
                    continue;
                }
                let delay = self.delay.load(Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(delay));
            }
            if (&*daemon).write_all(line.as_bytes()).is_err() {
                return;
            }
        }
    }

    /// The methods asked of the server since the last call.
    fn take(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut *self.asked.lock().unwrap())
    }
}

fn fake_user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// The project `name` on `user`'s machine, a throwaway repository.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

/// A session of `user` on `project`, which said [`REPLY`]: its id.
fn session(user: &User, project: &str, message: &str) -> String {
    let started = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the session answered", || {
        let shown = user
            .call("agent.show", json!({"agent": id, "after": 0}))
            .unwrap();
        shown.to_string().contains(REPLY)
    });
    id
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A hub, `pc` and `server`, each with the project `app` and a session there that answered;
/// `server` has `harbor` too. The ids of pc's session and the server's.
fn two_machines() -> (TestHub, Machine, Machine, String, String) {
    let script = json!({"turns": [{"when": "", "do": [{"say": REPLY}]}]});
    let pc_user = fake_user();
    pc_user.fake_script(script.clone());
    let server_user = fake_user();
    server_user.fake_script(script);
    let (hub, pc, server) = pc_and_server_on(Machine::of(pc_user), server_user);
    project(&pc.user, "app");
    project(&server.user, "app");
    project(&server.user, "harbor");
    let here = session(&pc.user, "app", "Tidy the readme");
    let there = session(&server.user, "harbor", "Write the release notes");
    (hub, pc, server, here, there)
}

fn window<'a>(
    machine: &Machine,
    relay: &Relay,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: machine.user.path("os-release"),
        state_home: machine.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect = relay.connect(Arc::new(machine.user.connector()));
    let device = Some(Device::in_folder(machine.folder()));
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    until(&shell, cx, "the server is read online", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .and_then(|read| read.machine("server"))
                .is_some_and(|server| server.online())
        })
    });
    settle(&shell, cx);
    (shell, cx)
}

/// One step of the window as the platform would run it.
fn step(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(Duration::from_millis(16));
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
        step(shell, cx);
        if check(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn settle(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..40 {
        step(shell, cx);
    }
}

fn click(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) {
    let id = id.into();
    cx.update(|window, cx| window.click(id, cx));
    cx.update(|window, cx| window.render_frame(cx));
}

fn present(cx: &mut VisualTestContext, id: impl Into<gpui_kit::ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

/// The open session's page shows `text` of its agent.
fn shows_reply(shell: &Entity<Shell>, cx: &mut VisualTestContext, id: &str) -> bool {
    shell.read_with(cx, |s, cx| {
        s.main().read(cx).session_page().is_some_and(|page| {
            let page = page.read(cx);
            page.id() == id
                && page
                    .conversation()
                    .read(cx)
                    .blocks()
                    .iter()
                    .any(|b| matches!(b, Block::Text { text } if text == REPLY))
        })
    })
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

/// The row of the server's session `id` in the one list.
fn row_there(id: &str) -> gpui_kit::ElementId {
    session_id(&elsewhere_id("server", id))
}

/// The server's session `there` opened from its row: the page drawn with its reply.
fn open_there(shell: &Entity<Shell>, cx: &mut VisualTestContext, there: &str) {
    until(shell, cx, "the server's session is listed", |cx| {
        present(cx, row_there(there))
    });
    click(cx, row_there(there));
    until(shell, cx, "the server's session is drawn", |cx| {
        shows_reply(shell, cx, there)
    });
}

#[gpui_kit::test]
fn opening_a_session_of_another_machine_reads_only_that_session(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    until(&shell, cx, "the server's session is listed", |cx| {
        present(cx, row_there(&there))
    });
    settle(&shell, cx);
    relay.take();

    open_there(&shell, cx, &there);
    settle(&shell, cx);
    let asked = relay.take();
    assert!(!asked.is_empty(), "the session was read on the server");
    let whole: Vec<&String> = asked
        .iter()
        .map(|(method, _)| method)
        .filter(|method| WHOLE_MACHINE.contains(&method.as_str()))
        .collect();
    assert!(
        whole.is_empty(),
        "the server was read whole for one session: {whole:?} in {asked:?}"
    );
    // Every agent it names is the session's own.
    for (method, params) in &asked {
        if let Some(agent) = params["agent"].as_str() {
            assert_eq!(agent, there, "{method} reads another session");
        }
    }
}

#[gpui_kit::test]
fn coming_back_to_a_machine_already_read_shows_its_copy_at_once(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, there) = two_machines();
    let relay = Relay::default();
    relay.delay.store(150, Ordering::SeqCst);
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);

    // This machine's session, then the server's again.
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });
    relay.take();
    click(cx, row_there(&there));
    step(&shell, cx);
    assert!(
        shows_reply(&shell, cx, &there),
        "the server's session waits for a read it already had"
    );
}

/// The values of Settings each machine has: its about's name and its developer mode.
fn set_settings(user: &User, name: &str, developer: bool) {
    user.call(
        "about.set",
        json!({"name": name, "pronouns": "", "notes": ""}),
    )
    .unwrap();
    user.call("developer.set", json!({"log": developer}))
        .unwrap();
    user.call(
        "rules.set",
        json!({"project": null, "text": format!("Rules of {name}.")}),
    )
    .unwrap();
}

/// What Settings shows now: the about's name, the developer mode and the rules, `None` for what
/// it does not show (it is being read).
fn settings_shown(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> (Option<String>, Option<bool>, Option<String>) {
    shell.read_with(cx, |s, cx| {
        let page = s.main().read(cx).settings_page().cloned().unwrap();
        let page = page.read(cx);
        (
            page.about()
                .read(cx)
                .shown()
                .map(|about| about.name.clone()),
            page.developer().map(|developer| developer.log),
            page.rules()
                .read(cx)
                .shown()
                .map(|rules| rules.text.clone()),
        )
    })
}

fn settings_of(shell: &Entity<Shell>, cx: &mut VisualTestContext, machine: &str) {
    click(cx, "settings-machine");
    step(shell, cx);
    click(cx, format!("settings-machine-{machine}"));
}

#[gpui_kit::test]
fn settings_of_another_machine_never_show_this_machine_s_values(cx: &mut TestAppContext) {
    let (_hub, pc, server, _here, _there) = two_machines();
    set_settings(&pc.user, "Pc person", true);
    set_settings(&server.user, "Server person", false);
    let relay = Relay::default();
    relay.delay.store(150, Ordering::SeqCst);
    let (shell, cx) = window(&pc, &relay, cx);
    click(cx, "settings");
    step(&shell, cx);
    click(cx, "settings-tabs-general");
    let pc_values = (
        Some("Pc person".to_owned()),
        Some(true),
        Some("Rules of Pc person.".to_owned()),
    );
    let server_values = (
        Some("Server person".to_owned()),
        Some(false),
        Some("Rules of Server person.".to_owned()),
    );
    until(&shell, cx, "this machine's settings are read", |cx| {
        settings_shown(&shell, cx) == pc_values
    });

    // The server chosen: while it is read nothing of this machine is shown, then its own.
    settings_of(&shell, cx, "server");
    let deadline = Instant::now() + WAIT;
    loop {
        step(&shell, cx);
        let shown = settings_shown(&shell, cx);
        assert_ne!(
            shown.0, pc_values.0,
            "the server's about says this machine's"
        );
        assert_ne!(
            shown.1, pc_values.1,
            "the server's developer mode is this machine's"
        );
        assert_ne!(
            shown.2, pc_values.2,
            "the server's rules are this machine's"
        );
        if shown == server_values {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the server's settings never came"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // And the reverse: this machine again, never the server's values.
    settings_of(&shell, cx, "here");
    for _ in 0..40 {
        step(&shell, cx);
        let shown = settings_shown(&shell, cx);
        assert_ne!(
            shown.0, server_values.0,
            "this machine's about says the server's"
        );
        assert_ne!(shown.1, server_values.1);
        assert_ne!(shown.2, server_values.2);
    }
    assert_eq!(settings_shown(&shell, cx), pc_values);
}

#[gpui_kit::test]
fn this_machine_s_connection_stays_open_while_another_machine_is_shown(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    settle(&shell, cx);
    let closed = relay.closed.load(Ordering::SeqCst);
    let read = shell.read_with(cx, |s, _| s.seen().snapshots.load(Ordering::SeqCst));

    open_there(&shell, cx, &there);
    click(cx, "settings");
    step(&shell, cx);
    settings_of(&shell, cx, "server");
    settle(&shell, cx);

    assert_eq!(
        relay.closed.load(Ordering::SeqCst),
        closed,
        "a connection of the window was cut to show the server"
    );
    // This machine was not read again from the start: its list is still its own.
    let reads = shell.read_with(cx, |s, _| s.seen().snapshots.load(Ordering::SeqCst));
    assert!(
        reads <= read + 1,
        "this machine was read {} times again",
        reads - read
    );
    assert!(
        present(cx, session_id(&here)),
        "this machine's session left the list"
    );
    assert!(present(cx, project_id("app")));
}

#[gpui_kit::test]
fn a_machine_that_does_not_answer_is_said_on_its_page_only(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    until(&shell, cx, "the server's session is listed", |cx| {
        present(cx, row_there(&there))
    });
    settle(&shell, cx);
    shell.update(cx, |_, _| {
        flottille_window::link::not_answering_after_on_this_thread(Duration::from_secs(2))
    });
    relay.silent.store(true, Ordering::SeqCst);

    click(cx, row_there(&there));
    let deadline = Instant::now() + WAIT;
    loop {
        step(&shell, cx);
        // Meanwhile the rest of the window is this machine's, untouched.
        assert!(present(cx, session_id(&here)), "this machine's list moved");
        if label(cx, "machine-not-answering").is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the page never said the server does not answer"
        );
        std::thread::sleep(Duration::from_millis(25));
        cx.executor().advance_clock(Duration::from_millis(200));
    }
    assert_eq!(
        label(cx, "machine-not-answering").as_deref(),
        Some("server does not answer")
    );
    assert!(
        toasts(&shell, cx).is_empty(),
        "said beyond its page: {:?}",
        toasts(&shell, cx)
    );
    // The window did not go back to this machine: the page is still the server's.
    assert_eq!(
        shell.read_with(cx, |s, _| s.machine_shown()),
        Some("server".to_owned())
    );

    // This machine's session opens as always.
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });
    assert!(!present(cx, "machine-not-answering"));
}

#[gpui_kit::test]
fn no_page_has_a_path_line(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    click(cx, "settings");
    step(&shell, cx);
    settings_of(&shell, cx, "server");
    until(&shell, cx, "the server's settings are shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    for tab in ["agents", "general", "vault"] {
        click(cx, format!("settings-tabs-{tab}"));
        settle(&shell, cx);
        assert!(!present(cx, "settings-where"), "Settings, {tab}");
    }
    // The server's page of a project: its name and its machines' marks say it, no path above.
    shell.update(cx, |shell, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "app", cx)
    });
    until(&shell, cx, "the server's page of app is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machine_shown().as_deref() == Some("server")
                && s.main().read(cx).project_page().is_some()
        })
    });
    settle(&shell, cx);
    assert!(!present(cx, "project-where"), "a project's page");
    click(cx, "project-settings");
    settle(&shell, cx);
    assert!(
        !present(cx, "project-settings-where"),
        "a project's settings"
    );
}

/// The fold of the project `name` in the sidebar.
fn fold_of(name: &str) -> gpui_kit::ElementId {
    toggle_id(project_id(name))
}

fn folded(shell: &Entity<Shell>, cx: &mut VisualTestContext, name: &str) -> bool {
    shell.read_with(cx, |s, cx| {
        s.project(name).is_some_and(|node| !node.read(cx).is_open())
    })
}

#[gpui_kit::test]
fn a_folded_project_stays_folded_and_listed_while_a_session_of_another_machine_is_open(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, _server, _here, there) = two_machines();
    project(&pc.user, "anchor");
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    until(&shell, cx, "anchor is listed", |cx| {
        present(cx, project_id("anchor"))
    });
    click(cx, fold_of("anchor"));
    settle(&shell, cx);
    assert!(folded(&shell, cx, "anchor"));

    open_there(&shell, cx, &there);
    settle(&shell, cx);
    assert!(
        present(cx, project_id("anchor")),
        "a project of this machine only left the list"
    );
    assert!(folded(&shell, cx, "anchor"), "the fold was lost");
}

#[gpui_kit::test]
fn coming_back_keeps_every_fold_as_it_was(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, there) = two_machines();
    project(&pc.user, "anchor");
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    until(&shell, cx, "anchor is listed", |cx| {
        present(cx, project_id("anchor"))
    });
    click(cx, fold_of("anchor"));
    settle(&shell, cx);

    open_there(&shell, cx, &there);
    settle(&shell, cx);
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });
    settle(&shell, cx);
    assert!(folded(&shell, cx, "anchor"), "anchor came back unfolded");
    assert!(!folded(&shell, cx, "app"), "app came back folded");
}

#[gpui_kit::test]
fn the_side_panel_of_another_machine_s_session_never_says_this_pc(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    // Its Workspace is read: the place of the session says where it is.
    until(&shell, cx, "the side panel's Workspace is read", |cx| {
        present(cx, "rail-workspace") && !present(cx, "rail-workspace-skeleton")
    });
    settle(&shell, cx);
    let said = cx.update(|window, _| {
        window
            .within("rail-workspace")
            .elements()
            .into_iter()
            .filter_map(|element| element.label().map(str::to_owned))
            .collect::<Vec<_>>()
    });
    assert!(!said.is_empty(), "the side panel's Workspace says nothing");
    assert!(
        said.iter().all(|said| !said.contains("this PC")),
        "{said:?}"
    );
    assert!(!present(cx, "rail-fact-Machine"), "{said:?}");
}

/// The time from a click on the server's session to its page drawn, behind a link of 150 ms a
/// round trip: the first visit, then coming back from this machine's session. For the pull
/// request (`--ignored --nocapture`).
#[gpui_kit::test]
#[ignore = "a measure, run by hand"]
fn the_time_from_the_click_to_the_page_drawn(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    until(&shell, cx, "the server's session is listed", |cx| {
        present(cx, row_there(&there))
    });
    settle(&shell, cx);
    relay.delay.store(150, Ordering::SeqCst);
    let mut measure = |cx: &mut VisualTestContext, what: &str| {
        let began = Instant::now();
        click(cx, row_there(&there));
        until(&shell, cx, "the server's session is drawn", |cx| {
            shows_reply(&shell, cx, &there)
        });
        println!("{what}: {} ms", began.elapsed().as_millis());
        settle(&shell, cx);
        click(cx, session_id(&here));
        until(&shell, cx, "this machine's session is drawn", |cx| {
            shows_reply(&shell, cx, &here)
        });
        settle(&shell, cx);
    };
    measure(cx, "first visit");
    measure(cx, "coming back");
    // Settings of the server, first then again.
    click(cx, "settings");
    step(&shell, cx);
    click(cx, "settings-tabs-general");
    settle(&shell, cx);
    for what in ["settings, first visit", "settings, coming back"] {
        let began = Instant::now();
        settings_of(&shell, cx, "server");
        until(&shell, cx, "the server's settings are drawn", |cx| {
            settings_shown(&shell, cx).0.is_some()
                && shell.read_with(cx, |s, _| s.machine_shown()).is_some()
        });
        println!("{what}: {} ms", began.elapsed().as_millis());
        settings_of(&shell, cx, "here");
        until(&shell, cx, "this machine's settings are drawn", |cx| {
            shell.read_with(cx, |s, _| s.machine_shown()).is_none()
        });
        settle(&shell, cx);
    }
    let _ = label;
}
