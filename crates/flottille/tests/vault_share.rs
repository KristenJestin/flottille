//! A vault entry shared with another machine of the user's (`docs/remote.md`, "Vault entries",
//! `docs/vault.md`, "Shared with another machine"): the user's device signs `vault.share` on the
//! machine that holds the entry, whose daemon reads the secret from its keyring and sends it, with
//! the entry's metadata and that signature, in a `share` message; the other machine checks the
//! signature itself and writes the secret into its own keyring. Two daemons and a hub served in
//! the test, each daemon with its own temporary folders and the in-memory keyring of its
//! configuration (but where a test plays a machine with none); fake agents.

mod common;
mod remote;

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::process::Output;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_link::{DeviceKey, Endpoint, Hello, MachineKey, PublicKey, Request, Traffic};
use remote::{Machine, NoKeyring, WAIT, error_of, json_of, pc_and_server, signed};
use serde_json::{Value, json};

/// The entry's secret: invented, long enough to be masked.
const SECRET: &str = "sh4red-t0ken-9Qx2";
/// The secret it is set to before it is shared again.
const SECOND: &str = "s3cond-t0ken-Lm4P";

/// A local API that answers every request, the `Authorization` each one carried kept in order.
struct Site {
    addr: SocketAddr,
    authorizations: Arc<Mutex<Vec<String>>>,
}

impl Site {
    fn start() -> Site {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let authorizations = Arc::new(Mutex::new(Vec::new()));
        let kept = authorizations.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut authorization = String::new();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = header.trim_end().split_once(':')
                        && name.eq_ignore_ascii_case("authorization")
                    {
                        authorization = value.trim().to_owned();
                    }
                }
                kept.lock().unwrap().push(authorization);
                let body = "{\"fields\":[]}";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                     connection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Site {
            addr,
            authorizations,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn authorizations(&self) -> Vec<String> {
        self.authorizations.lock().unwrap().clone()
    }
}

/// `flottille vault share <name> --machine <machine> --json` on `from`.
fn share(from: &Machine, name: &str, machine: &str) -> Output {
    from.user
        .run(&["vault", "share", name, "--machine", machine, "--json"])
}

fn failed(output: &Output) -> String {
    assert!(
        !output.status.success(),
        "it succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn entries(machine: &Machine) -> Vec<Value> {
    machine.user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

fn entry(machine: &Machine, name: &str) -> Option<Value> {
    entries(machine).into_iter().find(|entry| entry["name"] == name)
}

/// The project `name` on `user`'s machine, a throwaway repository.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

/// A session of the project `app` whose user's message names `url`: its tools.
fn session(user: &User, url: &str) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "message": format!("read the tracker's fields: {url}")}),
    )
    .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

/// Every file under `dir` holding `needle`, as a path.
fn files_holding(dir: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_holding(&path, needle));
        } else if let Ok(bytes) = std::fs::read(&path)
            && bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        {
            found.push(path.display().to_string());
        }
    }
    found
}

#[test]
fn a_shared_entry_is_in_the_other_machine_s_keyring_with_its_metadata() {
    let site = Site::start();
    let server_user =
        User::with_fake_agent().with_env("FLOTTILLE_TEST_WEB_OPEN", &site.addr.to_string());
    let (_hub, pc, server) = pc_and_server(server_user);
    project(&pc.user, "app");
    let set = |secret: &str| {
        pc.user
            .call(
                "vault.set",
                json!({"name": "tracker", "secret": secret, "description": "The tracker's API",
                    "url": site.url("/"), "project": "app"}),
            )
            .unwrap();
    };
    set(SECRET);

    let shared = json_of(share(&pc, "tracker", "server"));

    assert_eq!(shared["name"], "tracker", "{shared}");
    assert_eq!(shared["machine"], "server", "{shared}");
    let copy = entry(&server, "tracker").expect("the entry on the server");
    assert_eq!(copy["description"], "The tracker's API", "{copy}");
    assert_eq!(copy["url"], site.url("/"), "{copy}");
    assert_eq!(copy["project"], "app", "{copy}");
    assert_eq!(copy["shared_from"], "pc", "{copy}");
    // pc's own entry is pc's: it says nothing of where it went.
    assert!(entry(&pc, "tracker").unwrap()["shared_from"].is_null());

    // The server's keyring holds the secret: its tools sign in with it.
    project(&server.user, "app");
    let fields = site.url("/rest/api/3/field");
    let mcp = session(&server.user, &fields);
    let read = |mcp: &Mcp| {
        let answer = mcp.call(
            "web",
            json!({"action": "read", "url": fields, "entry": "tracker"}),
        );
        assert!(!answer.error, "{}", answer.text);
    };
    read(&mcp);
    assert_eq!(site.authorizations(), [format!("Bearer {SECRET}")]);

    // A copy, not kept in sync: sharing again replaces it.
    set(SECOND);
    json_of(share(&pc, "tracker", "server"));
    read(&mcp);
    assert_eq!(site.authorizations()[1], format!("Bearer {SECOND}"));
    assert_eq!(entries(&server).len(), 1);
}

