//! An agent asks the user for a vault entry (#566, #947, `docs/vault.md`, "Asking the user for an
//! entry"): any new entry, prefilled by the agent, or by the feature that names it; the refusal
//! that met a missing entry says it can be asked for, `vault {action: "ask"}` answers at once, the
//! user's own form of Settings, Vault stores it, and the agent is told what the user did, never
//! the secret. Against the real daemon with the fake ACP agent and the in-memory keyring the
//! tests' configuration chooses; no real agent, keyring or site is reached.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

const SECRET: &str = "w0rklog-t0ken-asked-for";
const ACCOUNT: &str = "dev@example.com";
const TRACKER: fn(&str) -> Value = |entry| {
    json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"],
        "worklog_token": entry})
};

/// A project `app` over a throwaway repository whose Jira tracker names the entry `entry`, which
/// the vault does not hold.
fn project(user: &User, entry: &str) {
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
    tracker(user, entry);
}

fn tracker(user: &User, entry: &str) {
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": TRACKER(entry)}),
    )
    .unwrap();
}

/// A root agent of `app` and the client of its tools.
fn root_in(user: &User) -> (Mcp, String) {
    root_of(user, "app")
}

/// A root agent of `project` and the client of its tools.
fn root_of(user: &User, project: &str) -> (Mcp, String) {
    let opened = user.received_requests("session/new").len();
    let run = user
        .call("agent.run", json!({"project": project, "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        Mcp::initialize(&url, &authorization),
        run["agent"]["id"].as_str().unwrap().to_owned(),
    )
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

fn asks(user: &User) -> Vec<Value> {
    user.call("vault.asks", json!({})).unwrap()["asks"]
        .as_array()
        .unwrap()
        .clone()
}

fn ask(mcp: &Mcp, name: &str, why: &str) -> common::mcp::Answer {
    mcp.call("vault", json!({"action": "ask", "name": name, "why": why}))
}

/// What the agent is handed once the user answered the request `number`.
fn told(user: &User, number: u64, agent: &str) -> String {
    let id = format!("vault-ask-{number}-{agent}");
    eventually("the delivery", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

#[test]
fn a_refusal_for_a_missing_entry_says_it_can_be_asked_for() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    // The agent has the tool though the project sees no entry: one is needed.
    assert!(mcp.names().iter().any(|name| name == "vault"), "not listed");
    let said = mcp.call(
        "tickets",
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h"}),
    );
    assert!(said.error, "{}", said.text);
    for how in [
        "jira-time",
        "vault {action: \"ask\", name: \"jira-time\", why",
    ] {
        assert!(said.text.contains(how), "{how} is not said: {}", said.text);
    }
}

#[test]
fn a_missing_worklog_entry_can_be_asked_for_and_the_user_sets_it() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, agent) = root_in(&user);

    let answer = ask(&mcp, "jira-time", "to log the two hours spent on ABC-1");
    assert!(!answer.error, "{}", answer.text);
    // At once: nothing waits for the user.
    assert!(answer.text.contains("jira-time"), "{}", answer.text);

    let open = asks(&user);
    assert_eq!(open.len(), 1, "{open:?}");
    let request = &open[0];
    assert_eq!(request["name"], "jira-time");
    assert_eq!(request["project"], "app");
    assert_eq!(request["agent"], agent.as_str());
    assert_eq!(request["why"], "to log the two hours spent on ABC-1");
    // The feature that names it prefills what it is for and its use.
    assert!(
        request["description"]
            .as_str()
            .is_some_and(|d| d.contains("Jira")),
        "prefilled by Flottille: {request}"
    );
    assert_eq!(request["for"], "worklog:app", "{request}");
    assert_eq!(request["takes_username"], true, "{request}");
    let inbox = user.call("inbox.list", json!({})).unwrap();
    let entry = inbox["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "vault")
        .unwrap_or_else(|| panic!("no inbox entry: {inbox}"))
        .clone();
    assert!(entry["text"].as_str().unwrap().contains("jira-time"));

    // The user saves Settings, Vault's form the card opened, prefilled.
    let number = request["ask"].as_u64().unwrap();
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": ACCOUNT, "project": "app",
            "description": request["description"], "for": request["for"]}),
    )
    .unwrap();

    let set = entry_of(&user, "jira-time");
    assert_eq!(set["project"], "app");
    assert_eq!(set["username"], ACCOUNT);
    assert_eq!(set["for"], "worklog:app");
    assert!(asks(&user).is_empty(), "answered, not open");

    let body = told(&user, number, &agent);
    assert!(
        body.contains("the user set the vault entry `jira-time`"),
        "{body}"
    );
    // Nothing the agent receives, or the daemon keeps (its store, its logs), holds the secret.
    assert!(!body.contains(SECRET), "{body}");
    nothing_holds(&user, SECRET);
}

