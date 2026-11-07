//! The vault (`docs/vault.md`, #382): the user's secrets, kept by the keyring, that agents use
//! and never see. Against the real daemon with the fake ACP agent and the in-memory keyring the
//! tests' configuration chooses (`User::new`): the user's own keyring is never reached. No real
//! agent ever runs here.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// The secret set in the vault, which nothing may give back.
const SECRET: &str = "v4ult-s3cret-for-jira";

/// A project `name` over a throwaway repository with one commit.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The entry `jira`, of every project, with all its metadata.
fn set_jira(user: &User) -> Value {
    user.call(
        "vault.set",
        json!({"name": "jira", "secret": SECRET, "description": "Logs time on Jira",
            "url": "https://tracker.example.com", "username": "dev@example.com"}),
    )
    .unwrap()
}

/// Starts an agent of `project` with a first message and waits for its first turn's end: its
/// id.
fn agent(user: &User, project: &str, message: &str) -> String {
    let answer = user
        .call("agent.run", json!({"project": project, "message": message}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let after = answer["after"].as_u64().unwrap();
    let of = id.clone();
    user.watch(&["--after", &after.to_string()])
        .until(|e| e["data"]["agent"] == of.as_str() && e["name"] == "agent.turn_ended");
    id
}

/// Every file under `dir` holding `needle`, as a path.
fn files_holding(dir: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_holding(&path, needle));
        } else if let Ok(bytes) = std::fs::read(&path)
            && bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        {
            found.push(path.display().to_string());
        }
    }
    found
}

#[test]
fn vault_list_shows_metadata_only() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "start", "do": [{"say": "ready"}]}]}));
    project(&user, "app");
    let set = set_jira(&user);
    assert_eq!(set["name"], "jira");

    let listed = user.call("vault.list", json!({})).unwrap();
    assert_eq!(
        listed["entries"],
        json!([{"name": "jira", "description": "Logs time on Jira",
            "url": "https://tracker.example.com", "username": "dev@example.com",
            "project": null}]),
        "{listed}"
    );

    // An agent lists the same metadata with its tool, and nothing more.
    agent(&user, "app", "start");
    let (url, authorization) = server_of(&user, 0);
    let mcp = Mcp::initialize(&url, &authorization);
    assert!(mcp.names().iter().any(|name| name == "vault"), "listed");
    let answer = mcp.call("vault", json!({"action": "list"}));
    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("jira"), "{}", answer.text);
    assert!(answer.text.contains("Logs time on Jira"), "{}", answer.text);
    assert!(
        answer.text.contains("https://tracker.example.com"),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("dev@example.com"));
    assert!(!answer.text.contains(SECRET), "{}", answer.text);
}

