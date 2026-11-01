//! A scripted `gh` for the tests of Flottille's forge reads and writes (`docs/forges.md`,
//! `docs/commands.md`), so that no test
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
//!   holding the forge variables it was given (`GH_TOKEN`, `GITHUB_TOKEN`, `GH_CONFIG_DIR`,
//!   `GH_HOST`).
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
//! the object `gh pr view --json` answers (`gh pr list --json` answers the fields it asks that the
//! object has); a branch with none has no pull request. `fail` makes every `pr` call on that
//! repository end with 1, saying its text. `issues` gives, by repository and number, the object
//! `gh issue view --json` answers; an issue it does not hold is not found. `fail` makes `issue`
//! calls fail the same way.
//!
//! As the real `gh` does, `-R` names the host first (`github.com/acme/app`) or takes it from
//! `GH_HOST`, else `github.com`. The script's keys are `owner/name` on `github.com`, and
//! `host/owner/name` on any other host.
//!
//! The writes answer as `gh` does when they work: `pr create` prints the new pull request's URL
//! (numbered `created`, 1 by default), `pr merge` and `run rerun` print nothing, and `run list`
//! prints `runs` of the repository (`{"runs": {"acme/app": [{"databaseId": 7, …}]}}`), `[]`
//! without. `pr comment`, `pr edit`, `pr close` and `pr reopen` print the pull request's URL, of one the script holds.
//! With `merge_methods` (`{"acme/app": ["merge", "rebase"]}`), `pr merge` fails for a method
//! (`--squash`, `--merge`, `--rebase`) the repository does not allow, as GitHub refuses it, and
//! `repo view <repository> --json …Allowed` says them (every one without). `pr view` gives a pull
//! request its branch as `headRefName` when the script does not. `view_omits`
//! (`{"acme/app": ["number"]}`) takes those fields out of what `pr view` answers, as a `gh` whose
//! output lost them.
//!
//! `pr list` keeps, as `gh` does, the pull requests of `--state` (`open` by default: `OPEN`,
//! drafts included; `closed`, `merged`, `all`) and of `--head` when given, newest first, at most
//! `--limit`; each with the fields `--json` asks that it has, `headRefName` being its branch.
//!
//! A `--search` of `pr list` and `issue list` keeps, for each `updated:>=<time>` it holds, the
//! objects whose `updatedAt` is that time or later (compared as the RFC 3339 texts they are, as
//! GitHub's search reads the qualifier), with `--state all` too; `pr list` reads no other word
//! of it. `api user` prints the script's `user` (`{"login": "someone"}` without).
//!
//! `run_fail` (`{"acme/app": "HTTP 502: Bad Gateway"}`) makes `run list` alone fail that way, the
//! pull request calls still working.
//!
//! `run view --job <id> --log-failed` prints the log `logs` holds for that job of the repository
//! (`{"logs": {"acme/app": {"456": "build\tRun tests\t2026-01-01T00:00:00.0000000Z …\n"}}}`),
//! as `gh` prints a failed job's steps; a job it does not hold is a log `gh` cannot find. A
//! `log_delay_ms` makes it wait that long first, as a slow network does. Any other `run view` is
//! not a command Flottille runs.
//!
//! `issue list` keeps the issues of `--state` (`open` by default), with a `--label` and an
//! `--assignee` (`labels[].name`, `assignees[].login`) when given and, for `--search`, those whose
//! title or body hold its words (a simplification of GitHub's search), newest first, at most
//! `--limit`. `issue create` prints the new issue's URL (numbered `created_issue`, 1 by default);
//! `issue comment`, `edit`, `close` and `reopen` need an issue the script holds and print its
//! URL; `issue close --reason` takes `completed` or `not planned` only.
//!
//! The boards of GitHub Projects: an issue of the script holds `projectItems` (`gh issue view
//! --json projectItems`: `[{"id": "PVTI_1", "project": {"id": "PVT_a", "title": "Roadmap"},
//! "status": {"optionId": "o1", "name": "Todo"}}]`); `projects` gives, by owner, what `gh project
//! list --owner` lists (`{"acme": [{"id": "PVT_a", "number": 3, "title": "Roadmap"}]}`);
//! `project_fields` gives, by `owner/number`, what `gh project field-list` lists (`{"acme/3":
//! [{"id": "F_s", "name": "Status", "options": [{"id": "o1", "name": "Todo"}]}]}`); `project
//! item-edit` answers nothing. `missing_scope: true` makes every `project` call, and an `issue
//! view` asking `projectItems`, fail as `gh` does without the `project` scope.