/// No file of the daemon's (its store, its logs) holds `secret`.
fn nothing_holds(user: &User, secret: &str) {
    let mut folders = vec![user.path("state"), user.path("runtime")];
    while let Some(folder) = folders.pop() {
        for item in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = item.path();
            if path.is_dir() {
                folders.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let found = bytes.windows(secret.len()).any(|w| w == secret.as_bytes());
                assert!(!found, "{} holds the secret", path.display());
            }
        }
    }
}

#[test]
fn an_agent_asks_for_an_entry_no_feature_names() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    connection(&user, "browser");
    let (mcp, agent) = root_in(&user);

    let answer = mcp.call(
        "vault",
        json!({"action": "ask", "name": "staging-login", "why": "to sign in to staging",
            "url": "https://staging.example.com/login", "username": "tester@example.com"}),
    );
    assert!(!answer.error, "{}", answer.text);
    let open = asks(&user);
    assert_eq!(open.len(), 1, "{open:?}");
    let request = &open[0];
    assert_eq!(request["name"], "staging-login");
    assert_eq!(request["url"], "https://staging.example.com/login");
    assert_eq!(request["username"], "tester@example.com");
    assert_eq!(request["why"], "to sign in to staging");
    // This project's, bound to no use, as asked.
    assert_eq!(request["project"], "app");
    assert_eq!(request["every"], false, "{request}");
    assert_eq!(request["for"], Value::Null, "{request}");
    let number = request["ask"].as_u64().unwrap();

    // A use asked for: a connection's entry is every project's.
    let bound = mcp.call(
        "vault",
        json!({"action": "ask", "name": "shop-test", "why": "to sign in to the shop",
            "url": "https://shop.example.com/login", "for": "connection:browser"}),
    );
    assert!(!bound.error, "{}", bound.text);
    let shop = asks(&user)
        .into_iter()
        .find(|ask| ask["name"] == "shop-test")
        .unwrap();
    assert_eq!(shop["for"], "connection:browser", "{shop}");
    assert_eq!(shop["every"], true, "{shop}");

    // A use the agent's project cannot have is refused.
    let refused = mcp.call(
        "vault",
        json!({"action": "ask", "name": "elsewhere", "why": "x", "for": "worklog:other"}),
    );
    assert!(refused.error, "{}", refused.text);

    // The user saves the form as prefilled: it is stored bound as asked, and the agent is told,
    // never the secret.
    user.call(
        "vault.set",
        json!({"name": "staging-login", "secret": SECRET, "username": "tester@example.com",
            "url": "https://staging.example.com/login", "project": "app"}),
    )
    .unwrap();
    let set = entry_of(&user, "staging-login");
    assert_eq!(set["project"], "app");
    assert_eq!(set["url"], "https://staging.example.com/login");
    let body = told(&user, number, &agent);
    assert!(
        body.contains("the user set the vault entry `staging-login`"),
        "{body}"
    );
    assert!(!body.contains(SECRET), "{body}");
    user.call(
        "vault.set",
        json!({"name": "shop-test", "secret": "sh0p-s3cret-typed", "for": "connection:browser",
            "url": "https://shop.example.com/login"}),
    )
    .unwrap();
    assert_eq!(entry_of(&user, "shop-test")["for"], "connection:browser");
    assert!(asks(&user).is_empty(), "both answered");
    nothing_holds(&user, SECRET);
}

