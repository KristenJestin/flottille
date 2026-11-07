//! The links of the tickets and the agent's own comments (#365, `docs/tickets.md`, "Links and
//! comments"): the link types of a Jira site, a link made and deleted, the links `show` lists, a
//! sub-task's parent, a comment an agent posted edited or deleted, and a comment it did not post
//! refused. The trackers are the fake `acli` and the fake `gh`; the remotes are throwaway bare
//! repositories; the escalating programs are fakes, first on the daemon's `PATH`, and none runs.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

const SITE: &str = "example.atlassian.net";

fn user() -> User {
    User::with_fake_agent()
        .with_fake_gh()
        .with_fake_bkt()
        .with_fake_acli()
        .with_fake_escalation()
}

/// A project `app` of one repository on GitHub (`acme/app`), its tracker `tracker`, its ready
/// workspace `login`.
fn fixture(user: &User, tracker: Value) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.git(
        &folder,
        &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": tracker}),
    )
    .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    folder
}

fn on_jira(user: &User) {
    fixture(
        user,
        json!({"kind": "jira", "site": SITE, "projects": ["ABC", "DEF"]}),
    );
}

fn on_github(user: &User) {
    fixture(user, json!({"kind": "github", "repository": "."}));
}

fn root_in(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn requests(user: &User) -> Vec<Value> {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .clone()
}

/// A call held for the user: its request's number.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// The user allows request `number`: the text the agent is handed for it.
fn allow(user: &User, number: u64) -> String {
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap().body
}

/// A write: held, then done once the user allows it. What the agent is handed.
fn written(user: &User, mcp: &Mcp, args: Value) -> String {
    let done = allow(user, held(&mcp.call("tickets", args)));
    assert!(done.contains("and Flottille has now done it"), "{done}");
    done
}

/// A call refused before anything is asked of the user, nothing written.
fn refused_at_once(user: &User, mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("tickets", args.clone());
    assert!(answer.error, "{args}: {}", answer.text);
    assert!(answer.text.starts_with("refused:"), "{}", answer.text);
    assert!(requests(user).is_empty(), "{args} was held");
    answer.text
}

/// A read: answered at once, nothing held.
fn read(user: &User, mcp: &Mcp, args: Value) -> String {
    let answer = mcp.call("tickets", args);
    assert!(!answer.error, "{}", answer.text);
    assert!(!answer.text.contains("Waiting for"), "{}", answer.text);
    assert!(requests(user).is_empty());
    answer.text
}

fn words(received: Vec<Value>) -> Vec<String> {
    received
        .iter()
        .map(|call| {
            call["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn acli_calls(user: &User) -> Vec<String> {
    words(user.acli_received())
}

fn gh_calls(user: &User) -> Vec<String> {
    words(user.gh_received())
}

fn work_item(key: &str, summary: &str, status: &str) -> Value {
    json!({"key": key, "self": format!("https://{SITE}/rest/api/3/issue/1"),
        "fields": {"summary": summary, "status": {"name": status}}})
}

fn document(text: &str) -> Value {
    json!({"type": "doc", "version": 1, "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

fn blocks() -> Value {
    json!({"id": "1", "name": "Blocks", "inward": "is blocked by", "outward": "blocks"})
}

fn relates() -> Value {
    json!({"id": "2", "name": "Relates", "inward": "relates to", "outward": "relates to"})
}

/// ABC-1 to ABC-3, the link types of a default Jira site, and `more` of the script.
fn jira_script(user: &User, more: Value) {
    let mut script = json!({
        "items": {
            "ABC-1": work_item("ABC-1", "The login fails", "To Do"),
            "ABC-2": work_item("ABC-2", "Add search", "In Progress"),
            "ABC-3": work_item("ABC-3", "Shipped", "Done"),
        },
        "link_types": [blocks(), relates(),
            {"id": "3", "name": "Duplicate", "inward": "is duplicated by", "outward": "duplicates"}],
    });
    for (key, value) in more.as_object().unwrap() {
        script[key] = value.clone();
    }
    user.acli_script(script);
}

#[test]
fn the_link_types_of_the_site_are_listed() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("acli/script.json")).unwrap())
            .unwrap();
    script["link_types"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "4", "name": "Escalates", "inward": "is escalated by", "outward": "escalates"}));
    user.acli_script(script);

    // The site names its types only: Jira's own wordings are known for its standard ones.
    let listed = read(&user, &mcp, json!({"action": "link_types"}));

    for said in [
        "blocks (is blocked by) [Blocks]",
        "- relates to [Relates]",
        "duplicates (is duplicated by) [Duplicate]",
        "- Escalates\n",
    ] {
        assert!(listed.contains(said), "{said} missing: {listed}");
    }
    assert!(
        listed.contains("ABC-2 blocks ABC-1"),
        "the direction is not said: {listed}"
    );
    assert_eq!(
        acli_calls(&user),
        ["jira auth status", "jira workitem link type --json"]
    );
}

#[test]
fn two_jira_items_are_linked_and_unlinked() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    let made = written(
        &user,
        &mcp,
        json!({"action": "link", "ticket": "ABC-2", "kind": "blocks", "other": "ABC-1"}),
    );

    assert!(made.contains("ABC-2 blocks ABC-1"), "{made}");
    assert!(
        acli_calls(&user).contains(
            &"jira workitem link create --out ABC-1 --in ABC-2 --type Blocks --yes".to_owned()
        ),
        "{:?}",
        acli_calls(&user)
    );
    // The link is on the work item now, seen from either end.
    let shown = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-1"}));
    assert!(
        shown.contains("- is blocked by ABC-2 (In Progress): Add search"),
        "{shown}"
    );

    let removed = written(
        &user,
        &mcp,
        json!({"action": "unlink", "ticket": "ABC-2", "kind": "blocks", "other": "ABC-1"}),
    );

    assert!(removed.contains("ABC-2 blocks ABC-1"), "{removed}");
    assert!(
        removed.contains("was removed") || removed.contains("is removed"),
        "{removed}"
    );
    let calls = acli_calls(&user);
    assert!(
        calls.contains(&"jira workitem view ABC-2 --json --fields issuelinks".to_owned()),
        "{calls:?}"
    );
    assert!(
        calls.contains(&"jira workitem link delete --id 10000 --yes".to_owned()),
        "{calls:?}"
    );
    let shown = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-1"}));
    assert!(!shown.contains("ABC-2"), "{shown}");
}

#[test]
fn the_inward_wording_swaps_the_two_items() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    let made = written(
        &user,
        &mcp,
        json!({"action": "link", "ticket": "ABC-1", "kind": "is blocked by", "other": "ABC-2"}),
    );

    assert!(made.contains("ABC-1 is blocked by ABC-2"), "{made}");
    assert!(
        acli_calls(&user).contains(
            &"jira workitem link create --out ABC-1 --in ABC-2 --type Blocks --yes".to_owned()
        ),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn unlinking_a_link_that_is_not_there_is_refused_with_the_links_of_the_item() {
    let user = user();
    on_jira(&user);
    jira_script(
        &user,
        json!({"links": [{"id": "500", "type": "Relates", "out": "ABC-1", "in": "ABC-3"}]}),
    );
    let mcp = root_in(&user);

    let said = allow(
        &user,
        held(&mcp.call(
            "tickets",
            json!({"action": "unlink", "ticket": "ABC-1", "kind": "blocks", "other": "ABC-3"}),
        )),
    );

    assert!(said.contains("refused:"), "{said}");
    assert!(said.contains("ABC-1 has no link"), "{said}");
    assert!(said.contains("relates to ABC-3"), "{said}");
    assert!(!acli_calls(&user).iter().any(|c| c.contains("link delete")));
}

#[test]
fn show_lists_an_item_s_links() {
    let user = user();
    on_jira(&user);
    jira_script(
        &user,
        json!({"links": [
            {"id": "500", "type": "Blocks", "out": "ABC-1", "in": "ABC-2"},
            {"id": "501", "type": "Relates", "out": "ABC-1", "in": "ABC-3"},
        ]}),
    );
    let mcp = root_in(&user);

    let shown = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-1"}));

    assert!(shown.contains("Links (2):"), "{shown}");
    assert!(
        shown.contains("- is blocked by ABC-2 (In Progress): Add search"),
        "{shown}"
    );
    assert!(
        shown.contains("- relates to ABC-3 (Done): Shipped"),
        "{shown}"
    );
    // `link list` names no other item when this one is the outward one: `view` gives both ends,
    // asked with the ticket's own fields so that one read serves both.
    assert!(
        acli_calls(&user).contains(
            &"jira workitem view ABC-1 --json --fields summary,status,description,comment,issuelinks,attachment,assignee,parent,*navigable"
                .to_owned()
        ),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn a_work_item_blocked_by_one_and_blocking_another_says_both_rightly() {
    let user = user();
    on_jira(&user);
    // As acli makes them, `--out X --in Y` being `Y blocks X` on a real site: ABC-1 blocks ABC-2,
    // which blocks ABC-3.
    jira_script(
        &user,
        json!({"links": [
            {"id": "500", "type": "Blocks", "out": "ABC-2", "in": "ABC-1"},
            {"id": "501", "type": "Blocks", "out": "ABC-3", "in": "ABC-2"},
        ]}),
    );
    let mcp = root_in(&user);

    let shown = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-2"}));

    assert!(
        shown.contains("- is blocked by ABC-1 (To Do): The login fails"),
        "{shown}"
    );
    assert!(shown.contains("- blocks ABC-3 (Done): Shipped"), "{shown}");
    let first = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-1"}));
    assert!(
        first.contains("- blocks ABC-2 (In Progress): Add search"),
        "{first}"
    );
    let last = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-3"}));
    assert!(
        last.contains("- is blocked by ABC-2 (In Progress): Add search"),
        "{last}"
    );
}

#[test]
fn linking_with_is_blocked_by_puts_the_other_item_on_the_blocking_end() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    let made = written(
        &user,
        &mcp,
        json!({"action": "link", "ticket": "ABC-2", "kind": "is blocked by", "other": "ABC-1"}),
    );

    assert!(made.contains("ABC-2 is blocked by ABC-1"), "{made}");
    assert!(made.contains("ABC-1 blocks ABC-2"), "{made}");
    // On a real site `--out A --in B --type Blocks` made B block A, whatever acli printed.
    assert!(
        acli_calls(&user).contains(
            &"jira workitem link create --out ABC-2 --in ABC-1 --type Blocks --yes".to_owned()
        ),
        "{:?}",
        acli_calls(&user)
    );
    let blocked = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-2"}));
    assert!(
        blocked.contains("- is blocked by ABC-1 (To Do): The login fails"),
        "{blocked}"
    );
    let blocking = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-1"}));
    assert!(
        blocking.contains("- blocks ABC-2 (In Progress): Add search"),
        "{blocking}"
    );
}

#[test]
fn a_cached_show_still_shows_the_links() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);
    written(
        &user,
        &mcp,
        json!({"action": "link", "ticket": "ABC-2", "kind": "blocks", "other": "ABC-1"}),
    );
    let before = acli_calls(&user).len();

    let first = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-1"}));
    let after_first = acli_calls(&user).len();
    let second = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-1"}));

    // One read for the ticket and its links; the second show runs no command line.
    assert_eq!(acli_calls(&user).len(), after_first);
    let reads: Vec<String> = acli_calls(&user)
        .split_off(before)
        .into_iter()
        .filter(|call| call.starts_with("jira workitem view ABC-1"))
        .collect();
    assert_eq!(reads.len(), 1, "{reads:?}");
    assert!(reads[0].contains("issuelinks"), "{reads:?}");
    assert!(!first.contains("as read"), "{first}");
    assert!(second.contains("as read"), "{second}");
    for shown in [&first, &second] {
        assert!(
            shown.contains("- is blocked by ABC-2 (In Progress): Add search"),
            "{shown}"
        );
    }
}

#[test]
fn a_link_made_one_way_reads_the_same_from_both_items() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    written(
        &user,
        &mcp,
        json!({"action": "link", "ticket": "ABC-2", "kind": "blocks", "other": "ABC-1"}),
    );

    let from_two = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-2"}));
    assert!(
        from_two.contains("- blocks ABC-1 (To Do): The login fails"),
        "{from_two}"
    );
    let from_one = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-1"}));
    assert!(
        from_one.contains("- is blocked by ABC-2 (In Progress): Add search"),
        "{from_one}"
    );
    // Removed as the other end reads it.
    let removed = written(
        &user,
        &mcp,
        json!({"action": "unlink", "ticket": "ABC-1", "kind": "is blocked by", "other": "ABC-2"}),
    );
    assert!(removed.contains("The link was removed"), "{removed}");
    assert!(
        acli_calls(&user).contains(&"jira workitem link delete --id 10000 --yes".to_owned()),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn a_link_with_an_unknown_kind_is_refused_naming_the_known_ones() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "link", "ticket": "ABC-2", "kind": "devours", "other": "ABC-1"}),
    );

    assert!(said.contains("\"devours\" is not a link type of"), "{said}");
    for known in ["blocks", "is blocked by", "relates to", "duplicates"] {
        assert!(said.contains(known), "{known} missing: {said}");
    }
    assert!(said.contains("nothing was written"), "{said}");
    assert!(!acli_calls(&user).iter().any(|c| c.contains("link create")));
}

