//! A Jira reply goes under the comment it answers, and a mention notifies (#1068,
//! `docs/tickets.md`, "Replies"): `reply` and a `comment` that mentions someone are `POST
//! /rest/api/3/issue/<key>/comment` with the entry the project names for `log_time` (a reply with
//! `parentId`, the comment answered or, one level deep, the root of its thread), and `[@Name]
//! (mention:<accountId>)` is a mention node in any text written to Jira. Jira is a fake HTTP
//! server on this machine, answering what a real site's read showed (a reply carries `parentId`, a
//! number, beside `id`, `author`, `body` and `created`), the vault's keyring the daemon's memory,
//! and `acli` and `gh` the fakes: no test reaches a site, the user's keyring or anyone's sign-in.

use crate::common;

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
const SECRET: &str = "r3ply-t0ken-never-shown";
const ACCOUNT: &str = "dev@example.com";
/// What the daemon reads to send its Jira calls to the fake server (a debug build only).
const BASE: &str = "FLOTTILLE_TEST_JIRA_BASE";
/// The account ids of the people of the fake site.
const ANA: &str = "5b10ac8d82e05b22cc7d4ef5";
const BEN: &str = "712020:9f8e7d6c-1111-4222-8333-444455556666";
/// A well-formed account id that the fake site has never heard of.
const NOBODY: &str = "5b10ac8d82e05b22cc7d4ef6";

/// The display name the fake site gives an account (`GET /rest/api/3/user?accountId=`), none for
/// an id it does not know.
fn name_of(account: &str) -> Option<&'static str> {
    match account {
        ANA => Some("Ana Example"),
        BEN => Some("Ben Sample"),
        _ => None,
    }
}

/// What the fake Jira received of one request.
#[derive(Clone, Debug)]
struct Received {
    method: String,
    path: String,
    authorization: String,
    body: Value,
}

