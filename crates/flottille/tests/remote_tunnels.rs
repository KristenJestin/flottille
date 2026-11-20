//! A run's port reached from another machine through a tunnel (`docs/remote.md`, "Tunnels"): a
//! hub served in the test on 127.0.0.1 and daemons, each with its own temporary HOME and XDG
//! folders and no keyring, paired as the user pairs them. The server runs the fake HTTP server as
//! a task's run; a client of `pc` opens a tunnel to it with the command line, and the test speaks
//! HTTP on the local port with a plain `TcpStream`. A test that sends a line to a daemon has the
//! fake `sudo`, `doas`, `pkexec`, `su` and `run0` at the head of its `PATH` (`User::new`).

use crate::common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::process::{Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use common::mcp::{Mcp, server_of};
use common::{Spawned, User};
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::{
    DeviceKey, Endpoint, Hello, Keyring, KeyringError, MachineKey, PublicKey, Request, Traffic,
};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
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

/// A hub, `pc`, `server` (a runner) and a third machine paired as `name` with `role`, the three
/// linked.
fn pc_server_and(
    server_user: User,
    third_user: User,
    name: &str,
    role: &str,
) -> (TestHub, Machine, Machine, Machine) {
    let (hub, pc, server) = pc_and_server(server_user);
    let third = Machine::of(third_user);
    pair(&pc, &third, name, role);
    pc.wait_for(name, "online");
    server.wait_for(name, "online");
    third.wait_for("server", "online");
    (hub, pc, server, third)
}

/// A project `app` on `user`'s machine over a throwaway repository.
fn app_on(user: &User) -> std::path::PathBuf {
    let folder = repository(user);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

/// The fake HTTP server, as the task `name` of `app` with `extra` after its line, run by the
/// user: its run.
fn run_server(user: &User, name: &str, line: &str, extra: Value) -> String {
    let mut task = json!({"project": "app", "name": name,
        "line": format!("{} {line}", common::fake_server().display()), "check": true});
    for (key, value) in extra.as_object().into_iter().flatten() {
        task[key] = value.clone();
    }
    user.call("task.add", task).unwrap();
    user.call("task.run", json!({"project": "app", "name": name}))
        .unwrap()["run"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The server's run of the fake HTTP server on a free port, once it says it listens: the run and
/// the port.
fn web_on(user: &User) -> (String, u16) {
    web_with(user, "--listen 0")
}

/// The same with `line` as the server's options (`--host`, `--serve`...): the run and the port it
/// took.
fn web_with(user: &User, line: &str) -> (String, u16) {
    app_on(user);
    let run = run_server(user, "web", line, json!({}));
    let mut port = 0;
    eventually("the server listens", || {
        let output = user.call("run.output", json!({"run": run})).unwrap();
        let text = output["output"].as_str().unwrap_or_default().to_owned();
        match text
            .lines()
            .find_map(|line| line.strip_prefix("listening on http://"))
        {
            Some(found) => {
                port = found.rsplit(':').next().unwrap().parse().unwrap();
                true
            }
            None => false,
        }
    });
    (run, port)
}

/// A tunnel the command line holds open: its answer, and the lines it prints after it.
struct Open {
    child: Spawned,
    lines: mpsc::Receiver<String>,
    answer: Value,
}

impl Open {
    fn local_port(&self) -> u16 {
        self.answer["local_port"].as_u64().unwrap() as u16
    }

    fn id(&self) -> String {
        self.answer["tunnel"].as_str().unwrap().to_owned()
    }

    /// Whether the command ended: its status, when it did.
    fn ended(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().unwrap()
    }
}

/// `flottille --machine <machine> tunnel open <run> <extra>` on `from`, left running: the tunnel
/// lives as long as it does.
fn open(from: &Machine, machine: &str, run: &str, extra: &[&str]) -> Open {
    let mut command = from.user.command(
        &[
            &["--machine", machine, "tunnel", "open", run, "--json"],
            extra,
        ]
        .concat(),
    );
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = Spawned::new(command);
    let stdout = child.stdout.take().unwrap();
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let first = lines.recv_timeout(WAIT).unwrap_or_else(|_| {
        let _ = child.kill();
        let mut said = String::new();
        let _ = child.stderr.take().unwrap().read_to_string(&mut said);
        panic!("no tunnel opened: {said}\n{}", from.log())
    });
    let answer = serde_json::from_str(&first).unwrap_or_else(|_| panic!("not JSON: {first}"));
    Open {
        child,
        lines,
        answer,
    }
}

/// One HTTP request on `port` of this machine, and the whole answer.
fn get(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: app\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

/// The tunnels `machine`'s daemon carries.
fn tunnels(machine: &Machine) -> Vec<Value> {
    machine.user.call("tunnel.list", json!({})).unwrap()["tunnels"]
        .as_array()
        .unwrap()
        .clone()
}

fn refused(output: Output) -> String {
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_run_s_port_on_another_machine_answers_on_a_local_port() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, port) = web_on(&server.user);

    let tunnel = open(&pc, "server", &run, &[]);
    assert_eq!(tunnel.answer["run"], run.as_str(), "{}", tunnel.answer);
    assert_eq!(tunnel.answer["port"], port, "{}", tunnel.answer);
    assert_eq!(tunnel.answer["machine"], "server");
    let local = tunnel.local_port();
    assert_eq!(
        tunnel.answer["address"],
        format!("127.0.0.1:{local}").as_str()
    );

    let answer = get(local);
    assert!(answer.starts_with("HTTP/1.1 204"), "{answer}");

    // The server's log says who asked, and the local side listens on the loopback alone.
    let log = server.log();
    assert!(log.contains("tunnel.open asked by pc from pc"), "{log}");
    let table = std::fs::read_to_string("/proc/net/tcp").unwrap();
    let hex = format!("{local:04X}");
    let listening = table
        .lines()
        .filter(|line| line.split_whitespace().nth(3) == Some("0A"))
        .filter(|line| {
            line.split_whitespace()
                .nth(1)
                .is_some_and(|at| at.ends_with(&format!(":{hex}")))
        })
        .collect::<Vec<_>>();
    assert!(!listening.is_empty(), "nothing listens on {local}: {table}");
    for line in listening {
        assert!(
            line.split_whitespace()
                .nth(1)
                .unwrap()
                .starts_with("0100007F:"),
            "{line}"
        );
    }
    let listed = tunnels(&pc);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["tunnel"], tunnel.answer["tunnel"]);
    assert_eq!(listed[0]["local_port"], local);
}

#[test]
fn a_port_no_run_of_flottille_listens_on_is_refused() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, _port) = web_on(&server.user);
    // Something of the machine's, which Flottille did not start: it must never be reached.
    let foreign = TcpListener::bind("127.0.0.1:0").unwrap();
    foreign.set_nonblocking(true).unwrap();
    let port = foreign.local_addr().unwrap().port().to_string();

    let said = refused(pc.on("server", &["tunnel", "open", &run, "--port", &port]));
    assert!(
        said.contains(&format!("refused: port {port} is none of run {run}'s")),
        "{said}"
    );
    // A run that is none of the server's, with that port or none.
    let said = refused(pc.on(
        "server",
        &["tunnel", "open", "no-such-run", "--port", &port],
    ));
    assert!(
        said.contains("refused: there is no run no-such-run"),
        "{said}"
    );
    // The server never connected to it, and the client listens on nothing.
    assert!(foreign.accept().is_err());
    assert!(tunnels(&pc).is_empty());
    // A port nothing listens on is refused the same way.
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let nothing = free.local_addr().unwrap().port().to_string();
    drop(free);
    let said = refused(pc.on("server", &["tunnel", "open", &run, "--port", &nothing]));
    assert!(
        said.contains(&format!("refused: port {nothing} is none of run {run}'s")),
        "{said}"
    );
}

#[test]
fn the_tunnel_closes_when_the_run_ends() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, _port) = web_on(&server.user);
    let events = pc.user.watch(&[]);

    let mut tunnel = open(&pc, "server", &run, &[]);
    let local = tunnel.local_port();
    assert!(get(local).starts_with("HTTP/1.1 204"));
    let opened = events.until(|event| event["name"] == "tunnel.opened");
    let opened = opened.last().unwrap();
    assert_eq!(
        opened["data"]["tunnel"], tunnel.answer["tunnel"],
        "{opened}"
    );
    assert_eq!(opened["data"]["local_port"], local, "{opened}");

    server.user.call("run.stop", json!({"run": run})).unwrap();
    let closed = events.until(|event| event["name"] == "tunnel.closed");
    let closed = closed.last().unwrap();
    assert_eq!(
        closed["data"]["tunnel"], tunnel.answer["tunnel"],
        "{closed}"
    );
    assert!(
        closed["data"]["reason"].as_str().unwrap().contains("ended"),
        "{closed}"
    );
    eventually("the command to end", || tunnel.ended().is_some());
    assert!(tunnel.ended().unwrap().success());
    assert!(tunnels(&pc).is_empty());
    eventually("the local port to close", || {
        TcpStream::connect(("127.0.0.1", local)).is_err()
    });
}

