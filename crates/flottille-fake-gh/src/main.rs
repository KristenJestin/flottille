//! A scripted `gh` for the tests of Flottille's forge reads and writes (`docs/forges.md`,
//! `docs/procedures.md`), so that no test
//! ever calls GitHub nor uses anyone's sign-in. It answers the few commands Flottille runs, from a
//! script, and records every call.
//!
//! # Its folder
//!
//! Everything goes through one folder, named by `FLOTTILLE_FAKE_GH_DIR`:
//!
//! - `script.json`, written by the test (optional: without it, signed in with no pull request);
//!   read again at every call, so that a test changes what the next call answers;
//! - `received.jsonl`, written by the fake: one `{"args": [...], "env": {...}}` per call, `env`
//!   holding the forge variables it was given (`GH_TOKEN`, `GITHUB_TOKEN`, `GH_CONFIG_DIR`).
//!
//! # The script
//!
//! ```json
//! {"signed_in": true,
//!  "pulls": {"acme/app": {"app/login": {"number": 12, "title": "…", "state": "OPEN", "…": "…"}}},
//!  "fail": {"acme/api": "HTTP 502: Bad Gateway"}}
//! ```
//!
//! `signed_in: false` makes `gh auth status` end with 1. `pulls` gives, by repository and branch,
//! the object `gh pr view --json` answers (its `number` is what `gh pr list` answers); a branch
//! with none has no pull request. `fail` makes every `pr` call on that repository end with 1,
//! saying its text.
//!
//! The writes answer as `gh` does when they work: `pr create` prints the new pull request's URL
//! (numbered `created`, 1 by default), `pr merge` and `run rerun` print nothing, and `run list`
//! prints `runs` of the repository (`{"runs": {"acme/app": [{"databaseId": 7, …}]}}`), `[]`
//! without.

use std::io::Write;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir =
        PathBuf::from(std::env::var_os("FLOTTILLE_FAKE_GH_DIR").expect("FLOTTILLE_FAKE_GH_DIR"));
    let env: Map<String, Value> = ["GH_TOKEN", "GITHUB_TOKEN", "GH_CONFIG_DIR"]
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

/// What `gh` prints for `args`, and its exit code.
fn answer(script: &Value, args: &[String]) -> i32 {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let option = |name: &str| {
        words
            .iter()
            .position(|word| *word == name)
            .and_then(|at| words.get(at + 1).copied())
    };
    match words.as_slice() {
        ["auth", "status", ..] => {
            if script["signed_in"] == false {
                eprintln!(
                    "You are not logged into any GitHub hosts. To log in, run: gh auth login"
                );
                return 1;
            }
            eprintln!("github.com\n  ✓ Logged in to github.com account someone (keyring)");
            0
        }
        ["pr", "create" | "merge", ..] | ["run", "list" | "rerun", ..] => {
            let Some(repository) = option("-R") else {
                eprintln!("fake gh: no -R");
                return 2;
            };
            if let Some(failure) = script["fail"][repository].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            match words[..2] {
                ["pr", "create"] => println!(
                    "https://github.com/{repository}/pull/{}",
                    script["created"].as_u64().unwrap_or(1)
                ),
                ["run", "list"] => match &script["runs"][repository] {
                    Value::Null => println!("[]"),
                    runs => println!("{runs}"),
                },
                _ => {}
            }
            0
        }
        ["pr", sub @ ("list" | "view"), rest @ ..] => {
            let Some(repository) = option("-R") else {
                eprintln!("fake gh: no -R");
                return 2;
            };
            if let Some(failure) = script["fail"][repository].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            let pulls = &script["pulls"][repository];
            if *sub == "list" {
                let found = option("--head")
                    .and_then(|branch| pulls[branch]["number"].as_u64())
                    .map(|number| vec![json!({"number": number})])
                    .unwrap_or_default();
                println!("{}", Value::from(found));
                return 0;
            }
            let number: u64 = rest.first().and_then(|n| n.parse().ok()).unwrap_or(0);
            let found = pulls
                .as_object()
                .into_iter()
                .flat_map(|branches| branches.values())
                .find(|pull| pull["number"] == number);
            match found {
                Some(pull) => {
                    println!("{pull}");
                    0
                }
                None => {
                    eprintln!("no pull requests found for #{number}");
                    1
                }
            }
        }
        _ => {
            eprintln!("fake gh: not a command Flottille runs: {}", words.join(" "));
            2
        }
    }
}