/// A Jira that remembers what it was asked and answers `GET .../comment` with `comments` and a
/// `POST` with `201` and the id `20100`, as a real site does.
struct Jira {
    base: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Jira {
    fn with_comments(comments: Value) -> Jira {
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
                let method = line.next().unwrap_or_default().to_owned();
                let path = line.next().unwrap_or_default().to_owned();
                kept.lock().unwrap().push(Received {
                    method: method.clone(),
                    path: path.clone(),
                    authorization: header("authorization"),
                    body: serde_json::from_slice(&rest).unwrap_or(Value::Null),
                });
                let (status, said) = match method.as_str() {
                    "GET" if path.starts_with("/rest/api/3/user?") => {
                        let account = path.split("accountId=").nth(1).unwrap_or_default();
                        let account = account.split('&').next().unwrap_or_default();
                        let account = account.replace("%3A", ":");
                        match name_of(&account) {
                            Some(name) => (
                                "200 OK",
                                json!({"accountId": account, "displayName": name,
                                    "active": true})
                                .to_string(),
                            ),
                            None => (
                                "404 Not Found",
                                json!({"errorMessages": [format!(
                                    "The user with the accountId '{account}' does not exist")],
                                    "errors": {}})
                                .to_string(),
                            ),
                        }
                    }
                    "GET" => (
                        "200 OK",
                        json!({"startAt": 0, "maxResults": 5000, "total": comments
                            .as_array().map_or(0, Vec::len), "comments": comments})
                        .to_string(),
                    ),
                    _ => (
                        "201 Created",
                        json!({"id": "20100", "author": {"accountId": "me"},
                            "created": "2026-10-02T09:00:00.000+0000"})
                        .to_string(),
                    ),
                };
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

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    /// The requests that wrote.
    fn posted(&self) -> Vec<Received> {
        self.received()
            .into_iter()
            .filter(|request| request.method == "POST")
            .collect()
    }
}

/// A comment as Jira's REST gives it back: a reply carries `parentId`, a number.
fn rest_comment(id: &str, account: &str, parent: Option<u64>) -> Value {
    let mut comment = json!({"id": id, "author": {"accountId": account, "displayName": "Ana"},
        "body": document("a comment"), "created": "2026-10-01T09:00:00.000+0000"});
    if let Some(parent) = parent {
        comment["parentId"] = parent.into();
    }
    comment
}

fn document(text: &str) -> Value {
    json!({"type": "doc", "version": 1, "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

/// A comment as the fake `acli` views it.
fn viewed(id: &str, who: &str, account: &str, text: &str) -> Value {
    json!({"id": id, "author": {"displayName": who, "accountId": account},
        "created": "2026-10-01T09:00:00.000+0000", "body": document(text)})
}

fn user(jira: &Jira) -> User {
    User::with_fake_agent()
        .with_fake_acli()
        .with_fake_gh()
        .with_env(BASE, &jira.base)
}

/// A project `app` over a throwaway repository, its tracker `tracker`.
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

fn entry(user: &User) {
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": ACCOUNT, "project": "app"}),
    )
    .unwrap();
}

/// A work item ABC-1 whose comments `acli` views as `comments`.
fn script(user: &User, comments: Value) {
    user.acli_script(json!({"items": {"ABC-1": {"key": "ABC-1", "self": format!("https://{SITE}/rest/api/3/issue/1"),
        "fields": {"summary": "The login fails", "status": {"name": "To Do"},
            "comment": {"comments": comments}}}}}));
}

/// A Jira project ABC whose vault entry is set, over `jira`, its work item ABC-1 holding `viewed`.
fn signed_in(jira: &Jira, comments: Value) -> (User, Mcp) {
    let user = user(jira);
    jira_with(&user, Some("jira-time"));
    entry(&user);
    script(&user, comments);
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

fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

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

/// The words of the card of the last request, one per line.
fn card_of_the_last(user: &User) -> String {
    requests(user).last().unwrap()["says"]
        .as_array()
        .unwrap()
        .iter()
        .map(|said| said.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n")
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

/// The mention nodes of a document, as `(id, text)`.
fn mentions(document: &Value) -> Vec<(String, String)> {
    fn walk(node: &Value, found: &mut Vec<(String, String)>) {
        if node["type"] == "mention" {
            found.push((
                node["attrs"]["id"].as_str().unwrap().to_owned(),
                node["attrs"]["text"].as_str().unwrap().to_owned(),
            ));
        }
        for child in node["content"].as_array().into_iter().flatten() {
            walk(child, found);
        }
    }
    let mut found = Vec::new();
    walk(document, &mut found);
    found
}

fn read(user: &User, mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("tickets", args);
    assert!(!answer.error, "{}", answer.text);
    assert!(requests(user).is_empty());
    answer.text
}

#[test]
fn a_jira_reply_is_posted_under_the_comment_it_answers() {
    let jira = Jira::with_comments(json!([rest_comment("20000", ANA, None)]));
    let (user, mcp) = signed_in(
        &jira,
        json!([viewed("20000", "Ana", ANA, "I can reproduce")]),
    );

    let done = written(
        &user,
        &mcp,
        json!({"action": "reply", "ticket": "ABC-1", "comment": "20000",
            "body": "Thanks, a fix is on app/login"}),
    );

    assert!(
        done.contains("The reply to the comment 20000 of ABC-1 was posted"),
        "{done}"
    );
    assert!(!done.contains(SECRET), "{done}");
    let posted = jira.posted();
    assert_eq!(posted.len(), 1, "{:?}", jira.received());
    assert_eq!(posted[0].path, "/rest/api/3/issue/ABC-1/comment");
    assert_eq!(posted[0].authorization, basic());
    // The comment answered, as a number, as Jira gives `parentId` back.
    assert_eq!(
        posted[0].body["parentId"],
        json!(20000),
        "{}",
        posted[0].body
    );
    assert!(
        posted[0].body["body"]
            .to_string()
            .contains("Thanks, a fix is on app/login"),
        "{}",
        posted[0].body
    );
    // Not `acli`'s comment create, which has no way to name a parent.
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| !call.starts_with("jira workitem comment create")),
        "{:?}",
        acli_calls(&user)
    );
    // It is recorded as the agent's comment, under the id Jira answered.
    let records: Vec<_> = store(&user)
        .posts("app", "jira", "", "ABC-1")
        .unwrap()
        .into_iter()
        .filter(|post| post.kind == "comment")
        .map(|post| post.comment_id)
        .collect();
    assert_eq!(records, [Some("20100".to_owned())]);
}

#[test]
fn a_reply_to_a_reply_goes_under_the_first_comment_of_the_thread() {
    // Jira's threads are one level deep: it refuses a reply to a reply ("Parent comment not
    // found"), so the answer goes under the root.
    let jira = Jira::with_comments(json!([
        rest_comment("20000", ANA, None),
        rest_comment("20001", BEN, Some(20000)),
        rest_comment("20002", ANA, None),
    ]));
    let (user, mcp) = signed_in(
        &jira,
        json!([
            viewed("20000", "Ana", ANA, "I can reproduce"),
            viewed("20001", "Ben", BEN, "Same here"),
            viewed("20002", "Ana", ANA, "Another thread")
        ]),
    );

    // The card names the comment the answer goes under, not the reply it was asked of.
    held(&mcp.call(
        "tickets",
        json!({"action": "reply", "ticket": "ABC-1", "comment": "20001", "body": "Agreed"}),
    ));
    let card = card_of_the_last(&user);
    assert!(card.contains("under the comment 20000 by Ana"), "{card}");
    assert!(!card.contains("20001"), "{card}");
    allow(
        &user,
        requests(&user).last().unwrap()["number"].as_u64().unwrap(),
    );

    let posted = jira.posted();
    assert_eq!(posted.len(), 1, "{:?}", jira.received());
    assert_eq!(
        posted[0].body["parentId"],
        json!(20000),
        "{}",
        posted[0].body
    );
}

#[test]
fn a_jira_reply_quotes_nothing() {
    let jira = Jira::with_comments(json!([rest_comment("20000", ANA, None)]));
    let (user, mcp) = signed_in(
        &jira,
        json!([viewed("20000", "Ana", ANA, "I can reproduce")]),
    );

    held(&mcp.call(
        "tickets",
        json!({"action": "reply", "ticket": "ABC-1", "comment": "20000", "body": "Fixed"}),
    ));
    let card = card_of_the_last(&user);
    // The card says the comment answered, and the answer, not a quote that goes out.
    assert!(card.contains("20000") && card.contains("Fixed"), "{card}");
    assert!(!card.contains("wrote:"), "{card}");
    allow(
        &user,
        requests(&user).last().unwrap()["number"].as_u64().unwrap(),
    );

    let posted = jira.posted();
    assert_eq!(posted.len(), 1, "{:?}", jira.received());
    let document = &posted[0].body["body"];
    // One paragraph: the answer. No quote, no "wrote:", no mention of the author.
    assert_eq!(
        *document,
        json!({"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Fixed"}]}]}),
        "{document}"
    );
}

#[test]
fn a_jira_reply_without_a_tracker_entry_is_refused_before_anything_runs() {
    let jira = Jira::with_comments(json!([rest_comment("20000", ANA, None)]));
    let user = user(&jira);
    jira_with(&user, None);
    script(
        &user,
        json!([viewed("20000", "Ana", ANA, "I can reproduce")]),
    );
    let mcp = root_in(&user);
    let ask = json!({"action": "reply", "ticket": "ABC-1", "comment": "20000", "body": "Fixed"});

    let said = refused_at_once(&user, &mcp, ask.clone());

    assert!(said.contains("no vault entry to reply"), "{said}");
    assert!(said.contains("worklog_token"), "{said}");

    // An entry the project names and the vault does not hold: the agent may ask the user for it.
    let named = self::user(&jira);
    jira_with(&named, Some("jira-time"));
    script(
        &named,
        json!([viewed("20000", "Ana", ANA, "I can reproduce")]),
    );
    let mcp = root_in(&named);
    let said = refused_at_once(&named, &mcp, ask);
    assert!(said.contains("jira-time"), "{said}");
    assert!(said.contains("action: \"ask\""), "{said}");

    // Never a separate comment in its place, and no call to the site.
    for user in [&user, &named] {
        assert!(
            acli_calls(user)
                .iter()
                .all(|call| !call.starts_with("jira workitem comment create")),
            "{:?}",
            acli_calls(user)
        );
    }
    assert!(jira.received().is_empty(), "{:?}", jira.received());
}

#[test]
fn show_full_draws_each_reply_under_its_comment() {
    let jira = Jira::with_comments(json!([
        rest_comment("20000", ANA, None),
        rest_comment("20001", BEN, Some(20000)),
        rest_comment("20002", ANA, None),
        rest_comment("20003", ANA, Some(20000)),
    ]));
    let (user, mcp) = signed_in(
        &jira,
        json!([
            viewed("20000", "Ana", ANA, "I can reproduce"),
            viewed("20001", "Ben", BEN, "Same here"),
            viewed("20002", "Ana", ANA, "Another thread"),
            viewed("20003", "Ana", ANA, "Thanks both")
        ]),
    );

    let shown = read(
        &user,
        &mcp,
        json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
    );

    let lines: Vec<&str> = shown.lines().collect();
    let at = |id: &str| {
        lines
            .iter()
            .position(|line| line.contains(&format!("[comment {id}]")))
            .unwrap_or_else(|| panic!("{id}: {shown}"))
    };
    let indent = |id: &str| lines[at(id)].len() - lines[at(id)].trim_start().len();
    // Each reply follows its parent, indented, with its id; the other thread comes after.
    assert!(
        at("20000") < at("20001") && at("20001") < at("20003"),
        "{shown}"
    );
    assert!(at("20003") < at("20002"), "{shown}");
    assert!(indent("20001") > indent("20000"), "{shown}");
    assert!(indent("20003") > indent("20000"), "{shown}");
    assert_eq!(indent("20002"), indent("20000"), "{shown}");
}

#[test]
fn a_mention_in_a_jira_comment_becomes_a_mention_node() {
    let jira = Jira::with_comments(json!([]));
    let (user, mcp) = signed_in(&jira, json!([]));

    let done = written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1",
            "body": format!("[@Ana Example](mention:{ANA}) the fix is in, [@Ben Sample](mention:{BEN}) too")}),
    );

    assert!(done.contains("The comment was posted on ABC-1"), "{done}");
    assert!(!done.contains(SECRET), "{done}");
    let posted = jira.posted();
    assert_eq!(posted.len(), 1, "{:?}", jira.received());
    assert_eq!(posted[0].path, "/rest/api/3/issue/ABC-1/comment");
    assert_eq!(posted[0].authorization, basic());
    // A comment of its own, not a reply.
    assert!(
        posted[0].body.get("parentId").is_none(),
        "{}",
        posted[0].body
    );
    assert_eq!(
        mentions(&posted[0].body["body"]),
        [
            (ANA.to_owned(), "@Ana Example".to_owned()),
            (BEN.to_owned(), "@Ben Sample".to_owned())
        ],
        "{}",
        posted[0].body
    );
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| !call.starts_with("jira workitem comment create")),
        "{:?}",
        acli_calls(&user)
    );
    // A comment with no mention still goes through `acli`, as before.
    let before = jira.received().len();
    written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1", "body": "plain words"}),
    );
    assert_eq!(jira.received().len(), before);
    assert!(
        acli_calls(&user)
            .iter()
            .any(|call| call.starts_with("jira workitem comment create --key ABC-1")),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn a_mention_in_a_jira_comment_without_a_tracker_entry_is_refused() {
    let jira = Jira::with_comments(json!([]));
    let user = user(&jira);
    jira_with(&user, None);
    script(&user, json!([]));
    let mcp = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1",
            "body": format!("[@Ana Example](mention:{ANA}) fixed")}),
    );

    assert!(said.contains("no vault entry to mention"), "{said}");
    // A comment with no mention is as it was: held for the user.
    held(&mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "ABC-1", "body": "plain words"}),
    ));
    // A mention that names no account is refused, and says where the account ids are.
    let signed = self::user(&jira);
    jira_with(&signed, Some("jira-time"));
    entry(&signed);
    script(&signed, json!([]));
    let mcp = root_in(&signed);
    let said = refused_at_once(
        &signed,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1", "body": "[@Ana](mention:ana) fixed"}),
    );
    assert!(said.contains("people"), "{said}");
    assert!(jira.received().is_empty(), "{:?}", jira.received());
}