#[test]
fn the_card_of_a_link_says_its_direction_in_words() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    held(&mcp.call(
        "tickets",
        json!({"action": "link", "ticket": "ABC-2", "kind": "blocks", "other": "ABC-1"}),
    ));
    held(&mcp.call(
        "tickets",
        json!({"action": "unlink", "ticket": "ABC-2", "kind": "blocks", "other": "ABC-1"}),
    ));

    let card = |at: usize| requests(&user)[at].to_string();
    assert!(card(0).contains("ABC-2 blocks ABC-1"), "{}", card(0));
    assert!(card(1).contains("ABC-2 blocks ABC-1"), "{}", card(1));
    assert!(
        acli_calls(&user)
            .iter()
            .all(|c| !c.contains("link create") && !c.contains("link delete")),
        "a write ran before the user allowed it"
    );
}

#[test]
fn a_link_names_both_items_and_its_kind() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    for (args, missing) in [
        (
            json!({"action": "link", "kind": "blocks", "other": "ABC-1"}),
            "ticket",
        ),
        (
            json!({"action": "link", "ticket": "ABC-2", "other": "ABC-1"}),
            "kind",
        ),
        (
            json!({"action": "unlink", "ticket": "ABC-2", "kind": "blocks"}),
            "other",
        ),
        (
            json!({"action": "link", "ticket": "ABC-2", "kind": "blocks", "other": "ABC-2"}),
            "itself",
        ),
        (
            json!({"action": "link", "ticket": "ABC-2", "kind": "blocks", "other": "#4"}),
            "GitHub issue",
        ),
    ] {
        let said = refused_at_once(&user, &mcp, args.clone());
        assert!(said.contains(missing), "{args}: {said}");
    }
}

