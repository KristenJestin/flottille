//! Two machines of the user's and their hub, for the tests of remote (`docs/remote.md`): a hub
//! served in the test on 127.0.0.1 and daemons, each with its own temporary HOME and XDG folders
//! and no keyring of the user's, each paired as the user pairs it: pair mode opened on the hub,
//! its code given to `flottille remote pair`.

// Each test file uses its own part of it.
#![allow(dead_code)]

use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::process::Output;
use std::time::{Duration, Instant, SystemTime};

use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    Device, DeviceKey, Endpoint, Hello, Keyring, KeyringError, Link, LinkError, MachineKey,
    PublicKey, Request, Tls, Traffic,
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::common::{self, User};

/// How long a link, or the hub's news, may take: the daemons try again every second.
pub const WAIT: Duration = Duration::from_secs(40);

/// A keyring that is not there: the device key is the file a client keeps in the key folder.
pub struct NoKeyring;

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
pub struct TestHub {
    /// Serves the hub while the test runs.
    _runtime: tokio::runtime::Runtime,
    _folder: tempfile::TempDir,
    served: Option<Served<SystemClock>>,
    address: SocketAddr,
}

impl TestHub {
    pub fn new() -> TestHub {
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
            _runtime: runtime,
            _folder: folder,
            served: Some(served),
            address,
        }
    }

    /// Stops the hub: every session to it ends.
    pub fn stop(&mut self) {
        let served = self.served.take().unwrap();
        self._runtime.block_on(served.stop());
    }

    /// Serves the hub again from its state folder, at the address it had.
    pub fn start_again(&mut self) {
        let (folder, address) = (self._folder.path().join("hub"), self.address);
        let served = self._runtime.block_on(async {
            let hub = Hub::open(&folder, SystemClock).unwrap();
            let listener = tokio::net::TcpListener::bind(address).await.unwrap();
            flottille_hub::serve(hub, listener, None, Hello::ours("0.4.0")).unwrap()
        });
        self.served = Some(served);
    }

    /// The hub's state file, `hub.json`, as it writes it.
    pub fn state_file(&self) -> std::path::PathBuf {
        self._folder.path().join("hub/hub.json")
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Pair mode opened, as `flottille-hub pair` opens it on the hub's host: its code.
    pub fn code(&self) -> String {
        let url = self.url();
        self.served
            .as_ref()
            .unwrap()
            .with_hub(|hub| hub.open_pairing(&url, None).unwrap().1.code().unwrap())
    }

    /// Does `act` with the hub, as its host's commands do.
    pub fn with_hub<R>(&self, act: impl FnOnce(&mut Hub<SystemClock>) -> R) -> R {
        self.served.as_ref().unwrap().with_hub(act)
    }
}

pub struct Machine {
    pub user: User,
}

impl Machine {
    /// `user`'s machine, with remote on: its links go through the hub.
    pub fn of(user: User) -> Machine {
        std::fs::write(
            user.path("config/flottille/daemon.toml"),
            "[remote]\non = true\n",
        )
        .unwrap();
        user.status();
        Machine { user }
    }

    pub fn new() -> Machine {
        Machine::of(User::new())
    }

    pub fn remote(&self) -> Value {
        self.user.call("remote.machines", json!({})).unwrap()
    }

    pub fn folder(&self) -> std::path::PathBuf {
        self.user.path("state/flottille/remote")
    }

