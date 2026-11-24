//! An agent proposes an edit of a vault entry the user accepts (#1307, `docs/vault.md`, "Proposing
//! an edit of an entry"): the request is the one of an asked entry, with each field changed said
//! as before → after; Accept applies it through the user's own Save, the secret kept; Dismiss
//! changes nothing; no mode applies it alone. Against the real daemon with the fake ACP agent and
//! the in-memory keyring the tests' configuration chooses; no real agent, keyring or site is
//! reached.

use crate::common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

const SECRET: &str = "r3alm-l0gin-passw0rd";
const ROOT: &str = "https://id.example.invalid/realms/dev/";
const ACCOUNT: &str = "https://id.example.invalid/realms/dev/account";

/// A project `app` over a throwaway repository.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// The entry `example-dev-login`, saved by the user with the realm's root, which shows no form.
fn login_entry(user: &User, extra: Value) {
    let mut entry = json!({"name": "example-dev-login", "secret": SECRET,
        "description": "The test account of the dev realm", "url": ROOT,
        "username": "tester@example.invalid", "project": "app"});
    entry
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    user.call("vault.set", entry).unwrap();
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

fn entry_of(user: &User, name: &str) -> Value {
    user.call("vault.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("no entry {name}"))
        .clone()
}

/// What the agent is handed once the user answered the request `number`.
fn told(user: &User, number: u64, agent: &str) -> String {
    let id = format!("vault-ask-{number}-{agent}");
    eventually("the delivery", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

/// The memory keyring refuses every store from here on: a call that wrote or replaced the secret
/// would fail.
fn keyring_untouchable(user: &User) {
    let file = user.path("config/flottille/keyring.toml");
    let kept = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("fail_stores = 1000\n{kept}")).unwrap();
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

fn propose(mcp: &Mcp, fields: Value) -> common::mcp::Answer {
    let mut args = json!({"action": "propose", "name": "example-dev-login",
        "why": "the realm's root shows no login form; the form is under account"});
    args.as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    mcp.call("vault", args)
}

#[test]
fn an_agent_proposes_a_new_url_for_an_entry_and_the_user_accepts_it() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user);
    login_entry(&user, json!({}));
    let (mcp, agent) = root_in(&user);

    let answer = propose(&mcp, json!({"url": ACCOUNT}));
    assert!(!answer.error, "{}", answer.text);
    // At once, held for the user: nothing waits, nothing is applied.
    assert!(answer.text.contains("held for the user"), "{}", answer.text);
    assert_eq!(entry_of(&user, "example-dev-login")["url"], ROOT);

    let open = asks(&user);
    assert_eq!(open.len(), 1, "{open:?}");
    let request = &open[0];
    assert_eq!(request["name"], "example-dev-login");
    assert_eq!(request["agent"], agent.as_str());
    assert_eq!(request["project"], "app");
    assert!(request["why"].as_str().unwrap().contains("no login form"));
    // Each field changed, before → after; the host the secret goes to, as the daemon reads it.
    assert_eq!(
        request["edit"],
        json!({"url": {"before": ROOT, "after": ACCOUNT}}),
        "{request}"
    );
    assert_eq!(request["host"], "id.example.invalid", "{request}");
    // The inbox says it is a proposed edit, not a new entry.
    let inbox = user.call("inbox.list", json!({})).unwrap();
    let text = inbox["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "vault")
        .unwrap_or_else(|| panic!("no inbox entry: {inbox}"))["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.contains("proposes an edit of the vault entry example-dev-login"),
        "{text}"
    );

    // Accept applies it in one action: the keyring is neither written nor read.
    keyring_untouchable(&user);
    let number = request["ask"].as_u64().unwrap();
    user.call("vault.ask_accept", json!({"ask": number}))
        .unwrap();
    let entry = entry_of(&user, "example-dev-login");
    assert_eq!(entry["url"], ACCOUNT);
    // What the proposal did not name is kept.
    assert_eq!(entry["username"], "tester@example.invalid");
    assert_eq!(entry["description"], "The test account of the dev realm");
    assert_eq!(entry["project"], "app");
    assert!(asks(&user).is_empty(), "answered, not open");

    let body = told(&user, number, &agent);
    assert!(
        body.contains(
            "the user accepted your proposed edit of the vault entry `example-dev-login`"
        ),
        "{body}"
    );
    assert!(!body.contains(SECRET), "{body}");
    nothing_holds(&user, SECRET);
}

#[test]
fn a_proposal_never_changes_the_secret() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user);
    login_entry(&user, json!({}));
    let (mcp, _) = root_in(&user);

    // The schema has no field for a secret, and one given anyway is refused, never repeated.
    let tool = mcp
        .tools()
        .into_iter()
        .find(|tool| tool["name"] == "vault")
        .unwrap();
    assert!(
        tool["inputSchema"]["properties"].get("secret").is_none(),
        "{tool}"
    );
    const OTHER: &str = "an0ther-s3cret-proposed";
    let refused = propose(&mcp, json!({"secret": OTHER}));
    assert!(refused.error, "{}", refused.text);
    assert!(!refused.text.contains(OTHER), "{}", refused.text);
    assert!(asks(&user).is_empty());

    // A field that is not the entry's metadata an agent may change is refused too: its name is
    // the user's, and a proposal that changes nothing is no proposal.
    for fields in [json!({"rename": "other"}), json!({})] {
        let refused = propose(&mcp, fields.clone());
        assert!(refused.error, "{fields}: {}", refused.text);
    }
    assert!(asks(&user).is_empty());

    // An accepted proposal of every field the agent may change never reaches the keyring.
    let answer = propose(
        &mcp,
        json!({"url": ACCOUNT, "username": "qa@example.invalid",
            "description": "The QA account of the dev realm", "project": "every"}),
    );
    assert!(!answer.error, "{}", answer.text);
    keyring_untouchable(&user);
    let number = asks(&user)[0]["ask"].as_u64().unwrap();
    user.call("vault.ask_accept", json!({"ask": number}))
        .unwrap();
    let entry = entry_of(&user, "example-dev-login");
    assert_eq!(entry["username"], "qa@example.invalid");
    assert!(entry["project"].is_null(), "every project's: {entry}");
    nothing_holds(&user, SECRET);
    // The text an agent wrote is its own, recorded with its call as any argument is; what matters
    // is that the vault took nothing of it.
    let listed = user.call("vault.list", json!({})).unwrap().to_string();
    assert!(!listed.contains(OTHER), "{listed}");
}

#[test]
fn a_dismissed_proposal_changes_nothing() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user);
    login_entry(&user, json!({}));
    let (mcp, agent) = root_in(&user);
    let before = entry_of(&user, "example-dev-login");

    let answer = propose(
        &mcp,
        json!({"url": ACCOUNT, "username": "qa@example.invalid"}),
    );
    assert!(!answer.error, "{}", answer.text);
    let number = asks(&user)[0]["ask"].as_u64().unwrap();
    user.call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();

    assert_eq!(entry_of(&user, "example-dev-login"), before);
    assert!(asks(&user).is_empty());
    // Dismissed, it is accepted no more.
    assert!(
        user.call("vault.ask_accept", json!({"ask": number}))
            .is_err()
    );
    assert_eq!(entry_of(&user, "example-dev-login"), before);
    let body = told(&user, number, &agent);
    assert!(
        body.contains(
            "the user dismissed your proposed edit of the vault entry `example-dev-login`"
        ),
        "{body}"
    );
}