#[test]
fn several_connections_share_one_tunnel() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, _port) = web_on(&server.user);
    let tunnel = open(&pc, "server", &run, &[]);
    let local = tunnel.local_port();

    // All of them open at once, then each one asks.
    let mut streams: Vec<TcpStream> = (0..6)
        .map(|_| TcpStream::connect(("127.0.0.1", local)).unwrap())
        .collect();
    for stream in &mut streams {
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: app\r\nConnection: close\r\n\r\n")
            .unwrap();
    }
    for stream in &mut streams {
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 204"), "{answer}");
    }
    assert_eq!(tunnels(&pc).len(), 1);
    assert_eq!(server.log().matches("tunnel.open asked by").count(), 1);
    // And one after the others.
    assert!(get(local).starts_with("HTTP/1.1 204"));
}

#[test]
fn a_taken_local_port_gets_another_said_in_the_answer() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, port) = web_on(&server.user);
    // Both daemons are on this host: the run's own port is taken here, by the run itself.
    let tunnel = open(&pc, "server", &run, &[]);
    assert_eq!(tunnel.answer["port"], port);
    assert_ne!(tunnel.local_port(), port, "{}", tunnel.answer);
    assert!(get(tunnel.local_port()).starts_with("HTTP/1.1 204"));
}

#[test]
fn an_agent_has_no_way_to_open_a_tunnel() {
    let pc_user = User::with_fake_agent().with_allowing_judge();
    let (_hub, pc, server) = pc_and_server_on(Machine::of(pc_user), User::new());
    let (run, _port) = web_on(&server.user);
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
    // No tool of its own server opens one, or names one.
    for name in mcp.names() {
        assert!(!name.contains("tunnel"), "{name}");
    }
    for tool in mcp.tools() {
        let text = tool.to_string();
        assert!(!text.contains("tunnel"), "{text}");
    }
    // The user's command line, run by the agent: the device key is there to sign with, and
    // nothing is carried.
    std::fs::write(
        folder.join("tunnel.sh"),
        format!(
            "XDG_RUNTIME_DIR='{}' XDG_STATE_HOME='{}' XDG_CONFIG_HOME='{}' exec '{}' \
             --machine server tunnel open {run} > tunnel.out 2>&1\n",
            pc.user.path("runtime").display(),
            pc.user.path("state").display(),
            pc.user.path("config").display(),
            env!("CARGO_BIN_EXE_flottille")
        ),
    )
    .unwrap();
    let answer = mcp.call("run", json!({"line": "sh tunnel.sh"}));
    assert!(
        answer.text.contains("exited with code 1"),
        "{}",
        answer.text
    );
    let said = std::fs::read_to_string(folder.join("tunnel.out")).unwrap();
    assert!(
        said.contains("refused: this client was started by an agent")
            || said.contains("daemon.sock: Permission denied"),
        "{said}"
    );
    assert!(tunnels(&pc).is_empty());
    assert!(!server.log().contains("tunnel.open"), "{}", server.log());
}

