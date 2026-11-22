//! Machines that know each other through the hub (`docs/remote.md`, "The hub", "Pairing",
//! "Architecture", "The transport"): a hub served in the test on 127.0.0.1, and daemons, each with
//! its own temporary HOME and XDG folders and in-memory keyring. Each machine pairs as the user
//! pairs it: pair mode opened on the hub, its code given to `flottille remote pair`.

use crate::common;

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use common::User;
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Action, Change, ChangeHash, Colour, Device, DeviceKey, Endpoint, Hello, HubKey, Icon, Keyring,
    KeyringError, LinkError, MachineKey, MemberList, Proposed, PublicKey, Role, Tls, ToDevice,
    Traffic,
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
    /// The pair mode last opened, closed before another opens, as its command would be stopped.
    pairing: std::cell::Cell<Option<u64>>,
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
            pairing: std::cell::Cell::new(None),
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

    /// Pair mode opened, as `flottille-hub pair` opens it on the hub's host: its code.
    fn code(&self) -> String {
        let url = self.url();
        let last = self.pairing.take();
        let (number, code) = self.served().with_hub(|hub| {
            if let Some(last) = last {
                hub.close_pairing(last);
            }
            hub.open_pairing(&url, None).unwrap()
        });
        self.pairing.set(Some(number));
        code.code().unwrap()
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
    /// A machine with remote on: its links go through the hub.
    fn new() -> Machine {
        Machine::of(User::new())
    }

    /// `user`'s machine, with remote on.
    fn of(user: User) -> Machine {
        std::fs::write(
            user.path("config/flottille/daemon.toml"),
            "[remote]\non = true\n",
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

    /// The members of the list this machine checked, by name, with their role.
    fn members(&self) -> Vec<(String, String)> {
        let members = self.user.call("remote.members", json!({})).unwrap();
        members["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|member| {
                (
                    member["name"].as_str().unwrap().to_owned(),
                    member["role"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    /// Waits until the list this machine checked names `name`; its members then.
    fn wait_for_member(&self, name: &str, present: bool) -> Vec<(String, String)> {
        let deadline = Instant::now() + WAIT;
        loop {
            let members = self.members();
            if members.iter().any(|(member, _)| member == name) == present {
                return members;
            }
            assert!(
                Instant::now() < deadline,
                "{name} is {}in the list: {members:?}\n{}",
                if present { "not " } else { "still " },
                self.log()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// The events this machine's daemon stored named `name`.
    fn events(&self, name: &str) -> Vec<Value> {
        self.user
            .watch(&["--after", "0"])
            .within(Duration::from_secs(2))
            .into_iter()
            .filter(|event| event["name"] == name)
            .map(|event| event["data"].clone())
            .collect()
    }
}

/// `pc` paired as the hub's first device, a runner.
fn set_up(hub: &TestHub, pc: &Machine) {
    pair(hub, pc, "pc", "runner");
}

/// `machine` paired with `hub` as the user pairs it: pair mode opened on the hub, its code given to
/// `flottille remote pair` as `name` with `role`. What the command printed.
fn pair(hub: &TestHub, machine: &Machine, name: &str, role: &str) -> String {
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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// `pc` paired as the hub's first device, then `server`, a runner.
fn members(hub: &TestHub, pc: &Machine, server: &Machine) {
    set_up(hub, pc);
    pair(hub, server, "server", "runner");
}

#[test]
fn the_hub_down_is_said_with_since_when_and_the_links_end() {
    let mut hub = TestHub::new();
    let (pc, server) = (Machine::new(), Machine::new());
    members(&hub, &pc, &server);

    // Each learns the other from the hub: they link, through it.
    let seen = pc.wait_for("server", "online");
    assert_eq!(seen["key"], json!(server.key()));
    server.wait_for("pc", "online");
    let hub_seen = pc.remote()["hub"].clone();
    assert_eq!(hub_seen["state"], "connected", "{hub_seen}");
    assert_eq!(hub_seen["url"], hub.url());

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
    // ...and the link, which went through it, ends.
    pc.wait_for("server", "offline");
    server.wait_for("pc", "offline");
    let _ = hub.folder.path();
}

#[test]
fn the_hub_s_state_is_said_as_an_event_when_it_changes() {
    let mut hub = TestHub::new();
    let (pc, server) = (Machine::new(), Machine::new());
    members(&hub, &pc, &server);
    pc.wait_for("server", "online");

    hub.down();
    pc.wait_until("the hub is not said unreachable", |seen| {
        seen["hub"]["state"] == "unreachable"
    });
    // The clients that watch learn it at once, not at their next read.
    let said = pc.events("remote.hub");
    assert!(
        said.iter().any(|event| event["state"] == "unreachable"),
        "{said:?}"
    );
}

#[test]
fn the_daemon_keeps_a_list_the_hub_forged_out() {
    // A member list that does not check back to the pinned root is never taken: the machines the
    // daemon trusts stay those the members signed.
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(), Machine::new());
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
        "the hub's member list was reset: pair this device again"
    );
}

/// A hub of the test's own, which a machine pinned: it holds its key, welcomes the machine's
/// daemon, sends `list` once the daemon asked something, and answers the n-th ask as `answer(n)`
/// says: never (`None`), done (`Some(None)`), or refused for a reason.
struct FakeHub {
    runtime: tokio::runtime::Runtime,
    address: SocketAddr,
    key: PublicKey,
    asked_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
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
        let asked_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = asked_count.clone();
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
                            // Sent once the daemon asks something: what it is, at its start.
                            if !sent {
                                sent = true;
                                session.send(&ToDevice::List(list.clone())).await.unwrap();
                            }
                            let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            let answer = answer(n);
                            if let Some(refused) = answer {
                                let answer = ToDevice::Answer {
                                    id: asked.id,
                                    refused,
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
            asked_count,
            _folder: folder,
        }
    }

    fn asked(&self) -> usize {
        self.asked_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The list a fake hub sends `machine`, as a hub set up before pair mode keeps it: its device the
/// root, declaring its machine, and a laptop the root added, with a machine of its own.
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
    let pc = Machine::new();
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

/// `remote.pair`, then `remote.declare`, of `machine` with `hub`, as `flottille remote pair` asks
/// them, its device `device`; `replace` when asked to replace the hub pinned. The pairing's answer.
fn pair_call(
    machine: &Machine,
    hub: &TestHub,
    device: &DeviceKey,
    name: &str,
    replace: bool,
) -> Result<Value, (String, String)> {
    let proposed = Proposed {
        key: device.public(),
        name: name.to_owned(),
        role: Role::Runner,
        icon: Icon::Desktop,
        colour: Colour::Blue,
    };
    let paired = machine.user.call(
        "remote.pair",
        json!({"code": hub.code(), "proposed": proposed, "replace": replace}),
    )?;
    let head: ChangeHash = serde_json::from_value(paired["head"].clone()).unwrap();
    let declared = Change::sign(
        head,
        Action::Machine {
            key: device.public(),
            machine: machine.key(),
        },
        device,
    );
    machine
        .user
        .call("remote.declare", json!({"change": declared}))?;
    Ok(paired)
}

#[test]
fn a_machine_paired_with_a_hub_pairs_with_another_only_when_asked_to_replace_it() {
    let (first, second) = (TestHub::new(), TestHub::new());
    let pc = Machine::new();
    pair_call(&pc, &first, &seeded(41), "pc", false).unwrap();
    let (_, refused) = pair_call(&pc, &second, &seeded(41), "pc", false).unwrap_err();
    assert!(refused.contains("--replace"), "{refused}");
    let pinned: Value =
        serde_json::from_str(&std::fs::read_to_string(pc.folder().join("hub.json")).unwrap())
            .unwrap();
    // The file keeps the address as it was given; only what is shown drops the slash.
    assert_eq!(pinned["url"], format!("{}/", first.url()));

    pair_call(&pc, &second, &seeded(41), "pc", true).unwrap();
    let pinned: Value =
        serde_json::from_str(&std::fs::read_to_string(pc.folder().join("hub.json")).unwrap())
            .unwrap();
    assert_eq!(pinned["url"], format!("{}/", second.url()));
    assert_eq!(pinned["hub"], json!(second.key()));
}

#[test]
fn the_pair_code_is_read_from_the_standard_input_never_from_the_command_line() {
    let hub = TestHub::new();
    let pc = Machine::new();
    let code = hub.code();
    let output = pc.user.run(&["remote", "pair", &code, "--name", "pc"]);
    assert!(
        !output.status.success(),
        "a pair code was taken from the command line"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unexpected argument"),
        "the refusal does not say the argument is not one"
    );
    let output = pc.user.run_with_input(
        &["remote", "pair", "--name", "pc", "--role", "runner"],
        &format!("{code}\n"),
    );
    assert!(
        output.status.success(),
        "the pairing from the standard input failed\n{}",
        pc.log()
    );
    // Not a pair code: said before anything is made.
    let output = pc
        .user
        .run_with_input(&["remote", "pair", "--replace"], "7KQ2-M4XP-9TRW-H3CD\n");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("this is not a pair code"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_headless_machine_pairs_with_the_code_and_is_in_every_list() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(), Machine::new());
    set_up(&hub, &pc);
    let paired = pair(&hub, &server, "server", "runner");
    assert!(
        paired.contains(&format!(
            "server is paired with the hub at {} as a runner",
            hub.url()
        )),
        "{paired}"
    );

    // In every list: the hub's, the inviter's, its own.
    let expected = vec![
        ("pc".to_owned(), "runner".to_owned()),
        ("server".to_owned(), "runner".to_owned()),
    ];
    let names: Vec<String> = hub.served().with_hub(|hub| {
        hub.members()
            .unwrap()
            .members()
            .iter()
            .map(|member| member.name.clone())
            .collect()
    });
    assert_eq!(names, ["pc", "server"]);
    assert_eq!(pc.wait_for_member("server", true), expected);
    assert_eq!(server.members(), expected);
    let this = server.user.call("remote.members", json!({})).unwrap()["this"].clone();
    assert_eq!(this, "server");
    // And the two link, through what the hub told.
    pc.wait_for("server", "online");
    server.wait_for("pc", "online");
}

#[test]
fn a_device_paired_again_keeps_its_key_and_its_card() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(), Machine::new());
    set_up(&hub, &pc);
    pair(&hub, &server, "server", "runner");
    let key = server.device().public();
    // Its hub forgotten on the machine, it pairs again with a new code, proposing another name and
    // role: its key, its name and its role are those it had, and the list did not grow.
    let changes = hub.served().with_hub(|hub| hub.list().changes().len());
    // Stopped first: a daemon that runs writes the list it takes into the file.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    std::fs::remove_file(server.folder().join("hub.json")).unwrap();
    server.user.status();
    let again = pair(&hub, &server, "renamed", "viewer");
    assert!(
        again.contains("server is paired with the hub at"),
        "{again}"
    );
    assert!(again.contains("as a runner"), "{again}");
    assert_eq!(server.device().public(), key);
    assert_eq!(
        hub.served().with_hub(|hub| hub.list().changes().len()),
        changes
    );
    assert_eq!(
        server.members(),
        [
            ("pc".to_owned(), "runner".to_owned()),
            ("server".to_owned(), "runner".to_owned())
        ]
    );
}

#[test]
fn the_name_icon_and_role_proposed_are_this_machines_own() {
    let hub = TestHub::new();
    let pc = Machine::new();
    let proposed = pc.remote()["proposed"].clone();
    // The host's name; an icon among the five; a role by the agents signed in here (none).
    assert!(
        proposed["name"]
            .as_str()
            .is_some_and(|name| !name.is_empty()),
        "{proposed}"
    );
    assert!(
        ["desktop", "laptop", "server"].contains(&proposed["icon"].as_str().unwrap()),
        "{proposed}"
    );
    assert_eq!(proposed["role"], "controller", "{proposed}");
    // Paired with nothing given, the device is what was proposed.
    let output = pc
        .user
        .run_with_input(&["remote", "pair", "--json"], &format!("{}\n", hub.code()));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let members = pc.user.call("remote.members", json!({})).unwrap();
    let this = &members["members"][0];
    assert_eq!(this["name"], proposed["name"]);
    assert_eq!(this["icon"], proposed["icon"]);
    assert_eq!(this["role"], "controller");
    assert_eq!(this["added_by"], "the hub");
    assert_eq!(this["colour"], json!(Colour::of(&pc.device().public())));
}

#[test]
fn name_icon_and_colour_change_from_any_device_and_the_role_on_the_hub_only() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(), Machine::new());
    members(&hub, &pc, &server);
    pc.wait_for_member("server", true);
    // From the server, pc's card; from pc, the server's name.
    for args in [
        &["remote", "icon", "pc", "laptop"][..],
        &["remote", "colour", "pc", "teal"],
    ] {
        let done = server.user.run(args);
        assert!(
            done.status.success(),
            "{}",
            String::from_utf8_lossy(&done.stderr)
        );
    }
    let renamed = pc.user.run(&["remote", "rename", "server", "nas"]);
    assert!(
        renamed.status.success(),
        "{}",
        String::from_utf8_lossy(&renamed.stderr)
    );
    pc.wait_for_member("nas", true);
    let card = |machine: &Machine, name: &str| {
        let deadline = Instant::now() + WAIT;
        loop {
            let members = machine.user.call("remote.members", json!({})).unwrap();
            let found = members["members"]
                .as_array()
                .unwrap()
                .iter()
                .find(|member| member["name"] == name)
                .cloned();
            if let Some(found) = found
                && found["colour"] == "teal"
            {
                return found;
            }
            assert!(Instant::now() < deadline, "{members:#}");
            std::thread::sleep(Duration::from_millis(200));
        }
    };
    let seen = card(&pc, "pc");
    assert_eq!(
        (seen["icon"].as_str(), seen["role"].as_str()),
        (Some("laptop"), Some("runner"))
    );

    // The role: no command of a device's changes it; the hub's host does, and every device sees it.
    assert!(
        !pc.user
            .run(&["remote", "role", "nas", "viewer"])
            .status
            .success()
    );
    hub.served()
        .with_hub(|hub| hub.change_role("nas", Role::Viewer).unwrap());
    let deadline = Instant::now() + WAIT;
    while !pc
        .members()
        .contains(&("nas".to_owned(), "viewer".to_owned()))
    {
        assert!(Instant::now() < deadline, "{:?}", pc.members());
        std::thread::sleep(Duration::from_millis(200));
    }
    let said = pc.events("remote.member_role");
    assert!(
        said.iter()
            .any(|event| event["name"] == "nas" && event["role"] == "viewer"),
        "{said:?}"
    );
}

#[test]
fn a_device_card_holds_what_it_says_of_itself_and_what_the_hub_knows() {
    let hub = TestHub::new();
    let pc = Machine::new();
    set_up(&hub, &pc);
    let deadline = Instant::now() + WAIT;
    let this = loop {
        let members = pc.user.call("remote.members", json!({})).unwrap();
        let this = members["members"][0].clone();
        if this["kind"].is_string() {
            break this;
        }
        assert!(Instant::now() < deadline, "{members:#}");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(this["kind"], "computer");
    assert_eq!(this["system"], std::env::consts::OS);
    assert!(this["version"].is_string(), "{this}");
    assert!(this["paired_at"].is_u64(), "{this}");
    assert!(this["last_seen"].is_u64(), "{this}");
}

#[test]
fn a_hub_set_up_before_pair_mode_keeps_its_machine_and_pairs_the_next() {
    // The state of a hub and of its one machine as they were set up with a setup code: the hub's
    // list, the machine's root key, its pinned hub and its device key.
    let hub_dir = tempfile::tempdir().unwrap();
    let state = hub_dir.path().join("hub");
    let hub_key = HubKey::load_or_make(&state).unwrap().public();
    let pc = Machine::new();
    let device = seeded(51);
    std::fs::write(pc.folder().join("device.key"), [51u8; 32]).unwrap();
    std::fs::set_permissions(
        pc.folder().join("device.key"),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    let mut list = MemberList::start("pc", &device).unwrap();
    list.declare(&device, &pc.machine_key());
    let hub_state = json!({"version": 1, "members": list});
    std::fs::write(state.join("hub.json"), hub_state.to_string()).unwrap();
    std::fs::set_permissions(
        state.join("hub.json"),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let served = TestHub::serve_on(&runtime, &state, "127.0.0.1:0");
    let address = served.address();
    let hub = TestHub {
        runtime,
        folder: hub_dir,
        served: Some(served),
        address,
        pairing: std::cell::Cell::new(None),
    };
    let pinned = Device {
        url: format!("{}/", hub.url()),
        hub: hub_key,
        root: device.public(),
        list: list.clone(),
    };
    std::fs::write(
        pc.folder().join("hub.json"),
        serde_json::to_string(&pinned).unwrap(),
    )
    .unwrap();
    pc.restart();
    pc.wait_until("pc does not connect to its hub", |seen| {
        seen["hub"]["state"] == "connected"
    });

    // The next device pairs in pair mode; the old machine takes the list and links to it.
    let server = Machine::new();
    pair(&hub, &server, "server", "runner");
    assert_eq!(
        pc.wait_for_member("server", true),
        [
            ("pc".to_owned(), "runner".to_owned()),
            ("server".to_owned(), "runner".to_owned())
        ]
    );
    pc.wait_for("server", "online");
    server.wait_for("pc", "online");
    // The old member's own changes still go: renamed from its own device.
    let renamed = pc.user.run(&["remote", "rename", "pc", "desk"]);
    assert!(
        renamed.status.success(),
        "{}",
        String::from_utf8_lossy(&renamed.stderr)
    );
    server.wait_for_member("desk", true);
}

#[test]
fn an_addition_is_announced_as_an_event_on_every_machine() {
    let hub = TestHub::new();
    let (pc, server, laptop) = (Machine::new(), Machine::new(), Machine::new());
    set_up(&hub, &pc);
    pair(&hub, &server, "server", "runner");
    pair(&hub, &laptop, "laptop", "viewer");
    server.wait_for_member("laptop", true);
    for machine in [&pc, &server, &laptop] {
        let added: Vec<Value> = machine
            .events("remote.member_added")
            .into_iter()
            .filter(|event| event["name"] == "laptop")
            .collect();
        assert_eq!(added.len(), 1, "{added:?}\n{}", machine.log());
        assert_eq!(added[0]["role"], "viewer");
        assert_eq!(added[0]["added_by"], "the hub");
        assert!(added[0]["at"].is_u64());
        assert_eq!(
            added[0]["fingerprint"],
            laptop.device().public().fingerprint()
        );
    }
}

#[test]
fn a_removed_device_is_cut_at_once_and_comes_back_only_paired_on_the_hub() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(), Machine::new());
    members(&hub, &pc, &server);
    pc.wait_for("server", "online");
    server.wait_for("pc", "online");

    let removed = pc.user.run(&["remote", "remove", "server"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    // Cut at once, on both sides, without waiting for the link to fall silent.
    let cut = Instant::now();
    pc.wait_until("pc still trusts the server", |seen| {
        !seen["machines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|machine| machine["name"] == "server")
    });
    server.wait_until("the server still says pc is online", |seen| {
        seen["machines"]
            .as_array()
            .unwrap()
            .iter()
            .all(|machine| machine["state"] != "online")
    });
    assert!(
        cut.elapsed() < Duration::from_secs(10),
        "{:?}",
        cut.elapsed()
    );
    let disconnected = pc.events("remote.disconnected");
    assert!(
        disconnected.iter().any(|event| event["name"] == "server"),
        "{disconnected:?}"
    );
    let removal = pc.events("remote.member_removed");
    assert_eq!(removal.len(), 1, "{removal:?}");
    assert_eq!(removal[0]["name"], "server");
    assert_eq!(removal[0]["removed_by"], "pc");

    // Its session to the hub is refused: it is no member.
    server.wait_until("the server's session is not refused", |seen| {
        seen["hub"]["state"] == "refused"
    });
    // Paired again on the hub, with its own key: a member again, and the two link.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    std::fs::remove_file(server.folder().join("hub.json")).unwrap();
    server.user.status();
    pair(&hub, &server, "server", "runner");
    pc.wait_for("server", "online");
}

#[test]
fn the_daemon_never_reads_the_device_key() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(), Machine::new());
    // Each machine's device key file is there, as a client without a Secret Service keeps it;
    // what signs here is the test's, as a client's would, so that any open of the file is the
    // daemon's.
    for machine in [&pc, &server] {
        std::fs::write(machine.folder().join("device.key"), [7u8; 32]).unwrap();
    }
    let watchers: Vec<std::fs::File> = [&pc, &server]
        .iter()
        .map(|machine| watch_opens(&machine.folder()))
        .collect();
    let (pc_device, server_device) = (seeded(41), seeded(42));
    pair_call(&pc, &hub, &pc_device, "pc", false).unwrap();
    pair_call(&server, &hub, &server_device, "server", false).unwrap();
    pc.wait_for_member("server", true);
    let removal = Change::sign(
        serde_json::from_value(pc.user.call("remote.members", json!({})).unwrap()["head"].clone())
            .unwrap(),
        Action::Remove {
            key: server_device.public(),
        },
        &pc_device,
    );
    pc.user
        .call("remote.change", json!({"change": removal}))
        .unwrap();

    for (watcher, machine) in watchers.iter().zip([&pc, &server]) {
        let seen = opened(watcher);
        assert!(
            seen.iter().any(|name| name == "hub.json"),
            "the daemon's own reads were not seen: {seen:?}"
        );
        assert!(
            !seen.iter().any(|name| name == "device.key"),
            "the daemon opened the device key: {seen:?}\n{}",
            machine.log()
        );
    }
}

/// An inotify instance watching the opens and reads of what is in `folder`, by any process.
fn watch_opens(folder: &Path) -> std::fs::File {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: inotify_init1 has no precondition; its result is checked.
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    assert!(fd >= 0, "{}", std::io::Error::last_os_error());
    // SAFETY: `fd` was just returned to this process and is owned by nothing else.
    let watcher = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    let path = std::ffi::CString::new(folder.as_os_str().as_bytes()).unwrap();
    // SAFETY: `path` is a valid C string that outlives the call.
    let watch =
        unsafe { libc::inotify_add_watch(fd, path.as_ptr(), libc::IN_OPEN | libc::IN_ACCESS) };
    assert!(watch >= 0, "{}", std::io::Error::last_os_error());
    watcher
}

/// The names of the files opened or read since the last call.
fn opened(mut watcher: &std::fs::File) -> Vec<String> {
    use std::io::Read;
    let mut names = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = match watcher.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return names,
            Err(error) => panic!("{error}"),
        };
        // Each event: wd (i32), mask, cookie, len (u32), then `len` bytes of name, padded.
        let mut at = 0;
        while at + 16 <= read {
            let len = u32::from_ne_bytes(buffer[at + 12..at + 16].try_into().unwrap()) as usize;
            let name = &buffer[at + 16..at + 16 + len];
            let name = name.split(|byte| *byte == 0).next().unwrap_or_default();
            names.push(String::from_utf8_lossy(name).into_owned());
            at += 16 + len;
        }
    }
}

#[test]
fn a_machine_off_during_a_removal_never_links_to_the_removed_machine() {
    let hub = TestHub::new();
    let (pc, server, laptop) = (Machine::new(), Machine::new(), Machine::new());
    members(&hub, &pc, &server);
    pair(&hub, &laptop, "laptop", "runner");
    server.wait_for("laptop", "online");
    laptop.wait_for("server", "online");
    let linked_to_laptop = |events: Vec<Value>| {
        events
            .iter()
            .filter(|event| event["name"] == "laptop")
            .count()
    };

    // The server is off while the laptop is removed; the laptop, which the hub now refuses, still
    // trusts the server by the list it kept, and dials it.
    let before = linked_to_laptop(server.events("remote.connected"));
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    let removed = pc.user.run(&["remote", "remove", "laptop"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    pc.wait_for_member("laptop", false);

    // On again, with the hub reachable: it hears the removal before it links to anything.
    server.user.status();
    server.wait_for_member("laptop", false);
    server.wait_for("pc", "online");
    assert_eq!(
        linked_to_laptop(server.events("remote.connected")),
        before,
        "the server linked to the removed laptop\n{}",
        server.log()
    );
}

/// A fake hub whose sessions the test cuts, which then sends the list it holds by then; a session
/// of the pc's after its first waits `HELD` before it sends it, as a hub slow to answer a machine
/// that comes back. It pushes nothing else: a list changed meanwhile is sent at the next session.
/// It carries the links' packets between whichever sessions are open, which it says online, as a
/// hub that carries a removed machine's packets would: what keeps such a machine out is the pc's
/// own judgement.
struct CuttingHub {
    runtime: tokio::runtime::Runtime,
    address: SocketAddr,
    key: PublicKey,
    list: std::sync::Arc<std::sync::Mutex<MemberList>>,
    sessions: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// When a session after the first sent its list, the first time.
    sent: std::sync::Arc<std::sync::Mutex<Option<Instant>>>,
    cut: tokio::sync::watch::Sender<usize>,
    _folder: tempfile::TempDir,
}

/// How long a cutting hub's session after the first waits before it sends the list: well within
/// the 10 s a machine holds its links for one.
const HELD: Duration = Duration::from_secs(5);

/// The sessions open on a cutting hub, by their device's key: what is sent to each.
type Open = std::sync::Arc<
    std::sync::Mutex<Vec<(PublicKey, tokio::sync::mpsc::UnboundedSender<ToDevice>)>>,
>;

/// Tells every session open the keys online.
fn tell_online(open: &Open) {
    let open = open.lock().unwrap();
    let keys: Vec<PublicKey> = open.iter().map(|(key, _)| *key).collect();
    for (_, sender) in open.iter() {
        let _ = sender.send(ToDevice::Online(keys.clone()));
    }
}

impl CuttingHub {
    /// A cutting hub that holds `list`, whose sessions of the machine `pc` it counts.
    fn new(list: MemberList, pc: PublicKey) -> CuttingHub {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let folder = tempfile::tempdir().unwrap();
        let key = HubKey::load_or_make(&folder.path().join("hub")).unwrap();
        let public = key.public();
        let list = std::sync::Arc::new(std::sync::Mutex::new(list));
        let sessions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sent = std::sync::Arc::new(std::sync::Mutex::new(None));
        let cut = tokio::sync::watch::Sender::new(0);
        let open = Open::default();
        let (held, counted, cuts, told) =
            (list.clone(), sessions.clone(), cut.clone(), sent.clone());
        let address = runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let key = std::sync::Arc::new(key);
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let (key, held, counted, told, open) = (
                        key.clone(),
                        held.clone(),
                        counted.clone(),
                        told.clone(),
                        open.clone(),
                    );
                    let mut cut = cuts.subscribe();
                    let generation = *cut.borrow_and_update();
                    tokio::spawn(async move {
                        let Ok(arriving) =
                            flottille_link::arrive(Box::new(stream), &key, &Hello::ours("0.4.0"))
                                .await
                        else {
                            return;
                        };
                        let mut session = arriving.welcome().await.unwrap();
                        let device = session.device();
                        let n = if device == pc {
                            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
                        } else {
                            1
                        };
                        let mut relays = session.relays().unwrap();
                        let (sender, mut outgoing) = tokio::sync::mpsc::unbounded_channel();
                        open.lock().unwrap().push((device, sender.clone()));
                        tell_online(&open);
                        // The list, once the device asked something: after `HELD` for a session of
                        // the pc's after its first.
                        let mut list_at: Option<tokio::time::Instant> = None;
                        let mut sent = false;
                        loop {
                            let due = list_at.filter(|_| !sent);
                            tokio::select! {
                                asked = session.ask() => {
                                    let Ok(asked) = asked else { break };
                                    if list_at.is_none() {
                                        let wait = if n > 1 { HELD } else { Duration::ZERO };
                                        list_at = Some(tokio::time::Instant::now() + wait);
                                    }
                                    let answer = ToDevice::Answer { id: asked.id, refused: None };
                                    if sender.send(answer).is_err() {
                                        break;
                                    }
                                }
                                _ = async { tokio::time::sleep_until(due.unwrap()).await }, if due.is_some() => {
                                    sent = true;
                                    let list = held.lock().unwrap().clone();
                                    if sender.send(ToDevice::List(list)).is_err() {
                                        break;
                                    }
                                    if n > 1 {
                                        told.lock().unwrap().get_or_insert_with(Instant::now);
                                    }
                                }
                                frame = outgoing.recv() => {
                                    let Some(frame) = frame else { break };
                                    if session.send(&frame).await.is_err() {
                                        break;
                                    }
                                }
                                (to, packet) = relays.next() => {
                                    let open = open.lock().unwrap();
                                    if let Some((_, to)) = open.iter().find(|(key, _)| *key == to) {
                                        let _ = to.send(ToDevice::Relayed {
                                            from: device,
                                            packet: flottille_link::LinkPacket(packet),
                                        });
                                    }
                                }
                                _ = async { let _ = cut.wait_for(|now| *now > generation).await; } => break,
                            }
                        }
                        open.lock()
                            .unwrap()
                            .retain(|(_, open)| !open.same_channel(&sender));
                        tell_online(&open);
                    });
                }
            });
            address
        });
        CuttingHub {
            runtime,
            address,
            key: public,
            list,
            sessions,
            sent,
            cut,
            _folder: folder,
        }
    }

    fn sessions(&self) -> usize {
        self.sessions.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Waits until the hub had `n` sessions.
    fn wait_for_sessions(&self, n: usize, pc: &Machine) {
        let deadline = Instant::now() + WAIT;
        while self.sessions() < n {
            assert!(Instant::now() < deadline, "no session {n}\n{}", pc.log());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn sent(&self) -> Option<Instant> {
        *self.sent.lock().unwrap()
    }
}

/// The pc's list through a cutting hub: its device and machine, and the laptop's, a member whose
/// machine's key is `laptop`.
struct Trusting {
    hub: CuttingHub,
    list: MemberList,
    device: DeviceKey,
    laptop: DeviceKey,
}

impl Trusting {
    /// `pc` pinned to a cutting hub whose list holds the laptop's machine `laptop`, once the pc
    /// trusts it.
    fn new(pc: &Machine, laptop: &MachineKey) -> Trusting {
        let (device, laptop_device) = (pc.device(), seeded(31));
        let mut list = MemberList::start("pc", &device).unwrap();
        list.declare(&device, &pc.machine_key());
        list.sign(
            Action::Add {
                key: laptop_device.public(),
                name: "laptop".to_owned(),
                role: Role::Runner,
            },
            &device,
        );
        list.declare(&laptop_device, laptop);
        let hub = CuttingHub::new(list.clone(), pc.machine_key().public());
        let pinned = Device {
            url: format!("http://{}", hub.address),
            hub: hub.key,
            root: device.public(),
            list: MemberList::default(),
        };
        std::fs::write(
            pc.folder().join("hub.json"),
            serde_json::to_string(&pinned).unwrap(),
        )
        .unwrap();
        pc.restart();
        pc.wait_until("the pc does not trust the laptop of its list", |seen| {
            trusts_laptop(seen)
        });
        Trusting {
            hub,
            list,
            device,
            laptop: laptop_device,
        }
    }

    /// The laptop removed from the hub's list, which the hub sends at its next session.
    fn remove_laptop(&mut self) {
        self.list.sign(
            Action::Remove {
                key: self.laptop.public(),
            },
            &self.device,
        );
        *self.hub.list.lock().unwrap() = self.list.clone();
    }

    /// The pc's session cut, and the next one started, its list held back.
    fn cut(&self, pc: &Machine) {
        self.hub.cut.send_modify(|cut| *cut += 1);
        self.hub.wait_for_sessions(2, pc);
    }
}

fn trusts_laptop(seen: &Value) -> bool {
    seen["machines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|machine| machine["name"] == "laptop")
}

/// A machine key of a machine that runs no daemon: the test dials with it.
fn machine_key_apart() -> (tempfile::TempDir, MachineKey) {
    let dir = tempfile::tempdir().unwrap();
    let key = MachineKey::load_or_make(&dir.path().join("remote")).unwrap();
    (dir, key)
}

impl Trusting {
    /// The endpoint of the machine whose key is `key`, through a session of its own to the
    /// cutting hub, in the hub's runtime.
    fn endpoint_of(&self, key: &MachineKey) -> Endpoint {
        let pinned = Device {
            url: format!("http://{}", self.hub.address),
            hub: self.hub.key,
            root: self.device.public(),
            list: MemberList::default(),
        };
        self.hub.runtime.block_on(async {
            let hello = Hello::ours(env!("CARGO_PKG_VERSION"));
            let mut session = pinned.connect(key, &hello, &Tls::System).await.unwrap();
            let endpoint = Endpoint::new(key).unwrap();
            endpoint.through(session.relay().unwrap());
            tokio::spawn(async move { while session.next().await.is_some() {} });
            endpoint
        })
    }

    /// Dials `pc` through the cutting hub as the machine whose key is `key`, until it links or is
    /// refused: whether a link was made, or why not.
    fn dial_as(&self, key: &MachineKey, pc: PublicKey) -> Result<(), String> {
        let endpoint = self.endpoint_of(key);
        self.hub.runtime.block_on(async {
            let deadline = Instant::now() + WAIT;
            let made = loop {
                match crate::remote::dial(&endpoint, pc, Traffic::Machine, "pc").await {
                    Err(error)
                        if !matches!(error, LinkError::Refused { .. })
                            && Instant::now() < deadline =>
                    {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    made => break made,
                }
            };
            endpoint.close().await;
            made.map(|_| ()).map_err(|error| error.to_string())
        })
    }

    /// The machine whose key is `key` on the cutting hub, taking every link that comes to it: how
    /// many it took.
    fn take_links(&self, key: &MachineKey) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
        let endpoint = self.endpoint_of(key);
        let taken = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = taken.clone();
        self.hub.runtime.spawn(async move {
            while let Some(accepting) = endpoint.accept().await {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = accepting
                    .handshake(&Hello::ours(env!("CARGO_PKG_VERSION")), "pc")
                    .await;
            }
        });
        taken
    }
}

#[test]
fn a_link_while_the_session_is_lost_is_judged_by_the_next_list() {
    let pc = Machine::new();
    let (_laptop_dir, laptop) = machine_key_apart();
    let mut trusting = Trusting::new(&pc, &laptop);

    // The laptop is removed while the pc's session is cut: the list the pc kept still trusts it.
    trusting.remove_laptop();
    trusting.cut(&pc);

    // Meanwhile, the laptop dials the pc, which waits for the next session's list to judge it.
    assert!(
        trusting.hub.sent().is_none(),
        "the hub sent the next list before the laptop dialled: nothing was held\n{}",
        pc.log()
    );
    let dialled = trusting.dial_as(&laptop, pc.key());
    let ended = Instant::now();
    let error = dialled.expect_err("the pc linked to the removed laptop by the list it kept");
    assert!(error.contains("not one of the user's"), "{error}");
    let sent = trusting
        .hub
        .sent()
        .expect("the pc refused the laptop before the hub sent the next list");
    assert!(
        ended >= sent,
        "the pc refused the laptop before the next list came\n{}",
        pc.log()
    );
    let linked = pc.events("remote.connected");
    assert!(
        linked.iter().all(|event| event["name"] != "laptop"),
        "the pc linked to the removed laptop by the list it kept: {linked:#?}\n{}",
        pc.log()
    );
    drop(trusting.hub.runtime);
}

#[test]
fn a_machine_whose_session_is_lost_dials_no_one_until_the_next_list() {
    let pc = Machine::new();
    let (_laptop_dir, laptop) = machine_key_apart();
    let mut trusting = Trusting::new(&pc, &laptop);

    // While the next list is held back, the laptop comes online on the hub: the list the pc kept
    // still trusts it. It takes every link that comes; it never dials the pc itself.
    trusting.remove_laptop();
    trusting.cut(&pc);
    let taken = trusting.take_links(&laptop);
    let deadline = Instant::now() + WAIT;
    while trusting.hub.sent().is_none() {
        assert!(Instant::now() < deadline, "no next list\n{}", pc.log());
        std::thread::sleep(Duration::from_millis(100));
    }
    pc.wait_until("the pc still trusts the removed laptop", |seen| {
        !trusts_laptop(seen)
    });
    // A dial of the next round, if one was due, has had its time.
    std::thread::sleep(Duration::from_secs(2));

    let linked = pc.events("remote.connected");
    assert!(
        linked.iter().all(|event| event["name"] != "laptop"),
        "the pc dialled the removed laptop by the list it kept: {linked:#?}\n{}",
        pc.log()
    );
    assert_eq!(
        taken.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the pc dialled the removed laptop by the list it kept\n{}",
        pc.log()
    );
    drop(trusting.hub.runtime);
}

#[test]
fn a_machine_that_wakes_from_a_suspend_judges_links_by_the_next_list() {
    let user = User::new();
    let ahead = user.path("wall-ahead");
    let pc = Machine::of(user.with_env("FLOTTILLE_TEST_WALL_AHEAD_FILE", ahead.to_str().unwrap()));
    let (_laptop_dir, laptop) = machine_key_apart();
    let mut trusting = Trusting::new(&pc, &laptop);

    // The laptop is removed while the pc sleeps: the hub's session looks alive on its waking, and
    // the list it kept still trusts the laptop, which dials the pc in the first second.
    trusting.remove_laptop();
    std::fs::write(&ahead, "60").unwrap();
    let dialled = trusting.dial_as(&laptop, pc.key());

    let error = dialled.expect_err("the pc linked to the removed laptop by the list it kept");
    assert!(error.contains("not one of the user's"), "{error}");
    assert!(
        trusting.hub.sessions() >= 2,
        "the pc kept the session it had before it slept\n{}",
        pc.log()
    );
    let linked = pc.events("remote.connected");
    assert!(
        linked.iter().all(|event| event["name"] != "laptop"),
        "the pc linked to the removed laptop by the list it kept: {linked:#?}\n{}",
        pc.log()
    );
    drop(trusting.hub.runtime);
}

#[test]
fn the_hubs_address_is_shown_without_its_default_port_or_a_trailing_slash() {
    let hub = TestHub::new();
    let pc = Machine::new();
    // A code whose address has a trailing slash: it is not shown with one, and the port the hub is
    // on, which is not its scheme's own, stays. The default ports are the unit test's of
    // `HubUrl::shown`.
    let typed = format!("{}/", hub.url());
    let (_, code) = hub
        .served()
        .with_hub(|served| served.open_pairing(&typed, None).unwrap());
    let output = pc.user.run_with_input(
        &["remote", "pair", "--name", "pc", "--role", "runner"],
        &format!("{}\n", code.code().unwrap()),
    );
    assert!(output.status.success(), "{}", pc.log());
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(said.contains(&format!("hub at {} as", hub.url())), "{said}");
    assert!(!said.contains(&typed), "{said}");

    let seen = pc.remote();
    assert_eq!(seen["hub"]["url"], hub.url());
    let members = pc.user.call("remote.members", json!({})).unwrap();
    assert_eq!(members["hub"], hub.url());
    // A second pairing without --replace is refused, naming the hub as it is shown. The refusal
    // comes before the code is used, so any hub's code will do.
    let other = TestHub::new();
    let (_, refused) = pair_call(&pc, &other, &seeded(43), "pc", false).unwrap_err();
    assert!(
        refused.contains(&format!("hub at {} already", hub.url())),
        "{refused}"
    );
}