#[test]
fn a_sub_task_gets_its_parent() {
    let user = user();
    on_github(&user);
    user.gh_script(json!({"issues": {"acme/app": {
        "1": {"number": 1, "title": "Epic", "state": "OPEN", "body": "", "comments": [], "id": 9001},
        "2": {"number": 2, "title": "Step", "state": "OPEN", "body": "", "comments": [], "id": 9002},
    }}}));
    let mcp = root_in(&user);

    let made = written(
        &user,
        &mcp,
        json!({"action": "link", "ticket": "#2", "kind": "sub-task of", "other": "#1"}),
    );

    assert!(made.contains("#2 is a sub-task of #1"), "{made}");
    let calls = gh_calls(&user);
    assert!(
        calls.contains(&"api repos/acme/app/issues/2".to_owned()),
        "the id of the sub-issue is read first: {calls:?}"
    );
    assert!(
        calls.contains(
            &"api repos/acme/app/issues/1/sub_issues --method POST -F sub_issue_id=9002".to_owned()
        ),
        "{calls:?}"
    );

    let removed = written(
        &user,
        &mcp,
        json!({"action": "unlink", "ticket": "#1", "kind": "parent of", "other": "#2"}),
    );

    assert!(removed.contains("#1 is the parent of #2"), "{removed}");
    assert!(
        gh_calls(&user).contains(
            &"api repos/acme/app/issues/1/sub_issue --method DELETE -F sub_issue_id=9002"
                .to_owned()
        ),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn a_github_link_the_api_has_none_of_is_refused_naming_why() {
    let user = user();
    on_github(&user);
    user.gh_script(json!({"issues": {"acme/app": {}}}));
    let mcp = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "link", "ticket": "#2", "kind": "relates to", "other": "#1"}),
    );

    assert!(
        said.contains("\"relates to\" is not a link of GitHub issues"),
        "{said}"
    );
    assert!(
        said.contains("sub-task of") && said.contains("blocked by"),
        "{said}"
    );
    let types = read(&user, &mcp, json!({"action": "link_types"}));
    assert!(
        types.contains("sub-task of") && types.contains("parent of"),
        "{types}"
    );
}

