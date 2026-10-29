//! A scripted `bkt` (<https://github.com/avivsinai/bitbucket-cli>) for the tests of Flottille's
//! Bitbucket reads and writes (`docs/forges.md`, `docs/commands.md`), so that no test ever calls
//! Bitbucket nor uses anyone's sign-in. It answers the few commands Flottille runs, in the shapes
//! `docs/forges.md` takes from the pinned release's source, and records every call.
//!
//! # Its folder
//!
//! Everything goes through one folder, named by `FLOTTILLE_FAKE_BKT_DIR`:
//!
//! - `script.json`, written by the test (optional), read again at every call;
//! - `received.jsonl`, written by the fake: one `{"args": [...], "env": {...}}` per call, `env`
//!   holding the forge variables it was given (`BKT_TOKEN`, `BKT_CONFIG_DIR`).
//!
//! # The script
//!
//! ```json
//! {"contexts": [{"name": "cloud", "host": "bitbucket.org"}],
//!  "pulls": {"acme/api": [{"id": 12, "title": "…", "state": "OPEN", "…": "…"}]},
//!  "checks": {"acme/api": {"12": [{"state": "SUCCESSFUL", "name": "build"}]}},
//!  "api": {"/repositories/acme/api/pullrequests/12/comments": {"values": []}},
//!  "pipelines": {"acme/api": [{"uuid": "{…}", "build_number": 7, "…": "…"}]},
//!  "fail": {"acme/api": "404 Not Found"}}
//! ```
//!
//! `contexts` are what `bkt context list --json` lists (a context on `bitbucket.org` is Cloud,
//! any other host is Data Center; without the key, one Cloud context named `cloud`). Like the
//! real one, every other command needs `--context <name>` naming one of them, and the repository
//! given as `--workspace` (Cloud) or `--project` (Data Center) and `--repo`: the script's
//! repositories are keyed `<workspace or project>/<repo>`. `pulls` holds what `bkt pr list --json`
//! prints in `pull_requests` (`--state` filters, `all` keeps every one), `checks` the `statuses`
//! of `bkt pr checks <id> --json`, `api` the body `bkt api <path>` prints (a path it does not hold
//! is a 404), `pipelines` the `pipelines` of `bkt pipeline list --json`, and `fail` makes every
//! call on that repository end with 1, saying its text.
//!
//! The writes answer as `bkt` does when they work: `pr create` prints `✓ Created pull request #N`
//! (N is `created`, 1 by default), `pr merge` prints `✓ Merged pull request #N` and `pipeline run`
//! `✓ Triggered pipeline …`. `pr comment <id> --text T` prints `✓ Commented on pull request #N`
//! and `pr edit <id>` (`--title`, `--description`, at least one) `✓ Updated pull request #N`, as
//! `pkg/cmd/pr/pr.go` does (`:1707`, `:829`); both need a pull request of `pulls`.

use std::io::Write;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir =
        PathBuf::from(std::env::var_os("FLOTTILLE_FAKE_BKT_DIR").expect("FLOTTILLE_FAKE_BKT_DIR"));
    let env: Map<String, Value> = ["BKT_TOKEN", "BKT_CONFIG_DIR"]
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

/// The contexts `bkt context list --json` lists.
fn contexts(script: &Value) -> Vec<Value> {
    match script["contexts"].as_array() {
        Some(contexts) => contexts.clone(),
        None => vec![json!({"name": "cloud", "host": "bitbucket.org", "active": false})],
    }
}

