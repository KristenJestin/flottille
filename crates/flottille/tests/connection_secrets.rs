//! The secrets of a URL connection's requests (`docs/extensions.md`, "Connections"): each vault
//! entry goes in a header (with its prefix), a query parameter, a cookie or basic authentication,
//! read by the daemon and never shown to an agent. Against the real daemon with the fake ACP agent
//! and the fake MCP server over HTTP: no real server, the in-memory keyring, temporary folders.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// A secret that URL-encoding changes, so that both of its forms are looked for.
const SECRET: &str = "s3cret/t0ken+of=penpot";
const ENCODED: &str = "s3cret%2Ft0ken%2Bof%3Dpenpot";

fn fake_mcp() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(
        path.exists(),
        "{} is missing: `cargo build -p flottille-fake-mcp` first",
        path.display()
    );
    path
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "", "do": [{"say": "ready"}]}]}));
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    user
}

fn session(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call("agent.run", json!({"project": "app", "message": "start"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn log_file(user: &User) -> PathBuf {
    user.path("notes-log.jsonl")
}

/// The requests the fake server logged.
fn requests(user: &User) -> Vec<Value> {
    std::fs::read_to_string(log_file(user))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["event"] == "http")
        .collect()
}

/// The fake server over HTTP, with one action, `search`.
struct Served {
    server: common::Spawned,
    port: String,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

fn serve(user: &User) -> Served {
    let script = json!({
        "server": {"name": "notes", "version": "1.0.0"},
        "tools": [{"name": "search", "description": "Finds notes.",
            "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}}}],
        "answers": {"search": {"text": "found: the login decision"}},
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
            log_file(user).to_str().unwrap(),
            "--http",
            port_file.to_str().unwrap(),
        ]);
        command
    });
    eventually("the server listens", || {
        port_file.exists() && !std::fs::read_to_string(&port_file).unwrap().is_empty()
    });
    let port = std::fs::read_to_string(&port_file).unwrap();
    Served { server, port }
}

/// The connection `notes` declared on the fake server with `more`, reviewed, accepted, and its
/// action called by an agent: what the agent read, of the call and of `connections show`.
fn reached(user: &User, served: &Served, more: Value) -> Vec<String> {
    let mut params = json!({"name": "notes", "about": "Notes.",
        "url": format!("http://127.0.0.1:{}/mcp", served.port)});
    for (key, value) in more.as_object().into_iter().flatten() {
        params[key] = value.clone();
    }
    user.call("connection.add", params).unwrap();
    user.call_within(
        "connection.review",
        json!({"name": "notes"}),
        Duration::from_secs(40),
    )
    .unwrap_or_else(|e| panic!("{e:?} {:?}", requests(user)));
    user.call(
        "connection.accept",
        json!({"name": "notes", "actions": {"search": "passes"}}),
    )
    .unwrap();
    let mcp = session(user);
    let called = mcp.call(
        "conn_notes",
        json!({"action": "search", "arguments": {"query": "a"}}),
    );
    assert!(!called.error, "{}", called.text);
    assert!(
        called.text.contains("found: the login decision"),
        "{}",
        called.text
    );
    let shown = mcp.call("connections", json!({"action": "show", "name": "notes"}));
    vec![called.text, shown.text]
}

fn connection(user: &User) -> Value {
    user.call("connection.list", json!({})).unwrap()["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "notes")
        .cloned()
        .unwrap()
}

fn never_said(texts: &[String]) {
    for text in texts {
        assert!(!text.contains(SECRET) && !text.contains(ENCODED), "{text}");
    }
}

fn set(user: &User, entry: Value) {
    user.call("vault.set", entry).unwrap();
}

#[test]
fn a_secret_in_a_query_parameter_reaches_the_server_and_never_the_agent() {
    let user = user();
    let served = serve(&user);
    set(&user, json!({"name": "penpot-usertoken", "secret": SECRET}));
    let read = reached(
        &user,
        &served,
        json!({"secrets": [{"in": "query", "name": "userToken", "entry": "penpot-usertoken"}]}),
    );
    never_said(&read);
    let requests = requests(&user);
    assert!(!requests.is_empty());
    for request in &requests {
        assert_eq!(
            request["query"],
            format!("userToken={ENCODED}"),
            "{requests:?}"
        );
    }
}

#[test]
fn a_bearer_prefix_is_added_before_the_secret() {
    let user = user();
    let served = serve(&user);
    set(&user, json!({"name": "notes-token", "secret": SECRET}));
    let read = reached(
        &user,
        &served,
        json!({"secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
            "entry": "notes-token"}]}),
    );
    never_said(&read);
    let requests = requests(&user);
    assert!(
        requests
            .iter()
            .all(|r| r["headers"]["authorization"] == format!("Bearer {SECRET}")),
        "{requests:?}"
    );
}

#[test]
fn a_cookie_secret_reaches_the_server() {
    let user = user();
    let served = serve(&user);
    set(&user, json!({"name": "notes-session", "secret": SECRET}));
    let read = reached(
        &user,
        &served,
        json!({"secrets": [{"in": "cookie", "name": "session", "entry": "notes-session"}]}),
    );
    never_said(&read);
    let requests = requests(&user);
    assert!(
        requests
            .iter()
            .all(|r| r["headers"]["cookie"] == format!("session={SECRET}")),
        "{requests:?}"
    );
}

#[test]
fn basic_authentication_uses_the_entry_s_username_and_secret() {
    let user = user();
    let served = serve(&user);
    set(
        &user,
        json!({"name": "notes-login", "secret": SECRET, "username": "ada@example.com"}),
    );
    let read = reached(
        &user,
        &served,
        json!({"secrets": [{"in": "basic", "entry": "notes-login"}]}),
    );
    never_said(&read);
    let pair =
        base64::engine::general_purpose::STANDARD.encode(format!("ada@example.com:{SECRET}"));
    for text in &read {
        assert!(!text.contains(&pair), "{text}");
    }
    let requests = requests(&user);
    assert!(
        requests
            .iter()
            .all(|r| r["headers"]["authorization"] == format!("Basic {pair}")),
        "{requests:?}"
    );
}

#[test]
fn a_query_secret_is_masked_in_the_url_shown() {
    let user = user();
    set(&user, json!({"name": "penpot-usertoken", "secret": SECRET}));
    // No server listens there: the error says the request failed, never where it went.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}/mcp");
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "Notes.", "url": url,
            "secrets": [{"in": "query", "name": "userToken", "entry": "penpot-usertoken"}]}),
    )
    .unwrap();
    let failed = user
        .call_within(
            "connection.review",
            json!({"name": "notes"}),
            Duration::from_secs(40),
        )
        .unwrap_err();
    let listed = user.call("connection.list", json!({})).unwrap();
    let shown = connection(&user);
    assert_eq!(
        shown["url"], url,
        "the URL shown is the declared one: {shown}"
    );
    assert!(shown["failing"].is_string(), "{shown}");
    let file = std::fs::read_to_string(user.path("config/flottille/extensions.toml")).unwrap();
    never_said(&[format!("{failed:?}"), listed.to_string(), file]);
}