#[test]
fn the_refusal_without_a_tracker_entry_says_how_to_get_one() {
    let jira = Jira::with_comments(json!([]));
    let user = user(&jira);
    jira_with(&user, None);
    script(&user, json!([]));
    let mcp = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1",
            "body": format!("[@Ana Example](mention:{ANA}) fixed")}),
    );

    assert!(said.contains("vault ask"), "{said}");
}

/// The requests of the fake site that were `method`, on a path that ends with `end`.
fn sent(jira: &Jira, method: &str, end: &str) -> Vec<Received> {
    jira.received()
        .into_iter()
        .filter(|request| request.method == method && request.path.ends_with(end))
        .collect()
}

/// The line of the card that says who is notified.
fn notified_line(user: &User) -> String {
    let card = card_of_the_last(user);
    card.lines()
        .find(|line| line.starts_with("It mentions"))
        .unwrap_or_else(|| panic!("no line of who is notified: {card}"))
        .to_owned()
}

#[test]
fn a_description_with_a_mention_goes_through_jiras_api() {
    let jira = Jira::with_comments(json!([]));
    let (user, mcp) = signed_in(&jira, json!([]));

    let done = written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "ABC-1",
            "body": format!("Owned by [@Ana Example](mention:{ANA})")}),
    );

    assert!(done.contains("ABC-1 was changed"), "{done}");
    let puts = sent(&jira, "PUT", "/rest/api/3/issue/ABC-1");
    assert_eq!(puts.len(), 1, "{:?}", jira.received());
    assert_eq!(puts[0].authorization, basic());
    assert_eq!(
        mentions(&puts[0].body["fields"]["description"]),
        [(ANA.to_owned(), "@Ana Example".to_owned())],
        "{}",
        puts[0].body
    );
    // Not through `acli`, whose keeping a mention node is not confirmed.
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| !call.starts_with("jira workitem edit")),
        "{:?}",
        acli_calls(&user)
    );

    // The summary still goes through `acli`, with no description beside it.
    written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "ABC-1", "title": "Renamed",
            "body": format!("Owned by [@Ben Sample](mention:{BEN})")}),
    );
    let calls = acli_calls(&user);
    let edit = calls
        .iter()
        .find(|call| call.starts_with("jira workitem edit"))
        .unwrap_or_else(|| panic!("{calls:?}"));
    assert!(edit.contains("--summary Renamed"), "{edit}");
    assert!(!edit.contains("--description-file"), "{edit}");
    assert_eq!(sent(&jira, "PUT", "/rest/api/3/issue/ABC-1").len(), 2);

    // Words with no mention are as they were: `acli`, no call to the site.
    let before = jira.received().len();
    written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "ABC-1", "body": "plain words"}),
    );
    assert_eq!(jira.received().len(), before);
    assert!(
        acli_calls(&user)
            .iter()
            .any(|call| call.contains("--description-file")),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn a_created_description_with_a_mention_is_set_through_jiras_api() {
    let jira = Jira::with_comments(json!([]));
    let (user, mcp) = signed_in(&jira, json!([]));

    let done = written(
        &user,
        &mcp,
        json!({"action": "create", "title": "A new one", "project": "ABC",
            "body": format!("Ask [@Ben Sample](mention:{BEN})")}),
    );

    assert!(done.contains("ABC-1"), "{done}");
    // `acli` creates the work item, with no description (it could drop the mention node)...
    let calls = acli_calls(&user);
    let created = calls
        .iter()
        .find(|call| call.starts_with("jira workitem create"))
        .unwrap_or_else(|| panic!("{calls:?}"));
    assert!(!created.contains("--description-file"), "{created}");
    // ...and the description is set right after, by the REST API, in the same action.
    let puts = sent(&jira, "PUT", "/rest/api/3/issue/ABC-1");
    assert_eq!(puts.len(), 1, "{:?}", jira.received());
    assert_eq!(
        mentions(&puts[0].body["fields"]["description"]),
        [(BEN.to_owned(), "@Ben Sample".to_owned())],
        "{}",
        puts[0].body
    );
}

#[test]
fn a_description_with_a_mention_without_a_tracker_entry_is_refused() {
    let jira = Jira::with_comments(json!([]));
    let user = user(&jira);
    jira_with(&user, None);
    script(&user, json!([]));
    let mcp = root_in(&user);
    let body = format!("Ask [@Ben Sample](mention:{BEN})");

    for ask in [
        json!({"action": "edit", "ticket": "ABC-1", "body": body}),
        json!({"action": "create", "title": "A new one", "project": "ABC", "body": body}),
    ] {
        let said = refused_at_once(&user, &mcp, ask);
        assert!(said.contains("no vault entry to mention"), "{said}");
    }
    // Neither `acli` nor the site was called.
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| !call.starts_with("jira workitem")),
        "{:?}",
        acli_calls(&user)
    );
    assert!(jira.received().is_empty(), "{:?}", jira.received());
}