#[test]
fn a_viewer_cannot_open_a_tunnel() {
    let (_hub, _pc, server, laptop) = pc_server_and(User::new(), User::new(), "laptop", "viewer");
    let (run, _port) = web_on(&server.user);

    let said = refused(laptop.on("server", &["tunnel", "open", &run]));
    assert!(
        said.contains("laptop may only view: a controller or a runner may open a tunnel"),
        "{said}"
    );
    let answer = laptop.raw(&signed(
        &laptop.device(),
        "1",
        "server",
        "tunnel.open",
        json!({"run": run}),
    ));
    assert_eq!(
        error_of(&answer),
        "refused: laptop may only view: a controller or a runner may open a tunnel"
    );
    assert!(tunnels(&laptop).is_empty());
    assert!(!server.log().contains(SERVED), "{}", server.log());
}

#[test]
fn a_controller_opens_a_tunnel_too() {
    let (_hub, _pc, server, tablet) =
        pc_server_and(User::new(), User::new(), "tablet", "controller");
    let (run, _port) = web_on(&server.user);
    let tunnel = open(&tablet, "server", &run, &[]);
    assert!(get(tunnel.local_port()).starts_with("HTTP/1.1 204"));
}

#[test]
fn a_tunnel_closes_with_its_client() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, _port) = web_on(&server.user);
    let tunnel = open(&pc, "server", &run, &[]);
    let local = tunnel.local_port();
    assert!(get(local).starts_with("HTTP/1.1 204"));

    drop(tunnel);
    eventually("the tunnel to close here", || tunnels(&pc).is_empty());
    eventually("the local port to close", || {
        TcpStream::connect(("127.0.0.1", local)).is_err()
    });
    eventually("the server to say it closed", || {
        server.log().contains("the tunnel asked by pc ended")
    });
}