#[test]
fn an_ask_for_an_existing_entry_is_refused_naming_it() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    other_project(&user);
    user.call(
        "vault.set",
        json!({"name": "deploy-key", "secret": "d3ploy-s3cret-key", "project": "app"}),
    )
    .unwrap();
    user.call(
        "vault.set",
        json!({"name": "theirs", "secret": "th3irs-s3cret-key", "project": "other"}),
    )
    .unwrap();
    let (mcp, _) = root_in(&user);

    for (name, says) in [("deploy-key", "use it"), ("theirs", "another name")] {
        let refused = ask(&mcp, name, "to deploy");
        assert!(refused.error, "{}", refused.text);
        assert!(refused.text.starts_with("refused:"), "{}", refused.text);
        assert!(refused.text.contains(name), "{}", refused.text);
        assert!(refused.text.contains(says), "{}", refused.text);
        // Another project's entry is not described to this one.
        assert!(!refused.text.contains("project other"), "{}", refused.text);
    }
    assert!(asks(&user).is_empty());
}

#[test]
fn the_ask_for_an_entry_has_no_field_for_a_secret() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    let tool = mcp
        .tools()
        .into_iter()
        .find(|tool| tool["name"] == "vault")
        .unwrap();
    let properties = tool["inputSchema"]["properties"].as_object().unwrap();
    let mut fields: Vec<&str> = properties.keys().map(String::as_str).collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "action",
            "connection",
            "for",
            "name",
            "project",
            "url",
            "username",
            "why"
        ],
        "{tool}"
    );

    // An agent that tries to give one anyway is refused, and nothing keeps it.
    let refused = mcp.call(
        "vault",
        json!({"action": "ask", "name": "jira-time", "why": "x", "secret": SECRET}),
    );
    assert!(refused.error, "{}", refused.text);
    assert!(!refused.text.contains(SECRET), "{}", refused.text);
    assert!(asks(&user).is_empty());
    assert!(
        user.call("vault.list", json!({})).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_second_ask_for_the_same_name_joins_the_first() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (first, one) = root_in(&user);
    let (second, two) = root_in(&user);

    assert!(!ask(&first, "staging-login", "to sign in to staging").error);
    let joined = ask(&second, "staging-login", "to test the checkout");
    assert!(!joined.error, "{}", joined.text);
    assert!(joined.text.contains("joins"), "{}", joined.text);
    let open = asks(&user);
    assert_eq!(open.len(), 1, "one request: {open:?}");
    let number = open[0]["ask"].as_u64().unwrap();

    user.call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();
    for agent in [&one, &two] {
        let body = told(&user, number, agent);
        assert!(body.contains("the user dismissed"), "{body}");
    }
}

#[test]
fn a_dismissed_name_may_be_asked_again() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    assert!(!ask(&mcp, "jira-time", "to log time").error);
    let number = asks(&user)[0]["ask"].as_u64().unwrap();
    user.call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();
    assert!(asks(&user).is_empty());

    // Still offered by the refusal, and asked again: the ask only opens the user's own form.
    let said = mcp.call(
        "tickets",
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h"}),
    );
    assert!(said.text.contains("action: \"ask\""), "{}", said.text);
    let again = ask(&mcp, "jira-time", "to log time, again");
    assert!(!again.error, "{}", again.text);
    assert_eq!(asks(&user).len(), 1);
}

/// A second project `other` over its own throwaway repository.
fn other_project(user: &User) {
    let folder = user.path("work/other");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "other\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "other"}))
        .unwrap();
}

fn entry_of(user: &User, name: &str) -> Value {
    user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("no entry {name}"))
        .clone()
}

#[test]
fn an_entry_made_from_a_feature_is_bound_to_it() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);
    assert!(!ask(&mcp, "jira-time", "to log time").error);
    let prefilled = asks(&user)[0]["for"].clone();

    // The form the worklog's request opens makes the entry the worklog's.
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": ACCOUNT, "project": "app",
            "for": prefilled}),
    )
    .unwrap();
    assert_eq!(entry_of(&user, "jira-time")["for"], "worklog:app");

    // So does a set from a feature's own screen, which says what it is for.
    user.call(
        "vault.set",
        json!({"name": "almanac-token", "secret": "s3cret-of-almanac", "for": "connection:almanac"}),
    )
    .unwrap();
    assert_eq!(
        entry_of(&user, "almanac-token")["for"],
        "connection:almanac"
    );

    // Replacing its secret from the Vault page, which names no use, keeps the binding; another
    // use is refused, naming the one it has.
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": "an0ther-w0rklog-t0ken", "username": ACCOUNT,
            "project": "app"}),
    )
    .unwrap();
    assert_eq!(entry_of(&user, "jira-time")["for"], "worklog:app");
    let (_, refused) = user
        .call(
            "vault.set",
            json!({"name": "jira-time", "secret": "an0ther-w0rklog-t0ken",
                "for": "connection:almanac"}),
        )
        .unwrap_err();
    assert!(
        refused.contains("the worklog of the project app"),
        "{refused}"
    );
    assert!(!refused.contains("an0ther-w0rklog-t0ken"), "{refused}");
}

