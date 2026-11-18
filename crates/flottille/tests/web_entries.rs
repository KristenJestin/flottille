//! The scenarios of `web` calling an API with a vault entry's secret (`docs/tools.md`, "The web",
//! `docs/vault.md`, "How a secret is used"): the daemon adds the sign-in itself, only on the
//! entry's host, and the agent never sees the secret. Two local servers bound for the test (the
//! addresses of this machine the daemon is told to open, `FLOTTILLE_TEST_WEB_OPEN`) stand for an
//! API and another site; the vault's keyring is the daemon's memory; the agents are fake ACP
//! agents, in throwaway repositories and temporary XDG folders.

mod common;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// The entry's secret: invented, long enough to be masked.
const SECRET: &str = "tok-7Hq2Wx9Lm4Pz";
const ACCOUNT: &str = "robin@example.test";
const ENTRY: &str = "tracker-key";

/// A request as a test server received it.
#[derive(Clone, Debug)]
struct Received {
    method: String,
    path: String,
    /// Lowercased names.
    headers: HashMap<String, String>,
    body: String,
}

/// A local server answering each path, what it received kept in order.
struct Site {
    addr: SocketAddr,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Site {
    /// A server on `ip`; `elsewhere` is where its `/away` redirects.
    fn start(ip: &str, elsewhere: Option<SocketAddr>) -> Site {
        let listener = TcpListener::bind(format!("{ip}:0")).unwrap();
        let addr = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let kept = received.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut words = line.split_whitespace();
                let method = words.next().unwrap_or_default().to_owned();
                let path = words.next().unwrap_or("/").to_owned();
                let mut headers = HashMap::new();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = header.trim_end().split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
                    }
                }
                let length: usize = headers
                    .get("content-length")
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let received = Received {
                    method,
                    path: path.clone(),
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                let answer = answer(&received, elsewhere);
                kept.lock().unwrap().push(received);
                let _ = stream.write_all(&answer);
            }
        });
        Site { addr, received }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

fn answer(received: &Received, elsewhere: Option<SocketAddr>) -> Vec<u8> {
    let (status, location, body) = match received.path.as_str() {
        "/rest/api/3/field" => (
            "200 OK",
            None,
            r#"{"fields":[{"id":"customfield_10010","name":"Team"}]}"#.to_owned(),
        ),
        // A site that says back what it was sent, its credential included.
        "/echo" => (
            "200 OK",
            None,
            json!({"authorization": received.headers.get("authorization"),
                "said": format!("your token is {SECRET}")})
            .to_string(),
        ),
        "/rest/api/3/issue" => ("201 Created", None, r#"{"key":"ABC-7"}"#.to_owned()),
        "/away" => (
            "302 Found",
            elsewhere.map(|addr| format!("http://{addr}/collect")),
            String::new(),
        ),
        _ => ("404 Not Found", None, r#"{"error":"none"}"#.to_owned()),
    };
    let location = location.map_or(String::new(), |l| format!("Location: {l}\r\n"));
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{location}Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// The API, and another site on another host (`127.0.0.2`, another loopback address): the API's
/// `/away` redirects there.
fn sites() -> (Site, Site) {
    let other = Site::start("127.0.0.2", None);
    let api = Site::start("127.0.0.1", Some(other.addr));
    (api, other)
}

fn user_of(api: &Site, other: &Site) -> User {
    User::with_fake_agent().with_env(
        "FLOTTILLE_TEST_WEB_OPEN",
        &format!("{},{}", api.addr, other.addr),
    )
}

/// The project `app` and the session of its root agent, the user's message naming `urls` (so
/// that reading them is no URL the agent built): the agent's id and its session.
fn session(user: &User, urls: &[String]) -> (String, Mcp) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let message = format!("read the tracker's fields: {}", urls.join(" "));
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization))
}

/// The entry `tracker-key` with `fields` beside its name and secret.
fn entry(user: &User, fields: Value) {
    let mut entry = json!({"name": ENTRY, "secret": SECRET});
    entry
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    user.call("vault.set", entry).unwrap();
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

/// Refused before anything is asked of the user.
fn refused(user: &User, answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    assert!(answer.text.starts_with("refused: "), "{}", answer.text);
    assert!(requests(user).is_empty(), "held: {}", answer.text);
    assert!(!answer.text.contains(SECRET), "{}", answer.text);
    answer.text
}

fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn basic() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{ACCOUNT}:{SECRET}"))
    )
}

