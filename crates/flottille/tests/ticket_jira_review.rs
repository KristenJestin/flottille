//! What a review reads of Jira (#1126, `docs/tickets.md`, "A search for a review"): a mention
//! as a name, a search that brings each ticket's comments and links, and whose account the user
//! is. Jira is the fake `acli` for the work items and a fake HTTP site for `GET
//! /rest/api/3/myself`, the vault's keyring the daemon's memory: no test reaches a site, the
//! user's keyring or anyone's sign-in. Names and keys are invented.

use crate::common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

const SITE: &str = "example.atlassian.net";
const ADA: &str = "5b10ac8d82e05b22cc7d4ef5";
const GRACE: &str = "712020:9f8e7d6c-1111-4222-8333-444455556666";
const SECRET: &str = "r3view-t0ken-never-shown";
/// What the fake site's `myself` answers: the account the entry belongs to.
const ME: &str = "5b10ac8d82e05b22cc7d4ef7";
const BASE: &str = "FLOTTILLE_TEST_JIRA_BASE";

/// A Jira that answers `GET /rest/api/3/myself` and remembers its requests' paths.
struct Jira {
    base: String,
    paths: Arc<Mutex<Vec<String>>>,
}

impl Jira {
    fn serving() -> Jira {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let paths = Arc::new(Mutex::new(Vec::new()));
        let kept = paths.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut raw = [0u8; 4096];
                let read = stream.read(&mut raw).unwrap_or(0);
                let head = String::from_utf8_lossy(&raw[..read]).into_owned();
                // Without the query string, which an empty list of parameters leaves as `?`.
                let path = head
                    .lines()
                    .next()
                    .and_then(|line| line.split(' ').nth(1))
                    .and_then(|target| target.split('?').next())
                    .unwrap_or_default()
                    .to_owned();
                kept.lock().unwrap().push(path.clone());
                let said = match path.as_str() {
                    "/rest/api/3/myself" => json!({"accountId": ME, "displayName": "Casey Example",
                        "emailAddress": "casey@example.org", "active": true}),
                    _ => json!({}),
                }
                .to_string();
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                         {}\r\nConnection: close\r\n\r\n{said}",
                        said.len()
                    )
                    .as_bytes(),
                );
            }
        });
        Jira { base, paths }
    }

    fn asked(&self, path: &str) -> usize {
        self.paths
            .lock()
            .unwrap()
            .iter()
            .filter(|asked| *asked == path)
            .count()
    }
}

fn document(content: Value) -> Value {
    json!({"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": content}]})
}

fn text(words: &str) -> Value {
    json!({"type": "text", "text": words})
}

fn mention(account: &str, name: &str) -> Value {
    json!({"type": "mention", "attrs": {"id": account, "text": format!("@{name}")}})
}

/// A comment as Jira's REST gives it.
fn comment(id: &str, who: (&str, &str), at: &str, body: Value) -> Value {
    json!({"id": id, "author": {"displayName": who.0, "accountId": who.1}, "created": at,
        "body": body})
}

fn status(name: &str, category: &str) -> Value {
    json!({"name": name, "statusCategory": {"key": category}})
}

/// A work item of the fake `acli`.
fn item(key: &str, title: &str, status: Value, comments: Value, links: Value) -> Value {
    json!({"key": key, "self": format!("https://{SITE}/rest/api/3/issue/1"),
        "fields": {"summary": title, "status": status, "labels": [],
            "comment": {"comments": comments}, "issuelinks": links}})
}

fn link(outward: bool, kind: (&str, &str, &str), other: &str, status: &str) -> Value {
    let issue = json!({"key": other, "fields": {"summary": "The other one",
        "status": {"name": status}}});
    let mut link = json!({"id": "10001",
        "type": {"name": kind.0, "inward": kind.1, "outward": kind.2}});
    link[if outward {
        "outwardIssue"
    } else {
        "inwardIssue"
    }] = issue;
    link
}

fn user(jira: &Jira) -> User {
    User::with_fake_agent()
        .with_fake_acli()
        .with_fake_gh()
        .with_env(BASE, &jira.base)
}

/// A project `app` over a throwaway repository, its Jira tracker with or without the entry.
fn fixture(user: &User, entry: bool) {
    let mut tracker = json!({"kind": "jira", "site": SITE, "projects": ["ABC"]});
    if entry {
        tracker["worklog_token"] = "jira-time".into();
    }
    project(user, tracker, entry);
}

