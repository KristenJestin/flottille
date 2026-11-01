//! The scenarios of the items (`docs/items.md`): every pull request and every ticket of a project,
//! kept up to date by the daemon through a fake `gh`, `bkt` and `acli` on its `PATH`, in a
//! throwaway repository whose `origin` says where its forge is. No test ever calls a forge or a
//! tracker.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A time long past: what the incremental reads never ask again.
const OLD: &str = "2020-01-01T00:00:00Z";
/// A time ahead of any cursor: what changed since the last round.
const NEW: &str = "2999-01-01T00:00:00Z";

/// A user whose daemon keeps the items, with the fake `gh`, `bkt` and `acli`, on a test clock.
fn user() -> User {
    User::new()
        .with_fake_gh()
        .with_fake_bkt()
        .with_fake_acli()
        .with_fetcher()
        .with_env("FLOTTILLE_TEST_CLOCK", "1")
}

/// A project `app` of one repository, `api`, whose `origin` is `url`; nothing declared.
fn project(user: &User, url: &str) {
    let dir = user.path("work/app/api");
    user.git_init(&dir);
    std::fs::write(dir.join("README.md"), "api\n").unwrap();
    user.git(&dir, &["add", "."]);
    user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    user.git(&dir, &["remote", "add", "origin", url]);
    user.call(
        "project.add",
        json!({"folder": user.path("work/app"), "name": "app"}),
    )
    .unwrap();
}

/// Declares `api`'s forge and the project's tracker, as the user does in the project's settings.
fn declare(user: &User, forge: Value, tracker: Value) {
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [{"path": "api", "forge": forge}],
            "tracker": tracker}),
    )
    .unwrap();
}

fn github() -> Value {
    json!({"kind": "github", "repository": "acme/api"})
}

fn jira() -> Value {
    json!({"kind": "jira", "site": "example.atlassian.net", "projects": ["ABC"]})
}

/// What `gh pr list --json` answers of a pull request of `acme/api`.
fn pull(number: u64, title: &str, state: &str, updated: &str) -> Value {
    json!({
        "number": number, "title": title, "state": state, "isDraft": false,
        "headRefName": format!("feature-{number}"), "baseRefName": "main",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "url": format!("https://github.com/acme/api/pull/{number}"),
        "author": {"login": "someone"}, "assignees": [], "labels": [{"name": "backend"}],
        "updatedAt": updated, "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED",
        "statusCheckRollup": [], "comments": [], "reviews": []
    })
}

/// What `gh issue list --json` answers of an issue of `acme/api`.
fn issue(number: u64, title: &str, updated: &str) -> Value {
    json!({
        "number": number, "title": title, "state": "OPEN", "body": "",
        "url": format!("https://github.com/acme/api/issues/{number}"),
        "author": {"login": "other"}, "assignees": [{"login": "someone"}], "labels": [],
        "updatedAt": updated, "comments": []
    })
}

/// What `acli jira workitem search --json` is assumed to print of a work item of ABC.
fn work_item(key: &str, summary: &str, status: &str) -> Value {
    json!({
        "key": key,
        "self": "https://example.atlassian.net/rest/api/3/issue/10001",
        "fields": {
            "summary": summary,
            "status": {"name": status, "statusCategory": {"key": "indeterminate"}},
            "assignee": {"displayName": "Alice"},
            "labels": ["web"],
            "issuetype": {"name": "Story"},
            "priority": {"name": "High"},
            "updated": "2026-10-01T10:00:00.000+0000"
        }
    })
}