use std::io::Write;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir =
        PathBuf::from(std::env::var_os("FLOTTILLE_FAKE_GH_DIR").expect("FLOTTILLE_FAKE_GH_DIR"));
    let env: Map<String, Value> = ["GH_TOKEN", "GITHUB_TOKEN", "GH_CONFIG_DIR", "GH_HOST"]
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

/// The fields `--json` asks of each object that it has (`number` when none is asked), and
/// `headRefName`, the branch a pull request is keyed by, when it is asked and not given.
fn fields_of(objects: &[&Value], fields: Option<&str>, branches: Option<&[&str]>) -> Vec<Value> {
    let fields = fields.unwrap_or("number");
    objects
        .iter()
        .enumerate()
        .map(|(at, object)| {
            let mut asked: Map<String, Value> = fields
                .split(',')
                .filter(|field| !object[field].is_null())
                .map(|field| (field.to_owned(), object[field].clone()))
                .collect();
            if let Some(branch) = branches.map(|branches| branches[at])
                && fields.split(',').any(|field| field == "headRefName")
            {
                asked
                    .entry("headRefName")
                    .or_insert_with(|| Value::from(branch));
            }
            Value::from(asked)
        })
        .collect()
}

/// Whether `object` was updated at or after every `updated:>=` of a search.
fn updated_since(object: &Value, search: Option<&str>) -> bool {
    search
        .into_iter()
        .flat_map(str::split_whitespace)
        .all(|word| {
            word.strip_prefix("updated:>=")
                .is_none_or(|since| object["updatedAt"].as_str().is_some_and(|at| at >= since))
        })
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
    // The repository as the script names it: its host first unless it is `github.com`.
    let repository = option("-R").map(|asked| match asked.split('/').collect::<Vec<_>>()[..] {
        ["github.com", owner, name] => format!("{owner}/{name}"),
        [_, _, _] => asked.to_owned(),
        _ => match std::env::var("GH_HOST") {
            Ok(host) if !host.is_empty() && host != "github.com" => format!("{host}/{asked}"),
            _ => asked.to_owned(),
        },
    });
    let repository = repository.as_deref();
    let asks_boards = words.starts_with(&["issue", "view"])
        && option("--json").is_some_and(|fields| fields.contains("projectItems"));
    if script["missing_scope"] == true && (words.first() == Some(&"project") || asks_boards) {
        eprintln!(
            "error: your authentication token is missing required scopes [read:project]\n\
             To request it, run:  gh auth refresh -s read:project"
        );
        return 1;
    }
    match words.as_slice() {
        ["project", "list", ..] => {
            let projects = &script["projects"][option("--owner").unwrap_or_default()];
            let projects = projects.as_array().cloned().unwrap_or_default();
            println!(
                "{}",
                json!({"projects": projects, "totalCount": projects.len()})
            );
            0
        }
        ["project", "field-list", number, ..] => {
            let key = format!("{}/{number}", option("--owner").unwrap_or_default());
            match script["project_fields"][&key].as_array() {
                Some(fields) => {
                    println!("{}", json!({"fields": fields, "totalCount": fields.len()}));
                    0
                }
                None => {
                    eprintln!(
                        "GraphQL: Could not resolve to a ProjectV2 with the number {number}."
                    );
                    1
                }
            }
        }
        ["project", "item-edit", ..] => {
            let flags = [
                "--id",
                "--project-id",
                "--field-id",
                "--single-select-option-id",
            ];
            if flags.iter().any(|flag| option(flag).is_none()) {
                eprintln!("fake gh: item-edit needs --id, --project-id, --field-id and an option");
                return 2;
            }
            0
        }
        ["api", "user", ..] => {
            match &script["user"] {
                Value::Null => println!("{}", json!({"login": "someone"})),
                user => println!("{user}"),
            }
            0
        }
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
        ["issue", "view", number, ..] => {
            let Some(repository) = repository else {
                eprintln!("fake gh: no -R");
                return 2;
            };
            if let Some(failure) = script["fail"][repository].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            match script["issues"][repository].get(*number) {
                Some(issue) => {
                    println!("{issue}");
                    0
                }
                None => {
                    eprintln!(
                        "GraphQL: Could not resolve to an Issue with the number of {number}."
                    );
                    1
                }
            }
        }
        ["issue", "list", ..] => {
            let Some(repository) = repository else {
                eprintln!("fake gh: no -R");
                return 2;
            };
            if let Some(failure) = script["fail"][repository].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            let state = option("--state").unwrap_or("open");
            let limit: usize = option("--limit").and_then(|n| n.parse().ok()).unwrap_or(30);
            let named = |list: &Value, key: &str, name: &str| {
                list.as_array()
                    .into_iter()
                    .flatten()
                    .any(|item| item[key] == name)
            };
            let mut issues: Vec<&Value> = script["issues"][repository]
                .as_object()
                .into_iter()
                .flat_map(|issues| issues.values())
                .filter(|issue| match state {
                    "all" => true,
                    state => issue["state"].as_str() == Some(&state.to_uppercase()),
                })
                .filter(|issue| {
                    option("--label").is_none_or(|l| named(&issue["labels"], "name", l))
                })
                .filter(|issue| {
                    option("--assignee").is_none_or(|a| named(&issue["assignees"], "login", a))
                })
                .filter(|issue| updated_since(issue, option("--search")))
                .filter(|issue| {
                    option("--search").is_none_or(|words| {
                        let text = format!("{} {}", issue["title"], issue["body"]).to_lowercase();
                        words
                            .split_whitespace()
                            .filter(|word| !word.starts_with("updated:"))
                            .all(|word| text.contains(&word.to_lowercase()))
                    })
                })
                .collect();
            issues.sort_by_key(|issue| std::cmp::Reverse(issue["number"].as_u64()));
            issues.truncate(limit);
            println!(
                "{}",
                Value::from(fields_of(&issues, option("--json"), None))
            );
            0
        }
        [
            "issue",
            "create" | "comment" | "edit" | "close" | "reopen",
            ..,
        ] => {
            let Some(repository) = repository else {
                eprintln!("fake gh: no -R");
                return 2;
            };
            if let Some(failure) = script["fail"][repository].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            if words[1] == "close"
                && option("--reason").is_some_and(|r| !["completed", "not planned"].contains(&r))
            {
                eprintln!("invalid argument for \"--reason\": expected completed or not planned");
                return 1;
            }
            let number = match words[1] {
                "create" => script["created_issue"].as_u64().unwrap_or(1).to_string(),
                _ => {
                    let number = words.get(2).copied().unwrap_or_default();
                    if script["issues"][repository].get(number).is_none() {
                        eprintln!(
                            "GraphQL: Could not resolve to an Issue with the number of {number}."
                        );
                        return 1;
                    }
                    number.to_owned()
                }
            };
            println!("https://github.com/{repository}/issues/{number}");
            0
        }
        ["pr", "comment" | "edit" | "close" | "reopen", number, ..] => {
            let Some(repository) = repository else {
                eprintln!("fake gh: no -R");
                return 2;
            };
            if let Some(failure) = script["fail"][repository].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            let held = script["pulls"][repository]
                .as_object()
                .into_iter()
                .flat_map(|branches| branches.values())
                .any(|pull| number.parse().ok() == pull["number"].as_u64());
            if !held {
                eprintln!("no pull requests found for #{number}");
                return 1;
            }
            println!("https://github.com/{repository}/pull/{number}");
            0
        }
        ["repo", "view", asked, ..] => {
            let repository = asked.strip_prefix("github.com/").unwrap_or(asked);
            let methods = &script["merge_methods"][repository];
            let allows = |method: &str| {
                methods
                    .as_array()
                    .is_none_or(|methods| methods.iter().any(|m| m == method))
            };
            let all = json!({"mergeCommitAllowed": allows("merge"),
                "rebaseMergeAllowed": allows("rebase"), "squashMergeAllowed": allows("squash")});
            println!("{}", fields_of(&[&all], option("--json"), None)[0]);
            0
        }
        ["pr", "merge", ..]
            if script["merge_methods"][repository.unwrap_or_default()]
                .as_array()
                .is_some_and(|methods| {
                    !["squash", "merge", "rebase"].iter().any(|method| {
                        words.contains(&format!("--{method}").as_str())
                            && methods.iter().any(|m| m == method)
                    })
                }) =>
        {
            let method = ["squash", "merge", "rebase"]
                .into_iter()
                .find(|method| words.contains(&format!("--{method}").as_str()))
                .unwrap_or("squash");
            eprintln!(
                "GraphQL: Merge method {method} merging is not allowed on this repository \
                 (mergePullRequest)"
            );
            1
        }
        ["run", "view", ..] if words.contains(&"--log-failed") => {
            let (Some(repository), Some(job)) = (repository, option("--job")) else {
                eprintln!("fake gh: run view needs -R and --job");
                return 2;
            };
            if let Some(failure) = script["fail"][repository].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            if let Some(ms) = script["log_delay_ms"].as_u64() {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            match script["logs"][repository][job].as_str() {
                Some(log) => {
                    print!("{log}");
                    0
                }
                None => {
                    eprintln!("failed to get run log: log not found");
                    1
                }
            }
        }
        ["pr", "create" | "merge", ..] | ["run", "list" | "rerun", ..] => {
            let Some(repository) = repository else {
                eprintln!("fake gh: no -R");
                return 2;
            };
            if let Some(failure) = script["fail"][repository].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            if words[..2] == ["run", "list"]
                && let Some(failure) = script["run_fail"][repository].as_str()
            {
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
            let Some(repository) = repository else {
                eprintln!("fake gh: no -R");
                return 2;
            };
            if let Some(failure) = script["fail"][repository].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            let pulls = &script["pulls"][repository];
            if *sub == "list" {
                let state = option("--state").unwrap_or("open");
                let limit: usize = option("--limit").and_then(|n| n.parse().ok()).unwrap_or(30);
                let mut found: Vec<(&String, &Value)> = pulls
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(branch, _)| option("--head").is_none_or(|head| head == *branch))
                    .filter(|(_, pull)| pull["number"].is_u64())
                    .filter(|(_, pull)| updated_since(pull, option("--search")))
                    .filter(|(_, pull)| match (state, pull["state"].as_str()) {
                        ("all", _) => true,
                        ("open", state) => state.is_none_or(|s| s == "OPEN"),
                        (asked, Some(state)) => state.eq_ignore_ascii_case(asked),
                        _ => false,
                    })
                    .collect();
                found.sort_by_key(|(_, pull)| std::cmp::Reverse(pull["number"].as_u64()));
                found.truncate(limit);
                let branches: Vec<&str> = found.iter().map(|(b, _)| b.as_str()).collect();
                let pulls: Vec<&Value> = found.iter().map(|(_, pull)| *pull).collect();
                let listed = fields_of(&pulls, option("--json"), Some(&branches));
                println!("{}", Value::from(listed));
                return 0;
            }
            let number: u64 = rest.first().and_then(|n| n.parse().ok()).unwrap_or(0);
            let found = pulls
                .as_object()
                .into_iter()
                .flatten()
                .find(|(_, pull)| pull["number"] == number);
            match found {
                Some((branch, pull)) => {
                    let mut pull = pull.clone();
                    if pull["headRefName"].is_null() {
                        pull["headRefName"] = Value::from(branch.as_str());
                    }
                    for omitted in script["view_omits"][repository]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        if let Some(field) = omitted.as_str() {
                            pull.as_object_mut().map(|fields| fields.remove(field));
                        }
                    }
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
