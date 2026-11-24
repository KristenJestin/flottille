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

/// What the window asks to read the whole of a machine, the shell's first read: none of it is a
/// session's page. (Its page reads the requests of its tree, `approval.list` with that tree, and
/// `vault.asks`, which has no filter and which it keeps to its session's.)
const WHOLE_MACHINE: [&str; 5] = [
    "project.list",
    "workspace.list",
    "agent.list",
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
    /// The methods whose request ends its connection there: the window hears it refused.
    cut: Arc<Mutex<Vec<&'static str>>>,
    /// The methods this machine's daemon is asked under a name it does not know: it refuses them.
    refuse: Arc<Mutex<Vec<&'static str>>>,
    /// The links to the server that follow it as a window (a copy's) and are still open.
    watching: Arc<AtomicU64>,
    /// The methods of this machine's daemon held this long (ms) before they are sent on.
    slow: Arc<Mutex<Vec<&'static str>>>,
    slow_by: Arc<AtomicU64>,
}

impl Relay {
    fn connect(&self, local: Connect) -> Connect {
        let relay = self.clone();
        Arc::new(move || {
            let daemon = local()?;
            let (window, ours) = UnixStream::pair()?;
            let (to, back) = (ours.try_clone()?, daemon.try_clone()?);
            let (to_daemon, from_window) = (daemon, ours);
            let carried = relay.clone();
            std::thread::spawn(move || {
                carried.carry(&from_window, &to_daemon);
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
        let mut watching = false;
        self.carry_counted(window, daemon, &mut watching);
        if watching {
            self.watching.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn carry_counted(&self, window: &UnixStream, daemon: &UnixStream, watching: &mut bool) {
        let mut lines = BufReader::new(window);
        let mut line = String::new();
        loop {
            line.clear();
            match lines.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let mut request: Value = serde_json::from_str(line.trim_end()).unwrap_or_default();
            let refused = request["method"]
                .as_str()
                .is_some_and(|method| self.refuse.lock().unwrap().contains(&method));
            if refused {
                request["method"] = json!("test.refused");
                line = format!("{request}\n");
            }
            let slow = request["method"]
                .as_str()
                .is_some_and(|method| self.slow.lock().unwrap().contains(&method));
            if slow {
                std::thread::sleep(Duration::from_millis(self.slow_by.load(Ordering::SeqCst)));
            }
            if request["machine"] == "server" {
                let method = request["signed"]["method"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let params: Value = request["signed"]["params"]
                    .as_str()
                    .and_then(|params| serde_json::from_str(params).ok())
                    .unwrap_or_default();
                if method == "events.watch" && params["client"] == "window" && !*watching {
                    *watching = true;
                    self.watching.fetch_add(1, Ordering::SeqCst);
                }
                let cut = self.cut.lock().unwrap().contains(&method.as_str());
                self.asked.lock().unwrap().push((method, params));
                if cut {
                    return;
                }
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
    let script = json!({"turns": [
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "", "do": [{"say": REPLY}]}
    ]});
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
    // Every agent or tree it names is the session's own, and the requests are its tree's.
    for (method, params) in &asked {
        for key in ["agent", "tree"] {
            if let Some(agent) = params[key].as_str() {
                assert_eq!(agent, there, "{method} reads another session");
            }
        }
        if method == "approval.list" {
            assert_eq!(
                params["tree"],
                there.as_str(),
                "the requests of the whole server"
            );
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
    let linked = shell.read_with(cx, |s, _| s.seen().connections.load(Ordering::SeqCst));
    let read = shell.read_with(cx, |s, _| s.seen().snapshots.load(Ordering::SeqCst));

    open_there(&shell, cx, &there);
    click(cx, "settings");
    step(&shell, cx);
    settings_of(&shell, cx, "server");
    settle(&shell, cx);

    assert_eq!(
        shell.read_with(cx, |s, _| s.seen().connections.load(Ordering::SeqCst)),
        linked,
        "this machine's link was cut to show the server"
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
    // The line folds away with the page it was said on.
    until(&shell, cx, "the line folds away", |cx| {
        !present(cx, "machine-not-answering")
    });
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
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "app", window, cx)
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
        present(cx, "rail-workspace-read")
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
    let (_hub, pc, server, here, there) = two_machines();
    set_settings(&pc.user, "Pc person", true);
    set_settings(&server.user, "Server person", false);
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    until(&shell, cx, "the server's session is listed", |cx| {
        present(cx, row_there(&there))
    });
    settle(&shell, cx);
    relay.delay.store(150, Ordering::SeqCst);
    let measure = |cx: &mut VisualTestContext, what: &str| {
        let began = Instant::now();
        click(cx, row_there(&there));
        until(&shell, cx, "the server's session is drawn", |cx| {
            shows_reply(&shell, cx, &there)
        });
        println!("{what}: {} ms", began.elapsed().as_millis());
        settle(&shell, cx);
        // Before #1271 this machine's row was another one while the server was in front.
        let home = match present(cx, session_id(&here)) {
            true => session_id(&here),
            false => session_id(&elsewhere_id("pc", &here)),
        };
        click(cx, home);
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
            settings_shown(&shell, cx).0.as_deref() == Some("Server person")
        });
        println!("{what}: {} ms", began.elapsed().as_millis());
        settings_of(&shell, cx, "here");
        until(&shell, cx, "this machine's settings are drawn", |cx| {
            settings_shown(&shell, cx).0.as_deref() == Some("Pc person")
        });
        settle(&shell, cx);
    }
    let _ = label;
}

// Round 2 of #1271: every page sends to its own machine, and the window's ways back keep it.

/// The id of a server's session listed on the server since `before`.
fn started_on(user: &User, before: &[String]) -> Option<String> {
    let listed = user.call("agent.list", json!({})).unwrap();
    listed["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| agent["id"].as_str())
        .find(|id| !before.iter().any(|known| known == id))
        .map(str::to_owned)
}

fn agents_of(user: &User) -> Vec<String> {
    let listed = user.call("agent.list", json!({})).unwrap();
    listed["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| agent["id"].as_str().map(str::to_owned))
        .collect()
}

/// The conversation of `agent` on `user`'s machine holds `text`.
fn conversation_holds(user: &User, agent: &str, text: &str) -> bool {
    user.call("agent.show", json!({"agent": agent, "after": 0}))
        .is_ok_and(|shown| shown.to_string().contains(text))
}

/// Whatever the server was asked since the last take names `text`.
fn asked_of_the_server(relay: &Relay, text: &str) -> Vec<String> {
    relay
        .take()
        .into_iter()
        .filter(|(_, params)| params.to_string().contains(text))
        .map(|(method, _)| method)
        .collect()
}

#[gpui_kit::test]
fn a_reply_on_a_page_of_this_pc_while_another_machine_is_in_front_goes_to_this_pc(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, _server, here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    until(&shell, cx, "this machine's session is listed", |cx| {
        present(cx, session_id(&here))
    });
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });

    // Add project, the server chosen in it, then cancelled: the page behind stayed.
    shell.update_in(cx, |s, window, cx| s.add_project(window, cx));
    settle(&shell, cx);
    click(cx, "add-project-machine-server");
    until(&shell, cx, "the server's folders are listed", |cx| {
        shell.read_with(cx, |s, cx| {
            s.add_project_form()
                .read(cx)
                .folders_of(Some("server"))
                .is_some()
        })
    });
    let form = shell.read_with(cx, |s, _| s.add_project_form().clone());
    form.update(cx, |form, cx| form.close(cx));
    settle(&shell, cx);
    assert!(shows_reply(&shell, cx, &here), "the page behind left");
    relay.take();

    click(cx, "reply-field");
    cx.simulate_input("Note the date");
    cx.simulate_keystrokes("enter");
    eventually("this machine's session got the reply", || {
        conversation_holds(&pc.user, &here, "Note the date")
    });
    settle(&shell, cx);
    assert_eq!(
        asked_of_the_server(&relay, "Note the date"),
        Vec::<String>::new()
    );
}

#[gpui_kit::test]
fn a_rename_from_the_sidebar_goes_to_the_session_s_machine(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    relay.take();

    // This machine's session, renamed from its row while the server's page is shown.
    let session = here.clone();
    shell.update_in(cx, |s, window, cx| {
        s.rename_of(&session, window, cx);
        let field = s.rename_form().read(cx).field().clone();
        field.update(cx, |field, cx| field.set_value("Renamed here", window, cx));
    });
    settle(&shell, cx);
    click(cx, "rename-save");
    eventually("this machine's session is renamed", || {
        pc.user
            .call("agent.show", json!({"agent": here, "after": 0}))
            .is_ok_and(|shown| shown["agent"]["title"] == "Renamed here")
    });
    settle(&shell, cx);
    assert_eq!(
        asked_of_the_server(&relay, "Renamed here"),
        Vec::<String>::new()
    );
}

/// A PNG of 4 by 4, as a picture pasted in a message.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x08, 0x02, 0x00, 0x00, 0x00, 0x26, 0x93, 0x09,
    0x29, 0x00, 0x00, 0x00, 0x13, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xfc, 0xcf, 0x80, 0x0f,
    0x30, 0x31, 0x20, 0x01, 0x46, 0x3c, 0x0a, 0x00, 0x3d, 0x4e, 0x01, 0x07, 0x2c, 0x6f, 0x7a, 0x86,
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn pictures_read(shell: &Entity<Shell>, cx: &mut VisualTestContext, id: &str) -> usize {
    shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .session_page()
            .filter(|page| page.read(cx).id() == id)
            .map_or(0, |page| page.read(cx).pictures_read())
    })
}

#[gpui_kit::test]
fn a_kept_page_of_another_machine_gets_its_pictures(cx: &mut TestAppContext) {
    let (_hub, pc, server, here, there) = two_machines_taking_images();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });

    // While this machine's page is shown, the server's session gets a picture: its kept page
    // follows it, and asks the server for the picture.
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.encode(PNG);
    server
        .user
        .call(
            "agent.send",
            json!({"agent": there, "message": "Look at this",
                   "images": [{"mime": "image/png", "data": data}]}),
        )
        .unwrap();
    eventually("the server's session took the picture", || {
        conversation_holds(&server.user, &there, "Look at this")
    });
    for _ in 0..100 {
        step(&shell, cx);
        std::thread::sleep(Duration::from_millis(20));
    }

    click(cx, row_there(&there));
    until(&shell, cx, "the server's page shows its picture", |cx| {
        pictures_read(&shell, cx, &there) >= 1
    });
}

/// [`two_machines`], each machine's fake agent taking images.
fn two_machines_taking_images() -> (TestHub, Machine, Machine, String, String) {
    let script = json!({"images": true, "turns": [{"when": "", "do": [{"say": REPLY}]}]});
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

fn back(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.update_in(cx, |s, window, cx| {
        s.go(flottille_window::history::Way::Back, window, cx)
    });
}

#[gpui_kit::test]
fn back_from_a_session_of_this_pc_returns_to_the_server_s_settings(cx: &mut TestAppContext) {
    let (_hub, pc, server, here, _there) = two_machines();
    set_settings(&pc.user, "Pc person", true);
    set_settings(&server.user, "Server person", false);
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    click(cx, "settings");
    step(&shell, cx);
    click(cx, "settings-tabs-general");
    settings_of(&shell, cx, "server");
    until(&shell, cx, "the server's settings are shown", |cx| {
        settings_shown(&shell, cx).0.as_deref() == Some("Server person")
    });
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });

    back(&shell, cx);
    for _ in 0..40 {
        step(&shell, cx);
        assert_eq!(
            shell.read_with(cx, |s, _| s.machine_shown()).as_deref(),
            Some("server"),
            "back went to this machine's settings"
        );
    }
    until(&shell, cx, "the server's settings are shown again", |cx| {
        settings_shown(&shell, cx).0.as_deref() == Some("Server person")
    });
}

#[gpui_kit::test]
fn back_from_a_session_of_this_pc_returns_to_the_server_s_settings_of_app(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "app", window, cx)
    });
    until(&shell, cx, "the server's page of app is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machine_shown().as_deref() == Some("server")
                && s.main().read(cx).project_page().is_some()
        })
    });
    settle(&shell, cx);
    click(cx, "project-settings");
    until(&shell, cx, "the server's settings of app", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().is_some()
        })
    });
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });

    back(&shell, cx);
    settle(&shell, cx);
    shell.read_with(cx, |s, cx| {
        assert_eq!(
            s.machine_shown().as_deref(),
            Some("server"),
            "back went to this machine's settings of app"
        );
        assert!(s.main().read(cx).project_settings_page().is_some());
    });
}