/// The connection `name` declared on the fake MCP server, never reviewed: it exists, and gives
/// no tool.
fn connection(user: &User, name: &str) {
    let fake = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-mcp");
    assert!(fake.exists(), "`cargo build -p flottille-fake-mcp` first");
    let script: PathBuf = user.path(&format!("{name}-script.json"));
    std::fs::write(
        &script,
        json!({"server": {"name": name, "version": "1.0.0"}, "tools": []}).to_string(),
    )
    .unwrap();
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    user.call(
        "connection.add",
        json!({"name": name, "about": format!("The {name} of the tests."),
            "command": [fake, "--script", script]}),
    )
    .unwrap();
}

/// The text of the inbox entry of the request for `name`.
fn inbox_text(user: &User, name: &str) -> String {
    let inbox = user.call("inbox.list", json!({})).unwrap();
    inbox["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "vault")
        .filter_map(|e| e["text"].as_str())
        .find(|text| text.contains(name))
        .unwrap_or_else(|| panic!("no inbox entry for {name}: {inbox}"))
        .to_owned()
}

#[test]
fn a_look_alike_host_is_shown_in_ascii() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    // A Cyrillic "а" in the place of a Latin "a": the host is said as the network reads it.
    let answer = mcp.call(
        "vault",
        json!({"action": "ask", "name": "bank-login", "why": "to sign in to the bank",
            "url": "https://b\u{0430}nk.example/login"}),
    );
    assert!(!answer.error, "{}", answer.text);
    let request = asks(&user)[0].clone();
    assert_eq!(request["host"], "xn--bnk-6cd.example", "{request}");
    let said = inbox_text(&user, "bank-login");
    assert!(said.contains("xn--bnk-6cd.example"), "{said}");
}

#[test]
fn an_ask_with_userinfo_shows_the_host_after_the_at() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    // What stands before the "@" is a username: the secret goes to the host after it.
    let answer = mcp.call(
        "vault",
        json!({"action": "ask", "name": "bank-login", "why": "to sign in to the bank",
            "url": "https://bank.example@evil.example/login"}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(asks(&user)[0]["host"], "evil.example");
}

#[test]
fn an_ask_with_an_ipv6_url_shows_it_in_brackets() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    let answer = mcp.call(
        "vault",
        json!({"action": "ask", "name": "lab-login", "why": "to sign in to the lab",
            "url": "https://[2001:db8::1]:8443/login"}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(asks(&user)[0]["host"], "[2001:db8::1]");
}

#[test]
fn an_ask_for_a_url_login_would_refuse_is_refused() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    for (url, says) in [
        ("http://intranet.example.test/login", "in clear"),
        ("ftp://files.example.com/", "no web address"),
        ("https://*.example.com/login", "no web address"),
        ("not a url at all", "no web address"),
    ] {
        let refused = mcp.call(
            "vault",
            json!({"action": "ask", "name": "odd-login", "why": "to sign in", "url": url}),
        );
        assert!(refused.error, "{url}: {}", refused.text);
        assert!(refused.text.starts_with("refused:"), "{}", refused.text);
        assert!(refused.text.contains(says), "{url}: {}", refused.text);
    }
    assert!(asks(&user).is_empty());
    // This machine over plain http is where a secret may go.
    let local = mcp.call(
        "vault",
        json!({"action": "ask", "name": "local-login", "why": "to sign in",
            "url": "http://localhost:3000/login"}),
    );
    assert!(!local.error, "{}", local.text);
    assert_eq!(asks(&user)[0]["host"], "localhost");
}

#[test]
fn an_ask_bound_to_a_missing_connection_is_refused() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    connection(&user, "browser");
    let (mcp, _) = root_in(&user);

    let refused = mcp.call(
        "vault",
        json!({"action": "ask", "name": "shop-test", "why": "to sign in to the shop",
            "url": "https://shop.example.com/login", "for": "connection:nowhere"}),
    );
    assert!(refused.error, "{}", refused.text);
    assert!(
        refused.text.contains("no connection nowhere"),
        "{}",
        refused.text
    );
    assert!(asks(&user).is_empty());
    let asked = mcp.call(
        "vault",
        json!({"action": "ask", "name": "shop-test", "why": "to sign in to the shop",
            "url": "https://shop.example.com/login", "for": "connection:browser"}),
    );
    assert!(!asked.error, "{}", asked.text);
}

