//! A Jira work item edited through Jira's REST API (#1067, `docs/tickets.md`, "Editing"): `acli`'s
//! `workitem edit --from-json` reads no `additionalAttributes` and `acli` has no flag for a
//! parent or a fix version, so `edit` with `fields`, `parent` or a milestone is a `PUT
//! /rest/api/3/issue/<key>` with the entry the project names for `log_time`, `worklogs` and
//! `fields`. Jira is a fake HTTP server on this machine, the vault's keyring the daemon's memory,
//! and the `acli` the fake: no test reaches a site, the user's keyring or anyone's sign-in.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

const SITE: &str = "example.atlassian.net";
const SECRET: &str = "ed1t-t0ken-never-shown";
const ACCOUNT: &str = "dev@example.com";
/// What the daemon reads to send its Jira calls to the fake server (a debug build only).
const BASE: &str = "FLOTTILLE_TEST_JIRA_BASE";

/// What the fake Jira received of one request.
#[derive(Clone, Debug)]
struct Received {
    method: String,
    path: String,
    authorization: String,
    body: Value,
}

/// A Jira that answers every request with `status` and `body`, and remembers what it was asked.
struct Jira {
    base: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Jira {
    fn start(status: &'static str, said: &'static str) -> Jira {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let kept = received.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut raw = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, mut rest) = loop {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    raw.extend_from_slice(&chunk[..read]);
                    if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (
                            String::from_utf8_lossy(&raw[..at]).into_owned(),
                            raw[at + 4..].to_vec(),
                        );
                    }
                    if read == 0 {
                        return;
                    }
                };
                let header = |name: &str| {
                    head.lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.trim().to_owned())
                        .unwrap_or_default()
                };
                let length: usize = header("content-length").parse().unwrap_or(0);
                while rest.len() < length {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    rest.extend_from_slice(&chunk[..read]);
                }
                let mut line = head.lines().next().unwrap_or_default().split(' ');
                kept.lock().unwrap().push(Received {
                    method: line.next().unwrap_or_default().to_owned(),
                    path: line.next().unwrap_or_default().to_owned(),
                    authorization: header("authorization"),
                    body: serde_json::from_slice(&rest).unwrap_or(Value::Null),
                });
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: \
                         {}\r\nConnection: close\r\n\r\n{said}",
                        said.len()
                    )
                    .as_bytes(),
                );
            }
        });
        Jira { base, received }
    }

    /// A site that accepts every edit: Jira answers `204 No Content`.
    fn accepting() -> Jira {
        Jira::start("204 No Content", "")
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

fn user(jira: &Jira) -> User {
    User::with_fake_agent()
        .with_fake_acli()
        .with_env(BASE, &jira.base)
}

/// A project `app` over a throwaway repository, its tracker `tracker`, and a ready workspace.
fn fixture(user: &User, tracker: Value) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": tracker}),
    )
    .unwrap();
}

fn jira_with(user: &User, entry: Option<&str>) {
    let mut tracker = json!({"kind": "jira", "site": SITE, "projects": ["ABC"]});
    if let Some(entry) = entry {
        tracker["worklog_token"] = entry.into();
    }
    fixture(user, tracker);
}

/// The entry `jira-time` of the project, with its secret and its account.
fn entry(user: &User) {
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": ACCOUNT, "project": "app"}),
    )
    .unwrap();
}