#[gpui_kit::test]
fn a_session_started_from_another_machine_s_project_page_opens_that_session(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server, _here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "app", window, cx)
    });
    until(&shell, cx, "the server's page of app is drawn", |cx| {
        present(cx, "composer-field")
            && shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    let before = agents_of(&server.user);
    click(cx, "composer-field");
    cx.simulate_input("Count the files");
    click(cx, "send");
    eventually("the session started on the server", || {
        started_on(&server.user, &before).is_some()
    });
    let started = started_on(&server.user, &before).unwrap();
    until(&shell, cx, "the started session is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machine_shown().as_deref() == Some("server")
                && s.main()
                    .read(cx)
                    .session_page()
                    .is_some_and(|page| page.read(cx).id() == started)
        })
    });
}

#[gpui_kit::test]
fn the_does_not_answer_line_shows_on_a_project_page_of_a_silent_machine(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    shell.update(cx, |_, _| {
        flottille_window::link::not_answering_after_on_this_thread(Duration::from_secs(2))
    });
    relay.silent.store(true, Ordering::SeqCst);

    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "harbor", window, cx)
    });
    let deadline = Instant::now() + WAIT;
    loop {
        step(&shell, cx);
        if label(cx, "machine-not-answering").is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the project's page never said the server does not answer"
        );
        std::thread::sleep(Duration::from_millis(25));
        cx.executor().advance_clock(Duration::from_millis(200));
    }
    assert_eq!(
        label(cx, "machine-not-answering").as_deref(),
        Some("server does not answer")
    );
    // The page's own link then says it waits: its words are no answer of the server.
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        step(&shell, cx);
        assert!(
            present(cx, "machine-not-answering"),
            "the line went while the server still does not answer"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[gpui_kit::test]
fn a_project_removed_answer_from_the_server_does_not_touch_this_pc_s_list(cx: &mut TestAppContext) {
    let (_hub, pc, server, _here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "app", window, cx)
    });
    until(&shell, cx, "the server's page of app is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machine_shown().as_deref() == Some("server")
                && s.main().read(cx).project_page().is_some()
        })
    });
    settle(&shell, cx);

    // The server's app removed, and this machine's app shown before the answer comes.
    relay.delay.store(1500, Ordering::SeqCst);
    shell.update(cx, |s, _| {
        s.perform(flottille_window::actions::Command::RemoveProject {
            project: "app".to_owned(),
            discard: None,
        })
    });
    shell.update_in(cx, |shell, window, cx| {
        shell.open(
            flottille_window::shell::Selection::Project("app".to_owned()),
            window,
            cx,
        )
    });
    until(&shell, cx, "this machine's page of app is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machine_shown().is_none()
                && s.main()
                    .read(cx)
                    .project_page()
                    .is_some_and(|page| page.read(cx).name() == "app")
        })
    });
    eventually("the server removed its app", || {
        let listed = server.user.call("project.list", json!({})).unwrap();
        !listed.to_string().contains("\"app\"")
    });
    relay.delay.store(0, Ordering::SeqCst);
    for _ in 0..60 {
        step(&shell, cx);
        std::thread::sleep(Duration::from_millis(20));
    }
    shell.read_with(cx, |s, cx| {
        assert!(
            s.snapshot().projects.iter().any(|p| p.name == "app"),
            "this machine's app left its list"
        );
        assert_eq!(s.machine_shown(), None);
        assert!(
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == "app"),
            "this machine's page of app was left"
        );
    });
    assert!(present(cx, project_id("app")));
}

