//! The scenarios of #814 and #1146: the window connects this machine to the hub, lists and
//! removes the devices, and switches between machines
//! (`docs/remote.md`, "Requests from another machine's clients", "The hub", "Pairing", "Roles").
//! On GPUI's test platform in front of real daemons, each with its own temporary HOME and XDG
//! folders and no keyring, and a hub served in the test on 127.0.0.1: the window signs with the
//! device key file of its machine's key folder, as `flottille remote` does there. No window opens
//! on a screen.

use crate::common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::Hello;
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::machines::{Machine as Seen, how_it_failed};
use flottille_window::machines_page::Removal;
use gpui_design_system::motion::PANEL;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// How long a link, or the hub's news, may take: the daemons try again every second.
const WAIT: Duration = Duration::from_secs(40);

/// The hub, served from a temporary state folder in a runtime of the test's own.
struct TestHub {
    runtime: tokio::runtime::Runtime,
    _folder: tempfile::TempDir,
    served: Option<Served<SystemClock>>,
    address: SocketAddr,
}

impl TestHub {
    fn new() -> TestHub {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let folder = tempfile::tempdir().unwrap();
        let served = runtime.block_on(async {
            let hub = Hub::open(&folder.path().join("hub"), SystemClock).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            flottille_hub::serve(hub, listener, None, Hello::ours("0.4.0")).unwrap()
        });
        let address = served.address();
        TestHub {
            runtime,
            _folder: folder,
            served: Some(served),
            address,
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Pair mode opened, as `flottille-hub pair` opens it on the hub's host: its code.
    fn code(&self) -> String {
        let url = self.url();
        self.served
            .as_ref()
            .unwrap()
            .with_hub(|hub| hub.open_pairing(&url, None).unwrap().1.code().unwrap())
    }

    /// The hub stops: down, as a host that is off.
    fn down(&mut self) {
        let served = self.served.take().unwrap();
        self.runtime.block_on(served.stop());
    }

    /// The hub serves again from its folder, at the address it had: a host that is back.
    fn up(&mut self) {
        self.up_as(Hello::ours("0.4.0"));
    }

    /// [`TestHub::up`], the build of the hub saying `hello`: an old one, say.
    fn up_as(&mut self, hello: Hello) {
        let served = self.runtime.block_on(async {
            let hub = Hub::open(&self._folder.path().join("hub"), SystemClock).unwrap();
            let listener = tokio::net::TcpListener::bind(self.address).await.unwrap();
            flottille_hub::serve(hub, listener, None, hello).unwrap()
        });
        self.served = Some(served);
    }

    /// The hub's members are gone and it is up again, as a hub set up anew on the same host does
    /// (`flottille-hub reset`): its key stays, its list starts over.
    fn reset(&mut self) {
        self.down();
        std::fs::remove_file(self._folder.path().join("hub/hub.json")).unwrap();
        self.up();
    }
}

struct Machine {
    user: User,
}

impl Machine {
    /// `user`'s machine, with remote on: its links go through the hub.
    fn of(user: User) -> Machine {
        std::fs::write(
            user.path("config/flottille/daemon.toml"),
            "[remote]\non = true\n",
        )
        .unwrap();
        std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
        user.status();
        Machine { user }
    }

    fn new() -> Machine {
        Machine::of(User::new())
    }

    fn remote(&self) -> Value {
        self.user.call("remote.machines", json!({})).unwrap()
    }

    fn members(&self) -> Vec<Value> {
        self.user.call("remote.members", json!({})).unwrap()["members"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn member(&self, name: &str) -> Option<Value> {
        self.members().into_iter().find(|m| m["name"] == name)
    }

    fn device(&self) -> Device {
        Device::in_folder(self.user.path("state/flottille/remote"))
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.user.path("state/flottille/daemon.log")).unwrap_or_default()
    }

    fn wait_for(&self, name: &str, state: &str) {
        let deadline = Instant::now() + WAIT;
        loop {
            let seen = self.remote();
            let found = seen["machines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|machine| machine["name"] == name && machine["state"] == state);
            if found {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{name} is not {state}: {seen:#}\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// `pc` paired as the hub's first device, a runner, from the command line.
fn set_up(hub: &TestHub, pc: &Machine) {
    pair(hub, pc, "pc", "runner");
}

/// `machine` paired with `hub` from the command line, as `name` with `role`: pair mode opened on
/// the hub, its code given to `flottille remote pair`.
fn pair(hub: &TestHub, machine: &Machine, name: &str, role: &str) {
    let output = machine.user.run_with_input(
        &["remote", "pair", "--name", name, "--role", role],
        &format!("{}\n", hub.code()),
    );
    assert!(
        output.status.success(),
        "the pairing failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        machine.log()
    );
}

/// A hub, `pc` its first member, and `server` (a runner on `server_user`) paired with it, the
/// two linked.
fn pc_and_server(server_user: User) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let server = Machine::of(server_user);
    set_up(&hub, &pc);
    pair(&hub, &server, "server", "runner");
    pc.wait_for("server", "online");
    server.wait_for("pc", "online");
    (hub, pc, server)
}

fn window<'a>(
    machine: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: machine.user.path("os-release"),
        state_home: machine.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(machine.user.connector());
    let device = Some(machine.device());
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
    (shell, cx)
}

/// One step of the window as the platform would run it: how many animation frames were asked.
fn step(shell: &Entity<Shell>, by: Duration, cx: &mut VisualTestContext) -> usize {
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    asked
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

fn visible(cx: &mut VisualTestContext, id: &str) -> bool {
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
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .map_or(0., |found| f32::from(found.bounds().size.height))
    })
}

fn top(cx: &mut VisualTestContext, id: &str) -> f32 {
    cx.update(|window, _| f32::from(window.find(id.to_owned()).bounds().origin.y))
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

/// Settings, Machines: the tab of its own, after Vault and before General.
fn machines_tab(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    // Tall enough for the whole of the tab: what is clicked must be in view.
    cx.simulate_resize(size(px(1280.), px(3000.)));
    click(shell, cx, "settings");
    until(shell, cx, "the settings page is shown", |cx| {
        present(cx, "settings-tabs-machines")
    });
    click(shell, cx, "settings-tabs-machines");
}

/// Whether the window read the machine `name` from this machine's daemon.
fn knows(shell: &Entity<Shell>, cx: &mut VisualTestContext, name: &str) -> bool {
    shell.read_with(cx, |s, cx| {
        s.machines()
            .read(cx)
            .machines()
            .is_some_and(|read| read.machine(name).is_some())
    })
}

#[gpui_kit::test]
fn the_machines_and_their_state_are_in_settings_not_in_the_sidebar(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the server is read", |cx| {
        knows(&shell, cx, "server")
    });
    settle(&shell, cx);
    // No switcher: the sidebar lists every machine's sessions in one list.
    assert!(!present(cx, "machine-switcher"));

    machines_tab(&shell, cx);
    until(&shell, cx, "the server's state", |cx| {
        label(cx, &member_tag("server")).as_deref() == Some("online")
    });
    // Its state follows the machine's: stopped, it is offline since a time of day.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(&shell, cx, "Settings say the server is offline", |cx| {
        label(cx, &member_tag("server")).is_some_and(|tag| tag.starts_with("offline since "))
    });
}

#[gpui_kit::test]
fn bringing_a_machine_in_front_shows_its_sessions(cx: &mut TestAppContext) {
    let server_user = User::with_fake_agent();
    server_user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    let (_hub, pc, server) = pc_and_server(server_user);
    let folder = server.user.path("work/harbor");
    server.user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "harbor\n").unwrap();
    server.user.git(&folder, &["add", "README.md"]);
    server
        .user
        .git(&folder, &["commit", "--quiet", "-m", "start"]);
    server
        .user
        .call("project.add", json!({"folder": folder, "name": "harbor"}))
        .unwrap();
    let started = server
        .user
        .call(
            "agent.start",
            json!({"project": "harbor", "message": "Write the release notes"}),
        )
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();

    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the server is read", |cx| {
        knows(&shell, cx, "server")
    });
    settle(&shell, cx);
    shell.update(cx, |shell, cx| {
        shell.choose_machine(Some("server".to_owned()), cx)
    });
    until(&shell, cx, "the server's session is in the tree", |cx| {
        present(cx, &format!("session-{id}"))
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.machine_shown()),
        Some("server".to_owned())
    );
    // Served for pc's device, signed.
    assert!(server.log().contains("asked by pc"), "{}", server.log());