#[test]
fn a_url_with_a_token_in_its_query_is_refused() {
    let user = user();
    for url in [
        "https://design.example.com/mcp?userToken=abc123",
        "https://design.example.com/mcp?page=2&API_KEY=abc123",
        "https://design.example.com/mcp?Authorization=abc123",
    ] {
        let refused = user
            .call(
                "connection.add",
                json!({"name": "design", "about": "Designs.", "url": url}),
            )
            .unwrap_err();
        assert!(refused.1.contains("Secrets of its requests"), "{refused:?}");
        assert!(!refused.1.contains("abc123"), "{refused:?}");
    }
    user.call(
        "connection.add",
        json!({"name": "design", "about": "Designs.",
            "url": "https://design.example.com/mcp?page=2"}),
    )
    .unwrap();
}

#[test]
fn old_headers_still_work() {
    let user = user();
    let served = serve(&user);
    set(&user, json!({"name": "notes-key", "secret": SECRET}));
    let read = reached(
        &user,
        &served,
        json!({"headers": {"X-Notes-Key": {"vault": "notes-key"}}}),
    );
    never_said(&read);
    let requests = requests(&user);
    assert!(
        requests
            .iter()
            .all(|r| r["headers"]["x-notes-key"] == SECRET),
        "{requests:?}"
    );
    // Written back as it was: the pin's digest does not move.
    let shown = connection(&user);
    assert_eq!(
        shown["headers"],
        json!({"X-Notes-Key": {"vault": "notes-key"}}),
        "{shown}"
    );
    assert!(shown.get("secrets").is_none(), "{shown}");
    assert_eq!(shown["state"], "ready", "{shown}");
}

