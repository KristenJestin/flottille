//! Two machines of the user's and their hub, for the tests of remote (`docs/remote.md`): a hub
//! served in the test on 127.0.0.1 and daemons, each with its own temporary HOME and XDG folders
//! and no keyring of the user's, the first machine set up with `flottille remote setup` and the
//! others joined as the user pairs them.

// Each test file uses its own part of it.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::process::{Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{DeviceKey, Hello, Keyring, KeyringError, Request};
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
    served: Served<SystemClock>,
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
            served,
            address,
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn code(&self) -> String {
        self.served
            .with_hub(|hub| hub.setup_code().unwrap().to_owned())
    }
}

pub struct Machine {
    pub user: User,
}

impl Machine {
    /// `user`'s machine, with remote on, its link on loopback.
    pub fn of(user: User) -> Machine {
        std::fs::write(
            user.path("config/flottille/daemon.toml"),
            "[remote]\non = true\nlisten = \"127.0.0.1:0\"\n",
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

/// `pc` set up as the hub's first member.
pub fn set_up(hub: &TestHub, pc: &Machine) {
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

/// `joiner` joins the hub of `inviter` as the user pairs them, as `name` with `role`.
pub fn pair(inviter: &Machine, joiner: &Machine, name: &str, role: &str) {
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
    {
        let mut input = joining.stdin.take().unwrap();
        input.write_all(format!("{code}\n").as_bytes()).unwrap();
    }
    let mut out = BufReader::new(joining.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    let deadline = Instant::now() + WAIT;
    let request = loop {
        let requests = inviter.user.call("remote.requests", json!({})).unwrap();
        if let Some(first) = requests.as_array().and_then(|all| all.first()) {
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
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
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
pub fn pc_and_server(server_user: User) -> (TestHub, Machine, Machine) {
    pc_and_server_on(Machine::new(), server_user)
}

pub fn pc_and_server_on(pc: Machine, server_user: User) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let server = Machine::of(server_user);
    set_up(&hub, &pc);
    pair(&pc, &server, "server", "runner");
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