/// The names of the files under `dir` whose bytes hold `needle`.
fn files_holding(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for item in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = item.path();
            if path.is_dir() {
                folders.push(path);
            } else if std::fs::read(&path)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
            {
                found.push(path);
            }
        }
    }
    found
}

fn call(url: &str, more: Value) -> Value {
    let mut args = json!({"action": "read", "url": url});
    args.as_object_mut()
        .unwrap()
        .extend(more.as_object().unwrap().clone());
    args
}

#[test]
fn web_with_an_entry_signs_in_with_basic_when_the_entry_has_a_username() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let fields = api.url("/rest/api/3/field");
    let (_, mcp) = session(&user, std::slice::from_ref(&fields));
    entry(
        &user,
        json!({"username": ACCOUNT, "url": api.url("/"), "project": "app"}),
    );

    let text = done(mcp.call("web", call(&fields, json!({"entry": ENTRY}))));

    let received = api.received();
    assert_eq!(received.len(), 1, "{received:?}");
    assert_eq!(received[0].method, "GET");
    assert_eq!(received[0].headers["authorization"], basic());
    // The JSON as it came, with its status and its type.
    assert!(text.contains("200 OK"), "{text}");
    assert!(text.contains("application/json"), "{text}");
    assert!(
        text.contains(r#"{"fields":[{"id":"customfield_10010","name":"Team"}]}"#),
        "{text}"
    );
}

#[test]
fn web_with_an_entry_without_a_username_sends_a_bearer_token() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let fields = api.url("/rest/api/3/field");
    let (_, mcp) = session(&user, std::slice::from_ref(&fields));
    entry(&user, json!({"url": api.url("/rest/")}));

    done(mcp.call("web", call(&fields, json!({"entry": ENTRY}))));

    let received = api.received();
    assert_eq!(received.len(), 1, "{received:?}");
    assert_eq!(
        received[0].headers["authorization"],
        format!("Bearer {SECRET}")
    );
}

#[test]
fn web_refuses_an_entry_for_another_host_before_sending_anything() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let fields = api.url("/rest/api/3/field");
    let (_, mcp) = session(&user, std::slice::from_ref(&fields));
    entry(&user, json!({"username": ACCOUNT, "url": other.url("/")}));

    let said = refused(
        &user,
        mcp.call("web", call(&fields, json!({"entry": ENTRY}))),
    );

    assert!(said.contains("127.0.0.1"), "{said}");
    assert!(said.contains("127.0.0.2"), "{said}");
    assert!(api.received().is_empty());
    assert!(other.received().is_empty());
}

#[test]
fn web_does_not_follow_a_redirect_to_another_host_with_the_credential() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let away = api.url("/away");
    let (_, mcp) = session(&user, std::slice::from_ref(&away));
    entry(&user, json!({"username": ACCOUNT, "url": api.url("/")}));

    let answer = mcp.call("web", call(&away, json!({"entry": ENTRY})));

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.starts_with("refused: "), "{}", answer.text);
    // It says where the redirect led.
    assert!(
        answer.text.contains(&other.url("/collect")),
        "{}",
        answer.text
    );
    assert_eq!(api.received().len(), 1);
    assert!(other.received().is_empty(), "{:?}", other.received());
}

#[test]
fn web_refuses_an_entry_bound_to_another_use() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let fields = api.url("/rest/api/3/field");
    let (_, mcp) = session(&user, std::slice::from_ref(&fields));
    entry(
        &user,
        json!({"username": ACCOUNT, "url": api.url("/"), "for": "connection:almanac"}),
    );

    let said = refused(
        &user,
        mcp.call("web", call(&fields, json!({"entry": ENTRY}))),
    );

    assert!(
        said.contains("bound to the connection almanac and serves no other use"),
        "{said}"
    );
    assert!(api.received().is_empty());
}

#[test]
fn web_refuses_an_entry_of_another_project() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let fields = api.url("/rest/api/3/field");
    let (_, mcp) = session(&user, std::slice::from_ref(&fields));
    let billing = user.path("work/billing");
    user.git_init(&billing);
    user.call("project.add", json!({"folder": billing, "name": "billing"}))
        .unwrap();
    entry(
        &user,
        json!({"username": ACCOUNT, "url": api.url("/"), "project": "billing"}),
    );

    let said = refused(
        &user,
        mcp.call("web", call(&fields, json!({"entry": ENTRY}))),
    );

    assert!(
        said.contains("the vault has no entry tracker-key for this project"),
        "{said}"
    );
    // The name is taken: asking the user for it would be refused.
    assert!(!said.contains("vault {action"), "{said}");
    assert!(api.received().is_empty());
}

