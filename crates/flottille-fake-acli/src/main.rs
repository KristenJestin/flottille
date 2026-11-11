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
//! - `edited.json`, written by the fake: the description each edited work item holds (below);
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
//! `site` (`example.atlassian.net` by default) is the `Site:` line `acli jira auth status` prints, none
//! with `site_line: false`; `delay_ms` makes that command slow, as one waiting on a network, and
//! `view_delay_ms` does the same for `workitem view`.
//! `signed_in: false` makes `acli jira auth status` end with 1. `items` gives, by key, what
//! `acli jira workitem view KEY --json` prints; a key with none ends with 1. `fail` makes the view
//! of that key end with 1, saying its text. With `--fields a,b`, the view prints the work item's
//! `key`, its `self` and only those of its `fields`, as the real one is assumed to: a read that
//! forgot to ask for a field does not find it. Its `fields.comment.comments` are the script's,
//! then those posted (below). A work item's `attachment` (`filename`, `mimeType`, `size`, `created`,
//! `author`, `content`, as a real site gives them) and the `mediaSingle` nodes of its texts are the
//! script's as they are: a `media`'s `alt` is its attachment's `filename`. A script's `user` (`Casey Example` by default) is whom they are by.
//!
//! `acli jira workitem search --jql Q --json [--limit N]` prints the script's `search` (an array
//! of work items, Jira's REST shape assumed as for `view`), or every item of `items` without it,
//! at most `--limit`. Without `search`, the items are those the JQL finds (`found_by`): its
//! `project in`, `text ~`, `comment ~` and `updated >= "-Nd"`; a scripted `search` is printed as
//! it is. A `--fields` with a field the real one refuses (`updated`, `created`, …) ends
//! with 1, saying `✗ Error: fields '<those>' are not allowed`. The test reads the JQL in the record. Without
//! `--jql` (or `--filter`) or `--json` it ends with 2, as a call Flottille never makes.
//!
//! A JQL that names `currentUser()` is answered with the script's `mine_search`, none without:
//! the work items of the signed-in user. A search whose `--fields` is `key` alone prints `null`
//! for each work item it finds, as the real one was seen to: `key` is no field of `fields`.
//! Otherwise each work item is Jira's REST issue as a real site printed it: `key`, `id` and `self`
//! at the top, the other parts (`changelog`, `names`, …) `null`; its `fields` are those of the
//! script that `--fields` asks (all of them without it). With `--count` it prints `✓ Number of work items in the search: N` instead. The
//! script's `page` is how many a search prints at most without `--paginate`, none by default.
//!
//! `acli jira project view --key P --json` prints `{"key": P, "issueTypes": [{"name": …, "subtask":
//! …}]}` (assumed, as Jira's REST project is: the page of the command gives no output), from the
//! script's `types` (`{"ABC": ["Task", "Bug", {"name": "Sub-task", "subtask": true}]}`); a project
//! with none ends with 1. With a `types` for the project, a `workitem create --type T` of a type
//! it does not hold ends with 1, as Jira refuses it.
//!
//! The versions (#764, `docs/tickets.md`, "Versions"): `project view` prints, beside `issueTypes`,
//! the project's `versions` as Jira's REST project holds them (`{"id", "name", "released",
//! "releaseDate", "archived"}`), from the script's `versions` (`{"ABC": [{"name": "1.0",
//! "released": false}]}`), an empty list for a project with none, and no `versions` at all with
//! the script's `no_versions_field: true` (what a real `acli` may print, its page giving no
//! output). A project the script gives versions and no types is viewed too. A JQL
//! `fixVersion = "name"` finds the work items whose `fields.fixVersions` hold it, and an `AND
//! project = "P"` those of that project. A `workitem
//! create --from-json` whose `additionalAttributes.fixVersions` names a version the script does
//! not give the project ends with 1, `✗ Error: Version name '…' is not valid`, as Jira refuses it.
//! What such a creation set is kept in `created.json` (`{key: fixVersions}`), and a `view` of its
//! key (one `items` does not give) prints it: `summary` and `fixVersions`, with a `self` on the
//! script's site. The script's `drops_fix_versions: true` makes Jira ignore the field silently,
//! as a site may: the creation ends with 0 and the view shows none.
//!
//! The writes change nothing but the status a transition moves and the comments a `comment
//! create` adds: the test reads them in the record. One more script key, `links_lose_their_url:
//! true`, makes an `edit --description` change what the next views show, as Jira's document format
//! does: the text written, each Markdown link `[text](url)` kept as its `text` alone.
//!
//! A text may come in a file (`--description-file`, `--body-file`), as `acli` takes it, plain or
//! as Atlassian Document Format. The fake reads the file when it is called, since Flottille
//! removes it after, and records it under `files` (`{flag, path, mode, dir_mode, adf | text}`: the
//! modes of the file and of its folder, so that a test sees they are private). A document that
//! is not valid ADF (the shape of its nodes and marks, as Jira holds them) ends with 1, saying
//! why: `✗ Error: invalid ADF: …`.
//!
//! - `acli jira workitem create --project P --summary S … --json` prints `{"key": …}`, the
//!   script's `created` (`P-1` without it), or the script's `created_said` as it is (an answer
//!   naming no key); without `--project` or `--summary` it ends with 2.
//! - `acli jira workitem comment create --key K --body B` and `acli jira workitem edit --key K …
//!   --yes` end with 0 for a key of `items`, with 1 for another. A comment created is added, as
//!   the script's `user` wrote it, to the next views of the work item.
//! - `acli jira workitem transition --key K --status S --yes` ends with 0 when S is one of the
//!   script's `transitions` of K (`{"ABC-1": ["In Progress", "Done"]}`), in any case, and with 1
//!   saying so otherwise. These are the work item's transitions as Jira would answer them: the
//!   fake is the only place they are known, since the real `acli` lists none.
//!
//! The links (#365, `docs/tickets.md`, "Links and comments"), in the shapes a real Jira Cloud
//! site was seen to answer: `acli jira workitem link type --json` prints the names of the
//! script's `link_types` only (`{"issueLinkTypes": [{"name": "Blocks"}]}`; the script gives each
//! its `inward` and `outward` wordings too, which `view` shows). `link create --out A --in B
//! --type T --yes` (B blocks A, as Jira's page says it: `--out` is the item the outward wording
//! points at) ends with 0 for two work items of `items` and a type whose name or outward wording
//! is T, printing `✓ A Blocks B` as the real acli does, with 1 saying why otherwise, and keeps
//! the link in `linked.json` numbered from 10000. `view K --json` gives K's links in
//! `fields.issuelinks`, those of the script's `links` (`[{"id": "500", "type": "Blocks", "out":
//! "ABC-2", "in": "ABC-1"}]`) and those created, as `{"id", "type": {name, inward, outward},
//! "inwardIssue", "outwardIssue"}`, one of the two `null`: on the item `out` the other is the
//! `inwardIssue` (`A is blocked by B`), on the item `in` the `outwardIssue` (`B blocks A`), each
//! `{"key", "fields": {summary, status}}`. `link list --key K --json` prints `{"issueLinks":
//! [{"id", "outwardIssueKey", "typeName"}]}`, the key `null` when K is the outward item. `link
//! delete --id N --yes` ends with 0 for a link that is there, 1 otherwise, and keeps the ids
//! deleted in `unlinked.json`.
//!
//! `comment update --key K --id I (--body | --body-file | --body-adf)` (the first two plain text,
//! the last a document, refused when it is none) and `comment delete --key K --id I`
//! change the comments a `comment create` added, which are numbered from 10100 (`id`, in the
//! views); the script's own comments take an `id` of their own.
//!
//! Assignment, parents and sprints (#362), as measured on a real site where it could be: a
//! `--assignee` that is no email, no account id and not `@me` ends with 1 (`✗ Error: invalid
//! format for assignee email: …`), and one that is none of the script's `users` (every one known
//! without them) ends `create` with 1 (`✗ Error: User not found for email: …`) and `edit` with 0,
//! saying `✗ Failure: K can't be edited: …` on its output, as the real bulk command does.
//! `workitem create --from-json F --json` reads the work item's definition from F (recorded under
//! `files` as `json`): no `type` ends with 1 (`no value given for required property type`), and
//! an `assignee` of `@me` too (its JSON takes no `@me`). `jira board search --project P --json`
//! prints the script's `boards` of P (`{"ABC": [{"id": 7, "name": "ABC board", "type":
//! "scrum"}]}`) as Jira's agile page (`{"values": [...], …}`, as a real site printed it), and
//! `jira board list-sprints --id N [--state a,b] --json` the script's `sprints` of that board
//! (`{"7": [{"id": 42, "name": "Sprint 42", "state": "active"}]}`) in those states, in a page
//! whose list is under `sprints` (`{"sprints": [...], "isLast", …}`, as `acli` 1.3.39 printed it
//! on a real board, #657); a board with none ends with 1,
//! `✗ The board does not support sprints`, as the real one does. A view asked `*navigable` or
//! `*all` gives every field of the script, and a search's `sprint is not EMPTY` finds the work
//! items with a field holding sprints (`[{"id", "name", "state", "boardId"}]`).
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
    // One `write` of the whole line: `writeln!` on a `File` writes the JSON piece by piece, and
    // the pieces of two processes appending together would mix (#325).
    let files = files_of(&args);
    let line = format!("{}\n", json!({"args": args, "env": env, "files": files}));
    record.write_all(line.as_bytes()).expect("the record");
    let script: Value = std::fs::read_to_string(dir.join("script.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}));
    if let Some(why) = files.iter().find_map(|file| file["invalid"].as_str()) {
        eprintln!("✗ Error: invalid ADF: {why}");
        std::process::exit(1);
    }
    std::process::exit(answer(&dir, &script, &args));
}

/// Whether a work item of the script's `items` is one the JQL finds, read as a real site does,
/// enough for the tests: `project in ("A", …)` by the key's project, `text ~ "w …"` by every word
/// in its summary, description or comments, `comment ~ "w …"` in its comments only, and
/// `updated >= "-Nd"` by its `fields.updated` (`YYYY-MM-DDTHH:MM:SS`, read in UTC). The rest of
/// the JQL is not read.
fn found_by(jql: &str, item: &Value) -> bool {
    let fields = &item["fields"];
    let mut comments = String::new();
    for comment in fields["comment"]["comments"]
        .as_array()
        .into_iter()
        .flatten()
    {
        texts(&comment["body"], &mut comments);
    }
    let mut all = String::new();
    texts(&fields["summary"], &mut all);
    texts(&fields["description"], &mut all);
    all.push_str(&comments);
    let holds = |text: &str, words: &str| {
        let text = text.to_lowercase();
        words
            .split_whitespace()
            .all(|word| text.contains(&word.to_lowercase()))
    };
    if let Some(list) = jql
        .split_once("project in (")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(list, _)| list)
    {
        let project = item["key"]
            .as_str()
            .unwrap_or("")
            .split('-')
            .next()
            .unwrap_or("");
        if !list
            .split(',')
            .any(|p| p.trim().trim_matches('"') == project)
        {
            return false;
        }
    }
    if jql.contains("sprint is not EMPTY")
        && !fields
            .as_object()
            .into_iter()
            .flatten()
            .any(|(_, value)| value[0]["boardId"].is_number())
    {
        return false;
    }
    if let Some(project) = quoted_after(jql, "AND project = ")
        && item["key"]
            .as_str()
            .and_then(|key| key.split('-').next())
            .is_none_or(|of| of != project)
    {
        return false;
    }
    if let Some(version) = quoted_after(jql, "fixVersion = ")
        && !fields["fixVersions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|held| held["name"] == version.as_str())
    {
        return false;
    }
    if let Some(words) = quoted_after(jql, "text ~ ")
        && !holds(&all, &words)
    {
        return false;
    }
    if let Some(words) = quoted_after(jql, "comment ~ ")
        && !holds(&comments, &words)
    {
        return false;
    }
    if let Some(days) = jql
        .split_once("updated >= ")
        .map(|(_, rest)| rest.trim_start_matches('"'))
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.split_once('d'))
        .and_then(|(n, _)| n.parse::<u64>().ok())
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let at = fields["updated"].as_str().and_then(seconds_of);
        if at.is_none_or(|at| at < now.saturating_sub(days * 86_400)) {
            return false;
        }
    }
    true
}