    /// The device key this machine's clients hold, read as a client reads it.
    pub fn device(&self) -> DeviceKey {
        DeviceKey::load(&NoKeyring, &self.folder())
            .unwrap()
            .expect("a member's device key")
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.user.path("state/flottille/daemon.log")).unwrap_or_default()
    }

    /// Waits until `check` holds of what `remote.machines` says.
    pub fn wait_until(&self, what: &str, check: impl Fn(&Value) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            let seen = self.remote();
            if check(&seen) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{what}: {seen:#}\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn wait_for(&self, name: &str, state: &str) {
        self.wait_until(&format!("{name} is not {state}"), |seen| {
            seen["machines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|machine| machine["name"] == name && machine["state"] == state)
        });
    }

    /// Waits until the list this machine checked names `name`, or no longer does.
    pub fn wait_for_member(&self, name: &str, present: bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            let members = self.user.call("remote.members", json!({})).unwrap();
            let listed = members["members"]
                .as_array()
                .unwrap()
                .iter()
                .any(|member| member["name"] == name);
            if listed == present {
                return;
            }
            assert!(Instant::now() < deadline, "{name}: {members:#}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// A command of this machine's command line aimed at the machine `name`.
    pub fn on(&self, name: &str, args: &[&str]) -> Output {
        self.user.run(&[&["--machine", name], args].concat())
    }

    /// One line written to this machine's socket as it is: the answer the daemon wrote back.
    pub fn raw(&self, line: &str) -> Value {
        self.user.status();
        let mut stream =
            UnixStream::connect(self.user.path("runtime/flottille/daemon.sock")).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        writeln!(stream, "{line}").unwrap();
        let line = common::read_line_patiently(stream);
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not an answer: {line:?}"))
    }
}

/// `pc` paired as the hub's first device, a runner.
pub fn set_up(hub: &TestHub, pc: &Machine) {
    pair(hub, pc, "pc", "runner");
}

/// `machine` paired with `hub` as the user pairs it, as `name` with `role`: pair mode opened on
/// the hub, its code given to `flottille remote pair`.
pub fn pair(hub: &TestHub, machine: &Machine, name: &str, role: &str) {
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

/// A hub, `pc` its first device, and `server` (a runner on `server_user`) paired with it, the
/// two linked.
pub fn pc_and_server(server_user: User) -> (TestHub, Machine, Machine) {
    pc_and_server_on(Machine::new(), server_user)
}

pub fn pc_and_server_on(pc: Machine, server_user: User) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let server = Machine::of(server_user);
    set_up(&hub, &pc);
    pair(&hub, &server, "server", "runner");
    pc.wait_for("server", "online");
    server.wait_for("pc", "online");
    (hub, pc, server)
}

/// `method` with `params` for `machine`, signed by `key` now: the line a client writes.
pub fn signed(key: &DeviceKey, id: &str, machine: &str, method: &str, params: Value) -> String {
    let request = Request::sign(key, id, machine, method, &params, SystemTime::now()).unwrap();
    serde_json::to_string(&request).unwrap()
}

pub fn error_of(answer: &Value) -> String {
    answer["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("not an error: {answer}"))
        .to_owned()
}

pub fn json_of(output: Output) -> Value {
    assert!(
        output.status.success(),
        "failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

pub fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The endpoint of the machine key in the key folder `folder` through its hub, as the machine's
/// daemon holds it, for a test that speaks as that machine: its daemon stopped first, since the
/// hub keeps one session a key. Within a Tokio runtime; the session lasts while the runtime does.
pub async fn endpoint_of(folder: &std::path::Path) -> Endpoint {
    let key = MachineKey::load_or_make(folder).unwrap();
    let pinned: Device =
        serde_json::from_str(&std::fs::read_to_string(folder.join("hub.json")).unwrap()).unwrap();
    let hello = Hello::ours(env!("CARGO_PKG_VERSION"));
    let mut session = pinned.connect(&key, &hello, &Tls::System).await.unwrap();
    let endpoint = Endpoint::new(&key).unwrap();
    endpoint.through(session.relay().expect("the session's relay"));
    tokio::spawn(async move { while session.next().await.is_some() {} });
    endpoint
}

/// A link of `traffic` from `endpoint` to the machine whose key is `peer`, named `name`, dialled
/// again while the hub does not say it online yet.
pub async fn dial(
    endpoint: &Endpoint,
    peer: PublicKey,
    traffic: Traffic,
    name: &str,
) -> Result<Link, LinkError> {
    let deadline = Instant::now() + WAIT;
    loop {
        match endpoint
            .connect(peer, traffic, &Hello::ours("0.4.1"), name)
            .await
        {
            Err(LinkError::Offline(_) | LinkError::NoHub(_)) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            made => return made,
        }
    }
}

/// The machine key of `machine`, as `remote.machines` says it.
pub fn key_of(machine: &Machine) -> PublicKey {
    serde_json::from_value(machine.remote()["this"]["key"].clone()).unwrap()
}

/// A phone of the user's, paired with the hub as the app pairs it (a controller, its endpoint's
/// key declared as its machine, saying it is a phone), its session to the hub kept open: what the
/// hub tells it for its notifications (`docs/remote.md`, "Notifications") is kept, in order, each
/// with the key of the machine it is from and the frame as it came.
pub struct Phone {
    runtime: tokio::runtime::Runtime,
    _folder: tempfile::TempDir,
    asker: flottille_link::Asker,
    told: std::sync::Arc<std::sync::Mutex<Vec<(PublicKey, flottille_link::Notice)>>>,
}

impl Phone {
    pub fn pair(hub: &TestHub, name: &str) -> Phone {
        use flottille_link::{Action, Ask, Change, Colour, Icon, PairCode, Proposed, Pushed, Role};
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let folder = tempfile::tempdir().unwrap();
        let key = DeviceKey::make().unwrap();
        let endpoint = MachineKey::load_or_make(&folder.path().join("endpoint")).unwrap();
        let code = PairCode::from_code(&hub.code()).unwrap();
        let told = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = told.clone();
        let name = name.to_owned();
        let asker = runtime.block_on(async move {
            let proposed = Proposed {
                key: key.public(),
                name,
                role: Role::Controller,
                icon: Icon::Phone,
                colour: Colour::of(&key.public()),
            };
            let hello = Hello::ours(env!("CARGO_PKG_VERSION"));
            let mut paired = Device::pair(&code, proposed, &endpoint, &hello, &Tls::System)
                .await
                .unwrap();
            let (mut device, _) = paired.listed().await.unwrap();
            let declared = Change::sign(
                device.list.head(),
                Action::Machine {
                    key: key.public(),
                    machine: endpoint.public(),
                },
                &key,
            )
            .claimed_by(&endpoint);
            device.submit(&mut paired.session, declared).await.unwrap();
            let mut session = paired.session;
            session
                .ask(Ask::About {
                    device: "phone".to_owned(),
                    system: "android".to_owned(),
                    notices: true,
                })
                .await
                .unwrap();
            let asker = session.asker();
            tokio::spawn(async move {
                while let Some(pushed) = session.next().await {
                    if let Pushed::Notice { from, notice } = pushed {
                        kept.lock().unwrap().push((from, notice));
                    }
                }
            });
            asker
        });
        Phone {
            runtime,
            _folder: folder,
            asker,
            told,
        }
    }

    /// What the app says when it opens its session again: an input now (`mobile.rs`).
    pub fn tell_presence(&self) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let asker = self.asker.clone();
        self.runtime
            .block_on(asker.ask(flottille_link::Ask::Presence {
                active: true,
                last_input: now,
            }))
            .unwrap();
    }

    /// What the hub told this phone so far.
    pub fn told(&self) -> Vec<(PublicKey, flottille_link::Notice)> {
        self.told.lock().unwrap().clone()
    }

    /// Waits until the hub told this phone `count` notices: all it told.
    pub fn wait_told(&self, count: usize) -> Vec<(PublicKey, flottille_link::Notice)> {
        eventually(&format!("{count} notices told the phone"), || {
            self.told().len() >= count
        });
        self.told()
    }

    /// Whether the hub tells this phone nothing more than `count` notices within `wait`.
    pub fn told_nothing_more(&self, count: usize, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if self.told().len() > count {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.told().len() == count
    }
}
