//! The user drives another machine (`docs/remote.md`, "Requests from another machine's
//! clients"): a hub served in the test on 127.0.0.1 and daemons, each with its own temporary HOME
//! and XDG folders and no keyring, the first machine set up with `flottille remote setup` and the
//! others joined as the user pairs them. A client of `pc` signs each request for `server` with
//! the device key, `pc`'s daemon carries it over its link, and `server` checks it before it serves
//! it as one of its own clients'.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    DeviceKey, Endpoint, Hello, Keyring, KeyringError, MachineKey, Message, PublicKey, Request,
    Traffic,
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

/// How long a link, or the hub's news, may take: the daemons try again every second.
const WAIT: Duration = Duration::from_secs(40);

/// What the server's log says of each request it served for another machine's client.
const SERVED: &str = "asked by";

/// A keyring that is not there: the device key is the file a client keeps in the key folder.
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
    /// Serves the hub while the test runs.
    _runtime: tokio::runtime::Runtime,
    _folder: tempfile::TempDir,
    served: Served<SystemClock>,
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
            _runtime: runtime,
            _folder: folder,
            served,
            address,
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn code(&self) -> String {
        self.served
            .with_hub(|hub| hub.setup_code().unwrap().to_owned())
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
        user.status();
        Machine { user }
    }

    fn new() -> Machine {
        Machine::of(User::new())
    }

    fn remote(&self) -> Value {
        self.user.call("remote.machines", json!({})).unwrap()
    }

    fn folder(&self) -> std::path::PathBuf {
        self.user.path("state/flottille/remote")
    }

    /// The device key this machine's clients hold, read as a client reads it.
    fn device(&self) -> DeviceKey {
        DeviceKey::load(&NoKeyring, &self.folder())
            .unwrap()
            .expect("a member's device key")
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.user.path("state/flottille/daemon.log")).unwrap_or_default()
    }

    /// Waits until `check` holds of what `remote.machines` says.
    fn wait_until(&self, what: &str, check: impl Fn(&Value) -> bool) {
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

    fn wait_for(&self, name: &str, state: &str) {
        self.wait_until(&format!("{name} is not {state}"), |seen| {
            seen["machines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|machine| machine["name"] == name && machine["state"] == state)
        });
    }

    /// Waits until the list this machine checked names `name`, or no longer does.
    fn wait_for_member(&self, name: &str, present: bool) {
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
    fn on(&self, name: &str, args: &[&str]) -> Output {
        self.user.run(&[&["--machine", name], args].concat())
    }

    /// One line written to this machine's socket as it is: the answer the daemon wrote back.
    fn raw(&self, line: &str) -> Value {
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

/// `joiner` joins the hub of `inviter` as the user pairs them, as `name` with `role`.
fn pair(inviter: &Machine, joiner: &Machine, name: &str, role: &str) {
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
fn pc_and_server(server_user: User) -> (TestHub, Machine, Machine) {
    pc_and_server_on(Machine::new(), server_user)
}

fn pc_and_server_on(pc: Machine, server_user: User) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let server = Machine::of(server_user);
    set_up(&hub, &pc);
    pair(&pc, &server, "server", "runner");
    pc.wait_for("server", "online");
    server.wait_for("pc", "online");
    (hub, pc, server)
}

/// `method` with `params` for `machine`, signed by `key` now: the line a client writes.
fn signed(key: &DeviceKey, id: &str, machine: &str, method: &str, params: Value) -> String {
    let request = Request::sign(key, id, machine, method, &params, SystemTime::now()).unwrap();
    serde_json::to_string(&request).unwrap()
}

fn error_of(answer: &Value) -> String {
    answer["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("not an error: {answer}"))
        .to_owned()
}

fn json_of(output: Output) -> Value {
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

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A throwaway repository with one commit, pushed to a bare remote: its folder.
fn repository(user: &User) -> std::path::PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    folder
}

fn fake_agent_user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "GREET", "do": [{"say": "hello from the server"}]},
    ]}));
    user
}