    // This machine again: the server's session is its light row of the one list.
    shell.update(cx, |shell, cx| shell.choose_machine(None, cx));
    until(&shell, cx, "this machine is in front again", |cx| {
        !present(cx, &format!("session-{id}"))
    });
    assert_eq!(shell.read_with(cx, |s, _| s.machine_shown()), None);
}

/// Settings, Machines: Connect to your hub opened, the code of `hub`'s pair mode pasted,
/// the name `name` and the role `runner` chosen, and Connect clicked; the code.
fn connect_from_the_page(
    hub: &TestHub,
    name: &str,
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> String {
    machines_tab(shell, cx);
    until(shell, cx, "the one way in is offered", |cx| {
        present(cx, "connect-hub")
    });
    click(shell, cx, "connect-hub");
    until(shell, cx, "the dialog opens", |cx| {
        present(cx, "connect-code")
    });
    let (code, field) = shell.read_with(cx, |s, cx| {
        let section = s.machines().read(cx);
        (
            section.connect_code_field().unwrap().clone(),
            section.connect_name_field().unwrap().clone(),
        )
    });
    let pair_code = hub.code();
    cx.update(|window, cx| {
        code.update(cx, |f, cx| f.set_value(pair_code.clone(), window, cx));
        field.update(cx, |f, cx| f.set_value(name, window, cx));
    });
    click(shell, cx, "connect-role-runner");
    click(shell, cx, "connect-save");
    let row = format!("member-{name}");
    until(shell, cx, "the devices are listed", |cx| present(cx, &row));
    pair_code
}

#[gpui_kit::test]
fn the_first_machine_connects_with_the_hubs_code_in_one_dialog(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the one way in is offered", |cx| {
        present(cx, "connect-hub")
    });
    // One action, no form on the page: no address to type, no setup code.
    assert!(!present(cx, "hub-url") && !present(cx, "connect-code"));
    click(&shell, cx, "connect-hub");
    until(&shell, cx, "the dialog opens", |cx| {
        present(cx, "connect-code")
    });
    settle(&shell, cx);
    // The card this machine proposes: its host's name, its icon, its role.
    let proposed = pc.remote()["proposed"].clone();
    let name = shell.read_with(cx, |s, cx| {
        s.machines()
            .read(cx)
            .connect_name_field()
            .unwrap()
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(json!(name), proposed["name"]);
    let icon = shell.read_with(cx, |s, cx| s.machines().read(cx).connect_icon());
    assert_eq!(
        json!(flottille_window::machines::ICONS[icon].0),
        proposed["icon"]
    );
    for role in ["runner", "controller", "viewer"] {
        assert!(present(cx, &format!("connect-role-{role}")), "{role}");
    }
    let (field, code) = shell.read_with(cx, |s, cx| {
        (
            s.machines().read(cx).connect_code_field().unwrap().clone(),
            hub.code(),
        )
    });
    cx.update(|window, cx| field.update(cx, |f, cx| f.set_value(code.clone(), window, cx)));
    click(&shell, cx, "connect-role-runner");
    click(&shell, cx, "connect-save");
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, &format!("member-{name}"))
    });
    let members = pc.members();
    assert_eq!(members.len(), 1, "{members:#?}");
    assert_eq!(members[0]["role"], "runner");
    // The hub's address is shown as a user writes it: no trailing slash.
    let shown = shell.read_with(cx, |s, cx| {
        s.machines()
            .read(cx)
            .machines()
            .and_then(|m| m.hub.as_ref().map(|h| h.url.clone()))
    });
    assert_eq!(shown, Some(hub.url()));
    assert_eq!(members[0]["added_by"], "the hub");
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "This machine is connected to your hub")
    );
    settle(&shell, cx);
    assert!(!present(cx, "connect-code"), "the dialog closed");
    // The code is used up, and never written where the window reaches.
    assert!(!pc.log().contains(&code));
    // The device key the window kept is the one the list names.
    let kept = flottille_link::DeviceKey::load(&NoKeyring, &pc.user.path("state/flottille/remote"))
        .unwrap()
        .expect("the device key kept");
    assert_eq!(json!(kept.public()), members[0]["key"]);
}

#[gpui_kit::test]
fn another_machine_connects_the_same_way_and_no_device_adds_one(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let (pc, laptop) = (Machine::new(), Machine::new());
    set_up(&hub, &pc);
    let (shell, cx) = window(&laptop, cx);
    connect_from_the_page(&hub, "laptop", &shell, cx);
    until(&shell, cx, "both devices are listed", |cx| {
        present(cx, "member-pc") && present(cx, "member-laptop")
    });
    // A device adds no device: no Add a device, no invitation.
    assert!(!present(cx, "add-device"));
    assert!(!present(cx, "connect-hub"));
}

#[gpui_kit::test]
fn a_refused_code_stays_in_the_dialog(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the one way in is offered", |cx| {
        present(cx, "connect-hub")
    });
    click(&shell, cx, "connect-hub");
    until(&shell, cx, "the dialog opens", |cx| {
        present(cx, "connect-code")
    });
    let code = shell.read_with(cx, |s, cx| {
        s.machines().read(cx).connect_code_field().unwrap().clone()
    });
    // A setup code of before pair mode, or a damaged code: refused, in the dialog, no toast.
    cx.update(|window, cx| code.update(cx, |f, cx| f.set_value("7KQ2-M4XP-9TRW-H3CD", window, cx)));
    click(&shell, cx, "connect-save");
    until(&shell, cx, "the refusal is said", |cx| {
        shell.read_with(cx, |s, cx| s.machines().read(cx).connect_error().is_some())
    });
    settle(&shell, cx);
    assert!(present(cx, "connect-problem"));
    assert!(present(cx, "connect-code"), "the dialog stays open");
    assert!(toasts(&shell, cx).is_empty(), "{:?}", toasts(&shell, cx));
    // The right code connects.
    let right = hub.code();
    cx.update(|window, cx| code.update(cx, |f, cx| f.set_value(right.clone(), window, cx)));
    click(&shell, cx, "connect-save");
    until(&shell, cx, "the machine is connected", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "This machine is connected to your hub")
    });
    assert_eq!(pc.members().len(), 1);
}