#[test]
fn a_jira_parent_is_refused_naming_what_acli_lacks() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "link", "ticket": "ABC-2", "kind": "sub-task of", "other": "ABC-1"}),
    );

    assert!(said.contains("parent"), "{said}");
    assert!(said.contains("workitem edit"), "{said}");
    assert!(said.contains("nothing was written"), "{said}");
}

/// A work item whose comments are `comments` (Jira's shape).
fn commented(key: &str, comments: Value) -> Value {
    let mut item = work_item(key, "The login fails", "To Do");
    item["fields"]["comment"] = json!({"comments": comments});
    item
}

fn person(id: &str, text: &str) -> Value {
    json!({"id": id, "author": {"displayName": "Ana"}, "created": "2026-10-01T09:00:00.000+0000",
        "body": document(text)})
}

#[test]
fn an_agent_edits_its_own_comment() {
    let user = user();
    on_jira(&user);
    jira_script(
        &user,
        json!({"items": {"ABC-1": commented("ABC-1", json!([person("20000", "I can reproduce")]))}}),
    );
    let mcp = root_in(&user);
    written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1", "body": "Fixed on app/login"}),
    );
    let shown = read(
        &user,
        &mcp,
        json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
    );
    // The agent finds the comment by the id the read gives it, and only its own has one shown.
    let posted = shown
        .lines()
        .find(|line| line.contains("Fixed on app/login"))
        .unwrap();
    assert!(posted.contains("[comment 10100]"), "{shown}");
    assert!(!shown.contains("[comment 20000]"), "{shown}");

    let edited = written(
        &user,
        &mcp,
        json!({"action": "comment_edit", "ticket": "ABC-1", "comment": "10100",
            "body": "Fixed on app/login, rebased"}),
    );

    assert!(
        edited.contains("The comment 10100 of ABC-1 was changed"),
        "{edited}"
    );
    assert!(
        acli_calls(&user)
            .iter()
            .any(|c| c
                .starts_with("jira workitem comment update --key ABC-1 --id 10100 --body-adf ")),
        "{:?}",
        acli_calls(&user)
    );
    // What the file held: a document, which `--body-adf` takes and `--body-file` would post as
    // its raw text.
    let posted: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("acli/posted.json")).unwrap())
            .unwrap();
    let held = &posted["ABC-1"][0];
    assert_eq!(held["type"], "doc", "{held}");
    assert!(
        held.to_string().contains("\"Fixed on app/login, rebased\""),
        "{held}"
    );
    // Still the agent's after the edit, and the record follows the new words.
    let shown = read(
        &user,
        &mcp,
        json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
    );
    let posted = shown
        .lines()
        .find(|line| line.contains("rebased"))
        .unwrap_or_else(|| panic!("{shown}"));
    assert!(posted.contains("[comment 10100]"), "{shown}");
    assert!(posted.contains("by an agent"), "{shown}");
}