#[test]
fn the_cli_starts_and_reads_a_session_on_another_machine() {
    let (_hub, pc, server) = pc_and_server(fake_agent_user());
    // The folder is the server's: a path typed on purpose is that machine's.
    let folder = repository(&server.user);
    let added = json_of(pc.on(
        "server",
        &[
            "project",
            "add",
            folder.to_str().unwrap(),
            "--name",
            "app",
            "--yes",
            "--json",
        ],
    ));
    assert_eq!(added["name"], "app", "{added}");
    assert!(
        server
            .user
            .call("project.show", json!({"name": "app"}))
            .is_ok()
    );
    assert!(
        pc.user
            .call("project.show", json!({"name": "app"}))
            .is_err()
    );

    let started = json_of(pc.on("server", &["agent", "start", "app", "GREET", "--json"]));
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    // It runs on the server, not here.
    assert!(server.user.call("agent.show", json!({"agent": id})).is_ok());
    assert!(pc.user.call("agent.show", json!({"agent": id})).is_err());

    let mut read = Value::Null;
    eventually("the server's reply read from pc", || {
        read = json_of(pc.on("server", &["agent", "read", &id, "--json"]));
        read["lines"].to_string().contains("hello from the server")
    });

    // FLOTTILLE_MACHINE aims every command the same way.
    let listed = pc
        .user
        .command(&["agent", "list", "--json"])
        .env("FLOTTILLE_MACHINE", "server")
        .output()
        .unwrap();
    let listed = json_of(listed);
    assert!(listed.to_string().contains(&id), "{listed}");
    let status = json_of(pc.on("server", &["status", "--json"]));
    assert_eq!(status["pid"], server.user.status()["pid"]);

    // The server names the device in its log, and in the events its serving emitted.
    let log = server.log();
    assert!(log.contains("agent.start asked by pc from pc"), "{log}");
    let events = server
        .user
        .watch(&["--after", "0"])
        .within(Duration::from_secs(2));
    assert!(
        events
            .iter()
            .any(|event| event["name"] == "project.added" && event["data"]["device"] == "pc"),
        "{events:#?}"
    );
}

#[test]
fn an_unsigned_or_badly_signed_request_is_refused() {
    let (_hub, pc, server) = pc_and_server(User::new());

    let unsigned =
        pc.raw(&json!({"id": "1", "machine": "server", "method": "daemon.stop"}).to_string());
    assert_eq!(unsigned["id"], "1");
    assert!(
        error_of(&unsigned).starts_with("refused: this is not a signed request"),
        "{unsigned}"
    );

    // Signed by a key that is no member's.
    let stranger = DeviceKey::make().unwrap();
    let answer = pc.raw(&signed(&stranger, "2", "server", "daemon.stop", json!({})));
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );

    // Signed by pc's device, then changed on the way.
    let line = signed(&pc.device(), "3", "server", "project.list", json!({}));
    let changed = line.replace("project.list", "daemon.stop");
    let answer = pc.raw(&changed);
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );
    // Its answer cannot be taken under another id either.
    let moved = line.replace("\"id\":\"3\"", "\"id\":\"4\"");
    let answer = pc.raw(&moved);
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );

    assert!(!server.log().contains(SERVED), "{}", server.log());
    server.user.status();
}

#[test]
fn a_replayed_request_is_refused() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let line = signed(&pc.device(), "1", "server", "daemon.status", json!({}));

    let first = pc.raw(&line);
    assert_eq!(first["id"], "1");
    assert_eq!(
        first["result"]["pid"],
        server.user.status()["pid"],
        "{first}"
    );
    let again = pc.raw(&line);
    assert_eq!(
        error_of(&again),
        "refused: this request was received already"
    );
    assert_eq!(server.log().matches(SERVED).count(), 1, "{}", server.log());
}

