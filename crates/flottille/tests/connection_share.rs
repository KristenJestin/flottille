//! A connection shared with another machine of the user's, and signed in from a client (#833,
//! `docs/remote.md`, "Connections"): two daemons paired through a hub served in the test, each
//! with its own temporary folders and in-memory keyring, and the fake MCP server over HTTP with
//! its fake authorization server. The browser is played by the test: a GET that follows nothing.
//!
//! Both daemons share one host: the server's sign-in listens on `127.0.0.2` there
//! (`FLOTTILLE_TEST_SIGN_IN_LOOPBACK`), so that the client's tunnel can take the very port of the
//! redirect on `127.0.0.1`, as it does on two machines.

use crate::common;
use crate::remote;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::remote::{Machine, eventually, pc_and_server, signed};
use common::User;
use serde_json::{Value, json};

/// The secret of the entry the connection sends: what never shows but in a keyring.
const SECRET: &str = "sh4red-notes-k3y-secret";
/// The OAuth tokens of the connection on pc, kept as its sign-in keeps them.
const TOKENS: &str = "pc-0wn-oauth-t0kens-never-travel";

fn server_user() -> User {
    User::new().with_env("FLOTTILLE_TEST_SIGN_IN_LOOPBACK", "127.0.0.2")
}

fn turn_on(user: &User) {
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
}