#[test]
fn an_agent_edits_its_own_github_comment_and_deletes_it() {
    let user = user();
    on_github(&user);
    user.gh_script(json!({"issues": {"acme/app": {"1": {
        "number": 1, "title": "The login fails", "state": "OPEN", "body": "", "comments": [
            {"author": {"login": "alice"}, "createdAt": "2026-10-01T10:00:00Z",
             "body": "I can reproduce", "url": "https://github.com/acme/app/issues/1#issuecomment-500"}
        ]}}}, "commented": 501}));
    let mcp = root_in(&user);
    written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "#1", "body": "Fixed on app/login"}),
    );
    // What GitHub holds once it took the comment: the id its link says.
    user.gh_script(json!({"issues": {"acme/app": {"1": {
        "number": 1, "title": "The login fails", "state": "OPEN", "body": "", "comments": [
            {"author": {"login": "alice"}, "createdAt": "2026-10-01T10:00:00Z",
             "body": "I can reproduce", "url": "https://github.com/acme/app/issues/1#issuecomment-500"},
            {"author": {"login": "someone"}, "createdAt": "2099-10-01T10:00:00Z",
             "body": "Fixed on app/login", "url": "https://github.com/acme/app/issues/1#issuecomment-501"}
        ]}}}}));

    let edited = written(
        &user,
        &mcp,
        json!({"action": "comment_edit", "ticket": "#1", "comment": "issuecomment-501",
            "body": "Fixed on app/login, rebased"}),
    );
    let deleted = written(
        &user,
        &mcp,
        json!({"action": "comment_delete", "ticket": "#1", "comment": "issuecomment-501"}),
    );

    assert!(
        edited.contains("The comment issuecomment-501 of #1 was changed"),
        "{edited}"
    );
    assert!(
        deleted.contains("The comment issuecomment-501 of #1 was deleted"),
        "{deleted}"
    );
    let calls = gh_calls(&user);
    assert!(
        calls.contains(
            &"api repos/acme/app/issues/comments/501 --method PATCH -f body=Fixed on app/login, \
              rebased"
                .to_owned()
        ),
        "{calls:?}"
    );
    assert!(
        calls.contains(&"api repos/acme/app/issues/comments/501 --method DELETE".to_owned()),
        "{calls:?}"
    );
}