#[test]
fn the_card_names_whoever_jira_notifies_not_whoever_the_agent_writes() {
    let jira = Jira::with_comments(json!([rest_comment("20000", ANA, None)]));
    let (user, mcp) = signed_in(
        &jira,
        json!([viewed("20000", "Ana", ANA, "I can reproduce")]),
    );
    // The text says Ana; the account id is Ben's.
    let lie = format!("[@Ana Example](mention:{BEN}) look");

    held(&mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "ABC-1", "body": lie}),
    ));
    let line = notified_line(&user);
    assert!(line.contains("Ben Sample") && line.contains(BEN), "{line}");
    assert!(!line.contains("Ana Example"), "{line}");
    let number = requests(&user).last().unwrap()["number"].as_u64().unwrap();
    allow(&user, number);
    // What is posted reads as who is notified too.
    assert_eq!(
        mentions(&jira.posted()[0].body["body"]),
        [(BEN.to_owned(), "@Ben Sample".to_owned())]
    );

    // A reply, a description and a worklog's comment say it the same way.
    for ask in [
        json!({"action": "reply", "ticket": "ABC-1", "comment": "20000", "body": lie}),
        json!({"action": "edit", "ticket": "ABC-1", "body": lie}),
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h",
            "started": "2026-11-06T09:00:00Z", "body": lie}),
    ] {
        held(&mcp.call("tickets", ask.clone()));
        let line = notified_line(&user);
        assert!(
            line.contains("Ben Sample") && line.contains(BEN),
            "{ask}: {line}"
        );
        assert!(!line.contains("Ana Example"), "{ask}: {line}");
    }
}