/// The text of a field: a string as it is, an Atlassian Document Format tree as its text nodes.
fn texts(field: &Value, into: &mut String) {
    match field {
        Value::String(text) => {
            into.push_str(text);
            into.push(' ');
        }
        Value::Object(node) => {
            if let Some(text) = node.get("text").and_then(Value::as_str) {
                into.push_str(text);
                into.push(' ');
            }
            for child in node
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                texts(child, into);
            }
        }
        _ => {}
    }
}

/// The JQL string quoted right after `before`, its `\"` and `\\` read back.
fn quoted_after(jql: &str, before: &str) -> Option<String> {
    let rest = jql.split_once(before)?.1.strip_prefix('"')?;
    let mut text = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => text.extend(chars.next()),
            '"' => return Some(text),
            c => text.push(c),
        }
    }
    None
}

/// The seconds since 1970 of `YYYY-MM-DDTHH:MM:SS…`, read in UTC.
fn seconds_of(at: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| at.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    // Howard Hinnant's `days_from_civil`.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second).ok()
}

/// A script's `delay_ms` makes the sign-in check slow, as a program that hangs on the network.
fn wait(script: &Value) {
    if let Some(ms) = script["delay_ms"].as_u64() {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
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
            wait(script);
            if script["signed_in"] == false {
                eprintln!("✗ Not logged in. Run: acli jira auth login");
                return 1;
            }
            let site = script["site"].as_str().unwrap_or("example.atlassian.net");
            match script["site_line"] == false {
                true => println!("✓ Authenticated"),
                false => println!("✓ Authenticated\n  Site: {site}"),
            }
            0
        }
        ["jira", "workitem", "view", key, rest @ ..] if rest.contains(&"--json") => {
            if let Some(ms) = script["view_delay_ms"].as_u64() {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            if let Some(failure) = script["fail"][*key].as_str() {
                eprintln!("{failure}");
                return 1;
            }
            let moved = moved(dir);
            if moved.get(*key).is_some() && listed(&script["unreadable_after_move"], key) {
                eprintln!("Error: work item {key} could not be fetched");
                return 1;
            }
            let made = created(dir);
            let site = script["site"].as_str().unwrap_or("example.atlassian.net");
            let made_item = made.get(*key).map(|made| {
                json!({"key": key, "id": "10099",
                    "self": format!("https://{site}/rest/api/3/issue/10099"),
                    "fields": {"summary": made["summary"], "fixVersions": made["fixVersions"]}})
            });
            match script["items"].get(*key).or(made_item.as_ref()) {
                Some(item) => {
                    let mut item = item.clone();
                    if let Some(status) = moved.get(*key) {
                        item["fields"]["status"]["name"] = status.clone();
                    }
                    if let Some(text) = edited(dir).get(*key).and_then(Value::as_str) {
                        item["fields"]["description"] = json!({"type": "doc", "version": 1,
                            "content": [{"type": "paragraph",
                                "content": [{"type": "text", "text": text}]}]});
                    }
                    add_posted(dir, script, key, &mut item);
                    // As a real site: the links come only when asked, never with the default fields.
                    if asks_links(rest) {
                        let links: Vec<Value> = live_links(dir, script)
                            .iter()
                            .filter_map(|link| link_of(script, link, key))
                            .collect();
                        item["fields"]["issuelinks"] = Value::from(links);
                    }
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
                _ => {
                    let jql = value(rest, "--jql").unwrap_or_default();
                    script["items"]
                        .as_object()
                        .into_iter()
                        .flat_map(|items| items.values().cloned())
                        .filter(|item| found_by(jql, item))
                        .collect()
                }
            };
            // A page at most, unless `--paginate` reads them all.
            if let Some(page) = script["page"].as_u64()
                && !rest.contains(&"--paginate")
            {
                found.truncate(page as usize);
            }
            found.truncate(limit);
            if rest.contains(&"--count") {
                println!("✓ Number of work items in the search: {}", found.len());
                return 0;
            }
            // As the real one does: `key` is no field of `fields`, so asked alone it prints
            // nothing of each work item but `null`.
            if value(rest, "--fields") == Some("key") {
                found = vec![Value::Null; found.len()];
            }
            // The real one prints only the fields asked, and `updated` is never among them
            // (refused): a round learns a change from the search's window, not from a date.
            for item in found.iter_mut().filter(|item| item.is_object()) {
                only_asked(rest, item);
                enveloped(item);
            }
            println!("{}", Value::from(found));
            0
        }
        ["jira", "workitem", "create", rest @ ..]
            if rest.contains(&"--json") && rest.contains(&"--from-json") =>
        {
            let Some(item) = value(rest, "--from-json").and_then(file_text) else {
                return not_run(&words);
            };
            if item["type"].as_str().is_none() {
                eprintln!("✗ Error: no value given for required property type");
                return 1;
            }
            // As measured: the JSON's assignee is an email or an account id, `@me` none of them.
            if let Some(who) = item["assignee"].as_str()
                && let Some(refused) = unknown_user(script, who, false)
            {
                eprintln!("{refused}");
                return 1;
            }
            let project = item["projectKey"].as_str().unwrap_or("X");
            for held in item["additionalAttributes"]["fixVersions"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let name = held["name"].as_str().unwrap_or_default();
                if !versions_of(script, project)
                    .iter()
                    .any(|version| version["name"] == name)
                {
                    eprintln!("✗ Error: Version name '{name}' is not valid");
                    return 1;
                }
            }
            let key = script["created"]
                .as_str()
                .map_or_else(|| format!("{project}-1"), str::to_owned);
            let held = match script["drops_fix_versions"] == true {
                true => json!([]),
                false => item["additionalAttributes"]["fixVersions"].clone(),
            };
            let mut made = created(dir);
            made.insert(
                key.clone(),
                json!({"summary": item["summary"], "fixVersions": held}),
            );
            std::fs::write(dir.join("created.json"), Value::from(made).to_string())
                .expect("the created work items");
            print_created(script, &key);
            0
        }
        ["jira", "workitem", "create", rest @ ..]
            if rest.contains(&"--json") && rest.contains(&"--summary") =>
        {
            let Some(project) = value(rest, "--project") else {
                return not_run(&words);
            };
            if let Some(who) = value(rest, "--assignee")
                && let Some(refused) = unknown_user(script, who, true)
            {
                eprintln!("{refused}");
                return 1;
            }
            if let (Some(kind), Some(types)) = (value(rest, "--type"), types_of(script, project))
                && !types.iter().any(|t| t.0.eq_ignore_ascii_case(kind))
            {
                eprintln!(
                    "✗ Error: the work item type {kind:?} is not valid for project {project}"
                );
                return 1;
            }
            let key = script["created"]
                .as_str()
                .map_or_else(|| format!("{project}-1"), str::to_owned);
            print_created(script, &key);
            0
        }
        ["jira", "project", "view", rest @ ..] if rest.contains(&"--json") => {
            let Some(project) = value(rest, "--key") else {
                return not_run(&words);
            };
            let types = types_of(script, project);
            if types.is_none() && script["versions"][project].is_null() {
                eprintln!(
                    "✗ Error: project {project} does not exist or you do not have permission to see it"
                );
                return 1;
            }
            let types: Vec<Value> = types
                .into_iter()
                .flatten()
                .map(|(name, subtask)| json!({"name": name, "subtask": subtask}))
                .collect();
            let mut view = json!({"key": project, "issueTypes": types});
            if script["no_versions_field"] != true {
                view["versions"] = Value::from(versions_of(script, project));
            }
            println!("{view}");
            0
        }
        ["jira", "workitem", "link", "type", rest @ ..] if rest.contains(&"--json") => {
            // Names only, as a real site answers: the wordings come with each link of a `view`.
            let types: Vec<Value> = link_types(script)
                .iter()
                .map(|kind| json!({"name": kind["name"]}))
                .collect();
            println!("{}", json!({"issueLinkTypes": types}));
            0
        }
        ["jira", "workitem", "link", "create", rest @ ..] if rest.contains(&"--yes") => {
            let (Some(out), Some(inward), Some(kind)) = (
                value(rest, "--out"),
                value(rest, "--in"),
                value(rest, "--type"),
            ) else {
                return not_run(&words);
            };
            for key in [out, inward] {
                if script["items"].get(key).is_none() {
                    eprintln!("✗ Error: work item {key} does not exist");
                    return 1;
                }
            }
            let Some(found) = link_types(script).into_iter().find(|t| {
                [&t["name"], &t["outward"]]
                    .iter()
                    .any(|word| word.as_str().is_some_and(|w| w.eq_ignore_ascii_case(kind)))
            }) else {
                eprintln!("✗ Error: link type {kind:?} not found");
                return 1;
            };
            let mut made = links_file(dir, "linked.json");
            let id = (10000 + made.len()).to_string();
            made.push(json!({"id": id, "type": found["name"], "out": out, "in": inward}));
            std::fs::write(dir.join("linked.json"), Value::from(made).to_string())
                .expect("the links");
            println!(
                "✓ {out} {} {inward}",
                found["name"].as_str().unwrap_or_default()
            );
            0
        }
        ["jira", "workitem", "link", "list", rest @ ..] if rest.contains(&"--json") => {
            let Some(key) = value(rest, "--key") else {
                return not_run(&words);
            };
            // As a real site answers: the outward item's key alone, none when K is that item.
            let entries: Vec<Value> = live_links(dir, script)
                .into_iter()
                .filter(|link| link["out"] == key || link["in"] == key)
                .map(|link| {
                    let outward = (link["out"] != key).then(|| link["out"].clone());
                    json!({"id": link["id"], "outwardIssueKey": outward, "typeName": link["type"]})
                })
                .collect();
            println!("{}", json!({"issueLinks": entries}));
            0
        }
        ["jira", "workitem", "link", "delete", rest @ ..] if rest.contains(&"--yes") => {
            let Some(id) = value(rest, "--id") else {
                return not_run(&words);
            };
            let mut gone = links_file(dir, "unlinked.json");
            let there = live_links(dir, script).iter().any(|link| link["id"] == id);
            if !there {
                eprintln!("✗ Error: link {id} does not exist");
                return 1;
            }
            gone.push(Value::from(id));
            std::fs::write(dir.join("unlinked.json"), Value::from(gone).to_string())
                .expect("the links");
            println!("✓ Link {id} deleted");
            0
        }
        ["jira", "workitem", "comment", "update", rest @ ..]
            if ["--body", "--body-file", "--body-adf"]
                .iter()
                .any(|flag| rest.contains(flag)) =>
        {
            // `--body` and `--body-file` are plain text, `--body-adf` a document, as its page says.
            let body = match (
                value(rest, "--body"),
                value(rest, "--body-file"),
                value(rest, "--body-adf"),
            ) {
                (Some(text), _, _) => Some(Value::from(text)),
                (_, Some(path), _) => std::fs::read_to_string(path).ok().map(Value::from),
                (_, _, Some(path)) => match file_text(path) {
                    Some(doc) if check(&doc).is_ok() => Some(doc),
                    _ => {
                        eprintln!("✗ Error: --body-adf holds no Atlassian Document Format");
                        return 1;
                    }
                },
                _ => None,
            };
            let done = known(script, rest);
            if let (0, Some(key), Some(id), Some(body)) =
                (done, value(rest, "--key"), value(rest, "--id"), body)
                && let Some(at) = posted_at(id)
            {
                change_post(dir, key, at, Some(body));
            }
            done
        }
        ["jira", "workitem", "comment", "delete", rest @ ..] => {
            let done = known(script, rest);
            if let (0, Some(key), Some(id)) = (done, value(rest, "--key"), value(rest, "--id"))
                && let Some(at) = posted_at(id)
            {
                change_post(dir, key, at, None);
            }
            done
        }
        ["jira", "workitem", "comment", "create", rest @ ..]
            if rest.contains(&"--body") || rest.contains(&"--body-file") =>
        {
            let done = known(script, rest);
            let body = match value(rest, "--body") {
                Some(text) => Some(Value::from(text)),
                None => value(rest, "--body-file").and_then(file_text),
            };
            if let (0, Some(key), Some(body)) = (done, value(rest, "--key"), body) {
                post(dir, key, body);
            }
            done
        }
        ["jira", "workitem", "edit", rest @ ..] if rest.contains(&"--yes") => {
            // As the real bulk command does: it ends with 0 and says its failure on its output.
            if let (Some(who), Some(key)) = (value(rest, "--assignee"), value(rest, "--key"))
                && unknown_user(script, who, true).is_some()
            {
                println!("✗ Failure: {key} can't be edited: unexpected error, trace id: 0");
                return 0;
            }
            let done = known(script, rest);
            if let (0, true, Some(key), Some(text)) = (
                done,
                script["links_lose_their_url"] == true,
                value(rest, "--key"),
                value(rest, "--description"),
            ) {
                let mut all = edited(dir);
                all.insert(key.to_owned(), Value::from(without_urls(text)));
                std::fs::write(dir.join("edited.json"), Value::from(all).to_string())
                    .expect("the edits");
            }
            done
        }
        ["jira", "board", "search", rest @ ..] if rest.contains(&"--json") => {
            let Some(project) = value(rest, "--project") else {
                return not_run(&words);
            };
            let boards = script["boards"][project]
                .as_array()
                .cloned()
                .unwrap_or_default();
            // As a real site printed it: Jira's agile REST page.
            println!(
                "{}",
                json!({"isLast": true, "maxResults": 50, "startAt": 0, "total": boards.len(),
                    "values": boards})
            );
            0
        }
        ["jira", "board", "list-sprints", rest @ ..] if rest.contains(&"--json") => {
            let Some(board) = value(rest, "--id") else {
                return not_run(&words);
            };
            let Some(sprints) = script["sprints"][board].as_array() else {
                eprintln!("✗ The board does not support sprints");
                eprintln!("✗ Error: command execution failed");
                return 1;
            };
            let states: Vec<&str> = value(rest, "--state")
                .map(|states| states.split(',').collect())
                .unwrap_or_default();
            let sprints: Vec<&Value> = sprints
                .iter()
                .filter(|s| states.is_empty() || states.iter().any(|st| s["state"] == *st))
                .collect();
            println!(
                "{}",
                json!({"isLast": true, "maxResults": 50, "startAt": 0, "total": sprints.len(),
                    "sprints": sprints})
            );
            0
        }
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

/// The work item types the script gives `project`: each one's name and whether it is a sub-task;
/// none when the script says nothing of the project.
fn types_of(script: &Value, project: &str) -> Option<Vec<(String, bool)>> {
    let types = script["types"][project].as_array()?;
    Some(
        types
            .iter()
            .filter_map(|kind| match kind.as_str() {
                Some(name) => Some((name.to_owned(), false)),
                None => Some((kind["name"].as_str()?.to_owned(), kind["subtask"] == true)),
            })
            .collect(),
    )
}

/// The versions the script gives `project`, as Jira's REST project holds them: an `id`, the
/// `name`, `released`, `archived` and, when it has one, the `releaseDate`.
fn versions_of(script: &Value, project: &str) -> Vec<Value> {
    script["versions"][project]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(at, version)| {
            let mut held = json!({"id": (10_000 + at).to_string(), "released": false,
                "archived": false});
            if let (Value::Object(held), Value::Object(version)) = (&mut held, version) {
                held.extend(version.clone());
            }
            held
        })
        .collect()
}

/// The texts the call passes in files, as the record holds them (see the top of this file).
fn files_of(args: &[String]) -> Vec<Value> {
    use std::os::unix::fs::PermissionsExt;
    let mut files = Vec::new();
    for (at, flag) in args.iter().enumerate() {
        if flag != "--description-file" && flag != "--body-file" && flag != "--from-json" {
            continue;
        }
        let Some(path) = args.get(at + 1) else {
            continue;
        };
        let mode = |path: &Path| {
            std::fs::metadata(path)
                .map(|meta| meta.permissions().mode() & 0o777)
                .unwrap_or_default()
        };
        let mut file = json!({"flag": flag, "path": path, "mode": mode(Path::new(path)),
            "dir_mode": Path::new(path).parent().map(mode)});
        match std::fs::read_to_string(path) {
            // A work item's definition, its description a document as `acli` takes it there.
            Ok(text) if flag == "--from-json" => match serde_json::from_str::<Value>(&text) {
                Ok(item) => match item.get("description").map_or(Ok(()), check) {
                    Ok(()) => file["json"] = item,
                    Err(why) => file["invalid"] = Value::from(why),
                },
                Err(error) => file["invalid"] = Value::from(format!("{path}: {error}")),
            },
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(adf) => match check(&adf) {
                    Ok(()) => file["adf"] = adf,
                    Err(why) => file["invalid"] = Value::from(why),
                },
                Err(_) => file["text"] = Value::from(text),
            },
            Err(error) => file["invalid"] = Value::from(format!("{path}: {error}")),
        }
        files.push(file);
    }
    files
}

/// The text a file holds: its document when it is one, else its words.
fn file_text(path: &str) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or(Value::from(text)))
}