#[gpui_kit::test]
fn a_new_member_is_a_toast_with_remove(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let laptop = Machine::new();
    set_up(&hub, &pc);
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the machines are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines().read(cx).machines().is_some_and(|m| m.member())
        })
    });
    pair(&hub, &laptop, "laptop", "controller");
    until(&shell, cx, "the new member is said", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t.starts_with("New device: laptop (control only), added at "))
    });
    let said = toasts(&shell, cx)
        .into_iter()
        .find(|t| t.starts_with("New device: laptop"))
        .unwrap();
    assert!(said.ends_with(" by the hub"), "{said}");

    // Its Remove asks once, over the page shown, then removes it.
    let number = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .position(|t| t.title().starts_with("New device: laptop"))
            .unwrap()
    });
    let action = (0..8)
        .map(|n| format!("toast-{n}-action"))
        .find(|id| present(cx, id))
        .unwrap_or_else(|| panic!("no toast action ({number})"));
    click(&shell, cx, &action);
    assert_eq!(
        shell.read_with(cx, |s, cx| s.machines().read(cx).removal().cloned()),
        Some(Removal::Member("laptop".to_owned()))
    );
    assert!(present(cx, "member-removal-action"));
    click(&shell, cx, "member-removal-action");
    let deadline = Instant::now() + WAIT;
    while pc.member("laptop").is_some() {
        assert!(Instant::now() < deadline, "laptop was never removed");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[gpui_kit::test]
fn remove_asks_once_and_removes_the_row(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let laptop = Machine::new();
    set_up(&hub, &pc);
    pair(&hub, &laptop, "laptop", "controller");
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "laptop's row is listed", |cx| {
        present(cx, "member-laptop")
    });
    settle(&shell, cx);
    click(&shell, cx, "member-more-laptop");
    click(&shell, cx, "member-remove-laptop");
    // One confirmation, a dialog: nothing is removed until it is confirmed.
    assert!(present(cx, "member-removal-action"));
    assert!(pc.member("laptop").is_some());
    click(&shell, cx, "member-removal-action");
    until(&shell, cx, "laptop's row goes", |cx| {
        !present(cx, "member-laptop")
    });
    assert!(pc.member("laptop").is_none());
    assert!(present(cx, "member-pc"));
    assert!(toasts(&shell, cx).iter().any(|t| t == "Removed laptop"));
}

#[gpui_kit::test]
fn a_machine_that_needs_an_update_says_so_not_offline(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let server = Machine::of(User::new().with_env(
        "FLOTTILLE_TEST_LINK_HELLO",
        r#"{"flottille": "0.9.0", "protocols": [99]}"#,
    ));
    set_up(&hub, &pc);
    pair(&hub, &server, "server", "runner");
    pc.wait_for("server", "needs an update");

    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the server is read", |cx| {
        knows(&shell, cx, "server")
    });
    settle(&shell, cx);
    let said = shell.read_with(cx, |s, cx| {
        s.machines()
            .read(cx)
            .machines()
            .and_then(|read| read.machine("server").map(|m| m.state_words()))
            .unwrap_or_default()
    });
    assert!(said.contains("needs an update"), "{said}");
    assert!(!said.contains("offline"), "{said}");

    // Brought in front, it is not: a failure toast says why.
    shell.update(cx, |shell, cx| {
        shell.choose_machine(Some("server".to_owned()), cx)
    });
    settle(&shell, cx);
    assert_eq!(shell.read_with(cx, |s, _| s.machine_shown()), None);
    let failed = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .find(|t| t.is_failure())
            .map(|t| (t.title().to_owned(), t.second_line().map(str::to_owned)))
    });
    let (title, why) = failed.expect("a failure toast");
    assert_eq!(title, "server cannot be shown");
    assert!(
        why.as_deref()
            .is_some_and(|why| why.contains("update one of them")),
        "{why:?}"
    );
}

#[gpui_kit::test]
fn every_change_of_the_machines_page_is_animated(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let laptop = Machine::new();
    set_up(&hub, &pc);
    pair(&hub, &laptop, "laptop", "controller");
    let (shell, cx) = window(&pc, cx);
    // Tall enough for the whole of the tab and the rows under it.
    cx.simulate_resize(size(px(1280.), px(3000.)));
    // Read whole before the page is measured: what the daemon answers later moves the sections.
    until(&shell, cx, "the machines are read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines().read(cx).machines().is_some_and(|m| {
                m.member() && m.members.iter().any(|member| member.name == "laptop")
            })
        })
    });
    settle(&shell, cx);
    click(&shell, cx, "settings");
    until(&shell, cx, "the settings page is shown", |cx| {
        present(cx, "settings-tabs-machines")
    });

    // The sections show up one after the other, each rising into place: the second measured
    // from the first's bottom, which moves as the first's rows unfold.
    cx.update(|window, cx| window.click("settings-tabs-machines", cx));
    let mut offsets = Vec::new();
    for _ in 0..60 {
        step(&shell, Duration::from_millis(16), cx);
        if present(cx, "enter-machines-hub") && present(cx, "enter-machines-devices") {
            // From the column's own top: what is above the machines moves as the page loads.
            let column = top(cx, "machines-blocks");
            let first = top(cx, "enter-machines-hub") - column;
            let below = first + height(cx, "enter-machines-hub");
            offsets.push((first, top(cx, "enter-machines-devices") - column - below));
        }
    }
    let (first, gap) = *offsets.last().unwrap();
    assert!(offsets[0].0 > first, "the first section rises: {offsets:?}");
    // Staggered: the second starts later, so the space between them first opens, then closes as
    // the second arrives.
    let widest = offsets.iter().map(|at| at.1).fold(f32::MIN, f32::max);
    assert!(
        widest > offsets[0].1 && widest > gap,
        "the second section rises after the first: {offsets:?}"
    );
    settle(&shell, cx);

    // A row removed folds away: a height between its own and none.
    let row = height(cx, "member-item-laptop");
    assert!(row > 0.);
    cx.update(|window, cx| window.hover("member-laptop", cx));
    settle(&shell, cx);
    click(&shell, cx, "member-more-laptop");
    click(&shell, cx, "member-remove-laptop");
    // Not through `click`, which settles: the daemon's answer must not fold the row unseen.
    cx.update(|window, cx| window.click("member-removal-action", cx));
    let deadline = Instant::now() + WAIT;
    let mut folding = None;
    while folding.is_none() {
        step(&shell, PANEL / 8, cx);
        let now = height(cx, "member-item-laptop");
        if now > 0. && now < row {
            folding = Some(now);
        }
        assert!(Instant::now() < deadline, "the row never folded");
        std::thread::sleep(Duration::from_millis(10));
    }
    settle(&shell, cx);
    assert!(!present(cx, "member-laptop"));

    // Nothing moves once it all ended.
    let quiet = (0..200).find(|_| step(&shell, Duration::from_millis(16), cx) == 0);
    assert!(quiet.is_some(), "the page never came to rest");
}

/// The hub's card and the link line of a machine's row.
const HUB_TITLE: &str = "hub-state";
const HUB_TAG: &str = "hub-state-tag-0";
const HUB_REASON: &str = "hub-reason";
const HUB_DOWN: &str = "devices-hub-down-line";
/// What waits for the hub while it does not answer: the state is the tag's to say.
const WAITS: &str = "Changes wait for the hub, which cannot be reached now:";

fn link_line(name: &str) -> String {
    format!("member-link-{name}")
}

fn member_tag(name: &str) -> String {
    format!("member-{name}-tag-0")
}