#[test]
fn an_account_id_the_site_does_not_know_is_refused_before_the_card() {
    let jira = Jira::with_comments(json!([rest_comment("20000", ANA, None)]));
    let (user, mcp) = signed_in(
        &jira,
        json!([viewed("20000", "Ana", ANA, "I can reproduce")]),
    );
    let ghost = format!("[@Nobody](mention:{NOBODY}) look");

    for ask in [
        json!({"action": "comment", "ticket": "ABC-1", "body": ghost}),
        json!({"action": "reply", "ticket": "ABC-1", "comment": "20000", "body": ghost}),
        json!({"action": "edit", "ticket": "ABC-1", "body": ghost}),
        json!({"action": "create", "title": "A new one", "project": "ABC", "body": ghost}),
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h",
            "started": "2026-11-06T09:00:00Z", "body": ghost}),
    ] {
        let said = refused_at_once(&user, &mcp, ask.clone());
        assert!(said.contains(NOBODY), "{ask}: {said}");
        assert!(said.contains("nothing was written"), "{ask}: {said}");
    }
    // Nothing was written, to the site or through `acli`.
    assert!(
        jira.received()
            .iter()
            .all(|request| request.method == "GET"),
        "{:?}",
        jira.received()
    );
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| !call.starts_with("jira workitem create")
                && !call.starts_with("jira workitem edit")),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn a_mention_in_an_edited_jira_comment_goes_through_jiras_api() {
    let jira = Jira::with_comments(json!([]));
    let (user, mcp) = signed_in(&jira, json!([viewed("20100", "Dev", "me", "first")]));
    // The agent's own comment, recorded under the id Jira answered.
    written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1",
            "body": format!("[@Ana Example](mention:{ANA}) first")}),
    );

    let edit = json!({"action": "comment_edit", "ticket": "ABC-1", "comment": "20100",
        "body": format!("[@Ana Example](mention:{BEN}) second")});
    held(&mcp.call("tickets", edit.clone()));
    let line = notified_line(&user);
    assert!(line.contains("Ben Sample") && line.contains(BEN), "{line}");
    assert!(
        card_of_the_last(&user).contains("through Jira's API with the vault entry jira-time"),
        "{}",
        card_of_the_last(&user)
    );
    let number = requests(&user).last().unwrap()["number"].as_u64().unwrap();
    let done = allow(&user, number);

    assert!(
        done.contains("The comment 20100 of ABC-1 was changed"),
        "{done}"
    );
    let puts = sent(&jira, "PUT", "/rest/api/3/issue/ABC-1/comment/20100");
    assert_eq!(puts.len(), 1, "{:?}", jira.received());
    assert_eq!(puts[0].authorization, basic());
    assert_eq!(
        mentions(&puts[0].body["body"]),
        [(BEN.to_owned(), "@Ben Sample".to_owned())],
        "{}",
        puts[0].body
    );
    assert!(
        acli_calls(&user)
            .iter()
            .all(|call| !call.starts_with("jira workitem comment update")),
        "{:?}",
        acli_calls(&user)
    );
    // Still the agent's comment, recorded once, by its id.
    let records: Vec<_> = store(&user)
        .posts("app", "jira", "", "ABC-1")
        .unwrap()
        .into_iter()
        .filter(|post| post.kind == "comment")
        .map(|post| post.comment_id)
        .collect();
    assert_eq!(records, [Some("20100".to_owned())]);

    // An id the site does not know is refused before the card.
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "comment_edit", "ticket": "ABC-1", "comment": "20100",
            "body": format!("[@Nobody](mention:{NOBODY}) second")}),
    );
    assert!(said.contains(NOBODY), "{said}");
}