/// Each call of a fake, its arguments joined.
fn calls(received: Vec<Value>) -> Vec<String> {
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

fn refresh(user: &User) -> Value {
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap()
}

fn list(user: &User, filters: Value) -> Vec<Value> {
    let mut params = json!({"project": "app"});
    params
        .as_object_mut()
        .unwrap()
        .extend(filters.as_object().unwrap().clone());
    user.call("items.list", params).unwrap()["items"]
        .as_array()
        .unwrap()
        .clone()
}

/// The stored events named `item.*`, the first first.
fn item_events(user: &User) -> Vec<Value> {
    let watch = user.watch(&["--after", "0"]);
    watch
        .within(Duration::from_millis(1500))
        .into_iter()
        .filter(|e| e["name"].as_str().is_some_and(|n| n.starts_with("item.")))
        .collect()
}

/// Waits until `until` holds, for 10 s at most.
fn eventually(what: &str, until: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !until() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn advance(user: &User, ms: u64) {
    user.call("test.advance", json!({"ms": ms})).unwrap();
}

/// How many times the fake `gh` listed the pull requests.
fn pull_lists(user: &User) -> usize {
    calls(user.gh_received())
        .iter()
        .filter(|c| c.starts_with("pr list"))
        .count()
}

#[test]
fn a_project_with_a_declared_github_forge_stores_its_pulls_and_issues_at_its_first_round_and_emits_nothing()
 {
    let user = user();
    project(&user, "git@github.com:acme/api.git");
    user.gh_script(json!({
        "pulls": {"acme/api": {"feature-1": pull(1, "Add the login", "OPEN", OLD),
                                "feature-2": pull(2, "Old work", "MERGED", OLD)}},
        "issues": {"acme/api": {"5": issue(5, "Users cannot sign in", OLD)}}
    }));
    declare(
        &user,
        github(),
        json!({"kind": "github", "repository": "api"}),
    );
    refresh(&user);

    let items = list(&user, json!({}));
    let keys: Vec<(String, String)> = items
        .iter()
        .map(|i| {
            (
                i["kind"].as_str().unwrap().to_owned(),
                i["key"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(keys.len(), 3, "{items:?}");
    for key in [("pull", "1"), ("pull", "2"), ("ticket", "5")] {
        assert!(
            keys.contains(&(key.0.into(), key.1.into())),
            "{key:?}: {items:?}"
        );
    }
    let first = items.iter().find(|i| i["key"] == "1").unwrap();
    assert_eq!(first["source"], "github");
    assert_eq!(first["repository"], "api");
    assert_eq!(first["title"], "Add the login");
    assert_eq!(first["state"], "open");
    assert_eq!(first["author"], "someone");
    assert_eq!(first["labels"], json!(["backend"]));
    assert_eq!(first["updated_at"], OLD);
    assert_eq!(first["details"]["head"], "feature-1");
    let merged = items.iter().find(|i| i["key"] == "2").unwrap();
    assert_eq!(merged["state"], "merged");
    // The first round reads everything, all states, with no search.
    let pulls: Vec<String> = calls(user.gh_received())
        .into_iter()
        .filter(|c| c.starts_with("pr list"))
        .collect();
    assert!(pulls[0].contains("--state all"), "{pulls:?}");
    assert!(!pulls[0].contains("--search"), "{pulls:?}");
    assert_eq!(item_events(&user), Vec::<Value>::new());
}

#[test]
fn the_next_round_asks_only_what_changed_since_the_cursor_and_emits_item_changed() {
    let user = user();
    project(&user, "git@github.com:acme/api.git");
    let script = |title: &str, updated: &str| {
        json!({"pulls": {"acme/api": {
            "feature-1": pull(1, title, "OPEN", updated),
            "feature-2": pull(2, "Untouched", "OPEN", OLD)
        }}})
    };
    user.gh_script(script("Add the login", OLD));
    declare(&user, github(), Value::Null);
    refresh(&user);
    user.gh_script(script("Add the login page", NEW));
    refresh(&user);

    let last = calls(user.gh_received())
        .into_iter()
        .rfind(|c| c.starts_with("pr list"))
        .unwrap();
    assert!(last.contains("--state all"), "{last}");
    assert!(last.contains("--search updated:>="), "{last}");
    let changed: Vec<Value> = item_events(&user);
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0]["name"], "item.changed");
    let data = &changed[0]["data"];
    assert_eq!(data["project"], "app");
    assert_eq!(data["kind"], "pull");
    assert_eq!(data["key"], "1");
    assert_eq!(
        data["changes"]["title"],
        json!({"from": "Add the login", "to": "Add the login page"})
    );
    let stored = user
        .call(
            "items.show",
            json!({"project": "app", "kind": "pull", "id": "1"}),
        )
        .unwrap();
    assert_eq!(stored["title"], "Add the login page");
    assert_eq!(stored["updated_at"], NEW);
}

#[test]
fn checks_that_end_between_two_rounds_without_the_pull_moving_emit_item_checks() {
    let user = user();
    project(&user, "git@github.com:acme/api.git");
    let with_check = |status: &str, conclusion: Value| {
        let mut pull = pull(1, "Add the login", "OPEN", OLD);
        pull["statusCheckRollup"] = json!([{"__typename": "CheckRun", "name": "test",
            "status": status, "conclusion": conclusion,
            "startedAt": "2026-10-01T10:00:00Z", "completedAt": null,
            "detailsUrl": "https://github.com/acme/api/actions/runs/1/job/2"}]);
        json!({"pulls": {"acme/api": {"feature-1": pull}}})
    };
    user.gh_script(with_check("IN_PROGRESS", Value::Null));
    declare(&user, github(), Value::Null);
    refresh(&user);
    let show = |user: &User| {
        user.call(
            "items.show",
            json!({"project": "app", "kind": "pull", "id": "1"}),
        )
        .unwrap()
    };
    assert_eq!(show(&user)["checks"][0]["state"], "pending");
    user.gh_script(with_check("COMPLETED", json!("FAILURE")));
    refresh(&user);

    let events = item_events(&user);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["name"], "item.checks");
    assert_eq!(events[0]["data"]["checks"], "failing");
    assert_eq!(
        events[0]["data"]["ended"],
        json!([{"name": "test", "state": "failing"}])
    );
    // Its checks were read on their own: the pull request itself did not change.
    assert!(
        calls(user.gh_received())
            .iter()
            .any(|c| c.starts_with("pr view 1") && c.contains("statusCheckRollup")),
        "{:?}",
        calls(user.gh_received())
    );
    let checks = &show(&user)["checks"];
    assert_eq!(checks[0]["state"], "failing");
    assert_eq!(
        checks[0]["link"],
        "https://github.com/acme/api/actions/runs/1/job/2"
    );
}

#[test]
fn bitbucket_cloud_passes_q_and_sort_as_params_and_a_new_comment_emits_item_commented() {
    let user = user();
    project(&user, "git@bitbucket.org:acme/api.git");
    let script = |comments: Value, updated: &str| {
        let count = comments.as_array().unwrap().len();
        json!({
            "contexts": [{"name": "cloud", "host": "bitbucket.org", "active": true}],
            "api": {
                "/repositories/acme/api/pullrequests": {"values": [{
                    "id": 3, "title": "Add the login", "state": "OPEN",
                    "author": {"display_name": "Alice"},
                    "source": {"branch": {"name": "login"}, "commit": {"hash": "abcdef1"},
                        "repository": {"full_name": "acme/api"}},
                    "destination": {"branch": {"name": "main"},
                        "repository": {"full_name": "acme/api"}},
                    "links": {"html": {"href": "https://bitbucket.org/acme/api/pull-requests/3"}},
                    "comment_count": count, "updated_on": updated
                }]},
                "/repositories/acme/api/pullrequests/3": {"participants": []},
                "/repositories/acme/api/pullrequests/3/comments": {"values": comments},
                "/repositories/acme/api/pullrequests/3/diffstat": {"values": []}
            },
            "checks": {"acme/api": {"3": []}}
        })
    };
    user.bkt_script(script(json!([]), "2020-01-01T00:00:00+00:00"));
    declare(
        &user,
        json!({"kind": "bitbucket-cloud", "repository": "acme/api"}),
        Value::Null,
    );
    refresh(&user);
    assert_eq!(list(&user, json!({}))[0]["key"], "3");
    user.bkt_script(script(
        json!([{"content": {"raw": "Please rename it"}, "user": {"display_name": "Bob"},
            "deleted": false}]),
        "2999-01-01T00:00:00+00:00",
    ));
    refresh(&user);

    let lists: Vec<String> = calls(user.bkt_received())
        .into_iter()
        .filter(|c| c.starts_with("api /repositories/acme/api/pullrequests --"))
        .collect();
    let last = lists.last().unwrap();
    assert!(last.contains("--param sort=-updated_on"), "{last}");
    assert!(last.contains("--param q=updated_on>=\""), "{last}");
    assert!(!last.contains('?'), "{last}");
    let events = item_events(&user);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["name"], "item.commented");
    assert_eq!(events[0]["data"]["source"], "bitbucket");
    assert_eq!(events[0]["data"]["comments"][0]["author"], "Bob");
    assert_eq!(
        events[0]["data"]["comments"][0]["excerpt"],
        "Please rename it"
    );
}

#[test]
fn jira_asks_what_was_updated_and_a_status_change_emits_item_changed_with_the_status() {
    let user = user();
    project(&user, "git@github.com:acme/api.git");
    user.acli_script(json!({"search": [work_item("ABC-1", "Log in", "To Do")]}));
    user.call(
        "project.set_settings",
        json!({"name": "app", "tracker": jira()}),
    )
    .unwrap();
    refresh(&user);
    let first = list(&user, json!({}));
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0]["source"], "jira");
    assert_eq!(first[0]["state"], "open");
    assert_eq!(first[0]["details"]["status"], "To Do");
    assert_eq!(first[0]["details"]["type"], "Story");
    user.acli_script(json!({"search": [work_item("ABC-1", "Log in", "In Progress")]}));
    refresh(&user);

    let searches: Vec<String> = calls(user.acli_received())
        .into_iter()
        .filter(|c| c.starts_with("jira workitem search"))
        .collect();
    assert!(!searches[0].contains("updated >="), "{searches:?}");
    assert!(
        searches[1].contains("project = \"ABC\" AND updated >= "),
        "{searches:?}"
    );
    let events = item_events(&user);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["name"], "item.changed");
    assert_eq!(events[0]["data"]["key"], "ABC-1");
    assert_eq!(
        events[0]["data"]["changes"]["status"],
        json!({"from": "To Do", "to": "In Progress"})
    );
}