#[gpui_kit::test]
fn the_key_read_overtaken_does_not_jump(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    until(&shell, cx, "the server's session is listed", |cx| {
        present(cx, row_there(&there))
    });
    settle(&shell, cx);

    // The server's session asked, its key not read yet; this machine's asked meanwhile.
    let (server_agent, home) = (there.clone(), here.clone());
    shell.update_in(cx, |shell, window, cx| {
        shell.open_elsewhere("server".to_owned(), server_agent, window, cx);
        shell.open_here(home, window, cx);
    });
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });
    for _ in 0..40 {
        step(&shell, cx);
        assert_eq!(
            shell.read_with(cx, |s, _| s.machine_shown()),
            None,
            "the window jumped to the server"
        );
        assert!(shows_reply(&shell, cx, &here), "the page jumped");
    }
}

#[gpui_kit::test]
fn removing_a_machine_drops_its_pages_and_copies(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    let dialogs = shell.read_with(cx, |s, cx| s.dialogs_held(cx));
    open_there(&shell, cx, &there);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "harbor", window, cx)
    });
    until(&shell, cx, "the server's page of harbor", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    settle(&shell, cx);
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });
    assert!(shell.read_with(cx, |s, _| s.holds_pages_of("server")));
    assert!(shell.read_with(cx, |s, cx| s.dialogs_held(cx)) > dialogs);

    let removed = pc.user.run(&["remote", "remove", "server"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    until(&shell, cx, "the server's pages are dropped", |cx| {
        !shell.read_with(cx, |s, _| s.holds_pages_of("server"))
    });
    assert_eq!(shell.read_with(cx, |s, cx| s.dialogs_held(cx)), dialogs);
}

fn run_for(by: Duration, cx: &mut VisualTestContext) {
    let frame = Duration::from_millis(16);
    let mut gone = Duration::ZERO;
    while gone < by {
        cx.executor().advance_clock(frame);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += frame;
    }
}

fn y_of(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| {
        let found = window.try_find(id.clone());
        f32::from(
            found
                .unwrap_or_else(|| panic!("{id} is not drawn"))
                .bounds()
                .origin
                .y,
        )
    })
}

#[gpui_kit::test]
fn choosing_another_machine_in_settings_enters_its_sections_one_after_the_other(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, _server, _here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    click(cx, "settings");
    step(&shell, cx);
    click(cx, "settings-tabs-general");
    settle(&shell, cx);
    // The server once, its key read; then this machine, then the server again.
    settings_of(&shell, cx, "server");
    until(&shell, cx, "the server's settings are shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    settings_of(&shell, cx, "here");
    settle(&shell, cx);
    run_for(Duration::from_millis(1500), cx);

    click(cx, "settings-machine");
    step(&shell, cx);
    click(cx, "settings-machine-server");
    let ids = ["header", "tabs", "about"].map(|part| format!("enter-settings-server-{part}"));
    run_for(Duration::from_millis(120), cx);
    let waiting: Vec<f32> = ids.iter().map(|id| y_of(cx, id)).collect();
    run_for(Duration::from_millis(1500), cx);
    let arrived: Vec<f32> = ids.iter().map(|id| y_of(cx, id)).collect();
    let rise = |at: usize| waiting[at] - arrived[at];
    for (at, id) in ids.iter().enumerate().skip(1) {
        assert!(
            rise(at) >= rise(at - 1) - 0.01,
            "{id} had less to rise ({}) than the part before it ({})",
            rise(at),
            rise(at - 1)
        );
    }
    assert!(
        rise(2) > 1.0,
        "the server's sections did not enter: {}",
        rise(2)
    );
}

/// Waits until the link has sent more reads of this machine than `before`, without the window
/// taking them: they wait in its queue.
fn queued(shell: &Entity<Shell>, before: u64, cx: &mut VisualTestContext) {
    let deadline = Instant::now() + WAIT;
    while snapshots(shell, cx) <= before {
        assert!(Instant::now() < deadline, "no read was sent");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn snapshots(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> u64 {
    shell.read_with(cx, |s, _| s.seen().snapshots.load(Ordering::SeqCst))
}

fn harbor_of_the_server_shown(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> bool {
    shell.read_with(cx, |s, cx| {
        s.machine_shown().as_deref() == Some("server")
            && s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == "harbor")
    })
}

#[gpui_kit::test]
fn a_read_of_this_machine_queued_before_the_server_s_page_does_not_fill_it(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, _server, _here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    settle(&shell, cx);

    // A read of this machine, sent and not yet taken, when the server's page is asked.
    let before = snapshots(&shell, cx);
    project(&pc.user, "anchor");
    queued(&shell, before, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "harbor", window, cx)
    });
    until(&shell, cx, "the server's page of harbor", |cx| {
        harbor_of_the_server_shown(&shell, cx)
    });
    settle(&shell, cx);
    // That read is this machine's: its list has anchor, and the server's page stays its own.
    assert!(harbor_of_the_server_shown(&shell, cx));
    assert!(
        shell.read_with(cx, |s, _| s
            .snapshot()
            .projects
            .iter()
            .any(|p| p.name == "anchor")),
        "this machine's read was lost"
    );
    let progress = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .project_page()
            .map(|page| page.read(cx).name().to_owned())
    });
    assert_eq!(progress.as_deref(), Some("harbor"));
}