#[test]
fn a_mention_in_an_edited_jira_comment_without_a_tracker_entry_is_refused() {
    let jira = Jira::with_comments(json!([]));
    let user = user(&jira);
    jira_with(&user, None);
    script(&user, json!([viewed("20100", "Dev", "me", "first")]));
    let mcp = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "comment_edit", "ticket": "ABC-1", "comment": "20100",
            "body": format!("[@Ana Example](mention:{ANA}) second")}),
    );

    assert!(said.contains("no vault entry to mention"), "{said}");
    assert!(jira.received().is_empty(), "{:?}", jira.received());
}

#[test]
fn a_mention_in_a_worklog_comment_is_checked_and_said() {
    let jira = Jira::with_comments(json!([]));
    let (user, mcp) = signed_in(&jira, json!([]));

    let done = written(
        &user,
        &mcp,
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h",
            "started": "2026-11-06T09:00:00Z",
            "body": format!("[@Ana Example](mention:{BEN}) paired")}),
    );

    assert!(done.contains("logged 1h on ABC-1"), "{done}");
    let worklogs = sent(&jira, "POST", "/worklog");
    assert_eq!(worklogs.len(), 1, "{:?}", jira.received());
    assert_eq!(
        mentions(&worklogs[0].body["comment"]),
        [(BEN.to_owned(), "@Ben Sample".to_owned())],
        "{}",
        worklogs[0].body
    );
}