#[gpui_kit::test]
fn settings_show_the_hub_and_whether_it_is_reachable(cx: &mut TestAppContext) {
    let mut hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    // Its session made a moment after the pairing: said reachable once it is.
    until(&shell, cx, "the hub is shown reachable", |cx| {
        label(cx, HUB_TAG).as_deref() == Some("reachable")
    });
    settle(&shell, cx);

    // Its address, that it is reachable, and what it is for.
    let title = label(cx, HUB_TITLE).unwrap();
    assert!(title.contains(&hub.address.to_string()), "{title}");
    assert_eq!(label(cx, HUB_TAG).as_deref(), Some("reachable"));
    assert!(!present(cx, HUB_REASON), "nothing to say while it answers");
    assert!(
        present(cx, "hub-sentence"),
        "its section says what it is for"
    );

    // Down, it is unreachable since a time, and says which address was tried and how it failed:
    // the state and the time are the tag's, the reason does not say them again.
    hub.down();
    until(&shell, cx, "the hub is unreachable", |cx| {
        label(cx, HUB_TAG).is_some_and(|tag| tag.starts_with("unreachable since "))
    });
    // The session's end is said first, then the first try that fails names the address.
    until(&shell, cx, "the reason names the address tried", |cx| {
        label(cx, HUB_REASON).is_some_and(|said| said.starts_with("tried "))
    });
    settle(&shell, cx);
    let reason = label(cx, HUB_REASON).unwrap();
    assert!(
        reason.starts_with(&format!("tried {}", hub.address)),
        "{reason}"
    );
    assert!(
        !reason.contains("unreachable") && !reason.contains("since"),
        "{reason}"
    );
}

#[gpui_kit::test]
fn each_machine_says_it_is_reached_through_the_hub_or_why_not(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    let line = link_line("server");
    until(&shell, cx, "the server's link is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("the hub"))
    });
    settle(&shell, cx);
    // Linked, through the hub, whichever side dialled.
    let said = label(cx, &line).unwrap();
    assert_eq!(said, "Linked through the hub");

    // Off, it says why: its tag says it is off.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    until(&shell, cx, "why is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("not connected to the hub"))
    });
    settle(&shell, cx);
    let tag = label(cx, &member_tag("server")).unwrap();
    assert!(tag.starts_with("offline since "), "{tag}");
}

#[test]
fn a_live_link_says_it_goes_through_the_hub_and_no_address() {
    let machine = |linked: bool| Seen {
        name: "server".to_owned(),
        fingerprint: String::new(),
        state: "online".to_owned(),
        since: None,
        reason: None,
        role: None,
        linked,
        projects: None,
    };
    assert_eq!(machine(true).link_words(), "Linked through the hub");
    assert_eq!(machine(false).link_words(), "Not reached yet");
}

#[test]
fn a_failure_is_said_without_the_state_and_the_time_the_tag_says() {
    assert_eq!(
        how_it_failed("server is unreachable: tried 192.0.2.10:7421 from pc, no answer in 10 s"),
        "tried 192.0.2.10:7421 from pc, no answer in 10 s"
    );
    assert_eq!(
        how_it_failed(
            "the hub at hub.example:7420 is unreachable since 22:14: tried 192.0.2.5:7420: refused"
        ),
        "tried 192.0.2.5:7420: refused"
    );
    // Any other reason says all it has.
    assert_eq!(
        how_it_failed("the connection was closed"),
        "the connection was closed"
    );
}

#[gpui_kit::test]
fn changing_the_list_while_the_hub_is_unreachable_says_so(cx: &mut TestAppContext) {
    let (mut hub, pc, _server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-pc")
    });
    settle(&shell, cx);
    assert!(!present(cx, HUB_DOWN), "the hub answers: nothing to say");

    hub.down();
    until(&shell, cx, "the hub's absence is said", |cx| {
        present(cx, HUB_DOWN)
    });
    settle(&shell, cx);
    // What waits is said once: the tag and the hub's reason say the state.
    assert!(
        label(cx, HUB_DOWN).is_some_and(|said| said.starts_with(WAITS)),
        "{:?}",
        label(cx, HUB_DOWN)
    );
}

#[gpui_kit::test]
fn the_notice_leaves_when_the_hub_is_back(cx: &mut TestAppContext) {
    let (mut hub, pc, _server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-pc")
    });
    hub.down();
    until(&shell, cx, "the hub's absence is said", |cx| {
        present(cx, HUB_DOWN) && present(cx, HUB_REASON)
    });

    hub.up();
    // The daemon tries again within its longest wait; the window hears it as an event.
    until(&shell, cx, "the notice and the reason leave", |cx| {
        !present(cx, HUB_DOWN) && !present(cx, HUB_REASON)
    });
    settle(&shell, cx);
    assert_eq!(label(cx, HUB_TAG).as_deref(), Some("reachable"));
}

#[gpui_kit::test]
fn the_link_change_of_a_row_fades(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    let line = link_line("server");
    until(&shell, cx, "the server's link is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("the hub"))
    });

    // The link ends: the row's tag says it is off, and the line says why.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    until(&shell, cx, "the row is off", |cx| {
        label(cx, &member_tag("server")).is_some_and(|tag| tag.starts_with("offline since "))
    });
    settle(&shell, cx);
    let before = label(cx, &line).unwrap();
    assert!(!before.contains("not connected to the hub"), "{before}");
    let rest = (0..200).find(|_| step(&shell, Duration::from_millis(16), cx) == 0);
    assert!(rest.is_some(), "the page came to rest before the change");

    // The next try fails: only the line changes, the tag stays as it was, so what moves is the
    // line, over HOVER from the last frame's text.
    let tag = label(cx, &member_tag("server")).unwrap();
    until(&shell, cx, "the address line changes", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("not connected to the hub"))
    });
    assert_eq!(label(cx, &member_tag("server")), Some(tag), "the tag stays");
    assert!(present(cx, &line), "the line fades, it is not swapped");
    assert!(
        cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
        "fading"
    );
    cx.executor().advance_clock(Duration::from_millis(500));
    let rest = (0..200).find(|_| step(&shell, Duration::from_millis(16), cx) == 0);
    assert!(rest.is_some(), "the page came to rest after the change");
}

#[gpui_kit::test]
fn a_machine_that_goes_down_before_its_first_read_is_given_up(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the server is read", |cx| {
        knows(&shell, cx, "server")
    });
    settle(&shell, cx);

    // This machine's daemon knows the server is gone; the window, which has not polled, still
    // takes it for online.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    shell.update(cx, |shell, cx| {
        shell.choose_machine(Some("server".to_owned()), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |s, _| s.machine_shown()),
        Some("server".to_owned())
    );

    // No first read comes: it waits for the limit the window gives a daemon that does not answer.
    for _ in 0..40 {
        step(&shell, Duration::from_millis(16), cx);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(visible(cx, "sidebar-loading"), "the tree waits");
    step(&shell, flottille_window::link::NOT_ANSWERING / 2, cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.machine_shown()),
        Some("server".to_owned()),
        "still within the limit"
    );

    // Past it, the window is on this machine again, and says why.
    step(&shell, flottille_window::link::NOT_ANSWERING, cx);
    assert_eq!(shell.read_with(cx, |s, _| s.machine_shown()), None);
    until(&shell, cx, "this machine's tree is read", |cx| {
        !visible(cx, "sidebar-loading")
    });
    let failed = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .find(|t| t.is_failure())
            .map(|t| (t.title().to_owned(), t.second_line().map(str::to_owned)))
    });
    let (title, why) = failed.expect("a failure toast");
    assert_eq!(title, "server cannot be shown");
    assert!(
        why.as_deref()
            .is_some_and(|why| why.starts_with("server is offline since ")),
        "{why:?}"
    );
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

