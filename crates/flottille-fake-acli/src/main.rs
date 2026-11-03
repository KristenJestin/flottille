//! A scripted `acli` (Atlassian's command line) for the tests of Flottille's Jira reads and writes
//! (`docs/tickets.md`), so that no test ever calls Jira nor uses anyone's sign-in. It answers the
//! commands Flottille runs, from a script, and records every call; any other command ends with 2
//! and is in the record for a test to find.
//!
//! The shape of what it prints is **assumed**, from Atlassian's documentation of
//! `acli jira workitem view` (which gives its flags and not its output): the work item as Jira's
//! REST API gives it. The maintainer checks it against the real `acli` (`docs/tickets.md`).
//! Its `self` link is the test's to write: on the site's own host (an API-token sign-in) or on an
//! Atlassian host that names no site (an OAuth sign-in, as the real `acli` answers then).
//!
//! # Its folder
//!
//! Everything goes through one folder, named by `FLOTTILLE_FAKE_ACLI_DIR`:
//!
//! - `script.json`, written by the test (optional: without it, signed in with no work item); read
//!   again at every call;
//! - `received.jsonl`, written by the fake: one `{"args": [...], "env": {...}}` per call;
//! - `moved.json`, written by the fake: the status each moved work item is in;
//! - `posted.json`, written by the fake: the comments `comment create` added to each work item.
//!
//! # The script
//!
//! ```json
//! {"signed_in": true,
//!  "items": {"ABC-123": {"key": "ABC-123", "fields": {"summary": "…"}}},
//!  "fail": {"ABC-9": "Error: the work item could not be fetched"}}
//! ```
//!
//! `signed_in: false` makes `acli jira auth status` end with 1. `items` gives, by key, what
//! `acli jira workitem view KEY --json` prints; a key with none ends with 1. `fail` makes the view
//! of that key end with 1, saying its text. With `--fields a,b`, the view prints the work item's
//! `key`, its `self` and only those of its `fields`, as the real one is assumed to: a read that
//! forgot to ask for a field does not find it. Its `fields.comment.comments` are the script's,
//! then those posted (below). A script's `user` (`Casey Example` by default) is whom they are by.
//!
//! `acli jira workitem search --jql Q --json [--limit N]` prints the script's `search` (an array
//! of work items, Jira's REST shape assumed as for `view`), or every item of `items` without it,
//! at most `--limit`. A `--fields` with a field the real one refuses (`updated`, `created`, …) ends
//! with 1, saying `✗ Error: fields '<those>' are not allowed`. The fake does not read the JQL: the test reads it in the record. Without
//! `--jql` (or `--filter`) or `--json` it ends with 2, as a call Flottille never makes.
//!
//! A JQL that names `currentUser()` is answered with the script's `mine_search`, none without:
//! the work items of the signed-in user.
//!
//! The writes change nothing but the status a transition moves and the comments a `comment
//! create` adds: the test reads them in the record.
//!
//! - `acli jira workitem create --project P --summary S … --json` prints `{"key": …}`, the
//!   script's `created` (`P-1` without it); without `--project` or `--summary` it ends with 2.
//! - `acli jira workitem comment create --key K --body B` and `acli jira workitem edit --key K …
//!   --yes` end with 0 for a key of `items`, with 1 for another. A comment created is added, as
//!   the script's `user` wrote it, to the next views of the work item.
//! - `acli jira workitem transition --key K --status S --yes` ends with 0 when S is one of the
//!   script's `transitions` of K (`{"ABC-1": ["In Progress", "Done"]}`), in any case, and with 1
//!   saying so otherwise. These are the work item's transitions as Jira would answer them: the
//!   fake is the only place they are known, since the real `acli` lists none.
//!
//! A transition that ends with 0 moves the work item: a later `view` shows the status as the
//! script spells it in `transitions` (kept in `moved.json`). Three more script keys, each a list
//! of work item keys (`die` a map of key to `"before"` or `"after"`), play what a real `acli` does:
//! `no_move` ends with 0 without moving, as a bulk command ignoring a refused transition may;
//! `die` ends the process by a signal, before or after the move; `unreadable_after_move` makes
//! a `view` end with 1 once the work item has moved.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(
        std::env::var_os("FLOTTILLE_FAKE_ACLI_DIR").expect("FLOTTILLE_FAKE_ACLI_DIR"),
    );
    // The variables an agent's environment empties or moves: what the fake was given of them.
    let env: Map<String, Value> = ["ATLASSIAN_API_TOKEN", "JIRA_API_TOKEN", "HOME"]
        .iter()
        .filter_map(|name| Some((name.to_string(), Value::from(std::env::var(name).ok()?))))
        .collect();
    let mut record = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("received.jsonl"))
        .expect("the record");
    writeln!(record, "{}", json!({"args": args, "env": env})).expect("the record");
    let script: Value = std::fs::read_to_string(dir.join("script.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}));
    std::process::exit(answer(&dir, &script, &args));
}

/// What `acli` prints for `args`, and its exit code.
fn answer(dir: &Path, script: &Value, args: &[String]) -> i32 {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["--version"] => {
            println!("acli version 1.3.2-stable");
            0
        }
        ["jira", "auth", "status", ..] => {
            if script["signed_in"] == false {
                eprintln!("✗ Not logged in. Run: acli jira auth login");
                return 1;
            }
            println!("✓ Authenticated\n  Site: example.atlassian.net");
            0
        }
        ["jira", "workitem", "view", key, rest @ ..] if rest.contains(&"--json") => {
            if let Some(failure) = script["fail"][*key].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            let moved = moved(dir);
            if moved.get(*key).is_some() && listed(&script["unreadable_after_move"], key) {
                eprintln!("Error: work item {key} could not be fetched");
                return 1;
            }
            match script["items"].get(*key) {
                Some(item) => {
                    let mut item = item.clone();
                    if let Some(status) = moved.get(*key) {
                        item["fields"]["status"]["name"] = status.clone();
                    }
                    add_posted(dir, script, key, &mut item);
                    only_asked(rest, &mut item);
                    println!("{item}");
                    0
                }
                None => {
                    eprintln!(
                        "Error: work item {key} does not exist or you do not have permission to see it"
                    );
                    1
                }
            }
        }
        ["jira", "workitem", "search", rest @ ..]
            if rest.contains(&"--json") && rest.contains(&"--jql") =>
        {
            if let Some(failure) = script["fail"]["search"].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            if let Some(refused) = not_allowed(rest) {
                eprintln!("✗ Error: fields '{refused}' are not allowed");
                return 1;
            }
            let limit = rest
                .iter()
                .position(|word| *word == "--limit")
                .and_then(|at| rest.get(at + 1))
                .and_then(|n| n.parse().ok())
                .unwrap_or(usize::MAX);
            // The user's own: what the script says is theirs, none without.
            let theirs = rest.iter().any(|word| word.contains("currentUser()"));
            let mut found: Vec<Value> = match &script[if theirs { "mine_search" } else { "search" }]
            {
                Value::Array(items) => items.clone(),
                _ if theirs => Vec::new(),
                _ => script["items"]
                    .as_object()
                    .into_iter()
                    .flat_map(|items| items.values().cloned())
                    .collect(),
            };
            found.truncate(limit);
            println!("{}", Value::from(found));
            0
        }
        ["jira", "workitem", "create", rest @ ..]
            if rest.contains(&"--json") && rest.contains(&"--summary") =>
        {
            let Some(project) = value(rest, "--project") else {
                return not_run(&words);
            };
            let key = script["created"]
                .as_str()
                .map_or_else(|| format!("{project}-1"), str::to_owned);
            println!("{}", json!({"key": key}));
            0
        }
        ["jira", "workitem", "comment", "create", rest @ ..] if rest.contains(&"--body") => {
            let done = known(script, rest);
            if let (0, Some(key), Some(body)) = (done, value(rest, "--key"), value(rest, "--body"))
            {
                post(dir, key, body);
            }
            done
        }
        ["jira", "workitem", "edit", rest @ ..] if rest.contains(&"--yes") => known(script, rest),
        ["jira", "workitem", "transition", rest @ ..] if rest.contains(&"--yes") => {
            let (Some(key), Some(status)) = (value(rest, "--key"), value(rest, "--status")) else {
                return not_run(&words);
            };
            let Some(to) = script["transitions"][key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .find(|to| to.eq_ignore_ascii_case(status))
            else {
                eprintln!("✗ Error: no transition to status {status} for work item {key}");
                return 1;
            };
            let die = script["die"][key].as_str();
            if die == Some("before") {
                std::process::abort();
            }
            // Like the real bulk command, it may end with 0 on a transition it did not take.
            if !listed(&script["no_move"], key) {
                let mut moved = moved(dir);
                moved.insert(key.to_owned(), Value::from(to));
                std::fs::write(dir.join("moved.json"), Value::from(moved).to_string())
                    .expect("the moves");
            }
            if die == Some("after") {
                std::process::abort();
            }
            println!("✓ Work item {key} transitioned to {to}");
            0
        }
        _ => not_run(&words),
    }
}

/// Where the transitions that took left the work items: `moved.json`, written by the fake.
fn moved(dir: &Path) -> Map<String, Value> {
    std::fs::read_to_string(dir.join("moved.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The comments `comment create` added, by work item: `posted.json`, written by the fake.
fn posted(dir: &Path) -> Map<String, Value> {
    std::fs::read_to_string(dir.join("posted.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Keeps a comment created on `key`, its text as it was passed.
fn post(dir: &Path, key: &str, body: &str) {
    let mut all = posted(dir);
    let mut comments = all
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    comments.push(Value::from(body));
    all.insert(key.to_owned(), Value::from(comments));
    std::fs::write(dir.join("posted.json"), Value::from(all).to_string()).expect("the comments");
}

/// Adds the comments created on `key` to the work item's, by the script's `user`.
fn add_posted(dir: &Path, script: &Value, key: &str, item: &mut Value) {
    let Some(created) = posted(dir).get(key).and_then(Value::as_array).cloned() else {
        return;
    };
    let user = script["user"].as_str().unwrap_or("Casey Example");
    let comments = &mut item["fields"]["comment"]["comments"];
    if !comments.is_array() {
        *comments = json!([]);
    }
    for body in created {
        comments.as_array_mut().expect("an array").push(
            json!({"author": {"displayName": user}, "created": "2026-10-01T12:00:00.000+0000",
                "body": body}),
        );
    }
}

/// With `--fields a,b` among `args`, only those of the work item's fields stay.
fn only_asked(args: &[&str], item: &mut Value) {
    let Some(asked) = value(args, "--fields") else {
        return;
    };
    let asked: Vec<&str> = asked.split(',').map(str::trim).collect();
    if let Some(fields) = item["fields"].as_object_mut() {
        fields.retain(|name, _| asked.contains(&name.as_str()));
    }
}

/// What the search's `--fields` holds that the real `acli` refuses, as it says it, in the order
/// asked. The documentation lists no allowed field; these are those a real site was seen to take
/// (`docs/tickets.md`), and `updated` and `created` are seen refused. `view` takes any.
fn not_allowed(args: &[&str]) -> Option<String> {
    const ALLOWED: [&str; 8] = [
        "key",
        "summary",
        "status",
        "assignee",
        "reporter",
        "labels",
        "issuetype",
        "priority",
    ];
    let refused: Vec<&str> = value(args, "--fields")?
        .split(',')
        .map(str::trim)
        .filter(|field| !ALLOWED.contains(field))
        .collect();
    (!refused.is_empty()).then(|| refused.join(", "))
}

/// Whether the script's `list` (an array of keys) holds `key`.
fn listed(list: &Value, key: &str) -> bool {
    list.as_array()
        .is_some_and(|keys| keys.iter().any(|k| k == key))
}

/// What follows `flag` in `args`.
fn value<'a>(args: &[&'a str], flag: &str) -> Option<&'a str> {
    let at = args.iter().position(|word| *word == flag)?;
    args.get(at + 1).copied()
}

/// A write to the work item of `--key`: done when the script holds it.
fn known(script: &Value, args: &[&str]) -> i32 {
    match value(args, "--key") {
        Some(key) if script["items"].get(key).is_some() => {
            println!("✓ Work item {key} updated");
            0
        }
        Some(key) => {
            eprintln!("✗ Error: work item {key} does not exist");
            1
        }
        None => not_run(args),
    }
}

fn not_run(words: &[&str]) -> i32 {
    eprintln!(
        "fake acli: not a command Flottille runs: {}",
        words.join(" ")
    );
    2
}