#[gpui_kit::test]
fn a_read_of_the_server_that_comes_after_its_page_was_left_is_not_shown(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, _there) = two_machines();
    let relay = Relay::default();
    // This machine's own list is the slow one to come, as on a loaded runner: the window is
    // shown before it holds `app`.
    relay.slow.lock().unwrap().push("project.list");
    relay.slow_by.store(3000, Ordering::SeqCst);
    let (shell, cx) = window(&pc, &relay, cx);
    settle(&shell, cx);

    // The server's page asked, its read slow; this machine's page shown before it comes.
    relay.delay.store(1500, Ordering::SeqCst);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "harbor", window, cx)
    });
    until(&shell, cx, "the server is in front", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    shell.update_in(cx, |shell, window, cx| {
        shell.open(
            flottille_window::shell::Selection::Project("app".to_owned()),
            window,
            cx,
        )
    });
    for _ in 0..200 {
        step(&shell, cx);
        std::thread::sleep(Duration::from_millis(20));
    }
    // Late, it is not drawn as this machine's: the list and the page are this machine's.
    shell.read_with(cx, |s, cx| {
        assert_eq!(
            s.machine_shown(),
            None,
            "the late read brought the server back"
        );
        assert!(
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == "app"),
            "the late read moved this machine's page"
        );
    });
}

/// Types `text` in the open page's composer and sends it.
fn send(cx: &mut VisualTestContext, text: &str) {
    click(cx, "reply-field");
    cx.simulate_input(text);
    cx.simulate_keystrokes("enter");
}

/// The composer of the server's session `there`.
fn reply_there(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    there: &str,
) -> Entity<flottille_window::session::Reply> {
    let row = elsewhere_id("server", there);
    shell.read_with(cx, |s, _| s.reply_of(&row).cloned().expect("its composer"))
}

#[gpui_kit::test]
fn a_second_message_to_the_server_s_session_is_sent_too(cx: &mut TestAppContext) {
    let (_hub, pc, server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);

    send(cx, "First note");
    eventually("the server's session got the first message", || {
        conversation_holds(&server.user, &there, "First note")
    });
    let reply = reply_there(&shell, cx, &there);
    until(&shell, cx, "the composer is free after the first", |cx| {
        !reply.read_with(cx, |reply, _| reply.is_sending())
    });
    settle(&shell, cx);
    send(cx, "Second note");
    eventually("the server's session got the second message", || {
        conversation_holds(&server.user, &there, "Second note")
    });
    until(&shell, cx, "the composer is free again", |cx| {
        !reply.read_with(cx, |reply, _| reply.is_sending())
    });
}

#[gpui_kit::test]
fn a_mode_change_on_the_server_s_session_gets_its_answer(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    // The server never takes it: what the window hears is its refusal.
    relay.cut.lock().unwrap().push("agent.mode");

    let mode = reply_there(&shell, cx, &there).read_with(cx, |reply, _| reply.mode.clone());
    mode.update(cx, |mode, cx| mode.choose("full", cx));
    until(&shell, cx, "the switch says the change was refused", |cx| {
        mode.read_with(cx, |mode, _| mode.said().is_some())
    });
}

#[gpui_kit::test]
fn a_model_change_on_the_server_s_session_gets_its_answer(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    relay.cut.lock().unwrap().push("agent.options");

    let options = reply_there(&shell, cx, &there).read_with(cx, |reply, _| reply.options().clone());
    options.update(cx, |options, cx| {
        let chosen = gpui_design_system::Chosen::Model("fake".into(), Some("fake-large".into()));
        options.choose(chosen, None, cx);
    });
    until(&shell, cx, "a toast says the change was refused", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|title| title == "The model or effort was not changed")
    });
}

#[gpui_kit::test]
fn interrupting_the_server_s_session_gets_its_answer(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    send(cx, "HANG on a long task");
    let reply = reply_there(&shell, cx, &there);
    until(&shell, cx, "the server's turn runs", |cx| {
        reply.read_with(cx, |reply, _| reply.is_running())
    });

    reply.update(cx, |reply, cx| reply.interrupt(cx));
    until(&shell, cx, "the interruption is answered", |cx| {
        !reply.read_with(cx, |reply, _| reply.is_interrupting())
    });
}

#[gpui_kit::test]
fn a_draft_read_of_the_server_s_session_reaches_its_composer(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);

    // The window asks what it keeps of the draft: the answer reaches the composer, which may
    // then write one.
    let reply = reply_there(&shell, cx, &there);
    until(&shell, cx, "the composer heard what its draft is", |cx| {
        reply.read_with(cx, |reply, _| reply.draft_was_read())
    });
}

#[gpui_kit::test]
fn renaming_the_server_s_session_from_its_header_renames_it_there(cx: &mut TestAppContext) {
    let (_hub, pc, server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);

    click(cx, "session-rename");
    settle(&shell, cx);
    shell.update_in(cx, |s, window, cx| {
        let field = s.rename_form().read(cx).field().clone();
        field.update(cx, |field, cx| field.set_value("Renamed there", window, cx));
    });
    settle(&shell, cx);
    click(cx, "rename-save");
    eventually("the server's session is renamed", || {
        server
            .user
            .call("agent.show", json!({"agent": there, "after": 0}))
            .is_ok_and(|shown| shown["agent"]["title"] == "Renamed there")
    });
}

#[gpui_kit::test]
fn a_failed_read_of_the_machines_forgets_no_machine(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });
    assert!(shell.read_with(cx, |s, _| s.holds_pages_of("server")));

    relay.refuse.lock().unwrap().push("remote.machines");
    until(&shell, cx, "the machines' read fails", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .is_some_and(|read| read.off.is_some())
        })
    });
    settle(&shell, cx);
    assert!(
        shell.read_with(cx, |s, _| s.holds_pages_of("server")),
        "a read that failed dropped the server's pages"
    );
}

#[gpui_kit::test]
fn back_to_a_machine_gone_offline_leaves_the_history_where_it_was(cx: &mut TestAppContext) {
    let (_hub, pc, server, here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    until(&shell, cx, "this machine's session is listed", |cx| {
        present(cx, session_id(&here))
    });
    click(cx, session_id(&here));
    until(&shell, cx, "this machine's session is drawn", |cx| {
        shows_reply(&shell, cx, &here)
    });
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    let again = here.clone();
    shell.update_in(cx, |s, window, cx| {
        s.open(
            flottille_window::shell::Selection::Session(again),
            window,
            cx,
        )
    });
    until(&shell, cx, "this machine's session is drawn again", |cx| {
        shows_reply(&shell, cx, &here)
    });

    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(&shell, cx, "the window knows the server is offline", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .and_then(|read| read.machine("server"))
                .is_some_and(|server| !server.online())
        })
    });
    let before = shell.read_with(cx, |s, _| s.visit_shown().cloned());
    back(&shell, cx);
    settle(&shell, cx);
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|title| title == "server cannot be shown"),
        "{:?}",
        toasts(&shell, cx)
    );
    assert!(shows_reply(&shell, cx, &here), "the page left");
    let after = shell.read_with(cx, |s, _| s.visit_shown().cloned());
    assert_eq!(
        after.map(|visit| visit.selection),
        before.map(|visit| visit.selection),
        "the history moved to a page not shown"
    );
}