/// `notes` on pc: an `https://` server whose requests carry the entry `notes-key`. Connections
/// stay off, so that nothing tries to reach it.
fn remote_notes(pc: &Machine) {
    pc.user
        .call(
            "vault.set",
            json!({"name": "notes-key", "secret": SECRET,
                "url": "https://notes.example.com", "description": "The notes server's key."}),
        )
        .unwrap();
    pc.user
        .call(
            "connection.add",
            json!({"name": "notes", "about": "The user's notes.",
                "url": "https://notes.example.com/mcp",
                "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                    "entry": "notes-key"}]}),
        )
        .unwrap();
}

fn connections(user: &User) -> Vec<Value> {
    user.call("connection.list", json!({})).unwrap()["connections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn entries(user: &User) -> Vec<Value> {
    user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every file under the user's folders, as text: where a secret must not be.
fn every_file(user: &User) -> String {
    fn walk(path: &Path, into: &mut String) {
        let Ok(read) = std::fs::read_dir(path) else {
            return;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_symlink() {
                continue;
            }
            if path.is_dir() {
                walk(&path, into);
            } else if let Ok(bytes) = std::fs::read(&path) {
                into.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }
    let mut all = String::new();
    for folder in ["state", "config", "data", "home"] {
        walk(&user.path(folder), &mut all);
    }
    all
}

fn share(pc: &Machine, args: &[&str]) -> std::process::Output {
    pc.user.run(
        &[
            &["connection", "share", "notes", "--machine", "server"],
            args,
        ]
        .concat(),
    )
}

fn said(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_remote_connection_is_shared_with_its_named_entries() {
    let (_hub, pc, server) = pc_and_server(server_user());
    remote_notes(&pc);

    // An entry the connection does not name is not shared through it.
    pc.user
        .call("vault.set", json!({"name": "other-key", "secret": SECRET}))
        .unwrap();
    let refused = share(&pc, &["--entry", "other-key"]);
    assert!(!refused.status.success());
    assert!(
        said(&refused).contains("the connection notes names no vault entry other-key"),
        "{}",
        said(&refused)
    );
    assert!(connections(&server.user).is_empty());

    let output = share(&pc, &["--entry", "notes-key"]);
    assert!(output.status.success(), "{}\n{}", said(&output), pc.log());
    assert!(
        said(&output).contains("Shared the connection notes with server"),
        "{}",
        said(&output)
    );

    // The declaration, as pc declared it.
    let shared = connections(&server.user);
    assert_eq!(shared.len(), 1, "{shared:?}");
    assert_eq!(shared[0]["name"], "notes");
    assert_eq!(shared[0]["url"], "https://notes.example.com/mcp");
    assert_eq!(shared[0]["about"], "The user's notes.");
    assert_eq!(
        shared[0]["secrets"],
        json!([{"in": "header", "name": "Authorization", "prefix": "Bearer ",
            "entry": "notes-key"}])
    );
    // The entry it names, with its metadata, marked as pc's copy; nothing else of pc's vault.
    let copied = entries(&server.user);
    assert_eq!(copied.len(), 1, "{copied:?}");
    assert_eq!(copied[0]["name"], "notes-key");
    assert_eq!(copied[0]["url"], "https://notes.example.com");
    assert_eq!(copied[0]["shared_from"], "pc");
    // The secret is in the server's keyring only: in no file, no answer, no log of either side.
    for machine in [&pc, &server] {
        assert!(!every_file(&machine.user).contains(SECRET));
    }
    assert!(!said(&output).contains(SECRET));
    assert!(!server.log().contains(SECRET));
    assert!(
        server
            .log()
            .contains("the connection notes was shared from pc"),
        "{}",
        server.log()
    );

    // A connection the server has already is not replaced.
    let again = share(&pc, &["--entry", "notes-key"]);
    assert!(!again.status.success());
    assert!(
        said(&again).contains("server has its own connection notes"),
        "{}",
        said(&again)
    );
}

#[test]
fn a_local_connection_is_refused_for_sharing() {
    let (_hub, pc, server) = pc_and_server(server_user());
    pc.user
        .call(
            "connection.add",
            json!({"name": "desk", "about": "The desktop.", "command": ["desk-mcp", "serve"]}),
        )
        .unwrap();
    pc.user
        .call(
            "connection.add",
            json!({"name": "memo", "about": "A memo server.", "url": "http://127.0.0.1:9/mcp"}),
        )
        .unwrap();
    // An https:// address of this machine is its own too.
    pc.user
        .call(
            "connection.add",
            json!({"name": "loop", "about": "A loopback server.", "enabled": false,
                "url": "https://localhost:9443/mcp"}),
        )
        .unwrap();
    for name in ["desk", "memo", "loop"] {
        let output = pc
            .user
            .run(&["connection", "share", name, "--machine", "server"]);
        assert!(!output.status.success(), "{name} was shared");
        assert!(
            said(&output).contains(&format!(
                "the connection {name} is this machine's own: only an https:// connection is shared"
            )),
            "{name}: {}",
            said(&output)
        );
    }
    assert!(connections(&server.user).is_empty());
}

#[test]
fn oauth_tokens_are_never_shared() {
    let (_hub, pc, server) = pc_and_server(server_user());
    remote_notes(&pc);
    // The tokens of pc's own sign-in, where its sign-in keeps them.
    pc.user
        .call(
            "vault.set",
            json!({"name": "notes.oauth", "secret": TOKENS, "for": "connection:notes"}),
        )
        .unwrap();

    // Asked for by name, they are refused: each machine signs in for itself.
    let refused = share(&pc, &["--entry", "notes-key", "--entry", "notes.oauth"]);
    assert!(!refused.status.success());
    assert!(
        said(&refused).contains("OAuth tokens are never shared: server signs in for itself"),
        "{}",
        said(&refused)
    );
    assert!(connections(&server.user).is_empty());

    // A device's signed request that names them, carried by pc as the window would: refused by
    // pc before anything leaves it.
    let line = signed(
        &pc.device(),
        "s1",
        "pc",
        "connection.share",
        json!({"name": "notes", "machine": "server", "entries": ["notes.oauth"]}),
    );
    let answer = pc.raw(&line);
    assert!(
        remote::error_of(&answer).contains("OAuth tokens are never shared"),
        "{answer}"
    );

    let output = share(&pc, &["--entry", "notes-key"]);
    assert!(output.status.success(), "{}", said(&output));
    let copied: Vec<String> = entries(&server.user)
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(copied, vec!["notes-key".to_owned()]);
    assert!(!every_file(&server.user).contains(TOKENS));
    assert!(!server.log().contains(TOKENS));
    // pc keeps its own.
    assert!(
        entries(&pc.user)
            .iter()
            .any(|entry| entry["name"] == "notes.oauth")
    );
}

// ── Signing in from a client ─────────────────────────────────────────────────────────────────

fn fake_mcp() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(
        path.exists(),
        "{} is missing: `cargo build -p flottille-fake-mcp` first",
        path.display()
    );
    path
}

/// The fake MCP server of the server's, protected by its fake authorization server, on
/// Streamable HTTP over loopback: the process and its URL. The server declares it as its own.
fn protected_server(user: &User) -> (common::Spawned, String) {
    let script = json!({
        "server": {"name": "notes", "version": "1.0.0"},
        "tools": [{"name": "search", "description": "Finds notes by words.",
            "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}}],
        "answers": {"search": {"text": "found"}},
        "oauth": {"tokens_file": user.path("notes-tokens")},
    });
    let script_file = user.path("notes-script.json");
    std::fs::write(&script_file, script.to_string()).unwrap();
    let port_file = user.path("notes-port");
    let server = common::Spawned::new({
        let mut command = std::process::Command::new(fake_mcp());
        command.args([
            "--script",
            script_file.to_str().unwrap(),
            "--log",
            user.path("notes-log.jsonl").to_str().unwrap(),
            "--http",
            port_file.to_str().unwrap(),
        ]);
        command
    });
    eventually("the server listens", || {
        port_file.exists() && !std::fs::read_to_string(&port_file).unwrap().is_empty()
    });
    let port = std::fs::read_to_string(&port_file).unwrap();
    (server, format!("http://127.0.0.1:{port}/mcp"))
}

/// What the fake's authorization server logged of its token requests.
fn grants(user: &User) -> Vec<Value> {
    std::fs::read_to_string(user.path("notes-log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["event"] == "token" && line["grant"] == "authorization_code")
        .collect()
}

/// A GET as the browser makes it, following nothing: its status, its `Location` and its body.
fn browse(address: &str) -> (u16, Option<String>, String) {
    use std::io::{Read, Write};
    let url = url::Url::parse(address).unwrap();
    let mut stream = std::net::TcpStream::connect((
        url.host_str().unwrap(),
        url.port_or_known_default().unwrap(),
    ))
    .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let target = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        url.host_str().unwrap()
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let location = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("location")
            .then(|| value.trim().to_owned())
    });
    (status, location, body.to_owned())
}

/// The same redirect with its `state` replaced.
fn with_state(redirect: &str, state: &str) -> String {
    let mut url = url::Url::parse(redirect).unwrap();
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "state")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let mut pairs = url.query_pairs_mut();
    pairs.clear();
    for (key, value) in &kept {
        pairs.append_pair(key, value);
    }
    pairs.append_pair("state", state);
    drop(pairs);
    url.to_string()
}

/// The server's own `notes`, protected, declared with Connections on.
fn server_notes(server: &Machine) -> common::Spawned {
    let (process, url) = protected_server(&server.user);
    turn_on(&server.user);
    server
        .user
        .call(
            "connection.add",
            json!({"name": "notes", "about": "The user's notes.", "url": url}),
        )
        .unwrap();
    process
}

/// `flottille --machine server connection login notes --no-browser` on pc, running: the command,
/// what it printed until the address, and the address its browser would open.
fn login_from_pc(
    pc: &Machine,
) -> (
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    String,
) {
    let mut child = pc
        .user
        .command(&[
            "--machine",
            "server",
            "connection",
            "login",
            "notes",
            "--no-browser",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let url = line
        .split_whitespace()
        .find(|word| word.starts_with("http"))
        .unwrap_or_else(|| {
            let mut rest = String::new();
            let _ = std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut rest);
            panic!("no address in {line:?}: {rest}\n{}", pc.log())
        })
        .to_owned();
    (child, out, url)
}

/// The redirect lands on pc's tunnel to the server's sign-in: pc carries one, on the very port
/// the redirect names.
fn carried_by_pc(pc: &Machine, redirect: &str) {
    let port = url::Url::parse(redirect).unwrap().port().unwrap();
    let tunnels = pc.user.call("tunnel.list", json!({})).unwrap()["tunnels"].clone();
    assert!(
        tunnels.as_array().unwrap().iter().any(|tunnel| {
            tunnel["sign_in"] == "notes"
                && tunnel["machine"] == "server"
                && tunnel["local_port"] == port
        }),
        "{tunnels}"
    );
}

#[test]
fn sign_in_from_a_client_lands_the_redirect_on_the_other_machine() {
    let (_hub, pc, server) = pc_and_server(server_user());
    let _process = server_notes(&server);

    let (child, mut out, url) = login_from_pc(&pc);
    let (status, location, body) = browse(&url);
    assert_eq!(status, 302, "{body}");
    let redirect = location.unwrap();
    // The redirect is the server's one-shot listener's address, on pc's own loopback: pc's
    // tunnel holds that port there, and the server's listener is not on it (`127.0.0.2`).
    assert!(redirect.starts_with("http://127.0.0.1:"), "{redirect}");
    carried_by_pc(&pc, &redirect);
    let (status, _, body) = browse(&redirect);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("signed in"), "{body}");

    let output = child.wait_with_output().unwrap();
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    assert!(output.status.success(), "{rest}\n{}", pc.log());
    assert!(
        rest.contains("Signed in to the connection notes on server"),
        "{rest}"
    );
    // The tokens are the server's, in its keyring; pc holds none.
    assert!(
        entries(&server.user)
            .iter()
            .any(|entry| entry["name"] == "notes.oauth")
    );
    assert!(
        !entries(&pc.user)
            .iter()
            .any(|entry| entry["name"] == "notes.oauth")
    );
    eventually("the server's notes is signed in", || {
        let listed = connections(&server.user);
        listed[0]["signed_out"].is_null() && listed[0]["state"] == "ready"
    });
    // The tunnel ended with the sign-in.
    eventually("the tunnel is closed", || {
        pc.user.call("tunnel.list", json!({})).unwrap()["tunnels"]
            .as_array()
            .unwrap()
            .is_empty()
    });
}

#[test]
fn a_redirect_with_another_state_is_still_refused_through_the_tunnel() {
    let (_hub, pc, server) = pc_and_server(server_user());
    let _process = server_notes(&server);

    let (child, mut out, url) = login_from_pc(&pc);
    let (_, location, _) = browse(&url);
    let redirect = location.unwrap();
    carried_by_pc(&pc, &redirect);
    // Another state, through the tunnel: refused by the server's listener, its code never
    // exchanged, and the sign-in still waits.
    let (status, _, body) = browse(&with_state(&redirect, "forged-state"));
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("This is not the end of a sign-in Flottille began"),
        "{body}"
    );
    assert!(grants(&server.user).is_empty());
    // The user's own redirect still lands.
    let (status, _, body) = browse(&redirect);
    assert_eq!(status, 200, "{body}");
    let output = child.wait_with_output().unwrap();
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    assert!(output.status.success(), "{rest}");
    assert_eq!(grants(&server.user).len(), 1);
}

#[test]
fn a_sign_in_tunnel_reaches_only_a_sign_in_its_device_began() {
    let (_hub, pc, server) = pc_and_server(server_user());
    let _process = server_notes(&server);
    let open = |id: &str| {
        pc.raw(&signed(
            &pc.device(),
            id,
            "server",
            "tunnel.open",
            json!({"sign_in": "notes"}),
        ))
    };
    // No sign-in waits: nothing to reach.
    let answer = open("t1");
    assert!(
        remote::error_of(&answer).contains("no sign-in of the connection notes waits on server"),
        "{answer}"
    );
    // One the server's own user began, from its own client: not pc's device's to reach.
    server
        .user
        .call_within(
            "connection.login",
            json!({"name": "notes"}),
            Duration::from_secs(40),
        )
        .unwrap();
    let answer = open("t2");
    assert!(
        remote::error_of(&answer).contains("the sign-in of notes was not begun by pc"),
        "{answer}"
    );
}