/// Connections written to `extensions.toml` by hand, beside what the daemon wrote there.
fn written_by_hand(user: &User, tables: &str) {
    let path = user.path("config/flottille/extensions.toml");
    let mut file = std::fs::read_to_string(&path).unwrap_or_default();
    file.push_str(tables);
    std::fs::write(&path, file).unwrap();
}

#[test]
fn an_existing_file_with_a_token_like_query_fails_only_that_connection() {
    let user = user();
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "Notes.", "url": "https://notes.example.com/mcp"}),
    )
    .unwrap();
    written_by_hand(
        &user,
        "\n[[connections.server]]\nname = \"design\"\nabout = \"Designs.\"\n\
         url = \"https://design.example.com/mcp?userToken=abc123\"\n\
         \n[[connections.server]]\nname = \"sketch\"\nabout = \"Sketches.\"\n\
         url = \"https://sketch.example.com/mcp?access_token=abc123\"\n",
    );

    // Every connection is listed; the one whose URL holds a token says why it fails, unsaid.
    let listed = user.call("connection.list", json!({})).unwrap();
    assert!(listed["error"].is_null(), "{listed}");
    let of = |name: &str| {
        listed["connections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} is not listed: {listed}"))
    };
    let design = of("design");
    let failing = design["failing"].as_str().unwrap_or_default();
    assert!(failing.contains("userToken"), "{design}");
    assert!(!failing.contains("abc123"), "{design}");
    // No server answers there: it may fail to start, never for its query.
    let notes = of("notes");
    assert!(
        !notes["failing"]
            .as_str()
            .unwrap_or_default()
            .contains("query holds"),
        "{notes}"
    );

    // It does not start, and says why; the others are edited as before.
    let started = user
        .call("connection.review", json!({"name": "design"}))
        .unwrap_err();
    assert!(started.1.contains("userToken"), "{started:?}");
    assert!(!started.1.contains("abc123"), "{started:?}");
    user.call(
        "connection.edit",
        json!({"name": "notes", "about": "The notes."}),
    )
    .unwrap();

    // The failing ones are mended or removed.
    user.call(
        "connection.edit",
        json!({"name": "design", "url": "https://design.example.com/mcp"}),
    )
    .unwrap();
    user.call("connection.remove", json!({"name": "sketch"}))
        .unwrap();
    let listed = user.call("connection.list", json!({})).unwrap();
    let names: Vec<&str> = listed["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["notes", "design"], "{listed}");
    let design = &listed["connections"][1];
    assert!(
        !design["failing"]
            .as_str()
            .unwrap_or_default()
            .contains("query holds"),
        "{design}"
    );
}