#[test]
fn a_mention_comment_does_not_need_acli_signed_in() {
    let jira = Jira::with_comments(json!([]));
    let (user, mcp) = signed_in(&jira, json!([]));
    // The comment goes through the REST API alone: a signed-out `acli` is no reason to fail it.
    user.acli_script(json!({"signed_in": false, "items": {}}));

    let done = written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1",
            "body": format!("[@Ana Example](mention:{ANA}) fixed")}),
    );

    assert!(done.contains("The comment was posted on ABC-1"), "{done}");
    assert_eq!(jira.posted().len(), 1, "{:?}", jira.received());
}

#[test]
fn the_card_says_who_a_mention_notifies() {
    let jira = Jira::with_comments(json!([rest_comment("20000", ANA, None)]));
    let (user, mcp) = signed_in(
        &jira,
        json!([viewed("20000", "Ana", ANA, "I can reproduce")]),
    );

    held(&mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "ABC-1",
            "body": format!("[@Ana Example](mention:{ANA}) and [@Ben Sample](mention:{BEN}) look")}),
    ));
    let card = card_of_the_last(&user);
    assert!(
        card.contains(&format!(
            "It mentions Ana Example ({ANA}), Ben Sample ({BEN}), who are notified"
        )),
        "{card}"
    );
    assert!(
        card.contains("through Jira's API with the vault entry jira-time"),
        "{card}"
    );

    // A reply says it too, beside the comment it answers.
    held(&mcp.call(
        "tickets",
        json!({"action": "reply", "ticket": "ABC-1", "comment": "20000",
            "body": format!("[@Ben Sample](mention:{BEN}) fixed")}),
    ));
    let card = card_of_the_last(&user);
    assert!(
        card.contains(&format!("It mentions Ben Sample ({BEN}), who is notified")),
        "{card}"
    );
    assert!(card.contains("20000"), "{card}");

    // Nothing is said when no one is mentioned.
    held(&mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "ABC-1", "body": "no one here"}),
    ));
    assert!(!card_of_the_last(&user).contains("notified"));
}