#[test]
fn a_comment_it_did_not_write_is_refused() {
    let user = user();
    on_jira(&user);
    jira_script(
        &user,
        json!({"items": {"ABC-1": commented("ABC-1", json!([person("20000", "I can reproduce")]))}}),
    );
    let mcp = root_in(&user);

    for args in [
        json!({"action": "comment_edit", "ticket": "ABC-1", "comment": "20000", "body": "Mine now"}),
        json!({"action": "comment_delete", "ticket": "ABC-1", "comment": "20000"}),
    ] {
        // Refused before the user is asked: nothing is held for a write that cannot be done.
        let said = refused_at_once(&user, &mcp, args.clone());
        assert!(
            said.contains(
                "refused: the comment 20000 of ABC-1 was not posted by an agent of Flottille"
            ),
            "{args}: {said}"
        );
        assert!(said.contains("nothing was written"), "{said}");
    }
    let unknown = refused_at_once(
        &user,
        &mcp,
        json!({"action": "comment_delete", "ticket": "ABC-1", "comment": "99"}),
    );
    assert!(unknown.contains("ABC-1 has no comment 99"), "{unknown}");
    assert!(
        !acli_calls(&user)
            .iter()
            .any(|c| c.contains("comment update") || c.contains("comment delete")),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn an_agent_deletes_its_own_comment() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);
    written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1", "body": "Wrong ticket"}),
    );

    let deleted = written(
        &user,
        &mcp,
        json!({"action": "comment_delete", "ticket": "ABC-1", "comment": "10100"}),
    );

    assert!(
        deleted.contains("The comment 10100 of ABC-1 was deleted"),
        "{deleted}"
    );
    assert!(
        acli_calls(&user)
            .contains(&"jira workitem comment delete --key ABC-1 --id 10100".to_owned()),
        "{:?}",
        acli_calls(&user)
    );
    let shown = read(
        &user,
        &mcp,
        json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
    );
    assert!(!shown.contains("Wrong ticket"), "{shown}");
}

