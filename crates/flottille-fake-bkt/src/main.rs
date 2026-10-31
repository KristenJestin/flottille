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
//!  "steps": {"acme/api": {"7": [{"uuid": "{…}", "name": "test", "…": "…"}]}},
//!  "logs": {"acme/api": {"7": {"{step uuid}": "the log"}}},
//!  "fail": {"acme/api": "404 Not Found"}}
//! ```
//!
//! `contexts` are what `bkt context list --json` lists (a context on `bitbucket.org` is Cloud,
//! any other host is Data Center; without the key, one Cloud context named `cloud`). Like the
//! real one, every other command needs `--context <name>` naming one of them, and the repository
//! given as `--workspace` (Cloud) or `--project` (Data Center) and `--repo`: the script's
//! repositories are keyed `<workspace or project>/<repo>`. `pulls` holds what `bkt pr list --json`
//! prints in `pull_requests` (`--state` filters, `all` keeps every one; a Cloud pull request's
//! `source.commit.hash` and a Data Center one's `fromRef.latestCommit` are passed through as the
//! script gives them), `checks` the `statuses`
//! of `bkt pr checks <id> --json` (a pull request with no entry answers `null`, as a commit with
//! no status does), `api` the body `bkt api <path>` prints (a path it does not hold
//! is a 404), `pipelines` the pipelines of a Cloud repository as Bitbucket's API gives them (newest
//! first; `target.ref_name` for a branch's, `target.source` for a pull request's), which
//! `bkt api /repositories/<w>/<r>/pipelines/` passes through and `bkt pipeline list --json`
//! prints the way `bkt` models them, with an empty `target.ref.name` (it maps `ref.name` while the
//! API says `ref_name`; seen with v0.37.0), `steps` the `steps` of `bkt pipeline view <build>
//! --json` and `logs` what `bkt pipeline logs <build> [--step <uuid>]` prints (the last step's by
//! default), and `fail` makes every call on that repository end with 1, saying its text.
//!
//! The writes answer as `bkt` does when they work: `pr create` prints `✓ Created pull request #N`
//! (N is `created`, 1 by default), `pr merge` prints `✓ Merged pull request #N` and `pipeline run`
//! `✓ Triggered pipeline …`. `pr comment <id> --text T` prints `✓ Commented on pull request #N`
//! and `pr edit <id>` (`--title`, `--description`, at least one) `✓ Updated pull request #N`, as
//! `pkg/cmd/pr/pr.go` does (`:1707`, `:829`); both need a pull request of `pulls`. With
//! `strategies` (`{"acme/api": ["merge_commit"]}`), `pr merge` fails for a `--strategy` the
//! repository's list does not hold, as the server refuses it. A pull request whose `checks` is the
//! string `"unreadable"` has a `bkt pr checks --json` answer without `statuses`, as a `bkt` whose
//! output changed shape.
//!
//! # Page sizes
//!
//! Bitbucket Cloud refuses a `pagelen` above the maximum of the endpoint (`400 Bad Request:
//! Invalid pagelen`, seen with `bkt` v0.37.0: its pull request list takes 50 at most), and
//! Atlassian's REST documentation names no maximum for the other endpoints Flottille reads, so the
//! fake refuses above `MAX_PAGELEN` (50, the value safe everywhere) on `pr list --limit` and on a
//! Cloud `bkt api` `pagelen`. A Cloud `api` body that holds more `values` than the `pagelen` asked
//! is served page by page, `next` an absolute link as the server gives it (`?page=2&pagelen=…`);
//! a Data Center body is served by `limit` and `start` (`isLastPage`, `nextPageStart`). A body
//! that says its own `next` or `isLastPage` is passed through as it is.

use std::io::Write;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

/// The largest `pagelen` the fake accepts: Bitbucket Cloud's pull request list takes 50 at most.
const MAX_PAGELEN: usize = 50;

