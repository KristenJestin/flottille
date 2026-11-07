//! An agent asks the user for a vault entry a feature of Flottille names and that is missing
//! (#566, `docs/vault.md`, "Asking the user for an entry"): the refusal that met the missing entry
//! says it can be asked for, `vault {action: "ask"}` answers at once, the user sees one request
//! prefilled by Flottille and types only the secret, and the agent is told what the user did,
//! never the secret. Against the real daemon with the fake ACP agent and the in-memory keyring the
//! tests' configuration chooses; no real agent, keyring or site is reached.

mod common;

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
    let opened = user.received_requests("session/new").len();
    let run = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        Mcp::initialize(&url, &authorization),
        run["agent"]["id"].as_str().unwrap().to_owned(),
    )
}

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
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
    assert!(
        request["description"]
            .as_str()
            .is_some_and(|d| d.contains("Jira")),
        "prefilled by Flottille: {request}"
    );
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

    let number = request["ask"].as_u64().unwrap();
    user.call(
        "vault.ask_set",
        json!({"ask": number, "secret": SECRET, "username": ACCOUNT}),
    )
    .unwrap();

    let entries = user.call("vault.list", json!({})).unwrap();
    let set = &entries["entries"][0];
    assert_eq!(set["name"], "jira-time");
    assert_eq!(set["project"], "app");
    assert_eq!(set["username"], ACCOUNT);
    assert!(asks(&user).is_empty(), "answered, not open");

    let body = told(&user, number, &agent);
    assert!(
        body.contains("the user set the vault entry `jira-time`"),
        "{body}"
    );
    // Nothing the agent receives, or the daemon keeps (its store, its logs), holds the secret.
    assert!(!body.contains(SECRET), "{body}");
    let mut folders = vec![user.path("state"), user.path("runtime")];
    while let Some(folder) = folders.pop() {
        for item in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = item.path();
            if path.is_dir() {
                folders.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let found = bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes());
                assert!(!found, "{} holds the secret", path.display());
            }
        }
    }
}

#[test]
fn an_ask_for_an_entry_nothing_needs_is_refused() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    let refused = ask(&mcp, "aws-root", "to deploy");
    assert!(refused.error, "{}", refused.text);
    assert!(refused.text.starts_with("refused:"), "{}", refused.text);
    assert!(refused.text.contains("aws-root"), "{}", refused.text);
    assert!(asks(&user).is_empty());

    // An entry the project names but the vault holds is no need either.
    user.call(
        "vault.set",
        json!({"name": "jira-time", "secret": SECRET, "username": ACCOUNT, "project": "app"}),
    )
    .unwrap();
    let refused = ask(&mcp, "jira-time", "to log time");
    assert!(refused.error, "{}", refused.text);
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
    assert_eq!(fields, ["action", "name", "why"], "{tool}");

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
fn a_second_ask_joins_the_open_request() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (first, one) = root_in(&user);
    let (second, two) = root_in(&user);

    assert!(!ask(&first, "jira-time", "to log time on ABC-1").error);
    let joined = ask(&second, "jira-time", "to log time on ABC-2");
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
fn a_dismissed_entry_is_not_offered_again_until_its_setting_changes() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user, "jira-time");
    let (mcp, _) = root_in(&user);

    assert!(!ask(&mcp, "jira-time", "to log time").error);
    let number = asks(&user)[0]["ask"].as_u64().unwrap();
    user.call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();

    // Not offered by the refusal, and refused when asked.
    let said = mcp.call(
        "tickets",
        json!({"action": "log_time", "ticket": "ABC-1", "duration": "1h"}),
    );
    assert!(!said.text.contains("action: \"ask\""), "{}", said.text);
    let refused = ask(&mcp, "jira-time", "to log time, again");
    assert!(refused.error, "{}", refused.text);
    assert!(refused.text.contains("dismissed"), "{}", refused.text);
    assert!(asks(&user).is_empty());

    // The user changes the setting that names it: it may be asked for again.
    tracker(&user, "jira-worklog");
    tracker(&user, "jira-time");
    let again = ask(&mcp, "jira-time", "to log time, the setting changed");
    assert!(!again.error, "{}", again.text);
    assert_eq!(asks(&user).len(), 1);
}