#[test]
fn a_connection_entry_on_plain_http_to_another_machine_is_refused_at_save() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    const TYPED: &str = "s3cret-of-the-intranet";

    let (_, refused) = user
        .call(
            "vault.set",
            json!({"name": "intranet-test", "secret": TYPED,
                "url": "http://intranet.example.test/login", "for": "connection:browser"}),
        )
        .unwrap_err();
    for says in ["intranet.example.test", "in clear", "https"] {
        assert!(refused.contains(says), "{says}: {refused}");
    }
    assert!(!refused.contains(TYPED), "{refused}");
    assert!(
        user.call("vault.list", json!({})).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Its https address is kept; replacing it with plain http, the binding kept, is refused too.
    user.call(
        "vault.set",
        json!({"name": "intranet-test", "secret": TYPED,
            "url": "https://intranet.example.test/login", "for": "connection:browser"}),
    )
    .unwrap();
    let (_, refused) = user
        .call(
            "vault.set",
            json!({"name": "intranet-test", "secret": TYPED,
                "url": "http://intranet.example.test/login"}),
        )
        .unwrap_err();
    assert!(refused.contains("in clear"), "{refused}");
    assert_eq!(
        entry_of(&user, "intranet-test")["url"],
        "https://intranet.example.test/login"
    );
    // This machine over plain http, and an entry bound to no connection, are saved.
    user.call(
        "vault.set",
        json!({"name": "local-test", "secret": TYPED, "url": "http://localhost:3000/login",
            "for": "connection:browser"}),
    )
    .unwrap();
    user.call(
        "vault.set",
        json!({"name": "plain-test", "secret": TYPED,
            "url": "http://intranet.example.test/login"}),
    )
    .unwrap();
}

#[test]
fn a_connection_entry_whose_url_login_cannot_read_is_refused_at_save() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    const TYPED: &str = "s3cret-of-the-bank";

    // The same words as the ask's refusal, before anything is stored.
    for url in [
        "bank.example",
        "https://*.example.com/login",
        "ftp://files.example.com/",
    ] {
        let (_, refused) = user
            .call(
                "vault.set",
                json!({"name": "bank-test", "secret": TYPED, "url": url,
                    "for": "connection:browser"}),
            )
            .unwrap_err();
        let says =
            format!("{url} is no web address a secret may go to: https, or http on this machine");
        assert!(refused.contains(&says), "{url}: {refused}");
        assert!(!refused.contains(TYPED), "{refused}");
    }
    assert!(
        user.call("vault.list", json!({})).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // A URL edited to something login cannot read does not replace the one it could.
    user.call(
        "vault.set",
        json!({"name": "bank-test", "secret": TYPED, "url": "https://bank.example/login",
            "for": "connection:browser"}),
    )
    .unwrap();
    let (_, refused) = user
        .call(
            "vault.set",
            json!({"name": "bank-test", "secret": TYPED, "url": "bank.example"}),
        )
        .unwrap_err();
    assert!(refused.contains("no web address"), "{refused}");
    assert_eq!(
        entry_of(&user, "bank-test")["url"],
        "https://bank.example/login"
    );
    // An entry bound to no connection keeps any URL: no login reads it.
    user.call(
        "vault.set",
        json!({"name": "note-test", "secret": TYPED, "url": "bank.example"}),
    )
    .unwrap();
}