/// What `bkt` prints for `args`, and its exit code.
fn answer(script: &Value, args: &[String]) -> i32 {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let option = |name: &str| {
        words
            .iter()
            .position(|word| *word == name)
            .and_then(|at| words.get(at + 1).copied())
    };
    let positional: Vec<&str> = {
        let mut found = Vec::new();
        let mut at = 0;
        while let Some(word) = words.get(at) {
            if word.starts_with("--") && !word.contains('=') && *word != "--json" {
                at += 2;
                continue;
            }
            if !word.starts_with('-') {
                found.push(*word);
            }
            at += 1;
        }
        found
    };
    if positional[..positional.len().min(2)] == ["context", "list"] {
        println!("{}", json!({"contexts": contexts(script)}));
        return 0;
    }
    // Every other command is asked for a context, and for its repository.
    let Some(context) = contexts(script)
        .into_iter()
        .find(|c| option("--context").is_some() && c["name"].as_str() == option("--context"))
    else {
        eprintln!(
            "Error: no active context; run `bkt context use <name>` (or --context names none)"
        );
        return 1;
    };
    let cloud = context["host"] == "bitbucket.org";
    let (owner_flag, owner_key) = match cloud {
        true => ("--workspace", "workspace"),
        false => ("--project", "project"),
    };
    let is_api = positional.first() == Some(&"api");
    let (Some(owner), Some(repo)) = (option(owner_flag), option("--repo")) else {
        if is_api {
            return api(script, positional[1]);
        }
        eprintln!(
            "Error: context must supply workspace and repo; use --workspace/--repo if needed"
        );
        return 1;
    };
    let key = format!("{owner}/{repo}");
    if let Some(failure) = script["fail"][&key].as_str() {
        eprintln!("{failure}");
        return 1;
    }
    let mut payload = json!({owner_key: owner, "repo": repo});
    match positional.as_slice() {
        ["pr", "list"] => {
            let state = option("--state").unwrap_or("OPEN").to_uppercase();
            let pulls: Vec<Value> = script["pulls"][&key]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|pull| state == "ALL" || pull["state"].as_str() == Some(&state))
                .cloned()
                .collect();
            payload["pull_requests"] = Value::from(pulls);
        }
        ["pr", "checks", id] => {
            payload["pull_request"] = json!(id.parse::<u64>().unwrap_or(0));
            payload["commit"] = json!("0123456789abcdef");
            payload["statuses"] = match &script["checks"][&key][*id] {
                Value::Null => json!([]),
                statuses => statuses.clone(),
            };
        }
        ["pipeline", "list"] => {
            payload["pipelines"] = match &script["pipelines"][&key] {
                Value::Null => json!([]),
                pipelines => pipelines.clone(),
            };
        }
        ["pr", "create"] => {
            println!(
                "✓ Created pull request #{}",
                script["created"].as_u64().unwrap_or(1)
            );
            return 0;
        }
        ["pr", verb @ ("comment" | "edit"), id] => {
            let held = script["pulls"][&key]
                .as_array()
                .into_iter()
                .flatten()
                .any(|pull| id.parse().ok() == pull["id"].as_u64());
            if !held {
                eprintln!("Error: 404 Not Found: pull request {id}");
                return 1;
            }
            if *verb == "comment" {
                if option("--text").is_none_or(str::is_empty) {
                    eprintln!("Error: --text is required");
                    return 1;
                }
                println!("✓ Commented on pull request #{id}");
            } else {
                if option("--title").is_none() && option("--description").is_none() {
                    eprintln!(
                        "Error: at least one of --title, --body, or --description is required"
                    );
                    return 1;
                }
                println!("✓ Updated pull request #{id}");
            }
            return 0;
        }
        ["pr", "merge", id] => {
            println!("✓ Merged pull request #{id}");
            return 0;
        }
        ["pipeline", "run"] => {
            println!("✓ Triggered pipeline {{0000}} on {key} (PENDING)");
            return 0;
        }
        _ => {
            eprintln!(
                "fake bkt: not a command Flottille runs: {}",
                words.join(" ")
            );
            return 2;
        }
    }
    println!("{payload}");
    0
}

/// `bkt api <path>`: the script's body for that path, as the server answers it.
fn api(script: &Value, path: &str) -> i32 {
    match script["api"].get(path) {
        Some(body) => {
            println!("{body}");
            0
        }
        None => {
            eprintln!("Error: 404 Not Found: {path}");
            1
        }
    }
}