#[test]
fn a_comment_write_names_the_ticket_the_comment_and_its_words() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);

    for (args, missing) in [
        (
            json!({"action": "comment_edit", "ticket": "ABC-1", "body": "x"}),
            "comment",
        ),
        (
            json!({"action": "comment_edit", "ticket": "ABC-1", "comment": "1"}),
            "body",
        ),
        (
            json!({"action": "comment_delete", "comment": "1"}),
            "ticket",
        ),
    ] {
        let said = refused_at_once(&user, &mcp, args.clone());
        assert!(said.contains(missing), "{args}: {said}");
    }
}

/// The comment records of ABC-1: their ids.
fn comment_records(user: &User, key: &str) -> Vec<Option<String>> {
    store(user)
        .posts("app", "jira", "", key)
        .unwrap()
        .into_iter()
        .filter(|post| post.kind == "comment")
        .map(|post| post.comment_id)
        .collect()
}

/// Adds a person's comments to a work item of the script, after what the fake posted.
fn people_write(user: &User, key: &str, comments: Value) {
    let mut script: Value =
        serde_json::from_str(&std::fs::read_to_string(user.path("acli/script.json")).unwrap())
            .unwrap();
    let item = &mut script["items"][key];
    let summary = item["fields"]["summary"].as_str().unwrap().to_owned();
    *item = commented(key, comments);
    item["fields"]["summary"] = summary.into();
    user.acli_script(script);
}

/// Long after the agent posted: the time does not tell a person's same words apart.
fn later_person(id: &str, text: &str) -> Value {
    json!({"id": id, "author": {"displayName": "Ana"}, "created": "2099-10-01T09:00:00.000+0000",
        "body": document(text)})
}

#[test]
fn a_person_s_comment_with_an_agent_s_words_is_not_the_agent_s() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);
    // The agent posts "On it" and edits it; a person then writes "On it".
    written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1", "body": "On it"}),
    );
    written(
        &user,
        &mcp,
        json!({"action": "comment_edit", "ticket": "ABC-1", "comment": "10100", "body": "Fixed"}),
    );
    people_write(&user, "ABC-1", json!([later_person("20001", "On it")]));
    // The agent posts "Thanks!" on ABC-2, the user deletes it in Jira, someone writes "Thanks!".
    written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-2", "body": "Thanks!"}),
    );
    std::fs::write(
        user.path("acli/posted.json"),
        json!({"ABC-1": [document("Fixed")], "ABC-2": [null]}).to_string(),
    )
    .unwrap();
    people_write(&user, "ABC-2", json!([later_person("20002", "Thanks!")]));

    for (key, id, text) in [("ABC-1", "20001", "On it"), ("ABC-2", "20002", "Thanks!")] {
        let shown = read(
            &user,
            &mcp,
            json!({"action": "show", "ticket": key, "detail": "full"}),
        );
        let line = shown
            .lines()
            .find(|line| line.ends_with(&format!(": {text}")))
            .unwrap_or_else(|| panic!("{shown}"));
        assert!(!line.contains("[comment"), "{shown}");
        assert!(!line.contains("by an agent"), "{shown}");
        let said = refused_at_once(
            &user,
            &mcp,
            json!({"action": "comment_delete", "ticket": key, "comment": id}),
        );
        assert!(said.contains("was not posted by an agent"), "{said}");
    }
    assert!(
        !acli_calls(&user)
            .iter()
            .any(|c| c.contains("comment delete")),
        "{:?}",
        acli_calls(&user)
    );
}