#[test]
fn an_ask_of_another_project_does_not_join() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    other_project(&user);
    // An entry every project sees: the agents of both have the tool.
    user.call(
        "vault.set",
        json!({"name": "deploy-key", "secret": "d3ploy-s3cret-key"}),
    )
    .unwrap();
    let (app, _) = root_in(&user);
    let (other, theirs) = root_of(&user, "other");

    // A request of the project app alone: another project's agent is told to choose another
    // name, and learns nothing of it.
    assert!(!ask(&app, "staging-login", "to sign in to staging").error);
    let refused = ask(&other, "staging-login", "to sign in to staging too");
    assert!(refused.error, "{}", refused.text);
    assert!(refused.text.contains("another name"), "{}", refused.text);
    assert!(!refused.text.contains("app"), "{}", refused.text);
    let open = asks(&user);
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0]["joined"], json!([]), "{open:?}");

    // A request of every project's is joined by another project's agent...
    let every = app.call(
        "vault",
        json!({"action": "ask", "name": "shared-login", "why": "to sign in",
            "project": "every"}),
    );
    assert!(!every.error, "{}", every.text);
    let joined = ask(&other, "shared-login", "to sign in too");
    assert!(!joined.error, "{}", joined.text);
    assert!(joined.text.contains("joins"), "{}", joined.text);
    let shared = asks(&user)
        .into_iter()
        .find(|ask| ask["name"] == "shared-login")
        .unwrap();
    let number = shared["ask"].as_u64().unwrap();
    let asker = shared["agent"].as_str().unwrap().to_owned();

    // ...but once the user keeps the entry to the project app, only the agents that see it are
    // told it is set.
    user.call(
        "vault.set",
        json!({"name": "shared-login", "secret": SECRET, "project": "app"}),
    )
    .unwrap();
    let body = told(&user, number, &asker);
    assert!(body.contains("the user set the vault entry"), "{body}");
    let theirs = format!("vault-ask-{number}-{theirs}");
    assert!(
        store(&user).delivery(&theirs).unwrap().is_none(),
        "the other project's agent is told of an entry it cannot see"
    );
}

#[test]
fn a_worklog_entry_asked_is_refused_at_save_without_its_username() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);
    assert!(!ask(&mcp, "jira-time", "to log time").error);

    let (_, refused) = user
        .call(
            "vault.set",
            json!({"name": "jira-time", "secret": SECRET, "project": "app",
                "for": "worklog:app"}),
        )
        .unwrap_err();
    assert!(refused.contains("username"), "{refused}");
    assert!(!refused.contains(SECRET), "{refused}");
    assert_eq!(asks(&user).len(), 1, "still open");
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": ACCOUNT, "project": "app",
            "for": "worklog:app"}),
    )
    .unwrap();
    assert!(asks(&user).is_empty());
}

#[test]
fn the_inbox_entry_says_the_host_and_every_project() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    let asked = mcp.call(
        "vault",
        json!({"action": "ask", "name": "shared-login", "why": "to sign in to staging",
            "url": "https://staging.example.com:8443/login", "project": "every"}),
    );
    assert!(!asked.error, "{}", asked.text);
    assert!(!ask(&mcp, "app-login", "to sign in to the app").error);

    let every = inbox_text(&user, "shared-login");
    for says in [
        "staging.example.com",
        "every project",
        "to sign in to staging",
    ] {
        assert!(every.contains(says), "{says}: {every}");
    }
    assert!(!every.contains("8443"), "{every}");
    let own = inbox_text(&user, "app-login");
    assert!(own.contains("the project app"), "{own}");
    assert!(!own.contains("every project"), "{own}");
}