#[test]
fn a_project_with_no_forge_nor_tracker_set_is_never_read() {
    let user = user();
    project(&user, "git@github.com:acme/api.git");
    user.gh_script(json!({"pulls": {"acme/api": {"feature-1": pull(1, "x", "OPEN", OLD)}}}));
    refresh(&user);
    advance(&user, 10 * 60_000);
    std::thread::sleep(Duration::from_millis(1000));

    assert_eq!(calls(user.gh_received()), Vec::<String>::new());
    assert_eq!(calls(user.bkt_received()), Vec::<String>::new());
    assert_eq!(calls(user.acli_received()), Vec::<String>::new());
    assert_eq!(list(&user, json!({})), Vec::<Value>::new());
}

#[test]
fn a_source_signed_out_backs_off_and_says_why_while_the_others_still_update() {
    let user = user();
    project(&user, "git@github.com:acme/api.git");
    user.gh_script(json!({"signed_in": false}));
    user.acli_script(json!({"search": [work_item("ABC-1", "Log in", "To Do")]}));
    declare(&user, github(), jira());
    refresh(&user);

    let items = list(&user, json!({}));
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["key"], "ABC-1");
    let sources = user.call("items.list", json!({"project": "app"})).unwrap()["sources"].clone();
    let github = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["source"] == "github")
        .unwrap();
    assert!(
        github["problem"]
            .as_str()
            .unwrap()
            .contains("gh is not signed in"),
        "{sources}"
    );
    let jira_source = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["source"] == "jira")
        .unwrap();
    assert_eq!(jira_source["problem"], Value::Null, "{sources}");

    // Five minutes on, Jira is read again and GitHub waits twice as long.
    let gh_calls = user.gh_received().len();
    let searches = |user: &User| {
        calls(user.acli_received())
            .iter()
            .filter(|c| c.starts_with("jira workitem search"))
            .count()
    };
    let before = searches(&user);
    advance(&user, 5 * 60_000 + 1000);
    eventually("Jira is read again", || searches(&user) > before);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(user.gh_received().len(), gh_calls);
    advance(&user, 5 * 60_000 + 1000);
    eventually("gh is asked again", || user.gh_received().len() > gh_calls);
}

