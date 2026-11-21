//! The scenarios of the findings (`docs/developer.md`, "The findings"): while the developer mode
//! is on an agent files what Flottille itself got wrong, searches the open findings first, adds an
//! occurrence to the one that already describes its bug, and the maintainer exports them for the
//! lead agent. The agents are fake ACP agents, in throwaway repositories and temporary XDG folders.

use crate::common;

use std::path::PathBuf;

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// A token of the kind `secrets` recognises anywhere. Invented.
const TOKEN: &str = "ghp_fakefakefakefakefakefakefakefake0002";

/// A project `app` over a throwaway repository, and the session of a root agent working in it:
/// the agent's id and its MCP session.
fn session(user: &User) -> (String, Mcp) {
    let folder: PathBuf = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization))
}

fn developer(user: &User, args: &[&str]) -> String {
    let mut all = vec!["developer"];
    all.extend(args);
    let output = user.run(&all);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The findings, as `flottille developer reports --json` says them.
fn findings(user: &User) -> Vec<Value> {
    let said: Value =
        serde_json::from_str(&developer(user, &["reports", "--json"])).expect("one JSON document");
    said["findings"].as_array().expect("findings").clone()
}

/// What `services report` is given for a new finding.
fn bug(title: &str) -> Value {
    json!({"action": "report", "title": title,
        "called": "tickets {\"action\": \"list\"}",
        "expected": "the open tickets",
        "happened": "an empty answer with no error"})
}

fn report(mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("services", args);
    assert!(!answer.error, "{}", answer.text);
    answer.text
}

/// The `action` values `services` offers its caller.
fn actions(mcp: &Mcp) -> Vec<String> {
    let tools = mcp.tools();
    let services = tools
        .iter()
        .find(|tool| tool["name"] == "services")
        .expect("services is listed");
    services["inputSchema"]["properties"]["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_report_action_is_offered_only_in_developer_mode() {
    let user = User::with_fake_agent();
    let (_, mcp) = session(&user);

    assert!(!actions(&mcp).contains(&"report".to_owned()));
    let refused = mcp.call("services", bug("Tickets list answers empty"));
    assert!(refused.error, "{}", refused.text);
    assert!(refused.text.contains("developer mode"), "{}", refused.text);

    developer(&user, &["on"]);

    // A session that lists its tools again is given them (a client keeps the list it holds
    // until it is told or starts over).
    let (url, authorization) = server_of(&user, 0);
    let mcp = Mcp::initialize(&url, &authorization);
    let on = actions(&mcp);
    assert!(on.contains(&"report".to_owned()), "{on:?}");
    assert!(on.contains(&"find_reports".to_owned()), "{on:?}");
    // The guide that tells an agent when to report is in the description, not only in the schema.
    let tools = mcp.tools();
    let description = tools.iter().find(|t| t["name"] == "services").unwrap()["description"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(description.contains("Flottille itself"), "{description}");
}

#[test]
fn an_agent_reports_a_finding_and_it_is_listed() {
    let user = User::with_fake_agent();
    developer(&user, &["on"]);
    let (agent, mcp) = session(&user);

    let answer = report(&mcp, bug("Tickets list answers empty"));
    assert!(answer.contains("#1"), "{answer}");

    let all = findings(&user);
    assert_eq!(all.len(), 1, "{all:?}");
    let finding = &all[0];
    assert_eq!(finding["title"], "Tickets list answers empty");
    assert_eq!(finding["expected"], "the open tickets");
    assert_eq!(finding["count"], 1);
    assert_eq!(finding["filed"], Value::Null);
    let first = &finding["occurrences"][0];
    assert_eq!(first["session"], agent.as_str());
    assert_eq!(first["called"], "tickets {\"action\": \"list\"}");
    assert_eq!(first["happened"], "an empty answer with no error");
    assert!(first["at"].is_u64(), "{first}");

    let text = developer(&user, &["reports"]);
    assert!(text.contains("Tickets list answers empty"), "{text}");
    assert!(text.contains("#1"), "{text}");
}

#[test]
fn the_agent_finds_an_open_finding_before_it_reports() {
    let user = User::with_fake_agent();
    developer(&user, &["on"]);
    let (_, mcp) = session(&user);
    report(&mcp, bug("Tickets list answers empty"));
    let mut other = bug("A refusal that makes no sense");
    other["happened"] = json!("fs_write refused a file inside the workspace");
    report(&mcp, other);

    let found = report(
        &mcp,
        json!({"action": "find_reports", "query": "refusal workspace"}),
    );

    assert!(found.contains("#2"), "{found}");
    assert!(found.contains("A refusal that makes no sense"), "{found}");
    assert!(!found.contains("Tickets list answers empty"), "{found}");
    let none = report(
        &mcp,
        json!({"action": "find_reports", "query": "kubernetes"}),
    );
    assert!(none.contains("No open finding"), "{none}");
}

#[test]
fn a_second_report_of_the_same_bug_adds_an_occurrence() {
    let user = User::with_fake_agent();
    developer(&user, &["on"]);
    let (agent, mcp) = session(&user);
    report(&mcp, bug("Tickets list answers empty"));

    let answer = report(
        &mcp,
        json!({"action": "report", "finding": 1,
            "called": "tickets {\"action\": \"list\", \"state\": \"open\"}",
            "happened": "empty again, after a retry",
            "differs": "it filtered on state this time"}),
    );
    assert!(answer.contains("#1"), "{answer}");

    let all = findings(&user);
    assert_eq!(all.len(), 1, "a second finding was made: {all:?}");
    let finding = &all[0];
    assert_eq!(finding["count"], 2);
    let occurrences = finding["occurrences"].as_array().unwrap();
    assert_eq!(occurrences.len(), 2);
    let second = &occurrences[1];
    assert_eq!(second["session"], agent.as_str());
    assert_eq!(second["happened"], "empty again, after a retry");
    assert_eq!(second["differs"], "it filtered on state this time");
    assert!(
        second["called"].as_str().unwrap().contains("\"state\""),
        "{second}"
    );
    assert!(finding["first_at"].as_u64().unwrap() <= finding["last_at"].as_u64().unwrap());
    assert_eq!(finding["first_at"], occurrences[0]["at"]);
    assert_eq!(finding["last_at"], second["at"]);
    let text = developer(&user, &["reports"]);
    assert!(text.contains("2 occurrences"), "{text}");
    let unknown = report_refused(
        &mcp,
        json!({"action": "report", "finding": 99,
        "called": "x", "happened": "y"}),
    );
    assert!(unknown.contains("no finding"), "{unknown}");
}

fn report_refused(mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("services", args);
    assert!(answer.error, "{}", answer.text);
    answer.text
}

#[test]
fn a_secret_in_a_finding_is_masked_before_it_is_stored() {
    let user = User::with_fake_agent();
    developer(&user, &["on"]);
    let (_, mcp) = session(&user);
    let mut leaky = bug(&format!("Header {TOKEN} shown back"));
    leaky["happened"] = json!(format!("the answer carried {TOKEN}"));
    report(&mcp, leaky);
    report(
        &mcp,
        json!({"action": "report", "finding": 1, "called": format!("web read with {TOKEN}"),
            "happened": format!("again {TOKEN}"), "differs": format!("{TOKEN} in the url")}),
    );

    // The store's own bytes, its write-ahead log too: a secret stored anywhere would be in them.
    for file in ["flottille.db", "flottille.db-wal"] {
        let bytes = std::fs::read(user.path("state/flottille").join(file)).unwrap_or_default();
        let found = bytes
            .windows(TOKEN.len())
            .any(|window| window == TOKEN.as_bytes());
        assert!(!found, "{file} holds the secret");
    }
    let said = developer(&user, &["reports", "--json"]);
    assert!(!said.contains(TOKEN), "{said}");
    assert!(said.contains(flottille_core::secrets::MASK), "{said}");
}

#[test]
fn the_export_holds_the_findings_and_their_occurrences() {
    let user = User::with_fake_agent();
    developer(&user, &["on"]);
    let (_, mcp) = session(&user);
    report(&mcp, bug("Tickets list answers empty"));
    report(
        &mcp,
        json!({"action": "report", "finding": 1, "called": "tickets list again",
            "happened": "empty", "differs": "nothing"}),
    );

    let said = developer(&user, &["reports", "--export"]);

    let file = user.path("state/flottille/findings.json");
    assert!(said.contains(file.to_str().unwrap()), "{said}");
    let exported: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(exported["findings"], json!(findings(&user)));
    assert_eq!(
        exported["findings"][0]["occurrences"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // Every field of a finding and nothing else: no path of the machine, no credential.
    let keys: Vec<&str> = exported["findings"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for key in [
        "id",
        "title",
        "expected",
        "count",
        "first_at",
        "last_at",
        "filed",
        "occurrences",
    ] {
        assert!(keys.contains(&key), "{key} missing in {keys:?}");
    }
    assert!(
        !std::fs::read_to_string(&file)
            .unwrap()
            .contains(user.path("").to_str().unwrap())
    );
}

#[test]
fn a_finding_marked_filed_keeps_the_issue_address_and_leaves_the_open_findings() {
    let user = User::with_fake_agent();
    developer(&user, &["on"]);
    let (_, mcp) = session(&user);
    report(&mcp, bug("Tickets list answers empty"));

    developer(
        &user,
        &[
            "reports",
            "filed",
            "1",
            "https://example.com/acme/app/issues/12",
        ],
    );

    let all = findings(&user);
    assert_eq!(all[0]["filed"], "https://example.com/acme/app/issues/12");
    let found = report(&mcp, json!({"action": "find_reports", "query": "tickets"}));
    assert!(found.contains("No open finding"), "{found}");
    // A bug that comes back after its issue is a new finding: the agent files it again.
    let text = developer(&user, &["reports"]);
    assert!(
        text.contains("https://example.com/acme/app/issues/12"),
        "{text}"
    );
}