#[test]
fn a_vault_ask_for_a_connection_is_every_project_s() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    let (mcp, _) = root_in(&user);
    let proposed = mcp.call(
        "connections",
        json!({"action": "add", "name": "hippocampe", "about": "The user's memory.",
            "url": "http://127.0.0.1:7468/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "hippocampe-key"}]}),
    );
    assert!(
        proposed.text.starts_with("Waiting for the user's approval"),
        "{}",
        proposed.text
    );

    // Named by the request waiting for it: the ask is that connection's, every project's, and
    // the entry's address is the connection's.
    let asked = ask(&mcp, "hippocampe-key", "the key of the memory server");
    assert!(!asked.error, "{}", asked.text);
    assert!(asked.text.contains("every project"), "{}", asked.text);
    let open = asks(&user);
    assert_eq!(open[0]["connection"], "hippocampe", "{open:?}");
    assert_eq!(open[0]["every"], true, "{open:?}");
    assert_eq!(open[0]["url"], "http://127.0.0.1:7468", "{open:?}");
    assert_eq!(open[0]["host"], "127.0.0.1", "{open:?}");
    assert!(
        inbox_text(&user, "hippocampe-key").contains("every project"),
        "{}",
        inbox_text(&user, "hippocampe-key")
    );

    // Named by the argument too, for an entry the connection gives its server: never this
    // project's alone, never a connection nobody asked.
    let proposed = mcp.call(
        "connections",
        json!({"action": "add", "name": "recall", "about": "Another memory.",
            "url": "https://recall.example.com/mcp",
            "secrets": [{"in": "query", "name": "key", "entry": "recall-key"}]}),
    );
    assert!(
        proposed.text.starts_with("Waiting for the user's approval"),
        "{}",
        proposed.text
    );
    let refused = mcp.call(
        "vault",
        json!({"action": "ask", "name": "recall-key", "why": "another key",
            "connection": "recall", "project": "this"}),
    );
    assert!(refused.error, "{}", refused.text);
    assert!(refused.text.contains("every project"), "{}", refused.text);
    let unknown = mcp.call(
        "vault",
        json!({"action": "ask", "name": "other-key", "why": "another key",
            "connection": "nowhere"}),
    );
    assert!(unknown.error, "{}", unknown.text);
    assert!(unknown.text.contains("nowhere"), "{}", unknown.text);
    // An entry the connection does not give its server (a login's) is not made every
    // project's through it.
    let unnamed = mcp.call(
        "vault",
        json!({"action": "ask", "name": "recall-login", "why": "a login",
            "connection": "recall"}),
    );
    assert!(unnamed.error, "{}", unnamed.text);
    assert!(
        unnamed
            .text
            .contains("gives its server no vault entry recall-login"),
        "{}",
        unnamed.text
    );
    let named = mcp.call(
        "vault",
        json!({"action": "ask", "name": "recall-key", "why": "another key",
            "connection": "recall"}),
    );
    assert!(!named.error, "{}", named.text);
    let second = asks(&user)
        .into_iter()
        .find(|ask| ask["name"] == "recall-key")
        .unwrap();
    assert_eq!(second["every"], true, "{second}");
    assert_eq!(second["connection"], "recall", "{second}");
    assert_eq!(second["url"], "https://recall.example.com", "{second}");
    assert!(
        !asks(&user).iter().any(|ask| ask["name"] == "recall-login"),
        "nothing asked for the login"
    );
}

#[test]
fn a_vault_ask_is_never_tied_to_another_agents_pending_connection() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    other_project(&user);
    user.call(
        "extension.set",
        json!({"name": "connections", "enabled": true}),
    )
    .unwrap();
    let (app, _) = root_in(&user);
    let (other, _) = root_of(&user, "other");
    // Another project's agent proposes a connection of its own naming the entry.
    let proposed = other.call(
        "connections",
        json!({"action": "add", "name": "evil", "about": "Somewhere else.",
            "url": "https://evil.example/mcp",
            "secrets": [{"in": "header", "name": "Authorization", "prefix": "Bearer ",
                "entry": "jira-token"}]}),
    );
    assert!(
        proposed.text.starts_with("Waiting for the user's approval"),
        "{}",
        proposed.text
    );

    // The ask of app's agent is its own: not every project's, no url filled, no connection.
    let asked = ask(&app, "jira-token", "to log time");
    assert!(!asked.error, "{}", asked.text);
    assert!(!asked.text.contains("evil"), "{}", asked.text);
    assert!(!asked.text.contains("every project"), "{}", asked.text);
    let open = asks(&user);
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0]["connection"], Value::Null, "{open:?}");
    assert_eq!(open[0]["every"], false, "{open:?}");
    assert_eq!(open[0]["url"], Value::Null, "{open:?}");
    let card = inbox_text(&user, "jira-token");
    assert!(!card.contains("evil"), "{card}");
    assert!(!card.contains("every project"), "{card}");

    // Nor may it name that connection itself: it is no request of its own.
    let named = app.call(
        "vault",
        json!({"action": "ask", "name": "evil-key", "why": "a key", "connection": "evil"}),
    );
    assert!(named.error, "{}", named.text);
    assert!(
        !asks(&user).iter().any(|ask| ask["name"] == "evil-key"),
        "nothing asked through another agent's connection"
    );
}