/// Waits until the link has sent more reads of the shell than `before`, without the window
/// taking them: they wait in its queue.
fn queued(shell: &Entity<Shell>, before: u64, cx: &mut VisualTestContext) {
    let deadline = Instant::now() + WAIT;
    while snapshots(shell, cx) <= before {
        assert!(Instant::now() < deadline, "no read was sent");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn snapshots(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> u64 {
    shell.read_with(cx, |s, _| {
        s.seen().snapshots.load(std::sync::atomic::Ordering::SeqCst)
    })
}

#[gpui_kit::test]
fn a_read_of_this_machine_queued_before_the_switch_does_not_end_the_wait(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the server is read", |cx| {
        knows(&shell, cx, "server")
    });
    settle(&shell, cx);

    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    // A read of this machine, sent and not yet taken, when the server is chosen.
    let before = snapshots(&shell, cx);
    project(&pc.user, "anchor");
    queued(&shell, before, cx);
    shell.update(cx, |shell, cx| {
        shell.choose_machine(Some("server".to_owned()), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |s, _| s.machine_shown()),
        Some("server".to_owned())
    );

    // That read is this machine's, not the server's: the wait goes on, and ends in a toast.
    step(&shell, Duration::from_millis(16), cx);
    step(&shell, flottille_window::link::NOT_ANSWERING / 2, cx);
    step(&shell, flottille_window::link::NOT_ANSWERING, cx);
    assert_eq!(shell.read_with(cx, |s, _| s.machine_shown()), None);
    assert!(
        toasts(&shell, cx).contains(&"server cannot be shown".to_owned()),
        "{:?}",
        toasts(&shell, cx)
    );
}

#[gpui_kit::test]
fn a_read_of_the_machine_given_up_is_not_shown(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    project(&server.user, "harbor");
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the server is read", |cx| {
        knows(&shell, cx, "server")
    });
    settle(&shell, cx);

    // The server's first read is sent, and the limit passes before the window takes it.
    let before = snapshots(&shell, cx);
    shell.update(cx, |shell, cx| {
        shell.choose_machine(Some("server".to_owned()), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |s, _| s.machine_shown()),
        Some("server".to_owned())
    );
    queued(&shell, before, cx);
    cx.executor()
        .advance_clock(flottille_window::link::NOT_ANSWERING + Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(shell.read_with(cx, |s, _| s.machine_shown()), None);

    // Back on this machine, the server's late read is not drawn as this machine's.
    shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
    assert!(
        shell.read_with(cx, |s, _| s
            .snapshot()
            .projects
            .iter()
            .all(|project| project.name != "harbor")),
        "the server's project is in this machine's tree"
    );
}

#[test]
fn switching_cuts_the_connections_made_for_the_machine_before() {
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;

    let daemons: Arc<Mutex<Vec<UnixStream>>> = Arc::default();
    let local: Connect = {
        let daemons = daemons.clone();
        Arc::new(move || {
            let (ours, theirs) = UnixStream::pair()?;
            daemons.lock().unwrap().push(theirs);
            Ok(ours)
        })
    };
    let aim = flottille_window::machines::Aim::default();
    let connect = aim.connect(local);
    let key = Arc::new(flottille_link::DeviceKey::make().unwrap());

    let mut watch = connect().unwrap();
    watch
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    aim.set(Some(("server".to_owned(), key.clone())));
    // The window's end of the old watch ends, and so does the daemon's.
    assert_eq!(watch.read(&mut [0; 1]).unwrap(), 0, "the old watch is cut");
    let mut daemon = daemons.lock().unwrap().remove(0);
    daemon
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    assert_eq!(daemon.read(&mut [0; 1]).unwrap(), 0, "so is its end");

    // What is opened for the machine now shown lives until the next choice.
    let mut fresh = connect().unwrap();
    fresh
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let error = fresh.read(&mut [0; 1]).unwrap_err();
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
        "{error}"
    );
    aim.set(None);
    fresh
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    assert_eq!(fresh.read(&mut [0; 1]).unwrap(), 0, "cut by the next one");
}

/// A key folder of this test's own: only its owner may enter it.
fn key_folder() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let folder = tempfile::tempdir().unwrap();
    std::fs::set_permissions(folder.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    folder
}

#[test]
fn the_device_key_is_made_once_and_kept_at_once() {
    let folder = key_folder();
    let device = Device::in_folder(folder.path().to_owned());
    let key = device.key_to_pair().unwrap();
    assert_eq!(device.key().unwrap().public(), key.public());
    assert_eq!(device.key_to_pair().unwrap().public(), key.public());
}

#[test]
fn a_pending_key_of_before_pair_mode_is_kept_when_pairing() {
    let folder = key_folder();
    let stale = flottille_link::DeviceKey::make().unwrap();
    let public = stale.public();
    stale.keep_pending(&NoKeyring, folder.path()).unwrap();
    let device = Device::in_folder(folder.path().to_owned());
    assert_eq!(device.key_to_pair().unwrap().public(), public);
    assert_eq!(device.key().unwrap().public(), public);
    assert!(
        flottille_link::DeviceKey::load_pending(&NoKeyring, folder.path())
            .unwrap()
            .is_none()
    );
}

/// No keyring: the device key is the file of the key folder.
struct NoKeyring;

impl flottille_link::Keyring for NoKeyring {
    fn read(
        &self,
        _attributes: &[(&str, &str)],
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, flottille_link::KeyringError> {
        Err(flottille_link::KeyringError::Absent)
    }

    fn store(
        &self,
        _label: &str,
        _attributes: &[(&str, &str)],
        _secret: &[u8],
    ) -> Result<(), flottille_link::KeyringError> {
        Err(flottille_link::KeyringError::Absent)
    }
}

// ── Remote in its own tab, turned on and configured from the window (#1144) ────────────────────

/// A machine with remote off.
fn off_machine() -> Machine {
    let user = User::new();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user.status();
    Machine { user }
}

#[gpui_kit::test]
fn machines_is_a_tab_of_settings(cx: &mut TestAppContext) {
    let keys: Vec<&str> = flottille_window::settings::SECTIONS
        .iter()
        .map(|(key, _)| *key)
        .collect();
    let vault = keys.iter().position(|k| *k == "vault").unwrap();
    assert_eq!(keys[vault + 1], "machines", "after Vault: {keys:?}");
    assert_eq!(keys[vault + 2], "general", "before General: {keys:?}");

    let hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    // Its sections, and none of them in General.
    until(&shell, cx, "remote", |cx| present(cx, "remote"));
    for section in ["hub", "devices"] {
        let title = format!("{section}-title");
        until(&shell, cx, section, |cx| present(cx, &title));
    }
    click(&shell, cx, "settings-tabs-general");
    settle(&shell, cx);
    for section in ["devices-title", "remote"] {
        assert!(!present(cx, section), "{section} is not in General");
    }

    // An old link to General's machines section opens the tab.
    let machines = flottille_window::settings::SECTIONS
        .iter()
        .position(|(key, _)| *key == "machines")
        .unwrap();
    for link in ["general#machines", "settings:general#machines"] {
        let at = flottille_window::settings::Reveal::from_link(link)
            .unwrap_or_else(|| panic!("{link} is no link"));
        shell.update_in(cx, |shell, window, cx| {
            shell.open_settings_at(at, window, cx)
        });
        settle(&shell, cx);
        let page = shell
            .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
            .unwrap();
        assert_eq!(page.read_with(cx, |page, _| page.tab()), machines, "{link}");
        click(&shell, cx, "settings-tabs-general");
    }
    assert!(flottille_window::settings::Reveal::from_link("general#appearance").is_none());
}

#[gpui_kit::test]
fn turning_remote_on_in_settings_starts_the_link(cx: &mut TestAppContext) {
    let pc = off_machine();
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the notice is shown", |cx| {
        present(cx, "remote-notice")
    });
    assert!(present(cx, "turn-on-remote"));
    assert!(
        !present(cx, "remote-switch"),
        "the switch comes once it is on"
    );
    let said = label(cx, "remote-notice").unwrap();
    assert!(said.contains("join a hub"), "{said}");
    assert!(
        !said.contains("reached"),
        "no address is chosen here: {said}"
    );
    assert!(!said.contains("daemon.toml"), "{said}");

    click(&shell, cx, "turn-on-remote");
    until(&shell, cx, "the daemon says it is on", |cx| {
        pc.user.call("remote.settings", json!({})).unwrap()["on"] == true
            && present(cx, "remote-switch")
    });
    settle(&shell, cx);
    assert!(pc.user.call("remote.machines", json!({})).is_ok());
    assert!(!present(cx, "remote-notice"), "the notice is gone");
    assert!(!present(cx, "turn-on-remote"));
    // A machine with no hub is offered the one way in.
    assert!(present(cx, "connect-hub"));
}