#[test]
fn a_secret_is_never_returned_by_any_client() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "start", "do": [{"say": "ready"}]}]}));
    project(&user, "app");
    let set = set_jira(&user);
    // Replaced: the answer of a replacement says no more than the first.
    let replaced = user
        .call(
            "vault.set",
            json!({"name": "jira", "secret": format!("{SECRET}-2"), "description": "Logs time"}),
        )
        .unwrap();
    let mut said = vec![set.to_string(), replaced.to_string()];

    // The methods.
    said.push(user.call("vault.list", json!({})).unwrap().to_string());
    said.push(
        user.call("vault.list", json!({"project": "app"}))
            .unwrap()
            .to_string(),
    );
    // A refusal that names a field of the call: its message never repeats the secret.
    for refused in [
        json!({"name": "other", "secret": SECRET, "project": "nowhere"}),
        json!({"name": "bad name!", "secret": SECRET}),
        json!({"name": "other", "secret": SECRET, "url": 7}),
        json!({"name": "other", "secret": 12345678}),
        json!({"name": "other", "secret": "short"}),
    ] {
        let (_, message) = user.call("vault.set", refused).unwrap_err();
        said.push(message);
    }
    // The command line, in words and in JSON.
    for args in [&["vault", "list"][..], &["vault", "list", "--json"]] {
        let output = user.run(args);
        assert!(output.status.success(), "{output:?}");
        said.push(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    // The events, every one since the first.
    let events = user.watch(&["--after", "0"]).within(Duration::from_secs(1));
    assert!(
        events.iter().any(|e| e["name"] == "vault.changed"),
        "{events:?}"
    );
    said.push(serde_json::to_string(&events).unwrap());
    // An agent's tool, whatever action it asks.
    agent(&user, "app", "start");
    let (url, authorization) = server_of(&user, 0);
    let mcp = Mcp::initialize(&url, &authorization);
    for asked in [
        json!({"action": "list"}),
        json!({"action": "show", "name": "jira"}),
        json!({"action": "get", "name": "jira"}),
        json!({"action": "list", "secret": true}),
    ] {
        said.push(mcp.call("vault", asked).text);
    }

    for text in &said {
        assert!(!text.contains(SECRET), "a secret came back: {text}");
    }
    // Nor is it written anywhere of Flottille's: its store, its log, its files.
    eventually("the store is written", || {
        user.path("state/flottille").join("flottille.db").exists()
    });
    for folder in ["state", "config", "data", "runtime", "home"] {
        let holding = files_holding(&user.path(folder), SECRET);
        assert!(holding.is_empty(), "{holding:?}");
    }
}

#[test]
fn a_project_entry_is_not_seen_by_another_project() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "start", "do": [{"say": "ready"}]}]}));
    project(&user, "app");
    project(&user, "other");
    user.call(
        "vault.set",
        json!({"name": "deploy", "secret": SECRET, "project": "app",
            "description": "The app's deploy key"}),
    )
    .unwrap();
    user.call(
        "vault.set",
        json!({"name": "shared", "secret": "an0ther-s3cret", "description": "For all"}),
    )
    .unwrap();

    let names = |listed: Value| -> Vec<String> {
        listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        names(user.call("vault.list", json!({"project": "app"})).unwrap()),
        ["deploy", "shared"]
    );
    assert_eq!(
        names(
            user.call("vault.list", json!({"project": "other"}))
                .unwrap()
        ),
        ["shared"]
    );

    // An agent of `other` lists only what every project sees.
    agent(&user, "other", "start");
    let (url, authorization) = server_of(&user, 0);
    let mcp = Mcp::initialize(&url, &authorization);
    let answer = mcp.call("vault", json!({"action": "list"}));
    assert!(answer.text.contains("shared"), "{}", answer.text);
    assert!(!answer.text.contains("deploy"), "{}", answer.text);

    // And an agent of `app`, both.
    agent(&user, "app", "start");
    let (url, authorization) = server_of(&user, 1);
    let mcp = Mcp::initialize(&url, &authorization);
    let answer = mcp.call("vault", json!({"action": "list"}));
    assert!(answer.text.contains("shared"), "{}", answer.text);
    assert!(answer.text.contains("deploy"), "{}", answer.text);
}

#[test]
fn a_secret_value_is_masked_in_what_is_stored() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "tell", "do": [{"say": format!("the jira token is {SECRET}, keep it")}]}
    ]}));
    project(&user, "app");
    set_jira(&user);

    let id = agent(&user, "app", "tell");

    let page = user
        .call("agent.read", json!({"agent": id, "after": 0}))
        .unwrap();
    let text = page.to_string();
    assert!(!text.contains(SECRET), "{text}");
    assert!(
        text.contains("the jira token is [masked], keep it"),
        "{text}"
    );
    let events = user.watch(&["--after", "0"]).within(Duration::from_secs(1));
    let events = serde_json::to_string(&events).unwrap();
    assert!(!events.contains(SECRET), "{events}");
    assert!(files_holding(&user.path("state"), SECRET).is_empty());

    // A removed entry's value is no longer the vault's to mask.
    user.call("vault.remove", json!({"name": "jira"})).unwrap();
    assert_eq!(
        user.call("vault.list", json!({})).unwrap()["entries"],
        json!([])
    );
}

#[test]
fn vault_set_reads_stdin_not_an_argument() {
    let user = User::new();

    // A secret given as an argument would be in the shell's history and the process list.
    let refused = user.run(&["vault", "set", "jira", SECRET]);
    assert!(!refused.status.success(), "{refused:?}");
    assert_eq!(
        user.call("vault.list", json!({})).unwrap()["entries"],
        json!([])
    );

    let output = user.run_with_input(
        &["vault", "set", "jira", "--description", "Logs time on Jira"],
        &format!("{SECRET}\n"),
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("jira"), "{stdout}");
    assert!(!stdout.contains(SECRET), "{stdout}");
    let listed = user.call("vault.list", json!({})).unwrap();
    assert_eq!(listed["entries"][0]["name"], "jira");
    assert_eq!(listed["entries"][0]["description"], "Logs time on Jira");

    // Nothing on stdin is refused, not kept empty.
    let empty = user.run_with_input(&["vault", "set", "empty"], "");
    assert!(!empty.status.success(), "{empty:?}");

    let removed = user.run(&["vault", "remove", "jira", "--json"]);
    assert!(removed.status.success(), "{removed:?}");
    assert_eq!(
        user.call("vault.list", json!({})).unwrap()["entries"],
        json!([])
    );
}