/// A Jira project ABC whose vault entry is set, over `jira`, its work item ABC-1 and its versions
/// known to the fake `acli`.
fn signed_in(jira: &Jira) -> (User, Mcp) {
    let user = user(jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    user.acli_script(json!({
        "items": {"ABC-1": {"key": "ABC-1"}},
        "versions": {"ABC": [{"name": "1.0", "released": true}, {"name": "1.1"}]}}));
    let mcp = root_in(&user);
    (user, mcp)
}

fn root_in(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn store(user: &User) -> Store {
    Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

/// A call held for the user: its request's number.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// The user allows request `number`: the text the agent is handed for it.
fn allow(user: &User, number: u64) -> String {
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

/// A call held, then allowed by the user: what the agent is handed.
fn written(user: &User, mcp: &Mcp, args: Value) -> String {
    let number = held(&mcp.call("tickets", args));
    allow(user, number)
}

/// A call refused before anything is asked of the user: what the agent is told.
fn refused_at_once(user: &User, mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("tickets", args.clone());
    assert!(answer.error, "{args}: {}", answer.text);
    assert!(answer.text.starts_with("refused:"), "{}", answer.text);
    assert!(requests(user).is_empty(), "{args} was held");
    answer.text
}

/// What the fake `acli` was asked, one line a call.
fn acli_calls(user: &User) -> Vec<String> {
    user.acli_received()
        .iter()
        .map(|call| {
            call["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn basic() -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{ACCOUNT}:{SECRET}"))
    )
}

#[test]
fn edit_with_fields_on_jira_sets_them_through_the_api() {
    let jira = Jira::accepting();
    let (user, mcp) = signed_in(&jira);
    let fields = json!({"customfield_10100": {"value": "Minor"}, "priority": {"name": "High"}});

    let done = written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "abc-1", "fields": fields}),
    );

    assert!(done.contains("ABC-1 was changed."), "{done}");
    assert!(!done.contains(SECRET), "{done}");
    let received = jira.received();
    assert_eq!(received.len(), 1, "{received:?}");
    assert_eq!(received[0].method, "PUT");
    assert_eq!(received[0].path, "/rest/api/3/issue/ABC-1");
    assert_eq!(received[0].authorization, basic());
    assert_eq!(received[0].body, json!({"fields": fields}));
    // `acli` was not asked to edit what its JSON cannot take.
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| !call.starts_with("jira workitem edit")),
        "{:?}",
        acli_calls(&user)
    );

    // What `acli` does take goes to `acli`, after the fields.
    let both = written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "ABC-1", "title": "A new title", "fields": fields}),
    );
    assert!(both.contains("ABC-1 was changed."), "{both}");
    assert_eq!(jira.received().len(), 2);
    assert!(
        acli_calls(&user)
            .contains(&"jira workitem edit --key ABC-1 --summary A new title --yes".to_owned()),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn a_site_that_refuses_the_fields_is_said_with_nothing_else_written() {
    let jira = Jira::start(
        "400 Bad Request",
        "{\"errorMessages\":[],\"errors\":{\"customfield_10100\":\"Option id 'x' is not valid\"}}",
    );
    let (user, mcp) = signed_in(&jira);

    let said = written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "ABC-1", "title": "A new title",
            "fields": {"customfield_10100": {"value": "x"}}}),
    );

    assert!(said.contains("400"), "{said}");
    assert!(said.contains("Option id 'x' is not valid"), "{said}");
    assert!(said.contains("nothing was written"), "{said}");
    assert!(!said.contains(SECRET), "{said}");
    // The fields go first: a refusal of them leaves the title unwritten.
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| !call.starts_with("jira workitem edit")),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn edit_with_a_parent_on_jira_moves_the_work_item_under_it() {
    let jira = Jira::accepting();
    let (user, mcp) = signed_in(&jira);

    let done = written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "ABC-1", "parent": "abc-10"}),
    );

    assert!(done.contains("ABC-1 was changed."), "{done}");
    let received = jira.received();
    assert_eq!(received.len(), 1, "{received:?}");
    assert_eq!(received[0].method, "PUT");
    assert_eq!(received[0].path, "/rest/api/3/issue/ABC-1");
    assert_eq!(received[0].authorization, basic());
    assert_eq!(
        received[0].body,
        json!({"fields": {"parent": {"key": "ABC-10"}}})
    );
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| !call.starts_with("jira workitem edit")),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn edit_with_fields_without_a_tracker_entry_is_refused_before_anything_runs() {
    let jira = Jira::accepting();
    let user = user(&jira);
    jira_with(&user, None);
    entry(&user);
    let mcp = root_in(&user);
    let asks = [
        json!({"action": "edit", "ticket": "ABC-1", "fields": {"priority": {"name": "High"}}}),
        json!({"action": "edit", "ticket": "ABC-1", "parent": "ABC-10"}),
        json!({"action": "edit", "ticket": "ABC-1", "milestone": "1.0"}),
        json!({"action": "edit", "ticket": "ABC-1", "title": "T", "fields": {"a": 1}}),
    ];

    for ask in &asks {
        let said = refused_at_once(&user, &mcp, ask.clone());
        // It says how to set one, never that Jira refused.
        for how in ["worklog_token", "[tracker]", "vault", "Settings"] {
            assert!(said.contains(how), "{how} is not said: {said}");
        }
        assert!(!said.contains("Jira refused"), "{said}");
    }

    // An entry named and not in the vault is refused too, and the agent may ask the user for it.
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "jira", "site": SITE, "projects": ["ABC"],
                "worklog_token": "nowhere"}}),
    )
    .unwrap();
    for ask in &asks {
        let said = refused_at_once(&user, &mcp, ask.clone());
        assert!(said.contains("nowhere"), "{said}");
        assert!(said.contains("\"ask\""), "{said}");
    }
    assert!(jira.received().is_empty());
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| call == "jira auth status"),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn a_fix_version_of_an_existing_work_item_is_set_through_the_api() {
    let jira = Jira::accepting();
    let (user, mcp) = signed_in(&jira);

    // The version is set by its name, and taken away with an empty list.
    let done = written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "ABC-1", "milestone": "1.1"}),
    );
    assert!(done.contains("ABC-1 was changed."), "{done}");
    let removed = written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "ABC-1", "remove_milestone": true}),
    );
    assert!(removed.contains("ABC-1 was changed."), "{removed}");

    let received = jira.received();
    assert_eq!(received.len(), 2, "{received:?}");
    for put in &received {
        assert_eq!(put.method, "PUT");
        assert_eq!(put.path, "/rest/api/3/issue/ABC-1");
        assert_eq!(put.authorization, basic());
    }
    assert_eq!(
        received[0].body,
        json!({"fields": {"fixVersions": [{"name": "1.1"}]}})
    );
    assert_eq!(received[1].body, json!({"fields": {"fixVersions": []}}));

    // A version the project has not is refused before anything is written.
    let said = written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "ABC-1", "milestone": "9.9"}),
    );
    assert!(said.contains("9.9"), "{said}");
    assert!(said.contains("nothing was written"), "{said}");
    assert_eq!(jira.received().len(), 2);
}