#[test]
fn a_tunnel_closes_with_the_link() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, _port) = web_on(&server.user);
    let mut tunnel = open(&pc, "server", &run, &[]);
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    eventually("the tunnel to close", || tunnels(&pc).is_empty());
    eventually("the command to end", || tunnel.ended().is_some());
}

#[test]
fn the_tunnels_are_listed_and_closed_from_the_command_line() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, _port) = web_on(&server.user);
    let mut tunnel = open(&pc, "server", &run, &[]);
    let id = tunnel.id();

    let listed = json_of(pc.user.run(&["tunnel", "list", "--json"]));
    assert_eq!(listed["tunnels"][0]["tunnel"], id.as_str(), "{listed}");
    // `--machine` filters the list: it is this machine's own tunnels, never a request carried.
    let none = json_of(pc.on("laptop", &["tunnel", "list", "--json"]));
    assert!(none["tunnels"].as_array().unwrap().is_empty(), "{none}");
    let some = json_of(pc.on("server", &["tunnel", "list", "--json"]));
    assert_eq!(some["tunnels"].as_array().unwrap().len(), 1, "{some}");

    let closed = pc.user.run(&["tunnel", "close", &id]);
    assert!(
        closed.status.success(),
        "{}",
        String::from_utf8_lossy(&closed.stderr)
    );
    eventually("the command to end", || tunnel.ended().is_some());
    assert!(tunnels(&pc).is_empty());
    let said = refused(pc.user.run(&["tunnel", "close", &id]));
    assert!(said.contains(&format!("no tunnel {id}")), "{said}");
    let _ = tunnel.lines.try_recv();
}

#[test]
fn a_ninth_tunnel_of_a_device_is_refused() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, _port) = web_on(&server.user);
    let _open: Vec<Open> = (0..8).map(|_| open(&pc, "server", &run, &[])).collect();
    assert_eq!(tunnels(&pc).len(), 8);
    let said = refused(pc.on("server", &["tunnel", "open", &run]));
    assert!(
        said.contains("refused: 8 tunnels are open for this device already"),
        "{said}"
    );
    assert_eq!(tunnels(&pc).len(), 8);
}

#[test]
fn a_tunnel_carries_at_most_64_connections_at_once() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let (run, _port) = web_on(&server.user);
    let tunnel = open(&pc, "server", &run, &[]);
    let local = tunnel.local_port();
    // Held with no request: the fake server serves one at a time, so these wait on it.
    let held: Vec<TcpStream> = (0..64)
        .map(|_| TcpStream::connect(("127.0.0.1", local)).unwrap())
        .collect();
    eventually("64 connections to be carried", || {
        tunnels(&pc)[0]["connections"] == 64
    });
    // The 65th is closed at once.
    let mut over = TcpStream::connect(("127.0.0.1", local)).unwrap();
    over.set_read_timeout(Some(WAIT)).unwrap();
    let mut said = Vec::new();
    assert_eq!(over.read_to_end(&mut said).unwrap_or(0), 0);
    assert_eq!(tunnels(&pc)[0]["connections"], 64);
    drop(held);
    eventually("the connections to be let go", || {
        tunnels(&pc)[0]["connections"] == 0
    });
}