#[gpui_kit::test]
fn a_read_asked_by_the_server_s_answer_after_its_page_was_left_goes_to_the_server(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server, here, _there) = two_machines();
    server
        .user
        .call(
            "vault.set",
            json!({"name": "old-key", "secret": "an-invented-test-value"}),
        )
        .unwrap();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    click(cx, "settings");
    step(&shell, cx);
    settings_of(&shell, cx, "server");
    until(&shell, cx, "the server's settings are shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    let vault = shell.read_with(cx, |s, cx| {
        let page = s.main().read(cx).settings_page().cloned().unwrap();
        page.read(cx).vault().clone()
    });
    vault.update(cx, |vault, cx| vault.read(cx));
    until(&shell, cx, "the server's vault is listed", |cx| {
        vault.read_with(cx, |vault, _| {
            vault
                .entries()
                .is_some_and(|entries| entries.iter().any(|e| e.name == "old-key"))
        })
    });

    // Removed on the server, whose answer comes once this machine's session is shown: the list
    // it reads again is the server's.
    relay.delay.store(1500, Ordering::SeqCst);
    vault.update(cx, |vault, cx| {
        vault.remove("old-key", cx);
        vault.remove("old-key", cx);
    });
    step(&shell, cx);
    click(cx, session_id(&here));
    relay.delay.store(0, Ordering::SeqCst);
    eventually("the server's entry is removed", || {
        let listed = server.user.call("vault.list", json!({})).unwrap();
        !listed.to_string().contains("old-key")
    });
    relay.take();
    until(&shell, cx, "the server's vault is read again", |cx| {
        let _ = cx;
        relay
            .asked
            .lock()
            .unwrap()
            .iter()
            .any(|(method, _)| method == "vault.list")
    });
}

#[gpui_kit::test]
fn the_link_of_a_copy_ends_once_its_page_is_left(cx: &mut TestAppContext) {
    let (_hub, pc, _server, here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "harbor", window, cx)
    });
    until(
        &shell,
        cx,
        "the server's page of harbor follows the server",
        |cx| {
            shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
                && relay.watching.load(Ordering::SeqCst) == 1
        },
    );
    settle(&shell, cx);

    // Nothing happens on the server meanwhile: the link ends all the same.
    click(cx, session_id(&here));
    until(&shell, cx, "the copy's link ended", |cx| {
        let _ = cx;
        relay.watching.load(Ordering::SeqCst) == 0
    });
}

#[gpui_kit::test]
fn add_project_on_this_pc_is_not_held_by_the_server_behind_gone_offline(cx: &mut TestAppContext) {
    let (_hub, pc, server, _here, there) = two_machines();
    let folder = pc.user.path("work/notes");
    pc.user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "notes\n").unwrap();
    pc.user.git(&folder, &["add", "README.md"]);
    pc.user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(&shell, cx, "the server's page is held", |cx| {
        cx.update(|_, cx| flottille_window::machines::held(cx).is_some())
    });

    // Added on this PC, while the server's page stays behind.
    shell.update_in(cx, |s, window, cx| s.add_project(window, cx));
    settle(&shell, cx);
    if present(cx, "add-project-machine-here") {
        click(cx, "add-project-machine-here");
        settle(&shell, cx);
        click(cx, "add-project-step-next");
        settle(&shell, cx);
    }
    let form = shell.read_with(cx, |s, _| s.add_project_form().clone());
    assert_eq!(
        form.read_with(cx, |f, _| f.machine().map(str::to_owned)),
        None
    );
    cx.simulate_input(folder.to_str().unwrap());
    cx.simulate_keystrokes("enter");
    until(&shell, cx, "the folder is detected", |cx| {
        form.read_with(cx, |f, _| f.detected().is_some())
    });
    settle(&shell, cx);
    click(cx, "add-project-next");
    eventually("the project is this PC's", || {
        pc.user
            .call("project.list", json!({}))
            .unwrap()
            .to_string()
            .contains("\"notes\"")
    });
}

#[gpui_kit::test]
fn a_session_started_on_the_server_is_listed_with_its_brief_before_the_server_lists_it(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server, _here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "app", window, cx)
    });
    until(&shell, cx, "the server's page of app is drawn", |cx| {
        present(cx, "composer-field")
            && shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    // The lists of the machines come late: the row is the window's own until then.
    relay.slow_by.store(8000, Ordering::SeqCst);
    relay.slow.lock().unwrap().push("remote.machines");
    let before = agents_of(&server.user);
    click(cx, "composer-field");
    cx.simulate_input("Count the files");
    click(cx, "send");
    eventually("the session started on the server", || {
        started_on(&server.user, &before).is_some()
    });
    let started = started_on(&server.user, &before).unwrap();
    let row = elsewhere_id("server", &started);
    until(&shell, cx, "the started session is listed", |cx| {
        shell.read_with(cx, |s, _| s.snapshot().elsewhere(&row).is_some())
    });
    let title = shell.read_with(cx, |s, _| {
        s.snapshot().elsewhere(&row).unwrap().title.clone()
    });
    assert_eq!(title, "Count the files");
}

#[gpui_kit::test]
fn a_connection_request_of_a_removed_machine_closes(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    open_there(&shell, cx, &there);
    settle(&shell, cx);
    relay.take();

    // A request's dialog opened from the server's page reads it there.
    shell.update(cx, |s, cx| s.open_connection_request(7, cx));
    settle(&shell, cx);
    let dialog = shell.read_with(cx, |s, _| s.connection_request().clone());
    assert!(dialog.read_with(cx, |d, _| d.is_open()));
    assert_eq!(
        asked_of_the_server(&relay, "7"),
        vec!["connection.proposed".to_owned()]
    );

    let removed = pc.user.run(&["remote", "remove", "server"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    until(&shell, cx, "the server's request is let go", |cx| {
        !dialog.read_with(cx, |d, _| d.is_open())
    });
}

/// The Settings of the server, in front: its page as the window holds it.
fn the_server_s_settings(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Entity<flottille_window::settings::SettingsPage> {
    click(cx, "settings");
    step(shell, cx);
    settings_of(shell, cx, "server");
    until(shell, cx, "the server's settings are shown", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(shell, cx);
    shell.read_with(cx, |s, cx| {
        s.main().read(cx).settings_page().cloned().unwrap()
    })
}

/// Whether the server was asked `method`, without forgetting what it was asked.
fn the_server_was_asked(relay: &Relay, method: &str) -> bool {
    relay.asked.lock().unwrap().iter().any(|(m, _)| m == method)
}

/// A connection of the server that waits for its review: its server starts (a fake one that
/// announces one action), so the review has something to accept.
fn a_connection_to_review(server: &Machine, name: &str) {
    server
        .user
        .call(
            "extension.set",
            json!({"name": "connections", "enabled": true}),
        )
        .unwrap();
    let script = server.user.path(&format!("{name}-script.json"));
    let announced = json!({
        "server": {"name": name, "version": "1.0.0"},
        "tools": [{"name": "search", "description": "Finds notes by words.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true}}]
    });
    std::fs::write(&script, announced.to_string()).unwrap();
    let fake =
        std::path::Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(fake.exists(), "`cargo build -p flottille-fake-mcp` first");
    server
        .user
        .call(
            "connection.add",
            json!({"name": name, "about": "The notes of the user.",
                "command": [fake.display().to_string(), "--script", script.display().to_string()]}),
        )
        .unwrap();
}

#[gpui_kit::test]
fn accepting_a_review_opened_from_another_machine_s_toast_reaches_that_machine(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server, _here, _there) = two_machines();
    a_connection_to_review(&server, "almanac");
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    let page = the_server_s_settings(&shell, cx);
    let section = page.read_with(cx, |page, _| page.connections().clone());

    // Left without an Accept, the server's review leaves a toast that leads back to it.
    section.update(cx, |section, cx| {
        section.open_review_after_request("almanac", cx)
    });
    until(&shell, cx, "the server's review is ready", |cx| {
        present(cx, "accept-connection")
    });
    click(cx, "connection-review-cancel");
    until(&shell, cx, "the toast", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "The server almanac still waits for your review")
    });
    // This machine in front, the toast's Review opens the server's review over its page.
    shell.update_in(cx, |s, window, cx| s.choose_machine(None, window, cx));
    until(&shell, cx, "this machine is in front", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).is_none()
    });
    settle(&shell, cx);
    let review = (1..16)
        .map(|n| format!("toast-{n}-action"))
        .find(|id| label(cx, id).as_deref() == Some("Review"))
        .expect("the toast's Review");
    click(cx, review);
    until(&shell, cx, "the server's review is ready again", |cx| {
        present(cx, "accept-connection")
    });
    settle(&shell, cx);
    relay.take();
    click(cx, "accept-connection");
    until(&shell, cx, "the accept is sent somewhere", |cx| {
        the_server_was_asked(&relay, "connection.accept") || present(cx, "connection-accept-error")
    });
    assert!(
        the_server_was_asked(&relay, "connection.accept"),
        "the accept went to the machine in front"
    );
    eventually("the server's connection is accepted", || {
        let listed = server.user.call("connection.list", json!({})).unwrap();
        listed["connections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "almanac" && c["state"] != "awaiting review")
    });
}

/// A connection `notes` of `machine` whose header carries the vault entry `entry`, to a fake
/// server that asks for a sign-in: it refuses what it is given. The server, stopped once dropped.
fn a_server_refusing_its_header(machine: &Machine, entry: &str) -> common::Spawned {
    let user = &machine.user;
    let script = user.path("notes-script.json");
    let announced = json!({
        "server": {"name": "notes", "version": "1.0.0"},
        "tools": [{"name": "search", "description": "Finds notes by words.",
            "inputSchema": {"type": "object", "properties": {}}}],
        "oauth": {"tokens_file": user.path("notes-tokens")}
    });
    std::fs::write(&script, announced.to_string()).unwrap();
    let fake =
        std::path::Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    let port_file = user.path("notes-port");
    let spawned = common::Spawned::new({
        let mut command = std::process::Command::new(fake);
        command.args([
            "--script",
            script.to_str().unwrap(),
            "--http",
            port_file.to_str().unwrap(),
        ]);
        command
    });
    eventually("the server listens", || {
        std::fs::read_to_string(&port_file).is_ok_and(|port| !port.is_empty())
    });
    let port = std::fs::read_to_string(&port_file).unwrap();
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "A notes server.",
            "url": format!("http://127.0.0.1:{port}/mcp"),
            "headers": {"Authorization": {"vault": entry}}}),
    )
    .unwrap();
    spawned
}

