//! The scenarios of #814: the window adds devices and switches between machines
//! (`docs/remote.md`, "Requests from another machine's clients", "The hub", "Pairing", "Roles").
//! On GPUI's test platform in front of real daemons, each with its own temporary HOME and XDG
//! folders and no keyring, and a hub served in the test on 127.0.0.1: the window signs with the
//! device key file of its machine's key folder, as `flottille remote` does there. No window opens
//! on a screen.

use crate::common;

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::Hello;
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::Connect;
use flottille_window::machines::{Machine as Seen, Reached, how_it_failed};
use flottille_window::machines_page::Removal;
use flottille_window::tree::{SWITCHER, SWITCHER_THIS};
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

    fn code(&self) -> String {
        self.served
            .as_ref()
            .unwrap()
            .with_hub(|hub| hub.setup_code().unwrap().to_owned())
    }

    /// The hub stops: down, as a host that is off.
    fn down(&mut self) {
        let served = self.served.take().unwrap();
        self.runtime.block_on(served.stop());
    }

    /// The hub serves again from its folder, at the address it had: a host that is back.
    fn up(&mut self) {
        let served = self.runtime.block_on(async {
            let hub = Hub::open(&self._folder.path().join("hub"), SystemClock).unwrap();
            let listener = tokio::net::TcpListener::bind(self.address).await.unwrap();
            flottille_hub::serve(hub, listener, None, Hello::ours("0.4.0")).unwrap()
        });
        self.served = Some(served);
    }
}

struct Machine {
    user: User,
}