/// Whether `doc` is Atlassian Document Format as Jira holds it: a `doc` of version 1 whose
/// nodes are known, each in a parent that takes it, its marks known too. Not all of ADF: the
/// nodes Flottille writes and Jira's own that it reads.
fn check(doc: &Value) -> Result<(), String> {
    if doc["type"] != "doc" || doc["version"] != 1 {
        return Err("the document is not {\"type\": \"doc\", \"version\": 1}".into());
    }
    let content = doc["content"].as_array().filter(|c| !c.is_empty());
    let content = content.ok_or("a doc holds at least one block")?;
    content.iter().try_for_each(|node| node_in(node, "doc"))
}

/// What a node of each type may hold.
fn children_of(kind: &str) -> Option<&'static [&'static str]> {
    const INLINE: &[&str] = &["text", "hardBreak", "mention", "emoji", "inlineCard"];
    Some(match kind {
        "doc" => &[
            "paragraph",
            "heading",
            "blockquote",
            "codeBlock",
            "bulletList",
            "orderedList",
            "rule",
            "table",
        ],
        "paragraph" | "heading" => INLINE,
        "blockquote" => &["paragraph", "bulletList", "orderedList", "codeBlock"],
        "codeBlock" => &["text"],
        "bulletList" | "orderedList" => &["listItem"],
        "listItem" => &["paragraph", "bulletList", "orderedList", "codeBlock"],
        "table" => &["tableRow"],
        "tableRow" => &["tableHeader", "tableCell"],
        "tableHeader" | "tableCell" => &["paragraph"],
        "rule" => &[],
        _ => return None,
    })
}