#[gpui_kit::test]
fn a_vault_entry_edited_from_another_machine_s_review_is_written_there(cx: &mut TestAppContext) {
    let (_hub, pc, server, _here, _there) = two_machines();
    server
        .user
        .call(
            "vault.set",
            json!({"name": "notes-token", "secret": "an-invented-test-value"}),
        )
        .unwrap();
    // The server's connection sends the entry to a server that refuses it: its review names the
    // entry, with Edit entry.
    let _notes = a_server_refusing_its_header(&server, "notes-token");
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    let page = the_server_s_settings(&shell, cx);
    let (vault, section) = page.read_with(cx, |page, _| {
        (page.vault().clone(), page.connections().clone())
    });
    section.update(cx, |section, cx| section.open_review("notes", cx));
    until(&shell, cx, "the review names the entry", |cx| {
        present(cx, "connection-review-edit-entry")
    });
    settle(&shell, cx);
    click(cx, "connection-review-edit-entry");
    until(&shell, cx, "the entry's dialog", |cx| {
        vault.read_with(cx, |vault, _| vault.is_open())
    });
    // This machine brought in front while the dialog is open (the review was opened from the
    // server's toast over this machine's page).
    shell.update_in(cx, |s, window, cx| s.choose_machine(None, window, cx));
    until(&shell, cx, "this machine is in front", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).is_none()
    });
    settle(&shell, cx);
    assert!(vault.read_with(cx, |vault, _| vault.is_open()));
    relay.take();
    let field = vault.read_with(cx, |vault, _| vault.secret_field().unwrap().clone());
    cx.update(|window, cx| {
        field.update(cx, |field, cx| {
            field.set_value("another-invented-value", window, cx)
        })
    });
    cx.update(|window, cx| vault.update(cx, |vault, cx| vault.save(window, cx)));
    until(&shell, cx, "the entry is written somewhere", |cx| {
        the_server_was_asked(&relay, "vault.set") || vault.read_with(cx, |v, _| !v.is_open())
    });
    assert!(
        the_server_was_asked(&relay, "vault.set"),
        "the entry went to the machine in front"
    );
}

