//! Links between the user's machines through their hub (`docs/remote.md`, "The transport", "The
//! hub", "When the hub is down"): daemons that listen nowhere, each with its own temporary HOME
//! and XDG folders, and a hub served in the test on 127.0.0.1 behind a proxy of the test's own,
//! which the daemons reach it through: the proxy records every byte both ways (what the hub
//! sees), may change the packets it carries (what a hub that tampers does), and may be cut (the
//! hub down). Fake agents and throwaway repositories.

use crate::common;
use crate::remote_deliveries::{child_on_server, release, script};
use crate::remote_launch::{
    Machine, allowing, eventually, prompted, runner_user, server_user, sign_in,
};

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use common::{Spawned, User};
use flottille_hub::{Hub, Served, SystemClock};
use flottille_link::Hello;
use serde_json::{Value, json};

/// How long a link, or the hub's news, may take.
const WAIT: Duration = Duration::from_secs(40);

/// A proxy between the daemons and the hub, as a reverse proxy in front of it would be.
struct Proxy {
    address: SocketAddr,
    /// Every byte it carried, both ways.
    recorded: Arc<Mutex<Vec<u8>>>,
    /// Whether it changes a byte of every packet a device sends for another.
    tampers: Arc<AtomicBool>,
    /// Whether it is cut: what it carried is closed, and what comes is closed at once.
    cut: Arc<AtomicBool>,
    /// The sender it names in place of another, in what the hub sends the devices, if any: what
    /// a hub that passes one member for another does.
    retags: Arc<Mutex<Option<(Value, Value)>>>,
    streams: Arc<Mutex<Vec<TcpStream>>>,
}

impl Proxy {
    fn to(hub: SocketAddr) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = Proxy {
            address: listener.local_addr().unwrap(),
            recorded: Arc::default(),
            tampers: Arc::default(),
            cut: Arc::default(),
            retags: Arc::default(),
            streams: Arc::default(),
        };
        let (recorded, tampers, cut, retags, streams) = (
            proxy.recorded.clone(),
            proxy.tampers.clone(),
            proxy.cut.clone(),
            proxy.retags.clone(),
            proxy.streams.clone(),
        );
        std::thread::spawn(move || {
            for device in listener.incoming() {
                let Ok(device) = device else { return };
                if cut.load(Ordering::SeqCst) {
                    continue;
                }
                let Ok(hub) = TcpStream::connect(hub) else {
                    continue;
                };
                streams
                    .lock()
                    .unwrap()
                    .extend([device.try_clone().unwrap(), hub.try_clone().unwrap()]);
                let (recorded_up, tampers) = (recorded.clone(), tampers.clone());
                let (device_in, mut hub_out) =
                    (device.try_clone().unwrap(), hub.try_clone().unwrap());
                std::thread::spawn(move || {
                    let mut lines = BufReader::new(device_in);
                    let mut line = Vec::new();
                    while lines
                        .read_until(b'\n', &mut line)
                        .is_ok_and(|read| read > 0)
                    {
                        recorded_up.lock().unwrap().extend_from_slice(&line);
                        if tampers.load(Ordering::SeqCst) {
                            line = tampered(&line);
                        }
                        if hub_out.write_all(&line).is_err() {
                            break;
                        }
                        line.clear();
                    }
                    let _ = hub_out.shutdown(Shutdown::Both);
                });
                let (hub_in, mut device_out) = (hub, device);
                let (recorded_down, retags) = (recorded.clone(), retags.clone());
                std::thread::spawn(move || {
                    let mut lines = BufReader::new(hub_in);
                    let mut line = Vec::new();
                    while lines
                        .read_until(b'\n', &mut line)
                        .is_ok_and(|read| read > 0)
                    {
                        recorded_down.lock().unwrap().extend_from_slice(&line);
                        if let Some((from, to)) = retags.lock().unwrap().as_ref() {
                            line = retagged(&line, from, to);
                        }
                        if device_out.write_all(&line).is_err() {
                            break;
                        }
                        line.clear();
                    }
                    let _ = device_out.shutdown(Shutdown::Both);
                });
            }
        });
        proxy
    }

    /// The hub down: every session through the proxy closed, and every new one.
    fn cut(&self) {
        self.cut.store(true, Ordering::SeqCst);
        for stream in self.streams.lock().unwrap().drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    fn recorded(&self) -> Vec<u8> {
        self.recorded.lock().unwrap().clone()
    }
}