fn node_in(node: &Value, parent: &str) -> Result<(), String> {
    let kind = node["type"].as_str().unwrap_or_default();
    if !children_of(parent).is_some_and(|allowed| allowed.contains(&kind)) {
        return Err(format!("{kind:?} is not a node a {parent} takes"));
    }
    match kind {
        "text" => return text_node(node),
        "hardBreak" | "mention" | "emoji" | "inlineCard" => return Ok(()),
        "heading" => match node["attrs"]["level"].as_u64() {
            Some(1..=6) => {}
            _ => return Err("a heading has a level from 1 to 6".into()),
        },
        _ => {}
    }
    let content = node["content"].as_array();
    // These hold at least one child; a paragraph, a heading and a code block may be empty.
    let needs = !matches!(kind, "paragraph" | "heading" | "codeBlock" | "rule");
    if needs && content.is_none_or(|c| c.is_empty()) {
        return Err(format!("a {kind} holds at least one node"));
    }
    if node.get("marks").is_some() {
        return Err(format!("a {kind} has no marks"));
    }
    content
        .into_iter()
        .flatten()
        .try_for_each(|child| node_in(child, kind))
}

fn text_node(node: &Value) -> Result<(), String> {
    if node["text"].as_str().is_none_or(str::is_empty) {
        return Err("a text node is not empty".into());
    }
    let marks: Vec<&str> = node["marks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|mark| mark["type"].as_str().unwrap_or_default())
        .collect();
    for mark in node["marks"].as_array().into_iter().flatten() {
        match mark["type"].as_str().unwrap_or_default() {
            "strong" | "em" | "strike" | "code" | "underline" => {}
            "link" if mark["attrs"]["href"].as_str().is_some() => {}
            other => return Err(format!("the mark {other:?} is not one Jira knows here")),
        }
    }
    if marks.contains(&"code") && marks.iter().any(|m| !matches!(*m, "code" | "link")) {
        return Err("a code mark goes with a link only".into());
    }
    Ok(())
}

/// Where the transitions that took left the work items: `moved.json`, written by the fake.
/// The work items a creation from a JSON definition made: each key's `summary` and `fixVersions`.
fn created(dir: &Path) -> Map<String, Value> {
    std::fs::read_to_string(dir.join("created.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn moved(dir: &Path) -> Map<String, Value> {
    std::fs::read_to_string(dir.join("moved.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The descriptions `edit` changed, by work item: `edited.json`, written by the fake.
fn edited(dir: &Path) -> Map<String, Value> {
    std::fs::read_to_string(dir.join("edited.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// `text` with each Markdown link `[label](url)` reduced to its label.
fn without_urls(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let linked = rest[open..].find("](").and_then(|middle| {
            let close = rest[open + middle..].find(')')?;
            Some((open + middle, open + middle + close))
        });
        match linked {
            Some((middle, close)) => {
                out.push_str(&rest[..open]);
                out.push_str(&rest[open + 1..middle]);
                rest = &rest[close + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// The comments `comment create` added, by work item: `posted.json`, written by the fake.
fn posted(dir: &Path) -> Map<String, Value> {
    std::fs::read_to_string(dir.join("posted.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Keeps a comment created on `key`, its text or document as it was passed.
fn post(dir: &Path, key: &str, body: Value) {
    let mut all = posted(dir);
    let mut comments = all
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    comments.push(body);
    all.insert(key.to_owned(), Value::from(comments));
    std::fs::write(dir.join("posted.json"), Value::from(all).to_string()).expect("the comments");
}

/// The types the script gives for links.
fn link_types(script: &Value) -> Vec<Value> {
    script["link_types"].as_array().cloned().unwrap_or_default()
}

/// A list of links kept in `name` of the fake's folder.
fn links_file(dir: &Path, name: &str) -> Vec<Value> {
    std::fs::read_to_string(dir.join(name))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The links there: the script's and those created, less those deleted.
fn live_links(dir: &Path, script: &Value) -> Vec<Value> {
    let gone = links_file(dir, "unlinked.json");
    script["links"]
        .as_array()
        .into_iter()
        .flatten()
        .cloned()
        .chain(links_file(dir, "linked.json"))
        .filter(|link| !gone.contains(&link["id"]))
        .collect()
}

/// Whether `--fields` asks a view for the links (`issuelinks`, or every field with `*all`).
fn asks_links(rest: &[&str]) -> bool {
    rest.iter()
        .position(|word| *word == "--fields" || *word == "-f")
        .and_then(|at| rest.get(at + 1))
        .is_some_and(|fields| {
            fields
                .split(',')
                .any(|field| matches!(field.trim(), "issuelinks" | "*all"))
        })
}

/// One link of `view`'s `issuelinks` on K, none when K is not on it. As a real site gives it
/// (#520): on the item `--out` named (the one that is blocked) the other is an `inwardIssue`, on
/// the one `--in` named (the one that blocks) an `outwardIssue`.
fn link_of(script: &Value, link: &Value, key: &str) -> Option<Value> {
    let kind = link_types(script)
        .into_iter()
        .find(|t| t["name"] == link["type"])?;
    let issue = |other: &Value| {
        let item = &script["items"][other.as_str().unwrap_or_default()];
        json!({"key": other, "fields": {"summary": item["fields"]["summary"],
            "status": item["fields"]["status"]}})
    };
    let kind = json!({"name": kind["name"], "inward": kind["inward"], "outward": kind["outward"]});
    if link["out"] == key {
        Some(
            json!({"id": link["id"], "type": kind, "inwardIssue": issue(&link["in"]),
            "outwardIssue": null}),
        )
    } else if link["in"] == key {
        Some(json!({"id": link["id"], "type": kind, "inwardIssue": null,
            "outwardIssue": issue(&link["out"])}))
    } else {
        None
    }
}

/// The place in `posted.json` of a comment id handed out by the views (10100, 10101, …).
fn posted_at(id: &str) -> Option<usize> {
    id.parse::<usize>().ok()?.checked_sub(10100)
}

/// Replaces (or, for none, deletes) the comment created at `at` on `key`.
fn change_post(dir: &Path, key: &str, at: usize, body: Option<Value>) {
    let mut all = posted(dir);
    let mut comments = all
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(slot) = comments.get_mut(at) {
        *slot = body.unwrap_or(Value::Null);
    }
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
    for (at, body) in created.into_iter().enumerate() {
        if body.is_null() {
            continue;
        }
        comments.as_array_mut().expect("an array").push(
            json!({"id": (10100 + at).to_string(), "author": {"displayName": user},
                "created": "2026-10-01T12:00:00.000+0000", "body": body}),
        );
    }
}

/// What the real search prints around a work item, as a real site was seen to: Jira's REST
/// issue, its parts not asked for `null`.
fn enveloped(item: &mut Value) {
    let Some(item) = item.as_object_mut() else {
        return;
    };
    let id = item
        .get("self")
        .and_then(Value::as_str)
        .and_then(|link| link.rsplit('/').next())
        .unwrap_or("10000")
        .to_owned();
    item.entry("id").or_insert(id.into());
    item.entry("expand").or_insert(
        "renderedFields,names,schema,operations,editmeta,changelog,versionedRepresentations".into(),
    );
    for part in [
        "changelog",
        "editmeta",
        "fieldsToInclude",
        "names",
        "operations",
        "properties",
        "renderedFields",
        "schema",
        "transitions",
        "versionedRepresentations",
    ] {
        item.entry(part).or_insert(Value::Null);
    }
}

/// With `--fields a,b` among `args`, only those of the work item's fields stay.
fn only_asked(args: &[&str], item: &mut Value) {
    let Some(asked) = value(args, "--fields") else {
        return;
    };
    let asked: Vec<&str> = asked.split(',').map(str::trim).collect();
    // The script's fields are all navigable ones: asked so, they all come.
    if asked.contains(&"*navigable") || asked.contains(&"*all") {
        return;
    }
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

/// What a creation prints: the new work item, `{"key": …}`, or the script's `created_said` as it
/// is, for an answer that names no key.
fn print_created(script: &Value, key: &str) {
    match script["created_said"].as_str() {
        Some(said) => println!("{said}"),
        None => println!("{}", json!({"key": key})),
    }
}

/// Why the site refuses `who` as an assignee, in the real `acli`'s words: not an email nor an
/// account id (`@me` is taken by its flags, `flag`, and not in its JSON), or one of none of the
/// script's `users`. Without `users`, every email and account id is known.
fn unknown_user(script: &Value, who: &str, flag: bool) -> Option<String> {
    if who == "@me" {
        return (!flag).then(|| "✗ Error: User not found for email: @me".to_owned());
    }
    let hex = who.len() == 24 && who.chars().all(|c| c.is_ascii_hexdigit());
    if !who.contains('@') && !hex && !who.contains(':') {
        return Some(
            "✗ Error: invalid format for assignee email: mail: missing '@' or angle-addr".into(),
        );
    }
    let users = script["users"].as_array()?;
    (!users.iter().any(|user| user == who))
        .then(|| format!("✗ Error: User not found for email: {who}"))
}

/// What follows `flag` in `args`.
fn value<'a>(args: &[&'a str], flag: &str) -> Option<&'a str> {
    let at = args.iter().position(|word| *word == flag)?;
    args.get(at + 1).copied()
}

/// A write to the work item of `--key`: done when the script holds it, or it is the one a
/// creation answers (`created`).
fn known(script: &Value, args: &[&str]) -> i32 {
    match value(args, "--key") {
        Some(key) if script["items"].get(key).is_some() || script["created"] == key => {
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
