//! Machines that know each other through the hub (`docs/remote.md`, "The hub", "The first
//! member", "Pairing", "Architecture", "The transport"): a hub served in the test on 127.0.0.1,
//! and daemons, each with its own temporary HOME and XDG folders and in-memory keyring. The first
//! machine is set up with `flottille remote setup`; the others join it with `flottille remote
//! invite`, `join` and `accept`, as the user pairs them.

mod common;

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::User;
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Action, Change, ChangeHash, Device, DeviceKey, Hello, HubKey, InvitationAsk, Keyring,
    KeyringError, MachineKey, MemberList, PublicKey, Requested, Role, ToDevice, Token,
    verification_words,
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

    /// The first join request that waits for this machine's member, once the hub told it.
    fn wait_until_requested(&self) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let requests = self.user.call("remote.requests", json!({})).unwrap();
            if let Some(first) = requests.as_array().and_then(|all| all.first()) {
                return first.clone();
            }
            assert!(
                Instant::now() < deadline,
                "no join request came\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
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

/// `pc` set up as the hub's first member with `flottille remote setup`.
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
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(said.contains("the first member"), "{said}");
}

/// What a pairing said on both sides.
struct Paired {
    /// What `flottille remote join` printed.
    joined: String,
    /// What `flottille remote accept` printed.
    accepted: String,
    /// The four words of the request, as `remote.requests` on the inviter said them.
    words: Vec<String>,
    /// The new device's key, as the request carried it.
    key: PublicKey,
}

/// `joiner` joins the hub of `inviter` as the user pairs them: `flottille remote invite` on the
/// inviter, `flottille remote join` given the code on the new machine, which waits, and `flottille remote
/// accept` on the inviter once its request is there, as `name` with `role`.
fn pair(inviter: &Machine, joiner: &Machine, name: &str, role: &str) -> Paired {
    let invited = inviter.user.run(&["remote", "invite", "--json"]);
    assert!(
        invited.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&invited.stderr),
        inviter.log()
    );
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
    {
        let mut input = joining.stdin.take().unwrap();
        std::io::Write::write_all(&mut input, format!("{code}\n").as_bytes()).unwrap();
    }
    let mut out = BufReader::new(joining.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    let request = inviter.wait_until_requested();
    let words: Vec<String> = request["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|word| word.as_str().unwrap().to_owned())
        .collect();
    let key = serde_json::from_value(request["key"].clone()).unwrap();
    let id = request["request"].as_str().unwrap();
    let accepted = inviter
        .user
        .run(&["remote", "accept", id, "--name", name, "--role", role]);
    assert!(
        accepted.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&accepted.stderr),
        inviter.log()
    );
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    let ended = joining.wait_with_output().unwrap();
    assert!(
        ended.status.success(),
        "the join failed: {}\n{}",
        String::from_utf8_lossy(&ended.stderr),
        joiner.log()
    );
    Paired {
        joined: first + &rest,
        accepted: String::from_utf8_lossy(&accepted.stdout).into_owned(),
        words,
        key,
    }
}