#[test]
fn a_proposal_in_full_mode_still_waits_for_the_user() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user);
    login_entry(&user, json!({}));
    let (mcp, agent) = root_in(&user);
    user.call("agent.mode", json!({"agent": agent, "mode": "full"}))
        .unwrap();

    let answer = propose(&mcp, json!({"url": ACCOUNT}));
    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("held for the user"), "{}", answer.text);
    // Nothing is applied alone, Full included: the request waits, the entry is as it was.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(entry_of(&user, "example-dev-login")["url"], ROOT);
    assert_eq!(asks(&user).len(), 1);
    // A second proposal for the entry while one waits is refused: one proposal a name.
    let again = propose(&mcp, json!({"username": "qa@example.invalid"}));
    assert!(again.error, "{}", again.text);
    assert_eq!(asks(&user).len(), 1);
}

#[test]
fn a_proposal_that_moves_the_host_is_checked_as_a_hand_edit() {
    let user = User::with_fake_agent().with_fake_acli();
    project(&user);
    // A login's entry: the browser types its password at its URL.
    login_entry(&user, json!({"for": "connection:browser"}));
    let (mcp, _) = root_in(&user);

    // What the user's own Save refuses (plain http to another machine, for a login) is refused
    // the same before the user is asked: its password would travel in clear to that host. (The
    // tool's words name no use: an agent is never told what an entry is bound to.)
    let plain = "http://id.example.invalid/realms/dev/account";
    let (_, by_hand) = user
        .call(
            "vault.set",
            json!({"name": "example-dev-login", "url": plain}),
        )
        .unwrap_err();
    let refused = propose(&mcp, json!({"url": plain}));
    assert!(refused.error, "{}", refused.text);
    for said in [&by_hand, &refused.text] {
        for says in ["id.example.invalid", "in clear", "https"] {
            assert!(said.contains(says), "{says}: {said}");
        }
    }
    assert!(!refused.text.contains("browser"), "{}", refused.text);
    assert!(asks(&user).is_empty());

    // A new host the Save takes is said: the card names where the secret will go.
    let moved = "https://login.example.invalid/realms/dev/account";
    let answer = propose(&mcp, json!({"url": moved}));
    assert!(!answer.error, "{}", answer.text);
    let request = asks(&user)[0].clone();
    assert_eq!(request["host"], "login.example.invalid", "{request}");
    assert_eq!(request["for"], "connection:browser", "{request}");

    // Accept goes through the Save's checks again: the entry removed meanwhile is not made anew
    // from a proposal (it would have no secret).
    let number = request["ask"].as_u64().unwrap();
    user.call(
        "vault.remove",
        json!({"name": "example-dev-login", "confirmed": true}),
    )
    .unwrap();
    let (_, refused) = user
        .call("vault.ask_accept", json!({"ask": number}))
        .unwrap_err();
    assert!(refused.contains("needs its secret"), "{refused}");
    assert!(
        user.call("vault.list", json!({})).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