#[test]
fn items_list_answers_from_the_store_with_seen_at_filtered_by_kind_state_repository_and_mine() {
    let user = user();
    project(&user, "git@github.com:acme/api.git");
    let mut theirs = pull(2, "Their work", "MERGED", OLD);
    theirs["author"] = json!({"login": "other"});
    user.gh_script(json!({
        "user": {"login": "someone"},
        "pulls": {"acme/api": {"feature-1": pull(1, "Mine", "OPEN", OLD), "feature-2": theirs}}
    }));
    user.acli_script(json!({"search": [work_item("ABC-1", "Log in", "To Do")]}));
    declare(&user, github(), jira());
    refresh(&user);

    let keys = |items: Vec<Value>| -> Vec<String> {
        let mut keys: Vec<String> = items
            .iter()
            .map(|i| i["key"].as_str().unwrap().to_owned())
            .collect();
        keys.sort();
        keys
    };
    let all = list(&user, json!({}));
    assert_eq!(keys(all.clone()), ["1", "2", "ABC-1"]);
    assert!(
        all.iter().all(|i| i["seen_at"].as_u64().is_some()),
        "{all:?}"
    );
    assert_eq!(keys(list(&user, json!({"kind": "pull"}))), ["1", "2"]);
    assert_eq!(keys(list(&user, json!({"kind": "ticket"}))), ["ABC-1"]);
    assert_eq!(keys(list(&user, json!({"state": "open"}))), ["1", "ABC-1"]);
    assert_eq!(keys(list(&user, json!({"state": "merged"}))), ["2"]);
    assert_eq!(keys(list(&user, json!({"repository": "api"}))), ["1", "2"]);
    assert_eq!(keys(list(&user, json!({"mine": true}))), ["1"]);
}