/// `pc` set up as the hub's first member, then `server` paired with it, a runner; each machine's
/// addresses told to the hub by its daemon.
fn members(hub: &TestHub, pc: &Machine, server: &Machine) {
    set_up(hub, pc);
    pair(pc, server, "server", "runner");
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
        FakeHub::pushing(list, answer, |_| Vec::new())
    }

    /// A fake hub that answers an invitation's ask with a token of its own, `ISSUED`, then
    /// pushes the join requests `requests` makes of it.
    fn pushing(
        list: MemberList,
        answer: fn(usize) -> Option<Option<String>>,
        requests: fn(Token) -> Vec<Requested>,
    ) -> FakeHub {
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
                            let invited = matches!(asked.ask, flottille_link::Ask::Invite { .. });
                            if let Some(refused) = answer {
                                let answer = ToDevice::Answer {
                                    id: asked.id,
                                    refused,
                                    proof: None,
                                    token: invited.then_some(ISSUED),
                                };
                                if session.send(&answer).await.is_err() {
                                    return;
                                }
                            }
                            if invited {
                                let pushed = ToDevice::Requests(requests(ISSUED));
                                if session.send(&pushed).await.is_err() {
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

/// The token a fake hub gives for an invitation.
const ISSUED: Token = Token([7; 16]);

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

/// The join requests a hub that lies pushes once the pc asked for an invitation: one for the
/// token it issued, from a tablet, and one for a token it never issued, from a phone, which it
/// searched so that the pc's words would match the phone's.
fn lying_requests(issued: Token) -> Vec<Requested> {
    vec![
        Requested {
            token: issued,
            key: seeded(41).public(),
        },
        Requested {
            token: Token([9; 16]),
            key: seeded(42).public(),
        },
    ]
}

/// `pc`, pinned to a hub that pushes `lying_requests`, asks for an invitation; the requests it
/// shows then.
fn invited_by_a_lying_hub(pc: &Machine) -> (FakeHub, Vec<Value>) {
    let hub = FakeHub::pushing(list_for(pc), |_| Some(None), lying_requests);
    pin(pc, &hub);
    pc.wait_until("the pc does not take the hub's list", |seen| {
        seen["machines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|machine| machine["name"] == "laptop")
    });
    let invited = pc.user.run(&["remote", "invite", "--json"]);
    assert!(
        invited.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&invited.stderr),
        pc.log()
    );
    pc.wait_until_requested();
    // Both were pushed at once: what the first one shows, the second would by now.
    let requests = pc.user.call("remote.requests", json!({})).unwrap();
    (hub, requests.as_array().unwrap().clone())
}

#[test]
fn a_request_for_a_token_this_member_never_issued_is_dropped() {
    let pc = Machine::new(None);
    let (hub, requests) = invited_by_a_lying_hub(&pc);
    let phone = seeded(42).public();
    assert_eq!(requests.len(), 1, "{requests:#?}");
    assert_eq!(requests[0]["key"], json!(seeded(41).public()));
    let told = pc.events("remote.join_requested");
    assert!(
        told.iter().all(|event| event["key"] != json!(phone)),
        "{told:#?}"
    );
    assert!(
        pc.user
            .run(&["remote", "refuse", &phone.fingerprint()])
            .status
            .code()
            != Some(0)
    );
    drop(hub.runtime);
}

#[test]
fn a_request_for_an_expired_invitation_is_dropped() {
    let pc = Machine::new(None);
    let hub = FakeHub::pushing(list_for(&pc), |_| Some(None), lying_requests);
    pin(&pc, &hub);
    pc.wait_until("the pc does not take the hub's list", |seen| {
        seen["machines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|machine| machine["name"] == "laptop")
    });
    // An invitation asked 270 s ago: valid 30 s more, room enough to see its request first.
    let ask = InvitationAsk::sign(&hub.key, unix_now() - 270, &pc.device()).unwrap();
    pc.user.call("remote.invite", json!({"ask": ask})).unwrap();
    let request = pc.wait_until_requested();
    assert_eq!(request["key"], json!(seeded(41).public()));

    // Past its 5 minutes, the request for it is no longer shown, though the hub still tells it.
    let deadline = Instant::now() + Duration::from_secs(60);
    while pc.user.call("remote.requests", json!({})).unwrap() != json!([]) {
        assert!(
            Instant::now() < deadline,
            "the expired request is still shown"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    drop(hub.runtime);
}

#[test]
fn the_inviters_words_come_from_the_token_it_issued() {
    let pc = Machine::new(None);
    let (hub, requests) = invited_by_a_lying_hub(&pc);
    let tablet = seeded(41).public();
    let words = verification_words(&tablet, &pc.device().public(), &ISSUED);
    assert_eq!(requests[0]["words"], json!(words), "{requests:#?}");
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

#[test]
fn the_invitation_code_is_read_from_the_standard_input_never_from_the_command_line() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(None), Machine::new(None));
    set_up(&hub, &pc);
    // The invitation prints its code on a line of its own, never in a command to copy.
    let invited = pc.user.run(&["remote", "invite"]);
    assert!(invited.status.success());
    let said = String::from_utf8_lossy(&invited.stdout);
    let code = said
        .lines()
        .map(str::trim)
        .find(|line| line.len() > 100 && !line.contains(' '))
        .unwrap_or_else(|| panic!("no code on a line of its own: {said}"))
        .to_owned();
    assert!(
        said.lines()
            .filter(|line| line.contains("flottille remote join"))
            .all(|line| !line.contains(&code)),
        "{said}"
    );
    let output = server.user.run(&["remote", "join", &code]);
    assert!(
        !output.status.success(),
        "an invitation code was taken from the command line"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unexpected argument"),
        "the refusal does not say the argument is not one"
    );
    // From the standard input, the join goes as far as the hub, where it waits for pc.
    let mut joining = server
        .user
        .command(&["remote", "join"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut input = joining.stdin.take().unwrap();
        std::io::Write::write_all(&mut input, format!("{code}\n").as_bytes()).unwrap();
    }
    let mut first = String::new();
    BufReader::new(joining.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    assert!(first.starts_with("Verification words: "), "{first}");
    pc.wait_until_requested();
    let _ = joining.kill();
    let _ = joining.wait();
}

/// A port nobody uses now on loopback: the system's choice, let go at once.
fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn a_headless_machine_joins_with_the_code_and_is_in_every_list() {
    let hub = TestHub::new();
    let (pc, server) = (
        Machine::new(Some("127.0.0.1:0")),
        Machine::new(Some("127.0.0.1:0")),
    );
    set_up(&hub, &pc);
    let paired = pair(&pc, &server, "server", "runner");
    assert!(
        paired.joined.contains("server joined the hub at"),
        "{}",
        paired.joined
    );
    assert!(
        paired.joined.contains("as a runner, added by pc"),
        "{}",
        paired.joined
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
fn a_second_join_from_the_same_machine_uses_a_new_device_key() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(None), Machine::new(None));
    set_up(&hub, &pc);

    // A first join that does not succeed: refused here, as a hub that dropped it would leave it.
    let invited = pc.user.run(&["remote", "invite", "--json"]);
    assert!(invited.status.success());
    let invited: Value = serde_json::from_slice(&invited.stdout).unwrap();
    let joining = server
        .user
        .command(&["remote", "join"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut input = joining.stdin.as_ref().unwrap();
        std::io::Write::write_all(
            &mut input,
            format!("{}\n", invited["code"].as_str().unwrap()).as_bytes(),
        )
        .unwrap();
    }
    let request = pc.wait_until_requested();
    let first: PublicKey = serde_json::from_value(request["key"].clone()).unwrap();
    let refused = pc
        .user
        .run(&["remote", "refuse", request["request"].as_str().unwrap()]);
    assert!(
        refused.status.success(),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let ended = joining.wait_with_output().unwrap();
    assert!(!ended.status.success());
    // The key of a join that failed is not kept.
    assert!(!server.folder().join("device.key").exists());
    let deadline = Instant::now() + WAIT;
    while pc.user.call("remote.requests", json!({})).unwrap() != json!([]) {
        assert!(Instant::now() < deadline, "the refused request stays");
        std::thread::sleep(Duration::from_millis(200));
    }

    // The next join sends a key the hub never saw, and that one is kept.
    let paired = pair(&pc, &server, "server", "runner");
    assert_ne!(paired.key, first);
    assert_eq!(server.device().public(), paired.key);
}

#[test]
fn both_sides_show_the_same_four_words() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(None), Machine::new(None));
    set_up(&hub, &pc);
    let paired = pair(&pc, &server, "server", "controller");
    let words = paired.words.join(" ");
    assert_eq!(paired.words.len(), 4);
    assert!(
        paired
            .joined
            .starts_with(&format!("Verification words: {words}\n")),
        "{}",
        paired.joined
    );
    assert!(
        paired
            .accepted
            .starts_with(&format!("Verification words: {words}\n")),
        "{}",
        paired.accepted
    );
}

#[test]
fn an_addition_is_announced_as_an_event_on_every_machine() {
    let hub = TestHub::new();
    let (pc, server, laptop) = (Machine::new(None), Machine::new(None), Machine::new(None));
    set_up(&hub, &pc);
    pair(&pc, &server, "server", "runner");
    pair(&pc, &laptop, "laptop", "viewer");
    server.wait_for_member("laptop", true);
    for machine in [&pc, &server, &laptop] {
        let added: Vec<Value> = machine
            .events("remote.member_added")
            .into_iter()
            .filter(|event| event["name"] == "laptop")
            .collect();
        assert_eq!(added.len(), 1, "{added:?}\n{}", machine.log());
        assert_eq!(added[0]["role"], "viewer");
        assert_eq!(added[0]["added_by"], "pc");
        assert!(added[0]["at"].is_u64());
        assert_eq!(
            added[0]["fingerprint"],
            laptop.device().public().fingerprint()
        );
    }
    // The inviter was told of the request first.
    let requested = pc.events("remote.join_requested");
    assert_eq!(requested.len(), 2, "{requested:?}");
}

#[test]
fn a_removed_device_is_cut_at_once_and_cannot_join_again() {
    let hub = TestHub::new();
    let (pc, server) = (
        Machine::new(Some("127.0.0.1:0")),
        Machine::new(Some("127.0.0.1:0")),
    );
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

    // Its key cannot join again: the hub refuses the request.
    let invited = pc.user.run(&["remote", "invite", "--json"]);
    assert!(invited.status.success());
    let invited: Value = serde_json::from_slice(&invited.stdout).unwrap();
    std::fs::remove_file(server.folder().join("hub.json")).unwrap();
    let again = server.user.run_with_input(
        &["remote", "join"],
        &format!("{}\n", invited["code"].as_str().unwrap()),
    );
    assert!(!again.status.success());
    let said = String::from_utf8_lossy(&again.stderr);
    assert!(said.contains("was removed from the member list"), "{said}");
}

#[test]
fn the_daemon_never_reads_the_device_key() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(None), Machine::new(None));
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

    let mut list = MemberList::start("pc", &pc_device).unwrap();
    list.sign(
        Action::Machine {
            key: pc_device.public(),
            machine: pc.key(),
        },
        &pc_device,
    );
    pc.user
        .call(
            "remote.setup",
            json!({"url": hub.url(), "code": hub.code(), "changes": list.changes()}),
        )
        .unwrap();
    let ask = InvitationAsk::sign(&hub.key(), unix_now(), &pc_device).unwrap();
    let invited = pc.user.call("remote.invite", json!({"ask": ask})).unwrap();
    let code = invited["code"].as_str().unwrap().to_owned();
    std::thread::scope(|scope| {
        let joining = scope.spawn(|| {
            server.user.call_within(
                "remote.join",
                json!({"code": code, "key": server_device.public()}),
                Duration::from_secs(60),
            )
        });
        let request = pc.wait_until_requested();
        let members = pc.user.call("remote.members", json!({})).unwrap();
        let head: ChangeHash = serde_json::from_value(members["head"].clone()).unwrap();
        let added = Change::sign(
            head,
            Action::Add {
                key: serde_json::from_value(request["key"].clone()).unwrap(),
                name: "server".to_owned(),
                role: Role::Runner,
            },
            &pc_device,
        );
        pc.user
            .call("remote.change", json!({"change": added}))
            .unwrap();
        let accepted = joining.join().unwrap().unwrap();
        let head: ChangeHash = serde_json::from_value(accepted["head"].clone()).unwrap();
        let declared = Change::sign(
            head,
            Action::Machine {
                key: server_device.public(),
                machine: server.key(),
            },
            &server_device,
        );
        server
            .user
            .call("remote.declare", json!({"change": declared}))
            .unwrap();
    });
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

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
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
    let (pc, server, laptop) = (
        Machine::new(Some("127.0.0.1:0")),
        Machine::new(Some("127.0.0.1:0")),
        Machine::new(Some("127.0.0.1:0")),
    );
    members(&hub, &pc, &server);
    pair(&pc, &laptop, "laptop", "runner");
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
/// after the first waits `HELD` before it sends it, as a hub slow to answer a machine that comes
/// back.
struct CuttingHub {
    runtime: tokio::runtime::Runtime,
    address: SocketAddr,
    key: PublicKey,
    list: std::sync::Arc<std::sync::Mutex<MemberList>>,
    sessions: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    cut: tokio::sync::watch::Sender<usize>,
    _folder: tempfile::TempDir,
}

/// How long a cutting hub's session after the first waits before it sends the list: well within
/// the 10 s a machine holds its links for one.
const HELD: Duration = Duration::from_secs(5);

impl CuttingHub {
    fn new(list: MemberList) -> CuttingHub {
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
        let cut = tokio::sync::watch::Sender::new(0);
        let (held, counted, cuts) = (list.clone(), sessions.clone(), cut.clone());
        let address = runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let key = std::sync::Arc::new(key);
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let (key, held, counted) = (key.clone(), held.clone(), counted.clone());
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
                        let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        let mut sent = false;
                        loop {
                            let asked = tokio::select! {
                                asked = session.ask() => asked,
                                _ = cut.wait_for(|now| *now > generation) => return,
                            };
                            let Ok(asked) = asked else {
                                return;
                            };
                            if !sent {
                                sent = true;
                                if n > 1 {
                                    tokio::time::sleep(HELD).await;
                                }
                                let list = held.lock().unwrap().clone();
                                if session.send(&ToDevice::List(list)).await.is_err() {
                                    return;
                                }
                            }
                            let answer = ToDevice::Answer {
                                id: asked.id,
                                refused: None,
                                proof: None,
                                token: None,
                            };
                            if session.send(&answer).await.is_err() {
                                return;
                            }
                        }
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
            cut,
            _folder: folder,
        }
    }

    fn sessions(&self) -> usize {
        self.sessions.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[test]
fn a_link_while_the_session_is_lost_is_judged_by_the_next_list() {
    let (pc, laptop) = (
        Machine::new(Some("127.0.0.1:0")),
        Machine::new(Some("127.0.0.1:0")),
    );
    // The pc's list: its device and machine, and the laptop's, a member.
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
    list.declare(&laptop_device, &laptop.machine_key());
    let hub = CuttingHub::new(list.clone());
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
        seen["machines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|machine| machine["name"] == "laptop")
    });

    // The laptop is removed while the pc's session is cut: the list the pc kept still trusts it.
    list.sign(
        Action::Remove {
            key: laptop_device.public(),
        },
        &device,
    );
    *hub.list.lock().unwrap() = list;
    hub.cut.send_modify(|cut| *cut += 1);
    let deadline = Instant::now() + WAIT;
    while hub.sessions() < 2 {
        assert!(Instant::now() < deadline, "no new session\n{}", pc.log());
        std::thread::sleep(Duration::from_millis(50));
    }

    // Meanwhile, the laptop dials the pc, which waits for the next session's list to judge it.
    let at = pc.remote()["this"]["addresses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|address| address["address"].as_str().unwrap().to_owned())
        .find(|address| address.starts_with("127.0.0.1:"))
        .unwrap();
    let peers = json!({"machines": [{"name": "pc", "key": pc.key(), "addresses": [at]}]});
    std::fs::write(laptop.folder().join("peers.json"), peers.to_string()).unwrap();
    pc.wait_until("the pc still trusts the removed laptop", |seen| {
        !seen["machines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|machine| machine["name"] == "laptop")
    });
    let fingerprint = laptop.key().fingerprint();
    let refused = format!("refused a link from {fingerprint}");
    let deadline = Instant::now() + WAIT;
    while !pc.log().contains(&refused) {
        assert!(
            Instant::now() < deadline,
            "the laptop never dialled\n{}",
            pc.log()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let linked = pc.events("remote.connected");
    assert!(
        linked.iter().all(|event| event["name"] != "laptop"),
        "the pc linked to the removed laptop by the list it kept: {linked:#?}\n{}",
        pc.log()
    );
    drop(hub.runtime);
}

/// `flottille remote join` on `joiner`, given `code`, left running: it waits for the member that
/// invited.
fn join_in_background(joiner: &Machine, code: &str) -> std::process::Child {
    let joining = joiner
        .user
        .command(&["remote", "join"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = joining.stdin.as_ref().unwrap();
    std::io::Write::write_all(&mut input, format!("{code}\n").as_bytes()).unwrap();
    joining
}

/// An invitation's code, from `flottille remote invite` on `inviter`.
fn invitation(inviter: &Machine) -> String {
    let invited = inviter.user.run(&["remote", "invite", "--json"]);
    assert!(
        invited.status.success(),
        "{}",
        String::from_utf8_lossy(&invited.stderr)
    );
    let invited: Value = serde_json::from_slice(&invited.stdout).unwrap();
    invited["code"].as_str().unwrap().to_owned()
}

#[test]
fn a_join_whose_command_was_gone_keeps_its_key_on_the_next_try() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(None), Machine::new(None));
    set_up(&hub, &pc);

    // The command that joins is gone (its terminal closed) while it waits; the member accepts the
    // request all the same, and the server's daemon takes the answer.
    let code = invitation(&pc);
    let mut joining = join_in_background(&server, &code);
    let request = pc.wait_until_requested();
    let key: PublicKey = serde_json::from_value(request["key"].clone()).unwrap();
    joining.kill().unwrap();
    joining.wait().unwrap();
    let accepted = pc.user.run(&[
        "remote",
        "accept",
        request["request"].as_str().unwrap(),
        "--name",
        "server",
        "--role",
        "runner",
    ]);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );

    // Joining again goes on with the key the hub took, and keeps it: the member has a key that
    // signs for it.
    let again = server
        .user
        .run_with_input(&["remote", "join"], &format!("{code}\n"));
    assert!(
        again.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&again.stderr),
        server.log()
    );
    assert_eq!(server.device().public(), key);
    pc.wait_for_member("server", true);
    let this = server.user.call("remote.members", json!({})).unwrap()["this"].clone();
    assert_eq!(this, "server");
}

#[test]
fn a_pending_key_from_a_refused_join_is_never_sent_again() {
    let hub = TestHub::new();
    let (pc, server) = (Machine::new(None), Machine::new(None));
    set_up(&hub, &pc);

    // The command that joins is gone while it waits, and the member refuses the request.
    let mut joining = join_in_background(&server, &invitation(&pc));
    let request = pc.wait_until_requested();
    let first: PublicKey = serde_json::from_value(request["key"].clone()).unwrap();
    joining.kill().unwrap();
    joining.wait().unwrap();
    let refused = pc
        .user
        .run(&["remote", "refuse", request["request"].as_str().unwrap()]);
    assert!(
        refused.status.success(),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let deadline = Instant::now() + WAIT;
    while pc.user.call("remote.requests", json!({})).unwrap() != json!([]) {
        assert!(Instant::now() < deadline, "the refused request stays");
        std::thread::sleep(Duration::from_millis(200));
    }

    // The next join sends a key the hub never saw, and keeps that one.
    let paired = pair(&pc, &server, "server", "runner");
    assert_ne!(paired.key, first);
    assert_eq!(server.device().public(), paired.key);
}