impl Machine {
    /// `user`'s machine, with remote on, its link on loopback.
    fn of(user: User) -> Machine {
        std::fs::write(
            user.path("config/flottille/daemon.toml"),
            "[remote]\non = true\nlisten = \"127.0.0.1:0\"\n",
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

    fn key(&self) -> String {
        self.remote()["this"]["key"].as_str().unwrap().to_owned()
    }

    fn found(&self) -> Vec<String> {
        self.remote()["this"]["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|address| address["how"] == "found")
            .map(|address| address["address"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Trusts `other` as `name` with no hub: what the link of #811 does by hand.
    fn trust(&self, name: &str, other: &Machine) {
        self.trust_at(name, other, other.found());
    }

    /// [`Machine::trust`], knowing `other` at `addresses` only.
    fn trust_at(&self, name: &str, other: &Machine, addresses: Vec<String>) {
        let file = self.user.path("state/flottille/remote/peers.json");
        let mut peers: Value = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| json!({"machines": []}));
        peers["machines"].as_array_mut().unwrap().push(json!({
            "name": name, "key": other.key(), "addresses": addresses,
        }));
        std::fs::write(file, peers.to_string()).unwrap();
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

    /// The join requests that wait for this machine's member.
    fn requests(&self) -> Vec<Value> {
        self.user.call("remote.requests", json!({})).unwrap()["requests"]
            .as_array()
            .cloned()
            .or_else(|| {
                self.user
                    .call("remote.requests", json!({}))
                    .unwrap()
                    .as_array()
                    .cloned()
            })
            .unwrap_or_default()
    }
}

/// `pc` set up as the hub's first member, from the command line.
fn set_up(hub: &TestHub, pc: &Machine) {
    let output = pc.user.run_with_input(
        &["remote", "setup", &hub.url(), "--name", "pc"],
        &format!("{}\n", hub.code()),
    );
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        pc.log()
    );
}

/// `joiner` asks to join with an invitation of `inviter`'s: the command, waiting until the
/// request is accepted or refused.
fn ask_to_join(inviter: &Machine, joiner: &Machine) -> Child {
    let invited = inviter.user.run(&["remote", "invite", "--json"]);
    assert!(invited.status.success(), "{}", inviter.log());
    let invited: Value = serde_json::from_slice(&invited.stdout).unwrap();
    let code = invited["code"].as_str().unwrap().to_owned();
    let mut joining = joiner
        .user
        .command(&["remote", "join"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    joining
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{code}\n").as_bytes())
        .unwrap();
    joining
}

/// The join of `joiner` accepted from the command line, as `name` with `role`.
fn pair(inviter: &Machine, joiner: &Machine, name: &str, role: &str) {
    let mut joining = ask_to_join(inviter, joiner);
    let mut out = BufReader::new(joining.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    let deadline = Instant::now() + WAIT;
    let request = loop {
        if let Some(first) = inviter.requests().first() {
            break first.clone();
        }
        assert!(Instant::now() < deadline, "no join request came");
        std::thread::sleep(Duration::from_millis(200));
    };
    let id = request["request"].as_str().unwrap();
    let accepted = inviter
        .user
        .run(&["remote", "accept", id, "--name", name, "--role", role]);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let ended = joining.wait_with_output().unwrap();
    assert!(
        ended.status.success(),
        "the join failed: {}\n{}",
        String::from_utf8_lossy(&ended.stderr),
        joiner.log()
    );
}

/// A hub, `pc` its first member, and `server` (a runner on `server_user`) paired with it, the
/// two linked.
fn pc_and_server(server_user: User) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let server = Machine::of(server_user);
    set_up(&hub, &pc);
    pair(&pc, &server, "server", "runner");
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

/// The line under an option of the switcher: the machine's state.
fn state_of(cx: &mut VisualTestContext, name: &str) -> String {
    label(cx, &format!("{SWITCHER}-{name}-line")).unwrap_or_default()
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

fn option(name: &str) -> String {
    format!("{SWITCHER}-{name}")
}

#[gpui_kit::test]
fn the_switcher_lists_this_machine_first_then_the_runners_with_their_state(
    cx: &mut TestAppContext,
) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the switcher shows", |cx| visible(cx, SWITCHER));
    settle(&shell, cx);
    click(&shell, cx, SWITCHER);
    assert!(present(cx, &option(SWITCHER_THIS)));
    assert!(present(cx, &option("server")));
    assert!(
        top(cx, &option(SWITCHER_THIS)) < top(cx, &option("server")),
        "this machine comes first"
    );
    assert_eq!(state_of(cx, "server"), "online");

    // Its state follows the machine's: stopped, it is offline since a time of day.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");
    until(
        &shell,
        cx,
        "the switcher says the server is offline",
        |cx| state_of(cx, "server").starts_with("offline since "),
    );
}

#[gpui_kit::test]
fn choosing_a_machine_shows_its_sessions(cx: &mut TestAppContext) {
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
    until(&shell, cx, "the switcher shows", |cx| visible(cx, SWITCHER));
    settle(&shell, cx);
    assert!(shell.read_with(cx, |s, _| s.snapshot().projects.is_empty()));
    click(&shell, cx, SWITCHER);
    click(&shell, cx, &option("server"));
    until(&shell, cx, "the server's session is in the tree", |cx| {
        present(cx, &format!("session-{id}"))
    });
    assert_eq!(
        shell.read_with(cx, |s, _| s.machine_shown()),
        Some("server".to_owned())
    );
    // Served for pc's device, signed.
    assert!(server.log().contains("asked by pc"), "{}", server.log());

    // This machine again: its own (none) are shown.
    click(&shell, cx, SWITCHER);
    click(&shell, cx, &option(SWITCHER_THIS));
    until(&shell, cx, "this machine's tree is back", |cx| {
        !present(cx, &format!("session-{id}"))
    });
    assert_eq!(shell.read_with(cx, |s, _| s.machine_shown()), None);
}

/// Settings, Machines: Set up a hub opens its dialog, the hub's address and its setup code are
/// typed there, and Set up is clicked.
fn set_up_from_the_page(
    hub: &TestHub,
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> String {
    machines_tab(shell, cx);
    until(shell, cx, "the setup is offered", |cx| {
        present(cx, "open-hub-setup")
    });
    click(shell, cx, "open-hub-setup");
    assert!(present(cx, "set-up-hub"), "the dialog holds its button");
    let (url, code) = shell.read_with(cx, |s, cx| {
        let section = s.machines().read(cx);
        (
            section.url_field().unwrap().clone(),
            section.code_field().unwrap().clone(),
        )
    });
    let (address, setup_code) = (hub.url(), hub.code());
    cx.update(|window, cx| {
        url.update(cx, |f, cx| f.set_value(address.clone(), window, cx));
        code.update(cx, |f, cx| f.set_value(setup_code.clone(), window, cx));
    });
    click(shell, cx, "set-up-hub");
    until(shell, cx, "the devices are listed", |cx| {
        present(cx, "add-device")
    });
    setup_code
}

#[gpui_kit::test]
fn set_up_a_hub_joins_with_the_setup_code(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let (shell, cx) = window(&pc, cx);
    let setup_code = set_up_from_the_page(&hub, &shell, cx);
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
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "This machine set up the hub")
    );
    // The code is used up, and never written where the window reaches.
    assert!(!pc.log().contains(&setup_code));
    // The device key the window kept is the one the list names.
    let kept = flottille_link::DeviceKey::load(&NoKeyring, &pc.user.path("state/flottille/remote"))
        .unwrap()
        .expect("the device key kept");
    assert_eq!(json!(kept.public()), members[0]["key"]);
}

#[gpui_kit::test]
fn add_a_device_shows_a_qr_and_a_code_until_it_expires(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "Add a device is offered", |cx| {
        present(cx, "add-device")
    });
    click(&shell, cx, "add-device");
    until(&shell, cx, "the invitation is shown", |cx| {
        present(cx, "invitation-qr-code")
    });
    settle(&shell, cx);
    let code = label(cx, "invitation-code").unwrap();
    assert!(!code.is_empty());
    assert_eq!(
        label(cx, "invitation-qr-code").as_deref(),
        Some("Invitation code")
    );
    assert!(
        label(cx, "invitation-expiry")
            .unwrap()
            .starts_with("Valid once, until ")
    );
    // The same text as the command line's invitation: a code that joins.
    assert!(flottille_link::Invitation::from_code(&code).is_ok());

    // Five minutes later it folds away.
    step(&shell, Duration::from_secs(5 * 60), cx);
    settle(&shell, cx);
    assert!(shell.read_with(cx, |s, cx| s.machines().read(cx).invitation().is_none()));
    assert!(!present(cx, "invitation-qr-code"));
}

#[gpui_kit::test]
fn a_join_request_shows_the_words_and_the_role_choice(cx: &mut TestAppContext) {
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
    let mut joining = ask_to_join(&pc, &laptop);
    let mut out = BufReader::new(joining.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();

    until(&shell, cx, "the join dialog opens", |cx| {
        present(cx, "join-words")
    });
    settle(&shell, cx);
    let words = pc.requests()[0]["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_owned())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(label(cx, "join-words").as_deref(), Some(words.as_str()));
    assert!(
        first.contains(&words),
        "the new device shows the same: {first}"
    );
    for role in ["runner", "controller", "viewer"] {
        assert!(present(cx, &format!("join-role-{role}")), "{role}");
    }
    assert_eq!(label(cx, "join-role-runner").as_deref(), Some("Run agents"));
    assert_eq!(
        label(cx, "join-role-controller").as_deref(),
        Some("Control only")
    );
    assert_eq!(label(cx, "join-role-viewer").as_deref(), Some("View only"));
    assert!(present(cx, "join-accept") && present(cx, "join-refuse"));

    let name = shell.read_with(cx, |s, cx| {
        s.machines().read(cx).join_name_field().unwrap().clone()
    });
    cx.update(|window, cx| name.update(cx, |f, cx| f.set_value("laptop", window, cx)));
    click(&shell, cx, "join-role-viewer");
    click(&shell, cx, "join-accept");
    let ended = joining.wait_with_output().unwrap();
    assert!(
        ended.status.success(),
        "{}",
        String::from_utf8_lossy(&ended.stderr)
    );
    let member = pc.member("laptop").expect("laptop is a member");
    assert_eq!(member["role"], "viewer");
    assert_eq!(member["added_by"], "pc");
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
    pair(&pc, &laptop, "laptop", "controller");
    until(&shell, cx, "the new member is said", |cx| {
        toasts(&shell, cx)
            .iter()
            .any(|t| t.starts_with("New device: laptop (control only), added at "))
    });
    let said = toasts(&shell, cx)
        .into_iter()
        .find(|t| t.starts_with("New device: laptop"))
        .unwrap();
    assert!(said.ends_with(" from pc"), "{said}");

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
    pair(&pc, &laptop, "laptop", "controller");
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
    let pc = Machine::new();
    let server = Machine::of(User::new().with_env(
        "FLOTTILLE_TEST_LINK_HELLO",
        r#"{"flottille": "0.9.0", "protocols": [99]}"#,
    ));
    pc.trust("server", &server);
    server.trust("pc", &pc);
    pc.wait_for("server", "needs an update");

    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the switcher shows", |cx| visible(cx, SWITCHER));
    settle(&shell, cx);
    click(&shell, cx, SWITCHER);
    let said = state_of(cx, "server");
    assert!(said.contains("needs an update"), "{said}");
    assert!(!said.contains("offline"), "{said}");

    // Chosen, it is not shown: a failure toast says why.
    click(&shell, cx, &option("server"));
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
    pair(&pc, &laptop, "laptop", "controller");
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

    // The invitation is a dialog: it rises and fades in over the page, never unfolds in it.
    cx.update(|window, cx| window.click("add-device", cx));
    // The dialog rises and fades in through the kit's `Modal`, whose motion its own scenarios hold.
    until(&shell, cx, "the invitation comes", |cx| {
        shell.read_with(cx, |s, cx| s.machines().read(cx).invitation().is_some())
    });
    settle(&shell, cx);
    assert_eq!(stack(&shell, cx), vec!["invitation"]);
    assert!(
        !present(cx, "invitation-card"),
        "nothing unfolds in the page"
    );
    click(&shell, cx, "invitation-cancel");

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

/// The hub's card and the address line of a machine's row.
const HUB_TITLE: &str = "hub-state";
const HUB_TAG: &str = "hub-state-tag-0";
const HUB_REASON: &str = "hub-reason";
const HUB_DOWN: &str = "devices-hub-down-line";
/// What waits for the hub while it does not answer: the state is the tag's to say.
const WAITS: &str = "Adding a device and changing the list wait for the hub.";

fn address_line(name: &str) -> String {
    format!("member-address-{name}")
}

fn member_tag(name: &str) -> String {
    format!("member-{name}-tag-0")
}

/// How opaque a control is drawn: the widest alpha of what is painted inside it (its border, its
/// fill), which a faded control lowers.
fn drawn_alpha(cx: &mut VisualTestContext, id: &str) -> f32 {
    cx.update(|window, _| {
        let Some(found) = window.try_find(id.to_owned()) else {
            return 0.0;
        };
        let bounds = found.bounds().scale(window.scale_factor());
        window
            .painted_quads()
            .iter()
            .filter(|quad| bounds.contains(&quad.bounds.center()))
            .flat_map(|quad| {
                [
                    quad.background.as_solid().map_or(0.0, |bg| bg.a),
                    quad.border_color.a,
                ]
            })
            .fold(0.0, f32::max)
    })
}

#[gpui_kit::test]
fn settings_show_the_hub_and_whether_it_is_reachable(cx: &mut TestAppContext) {
    let mut hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the hub is shown", |cx| present(cx, HUB_TITLE));
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
fn each_machine_says_the_address_it_is_reached_at_or_the_one_that_failed(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let at = server.found().remove(0);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    let line = address_line("server");
    until(&shell, cx, "the server's address is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("127.0.0.1"))
    });
    settle(&shell, cx);
    // Linked, it says where: the address pc dialled, or the host the link came from (its port is
    // the one it dialled from, which tells nothing).
    let said = label(cx, &line).unwrap();
    assert!(
        said == format!("Reached at {at}") || said == "Linked from 127.0.0.1",
        "{said}"
    );

    // Off, it says the address that was tried and how it failed: its tag says it is off.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    until(&shell, cx, "the failed address is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains(&format!("tried {at} from pc")))
    });
    settle(&shell, cx);
    let said = label(cx, &line).unwrap();
    assert!(
        said.starts_with("tried ") && !said.contains("is unreachable"),
        "{said}"
    );
    let tag = label(cx, &member_tag("server")).unwrap();
    assert!(tag.starts_with("offline since "), "{tag}");
}

#[test]
fn a_link_the_other_machine_dialled_says_its_host_not_its_port() {
    let linked = |address: &str, dialled: bool| Seen {
        name: "server".to_owned(),
        fingerprint: String::new(),
        state: "online".to_owned(),
        since: None,
        reason: None,
        role: None,
        reached: Some(Reached {
            address: address.to_owned(),
            dialled,
        }),
        addresses: Vec::new(),
    };
    assert_eq!(
        linked("127.0.0.1:54321", false).address_words(true),
        "Linked from 127.0.0.1"
    );
    assert_eq!(
        linked("[::1]:54321", false).address_words(true),
        "Linked from ::1"
    );
    // The address this machine dialled is the one to dial again: with its port.
    assert_eq!(
        linked("127.0.0.1:7421", true).address_words(true),
        "Reached at 127.0.0.1:7421"
    );
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
fn adding_a_device_while_the_hub_is_unreachable_says_so(cx: &mut TestAppContext) {
    let (mut hub, pc, _server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "Add a device is offered", |cx| {
        present(cx, "add-device")
    });
    settle(&shell, cx);
    assert!(!present(cx, HUB_DOWN), "the hub answers: nothing to say");
    let enabled = drawn_alpha(cx, "add-device");
    assert!(enabled > 0.0, "the button is drawn: {enabled}");

    hub.down();
    until(&shell, cx, "the hub's absence is said", |cx| {
        present(cx, HUB_DOWN)
    });
    settle(&shell, cx);
    // What waits is said once: the tag and the hub's reason say the state.
    assert_eq!(label(cx, HUB_DOWN).as_deref(), Some(WAITS));

    // Add a device cannot work: it is faded, and it asks nothing of the hub, whatever asks it.
    let disabled = drawn_alpha(cx, "add-device");
    assert!(
        disabled < enabled * 0.6,
        "Add a device is faded: {disabled} against {enabled}"
    );
    click(&shell, cx, "add-device");
    shell.update(cx, |shell, cx| {
        shell
            .machines()
            .update(cx, |section, cx| section.invite(cx))
    });
    assert!(
        !shell.read_with(cx, |s, cx| s.machines().read(cx).is_inviting()),
        "nothing is asked of a hub that does not answer"
    );
    assert!(shell.read_with(cx, |s, cx| s.machines().read(cx).invitation().is_none()));
}

#[gpui_kit::test]
fn a_machine_only_the_hub_could_tell_shows_its_last_known_address(cx: &mut TestAppContext) {
    let (mut hub, pc, server) = pc_and_server(User::new());
    let at = server.found().remove(0);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    let line = address_line("server");
    until(&shell, cx, "the server's address is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("127.0.0.1"))
    });
    hub.down();
    until(&shell, cx, "the hub's absence is said", |cx| {
        present(cx, HUB_DOWN)
    });

    // The link ends: its reason does not name an address (the next try, 10 s later, will).
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    until(&shell, cx, "the last known address is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("Last known address"))
    });
    let said = label(cx, &line).unwrap();
    assert!(
        said.ends_with(&format!("Last known address {at}.")),
        "{said}"
    );

    // Once the try names the address, it is said once, by the failure.
    until(&shell, cx, "the failed try is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("tried "))
    });
    let said = label(cx, &line).unwrap();
    assert!(!said.contains("Last known address"), "{said}");
    assert_eq!(said.matches(&at).count(), 1, "{said}");
}

#[gpui_kit::test]
fn the_notice_leaves_when_the_hub_is_back(cx: &mut TestAppContext) {
    let (mut hub, pc, _server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "Add a device is offered", |cx| {
        present(cx, "add-device")
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

    // Add a device works again.
    click(&shell, cx, "add-device");
    until(&shell, cx, "the invitation comes", |cx| {
        shell.read_with(cx, |s, cx| s.machines().read(cx).invitation().is_some())
    });
}

#[gpui_kit::test]
fn the_address_change_of_a_row_fades(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let at = server.found().remove(0);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    let line = address_line("server");
    until(&shell, cx, "the server's address is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("127.0.0.1"))
    });

    // The link ends: the row's tag says it is off, and the line says why.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    until(&shell, cx, "the row is off", |cx| {
        label(cx, &member_tag("server")).is_some_and(|tag| tag.starts_with("offline since "))
    });
    settle(&shell, cx);
    let before = label(cx, &line).unwrap();
    assert!(!before.contains("tried "), "{before}");
    let rest = (0..200).find(|_| step(&shell, Duration::from_millis(16), cx) == 0);
    assert!(rest.is_some(), "the page came to rest before the change");

    // The next try, 10 s later, fails: only the line changes, the tag stays as it was, so what
    // moves is the line, over HOVER from the last frame's text.
    let tag = label(cx, &member_tag("server")).unwrap();
    until(&shell, cx, "the address line changes", |cx| {
        label(cx, &line).is_some_and(|said| said.contains(&format!("tried {at} from pc")))
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
    until(&shell, cx, "the switcher shows", |cx| visible(cx, SWITCHER));
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
    until(&shell, cx, "the switcher shows", |cx| visible(cx, SWITCHER));
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
    until(&shell, cx, "the switcher shows", |cx| visible(cx, SWITCHER));
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

/// What the hub says of `members` for a settle: a hub with the keys listed as its members.
fn hub_says(members: Vec<Value>) -> impl FnMut(&str, Value) -> Result<Value, String> {
    move |method, _| match method {
        "remote.join" => Ok(Value::Null),
        "remote.machines" => Ok(json!({"hub": {"url": "http://hub.example"}})),
        "remote.members" => Ok(json!({"members": members})),
        other => Err(format!("unexpected {other}")),
    }
}

#[test]
fn a_pending_key_the_hub_lists_is_kept() {
    let folder = key_folder();
    let device = Device::in_folder(folder.path().to_owned());
    let (key, new) = device.key_to_join().unwrap();
    assert!(new);
    let public = key.public();
    assert!(device.key().is_err(), "a pending key signs nothing yet");

    device
        .settle_pending(&mut hub_says(vec![json!({"key": public, "name": "pc"})]))
        .unwrap();
    assert_eq!(device.key().unwrap().public(), public);
    assert!(
        flottille_link::DeviceKey::load_pending(&NoKeyring, folder.path())
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_pending_key_no_member_list_holds_is_removed() {
    let folder = key_folder();
    let device = Device::in_folder(folder.path().to_owned());
    let (key, _) = device.key_to_join().unwrap();
    let other = flottille_link::DeviceKey::make().unwrap();

    device
        .settle_pending(&mut hub_says(vec![
            json!({"key": other.public(), "name": "laptop"}),
        ]))
        .unwrap();
    assert!(device.key().is_err(), "{:?}", key.public());
    assert!(
        flottille_link::DeviceKey::load_pending(&NoKeyring, folder.path())
            .unwrap()
            .is_none(),
        "a key the hub saw in a setup that did not succeed is never sent again"
    );
}

#[gpui_kit::test]
fn a_setup_after_one_that_did_not_end_uses_a_new_key(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    // The window was closed in the middle of a setup: its key was kept as pending, the hub never
    // saw it in a list.
    let stale = flottille_link::DeviceKey::make().unwrap();
    let stale_public = stale.public();
    stale
        .keep_pending(&NoKeyring, &pc.user.path("state/flottille/remote"))
        .unwrap();
    let (shell, cx) = window(&pc, cx);
    set_up_from_the_page(&hub, &shell, cx);

    let members = pc.members();
    assert_eq!(members.len(), 1, "{members:#?}");
    assert_ne!(members[0]["key"], json!(stale_public), "the stale key");
    let folder = pc.user.path("state/flottille/remote");
    let kept = flottille_link::DeviceKey::load(&NoKeyring, &folder)
        .unwrap()
        .expect("the device key kept");
    assert_eq!(json!(kept.public()), members[0]["key"]);
    assert!(
        flottille_link::DeviceKey::load_pending(&NoKeyring, &folder)
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

/// A machine with remote off, its interfaces said as a test says them.
fn off_machine(interfaces: &str) -> Machine {
    let user = User::new().with_env("FLOTTILLE_TEST_INTERFACES", interfaces);
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user.status();
    Machine { user }
}

const INTERFACES: &str =
    r#"[["eth0","192.168.1.20"],["wg0","10.8.0.2"],["tailscale0","100.64.0.7"]]"#;

fn stack(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<&'static str> {
    shell.read_with(cx, |s, cx| s.modal_stack(cx))
}

fn type_into(
    cx: &mut VisualTestContext,
    field: &Entity<gpui_kit::base::input::InputState>,
    text: &str,
) {
    let text = text.to_owned();
    cx.update(|window, cx| field.update(cx, |f, cx| f.set_value(text, window, cx)));
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
    for section in ["reached", "hub", "devices", "addresses"] {
        let title = format!("{section}-title");
        until(&shell, cx, section, |cx| present(cx, &title));
    }
    click(&shell, cx, "settings-tabs-general");
    settle(&shell, cx);
    for section in [
        "devices-title",
        "addresses-title",
        "reached-title",
        "remote",
    ] {
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
    let pc = off_machine(INTERFACES);
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
    assert!(
        said.contains("nothing is opened until you say where it is reached"),
        "{said}"
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
    // A machine with no hub is offered Set up a hub.
    assert!(present(cx, "open-hub-setup"));
}

#[gpui_kit::test]
fn a_tailnet_address_is_offered_first_and_marked(cx: &mut TestAppContext) {
    let pc = off_machine(INTERFACES);
    pc.user
        .call("remote.configure", json!({"on": true}))
        .unwrap();
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the choice is offered", |cx| {
        present(cx, "reach")
    });
    click(&shell, cx, "reach");
    for key in [
        "nowhere",
        "100.64.0.7",
        "10.8.0.2",
        "192.168.1.20",
        "every",
        "other",
    ] {
        until(&shell, cx, key, |cx| present(cx, &format!("reach-{key}")));
    }
    // Nowhere first (the default), then the tailnet's, then the local network's.
    let order: Vec<f32> = [
        "nowhere",
        "100.64.0.7",
        "10.8.0.2",
        "192.168.1.20",
        "every",
        "other",
    ]
    .iter()
    .map(|key| top(cx, &format!("reach-{key}")))
    .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    for tailnet in ["100.64.0.7", "10.8.0.2"] {
        assert_eq!(
            label(cx, &format!("reach-{tailnet}-line")).as_deref(),
            Some("Only your tailnet"),
            "{tailnet}"
        );
    }
    assert!(
        label(cx, "reach-100.64.0.7")
            .unwrap()
            .contains("tailscale0"),
        "named by its interface"
    );
    assert!(
        label(cx, "reach-192.168.1.20-line")
            .unwrap()
            .contains("local network")
    );
    assert!(
        label(cx, "reach-every-line")
            .unwrap()
            .contains("any network")
    );
}

#[gpui_kit::test]
fn choosing_where_this_machine_is_reached_listens_there(cx: &mut TestAppContext) {
    let pc = off_machine(INTERFACES);
    pc.user
        .call("remote.configure", json!({"on": true}))
        .unwrap();
    let pid = pc.user.status()["pid"].clone();
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the choice is offered", |cx| {
        present(cx, "reach")
    });
    // Another address…: its field unfolds under the choice, and the address is used from it.
    click(&shell, cx, "reach");
    click(&shell, cx, "reach-other");
    until(&shell, cx, "its field unfolds", |cx| {
        present(cx, "reach-other-use")
    });
    let field = shell.read_with(cx, |s, cx| {
        s.machines().read(cx).listen_field().unwrap().clone()
    });
    type_into(cx, &field, "127.0.0.1:0");
    click(&shell, cx, "reach-other-use");
    until(&shell, cx, "the daemon listens there", |_| {
        pc.user.call("remote.settings", json!({})).unwrap()["listening"]
            .as_str()
            .is_some_and(|at| at.starts_with("127.0.0.1:"))
    });
    assert_eq!(pc.user.status()["pid"], pid, "no restart");
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|t| t.starts_with("This machine is reached at 127.0.0.1")),
        "{:?}",
        toasts(&shell, cx)
    );
    // Back to nowhere.
    click(&shell, cx, "reach");
    click(&shell, cx, "reach-nowhere");
    until(&shell, cx, "the daemon listens nowhere", |_| {
        pc.user.call("remote.settings", json!({})).unwrap()["listening"].is_null()
    });
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
    // Turned on again, the hub's setup is the same.
    click(&shell, cx, "turn-on-remote");
    let deadline = Instant::now() + WAIT;
    while !(present(cx, "member-server") && present(cx, "member-pc")) {
        step(&shell, Duration::from_millis(16), cx);
        assert!(
            Instant::now() < deadline,
            "its devices are not listed again: settings {:?}, switch {}, no hub {}, rest {}, read {:?}\n{}",
            pc.user.call("remote.settings", json!({})),
            present(cx, "remote-switch"),
            present(cx, "no-hub-title"),
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

#[gpui_kit::test]
fn a_change_that_waits_for_the_next_start_says_so(cx: &mut TestAppContext) {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    let pc = Machine::of(user);
    let folder = pc.user.path("work/harbor");
    pc.user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "harbor\n").unwrap();
    pc.user.git(&folder, &["add", "README.md"]);
    pc.user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    pc.user
        .call("project.add", json!({"folder": folder, "name": "harbor"}))
        .unwrap();
    pc.user
        .call(
            "agent.start",
            json!({"project": "harbor", "message": "Write the notes"}),
        )
        .unwrap();
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the choice is offered", |cx| {
        present(cx, "reach")
    });
    assert!(!present(cx, "reach-waits"));
    click(&shell, cx, "reach");
    click(&shell, cx, "reach-other");
    until(&shell, cx, "its field unfolds", |cx| {
        present(cx, "reach-other-use")
    });
    let field = shell.read_with(cx, |s, cx| {
        s.machines().read(cx).listen_field().unwrap().clone()
    });
    type_into(cx, &field, "127.0.0.2:0");
    click(&shell, cx, "reach-other-use");
    until(&shell, cx, "the wait is said", |cx| {
        present(cx, "reach-waits")
    });
    let said = label(cx, "reach-waits").unwrap();
    assert!(said.contains("next start"), "{said}");
    assert!(said.contains("agent"), "{said}");
}

#[gpui_kit::test]
fn setting_up_a_hub_is_a_dialog_not_a_form_on_the_page(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "the setup is offered", |cx| {
        present(cx, "open-hub-setup")
    });
    settle(&shell, cx);
    // The page draws no field of the setup, and no button that sets up.
    for id in ["hub-url", "hub-code", "set-up-hub"] {
        assert!(!present(cx, id), "{id} is drawn on the page");
    }
    assert!(stack(&shell, cx).is_empty());

    click(&shell, cx, "open-hub-setup");
    assert_eq!(stack(&shell, cx), vec!["hub-setup"]);
    for id in ["hub-url", "hub-code", "set-up-hub"] {
        assert!(present(cx, id), "{id} is in the dialog");
    }
    let (url, code) = shell.read_with(cx, |s, cx| {
        let section = s.machines().read(cx);
        (
            section.url_field().unwrap().clone(),
            section.code_field().unwrap().clone(),
        )
    });
    // A refusal stays in the dialog, under its field: no toast says it.
    type_into(cx, &url, &hub.url());
    type_into(cx, &code, "0000-0000-0000-0000");
    click(&shell, cx, "set-up-hub");
    until(&shell, cx, "the refusal is said", |cx| {
        shell.read_with(cx, |s, cx| s.machines().read(cx).setup_problem().is_some())
            && present(cx, "hub-setup-problem")
    });
    assert_eq!(
        stack(&shell, cx),
        vec!["hub-setup"],
        "the dialog stays open"
    );
    assert!(
        !toasts(&shell, cx)
            .iter()
            .any(|t| t == "The hub was not set up")
    );
    assert!(pc.user.call("remote.members", json!({})).is_err() || pc.members().is_empty());

    // The right code closes it, and a toast says it.
    type_into(cx, &code, &hub.code());
    click(&shell, cx, "set-up-hub");
    until(&shell, cx, "the devices are listed", |cx| {
        present(cx, "add-device")
    });
    settle(&shell, cx);
    assert!(stack(&shell, cx).is_empty());
    assert!(
        toasts(&shell, cx)
            .iter()
            .any(|t| t == "This machine set up the hub")
    );
    assert_eq!(pc.members().len(), 1);
}

#[gpui_kit::test]
fn adding_a_device_shows_its_invitation_in_a_dialog(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    let laptop = Machine::new();
    set_up(&hub, &pc);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "Add a device is offered", |cx| {
        present(cx, "add-device")
    });
    click(&shell, cx, "add-device");
    until(&shell, cx, "the invitation is shown", |cx| {
        present(cx, "invitation-qr-code")
    });
    settle(&shell, cx);
    assert_eq!(stack(&shell, cx), vec!["invitation"]);
    // Its QR, its code, Copy code and its expiry are the dialog's, none the section's.
    for id in ["invitation-code", "invitation-expiry", "copy-invitation"] {
        assert!(present(cx, id), "{id}");
    }
    assert!(
        !present(cx, "invitation-card"),
        "nothing unfolds in Devices"
    );
    let code = label(cx, "invitation-code").unwrap();

    // Used: the device asks to join with it, and the dialog closes (the join request opens).
    let mut joining = laptop
        .user
        .command(&["remote", "join"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    joining
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{code}\n").as_bytes())
        .unwrap();
    until(&shell, cx, "the invitation closes", |cx| {
        !stack(&shell, cx).contains(&"invitation")
    });
    joining.kill().ok();
}

#[gpui_kit::test]
fn an_invitation_that_expires_closes_its_dialog(cx: &mut TestAppContext) {
    let hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "Add a device is offered", |cx| {
        present(cx, "add-device")
    });
    click(&shell, cx, "add-device");
    until(&shell, cx, "the invitation is shown", |cx| {
        stack(&shell, cx) == vec!["invitation"]
    });
    step(&shell, Duration::from_secs(5 * 60), cx);
    settle(&shell, cx);
    assert!(stack(&shell, cx).is_empty());
    assert!(!present(cx, "invitation-qr-code"));
}
