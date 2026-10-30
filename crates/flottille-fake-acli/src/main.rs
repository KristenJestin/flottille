//! A scripted `acli` (Atlassian's command line) for the tests of Flottille's Jira reads
//! (`docs/tickets.md`), so that no test ever calls Jira nor uses anyone's sign-in. It answers the
//! three commands Flottille runs, from a script, and records every call; any other command, a
//! write above all, ends with 2 and is in the record for a test to find.
//!
//! The shape of what it prints is **assumed**, from Atlassian's documentation of
//! `acli jira workitem view` (which gives its flags and not its output): the work item as Jira's
//! REST API gives it. The maintainer checks it against the real `acli` (`docs/tickets.md`).
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
        _ => {
            eprintln!(
                "fake acli: not a command Flottille runs: {}",
                words.join(" ")
            );
            2
        }
    }
}