#[test]
fn a_github_reply_still_quotes_the_comment() {
    let jira = Jira::with_comments(json!([]));
    let user = user(&jira);
    fixture(&user, json!({"kind": "github", "repository": "."}));
    user.gh_script(json!({"issues": {"acme/app": {"1": {
        "number": 1, "title": "The login fails", "state": "OPEN", "body": "", "comments": [
            {"author": {"login": "alice"}, "createdAt": "2026-10-01T10:00:00Z",
             "body": "I can reproduce\nit fails twice",
             "url": "https://github.com/acme/app/issues/1#issuecomment-500"}
        ]}}}, "commented": 501}));
    let mcp = root_in(&user);

    let done = written(
        &user,
        &mcp,
        json!({"action": "reply", "ticket": "#1", "comment": "issuecomment-500",
            "body": "Thanks, a fix is on app/login"}),
    );

    assert!(
        done.contains("The reply to the comment issuecomment-500 of #1"),
        "{done}"
    );
    let calls: Vec<String> = user
        .gh_received()
        .iter()
        .map(|call| {
            call["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let call = calls
        .iter()
        .find(|c| c.starts_with("issue comment 1 -R ") && c.contains(" --body "))
        .unwrap_or_else(|| panic!("{calls:?}"));
    assert!(
        call.ends_with(
            "> @alice wrote:\n> I can reproduce\n> it fails twice\n\nThanks, a fix is on app/login"
        ),
        "{call}"
    );
    // No call to a Jira site, and a `mention:` link is plain text on GitHub.
    assert!(jira.received().is_empty());
}