#[gpui_kit::test]
fn turning_remote_off_closes_the_links_and_keeps_the_hub(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the switch is shown", |cx| {
        present(cx, "remote-switch")
    });
    settle(&shell, cx);
    // One confirmation, a dialog: nothing closes until it is confirmed.
    click(&shell, cx, "remote-switch");
    assert!(present(cx, "member-removal-action"));
    assert_eq!(
        pc.user.call("remote.settings", json!({})).unwrap()["on"],
        true
    );
    click(&shell, cx, "member-removal-action");
    until(&shell, cx, "the notice is back", |cx| {
        present(cx, "remote-notice")
    });
    assert_eq!(
        pc.user.call("remote.settings", json!({})).unwrap()["on"],
        false
    );
    server.wait_for("pc", "offline");
    // Turned on again, this machine is paired with the same hub. The confirmation has faded out (its scrim takes
    // no click while it does) and the notice has unfolded before it is pressed.
    until(&shell, cx, "the confirmation is gone", |cx| {
        !present(cx, "member-removal-action")
    });
    settle(&shell, cx);
    // The notice unfolds over wall-clock time: a press that lands while it is still folded hits
    // nothing, so it is pressed again until the daemon is on.
    let mut pressed = Instant::now() - Duration::from_secs(1);
    let deadline = Instant::now() + WAIT;
    while !(present(cx, "member-server") && present(cx, "member-pc")) {
        step(&shell, Duration::from_millis(16), cx);
        if present(cx, "turn-on-remote") && pressed.elapsed() > Duration::from_millis(500) {
            click(&shell, cx, "turn-on-remote");
            pressed = Instant::now();
        }
        assert!(
            Instant::now() < deadline,
            "its devices are not listed again: toasts {:?}, notice {}, settings {:?}, switch {}, no hub {}, rest {}, read {:?}\n{}",
            toasts(&shell, cx),
            present(cx, "turn-on-remote"),
            pc.user.call("remote.settings", json!({})),
            present(cx, "remote-switch"),
            present(cx, "hub-connect-title"),
            present(cx, "devices-title"),
            shell.read_with(cx, |s, cx| s.machines().read(cx).machines().map(|m| (
                m.off.clone(),
                m.this.clone(),
                m.hub.is_some(),
                m.members.len()
            ))),
            pc.log()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

// ── #1177: the hub's truth, a device's look, where this machine listens ───────────────────────

fn hub_state(machine: &Machine) -> Value {
    machine.remote()["hub"].clone()
}

fn wait_for_hub(machine: &Machine, state: &str) {
    let deadline = Instant::now() + WAIT;
    while hub_state(machine)["state"] != state {
        assert!(
            Instant::now() < deadline,
            "the hub is not {state}: {:#}\n{}",
            hub_state(machine),
            machine.log()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[gpui_kit::test]
fn a_protocol_mismatch_says_the_hub_is_old_and_how_to_update_it(cx: &mut TestAppContext) {
    let mut hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    // The hub is updated on its host to a build of before pair mode, or never was.
    hub.down();
    hub.up_as(Hello {
        flottille: "0.1.0".to_owned(),
        protocols: vec![1],
    });
    wait_for_hub(&pc, "needs an update");
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the hub's reason is said", |cx| {
        present(cx, "hub-reason")
    });
    settle(&shell, cx);
    let said = label(cx, "hub-reason").unwrap();
    // This machine says its own build, which the hub session always does: its version is not fixed.
    assert!(
        said.starts_with("The hub runs 0.1.0 (protocol 1), this machine ")
            && said.ends_with(" (protocol 2): update the hub on its host."),
        "{said}"
    );
    // The hub is the one to update, on its host: never `flottille update`, never "this machine"
    // for the hub.
    assert!(!said.contains("flottille update"), "{said}");
}

#[gpui_kit::test]
fn a_machine_the_hub_no_longer_knows_says_so_and_offers_pair_again(cx: &mut TestAppContext) {
    let mut hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-pc")
    });
    settle(&shell, cx);
    assert!(!present(cx, "devices-forgotten"), "the hub knows it");
    assert!(!shell.read_with(cx, |s, cx| s.machines().read(cx).list_is_old()));

    // A new hub on the same host: it has no member.
    hub.reset();
    until(&shell, cx, "the page says the hub forgot it", |cx| {
        present(cx, "devices-forgotten")
    });
    settle(&shell, cx);
    assert_eq!(
        label(cx, "devices-forgotten").as_deref(),
        Some("The hub no longer knows this machine")
    );
    // The list kept is shown, greyed as old, and Pair again is the way out.
    assert!(present(cx, "member-pc"));
    assert!(shell.read_with(cx, |s, cx| s.machines().read(cx).list_is_old()));
    assert!(present(cx, "pair-again"));
    click(&shell, cx, "pair-again");
    until(&shell, cx, "the pairing dialog opens", |cx| {
        present(cx, "connect-code")
    });
    assert!(shell.read_with(cx, |s, cx| s.machines().read(cx).connect_is_open()));
    // Not Connect to your hub: this machine was a member.
    assert!(!present(cx, "connect-hub"));
}

#[gpui_kit::test]
fn a_device_the_list_dropped_says_the_hub_no_longer_knows_it(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    assert!(
        pc.user
            .run(&["remote", "remove", "server"])
            .status
            .success()
    );
    wait_for_hub(&server, "refused");
    let (shell, cx) = window(&server, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the page says the hub forgot it", |cx| {
        present(cx, "devices-forgotten")
    });
    assert!(present(cx, "pair-again"));
}

