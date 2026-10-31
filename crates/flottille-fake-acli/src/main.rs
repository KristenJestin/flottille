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
//! - `received.jsonl`, written by the fake: one `{"args": [...], "env": {...}}` per call.
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
//! of that key end with 1, saying its text.
//!
//! `acli jira workitem search --jql Q --json [--limit N]` prints the script's `search` (an array
//! of work items, Jira's REST shape assumed as for `view`), or every item of `items` without it,
//! at most `--limit`. The fake does not read the JQL: the test reads it in the record. Without
//! `--jql` (or `--filter`) or `--json` it ends with 2, as a call Flottille never makes.
//!
//! The writes change nothing: the test reads them in the record.
//!
//! - `acli jira workitem create --project P --summary S … --json` prints `{"key": …}`, the
//!   script's `created` (`P-1` without it); without `--project` or `--summary` it ends with 2.
//! - `acli jira workitem comment create --key K --body B` and `acli jira workitem edit --key K …
//!   --yes` end with 0 for a key of `items`, with 1 for another.
//! - `acli jira workitem transition --key K --status S --yes` ends with 0 when S is one of the
//!   script's `transitions` of K (`{"ABC-1": ["In Progress", "Done"]}`), in any case, and with 1
//!   saying so otherwise.

use std::io::Write;
use std::path::PathBuf;

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
    std::process::exit(answer(&script, &args));
}

/// What `acli` prints for `args`, and its exit code.
fn answer(script: &Value, args: &[String]) -> i32 {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
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
            match script["items"].get(*key) {
                Some(item) => {
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
            let limit = rest
                .iter()
                .position(|word| *word == "--limit")
                .and_then(|at| rest.get(at + 1))
                .and_then(|n| n.parse().ok())
                .unwrap_or(usize::MAX);
            let mut found: Vec<Value> = match &script["search"] {
                Value::Array(items) => items.clone(),
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
            known(script, rest)
        }
        ["jira", "workitem", "edit", rest @ ..] if rest.contains(&"--yes") => known(script, rest),
        ["jira", "workitem", "transition", rest @ ..] if rest.contains(&"--yes") => {
            let (Some(key), Some(status)) = (value(rest, "--key"), value(rest, "--status")) else {
                return not_run(&words);
            };
            let possible = script["transitions"][key].as_array().into_iter().flatten();
            if possible
                .filter_map(Value::as_str)
                .any(|to| to.eq_ignore_ascii_case(status))
            {
                println!("✓ Work item {key} transitioned to {status}");
                0
            } else {
                eprintln!("✗ Error: no transition to status {status} for work item {key}");
                1
            }
        }
        _ => not_run(&words),
    }
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