#[test]
fn a_connected_window_makes_a_round_every_minute_and_none_every_five_both_from_the_config() {
    let user = user();
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/daemon.toml"),
        "[items]\nwindow_minutes = 2\nminutes = 7\n",
    )
    .unwrap();
    project(&user, "git@github.com:acme/api.git");
    user.gh_script(json!({"pulls": {"acme/api": {"feature-1": pull(1, "x", "OPEN", OLD)}}}));
    declare(&user, github(), Value::Null);
    advance(&user, 0);
    eventually("the first round", || pull_lists(&user) == 1);

    // No window: a round every 7 minutes.
    advance(&user, 2 * 60_000 + 1000);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(pull_lists(&user), 1);
    advance(&user, 5 * 60_000);
    eventually("a round 7 minutes on", || pull_lists(&user) == 2);

    // A window connected: every 2 minutes.
    let mut window = UnixStream::connect(user.path("runtime/flottille/daemon.sock")).unwrap();
    writeln!(
        window,
        "{}",
        json!({"id": "1", "method": "events.watch", "params": {"client": "window"}})
    )
    .unwrap();
    let mut answer = String::new();
    BufReader::new(window.try_clone().unwrap())
        .read_line(&mut answer)
        .unwrap();
    assert!(answer.contains("\"result\""), "{answer}");
    advance(&user, 2 * 60_000 + 1000);
    eventually("a round 2 minutes on", || pull_lists(&user) == 3);
    drop(window);
}

#[test]
fn the_items_of_a_repository_whose_forge_is_unset_go_with_it() {
    let user = user();
    project(&user, "git@github.com:acme/api.git");
    user.gh_script(json!({"pulls": {"acme/api": {"feature-1": pull(1, "x", "OPEN", OLD)}}}));
    declare(&user, github(), Value::Null);
    refresh(&user);
    assert_eq!(list(&user, json!({})).len(), 1);
    declare(&user, Value::Null, Value::Null);
    advance(&user, 0);
    eventually("the items are forgotten", || {
        list(&user, json!({})).is_empty()
    });
}