#[test]
fn a_connection_for_a_tunnel_nobody_opened_is_closed() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let _web = web_on(&server.user);
    let server_key: PublicKey =
        serde_json::from_value(server.remote()["this"]["key"].clone()).unwrap();
    let address: SocketAddr = server.remote()["this"]["listening"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // pc's daemon is stopped, so that its own link does not replace the one below: the key is
    // what a program that held pc's machine key would have.
    assert!(pc.user.run(&["daemon", "stop"]).status.success());
    server.wait_for("pc", "offline");
    let machine_key = MachineKey::load_or_make(&pc.folder()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let read = runtime.block_on(async {
        let endpoint = Endpoint::dialler(&machine_key).unwrap();
        let link = endpoint
            .connect(
                server_key,
                &[address],
                Traffic::Client,
                &Hello::ours("0.4.1"),
                "server",
            )
            .await
            .unwrap();
        // A tunnel that does not exist: closed, and no byte of the run's port comes back.
        let mut stream = link.tunnel("t0000000000000000").await.unwrap();
        let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, b"GET / HTTP/1.1\r\n\r\n").await;
        let mut back = Vec::new();
        let ended = tokio::time::timeout(WAIT, async { stream.read_to_end(&mut back).await }).await;
        endpoint.close().await;
        (ended.is_ok(), back)
    });
    assert!(read.0, "the connection was left open");
    assert!(read.1.is_empty(), "{:?}", read.1);
    eventually("the server to say it closed it", || {
        server
            .log()
            .contains("opened a connection of a tunnel t0000000000000000 that is none of its")
    });
}

/// Another machine's link to `server`, with the machine key of `machine` (whose daemon is
/// stopped, so that its own link does not replace this one): `tunnel` is opened on it, and what
/// comes back of a request line written to it.
fn read_through_a_link_of(
    machine: &Machine,
    name: &str,
    server: &Machine,
    tunnel: &str,
) -> (bool, Vec<u8>) {
    let server_key: PublicKey =
        serde_json::from_value(server.remote()["this"]["key"].clone()).unwrap();
    let address: SocketAddr = server.remote()["this"]["listening"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(machine.user.run(&["daemon", "stop"]).status.success());
    server.wait_for(name, "offline");
    let machine_key = MachineKey::load_or_make(&machine.folder()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let endpoint = Endpoint::dialler(&machine_key).unwrap();
        let link = endpoint
            .connect(
                server_key,
                &[address],
                Traffic::Client,
                &Hello::ours("0.4.1"),
                "server",
            )
            .await
            .unwrap();
        let mut stream = link.tunnel(tunnel).await.unwrap();
        let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, b"GET / HTTP/1.1\r\n\r\n").await;
        let mut back = Vec::new();
        let ended = tokio::time::timeout(WAIT, async { stream.read_to_end(&mut back).await }).await;
        endpoint.close().await;
        (ended.is_ok(), back)
    })
}

#[test]
fn a_tunnel_id_from_another_machine_s_link_is_refused() {
    let (_hub, pc, server, tablet) =
        pc_server_and(User::new(), User::new(), "tablet", "controller");
    let (run, _port) = web_on(&server.user);
    // A real tunnel, opened for pc: its id is no secret, and the tablet's machine knows it.
    let tunnel = open(&pc, "server", &run, &[]);
    let id = tunnel.id();
    assert!(get(tunnel.local_port()).starts_with("HTTP/1.1 204"));

    let (ended, back) = read_through_a_link_of(&tablet, "tablet", &server, &id);
    assert!(ended, "the connection was left open");
    assert!(back.is_empty(), "{:?}", String::from_utf8_lossy(&back));
    eventually("the server to say it closed it", || {
        server.log().contains(&format!(
            "opened a connection of a tunnel {id} that is none of its"
        ))
    });
    // pc's own tunnel still carries.
    assert!(get(tunnel.local_port()).starts_with("HTTP/1.1 204"));
}