#[gpui_kit::test]
fn a_dialog_opened_while_another_machine_is_reached_sends_to_its_own_machine(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, _server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    click(cx, "settings");
    step(&shell, cx);
    click(cx, "settings-tabs-vault");
    settle(&shell, cx);
    let vault = shell.read_with(cx, |s, cx| {
        let page = s.main().read(cx).settings_page().cloned().unwrap();
        page.read(cx).vault().clone()
    });
    until(&shell, cx, "the server's session is listed", |cx| {
        present(cx, row_there(&there))
    });

    // The server's session is asked, its key still read; this machine's Add entry is opened
    // meanwhile, and saved once the server is in front.
    let server_agent = there.clone();
    shell.update_in(cx, |shell, window, cx| {
        shell.open_elsewhere("server".to_owned(), server_agent, window, cx);
        vault.update(cx, |vault, cx| vault.add(window, cx));
    });
    until(&shell, cx, "the server is in front", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    assert!(vault.read_with(cx, |vault, _| vault.is_open()));
    relay.take();
    let (name, secret) = vault.read_with(cx, |vault, _| {
        (
            vault.name_field().unwrap().clone(),
            vault.secret_field().unwrap().clone(),
        )
    });
    cx.update(|window, cx| {
        name.update(cx, |field, cx| field.set_value("pc-token", window, cx));
        secret.update(cx, |field, cx| {
            field.set_value("an-invented-test-value", window, cx)
        });
    });
    cx.update(|window, cx| vault.update(cx, |vault, cx| vault.save(window, cx)));
    until(&shell, cx, "the entry is written somewhere", |cx| {
        the_server_was_asked(&relay, "vault.set") || vault.read_with(cx, |v, _| !v.is_open())
    });
    assert!(
        !the_server_was_asked(&relay, "vault.set"),
        "this machine's entry went to the server"
    );
    eventually("this machine's vault has the entry", || {
        let listed = pc.user.call("vault.list", json!({})).unwrap();
        listed.to_string().contains("pc-token")
    });
}

/// What an agent asks to edit the connection `name` that [`a_connection_to_review`] added, its
/// server announcing one more action from now on: accepted, the connection waits for its review.
fn an_edit_giving_an_action(machine: &Machine, name: &str) -> Value {
    let script = machine.user.path(&format!("{name}-script.json"));
    let mut announced: Value =
        serde_json::from_str(&std::fs::read_to_string(&script).unwrap()).unwrap();
    announced["tools"].as_array_mut().unwrap().push(json!({
        "name": "link", "description": "Links two notes.", "inputSchema": {"type": "object"}}));
    std::fs::write(&script, announced.to_string()).unwrap();
    let fake =
        std::path::Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    json!({"action": "edit", "name": name, "command": format!(
        "{} --script {} --verbose", fake.display(), script.display())})
}

/// An agent of `machine` asking `connections` with `args`, held for the user: its number there.
fn a_request_held_on(machine: &Machine, args: Value) -> u64 {
    let opened = machine.user.received_requests("session/new").len();
    machine
        .user
        .call(
            "agent.run",
            json!({"project": "app", "message": "Add the notes server"}),
        )
        .unwrap();
    let (url, authorization) = common::mcp::server_of(&machine.user, opened);
    let mcp = common::mcp::Mcp::initialize(&url, &authorization);
    let answer = mcp.call("connections", args);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

#[gpui_kit::test]
fn a_connection_request_s_new_entry_and_review_stay_on_its_machine(cx: &mut TestAppContext) {
    let (_hub, pc, _server, _here, _there) = two_machines();
    // This PC's connection waits for its review; its agents ask to add one whose header carries
    // an entry not made yet, and to set the one awaiting review.
    a_connection_to_review(&pc, "almanac");
    let edit = a_request_held_on(&pc, an_edit_giving_an_action(&pc, "almanac"));
    pc.user
        .call("approval.approve", json!({"number": edit}))
        .unwrap();
    let bearer = a_request_held_on(
        &pc,
        json!({"action": "add", "name": "notes", "about": "Notes elsewhere.",
            "url": "https://notes.example.com/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "notes-key"}]}),
    );
    let set = a_request_held_on(
        &pc,
        json!({"action": "set", "name": "almanac", "settings": {"search": "passes"}}),
    );
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    let (vault, connections, dialog) = shell.read_with(cx, |s, cx| {
        (
            s.vault(cx),
            s.connections(cx),
            s.connection_request().clone(),
        )
    });

    // This PC's request, opened over its page, waits for its entry; the server comes in front.
    shell.update(cx, |s, cx| s.open_connection_request(bearer, cx));
    until(&shell, cx, "the request is read", |cx| {
        present(cx, "connection-request-accept")
    });
    settle(&shell, cx);
    click(cx, "connection-request-accept");
    until(&shell, cx, "the button for the entry", |cx| {
        present(cx, "connection-request-new-entry-notes-key")
    });
    shell.update_in(cx, |s, window, cx| {
        s.choose_machine(Some("server".to_owned()), window, cx)
    });
    until(&shell, cx, "the server is in front", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    relay.take();

    // New entry… is this PC's vault, and the dialog it returns to is this PC's request.
    click(cx, "connection-request-new-entry-notes-key");
    let front = shell.read_with(cx, |s, cx| s.vault(cx));
    until(&shell, cx, "a vault's dialog opens", |cx| {
        vault.read_with(cx, |v, _| v.is_open()) || front.read_with(cx, |v, _| v.is_open())
    });
    assert!(
        vault.read_with(cx, |v, _| v.is_open()),
        "the entry's dialog is the server's vault"
    );
    settle(&shell, cx);
    let secret = vault.read_with(cx, |v, _| v.secret_field().unwrap().clone());
    cx.update(|window, cx| {
        secret.update(cx, |f, cx| {
            f.set_value("an-invented-test-value", window, cx)
        })
    });
    click(cx, "save-vault-entry");
    eventually("this PC's vault has the entry", || {
        let listed = pc.user.call("vault.list", json!({})).unwrap();
        listed.to_string().contains("notes-key")
    });
    until(&shell, cx, "the request's dialog is back", |cx| {
        present(cx, "connection-request-accept") && !vault.read_with(cx, |v, _| v.is_open())
    });
    settle(&shell, cx);
    assert!(!the_server_was_asked(&relay, "vault.set"));
    assert_eq!(
        dialog.read_with(cx, |d, _| d.machine().map(str::to_owned)),
        None
    );
    assert!(
        !the_server_was_asked(&relay, "connection.proposed"),
        "the dialog reopened as the server's"
    );
    cx.update(|_, cx| dialog.update(cx, |d, cx| d.close(cx)));
    settle(&shell, cx);

    // Review on this PC's request opens this PC's review, the server in front.
    shell.update_in(cx, |s, window, cx| s.choose_machine(None, window, cx));
    until(&shell, cx, "this machine is in front", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).is_none()
    });
    settle(&shell, cx);
    shell.update(cx, |s, cx| s.open_connection_request(set, cx));
    until(&shell, cx, "the review is offered first", |cx| {
        present(cx, "connection-request-review")
    });
    shell.update_in(cx, |s, window, cx| {
        s.choose_machine(Some("server".to_owned()), window, cx)
    });
    until(&shell, cx, "the server is in front again", |cx| {
        shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    settle(&shell, cx);
    relay.take();
    click(cx, "connection-request-review");
    until(&shell, cx, "a review opens", |cx| {
        connections.read_with(cx, |c, _| c.review_is_open())
            || the_server_was_asked(&relay, "connection.review")
    });
    assert!(
        !the_server_was_asked(&relay, "connection.review"),
        "the review is the server's"
    );
}

#[gpui_kit::test]
fn another_machine_s_pull_request_is_kept_after_a_refresh(cx: &mut TestAppContext) {
    let pc_user = fake_user();
    pc_user.fake_script(json!({"turns": [{"when": "", "do": [{"say": REPLY}]}]}));
    let server_user = fake_user().with_fake_gh();
    server_user.fake_script(json!({"turns": [{"when": "", "do": [{"say": REPLY}]}]}));
    let (_hub, pc, server) = pc_and_server_on(Machine::of(pc_user), server_user);
    project(&pc.user, "app");
    project(&server.user, "harbor");
    session(&pc.user, "app", "Tidy the readme");
    // A workspace of the server's alone, whose branch has a pull request.
    let folder = server.user.path("work/harbor");
    server.user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/harbor.git"],
    );
    server
        .user
        .call(
            "project.set_settings",
            json!({"name": "harbor", "repositories": [
                {"path": ".", "forge": {"kind": "github", "repository": "acme/harbor"}}]}),
        )
        .unwrap();
    server
        .user
        .call(
            "workspace.create",
            json!({"project": "harbor", "name": "login"}),
        )
        .unwrap();
    eventually("the workspace is ready", || {
        server
            .user
            .call(
                "workspace.show",
                json!({"project": "harbor", "name": "login"}),
            )
            .unwrap()["state"]
            == "ready"
    });
    server
        .user
        .gh_script(json!({"pulls": {"acme/harbor": {"harbor/login": {
        "number": 12, "title": "A change", "state": "OPEN", "isDraft": false,
        "headRefOid": "aaaaaaa1", "baseRefName": "main",
        "url": "https://github.com/acme/harbor/pull/12",
        "statusCheckRollup": [], "comments": [], "reviews": []}}}}));
    let started = server
        .user
        .call(
            "agent.start",
            json!({"project": "harbor", "workspace": "login", "message": "Fix the login"}),
        )
        .unwrap();
    let there = started["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the server's session answered", || {
        let shown = server
            .user
            .call("agent.show", json!({"agent": there, "after": 0}))
            .unwrap();
        shown.to_string().contains(REPLY)
    });
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);

    until(&shell, cx, "the server's session is listed", |cx| {
        present(cx, row_there(&there))
    });
    // The server's page of harbor reads its workspaces' pull requests.
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "harbor", window, cx)
    });
    until(&shell, cx, "the server's page of harbor is shown", |cx| {
        shell.read_with(cx, |s, cx| s.main().read(cx).project_page().is_some())
    });
    // Its rows have no workspace to show it on (the machines' lists do not say it): the window
    // keeps it for them all the same.
    let kept = |shell: &Entity<Shell>, cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, _| {
            s.pull_request_of(Some("server"), "harbor", "login")
                .map(|pull| pull.number)
        })
    };
    until(&shell, cx, "the server's pull request is read", |cx| {
        kept(&shell, cx).is_some()
    });
    settle(&shell, cx);

    // This machine's list is read again: the server's workspace is not in it.
    let before = snapshots(&shell, cx);
    session(&pc.user, "app", "Count the files");
    until(&shell, cx, "this machine's list is read again", |cx| {
        snapshots(&shell, cx) > before
    });
    settle(&shell, cx);
    assert_eq!(
        kept(&shell, cx),
        Some(12),
        "the server's pull request was dropped at the refresh"
    );
}