#[test]
fn sharing_needs_the_device_s_signature() {
    let (_hub, pc, server) = pc_and_server(User::new());
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();
    let params = json!({"name": "tracker", "machine": "server"});

    // A client of pc that does not sign: its daemon alone cannot share.
    let (_, unsigned) = pc.user.call("vault.share", params.clone()).unwrap_err();
    assert!(
        unsigned.contains("signed by the user's device"),
        "{unsigned}"
    );
    // Signed by a key that is no member's.
    let stranger = DeviceKey::make().unwrap();
    let answer = pc.raw(&signed(
        &stranger,
        "1",
        "pc",
        "vault.share",
        params.clone(),
    ));
    assert_eq!(
        error_of(&answer),
        "refused: this request is not signed by a member"
    );
    assert!(entry(&server, "tracker").is_none());

    // Signed by the user's device, it is shared.
    let answer = pc.raw(&signed(&pc.device(), "2", "pc", "vault.share", params));
    assert_eq!(answer["result"]["machine"], "server", "{answer}");
    assert_eq!(entry(&server, "tracker").unwrap()["shared_from"], "pc");
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

/// A `share` message carrying `request`, the entry `name` and the secret.
fn share_message(request: Value, name: &str) -> Value {
    json!({"machine": {"share": {
        "request": request,
        "entry": {"name": name, "description": "The tracker's API"},
        "secret": SECRET,
    }}})
}

/// `vault.share` of `name` to `to`, signed by `key` for `machine` now.
fn share_request(key: &DeviceKey, machine: &str, method: &str, name: &str, to: &str) -> Value {
    serde_json::to_value(
        Request::sign(
            key,
            "1",
            machine,
            method,
            &json!({"name": name, "machine": to}),
            SystemTime::now(),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn a_machine_message_alone_cannot_share_an_entry() {
    let (_hub, pc, server) = pc_and_server(User::new());
    let server_key: PublicKey =
        serde_json::from_value(server.remote()["this"]["key"].clone()).unwrap();
    let address: SocketAddr = server.remote()["this"]["listening"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // pc's daemon is stopped, so that no link of its own takes the one below: pc's machine key is
    // only what the test holds, as an agent that read it would hold it.
    assert!(pc.user.run(&["daemon", "stop"]).status.success());
    server.wait_for("pc", "offline");
    let machine_key = MachineKey::load_or_make(&pc.folder()).unwrap();
    let apart = tempfile::tempdir().unwrap();
    let device = pc.device();
    let valid = share_request(&device, "pc", "vault.share", "tracker", "server");
    let frames = [
        // No request of the user's at all.
        json!({"machine": {"share": {"entry": {"name": "tracker"}, "secret": SECRET}}}),
        // A request the machine key signed, as if it were a device's.
        share_message(
            share_request(
                &machine_as_device(&pc.folder(), apart.path()),
                "pc",
                "vault.share",
                "tracker",
                "server",
            ),
            "tracker",
        ),
        // The device's signature over another method.
        share_message(
            share_request(&device, "pc", "vault.list", "tracker", "server"),
            "tracker",
        ),
        // The device's share of another entry, carrying this one.
        share_message(
            share_request(&device, "pc", "vault.share", "other", "server"),
            "tracker",
        ),
        // The device's share signed for the server itself, not for the machine that holds it.
        share_message(
            share_request(&device, "server", "vault.share", "tracker", "server"),
            "tracker",
        ),
        // The device's share for another machine.
        share_message(
            share_request(&device, "pc", "vault.share", "tracker", "laptop"),
            "tracker",
        ),
        // The device's share, then the same message again.
        share_message(valid.clone(), "tracker"),
        share_message(valid, "tracker"),
    ];
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let answers = runtime.block_on(async {
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
        let mut answers = Vec::new();
        for frame in &frames {
            answers.push(
                link.send_raw(frame.to_string().as_bytes())
                    .await
                    .map_err(|error| error.to_string()),
            );
        }
        endpoint.close().await;
        answers
    });

    let refusal = |at: usize| answers[at].clone().unwrap_err();
    assert_eq!(
        refusal(0),
        "server refused: not a message a machine may send",
        "{answers:?}"
    );
    assert_eq!(
        refusal(1),
        "server refused: refused: this request is not signed by a member"
    );
    assert!(
        refusal(2).contains("is not the user's share of a vault entry"),
        "{answers:?}"
    );
    assert!(
        refusal(3).contains("the user shared other, not tracker"),
        "{answers:?}"
    );
    assert!(
        refusal(4).contains("this request is for server, not for pc"),
        "{answers:?}"
    );
    assert!(
        refusal(5).contains("the user shared it with laptop, not with server"),
        "{answers:?}"
    );
    assert!(answers[6].is_ok(), "{answers:?}");
    assert_eq!(
        refusal(7),
        "server refused: refused: this request was received already"
    );
    // Only the one the device signed, once, is in the server's vault.
    let names: Vec<Value> = entries(&server).iter().map(|e| e["name"].clone()).collect();
    assert_eq!(names, [json!("tracker")]);
    for answer in &answers {
        let said = format!("{answer:?}");
        assert!(!said.contains(SECRET), "{said}");
    }
}

#[test]
fn the_secret_is_in_no_event_log_or_store_on_either_side() {
    let (_hub, pc, server) = pc_and_server(User::new());
    project(&pc.user, "app");
    pc.user
        .call(
            "vault.set",
            json!({"name": "tracker", "secret": SECRET, "username": "robin@example.test"}),
        )
        .unwrap();
    // One refused on the server: its project is not there.
    pc.user
        .call(
            "vault.set",
            json!({"name": "jira", "secret": SECOND, "username": "robin@example.test",
                "for": "worklog:app"}),
        )
        .unwrap();

    let shared = share(&pc, "tracker", "server");
    let refused = share(&pc, "jira", "server");

    let said = |output: &Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    };
    json_of(shared.clone());
    failed(&refused);
    for secret in [SECRET, SECOND] {
        assert!(!said(&shared).contains(secret));
        assert!(!said(&refused).contains(secret));
        for machine in [&pc, &server] {
            for folder in ["state", "config", "data", "home"] {
                let found = files_holding(&machine.user.path(folder), secret);
                assert!(found.is_empty(), "{secret} in {found:?}");
            }
            let events = machine
                .user
                .watch(&["--after", "0"])
                .within(Duration::from_secs(1));
            let events = serde_json::to_string(&events).unwrap();
            assert!(!events.contains(secret), "{events}");
            let listed = machine.user.call("vault.list", json!({})).unwrap();
            assert!(!listed.to_string().contains(secret), "{listed}");
        }
    }
    // The server says what came, and from where, never the value.
    assert!(
        server.log().contains("the vault entry tracker was shared from pc"),
        "{}",
        server.log()
    );
}

#[test]
fn a_worklog_entry_needs_its_project_on_the_other_machine() {
    let (_hub, pc, server) = pc_and_server(User::new());
    project(&pc.user, "app");
    pc.user
        .call(
            "vault.set",
            json!({"name": "jira", "secret": SECRET, "username": "robin@example.test",
                "for": "worklog:app"}),
        )
        .unwrap();

    let refused = failed(&share(&pc, "jira", "server"));
    assert!(
        refused.contains(
            "server refused: the vault entry jira is bound to the worklog of the project app, \
             which is not on this machine: add the project here first"
        ),
        "{refused}"
    );
    assert!(entry(&server, "jira").is_none());

    project(&server.user, "app");
    json_of(share(&pc, "jira", "server"));
    let copy = entry(&server, "jira").unwrap();
    assert_eq!(copy["for"], "worklog:app", "{copy}");
    assert_eq!(copy["project"], "app", "{copy}");
}

#[test]
fn a_machine_with_no_keyring_refuses_saying_so() {
    let (_hub, pc, server) = pc_and_server(User::new());
    // The server started again as a headless one: no memory keyring chosen, and no Secret Service
    // on its session bus (the test's own, where nothing listens).
    std::fs::remove_file(server.user.path("config/flottille/keyring.toml")).unwrap();
    assert!(server.user.run(&["daemon", "stop"]).status.success());
    server.user.status();
    pc.wait_for("server", "online");
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();

    let refused = failed(&share(&pc, "tracker", "server"));

    assert!(
        refused.contains("server refused: this machine has no keyring"),
        "{refused}"
    );
    assert!(entry(&server, "tracker").is_none());
}

#[test]
fn the_shared_value_is_masked_on_the_other_machine() {
    let server_user = User::with_fake_agent();
    server_user.fake_script(json!({"turns": [
        {"when": "tell", "do": [{"say": format!("the tracker token is {SECRET}, keep it")}]}
    ]}));
    let (_hub, pc, server) = pc_and_server(server_user);
    pc.user
        .call("vault.set", json!({"name": "tracker", "secret": SECRET}))
        .unwrap();
    json_of(share(&pc, "tracker", "server"));
    project(&server.user, "app");

    let answer = server
        .user
        .call("agent.run", json!({"project": "app", "message": "tell"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let after = answer["after"].as_u64().unwrap();
    let of = id.clone();
    server
        .user
        .watch(&["--after", &after.to_string()])
        .until(|e| e["data"]["agent"] == of.as_str() && e["name"] == "agent.turn_ended");

    let page = server
        .user
        .call("agent.read", json!({"agent": id, "after": 0}))
        .unwrap()
        .to_string();
    assert!(!page.contains(SECRET), "{page}");
    assert!(
        page.contains("the tracker token is [masked], keep it"),
        "{page}"
    );
}