/// The project over `tracker`; the vault's entry `jira-time` is set when `entry`.
fn project(user: &User, tracker: Value, entry: bool) {
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
    if entry {
        user.call(
            "vault.set",
            json!({"name": "jira-time", "secret": SECRET, "username": "casey@example.org",
                "project": "app"}),
        )
        .unwrap();
    }
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": tracker}),
    )
    .unwrap();
}

fn root_in(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

/// The project over the fake site, `acli` answering `script`.
fn setup(entry: bool, script: Value) -> (Jira, User, Mcp) {
    let jira = Jira::serving();
    let user = user(&jira);
    fixture(&user, entry);
    user.acli_script(script);
    let mcp = root_in(&user);
    (jira, user, mcp)
}

fn read(mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("tickets", args);
    assert!(!answer.error, "{}", answer.text);
    answer.text
}

#[test]
fn a_comment_s_mentions_are_names() {
    let comments = json!([comment(
        "20000",
        ("Casey Example", ME),
        "2026-10-01T09:00:00.000+0000",
        document(json!([
            text("Could you look, "),
            mention(ADA, "Ada Lovelace"),
            text(" and "),
            mention(GRACE, "Grace Hopper"),
            text("?")
        ]))
    )]);
    let (_jira, _user, mcp) = setup(
        false,
        json!({"items": {"ABC-1": item("ABC-1", "The login fails", status("To Do", "new"),
            comments, json!([]))}}),
    );

    let shown = read(
        &mcp,
        json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
    );

    assert!(
        shown.contains("Could you look, @Ada Lovelace and @Grace Hopper?"),
        "{shown}"
    );
    // The account ids are kept beside the text, for a comparison, never inside it.
    assert!(!shown.contains("](mention:"), "{shown}");
    assert!(
        shown.contains(&format!("mentions @Ada Lovelace (account {ADA})")),
        "{shown}"
    );
    assert!(
        shown.contains(&format!("@Grace Hopper (account {GRACE})")),
        "{shown}"
    );
}

#[test]
fn markup_the_tracker_adds_is_removed_from_the_text() {
    let comments = json!([comment(
        "20000",
        ("Casey Example", ME),
        "2026-10-01T09:00:00.000+0000",
        document(json!([text(
            "Please <custom data-type=\"mention\" data-id=\"x\">@Ada Lovelace</custom> check"
        )]))
    )]);
    let (_jira, _user, mcp) = setup(
        false,
        json!({"items": {"ABC-1": item("ABC-1", "The login fails", status("To Do", "new"),
            comments, json!([]))}}),
    );

    let shown = read(
        &mcp,
        json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
    );

    assert!(shown.contains("Please @Ada Lovelace check"), "{shown}");
    assert!(
        !shown.contains("<custom") && !shown.contains("</custom>"),
        "{shown}"
    );
}

/// Three tickets, each with a comment, one with a link.
fn review_script() -> Value {
    let first = item(
        "ABC-1",
        "The login fails",
        status("In Review", "indeterminate"),
        json!([
            comment(
                "20000",
                ("Casey Example", ME),
                "2026-10-01T09:00:00.000+0000",
                document(json!([
                    text("Ready for you, "),
                    mention(ADA, "Ada Lovelace")
                ]))
            ),
            comment(
                "20001",
                ("Ada Lovelace", ADA),
                "2026-10-02T10:30:00.000+0000",
                document(json!([text("Looking now")]))
            )
        ]),
        json!([link(
            true,
            ("Blocks", "is blocked by", "blocks"),
            "ABC-9",
            "To Do"
        )]),
    );
    let second = item(
        "ABC-2",
        "The logout hangs",
        status("To Do", "new"),
        json!([comment(
            "20002",
            ("Grace Hopper", GRACE),
            "2026-10-03T08:00:00.000+0000",
            document(json!([text("Second ticket comment")]))
        )]),
        json!([]),
    );
    let third = item(
        "ABC-3",
        "The reset mail is late",
        status("Done", "done"),
        json!([comment(
            "20003",
            ("Ada Lovelace", ADA),
            "2026-10-04T08:00:00.000+0000",
            document(json!([text("Third ticket comment")]))
        )]),
        json!([]),
    );
    // A page holds two: only `--paginate` reads them all.
    json!({"search": [first, second, third], "page": 2})
}

const JQL: &str = "project = ABC AND statusCategory != Done AND comment ~ \"Ada\"";

#[test]
fn a_search_pages_to_the_end_with_comments_and_links() {
    let (_jira, user, mcp) = setup(false, review_script());

    let found = read(
        &mcp,
        json!({"action": "search", "jql": JQL, "detail": "full"}),
    );

    for key in ["ABC-1", "ABC-2", "ABC-3"] {
        assert!(found.contains(key), "{key}: {found}");
    }
    // Status and its category, each comment with its author, date and text, mentions as names.
    assert!(
        found.contains("In Review (category: indeterminate)"),
        "{found}"
    );
    assert!(found.contains("To Do (category: new)"), "{found}");
    assert!(found.contains("Done (category: done)"), "{found}");
    assert!(found.contains("Ada Lovelace"), "{found}");
    assert!(found.contains("2026-10-02T10:30:00.000+0000"), "{found}");
    assert!(found.contains("Looking now"), "{found}");
    assert!(found.contains("Ready for you, @Ada Lovelace"), "{found}");
    assert!(found.contains("Second ticket comment"), "{found}");
    assert!(
        found.contains(&format!("mentions @Ada Lovelace (account {ADA})")),
        "{found}"
    );
    assert!(!found.contains("](mention:"), "{found}");
    // It asked for every page; a search takes no `comment` nor `issuelinks` among its fields
    // (`acli` refuses them), so each ticket is read with its comments and links.
    let calls = user.acli_received();
    let search = calls
        .iter()
        .find(|call| call["args"][2] == "search")
        .expect("a search");
    let args = search["args"].to_string();
    assert!(args.contains("--paginate"), "{args}");
    assert!(args.contains("comment ~"), "{args}");
    let views: Vec<String> = calls
        .iter()
        .filter(|call| call["args"][2] == "view")
        .map(|call| call["args"].to_string())
        .collect();
    for key in ["ABC-1", "ABC-2", "ABC-3"] {
        let view = views.iter().find(|view| view.contains(key));
        let view = view.unwrap_or_else(|| panic!("{key} was not read: {views:?}"));
        assert!(
            view.contains("comment") && view.contains("issuelinks"),
            "{view}"
        );
    }
}

#[test]
fn a_search_with_a_limit_stops_at_it() {
    let (_jira, _user, mcp) = setup(false, review_script());

    let found = read(
        &mcp,
        json!({"action": "search", "jql": JQL, "detail": "full", "limit": 1}),
    );

    assert!(found.contains("ABC-1"), "{found}");
    assert!(
        !found.contains("ABC-2") && !found.contains("ABC-3"),
        "{found}"
    );
}

#[test]
fn blocks_links_are_given_with_the_other_key() {
    let (_jira, _user, mcp) = setup(false, review_script());

    let found = read(
        &mcp,
        json!({"action": "search", "jql": JQL, "detail": "full"}),
    );

    assert!(found.contains("blocks ABC-9"), "{found}");
    let shown = read(&mcp, json!({"action": "show", "ticket": "ABC-1"}));
    assert!(shown.contains("blocks ABC-9"), "{shown}");
}

#[test]
fn the_tool_says_which_account_is_the_user() {
    let (jira, _user, mcp) = setup(true, review_script());

    let said = read(&mcp, json!({"action": "me"}));

    assert!(said.contains("Casey Example"), "{said}");
    assert!(said.contains(&format!("account {ME}")), "{said}");
    assert_eq!(jira.asked("/rest/api/3/myself"), 1);
    // Never the secret.
    assert!(!said.contains(SECRET), "{said}");
}

#[test]
fn the_account_of_the_user_needs_the_connection_s_entry() {
    let (jira, _user, mcp) = setup(false, review_script());

    let answer = mcp.call("tickets", json!({"action": "me"}));

    assert!(answer.text.contains("entry"), "{}", answer.text);
    assert_eq!(jira.asked("/rest/api/3/myself"), 0);
}

#[test]
fn the_tool_says_which_github_account_is_the_user() {
    let jira = Jira::serving();
    let user = user(&jira);
    project(&user, json!({"kind": "github", "repository": "."}), false);
    user.gh_script(json!({"user": {"login": "casey-example"}}));
    let mcp = root_in(&user);

    let said = read(&mcp, json!({"action": "me"}));

    assert!(said.contains("casey-example"), "{said}");
}

#[test]
fn a_list_takes_no_detail_full_nor_limit_and_github_no_full_search() {
    let (_jira, _user, mcp) = setup(false, review_script());
    for args in [
        json!({"action": "list", "detail": "full"}),
        json!({"action": "list", "limit": 5}),
    ] {
        let answer = mcp.call("tickets", args);
        assert!(answer.text.contains("search"), "{}", answer.text);
    }

    let jira = Jira::serving();
    let user = user(&jira);
    project(&user, json!({"kind": "github", "repository": "."}), false);
    let mcp = root_in(&user);
    let answer = mcp.call(
        "tickets",
        json!({"action": "search", "query": "login", "detail": "full"}),
    );
    assert!(answer.text.contains("Jira"), "{}", answer.text);
}