#[gpui_kit::test]
fn pairing_again_replaces_the_old_membership(cx: &mut TestAppContext) {
    let mut hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let old_key = pc.members()[0]["key"].clone();
    hub.reset();
    wait_for_hub(&pc, "reset");
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "Pair again is offered", |cx| {
        present(cx, "pair-again")
    });
    click(&shell, cx, "pair-again");
    until(&shell, cx, "the dialog opens", |cx| {
        present(cx, "connect-code")
    });
    assert!(shell.read_with(cx, |s, cx| s.machines().read(cx).connect_replaces()));
    let (code, name) = shell.read_with(cx, |s, cx| {
        let section = s.machines().read(cx);
        (
            section.connect_code_field().unwrap().clone(),
            section.connect_name_field().unwrap().clone(),
        )
    });
    let pair_code = hub.code();
    cx.update(|window, cx| {
        code.update(cx, |f, cx| f.set_value(pair_code.clone(), window, cx));
        name.update(cx, |f, cx| f.set_value("desk", window, cx));
    });
    click(&shell, cx, "connect-save");
    until(&shell, cx, "the new membership is listed", |cx| {
        present(cx, "member-desk") && !present(cx, "devices-forgotten")
    });
    // The old membership is replaced, not added to: one member of the new list.
    let members = pc.members();
    assert_eq!(members.len(), 1, "{members:#?}");
    assert_eq!(members[0]["name"], "desk");
    assert_ne!(members[0]["key"], Value::Null);
    let _ = old_key;
    assert!(!present(cx, "member-pc"));
    assert!(!shell.read_with(cx, |s, cx| s.machines().read(cx).list_is_old()));
}

#[gpui_kit::test]
fn changes_are_not_offered_while_the_hub_refuses(cx: &mut TestAppContext) {
    let (mut hub, pc, _server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-server")
    });
    settle(&shell, cx);
    // The hub answers: every item is offered, none says it waits.
    click(&shell, cx, "member-more-server");
    for item in ["rename", "look", "remove"] {
        assert!(
            label(cx, &format!("member-{item}-server-detail")).is_none(),
            "{item} is offered"
        );
    }
    cx.simulate_keystrokes("escape");
    settle(&shell, cx);

    hub.down();
    until(&shell, cx, "the section says changes wait", |cx| {
        present(cx, "devices-hub-down-line")
    });
    settle(&shell, cx);
    let said = label(cx, "devices-hub-down-line").unwrap();
    assert!(
        said.starts_with("Changes wait for the hub, which cannot be reached now: "),
        "{said}"
    );
    // The ⋯ items are there, disabled, each with the reason in one line.
    click(&shell, cx, "member-more-server");
    for item in ["rename", "look", "remove"] {
        let reason = label(cx, &format!("member-{item}-server-detail"));
        assert!(
            reason.is_some_and(|reason| reason.contains("hub")),
            "{item} says why it waits"
        );
    }
    // And a click on one asks nothing: no dialog.
    click(&shell, cx, "member-rename-server");
    assert!(!shell.read_with(cx, |s, cx| s.machines().read(cx).rename_is_open()));
}

#[gpui_kit::test]
fn one_click_is_one_request_and_one_toast(cx: &mut TestAppContext) {
    let (mut hub, pc, _server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-server")
    });
    hub.down();
    until(&shell, cx, "the hub's absence is said", |cx| {
        present(cx, "devices-hub-down-line")
    });
    // The confirmation is asked from elsewhere (the toast of a new device): the request goes out
    // and the button is busy until it is answered.
    shell.update(cx, |shell, cx| {
        shell.machines().update(cx, |section, cx| {
            section.ask_removal(Removal::Member("server".to_owned()), cx)
        })
    });
    settle(&shell, cx);
    click(&shell, cx, "member-removal-action");
    assert!(shell.read_with(cx, |s, cx| s.machines().read(cx).is_changing()));
    // The hub never answers: the daemon says so after its bound, once.
    let deadline = Instant::now() + WAIT;
    while shell.read_with(cx, |s, cx| s.machines().read(cx).is_changing()) {
        step(&shell, Duration::from_millis(16), cx);
        assert!(Instant::now() < deadline, "the request never ended");
        std::thread::sleep(Duration::from_millis(25));
    }
    settle(&shell, cx);
    let failures: Vec<_> = toasts(&shell, cx)
        .into_iter()
        .filter(|title| title.contains("was not removed"))
        .collect();
    assert_eq!(failures, ["server was not removed"], "one toast, not two");
    // Said in words, with what to do: never the raw cause.
    let why = shell.read_with(cx, |s, cx| {
        s.toasts()
            .read(cx)
            .toasts()
            .find(|t| t.is_failure())
            .and_then(|t| t.second_line().map(str::to_owned))
    });
    let why = why.expect("the toast says why");
    assert!(why.contains("try again"), "{why}");
    assert!(
        !why.contains("withheld") && !why.contains("sent back no list"),
        "{why}"
    );
}

#[gpui_kit::test]
fn removing_this_machine_says_it_leaves_the_hub(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-server")
    });
    settle(&shell, cx);
    // Another device: the links to it close, as ever.
    click(&shell, cx, "member-more-server");
    click(&shell, cx, "member-remove-server");
    until(&shell, cx, "the confirmation is asked", |cx| {
        present(cx, "member-removal-sentence")
    });
    let other = label(cx, "member-removal-sentence").unwrap();
    assert!(!other.contains("this machine leaves"), "{other}");
    click(&shell, cx, "member-removal-cancel");
    settle(&shell, cx);
    // This machine: it leaves the hub, and its links close.
    click(&shell, cx, "member-more-pc");
    click(&shell, cx, "member-remove-pc");
    until(&shell, cx, "the confirmation is asked", |cx| {
        label(cx, "member-removal-sentence").is_some_and(|said| said != other)
    });
    let this = label(cx, "member-removal-sentence").unwrap();
    assert!(
        this.contains("This machine leaves the hub and its links close"),
        "{this}"
    );
}

/// How a member's icon in its colour is said, from `remote.members`.
fn look_words(member: &Value) -> String {
    format!(
        "{} in {}",
        member["icon"].as_str().unwrap(),
        member["colour"].as_str().unwrap()
    )
}

#[gpui_kit::test]
fn a_device_row_shows_its_icon_in_its_colour(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-server")
    });
    settle(&shell, cx);
    for (name, machine) in [("pc", &pc), ("server", &server)] {
        let member = machine.member(name).unwrap();
        assert_eq!(
            label(cx, &format!("member-icon-{name}")),
            Some(look_words(&member)),
            "{name}"
        );
    }
}

#[gpui_kit::test]
fn a_controller_is_in_the_machines_tab_with_its_role_and_the_hub_command(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let (pc, laptop, phone) = (Machine::new(), Machine::new(), Machine::new());
    set_up(&hub, &pc);
    pair(&hub, &laptop, "laptop", "controller");
    pair(&hub, &phone, "phone", "viewer");
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-laptop") && present(cx, "member-phone")
    });
    settle(&shell, cx);
    // Its role is said, and that it runs nothing, with the command of the hub's host, copyable.
    assert_eq!(
        label(cx, "member-role-laptop-command").as_deref(),
        Some("flottille-hub role laptop runner")
    );
    assert_eq!(
        label(cx, "member-role-phone-command").as_deref(),
        Some("flottille-hub role phone runner")
    );
    assert!(label(cx, "member-role-laptop-said").is_some_and(|said| said.contains("runs nothing")));
    // A runner has nothing to explain: it is in the Settings picker.
    assert!(!present(cx, "member-role-pc-command"));
    // The window offers no button that changes a role: only the copy.
    click(&shell, cx, "member-role-laptop-command-copy");
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("flottille-hub role laptop runner".to_owned())
    );
    // Neither is offered where a machine is chosen: they run nothing.
    let offered: Vec<_> = shell.read_with(cx, |s, cx| {
        let read = s.machines().read(cx).machines().expect("read");
        flottille_window::machines::choices(read)
            .into_iter()
            .filter_map(|choice| choice.name)
            .collect()
    });
    assert!(
        !offered
            .iter()
            .any(|name| name == "laptop" || name == "phone"),
        "{offered:?}"
    );
}