#[test]
fn a_token_like_query_is_refused_on_add_and_on_edit() {
    let user = user();
    for parameter in [
        "token",
        "userToken",
        "access_token",
        "api_key",
        "apikey",
        "X-API-Key",
        "client_secret",
        "password",
    ] {
        let url = format!("https://design.example.com/mcp?page=2&{parameter}=abc123");
        let refused = user
            .call(
                "connection.add",
                json!({"name": "design", "about": "Designs.", "url": url}),
            )
            .unwrap_err();
        assert!(refused.1.contains(parameter), "{parameter}: {refused:?}");
        assert!(!refused.1.contains("abc123"), "{refused:?}");
    }
    user.call(
        "connection.add",
        json!({"name": "design", "about": "Designs.", "url": "https://design.example.com/mcp"}),
    )
    .unwrap();
    let refused = user
        .call(
            "connection.edit",
            json!({"name": "design", "url": "https://design.example.com/mcp?access_token=abc123"}),
        )
        .unwrap_err();
    assert!(refused.1.contains("access_token"), "{refused:?}");
    assert!(!refused.1.contains("abc123"), "{refused:?}");
    assert_eq!(
        connection_named(&user, "design")["url"],
        "https://design.example.com/mcp"
    );
}

#[test]
fn a_parameter_that_only_contains_token_is_allowed() {
    let user = user();
    let url = "https://design.example.com/mcp?author=ada&oauth_state=x&pageToken=2&keyword=a\
               &project_key=ab1";
    user.call(
        "connection.add",
        json!({"name": "design", "about": "Designs.", "url": url}),
    )
    .unwrap();
    assert_eq!(connection_named(&user, "design")["url"], url);
}

fn connection_named(user: &User, name: &str) -> Value {
    user.call("connection.list", json!({})).unwrap()["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
        .unwrap()
}

#[test]
fn a_secret_line_on_edit_replaces_the_old_header_line() {
    let user = user();
    set(&user, json!({"name": "notes-token", "secret": SECRET}));
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "Notes.", "url": "https://notes.example.com/mcp",
            "headers": {"Authorization": {"vault": "notes-token"}}}),
    )
    .unwrap();
    // What `--secret header:Authorization=Bearer notes-token` sends.
    user.call(
        "connection.edit",
        json!({"name": "notes", "secrets": [{"in": "header", "name": "Authorization",
            "prefix": "Bearer ", "entry": "notes-token"}]}),
    )
    .unwrap();
    let shown = connection(&user);
    assert!(shown.get("headers").is_none(), "{shown}");
    assert_eq!(
        shown["secrets"],
        json!([{"in": "header", "name": "Authorization", "prefix": "Bearer ",
            "entry": "notes-token"}]),
        "{shown}"
    );
}

#[test]
fn null_secrets_on_edit_clear_the_old_header_lines_too() {
    let user = user();
    set(&user, json!({"name": "notes-token", "secret": SECRET}));
    set(&user, json!({"name": "notes-session", "secret": SECRET}));
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "Notes.", "url": "https://notes.example.com/mcp",
            "headers": {"X-Notes-Key": {"vault": "notes-token"}},
            "secrets": [{"in": "cookie", "name": "session", "entry": "notes-session"}]}),
    )
    .unwrap();
    user.call("connection.edit", json!({"name": "notes", "secrets": null}))
        .unwrap();
    let shown = connection(&user);
    assert!(shown.get("headers").is_none(), "{shown}");
    assert!(shown.get("secrets").is_none(), "{shown}");
}

#[test]
fn a_cookie_header_beside_cookie_lines_is_refused() {
    let user = user();
    set(&user, json!({"name": "notes-token", "secret": SECRET}));
    set(&user, json!({"name": "notes-session", "secret": SECRET}));
    let refused = user
        .call(
            "connection.add",
            json!({"name": "notes", "about": "Notes.", "url": "https://notes.example.com/mcp",
                "headers": {"Cookie": {"vault": "notes-token"}},
                "secrets": [{"in": "cookie", "name": "session", "entry": "notes-session"}]}),
        )
        .unwrap_err();
    assert!(refused.1.contains("given twice"), "{refused:?}");
}
