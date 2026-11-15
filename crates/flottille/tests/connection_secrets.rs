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

#[test]
fn run_together_and_short_token_names_are_refused() {
    let user = user();
    for parameter in [
        "usertoken",
        "accesstoken",
        "authtoken",
        "apitoken",
        "sessiontoken",
        "securitytoken",
        "clientsecret",
        "appsecret",
        "apisecret",
        "secretkey",
        "accesskey",
        "sig",
        "signature",
        "X-Amz-Signature",
        "X-Amz-Security-Token",
        "jwt",
        "bearer",
        "sessionid",
        "session_id",
        "sid",
        "token2",
        "gitlab_private_token",
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
    assert!(
        user.call("connection.list", json!({})).unwrap()["connections"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn page_and_next_tokens_and_project_key_are_still_allowed() {
    let user = user();
    let url = "https://design.example.com/mcp?pageToken=2&nextToken=b&page_token=c\
               &next_page_token=d&continuation_token=e&project_key=ab1&signed=1&session=7\
               &keyword=a#section-2";
    user.call(
        "connection.add",
        json!({"name": "design", "about": "Designs.", "url": url}),
    )
    .unwrap();
    assert_eq!(connection_named(&user, "design")["url"], url);
}

#[test]
fn a_token_in_the_fragment_is_refused() {
    let user = user();
    let refused = user
        .call(
            "connection.add",
            json!({"name": "design", "about": "Designs.",
                "url": "https://design.example.com/mcp#access_token=abc123&state=x"}),
        )
        .unwrap_err();
    assert!(refused.1.contains("access_token"), "{refused:?}");
    assert!(!refused.1.contains("abc123"), "{refused:?}");
    user.call(
        "connection.add",
        json!({"name": "design", "about": "Designs.", "url": "https://design.example.com/mcp"}),
    )
    .unwrap();
    let refused = user
        .call(
            "connection.edit",
            json!({"name": "design", "url": "https://design.example.com/mcp#userToken=abc123"}),
        )
        .unwrap_err();
    assert!(refused.1.contains("userToken"), "{refused:?}");
    assert!(!refused.1.contains("abc123"), "{refused:?}");
    assert_eq!(
        connection_named(&user, "design")["url"],
        "https://design.example.com/mcp"
    );
}

/// What may be a secret in a URL that is refused for something else: its login, its query, its
/// fragment.
const IN_A_URL: &str = "Zq7Wx3Vp9";

#[test]
fn a_url_error_never_shows_a_query_value() {
    let user = user();
    for url in [
        format!("http://notes.example.com/mcp?access_token={IN_A_URL}"),
        format!("http://notes.example.com/mcp?page={IN_A_URL}"),
        format!("ftp://notes.example.com/mcp?key={IN_A_URL}"),
        format!("http://notes.example.com/mcp#access_token={IN_A_URL}"),
        format!("https://ada:{IN_A_URL}@notes.example.com/mcp"),
        format!("https://[notes?token={IN_A_URL}"),
    ] {
        let refused = user
            .call(
                "connection.add",
                json!({"name": "notes", "about": "Notes.", "url": url}),
            )
            .unwrap_err();
        assert!(!refused.1.contains(IN_A_URL), "{url}: {refused:?}");
    }

    // Declared by hand: that connection alone fails, saying why without its value.
    user.call(
        "connection.add",
        json!({"name": "notes", "about": "Notes.", "url": "https://notes.example.com/mcp"}),
    )
    .unwrap();
    written_by_hand(
        &user,
        &format!(
            "\n[[connections.server]]\nname = \"design\"\nabout = \"Designs.\"\n\
             url = \"http://design.example.com/mcp?access_token={IN_A_URL}\"\n"
        ),
    );
    let listed = user.call("connection.list", json!({})).unwrap();
    assert!(listed["error"].is_null(), "{listed}");
    let design = connection_named(&user, "design");
    let failing = design["failing"].as_str().unwrap_or_default();
    assert!(failing.contains("this machine"), "{design}");
    assert!(!failing.contains(IN_A_URL), "{design}");
    let started = user
        .call("connection.review", json!({"name": "design"}))
        .unwrap_err();
    assert!(started.1.contains("this machine"), "{started:?}");
    assert!(!started.1.contains(IN_A_URL), "{started:?}");
    // The others are edited as before, and it is mended.
    user.call(
        "connection.edit",
        json!({"name": "notes", "about": "The notes."}),
    )
    .unwrap();
    // Mended to this machine, where no server listens: it may fail to start, never for its URL.
    let mended = format!("http://127.0.0.1:{}/mcp", closed_port());
    user.call("connection.edit", json!({"name": "design", "url": mended}))
        .unwrap();
    let design = connection_named(&user, "design");
    assert!(
        !design["failing"]
            .as_str()
            .unwrap_or_default()
            .starts_with("url"),
        "{design}"
    );
    assert_eq!(design["url"], mended, "{design}");
}

/// A port of this machine no server listens on.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn a_url_flagged_as_holding_a_secret_is_served_without_it() {
    let user = user();
    written_by_hand(
        &user,
        &format!(
            "\n[[connections.server]]\nname = \"design\"\nabout = \"Designs.\"\n\
             url = \"https://design.example.com/mcp?access_token={IN_A_URL}#k\"\n\
             \n[[connections.server]]\nname = \"sketch\"\nabout = \"Sketches.\"\n\
             url = \"https://ada:{IN_A_URL}@sketch.example.com/mcp\"\n\
             \n[[connections.server]]\nname = \"notes\"\nabout = \"Notes.\"\n\
             url = \"ada:{IN_A_URL}@notes.example.com/mcp\"\n\
             \n[[connections.server]]\nname = \"plain\"\nabout = \"Plain.\"\n\
             url = \"https://plain.example.com/mcp?page=2\"\n"
        ),
    );
    let listed = user.call("connection.list", json!({})).unwrap();
    assert!(!listed.to_string().contains(IN_A_URL), "{listed}");
    for (name, url) in [
        ("design", "https://design.example.com/mcp"),
        ("sketch", "https://sketch.example.com/mcp"),
        ("notes", ""),
        ("plain", "https://plain.example.com/mcp?page=2"),
    ] {
        assert_eq!(connection_named(&user, name)["url"], url, "{listed}");
    }
    // Nor does an agent see it, listed or shown.
    let mcp = session(&user);
    let mut said = vec![mcp.call("connections", json!({"action": "list"})).text];
    for name in ["design", "sketch", "notes"] {
        said.push(
            mcp.call("connections", json!({"action": "show", "name": name}))
                .text,
        );
    }
    for text in &said {
        assert!(!text.contains(IN_A_URL), "{text}");
    }
    assert!(
        said[1].contains("https://design.example.com/mcp"),
        "{}",
        said[1]
    );
}

#[test]
fn longer_token_key_and_password_names_are_refused() {
    let user = user();
    for parameter in [
        "x_user_token",
        "google_id_token",
        "x_refresh_token",
        "bearer_token",
        "jwt_token",
        "secret_token",
        "oauth_token",
        "auth_key",
        "app_key",
        "client_key",
        "subscription-key",
        "pwd",
        "passphrase",
        "db_password",
        "url_sig",
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
}

#[test]
fn harmless_words_before_a_token_or_key_are_allowed() {
    let user = user();
    let url = "https://design.example.com/mcp?pageToken=a&next_page_token=b&cursor_key=c\
               &project_key=d&idempotency_key=e&sync_token=f&continuationToken=g&nextpagetoken=h";
    user.call(
        "connection.add",
        json!({"name": "design", "about": "Designs.", "url": url}),
    )
    .unwrap();
    assert_eq!(connection_named(&user, "design")["url"], url);
}

#[test]
fn a_semicolon_does_not_hide_a_token_in_the_query_or_fragment() {
    let user = user();
    for url in [
        format!("https://design.example.com/mcp?page=1;access_token={IN_A_URL}"),
        format!("https://design.example.com/mcp#state=x;access_token={IN_A_URL}"),
    ] {
        let refused = user
            .call(
                "connection.add",
                json!({"name": "design", "about": "Designs.", "url": url}),
            )
            .unwrap_err();
        assert!(refused.1.contains("access_token"), "{url}: {refused:?}");
        assert!(!refused.1.contains(IN_A_URL), "{refused:?}");
    }
}

#[test]
fn a_url_pasted_without_its_scheme_names_only_the_scheme() {
    let user = user();
    let refused = user
        .call(
            "connection.add",
            json!({"name": "notes", "about": "Notes.",
                "url": format!("ada:{IN_A_URL}@notes.example.com/mcp")}),
        )
        .unwrap_err();
    assert!(refused.1.contains("is no http(s) URL"), "{refused:?}");
    assert!(!refused.1.contains("ada"), "{refused:?}");
    assert!(!refused.1.contains(IN_A_URL), "{refused:?}");
    assert!(!refused.1.contains("notes.example.com"), "{refused:?}");
}

#[test]
fn login_of_a_connection_failing_at_load_is_refused() {
    let user = user();
    written_by_hand(
        &user,
        "\n[[connections.server]]\nname = \"design\"\nabout = \"Designs.\"\n\
         url = \"https://design.example.com/mcp?userToken=abc123\"\n\
         \n[[connections.server]]\nname = \"sketch\"\nabout = \"Sketches.\"\n\
         url = \"http://sketch.example.com/mcp\"\n",
    );
    let refused = user
        .call("connection.login", json!({"name": "design"}))
        .unwrap_err();
    assert!(refused.1.contains("userToken"), "{refused:?}");
    assert!(!refused.1.contains("abc123"), "{refused:?}");
    let refused = user
        .call("connection.login", json!({"name": "sketch"}))
        .unwrap_err();
    assert!(refused.1.contains("this machine"), "{refused:?}");
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

/// A token pasted as the login of a URL without its scheme: the URL parser reads it as the scheme.
const SLACK_LIKE: &str = "xoxb-1-2-abc";
const HEX_TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";

/// What an agent's request was held as: its number.
fn held_number(text: &str) -> u64 {
    text.strip_prefix("Waiting for the user's approval, request #")
        .unwrap_or_else(|| panic!("not held: {text}"))
        .split('.')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn an_unknown_scheme_is_never_printed() {
    let user = user();
    for token in [SLACK_LIKE, HEX_TOKEN] {
        let refused = user
            .call(
                "connection.add",
                json!({"name": "notes", "about": "Notes.",
                    "url": format!("{token}:x@notes.example.com/mcp")}),
            )
            .unwrap_err();
        assert!(refused.1.contains("is no http(s) URL"), "{refused:?}");
        assert!(!refused.1.contains(token), "{refused:?}");
    }
    written_by_hand(
        &user,
        &format!(
            "\n[[connections.server]]\nname = \"slack\"\nabout = \"Chat.\"\n\
             url = \"{SLACK_LIKE}:x@slack.example.com/mcp\"\n\
             \n[[connections.server]]\nname = \"hex\"\nabout = \"Hex.\"\n\
             url = \"{HEX_TOKEN}:x@hex.example.com/mcp\"\n"
        ),
    );
    let listed = user.call("connection.list", json!({})).unwrap();
    for token in [SLACK_LIKE, HEX_TOKEN] {
        assert!(!listed.to_string().contains(token), "{listed}");
    }
    for name in ["slack", "hex"] {
        let connection = connection_named(&user, name);
        assert_eq!(connection["url"], "", "{connection}");
        let failing = connection["failing"].as_str().unwrap_or_default();
        assert!(failing.contains("is no http(s) URL"), "{connection}");
    }
    // Nor an agent, shown it or proposing to mend it.
    let mcp = session(&user);
    let mut said = Vec::new();
    for name in ["slack", "hex"] {
        said.push(
            mcp.call("connections", json!({"action": "show", "name": name}))
                .text,
        );
        let held = mcp.call(
            "connections",
            json!({"action": "edit", "name": name,
                "url": format!("https://{name}.example.com/mcp")}),
        );
        assert!(!held.error, "{}", held.text);
        let number = held_number(&held.text);
        let request = user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["number"] == number)
            .cloned()
            .unwrap();
        said.push(request.to_string());
        said.push(
            user.call("connection.proposed", json!({"number": number}))
                .unwrap()
                .to_string(),
        );
        said.push(held.text);
    }
    for text in &said {
        for token in [SLACK_LIKE, HEX_TOKEN] {
            assert!(!text.contains(token), "{text}");
        }
    }
}

#[test]
fn harmless_words_excuse_only_a_token_or_a_key() {
    let user = user();
    for parameter in [
        "project_secret",
        "projectSecret",
        "sync_password",
        "page_passphrase",
        "cursor_signature",
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
}

#[test]
fn plural_secret_names_and_a_plain_key_are_refused() {
    let user = user();
    for parameter in ["access_tokens", "apiKeys", "secrets", "passwords", "key"] {
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
}

#[test]
fn common_names_ending_in_key_or_token_are_allowed() {
    let user = user();
    let url = "https://design.example.com/mcp?issueKey=a&spaceKey=b&partition_key=c&cache_key=d\
               &object_key=e&public_key=f&publishable_key=g&site_key=h&sort_key=i\
               &pagination_token=j&resume_token=k&last_evaluated_key=l";
    user.call(
        "connection.add",
        json!({"name": "design", "about": "Designs.", "url": url}),
    )
    .unwrap();
    assert_eq!(connection_named(&user, "design")["url"], url);
}

#[test]
fn an_edit_card_marks_a_url_served_cleaned() {
    let user = user();
    written_by_hand(
        &user,
        &format!(
            "\n[[connections.server]]\nname = \"design\"\nabout = \"Designs.\"\n\
             url = \"https://design.example.com/mcp?project=p&access_token={IN_A_URL}\"\n"
        ),
    );
    let mcp = session(&user);
    let held = mcp.call(
        "connections",
        json!({"action": "edit", "name": "design",
            "url": "https://design.example.com/mcp?project=p"}),
    );
    assert!(!held.error, "{}", held.text);
    let number = held_number(&held.text);
    let says = user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .cloned()
        .unwrap()["says"]
        .to_string();
    assert!(!says.contains(IN_A_URL), "{says}");
    assert!(
        says.contains(
            "https://design.example.com/mcp (cleaned: its query is dropped) → \
             https://design.example.com/mcp?project=p"
        ),
        "{says}"
    );
}