#[test]
fn a_member_with_no_look_gets_the_defaults_of_its_key() {
    // A member added before pair mode has no icon nor colour (`remote.members`: null).
    let folder = key_folder();
    let key = flottille_link::DeviceKey::load_or_make(&NoKeyring, folder.path()).unwrap();
    let member = flottille_window::machines::Member {
        name: "old".to_owned(),
        key: json!(key.public()),
        fingerprint: String::new(),
        role: "runner".to_owned(),
        added_by: String::new(),
        machine: None,
        icon: None,
        colour: None,
    };
    let (icon, colour) = member.look();
    assert_eq!(icon, "desktop");
    assert_eq!(colour, flottille_link::Colour::of(&key.public()).word());
}

#[gpui_kit::test]
fn change_look_sets_the_icon_and_colour(cx: &mut TestAppContext) {
    let (_hub, pc, _server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-server")
    });
    settle(&shell, cx);
    click(&shell, cx, "member-more-server");
    click(&shell, cx, "member-look-server");
    until(&shell, cx, "the dialog opens", |cx| {
        present(cx, "look-colour-teal")
    });
    // The five icons as a select, the eight colours as swatches.
    for colour in flottille_link::Colour::ALL {
        assert!(present(cx, &format!("look-colour-{}", colour.word())));
    }
    shell.update(cx, |shell, cx| {
        shell
            .machines()
            .update(cx, |section, cx| section.choose_look_icon(3, cx))
    });
    // Another colour than the one the device has (its key proposed one of the eight).
    let colour = match pc.member("server").unwrap()["colour"].as_str() {
        Some("orange") => "blue",
        _ => "orange",
    };
    click(&shell, cx, &format!("look-colour-{colour}"));
    // A live preview of the row: the icon and colour chosen, before anything is saved.
    settle(&shell, cx);
    assert_eq!(
        label(cx, "look-preview"),
        Some(format!("phone in {colour}"))
    );
    assert_ne!(pc.member("server").unwrap()["colour"], colour);
    click(&shell, cx, "member-look-save");
    until(&shell, cx, "the row shows the new look", |cx| {
        label(cx, "member-icon-server") == Some(format!("phone in {colour}"))
    });
    // A signed change, as `flottille remote icon` and `colour` make it.
    let server = pc.member("server").unwrap();
    assert_eq!(
        (server["icon"].as_str(), server["colour"].as_str()),
        (Some("phone"), Some(colour))
    );
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "Changed the look of server")
    );
}

#[gpui_kit::test]
fn the_machine_of_a_session_in_the_sidebar_shows_its_look(cx: &mut TestAppContext) {
    let server_user = User::with_fake_agent();
    server_user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    let (_hub, pc, server) = pc_and_server(server_user);
    let folder = server.user.path("work/harbor");
    server.user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "harbor\n").unwrap();
    server.user.git(&folder, &["add", "README.md"]);
    server
        .user
        .git(&folder, &["commit", "--quiet", "-m", "start"]);
    server
        .user
        .call("project.add", json!({"folder": folder, "name": "harbor"}))
        .unwrap();
    let started = server
        .user
        .call(
            "agent.start",
            json!({"project": "harbor", "message": "Write the release notes"}),
        )
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (shell, cx) = window(&pc, cx);
    let row = flottille_window::model::elsewhere_id("server", &id);
    until(&shell, cx, "the server's session is listed", |cx| {
        present(cx, &format!("session-machine-icon-{row}"))
    });
    assert_eq!(
        label(cx, &format!("session-machine-icon-{row}")),
        Some(look_words(&server.member("server").unwrap()))
    );
}

#[gpui_kit::test]
fn the_machines_page_shows_no_addresses(cx: &mut TestAppContext) {
    // Every link goes through the hub: the page has no Addresses section and no place that asks
    // where this machine listens. Its devices and its hub are there.
    let (_hub, pc, _server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "member-pc") && present(cx, "hub-state")
    });
    settle(&shell, cx);
    for id in [
        "addresses",
        "add-address",
        "address-list",
        "no-address-block",
        "listen",
        "listen-nowhere",
        "listen-save",
    ] {
        assert!(!present(cx, id), "{id} is on the page");
    }
}

/// Whether the window's read of the machines says the phone is notified `always` or `away`.
fn notify_read(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    shell.read_with(cx, |s, cx| {
        s.machines()
            .read(cx)
            .machines()
            .map(|read| read.notify.clone())
    })
}

#[gpui_kit::test]
fn notify_the_phone_is_chosen_in_settings_machines(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the phone's section", |cx| {
        present(cx, "phone-title")
    });
    settle(&shell, cx);
    for choice in ["notify-phone-away", "notify-phone-always"] {
        assert!(visible(cx, choice), "{choice}");
    }
    assert_eq!(notify_read(&shell, cx).as_deref(), Some("away"));

    click(&shell, cx, "notify-phone-always");
    until(&shell, cx, "the daemon says always", |cx| {
        pc.user.call("remote.settings", json!({})).unwrap()["notify"] == "always"
            && notify_read(&shell, cx).as_deref() == Some("always")
    });
    let file = std::fs::read_to_string(pc.user.path("config/flottille/daemon.toml")).unwrap();
    assert!(file.contains("notify = \"always\""), "{file}");
    assert!(file.contains("on = true"), "remote stays on: {file}");

    click(&shell, cx, "notify-phone-away");
    until(&shell, cx, "the daemon says away", |cx| {
        pc.user.call("remote.settings", json!({})).unwrap()["notify"] == "away"
            && notify_read(&shell, cx).as_deref() == Some("away")
    });
    // Each result is a toast.
    settle(&shell, cx);
    for toast in [
        "Your phone is told always",
        "Your phone is told when you are away",
    ] {
        assert!(toasts(&shell, cx).iter().any(|t| t == toast), "{toast}");
    }
}

#[gpui_kit::test]
fn the_window_tells_the_hub_its_presence_on_input(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let member: flottille_link::PublicKey =
        serde_json::from_value(pc.member("pc").unwrap()["key"].clone()).unwrap();
    let deadline = Instant::now() + WAIT;
    while pc.remote()["hub"]["state"] != "connected" {
        assert!(Instant::now() < deadline, "the hub never connected");
        std::thread::sleep(Duration::from_millis(200));
    }
    let (shell, cx) = window(&pc, cx);
    settle(&shell, cx);
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 1;

    // A click, as any input.
    click(&shell, cx, "settings");

    let served = hub.served.as_ref().unwrap();
    until(&shell, cx, "the hub has the window's input", |_| {
        served
            .with_hub(|hub| hub.presence(&member))
            .is_some_and(|presence| presence.last_input >= before)
    });
}