#[test]
fn a_deleted_comment_s_record_is_dropped() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);
    written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1", "body": "Thanks!"}),
    );
    assert_eq!(comment_records(&user, "ABC-1"), [Some("10100".to_owned())]);

    written(
        &user,
        &mcp,
        json!({"action": "comment_delete", "ticket": "ABC-1", "comment": "10100"}),
    );

    assert!(comment_records(&user, "ABC-1").is_empty());
    // Someone's later "Thanks!" is theirs.
    people_write(&user, "ABC-1", json!([later_person("20001", "Thanks!")]));
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "comment_delete", "ticket": "ABC-1", "comment": "20001"}),
    );
    assert!(said.contains("was not posted by an agent"), "{said}");
}

#[test]
fn an_edited_comment_keeps_one_record() {
    let user = user();
    on_jira(&user);
    jira_script(&user, json!({}));
    let mcp = root_in(&user);
    written(
        &user,
        &mcp,
        json!({"action": "comment", "ticket": "ABC-1", "body": "On it"}),
    );

    for body in ["Fixed", "Fixed, rebased"] {
        written(
            &user,
            &mcp,
            json!({"action": "comment_edit", "ticket": "ABC-1", "comment": "10100", "body": body}),
        );
    }

    assert_eq!(comment_records(&user, "ABC-1"), [Some("10100".to_owned())]);
    let shown = read(
        &user,
        &mcp,
        json!({"action": "show", "ticket": "ABC-1", "detail": "full"}),
    );
    let line = shown
        .lines()
        .find(|line| line.ends_with(": Fixed, rebased"))
        .unwrap_or_else(|| panic!("{shown}"));
    assert!(line.contains("[comment 10100]"), "{shown}");
}

#[test]
fn a_call_prepared_while_its_agent_is_replaced_is_refused() {
    let user = user();
    on_github(&user);
    // The root dies once the test creates `die-now`; its replacement plays nothing.
    user.fake_script(json!({"turns": [
        {"when": "hello", "times": 1, "do": [{"hold": "die-now"}, {"die": 3}]}
    ]}));
    user.gh_script(json!({"issues": {"acme/app": {"1": {
        "number": 1, "title": "The login fails", "state": "OPEN", "body": "", "comments": []
    }}}, "commented": 501}));
    let opened = user.received_requests("session/new").len();
    let id = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (url, authorization) = server_of(&user, opened);
    // In Full, the edit is performed as soon as it is prepared: no judge, no card.
    let mode = user.run(&["agent", "mode", &id, "full"]);
    assert!(mode.status.success(), "{mode:?}");
    let mcp = Mcp::initialize(&url, &authorization);
    let posted = mcp.call(
        "tickets",
        json!({"action": "comment", "ticket": "#1", "body": "Fixed on app/login"}),
    );
    assert!(!posted.error, "{}", posted.text);
    // Preparing the edit reads the issue, which takes seconds.
    user.gh_script(json!({"issues": {"acme/app": {"1": {
        "number": 1, "title": "The login fails", "state": "OPEN", "body": "", "comments": [
            {"author": {"login": "someone"}, "createdAt": "2099-10-01T10:00:00Z",
             "body": "Fixed on app/login", "url": "https://github.com/acme/app/issues/1#issuecomment-501"}
        ]}}}, "read_delay_ms": 4000}));
    let before = gh_calls(&user).len();
    let call = std::thread::spawn(move || {
        Mcp::initialize(&url, &authorization).call(
            "tickets",
            json!({"action": "comment_edit", "ticket": "#1", "comment": "issuecomment-501",
                "body": "the old session's"}),
        )
    });
    eventually("the edit being prepared", || {
        gh_calls(&user)[before..]
            .iter()
            .any(|call| call.starts_with("issue view"))
    });

    std::fs::write(user.path("fake/die-now"), "").unwrap();
    eventually("the replacement", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["epoch"] == 1
    });
    assert!(
        !call.is_finished(),
        "the edit was prepared before the replacement"
    );

    let answer = call.join().unwrap();
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("this session has been replaced"),
        "{}",
        answer.text
    );
    let calls = gh_calls(&user);
    assert!(
        !calls.iter().any(|call| call.contains("--method PATCH")),
        "{calls:?}"
    );
}