/// What Bitbucket Cloud answers to a `pagelen` it does not take, as `bkt` prints it.
const INVALID_PAGELEN: &str = "400 Bad Request: Invalid pagelen";

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
    // Every `--param key=value` of `bkt api`.
    let params: Vec<(&str, &str)> = words
        .windows(2)
        .filter(|pair| pair[0] == "--param")
        .filter_map(|pair| pair[1].split_once('='))
        .collect();
    let (Some(owner), Some(repo)) = (option(owner_flag), option("--repo")) else {
        if is_api {
            return api(script, positional[1], &params);
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
            if cloud
                && option("--limit")
                    .and_then(|limit| limit.parse::<usize>().ok())
                    .is_some_and(|limit| limit > MAX_PAGELEN)
            {
                eprintln!("{INVALID_PAGELEN}");
                return 1;
            }
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
            match &script["checks"][&key][*id] {
                // A commit with no status: `bkt` answers `null` (a nil Go slice), not `[]`.
                Value::Null => payload["statuses"] = Value::Null,
                Value::String(shape) if shape == "unreadable" => {}
                statuses => payload["statuses"] = statuses.clone(),
            }
        }
        ["pipeline", "list"] => {
            // `bkt` decodes `target.ref.name`, which the API never sends: it is always empty.
            payload["pipelines"] = script["pipelines"][&key]
                .as_array()
                .into_iter()
                .flatten()
                .map(|pipeline| {
                    let mut listed = pipeline.clone();
                    listed["target"] =
                        json!({"type": pipeline["target"]["type"], "ref": {"name": ""}});
                    listed
                })
                .collect();
        }
        ["pipeline", "view", build] => {
            let Some(steps) = script["steps"][&key].get(*build) else {
                eprintln!("Error: 404 Not Found: pipeline {build}");
                return 1;
            };
            payload["pipeline"] = json!({"build_number": build.parse::<u64>().unwrap_or(0)});
            payload["steps"] = steps.clone();
        }
        ["pipeline", "logs", build] => {
            let Some(logs) = script["logs"][&key][*build].as_object() else {
                eprintln!("Error: 404 Not Found: pipeline {build}");
                return 1;
            };
            let step = option("--step").map(str::to_owned).or_else(|| {
                let steps = script["steps"][&key][*build].as_array()?;
                Some(steps.last()?["uuid"].as_str()?.to_owned())
            });
            match step.as_deref().and_then(|step| logs.get(step)?.as_str()) {
                Some(log) => print!("{log}"),
                None => {
                    eprintln!("Error: 404 Not Found: the log of that step");
                    return 1;
                }
            }
            return 0;
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
            let strategy = option("--strategy").unwrap_or_default();
            if let Some(allowed) = script["strategies"][&key].as_array()
                && !allowed.iter().any(|s| s == strategy)
            {
                eprintln!(
                    "Error: 400 Bad Request: the merge strategy {strategy} is not allowed for \
                     this repository"
                );
                return 1;
            }
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

/// The body the script holds for `path`: its `api`, else the `pipelines` of a Cloud repository.
fn body_of(script: &Value, path: &str) -> Option<Value> {
    if let Some(body) = script["api"].get(path) {
        return Some(body.clone());
    }
    let repository = path
        .strip_prefix("/repositories/")?
        .strip_suffix("/pipelines/")?;
    Some(json!({"values": script["pipelines"].get(repository)?}))
}

/// `bkt api <path>`: the script's body for that path, as the server answers it. A `next` link is
/// followed as `bkt` does: an absolute URL has its host and the `/2.0` of the base path cut.
fn api(script: &Value, path: &str, params: &[(&str, &str)]) -> i32 {
    let relative = path
        .strip_prefix("https://api.bitbucket.org")
        .unwrap_or(path)
        .trim_start_matches("/2.0");
    let (path, query) = relative.split_once('?').unwrap_or((relative, ""));
    let mut asked: Vec<(&str, &str)> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .collect();
    asked.extend(params);
    let get = |name: &str| {
        asked
            .iter()
            .rev()
            .find(|(key, _)| *key == name)
            .and_then(|(_, value)| value.parse::<usize>().ok())
    };
    let Some(mut body) = body_of(script, path) else {
        eprintln!("Error: 404 Not Found: {path}");
        return 1;
    };
    let values = body["values"].as_array().cloned();
    if path.starts_with("/rest/api/") {
        // Data Center clamps a `limit` it does not take, and says where the next page starts.
        if let (Some(values), Some(limit)) = (values, get("limit"))
            && body.get("isLastPage").is_none()
        {
            let start = get("start").unwrap_or(0);
            let end = (start + limit).min(values.len());
            body["values"] = Value::from(values[start.min(end)..end].to_vec());
            body["isLastPage"] = json!(end >= values.len());
            if end < values.len() {
                body["nextPageStart"] = json!(end);
            }
        }
    } else if let Some(pagelen) = get("pagelen") {
        if pagelen > MAX_PAGELEN {
            eprintln!("{INVALID_PAGELEN}");
            return 1;
        }
        if let Some(values) = values
            && body.get("next").is_none()
        {
            let page = get("page").unwrap_or(1);
            let (from, to) = ((page - 1) * pagelen, page * pagelen);
            body["values"] =
                Value::from(values[from.min(values.len())..to.min(values.len())].to_vec());
            body["pagelen"] = json!(pagelen);
            body["page"] = json!(page);
            if to < values.len() {
                body["next"] = json!(format!(
                    "https://api.bitbucket.org/2.0{path}?pagelen={pagelen}&page={}",
                    page + 1
                ));
            }
        }
    }
    println!("{body}");
    0
}