/// A line a device sent, with one byte changed in the packet it carries for another device, if it
/// carries one.
fn tampered(line: &[u8]) -> Vec<u8> {
    let Ok(mut frame) = serde_json::from_slice::<Value>(line) else {
        return line.to_vec();
    };
    let Some(packet) = frame["relay"]["packet"].as_str() else {
        return line.to_vec();
    };
    let mut bytes = base64::engine::general_purpose::STANDARD
        .decode(packet)
        .unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x5a;
    frame["relay"]["packet"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
    let mut changed = serde_json::to_vec(&frame).unwrap();
    changed.push(b'\n');
    changed
}

/// A line the hub sent, said to be from `to` if it carries a packet from `from`.
fn retagged(line: &[u8], from: &Value, to: &Value) -> Vec<u8> {
    let Ok(mut frame) = serde_json::from_slice::<Value>(line) else {
        return line.to_vec();
    };
    if frame["relayed"]["from"] != *from {
        return line.to_vec();
    }
    frame["relayed"]["from"] = to.clone();
    let mut changed = serde_json::to_vec(&frame).unwrap();
    changed.push(b'\n');
    changed
}

/// Every packet of a link the recorded bytes carry, either way, as the hub holds it.
fn packets_in(recorded: &[u8]) -> Vec<Vec<u8>> {
    recorded
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter_map(|frame| {
            let packet = frame["relay"]["packet"]
                .as_str()
                .or_else(|| frame["relayed"]["packet"].as_str())?
                .to_owned();
            base64::engine::general_purpose::STANDARD
                .decode(packet)
                .ok()
        })
        .collect()
}

fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The hub, served from a temporary state folder in a runtime of the test's own, and the proxy
/// the daemons reach it through.
struct TestHub {
    _runtime: tokio::runtime::Runtime,
    _folder: tempfile::TempDir,
    served: Served<SystemClock>,
    proxy: Proxy,
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
        let proxy = Proxy::to(served.address());
        TestHub {
            _runtime: runtime,
            _folder: folder,
            served,
            proxy,
        }
    }

    /// The hub's URL as the daemons reach it: the proxy's.
    fn url(&self) -> String {
        format!("http://{}", self.proxy.address)
    }

    /// Pair mode opened, as `flottille-hub pair` opens it on the hub's host: its code, which holds
    /// the proxy's URL.
    fn code(&self) -> String {
        let url = self.url();
        self.served
            .with_hub(|hub| hub.open_pairing(&url, None).unwrap().1.code().unwrap())
    }
}

/// `user`'s machine with remote on and nothing set to listen on: no address, no port.
fn machine(user: User) -> Machine {
    std::fs::write(
        user.path("config/flottille/daemon.toml"),
        "[remote]\non = true\n",
    )
    .unwrap();
    user.status();
    Machine { user }
}

/// `machine` paired with `hub` as the user pairs it, as `name`, a runner.
fn pair(hub: &TestHub, machine: &Machine, name: &str) {
    let output = machine.user.run_with_input(
        &["remote", "pair", "--name", name, "--role", "runner"],
        &format!("{}\n", hub.code()),
    );
    assert!(
        output.status.success(),
        "the pairing failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        machine.log()
    );
}

/// A hub, `pc` (`pc_user`'s machine) its first member and `server` (`server_user`'s machine)
/// paired with it as a runner: neither listens, and each says the other online.
fn pc_and_server(pc_user: User, server_user: User) -> (TestHub, Machine, Machine) {
    let hub = TestHub::new();
    let pc = machine(pc_user);
    pair(&hub, &pc, "pc");
    let server = machine(server_user);
    pair(&hub, &server, "server");
    pc.wait_for("server");
    server.wait_for("pc");
    (hub, pc, server)
}