#[test]
fn a_port_taken_by_a_foreign_listener_after_the_run_s_closed_is_refused_at_the_next_connection() {
    let (_hub, pc, server) = pc_and_server(User::new());
    // The run answers one request, then closes its listener and stays.
    let (run, port) = web_with(&server.user, "--listen 0 --serve 1");
    let tunnel = open(&pc, "server", &run, &[]);
    assert!(get(tunnel.local_port()).starts_with("HTTP/1.1 204"));
    // Another process takes the port the run gave up.
    let mut foreign = None;
    eventually("the port to be free", || {
        foreign = TcpListener::bind(("127.0.0.1", port)).ok();
        foreign.is_some()
    });
    let foreign = foreign.unwrap();
    foreign.set_nonblocking(true).unwrap();

    let mut stream = TcpStream::connect(("127.0.0.1", tunnel.local_port())).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let _ = stream.write_all(b"GET / HTTP/1.1\r\nHost: app\r\n\r\n");
    let mut said = Vec::new();
    assert_eq!(stream.read_to_end(&mut said).unwrap_or(0), 0);
    assert!(
        foreign.accept().is_err(),
        "the foreign listener was reached"
    );
    eventually("the server to say it closed it", || {
        server.log().contains(&format!(
            "port {port} is no longer one of run {run}'s: a connection of tunnel"
        ))
    });
}

#[test]
fn a_tunnel_closes_when_its_device_is_removed() {
    let (_hub, pc, server, tablet) =
        pc_server_and(User::new(), User::new(), "tablet", "controller");
    let (run, _port) = web_on(&server.user);
    let mut tunnel = open(&tablet, "server", &run, &[]);
    let local = tunnel.local_port();
    assert!(get(local).starts_with("HTTP/1.1 204"));

    let removed = pc.user.run(&["remote", "remove", "tablet"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    eventually("the server to close the tunnel", || {
        server.log().contains("the tunnel asked by tablet ended")
    });
    eventually("the tunnel to close on the tablet", || {
        tunnels(&tablet).is_empty()
    });
    eventually("the command to end", || tunnel.ended().is_some());
    eventually("the local port to close", || {
        TcpStream::connect(("127.0.0.1", local)).is_err()
    });
}

#[test]
fn a_v6_listener_beside_a_foreign_v4_one_is_refused() {
    let (_hub, pc, server) = pc_and_server(User::new());
    // The run listens on [::1]:P. Linux lets another process listen on 127.0.0.1:P beside it:
    // the bytes of a tunnel to P must not reach that one.
    let (run, port) = web_with(&server.user, "--listen 0 --host ::1");
    let foreign = TcpListener::bind(("127.0.0.1", port)).unwrap();
    foreign.set_nonblocking(true).unwrap();

    for extra in [&[][..], &["--port", &port.to_string()][..]] {
        let said = refused(pc.on("server", &[&["tunnel", "open", &run], extra].concat()));
        assert!(said.contains("refused:"), "{said}");
        assert!(said.contains("another process"), "{said}");
    }
    assert!(
        foreign.accept().is_err(),
        "the foreign listener was reached"
    );
    assert!(tunnels(&pc).is_empty());
}

#[test]
fn an_address_port_no_process_of_the_run_listens_on_is_refused() {
    let (_hub, pc, server) = pc_and_server(User::new());
    app_on(&server.user);
    // Something else listens on the port the run reports (a script of the workspace may print
    // any address): the run itself listens on none.
    let foreign = TcpListener::bind("127.0.0.1:0").unwrap();
    foreign.set_nonblocking(true).unwrap();
    let port = foreign.local_addr().unwrap().port();
    let run = run_server(
        &server.user,
        "liar",
        "--wait 600000",
        json!({"url": format!("http://127.0.0.1:{port}")}),
    );

    for extra in [&[][..], &["--port", &port.to_string()][..]] {
        let said = refused(pc.on("server", &[&["tunnel", "open", &run], extra].concat()));
        assert!(said.contains("refused:"), "{said}");
    }
    assert!(
        foreign.accept().is_err(),
        "the foreign listener was reached"
    );
    assert!(tunnels(&pc).is_empty());
}