#[gpui_kit::test]
fn a_project_draft_of_another_machine_is_kept_here_and_answered_to_the_composer_that_typed_it(
    cx: &mut TestAppContext,
) {
    // The window keeps every draft on this machine's daemon (`docs/remote.md`), under the machine
    // it is of: the server's page writes it through its own pages, and its answer comes back to
    // the composer that typed it, not to this machine's composer of the same project.
    let (_hub, pc, _server, here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, cx);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "app", window, cx)
    });
    until(&shell, cx, "the server's page of app is drawn", |cx| {
        present(cx, "composer-field")
            && shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    let composer = shell.read_with(cx, |s, _| s.composer_of("app").cloned().unwrap());
    until(&shell, cx, "the server's draft of app is read", |cx| {
        composer.read_with(cx, |c, _| c.draft_was_read())
    });
    settle(&shell, cx);

    // Typed on the server's page, left before the draft is written.
    click(cx, "composer-field");
    cx.simulate_input("Plan the release");
    click(cx, session_id(&here));
    run_for(Duration::from_secs(1), cx);
    until(&shell, cx, "the draft is written", |cx| {
        composer.read_with(cx, |c, _| c.drafts_sent()) > 0
    });
    eventually("this machine's daemon keeps it", || {
        pc.user
            .call(
                "draft.get",
                json!({"kind": "project", "target": "app", "machine": "server"}),
            )
            .is_ok_and(|draft| draft.to_string().contains("Plan the release"))
    });
    until(
        &shell,
        cx,
        "its answer reaches the composer that typed it",
        |cx| composer.read_with(cx, |c, _| c.drafts_unanswered()) == 0,
    );
}

/// What this machine's daemon keeps of the draft of `kind` `target` of the server (`machine`
/// `Some`) or of this machine's own.
fn kept_draft(user: &User, kind: &str, target: &str, machine: Option<&str>) -> Option<String> {
    let mut params = json!({"kind": kind, "target": target});
    if let Some(machine) = machine {
        params["machine"] = json!(machine);
    }
    let answer = user.call("draft.get", params).ok()?;
    answer["draft"]["text"].as_str().map(str::to_owned)
}

#[gpui_kit::test]
fn a_draft_of_another_machine_s_session_is_read_back(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let (_hub, pc, server, _here, there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, first);
    open_there(&shell, cx, &there);
    let reply = reply_there(&shell, cx, &there);
    until(&shell, cx, "the composer heard what its draft is", |cx| {
        reply.read_with(cx, |reply, _| reply.draft_was_read())
    });

    click(cx, "reply-field");
    cx.simulate_input("Half an answer");
    shell.update(cx, |shell, cx| shell.closing(cx));
    eventually("this machine's daemon keeps the server's draft", || {
        kept_draft(&pc.user, "session", &there, Some("server")).as_deref() == Some("Half an answer")
    });
    // A draft never travels to a machine: the server knows nothing of it.
    assert_eq!(kept_draft(&server.user, "session", &there, None), None);

    let (shell, cx) = window(&pc, &relay, second);
    open_there(&shell, cx, &there);
    let reply = reply_there(&shell, cx, &there);
    until(&shell, cx, "the draft is back in its composer", |cx| {
        reply.read_with(cx, |reply, cx| {
            reply.field().read(cx).value() == "Half an answer"
        })
    });
}

#[gpui_kit::test]
fn a_project_draft_of_another_machine_is_not_this_machine_s_draft_of_the_same_name(
    first: &mut TestAppContext,
    second: &mut TestAppContext,
) {
    let (_hub, pc, server, _here, _there) = two_machines();
    let relay = Relay::default();
    let (shell, cx) = window(&pc, &relay, first);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "app", window, cx)
    });
    until(&shell, cx, "the server's page of app is drawn", |cx| {
        present(cx, "composer-field")
            && shell.read_with(cx, |s, _| s.machine_shown()).as_deref() == Some("server")
    });
    let composer = shell.read_with(cx, |s, _| s.composer_of("app").cloned().unwrap());
    until(&shell, cx, "the server's draft of app is read", |cx| {
        composer.read_with(cx, |c, _| c.draft_was_read())
    });
    click(cx, "composer-field");
    cx.simulate_input("Plan the release");
    shell.update(cx, |shell, cx| shell.closing(cx));
    eventually("this machine's daemon keeps the server's draft", || {
        kept_draft(&pc.user, "project", "app", Some("server")).as_deref()
            == Some("Plan the release")
    });
    assert_eq!(
        kept_draft(&pc.user, "project", "app", None),
        None,
        "this machine's project app holds the server's draft"
    );
    assert_eq!(kept_draft(&server.user, "project", "app", None), None);

    // Another window: this machine's page of app is empty, the server's has its draft.
    let (shell, cx) = window(&pc, &relay, second);
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(None, "app", window, cx)
    });
    until(&shell, cx, "this machine's page of app is drawn", |cx| {
        present(cx, "composer-field") && shell.read_with(cx, |s, _| s.machine_shown()).is_none()
    });
    let here_composer = shell.read_with(cx, |s, _| s.composer_of("app").cloned().unwrap());
    until(&shell, cx, "this machine's draft of app is read", |cx| {
        here_composer.read_with(cx, |c, _| c.draft_was_read())
    });
    settle(&shell, cx);
    assert_eq!(
        here_composer.read_with(cx, |c, cx| c.field().read(cx).value().to_string()),
        "",
        "the server's draft shows in this machine's composer of the same name"
    );
    shell.update_in(cx, |shell, window, cx| {
        shell.choose_machine_for(Some("server".to_owned()), "app", window, cx)
    });
    until(&shell, cx, "the server's draft is back", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machine_shown().as_deref() == Some("server")
                && s.composer_of("app")
                    .is_some_and(|c| c.read(cx).field().read(cx).value() == "Plan the release")
        })
    });
}
