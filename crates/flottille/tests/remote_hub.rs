//! Machines that know each other through the hub (`docs/remote.md`, "The hub", "The first
//! member", "Architecture", "The transport"): a hub served in the test on 127.0.0.1, and daemons,
//! each with its own temporary HOME and XDG folders and in-memory keyring. The first machine is
//! set up with `flottille remote setup`; a second one is added by the test as #812's joining will
//! add it: an invitation answered on the hub, the root's signed addition, the new device's
//! declaration of its machine, and its hub file written.

mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use common::User;
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Action, Change, Device, DeviceKey, Hello, HubKey, InvitationAsk, Keyring, KeyringError,
    MachineKey, MemberList, PublicKey, Role, ToDevice,
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

/// How long a link, or the hub's news, may take: the daemons try again every second.
const WAIT: Duration = Duration::from_secs(40);

/// A keyring that is not there, as on a machine with no Secret Service: the device key is the
/// file a client keeps in the key folder.
struct NoKeyring;

impl Keyring for NoKeyring {
    fn read(
        &self,
        _attributes: &[(&str, &str)],
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
        Err(KeyringError::Absent)
    }

    fn store(
        &self,
        _label: &str,
        _attributes: &[(&str, &str)],
        _secret: &[u8],
    ) -> Result<(), KeyringError> {
        Err(KeyringError::Absent)
    }
}

/// The hub, served from a temporary state folder in a runtime of the test's own.
struct TestHub {
    runtime: tokio::runtime::Runtime,
    folder: tempfile::TempDir,
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
        let served = TestHub::serve_on(&runtime, &folder.path().join("hub"), "127.0.0.1:0");
        let address = served.address();
        TestHub {
            runtime,
            folder,
            served: Some(served),
            address,
        }
    }

    fn serve_on(runtime: &tokio::runtime::Runtime, folder: &Path, at: &str) -> Served<SystemClock> {
        runtime.block_on(async {
            let hub = Hub::open(folder, SystemClock).unwrap();
            let listener = tokio::net::TcpListener::bind(at).await.unwrap();
            flottille_hub::serve(hub, listener, None, Hello::ours("0.4.0")).unwrap()
        })
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn served(&self) -> &Served<SystemClock> {
        self.served.as_ref().expect("the hub is up")
    }

    fn code(&self) -> String {
        self.served()
            .with_hub(|hub| hub.setup_code().unwrap().to_owned())
    }

    /// The hub stops: down, as a machine that is off.
    fn down(&mut self) {
        let served = self.served.take().unwrap();
        self.runtime.block_on(served.stop());
    }

    fn key(&self) -> PublicKey {
        self.served().with_hub(|hub| hub.key())
    }
}

struct Machine {
    user: User,
}

impl Machine {
    /// A machine with remote on, listening at `listen` (none: it only dials).
    fn new(listen: Option<&str>) -> Machine {
        let user = User::new();
        let listen = listen
            .map(|listen| format!("listen = \"{listen}\"\n"))
            .unwrap_or_default();
        std::fs::write(
            user.path("config/flottille/daemon.toml"),
            format!("[remote]\non = true\n{listen}"),
        )
        .unwrap();
        user.status();
        Machine { user }
    }

    fn remote(&self) -> Value {
        self.user.call("remote.machines", json!({})).unwrap()
    }

    fn key(&self) -> PublicKey {
        serde_json::from_value(self.remote()["this"]["key"].clone()).unwrap()
    }

    fn folder(&self) -> std::path::PathBuf {
        self.user.path("state/flottille/remote")
    }

    /// The device key this machine's clients hold: made by the first client that needs it, read
    /// here as a client reads it.
    fn device(&self) -> DeviceKey {
        DeviceKey::load_or_make(&NoKeyring, &self.folder()).unwrap()
    }

    /// The key this machine's daemon holds, read as the daemon reads it: what claims its
    /// declaration in a list the test makes.
    fn machine_key(&self) -> MachineKey {
        MachineKey::load_or_make(&self.folder()).unwrap()
    }