/// A command of `from`'s command line aimed at the machine `name`.
fn on(from: &Machine, name: &str, args: &[&str]) -> Output {
    from.user.run(&[&["--machine", name], args].concat())
}

fn json_of(output: Output) -> Value {
    assert!(
        output.status.success(),
        "failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// A project `name` on `user`'s machine over a throwaway repository with one commit.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "a project\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

#[test]
fn two_devices_with_no_listen_link_through_the_hub() {
    let (_hub, pc, server) = pc_and_server(User::new(), User::new());
    // Neither listens anywhere, nor has an address to give.
    for machine in [&pc, &server] {
        let this = machine.this();
        assert_eq!(this["listening"], Value::Null, "{this}");
        assert_eq!(this["addresses"], json!([]), "{this}");
    }
    assert_eq!(pc.seen("server")["reached"]["address"], "the hub");

    // A request from the pc's command line to the server, and its answer.
    let status = json_of(on(&pc, "server", &["status", "--json"]));
    assert_eq!(status["pid"], server.user.status()["pid"]);
    assert!(
        server.log().contains("asked by pc from pc"),
        "{}",
        server.log()
    );
}

#[test]
fn the_hub_never_holds_what_it_carries_in_clear() {
    let (hub, pc, server) = pc_and_server(User::new(), User::new());
    project(&server.user, "lantern-harbour");

    let listed = json_of(on(&pc, "server", &["project", "list", "--json"]));
    assert!(listed.to_string().contains("lantern-harbour"), "{listed}");

    // Every byte the hub was sent and sent back: the packets of the link are there, and neither
    // the request nor its answer can be read in them.
    let recorded = hub.proxy.recorded();
    let packets = packets_in(&recorded);
    assert!(packets.len() > 10, "{} packets carried", packets.len());
    for clear in [&b"lantern-harbour"[..], b"project.list"] {
        assert!(!holds(&recorded, clear));
        assert!(
            !packets.iter().any(|packet| holds(packet, clear)),
            "a packet holds {}",
            String::from_utf8_lossy(clear)
        );
    }
}

#[test]
fn the_hub_cannot_change_what_it_carries() {
    let (hub, pc, server) = pc_and_server(User::new(), User::new());
    hub.proxy.tampers.store(true, Ordering::SeqCst);

    // Every packet changed on its way is refused by the machine it is for: no request is served,
    // and the link ends rather than carry what the hub made.
    let asked = on(&pc, "server", &["status", "--json"]);
    assert!(!asked.status.success());
    assert!(!server.log().contains("asked by pc"), "{}", server.log());
    eventually("the link ended", || pc.seen("server")["state"] != "online");
}

#[test]
fn the_hub_cannot_pass_one_member_for_another() {
    let (hub, pc, server) = pc_and_server(User::new(), User::new());
    let laptop = machine(User::new());
    pair(&hub, &laptop, "laptop");
    server.wait_for("laptop");
    pc.wait_for("laptop");
    // What the pc sends the server, the hub says is the laptop's.
    *hub.proxy.retags.lock().unwrap() = Some((json!(pc.key()), json!(laptop.key())));

    // Whatever comes of the link, the server serves nothing as the laptop's: who a link is from
    // is the key its TLS proved, never the hub's word.
    let _ = on(&pc, "server", &["status", "--json"]);
    let log = server.log();
    assert!(
        !log.contains("asked by laptop") && !log.contains("from laptop"),
        "{log}"
    );
}

#[test]
fn a_link_to_a_member_not_online_says_it_is_offline() {
    let (_hub, pc, server) = pc_and_server(User::new(), User::new());
    let stopped = server.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", server.log());

    eventually("the server said offline", || {
        let seen = pc.seen("server");
        seen["state"] == "offline"
            && seen["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("server is offline"))
    });
    let refused = on(&pc, "server", &["status", "--json"]);
    assert!(!refused.status.success());
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("server is offline"), "{said}");
}

/// A run of the fake HTTP server on `user`'s machine, task `web` of the project `web`, on a free
/// port once it says it listens: the run and the port.
fn web_on(user: &User) -> (String, u16) {
    project(user, "web");
    let line = format!("{} --listen 0", common::fake_server().display());
    user.call(
        "task.add",
        json!({"project": "web", "name": "web", "line": line, "check": true}),
    )
    .unwrap();
    let run = user
        .call("task.run", json!({"project": "web", "name": "web"}))
        .unwrap()["run"]
        .as_str()
        .unwrap()
        .to_owned();
    let port = std::cell::Cell::new(0);
    eventually("the web server listens", || {
        let output = user.call("run.output", json!({"run": run})).unwrap();
        let text = output["output"].as_str().unwrap_or_default().to_owned();
        match text
            .lines()
            .find_map(|line| line.strip_prefix("listening on http://"))
        {
            Some(found) => {
                port.set(found.rsplit(':').next().unwrap().parse().unwrap());
                true
            }
            None => false,
        }
    });
    (run, port.get())
}

/// One HTTP request on `port` of this machine, and the whole answer.
fn get(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: web\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

#[test]
fn a_tunnel_and_a_git_transfer_work_through_the_hub() {
    let pc_user = runner_user();
    sign_in(&pc_user);
    let (_hub, pc, server) = pc_and_server(pc_user, server_user(allowing()));

    // A tunnel to a run of the server serves a page on the pc.
    let (run, _) = web_on(&server.user);
    let mut command = pc
        .user
        .command(&["--machine", "server", "tunnel", "open", &run, "--json"]);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut tunnel = Spawned::new(command);
    let mut first = String::new();
    BufReader::new(tunnel.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    let opened: Value = serde_json::from_str(&first)
        .unwrap_or_else(|_| panic!("no tunnel opened: {first:?}\n{}", pc.log()));
    let answer = get(opened["local_port"].as_u64().unwrap() as u16);
    assert!(answer.starts_with("HTTP/1.1 204"), "{answer}");
    let _ = tunnel.kill();

    // A session started on the server takes the pc's code there, and its commits come back.
    let repo = pc.folder();
    pc.user.git_init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    pc.git(&repo, &["add", "."]);
    pc.git(&repo, &["commit", "--quiet", "-m", "first"]);
    pc.git(&repo, &["checkout", "--quiet", "-b", "feat/export"]);
    pc.register("");
    server.register("");
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "Tests committed."}}}
        ]}]),
    );
    let (child, launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
    let there = std::path::PathBuf::from(server.agent(&child)["folder"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(there.join("a.txt")).unwrap(),
        "a\n",
        "the pc's code is there"
    );
    std::fs::write(there.join("tests.txt"), "the tests\n").unwrap();
    server.git(&there, &["add", "tests.txt"]);
    server.git(&there, &["commit", "--quiet", "-m", "the export tests"]);
    release(&server);

    prompted(&pc, "Tests committed.");
    let reference = format!("refs/flottille/machines/server/{launch}/feat/export");
    assert_eq!(
        pc.git(&pc.folder(), &["log", "-1", "--format=%s", &reference]),
        "the export tests"
    );
}

#[test]
fn the_hub_down_ends_the_links_and_says_so() {
    let (hub, pc, server) = pc_and_server(User::new(), User::new());

    hub.proxy.cut();

    // The link ends on both sides, and each says the hub is unreachable since when.
    for (machine, other) in [(&pc, "server"), (&server, "pc")] {
        eventually(
            &format!("{other} offline, the hub said unreachable"),
            || {
                let seen = machine.seen(other);
                seen["state"] == "offline"
                    && seen["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("is unreachable since"))
            },
        );
    }
    let hub_seen = pc.user.call("remote.machines", json!({})).unwrap()["hub"].clone();
    assert!(
        hub_seen["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("is unreachable since")),
        "{hub_seen}"
    );
    let refused = on(&pc, "server", &["status", "--json"]);
    assert!(!refused.status.success());
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("is unreachable since"), "{said}");
}