#[test]
fn a_request_of_a_removed_device_is_refused() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let laptop = Machine::new();
    pair(&pc, &laptop, "laptop", "controller");
    server.wait_for_member("laptop", true);
    let device = laptop.device();

    // Carried by pc's daemon, which holds no device key: the signature is laptop's.
    let answer = pc.raw(&signed(&device, "1", "server", "project.list", json!({})));
    assert!(answer["result"]["projects"].is_array(), "{answer}");

    // A client link laptop's machine key opened before its removal, kept open by a machine that
    // does not let it go.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let server_key: PublicKey =
        serde_json::from_value(server.remote()["this"]["key"].clone()).unwrap();
    let address: SocketAddr = server.remote()["this"]["listening"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let endpoint = runtime.block_on(async {
        Endpoint::dialler(&MachineKey::load_or_make(&laptop.folder()).unwrap()).unwrap()
    });
    let kept = runtime
        .block_on(endpoint.connect(
            server_key,
            &[address],
            Traffic::Client,
            &Hello::ours("0.4.1"),
            "server",
        ))
        .unwrap();

    let removed = pc.user.run(&["remote", "remove", "laptop"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    server.wait_for_member("laptop", false);
    let answer = pc.raw(&signed(&device, "2", "server", "project.list", json!({})));
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );
    let line = signed(&pc.device(), "3", "server", "project.list", json!({}));
    let refused = runtime.block_on(async {
        let request = Message::Client(serde_json::from_str(&line).unwrap());
        match kept.ask(&request).await {
            Ok(mut answers) => answers
                .next()
                .await
                .map(|answer| answer.map_err(|e| e.to_string())),
            Err(error) => Some(Err(error.to_string())),
        }
    });
    assert!(
        matches!(&refused, Some(Err(why)) if why.contains("removed from the member list")),
        "{refused:?}"
    );
    runtime.block_on(endpoint.close());
}

#[test]
fn a_request_to_an_offline_machine_is_refused_saying_since_when() {
    let (_hub, pc, server) = pc_and_server(User::new());
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    pc.wait_for("server", "offline");

    let refused = pc.on("server", &["project", "list"]);
    assert!(!refused.status.success());
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    let (_, since) = said
        .split_once("server is offline since ")
        .unwrap_or_else(|| panic!("{said}"));
    let since = since.trim();
    assert!(
        since.len() == 5 && since.as_bytes()[2] == b':',
        "not a time of day: {said}"
    );

    // Nothing was kept for it: the server, back, serves nothing it did not take.
    server.user.status();
    pc.wait_for("server", "online");
    std::thread::sleep(Duration::from_secs(2));
    assert!(!server.log().contains(SERVED), "{}", server.log());
}

#[test]
fn events_of_another_machine_resume_after_the_last_seen_when_the_link_drops() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let first_pid = server.user.status()["pid"].clone();

    let watch = pc.user.watch(&["--machine", "server", "--after", "0"]);
    let seen =
        watch.until(|event| event["name"] == "daemon.started" && event["data"]["pid"] == first_pid);
    let last = seen.last().unwrap()["seq"].as_u64().unwrap();

    // The server stops and starts again: the link drops, and comes back.
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    let second_pid = server.user.status()["pid"].clone();
    assert_ne!(first_pid, second_pid);
    let after = watch
        .until(|event| event["name"] == "daemon.started" && event["data"]["pid"] == second_pid);
    // Every event after the last seen, none twice, none missing.
    let seqs: Vec<u64> = after
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    let stored: Vec<u64> = server
        .user
        .watch(&["--after", &last.to_string()])
        .until(|event| event["name"] == "daemon.started" && event["data"]["pid"] == second_pid)
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, stored);
    assert!(seqs[0] > last, "{seqs:?} after {last}");
    // The server's events, not pc's: pc's own daemon never stopped.
    assert!(
        after.iter().any(|event| event["name"] == "daemon.stopping"),
        "{after:#?}"
    );
}