    fn machine(&self, name: &str) -> Option<Value> {
        self.remote()["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|machine| machine["name"] == name)
            .cloned()
    }

    /// Waits until `check` holds of what `remote.machines` says; that answer then.
    fn wait_until(&self, what: &str, check: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let seen = self.remote();
            if check(&seen) {
                return seen;
            }
            assert!(
                Instant::now() < deadline,
                "{what}: {seen:#}\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn wait_for(&self, name: &str, state: &str) -> Value {
        self.wait_until(&format!("{name} is not {state}"), |seen| {
            seen["machines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|machine| machine["name"] == name && machine["state"] == state)
        });
        self.machine(name).unwrap()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.user.path("state/flottille/daemon.log")).unwrap_or_default()
    }

    fn restart(&self) {
        assert!(self.user.run(&["daemon", "stop"]).status.success());
        self.user.status();
    }
}

/// `pc` set up as the hub's first member with `flottille remote setup`, then `server` added by
/// it, a runner; each machine's addresses told to the hub by its daemon.
fn members(hub: &TestHub, pc: &Machine, server: &Machine) {
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
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(said.contains("the first member"), "{said}");
    let pc_device = pc.device();
    let server_device = server.device();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let server_machine = server.key();
    let served = hub.served();
    served.with_hub(|hub| {
        let ask = InvitationAsk::sign(&hub.key(), now, &pc_device).unwrap();
        let token = hub.invite(&ask).unwrap();
        hub.join(&token, server_device.public()).unwrap();
        let added = Change::sign(
            hub.list().head(),
            Action::Add {
                key: server_device.public(),
                name: "server".to_owned(),
                role: Role::Runner,
            },
            &pc_device,
        );
        hub.submit(added.clone()).unwrap();
        hub.submit(
            Change::sign(
                added.hash(),
                Action::Machine {
                    key: server_device.public(),
                    machine: server_machine,
                },
                &server_device,
            )
            .claimed_by(&server.machine_key()),
        )
        .unwrap();
    });
    // What #812's joining will leave on the new machine: its hub, pinned.
    let joined = Device {
        url: hub.url(),
        hub: hub.key(),
        root: pc_device.public(),
        list: Default::default(),
    };
    std::fs::write(
        server.folder().join("hub.json"),
        serde_json::to_string(&joined).unwrap(),
    )
    .unwrap();
}

#[test]
fn links_go_on_while_the_hub_is_down() {
    let mut hub = TestHub::new();
    let (pc, server) = (
        Machine::new(Some("127.0.0.1:0")),
        Machine::new(Some("127.0.0.1:0")),
    );
    members(&hub, &pc, &server);

    // Each learns the other from the hub, and its addresses: they link, directly.
    let seen = pc.wait_for("server", "online");
    assert_eq!(seen["key"], json!(server.key()));
    server.wait_for("pc", "online");
    let hub_seen = pc.remote()["hub"].clone();
    assert_eq!(hub_seen["state"], "connected", "{hub_seen}");
    assert_eq!(hub_seen["url"], format!("{}/", hub.url()));

    hub.down();
    // What needs the hub says so, with since when...
    let seen = pc.wait_until("the hub is not said unreachable", |seen| {
        seen["hub"]["state"] == "unreachable"
    });
    assert!(seen["hub"]["since"].is_u64(), "{seen:#}");
    let reason = seen["hub"]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with(&format!("the hub at {} is unreachable since ", hub.address)),
        "{reason}"
    );
    assert!(pc.log().contains("is unreachable since"), "{}", pc.log());
    // ...and the link goes on.
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(pc.machine("server").unwrap()["state"], "online");
    assert_eq!(server.machine("pc").unwrap()["state"], "online");

    // A machine off and on again links again, without the hub.
    server.restart();
    server.wait_for("pc", "online");
    pc.wait_for("server", "online");
    assert_eq!(server.remote()["hub"]["state"], "unreachable");
    let _ = hub.folder.path();
}

#[test]
fn two_machines_that_cannot_reach_each_other_say_which_address_failed() {
    let hub = TestHub::new();
    // Neither listens; each declares an address nobody listens on, which the hub tells the other.
    let (pc, server) = (Machine::new(None), Machine::new(None));
    let (pc_dead, server_dead) = (free_port(), free_port());
    for (machine, port) in [(&pc, pc_dead), (&server, server_dead)] {
        machine
            .user
            .call(
                "remote.address.add",
                json!({"address": format!("127.0.0.1:{port}")}),
            )
            .unwrap();
    }
    members(&hub, &pc, &server);

    let seen = pc.wait_until("pc does not say the server is unreachable", |seen| {
        seen["machines"].as_array().unwrap().iter().any(|machine| {
            machine["name"] == "server"
                && machine["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("is unreachable"))
        })
    });
    let server_seen = seen["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|machine| machine["name"] == "server")
        .unwrap()
        .clone();
    assert_eq!(server_seen["state"], "offline");
    assert_eq!(
        server_seen["reason"],
        format!("server is unreachable: tried 127.0.0.1:{server_dead} from pc, no answer in 10 s")
    );
    assert!(
        pc.log().contains(&format!(
            "no link to server: server is unreachable: tried 127.0.0.1:{server_dead} from pc"
        )),
        "{}",
        pc.log()
    );

    // And on the other side, the other way.
    let seen = server.wait_until("the server does not say pc is unreachable", |seen| {
        seen["machines"].as_array().unwrap().iter().any(|machine| {
            machine["name"] == "pc"
                && machine["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("is unreachable"))
        })
    });
    let pc_seen = seen["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|machine| machine["name"] == "pc")
        .unwrap()
        .clone();
    assert_eq!(
        pc_seen["reason"],
        format!("pc is unreachable: tried 127.0.0.1:{pc_dead} from server, no answer in 10 s")
    );
}

#[test]
fn the_daemon_keeps_a_list_the_hub_forged_out() {
    // A member list that does not check back to the pinned root is never taken: the machines the
    // daemon trusts stay those the members signed.
    let hub = TestHub::new();
    let (pc, server) = (
        Machine::new(Some("127.0.0.1:0")),
        Machine::new(Some("127.0.0.1:0")),
    );
    members(&hub, &pc, &server);
    pc.wait_for("server", "online");
    // The server's hub file, as a hub that lied would leave it if the daemon took its word: the
    // pinned root changed. The daemon refuses the hub's list, saying it was reset.
    let file = server.folder().join("hub.json");
    let mut pinned: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    pinned["root"] = json!(server.device().public());
    pinned["list"] = json!([]);
    std::fs::write(&file, pinned.to_string()).unwrap();
    server.restart();
    let seen = server.wait_until("the server does not refuse the list", |seen| {
        seen["hub"]["state"] == "reset"
    });
    assert_eq!(
        seen["hub"]["reason"],
        "the hub's member list was reset: set this device up again"
    );
}

/// A hub of the test's own, which a machine pinned: it holds its key, welcomes the machine's
/// daemon, sends `list` once the daemon asked something, and answers the n-th ask of addresses as
/// `answer(n)` says: never (`None`), done (`Some(None)`), or refused for a reason.
struct FakeHub {
    runtime: tokio::runtime::Runtime,
    address: SocketAddr,
    key: PublicKey,
    addresses_asked: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    _folder: tempfile::TempDir,
}

impl FakeHub {
    fn new(list: MemberList, answer: fn(usize) -> Option<Option<String>>) -> FakeHub {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let folder = tempfile::tempdir().unwrap();
        let key = HubKey::load_or_make(&folder.path().join("hub")).unwrap();
        let public = key.public();
        let addresses_asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = addresses_asked.clone();
        let address = runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let key = std::sync::Arc::new(key);
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let (key, list, counted) = (key.clone(), list.clone(), counted.clone());
                    tokio::spawn(async move {
                        let Ok(arriving) =
                            flottille_link::arrive(Box::new(stream), &key, &Hello::ours("0.4.0"))
                                .await
                        else {
                            return;
                        };
                        let mut session = arriving.welcome().await.unwrap();
                        let mut sent = false;
                        while let Ok(asked) = session.ask().await {
                            if !sent {
                                sent = true;
                                session.send(&ToDevice::List(list.clone())).await.unwrap();
                            }
                            let answer = match asked.ask {
                                flottille_link::Ask::Addresses { .. } => {
                                    let n = counted
                                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                                        + 1;
                                    answer(n)
                                }
                                _ => Some(None),
                            };
                            if let Some(refused) = answer {
                                let answer = ToDevice::Answer {
                                    id: asked.id,
                                    refused,
                                    proof: None,
                                };
                                if session.send(&answer).await.is_err() {
                                    return;
                                }
                            }
                        }
                    });
                }
            });
            address
        });
        FakeHub {
            runtime,
            address,
            key: public,
            addresses_asked,
            _folder: folder,
        }
    }

