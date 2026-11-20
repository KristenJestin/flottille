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

/// Settings, General, Machines: the last section of the General tab.
fn machines_tab(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    // Tall enough for the whole of General: what is clicked must be in view.
    cx.simulate_resize(size(px(1280.), px(3000.)));
    click(shell, cx, "settings");
    until(shell, cx, "the settings page is shown", |cx| {
        present(cx, "settings-tabs-general")
    });
    click(shell, cx, "settings-tabs-general");
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

/// Settings, General, Machines: the hub's address and its setup code typed, and Set up a hub clicked.
fn set_up_from_the_page(
    hub: &TestHub,
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> String {
    machines_tab(shell, cx);
    until(shell, cx, "the setup is offered", |cx| {
        present(cx, "set-up-hub")
    });
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
    // Tall enough for the whole of General, the invitation's QR and the rows under it.
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
        present(cx, "settings-tabs-general")
    });

    // The sections show up one after the other, each rising into place: the second measured
    // from the first's bottom, which moves as the first's rows unfold.
    cx.update(|window, cx| window.click("settings-tabs-general", cx));
    let mut offsets = Vec::new();
    for _ in 0..60 {
        step(&shell, Duration::from_millis(16), cx);
        if present(cx, "enter-machines-devices") && present(cx, "enter-machines-addresses") {
            let devices = top(cx, "enter-machines-devices");
            let below = devices + height(cx, "enter-machines-devices");
            offsets.push((devices, top(cx, "enter-machines-addresses") - below));
        }
    }
    let (devices, gap) = *offsets.last().unwrap();
    assert!(
        offsets[0].0 > devices,
        "the first section rises: {offsets:?}"
    );
    // Staggered: the second starts later, so the space between them first opens, then closes as
    // the second arrives.
    let widest = offsets.iter().map(|at| at.1).fold(f32::MIN, f32::max);
    assert!(
        widest > offsets[0].1 && widest > gap,
        "the second section rises after the first: {offsets:?}"
    );
    settle(&shell, cx);

    // The invitation's panel unfolds: a height between none and its own.
    cx.update(|window, cx| window.click("add-device", cx));
    until(&shell, cx, "the invitation comes", |cx| {
        shell.read_with(cx, |s, cx| s.machines().read(cx).invitation().is_some())
    });
    let early = height(cx, "invitation");
    settle(&shell, cx);
    let full = height(cx, "invitation");
    assert!(early < full, "it unfolds: {early} then {full}");
    // The code wraps beside the QR, not a character to a line.
    assert!(full < 480., "the panel holds the QR and its code: {full}");

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

fn address_line(name: &str) -> String {
    format!("member-address-{name}")
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

    // Down, it is unreachable since a time, and says which address was tried and how it failed.
    hub.down();
    until(&shell, cx, "the hub is unreachable", |cx| {
        label(cx, HUB_TAG).is_some_and(|tag| tag.starts_with("unreachable since "))
    });
    until(&shell, cx, "the reason unfolds", |cx| {
        present(cx, HUB_REASON)
    });
    settle(&shell, cx);
    let reason = label(cx, HUB_REASON).unwrap();
    assert!(
        reason.contains(&format!("the hub at {} is unreachable", hub.address)),
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
        label(cx, &line).is_some_and(|said| said.contains("127.0.0.1:"))
    });
    settle(&shell, cx);
    // Linked, it says where: the address pc dialled, or the one the link came from.
    let said = label(cx, &line).unwrap();
    assert!(
        said == format!("Reached at {at}") || said.starts_with("Linked from 127.0.0.1:"),
        "{said}"
    );

    // Off, it says the address that was tried and how it failed.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    until(&shell, cx, "the failed address is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains(&format!("tried {at} from pc")))
    });
    settle(&shell, cx);
    let said = label(cx, &line).unwrap();
    assert!(said.contains("is unreachable"), "{said}");
}

#[gpui_kit::test]
fn adding_a_device_while_the_hub_is_unreachable_says_so(cx: &mut TestAppContext) {
    let (mut hub, pc, server) = pc_and_server(User::new());
    let at = server.found().remove(0);
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    until(&shell, cx, "Add a device is offered", |cx| {
        present(cx, "add-device")
    });
    assert!(!present(cx, HUB_DOWN), "the hub answers: nothing to say");

    hub.down();
    until(&shell, cx, "the hub's absence is said", |cx| {
        present(cx, HUB_DOWN)
    });
    settle(&shell, cx);
    let said = label(cx, HUB_DOWN).unwrap();
    assert!(
        said.starts_with("The hub is unreachable since ") && said.contains("adding a device"),
        "{said}"
    );

    // Add a device cannot work, and asks nothing of the hub.
    click(&shell, cx, "add-device");
    assert!(shell.read_with(cx, |s, cx| s.machines().read(cx).invitation().is_none()));
    assert!(
        !toasts(&shell, cx)
            .iter()
            .any(|t| t == "No invitation was made")
    );

    // A machine only the hub could tell shows the last address it was known at.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    until(&shell, cx, "the last known address is said", |cx| {
        label(cx, &address_line("server")).is_some_and(|said| said.contains(&at))
    });

    // The hub's answering again takes the notice away.
    // (Not tried here: a hub that stopped keeps no port to come back on.)
}

#[gpui_kit::test]
fn the_address_change_of_a_row_fades(cx: &mut TestAppContext) {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (shell, cx) = window(&pc, cx);
    machines_tab(&shell, cx);
    let line = address_line("server");
    until(&shell, cx, "the server's address is said", |cx| {
        label(cx, &line).is_some_and(|said| said.contains("127.0.0.1:"))
    });
    settle(&shell, cx);
    let before = label(cx, &line).unwrap();
    let rest = (0..200).find(|_| step(&shell, Duration::from_millis(16), cx) == 0);
    assert!(rest.is_some(), "the page came to rest before the change");

    assert!(server.user.run(&["daemon", "stop"]).status.success());
    until(&shell, cx, "the address line changes", |cx| {
        label(cx, &line).is_some_and(|said| said != before)
    });
    // The line fades back in over HOVER from the last frame's text: frames are asked while it
    // does, and no more once it has.
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