#[test]
fn a_machine_message_cannot_carry_a_user_method() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let server_key: PublicKey =
        serde_json::from_value(server.remote()["this"]["key"].clone()).unwrap();
    let address: SocketAddr = server.remote()["this"]["listening"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // pc's daemon is stopped, so that no link of its own takes the one below (two links of one
    // machine are not kept: the second closes the first, whoever dials again): the key is only
    // what the test holds.
    assert!(pc.user.run(&["daemon", "stop"]).status.success());
    server.wait_for("pc", "offline");
    // pc's machine key, as an agent that read it would hold it: it signs what a machine may
    // send, never what the user asks.
    let machine_key = MachineKey::load_or_make(&pc.folder()).unwrap();
    let folder = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let user_request = signed(&pc.device(), "1", "server", "daemon.stop", json!({}));
    let refusals = runtime.block_on(async {
        let endpoint = Endpoint::dialler(&machine_key).unwrap();
        let link = tokio::time::timeout(
            WAIT,
            endpoint.connect(
                server_key,
                &[address],
                Traffic::Machine,
                &Hello::ours("0.4.1"),
                "server",
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let mut refusals = Vec::new();
        for frame in [
            // A user's request, even one a member's device signed, on the machine side.
            json!({"client": serde_json::from_str::<Value>(&user_request).unwrap()}),
            // A user method dressed as a machine's message.
            json!({"machine": {"call": {"method": "daemon.stop", "params": {}}}}),
            json!({"machine": {"daemon.stop": {}}}),
            json!({"machine": {"method": "project.add", "params": {"folder": "/"}}}),
        ] {
            let answer = link.send_raw(frame.to_string().as_bytes()).await;
            refusals.push(answer.unwrap_err().to_string());
        }
        // And a client link takes no request the machine key signed.
        let client = endpoint
            .connect(
                server_key,
                &[address],
                Traffic::Client,
                &Hello::ours("0.4.1"),
                "server",
            )
            .await
            .unwrap();
        let forged = Request::sign(
            &machine_as_device(&pc.folder(), folder.path()),
            "2",
            "server",
            "daemon.stop",
            &json!({}),
            SystemTime::now(),
        )
        .unwrap();
        let mut answers = client
            .ask(&Message::Client(serde_json::to_value(&forged).unwrap()))
            .await
            .unwrap();
        refusals.push(answers.next().await.unwrap().unwrap().to_string());
        endpoint.close().await;
        refusals
    });
    assert_eq!(
        refusals[0],
        "server refused: a client's request is not taken on a machine link"
    );
    for refusal in &refusals[1..4] {
        assert_eq!(
            refusal, "server refused: not a message a machine may send",
            "{refusals:?}"
        );
    }
    assert!(
        refusals[4].contains("refused: this request is not signed by a member"),
        "{refusals:?}"
    );
    // The server still runs, and served nothing.
    server.user.status();
    assert!(!server.log().contains(SERVED), "{}", server.log());
}

/// The machine key kept in `folder` taken for a device key, as code that read it could: its 32
/// bytes written where a client keeps a device key, in `apart`.
fn machine_as_device(folder: &Path, apart: &Path) -> DeviceKey {
    use std::os::unix::fs::PermissionsExt;
    let apart = apart.join("remote");
    std::fs::create_dir(&apart).unwrap();
    std::fs::set_permissions(&apart, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::copy(folder.join("machine.key"), apart.join("device.key")).unwrap();
    DeviceKey::load(&NoKeyring, &apart).unwrap().unwrap()
}

#[test]
fn a_client_started_by_an_agent_is_still_refused_with_machine() {
    let pc_user = User::with_fake_agent().with_allowing_judge();
    let (_hub, pc, server) = pc_and_server_on(Machine::of(pc_user), User::new());
    let repository = repository(&pc.user);
    pc.user
        .call("project.add", json!({"folder": repository, "name": "app"}))
        .unwrap();
    let workspace = pc
        .user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    let folder = std::path::PathBuf::from(workspace["folder"].as_str().unwrap());
    let opened = pc.user.received_requests("session/new").len();
    pc.user
        .call(
            "agent.run",
            json!({"project": "app", "message": "hello", "workspace": "w"}),
        )
        .unwrap();
    let (url, authorization) = server_of(&pc.user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    // The user's own command line, aimed at the server, run by the agent: the device key is
    // there to sign with, and still nothing is carried.
    std::fs::write(
        folder.join("client.sh"),
        format!(
            "XDG_RUNTIME_DIR='{}' XDG_STATE_HOME='{}' XDG_CONFIG_HOME='{}' exec '{}' \
             --machine server status > client.out 2>&1\n",
            pc.user.path("runtime").display(),
            pc.user.path("state").display(),
            pc.user.path("config").display(),
            env!("CARGO_BIN_EXE_flottille")
        ),
    )
    .unwrap();
    let answer = mcp.call("run", json!({"line": "sh client.sh"}));
    assert!(
        answer.text.contains("exited with code 1"),
        "{}",
        answer.text
    );
    let said = std::fs::read_to_string(folder.join("client.out")).unwrap();
    // Refused by its ancestry; where the kernel's Landlock reaches sockets by their path, the
    // run cannot even open the socket.
    assert!(
        said.contains("refused: this client was started by an agent")
            || said.contains("daemon.sock: Permission denied"),
        "{said}"
    );
    server.user.status();
    assert!(!server.log().contains(SERVED), "{}", server.log());
}

/// What a machine's log says when a watch it served for a device ends.
const WATCH_ENDED: &str = "the watch asked by pc ended";

#[test]
fn a_watch_of_another_machine_ends_with_its_client_on_a_quiet_machine() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let pid = server.user.status()["pid"].clone();
    let watch = pc.user.watch(&["--machine", "server", "--after", "0"]);
    watch.until(|event| event["name"] == "daemon.started" && event["data"]["pid"] == pid);
    assert!(!server.log().contains(WATCH_ENDED), "{}", server.log());

    // The client leaves; nothing happens on the server meanwhile, so no event is sent that
    // could tell the carrying daemon its client is gone.
    drop(watch);
    eventually("the server's subscription to end", || {
        server.log().contains(WATCH_ENDED)
    });
}

#[test]
fn a_watch_of_this_machine_by_its_name_ends_with_its_client_too() {
    let (_hub, pc, _server) = pc_and_server(User::new());
    let watch = pc.user.watch(&["--machine", "pc", "--after", "0"]);
    watch.until(|event| event["name"] == "daemon.started");
    assert!(!pc.log().contains(WATCH_ENDED), "{}", pc.log());

    drop(watch);
    eventually("the subscription to end", || pc.log().contains(WATCH_ENDED));
}

#[test]
fn an_exported_machine_does_not_stop_the_local_daemon_from_starting() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let pid = server.user.status()["pid"].clone();
    // No daemon runs here: the command starts one, which must not read the variable the command
    // was given.
    assert!(pc.user.run(&["daemon", "stop"]).status.success());
    eventually(
        "the server's status read through a daemon the command started",
        || {
            let output = pc
                .user
                .command(&["status", "--json"])
                .env("FLOTTILLE_MACHINE", "server")
                .output()
                .unwrap();
            serde_json::from_slice::<Value>(&output.stdout).is_ok_and(|status| status["pid"] == pid)
        },
    );
    // `daemon` itself ignores the variable and refuses the flag.
    let refused = pc.user.run(&["--machine", "server", "daemon"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("this machine's own"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn the_commands_that_run_here_refuse_the_flag() {
    let pc = Machine::new();
    for line in [
        &["doctor"][..],
        &["vault", "provide", "/nonexistent/login.sock"],
        &["read-page", "web", "http://app.example.com/"],
        &["mcp"],
        &["update"],
        &["remote", "members"],
    ] {
        let flagged = pc.user.run(&[&["--machine", "server"], line].concat());
        let said = String::from_utf8_lossy(&flagged.stderr);
        assert!(
            !flagged.status.success() && said.contains("this machine's own"),
            "{line:?}: {said}"
        );
    }
}

#[test]
fn the_variable_alone_does_not_refuse_the_commands_that_run_here() {
    let pc = Machine::new();
    // Only the ones that touch nothing outside the test's folders.
    for line in [
        &["vault", "provide", "/nonexistent/login.sock"][..],
        &["read-page", "web", "http://app.example.com/"],
    ] {
        let exported = pc
            .user
            .command(line)
            .env("FLOTTILLE_MACHINE", "server")
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&exported.stderr);
        assert!(!said.contains("this machine's own"), "{line:?}: {said}");
    }
}

/// The pull request of `app/login` in `acme/api` as the fake `gh` answers it.
fn pull(checks: &str) -> Value {
    json!({
        "number": 12,
        "title": "Add the login page",
        "state": "OPEN",
        "isDraft": false,
        "baseRefName": "main",
        "url": "https://github.com/acme/api/pull/12",
        "reviewDecision": "",
        "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": checks}
        ],
        "comments": [],
        "reviews": []
    })
}

#[test]
fn a_forge_watched_on_another_machine_stays_read_while_the_watch_runs() {
    let server_user = User::new()
        .with_fake_gh()
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    let (_hub, pc, server) = pc_and_server(server_user);
    let folder = repository(&server.user);
    server
        .user
        .call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    server
        .user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    server.user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:acme/api.git"],
    );
    let settings = server
        .user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();
    let declared: Vec<Value> = settings["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!({"path": r["path"], "forge": r["proposal"]}))
        .collect();
    server
        .user
        .call(
            "project.set_settings",
            json!({"name": "app", "repositories": declared}),
        )
        .unwrap();
    let pulls = |checks: &str| {
        server
            .user
            .gh_script(json!({"pulls": {"acme/api": {"app/login": pull(checks)}}}));
    };
    pulls("SUCCESS");

    let watch = pc
        .user
        .watch(&["--machine", "server", "--forge", "app", "login"]);
    let first = watch
        .until(|event| event["name"] == "workspace.forge")
        .pop()
        .unwrap();
    assert_eq!(first["data"]["project"], "app", "{first}");

    // The checks fail; a minute later the server reads the forge again, for the watch is still
    // there, and says so.
    pulls("FAILURE");
    server
        .user
        .call("test.advance", json!({"ms": 61_000}))
        .unwrap();
    let changed = watch
        .until(|event| event["name"] == "workspace.forge")
        .pop()
        .unwrap();
    assert!(changed["data"].to_string().contains("failing"), "{changed}");
}