    fn asked(&self) -> usize {
        self.addresses_asked
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The list a fake hub sends `machine`: its device the root, declaring its machine, and a laptop
/// the root added, with a machine of its own.
fn list_for(machine: &Machine) -> MemberList {
    let (pc, laptop) = (machine.device(), seeded(31));
    let mut list = MemberList::start("pc", &pc).unwrap();
    list.declare(&pc, &machine.machine_key());
    list.sign(
        Action::Add {
            key: laptop.public(),
            name: "laptop".to_owned(),
            role: Role::Runner,
        },
        &pc,
    );
    let laptop_machine = MachineKey::load_or_make(&machine.user.path("laptop")).unwrap();
    list.declare(&laptop, &laptop_machine);
    list
}

/// `machine` pinned to `hub`, its daemon started again to take it.
fn pin(machine: &Machine, hub: &FakeHub) {
    let pinned = Device {
        url: format!("http://{}", hub.address),
        hub: hub.key,
        root: machine.device().public(),
        list: MemberList::default(),
    };
    std::fs::write(
        machine.folder().join("hub.json"),
        serde_json::to_string(&pinned).unwrap(),
    )
    .unwrap();
    machine.restart();
}

/// A device key made from a fixed seed.
fn seeded(n: u8) -> DeviceKey {
    struct Seeded([u8; 32]);
    impl Keyring for Seeded {
        fn read(
            &self,
            _attributes: &[(&str, &str)],
        ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
            Ok(Some(Zeroizing::new(self.0.to_vec())))
        }

        fn store(
            &self,
            _label: &str,
            _attributes: &[(&str, &str)],
            _secret: &[u8],
        ) -> Result<(), KeyringError> {
            unreachable!("the key is there")
        }
    }
    DeviceKey::load_or_make(&Seeded([n; 32]), Path::new("/nonexistent")).unwrap()
}

#[test]
fn the_daemon_takes_the_hub_s_news_while_its_ask_waits() {
    let pc = Machine::new(None);
    // A hub that never answers the daemon's ask, and sends the list meanwhile.
    let hub = FakeHub::new(list_for(&pc), |_| None);
    pin(&pc, &hub);
    pc.wait_until("the laptop the hub's list names is not trusted", |seen| {
        seen["machines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|machine| machine["name"] == "laptop")
    });
    assert!(hub.asked() >= 1);
    drop(hub.runtime);
}

#[test]
fn addresses_the_hub_refused_are_told_again() {
    let pc = Machine::new(None);
    let hub = FakeHub::new(list_for(&pc), |n| {
        Some((n == 1).then(|| "not now".to_owned()))
    });
    pin(&pc, &hub);
    let deadline = Instant::now() + Duration::from_secs(20);
    while hub.asked() < 2 {
        assert!(
            Instant::now() < deadline,
            "the addresses the hub refused were not told again\n{}",
            pc.log()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    drop(hub.runtime);
}

/// `remote.setup` of `machine` with `hub`, as `flottille remote setup` asks it; `replace` when
/// asked to replace the hub pinned.
fn setup_call(machine: &Machine, hub: &TestHub, replace: bool) -> Result<Value, (String, String)> {
    let device = machine.device();
    let mut list = MemberList::start("pc", &device).unwrap();
    list.sign(
        Action::Machine {
            key: device.public(),
            machine: machine.key(),
        },
        &device,
    );
    let mut params = json!({"url": hub.url(), "code": hub.code(), "changes": list.changes()});
    if replace {
        params["replace"] = json!(true);
    }
    machine.user.call("remote.setup", params)
}

#[test]
fn a_machine_with_a_hub_pinned_sets_up_another_only_when_asked_to_replace_it() {
    let (first, second) = (TestHub::new(), TestHub::new());
    let pc = Machine::new(None);
    setup_call(&pc, &first, false).unwrap();
    let (_, refused) = setup_call(&pc, &second, false).unwrap_err();
    assert!(refused.contains("--replace"), "{refused}");
    let pinned: Value =
        serde_json::from_str(&std::fs::read_to_string(pc.folder().join("hub.json")).unwrap())
            .unwrap();
    assert_eq!(pinned["url"], format!("{}/", first.url()));

    setup_call(&pc, &second, true).unwrap();
    let pinned: Value =
        serde_json::from_str(&std::fs::read_to_string(pc.folder().join("hub.json")).unwrap())
            .unwrap();
    assert_eq!(pinned["url"], format!("{}/", second.url()));
}

#[test]
fn the_setup_code_is_read_from_the_standard_input_never_from_the_command_line() {
    let hub = TestHub::new();
    let pc = Machine::new(None);
    let code = hub.code();
    let output = pc
        .user
        .run(&["remote", "setup", &hub.url(), &code, "--name", "pc"]);
    assert!(
        !output.status.success(),
        "a setup code was taken from the command line"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unexpected argument"),
        "the refusal does not say the argument is not one"
    );
    let output = pc.user.run_with_input(
        &["remote", "setup", &hub.url(), "--name", "pc"],
        &format!("{code}\n"),
    );
    assert!(
        output.status.success(),
        "the setup from the standard input failed\n{}",
        pc.log()
    );
}

/// A port nobody uses now on loopback: the system's choice, let go at once.
fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