#[test]
fn a_missing_entry_offers_to_ask_the_user() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let fields = api.url("/rest/api/3/field");
    let (_, mcp) = session(&user, std::slice::from_ref(&fields));
    // Another entry of the project's: its agents have the vault tool.
    user.call(
        "vault.set",
        json!({"name": "deploy-key", "secret": "dk-93jdk20sk1", "project": "app"}),
    )
    .unwrap();

    let said = refused(
        &user,
        mcp.call("web", call(&fields, json!({"entry": ENTRY}))),
    );

    assert!(
        said.contains("the vault has no entry tracker-key for this project"),
        "{said}"
    );
    assert!(
        said.contains(r#"vault {action: "ask", name: "tracker-key""#),
        "{said}"
    );
    assert!(said.contains(&api.url("")), "{said}");
    assert!(api.received().is_empty());
}

#[test]
fn a_post_is_judged_and_its_card_says_who_it_writes_as() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let issue = api.url("/rest/api/3/issue");
    let (id, mcp) = session(&user, std::slice::from_ref(&issue));
    set_mode(&user, &id, "ask");
    entry(&user, json!({"username": ACCOUNT, "url": api.url("/")}));
    let body = r#"{"fields":{"summary":"Broken login"}}"#;

    let number = held(&mcp.call(
        "web",
        call(
            &issue,
            json!({"entry": ENTRY, "method": "POST", "body": body}),
        ),
    ));

    assert!(api.received().is_empty(), "sent before the user's answer");
    let asked = requests(&user)
        .into_iter()
        .find(|request| request["number"] == number)
        .unwrap();
    assert_eq!(asked["class"], "judge");
    let card = asked["reasons"].to_string();
    for said in [
        "POST",
        issue.as_str(),
        "to 127.0.0.1",
        "as robin@example.test with the vault entry tracker-key",
        "Broken login",
    ] {
        assert!(card.contains(said), "{said} not in {card}");
    }
    assert!(!card.contains(SECRET), "{card}");

    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the post", || !api.received().is_empty());
    let received = &api.received()[0];
    assert_eq!(received.method, "POST");
    assert_eq!(received.body, body);
    assert_eq!(received.headers["authorization"], basic());
    assert_eq!(received.headers["content-type"], "application/json");
}

#[test]
fn a_get_with_an_entry_is_a_read() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let fields = api.url("/rest/api/3/field");
    let (id, mcp) = session(&user, std::slice::from_ref(&fields));
    set_mode(&user, &id, "ask");
    entry(&user, json!({"username": ACCOUNT, "url": api.url("/")}));

    done(mcp.call(
        "web",
        call(&fields, json!({"entry": ENTRY, "method": "GET"})),
    ));

    assert!(requests(&user).is_empty());
    assert_eq!(api.received().len(), 1);
}

#[test]
fn the_secret_is_masked_in_an_answer_that_echoes_it() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let echo = api.url("/echo");
    let (_, mcp) = session(&user, std::slice::from_ref(&echo));
    entry(&user, json!({"username": ACCOUNT, "url": api.url("/")}));

    let text = done(mcp.call("web", call(&echo, json!({"entry": ENTRY}))));

    let encoded = basic().strip_prefix("Basic ").unwrap().to_owned();
    assert!(text.contains("[masked]"), "{text}");
    for forbidden in [SECRET, encoded.as_str()] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
        assert_eq!(
            files_holding(&user.path("state"), forbidden),
            Vec::<PathBuf>::new(),
            "{forbidden} is stored"
        );
    }
}

#[test]
fn an_agent_header_cannot_set_authorization_or_cookie() {
    let (api, other) = sites();
    let user = user_of(&api, &other);
    let fields = api.url("/rest/api/3/field");
    let (_, mcp) = session(&user, std::slice::from_ref(&fields));
    entry(&user, json!({"username": ACCOUNT, "url": api.url("/")}));

    for name in ["Authorization", "cookie", "Proxy-Authorization"] {
        let said = refused(
            &user,
            mcp.call(
                "web",
                call(
                    &fields,
                    json!({"entry": ENTRY, "headers": {name: "Basic eDp5"}}),
                ),
            ),
        );
        assert!(
            said.to_ascii_lowercase()
                .contains(&format!("{} header", name.to_ascii_lowercase())),
            "{said}"
        );
    }
    assert!(api.received().is_empty());
}
