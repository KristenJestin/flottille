//! A Jira tracker's versions as the tickets tool's milestones (#764, `docs/tickets.md`,
//! "Versions"): a project's versions listed, a work item created in one, the list and the reading
//! of a work item that say it, and what `acli` cannot do refused. The tracker is the fake `acli`
//! (`crates/flottille-fake-acli`); the remotes are throwaway bare repositories; the escalating
//! programs are fakes, first on the daemon's `PATH`, and none runs.

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

/// A project `app` of one repository on GitHub (`acme/app`), its tracker `tracker`.
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
    folder
}

fn on_jira(user: &User) {
    on_projects(user, &["ABC"]);
}

fn on_projects(user: &User, projects: &[&str]) {
    fixture(
        user,
        json!({"kind": "jira", "site": SITE, "projects": projects}),
    );
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

/// The versions of ABC the tests share: one released, one archived, one coming, one without a date.
fn versions() -> Value {
    json!({"ABC": [
        {"name": "1.0", "released": true, "releaseDate": "2026-10-01"},
        {"name": "1.1", "releaseDate": "2026-12-01"},
        {"name": "0.9", "released": true, "archived": true, "releaseDate": "2026-06-01"},
        {"name": "2.0"}
    ]})
}

/// What `acli jira workitem view` prints of a work item in `versions`.
fn work_item(key: &str, summary: &str, versions: &[&str]) -> Value {
    json!({
        "key": key,
        "id": "10001",
        "self": "https://example.atlassian.net/rest/api/3/issue/10001",
        "fields": {
            "summary": summary,
            "status": {"name": "To Do", "statusCategory": {"key": "new"}},
            "fixVersions": versions.iter().map(|name| json!({"name": name})).collect::<Vec<_>>()
        }
    })
}

/// The words of every call of the fake `acli`.
fn acli_calls(user: &User) -> Vec<String> {
    user.acli_received()
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

/// The calls of `acli` that write.
fn acli_writes(user: &User) -> Vec<String> {
    acli_calls(user)
        .into_iter()
        .filter(|call| {
            [
                "workitem create",
                "workitem edit",
                "workitem comment",
                "workitem transition",
            ]
            .iter()
            .any(|write| call.contains(write))
        })
        .collect()
}

#[test]
fn milestones_lists_a_jira_project_s_versions() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions()}));
    let mcp = root_in(&user);

    // Open is unreleased, as GitHub's open milestones are those not yet closed.
    let open = read(&user, &mcp, json!({"action": "milestones"}));
    assert!(
        open.contains("1.1") && open.contains("due 2026-12-01"),
        "{open}"
    );
    assert!(open.contains("2.0"), "{open}");
    assert!(!open.contains("1.0") && !open.contains("0.9"), "{open}");
    let closed = read(
        &user,
        &mcp,
        json!({"action": "milestones", "state": "closed"}),
    );
    assert!(
        closed.contains("- 1.0 closed, released 2026-10-01"),
        "{closed}"
    );
    assert!(
        closed.contains("- 0.9 closed, released 2026-06-01, archived"),
        "{closed}"
    );
    assert!(!closed.contains("1.1"), "{closed}");
    let all = read(&user, &mcp, json!({"action": "milestones", "state": "all"}));
    for name in ["1.0", "1.1", "0.9", "2.0"] {
        assert!(all.contains(&format!("- {name} ")), "{name}: {all}");
    }
    assert!(all.contains("ABC") && all.contains(SITE), "{all}");

    // A read: through the user's own acli, and nothing written.
    assert!(
        acli_calls(&user).contains(&"jira project view --key ABC --json".to_owned()),
        "{:?}",
        acli_calls(&user)
    );
    assert!(acli_writes(&user).is_empty(), "{:?}", acli_writes(&user));
    assert_eq!(
        refused_at_once(
            &user,
            &mcp,
            json!({"action": "milestones", "state": "later"})
        ),
        "refused: tickets has no state \"later\": its states are open (the default), closed and all"
    );
}

#[test]
fn a_version_belongs_to_a_project_so_project_chooses() {
    let user = user();
    on_projects(&user, &["ABC", "DEF"]);
    user.acli_script(json!({"versions": {
        "ABC": [{"name": "1.0"}],
        "DEF": [{"name": "7.0"}]
    }}));
    let mcp = root_in(&user);

    let both = read(&user, &mcp, json!({"action": "milestones"}));
    assert!(both.contains("ABC") && both.contains("DEF"), "{both}");
    assert!(both.contains("- 1.0 ") && both.contains("- 7.0 "), "{both}");
    let one = read(
        &user,
        &mcp,
        json!({"action": "milestones", "project": "def"}),
    );
    assert!(one.contains("- 7.0 ") && !one.contains("1.0"), "{one}");
    let other = refused_at_once(
        &user,
        &mcp,
        json!({"action": "milestones", "project": "ZZZ"}),
    );
    assert!(other.contains("ZZZ is not a project"), "{other}");
    // A creation says which project's version it means: several projects, so it must be named.
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "create", "title": "T", "type": "Task", "milestone": "1.0"}),
    );
    assert!(said.contains("project"), "{said}");
}

#[test]
fn milestones_say_what_acli_printed_when_it_prints_no_versions() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions(), "no_versions_field": true}));
    let mcp = root_in(&user);

    let answer = mcp.call("tickets", json!({"action": "milestones"}));
    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("no versions") && answer.text.contains("ABC"),
        "{}",
        answer.text
    );
    // Not knowing the project's versions, a creation in one is refused before anyone is asked.
    let created = mcp.call(
        "tickets",
        json!({"action": "create", "title": "T", "type": "Task", "milestone": "1.1"}),
    );
    assert!(
        created.error && created.text.starts_with("refused:"),
        "{}",
        created.text
    );
    assert!(requests(&user).is_empty());
    assert!(acli_writes(&user).is_empty());
}

#[test]
fn create_sets_a_work_item_s_fix_version_and_edit_and_removal_are_refused() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions(), "created": "ABC-7",
        "items": {"ABC-1": work_item("ABC-1", "Sign in", &["1.1"])}}));
    let mcp = root_in(&user);

    // A creation puts the work item in the version, by its name.
    let done = written(
        &user,
        &mcp,
        json!({"action": "create", "title": "Sign in", "type": "Task", "body": "Let users in",
            "labels": ["auth"], "milestone": "1.1"}),
    );
    assert!(done.contains("ABC-7") && done.contains("\"1.1\""), "{done}");
    let creates: Vec<Value> = user
        .acli_received()
        .into_iter()
        .filter(|call| call["args"].as_array().unwrap()[..3] == ["jira", "workitem", "create"])
        .collect();
    assert_eq!(creates.len(), 1, "{:?}", acli_calls(&user));
    let args = creates[0]["args"].as_array().unwrap();
    assert!(args.iter().any(|a| a == "--from-json"), "{args:?}");
    let item = &creates[0]["files"][0]["json"];
    assert_eq!(item["projectKey"], "ABC");
    assert_eq!(item["type"], "Task");
    assert_eq!(item["summary"], "Sign in");
    assert_eq!(item["labels"], json!(["auth"]));
    assert_eq!(
        item["additionalAttributes"],
        json!({"fixVersions": [{"name": "1.1"}]})
    );
    // Jira is asked what the work item holds before it is said to be in the version.
    assert!(
        acli_calls(&user)
            .contains(&"jira workitem view ABC-7 --json --fields fixVersions".to_owned()),
        "{:?}",
        acli_calls(&user)
    );

    // The name is the version's, whatever the agent gave: its name, or its id.
    for named in ["1.1", "10001"] {
        let done = written(
            &user,
            &mcp,
            json!({"action": "create", "title": "Again", "type": "Task", "milestone": named}),
        );
        assert!(done.contains("\"1.1\""), "{named}: {done}");
    }
    let again = user.acli_received();
    let last = again
        .iter()
        .rfind(|call| call["files"][0]["json"].is_object())
        .unwrap();
    assert_eq!(
        last["files"][0]["json"]["additionalAttributes"]["fixVersions"],
        json!([{"name": "1.1"}])
    );

    // A work item already made is not moved: acli's edit takes no custom field, so no fix
    // version, and nothing is asked of the user, nothing run.
    let before = acli_calls(&user).len();
    for args in [
        json!({"action": "edit", "ticket": "ABC-1", "milestone": "1.0"}),
        json!({"action": "edit", "ticket": "ABC-1", "remove_milestone": true}),
    ] {
        let said = refused_at_once(&user, &mcp, args.clone());
        for word in ["acli", "edit", "fix version", "Jira"] {
            assert!(said.contains(word), "{args}: {word}: {said}");
        }
        assert!(said.contains("nothing was written"), "{said}");
    }
    assert_eq!(acli_calls(&user).len(), before, "{:?}", acli_calls(&user));
}

#[test]
fn a_creation_in_a_version_needs_a_type() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions()}));
    let mcp = root_in(&user);

    // acli sets a field outside its flags from a JSON definition, which has no default type.
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "create", "title": "T", "milestone": "1.1"}),
    );
    assert!(
        said.contains("needs type") && said.contains("milestone"),
        "{said}"
    );
    assert!(acli_calls(&user).is_empty(), "{:?}", acli_calls(&user));
}

#[test]
fn list_filters_by_fix_version_and_show_says_it() {
    let user = user();
    on_jira(&user);
    let mut third = work_item("ABC-3", "Sign out", &["2.0"]);
    third["fields"]["summary"] = json!("Sign out of everything");
    user.acli_script(json!({"versions": versions(), "items": {
        "ABC-1": work_item("ABC-1", "Sign in", &["1.1"]),
        "ABC-2": work_item("ABC-2", "Reset a password", &[]),
        "ABC-3": third,
        "ABC-4": work_item("ABC-4", "Sign in again", &["1.1", "2.0"])
    }}));
    let mcp = root_in(&user);

    let listed = read(
        &user,
        &mcp,
        json!({"action": "list", "milestone": "1.1", "state": "all"}),
    );
    assert!(
        listed.contains("ABC-1") && listed.contains("ABC-4"),
        "{listed}"
    );
    assert!(
        !listed.contains("ABC-2") && !listed.contains("ABC-3"),
        "{listed}"
    );
    assert!(listed.contains("in milestone 1.1"), "{listed}");
    assert!(
        listed.contains("Sign in") && listed.contains("milestone: 1.1"),
        "{listed}"
    );
    let calls = acli_calls(&user);
    let search = calls
        .iter()
        .find(|c| c.contains("workitem search"))
        .unwrap();
    assert!(
        search.contains("fixVersion = \"1.1\"") && search.contains("project in (\"ABC\")"),
        "{search}"
    );

    // A search takes it too, with its words; its id finds it too.
    let searched = read(
        &user,
        &mcp,
        json!({"action": "search", "query": "sign", "milestone": "10003", "state": "all"}),
    );
    assert!(
        searched.contains("ABC-3") && searched.contains("ABC-4"),
        "{searched}"
    );
    assert!(!searched.contains("ABC-1"), "{searched}");

    // show says it, with the other fields.
    let shown = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-4"}));
    assert!(shown.contains("Milestone: 1.1, 2.0"), "{shown}");
    let none = read(&user, &mcp, json!({"action": "show", "ticket": "ABC-2"}));
    assert!(!none.contains("Milestone"), "{none}");
    assert!(acli_writes(&user).is_empty());
}

#[test]
fn an_unknown_version_is_refused_listing_the_existing_ones() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions()}));
    let mcp = root_in(&user);

    for args in [
        json!({"action": "create", "title": "T", "type": "Task", "milestone": "9.9"}),
        json!({"action": "list", "milestone": "9.9"}),
        json!({"action": "search", "query": "x", "milestone": "9.9"}),
    ] {
        let said = refused_at_once(&user, &mcp, args.clone());
        assert!(
            said.contains("there is no milestone \"9.9\" among the versions of ABC"),
            "{args}: {said}"
        );
        for named in ["1.0", "1.1", "0.9", "2.0"] {
            assert!(said.contains(named), "{named}: {said}");
        }
        assert!(
            said.contains("nothing is created implicitly") && said.contains("Jira"),
            "{said}"
        );
    }
    assert!(acli_writes(&user).is_empty());
    assert!(
        !acli_calls(&user)
            .iter()
            .any(|call| call.contains("workitem search")),
        "{:?}",
        acli_calls(&user)
    );
    // A project with no version says so.
    user.acli_script(json!({"versions": {"ABC": []}}));
    let said = refused_at_once(&user, &mcp, json!({"action": "list", "milestone": "1.0"}));
    assert!(said.contains("it has none"), "{said}");
}

#[test]
fn milestone_create_and_edit_on_jira_do_what_acli_can_and_refuse_the_rest() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions()}));
    let mcp = root_in(&user);

    // acli lists no command for a project's versions but a read of the project, so a version is
    // neither made nor released nor renamed: said at once, with what the user does instead.
    for args in [
        json!({"action": "milestone_create", "title": "3.0", "due": "2027-01-31",
            "description": "Next"}),
        json!({"action": "milestone_edit", "milestone": "1.1", "state": "closed"}),
        json!({"action": "milestone_edit", "milestone": "1.1", "title": "1.1.0"}),
    ] {
        let said = refused_at_once(&user, &mcp, args.clone());
        for word in ["acli", "Releases", "REST"] {
            assert!(said.contains(word), "{args}: {word}: {said}");
        }
        assert!(said.contains("nothing was written"), "{said}");
        assert!(!said.contains("sprint"), "{said}");
    }
    assert!(acli_calls(&user).is_empty(), "{:?}", acli_calls(&user));
    // The GitHub shapes of arguments stay as they were: a due off its actions is refused.
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "create", "title": "T", "due": "2026-12-01"}),
    );
    assert!(said.contains("takes no due"), "{said}");
}

#[test]
fn version_writes_go_through_the_gate_as_other_ticket_writes() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions(), "created": "ABC-8"}));
    let mcp = root_in(&user);

    let create = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "Sign in", "type": "Task", "milestone": "1.1"}),
    ));
    let denied = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "Sign out", "type": "Task", "milestone": "2.0"}),
    ));
    // Held like any creation: the user reads the version as Jira names it, nothing ran before.
    let cards: Vec<String> = requests(&user).iter().map(Value::to_string).collect();
    assert_eq!(cards.len(), 2);
    assert!(cards[0].contains("opens a ticket"), "{}", cards[0]);
    assert!(
        cards[0].contains("fix version to \\\"1.1\\\" (id 10001")
            && cards[0].contains("2026-12-01"),
        "{}",
        cards[0]
    );
    assert!(acli_writes(&user).is_empty(), "{:?}", acli_writes(&user));
    user.call("approval.deny", json!({"number": denied}))
        .unwrap();
    eventually("the denial", || {
        store(&user)
            .delivery(&format!("approval-{denied}"))
            .unwrap()
            .is_some()
    });
    assert!(acli_writes(&user).is_empty(), "{:?}", acli_writes(&user));
    let done = allow(&user, create);
    assert!(done.contains("and Flottille has now done it"), "{done}");
    assert_eq!(acli_writes(&user).len(), 1, "{:?}", acli_writes(&user));
}

#[test]
fn a_version_is_found_in_the_tracker_s_project_only() {
    let user = user();
    on_projects(&user, &["ABC", "DEF"]);
    user.acli_script(json!({"versions": {"ABC": [{"name": "1.0"}], "DEF": [{"name": "7.0"}]}}));
    let mcp = root_in(&user);

    // `project` names where it is looked for: 7.0 is DEF's, not ABC's.
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "create", "title": "T", "type": "Task", "project": "ABC",
            "milestone": "7.0"}),
    );
    assert!(
        said.contains("among the versions of ABC") && !said.contains("DEF"),
        "{said}"
    );
    let listed = read(
        &user,
        &mcp,
        json!({"action": "list", "milestone": "7.0", "project": "DEF"}),
    );
    assert!(listed.contains("in milestone 7.0"), "{listed}");
    // A project the tracker does not hold is refused.
    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "list", "milestone": "7.0", "project": "ZZZ"}),
    );
    assert!(said.contains("ZZZ is not a project"), "{said}");
    // The JQL stays confined to the tracker's projects.
    let calls = acli_calls(&user);
    let search = calls
        .iter()
        .find(|c| c.contains("workitem search"))
        .unwrap();
    assert!(search.contains("project in (\"ABC\", \"DEF\")"), "{search}");
}

#[test]
fn a_fix_version_jira_ignores_is_said_and_never_claimed() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions(), "created": "ABC-12",
        "drops_fix_versions": true}));
    let mcp = root_in(&user);

    // acli ends the creation with 0 and Jira keeps no fix version: the work item is read back.
    let done = written(
        &user,
        &mcp,
        json!({"action": "create", "title": "Sign in", "type": "Task", "milestone": "1.1"}),
    );
    assert!(
        done.contains(
            "The ticket ABC-12 was opened in ABC, but Jira did not set its fix version \"1.1\""
        ),
        "{done}"
    );
    assert!(!done.contains("in the version"), "{done}");
    assert!(done.contains("the user sets it in Jira"), "{done}");

    // A work item that cannot be read back is not said to be in the version either.
    user.acli_script(json!({"versions": versions(), "created": "ABC-13",
        "fail": {"ABC-13": "Error: the work item could not be fetched"}}));
    let done = written(
        &user,
        &mcp,
        json!({"action": "create", "title": "Sign out", "type": "Task", "milestone": "1.1"}),
    );
    assert!(
        done.contains("ABC-13") && done.contains("could not be confirmed"),
        "{done}"
    );
    // The link is the work item's, never read as what could not be confirmed.
    assert!(
        done.contains("/browse/ABC-13; its fix version \"1.1\" was asked"),
        "{done}"
    );
    assert!(!done.contains("in the version"), "{done}");
}

#[test]
fn a_version_name_the_masking_would_change_is_used_as_it_is_and_shown_masked() {
    let token = "ghp_fakefakefakefakefakefakefake0079";
    let name = format!("rc-{token}");
    let user = user().with_env("GH_TOKEN", token);
    on_jira(&user);
    user.acli_script(
        json!({"versions": {"ABC": [{"name": name}]}, "created": "ABC-5",
        "items": {"ABC-1": work_item("ABC-1", "Sign in", &[&name])}}),
    );
    let mcp = root_in(&user);

    // What is shown is masked, and what Jira is given is not.
    let listed = read(&user, &mcp, json!({"action": "milestones"}));
    assert!(
        listed.contains("rc-[masked]") && !listed.contains(token),
        "{listed}"
    );
    let held = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "Sign in", "type": "Task", "milestone": &name}),
    ));
    let card = requests(&user)[0].to_string();
    assert!(
        card.contains("rc-[masked]") && !card.contains(token),
        "{card}"
    );
    let done = allow(&user, held);
    assert!(
        done.contains("in the version") && !done.contains(token),
        "{done}"
    );
    let created: Vec<Value> = user
        .acli_received()
        .into_iter()
        .filter(|call| call["files"][0]["json"]["additionalAttributes"].is_object())
        .collect();
    assert_eq!(created.len(), 1, "{:?}", acli_calls(&user));
    assert_eq!(
        created[0]["files"][0]["json"]["additionalAttributes"]["fixVersions"],
        json!([{"name": name}])
    );
    // The filter finds it by the real name, and says it masked.
    let listed = read(
        &user,
        &mcp,
        json!({"action": "list", "milestone": &name, "state": "all"}),
    );
    assert!(
        listed.contains("ABC-1") && listed.contains("rc-[masked]"),
        "{listed}"
    );
    assert!(!listed.contains(token), "{listed}");
    let calls = acli_calls(&user);
    let search = calls
        .iter()
        .find(|c| c.contains("workitem search"))
        .unwrap();
    assert!(
        search.contains(&format!("fixVersion = \"{name}\"")),
        "{search}"
    );
}

#[test]
fn a_version_whose_name_holds_a_quote_and_a_backslash_is_quoted_in_the_jql() {
    let user = user();
    on_jira(&user);
    let name = r#"a"b\c"#;
    user.acli_script(json!({"versions": {"ABC": [{"name": name}]}, "items": {
        "ABC-1": work_item("ABC-1", "Sign in", &[name]),
        "ABC-2": work_item("ABC-2", "Sign out", &[])
    }}));
    let mcp = root_in(&user);

    let listed = read(
        &user,
        &mcp,
        json!({"action": "list", "milestone": name, "state": "all"}),
    );
    assert!(
        listed.contains("ABC-1") && !listed.contains("ABC-2"),
        "{listed}"
    );
    let calls = acli_calls(&user);
    let search = calls
        .iter()
        .find(|c| c.contains("workitem search"))
        .unwrap();
    // The JQL, word for word: the name quoted with its quote and its backslash escaped.
    assert!(
        search.contains(
            r#"--jql project in ("ABC") AND fixVersion = "a\"b\\c" ORDER BY created DESC --fields"#
        ),
        "{search}"
    );
}

#[test]
fn project_and_milestone_list_the_items_of_that_project_only() {
    let user = user();
    on_projects(&user, &["ABC", "DEF"]);
    user.acli_script(
        json!({"versions": {"ABC": [{"name": "1.0"}], "DEF": [{"name": "1.0"}]},
        "items": {
            "ABC-1": work_item("ABC-1", "Sign in", &["1.0"]),
            "DEF-1": work_item("DEF-1", "Sign in elsewhere", &["1.0"])
        }}),
    );
    let mcp = root_in(&user);

    let both = read(
        &user,
        &mcp,
        json!({"action": "list", "milestone": "1.0", "state": "all"}),
    );
    assert!(both.contains("ABC-1") && both.contains("DEF-1"), "{both}");
    let one = read(
        &user,
        &mcp,
        json!({"action": "list", "milestone": "1.0", "state": "all", "project": "def"}),
    );
    assert!(one.contains("DEF-1") && !one.contains("ABC-1"), "{one}");
    let searched = read(
        &user,
        &mcp,
        json!({"action": "search", "query": "sign", "milestone": "1.0", "state": "all",
            "project": "ABC"}),
    );
    assert!(
        searched.contains("ABC-1") && !searched.contains("DEF-1"),
        "{searched}"
    );
    let calls = acli_calls(&user);
    let searches: Vec<&String> = calls
        .iter()
        .filter(|c| c.contains("workitem search"))
        .collect();
    assert_eq!(searches.len(), 3, "{calls:?}");
    // Confined to the tracker's projects still, and to the project the call names.
    assert!(
        searches[0].contains(r#"project in ("ABC", "DEF") AND fixVersion = "1.0" ORDER"#),
        "{}",
        searches[0]
    );
    assert!(
        searches[1].contains(
            r#"project in ("ABC", "DEF") AND fixVersion = "1.0" AND project = "DEF" ORDER"#
        ),
        "{}",
        searches[1]
    );
    assert!(
        searches[2].contains(r#"AND project = "ABC""#),
        "{}",
        searches[2]
    );
}

#[test]
fn a_version_changed_while_the_request_waits_is_not_written() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions(), "created": "ABC-9"}));
    let mcp = root_in(&user);

    let renamed = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "Sign in", "type": "Task", "milestone": "1.1"}),
    ));
    let gone = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "Sign out", "type": "Task", "milestone": "2.0"}),
    ));
    // The user is slow: Jira's 1.1 (id 10001) is renamed and another version takes its name, and
    // 2.0 is deleted. Nothing may be written to what merely has the name now.
    user.acli_script(json!({"created": "ABC-9", "versions": {"ABC": [
        {"id": "10000", "name": "1.0", "released": true, "releaseDate": "2026-10-01"},
        {"id": "10001", "name": "1.2", "releaseDate": "2026-12-01"},
        {"id": "20000", "name": "1.1", "releaseDate": "2027-01-01"}
    ]}}));
    for number in [renamed, gone] {
        let said = allow(&user, number);
        assert!(!said.contains("and Flottille has now done it"), "{said}");
    }
    assert!(acli_writes(&user).is_empty(), "{:?}", acli_writes(&user));
    let said = store(&user)
        .delivery(&format!("approval-{renamed}"))
        .unwrap()
        .unwrap()
        .body;
    assert!(
        said.contains("what it does changed since the user allowed it")
            && said.contains("\"1.1\" (id 20000"),
        "{said}"
    );
    let said = store(&user)
        .delivery(&format!("approval-{gone}"))
        .unwrap()
        .unwrap()
        .body;
    assert!(
        said.contains("there is no milestone \"2.0\" among the versions of ABC")
            && said.contains("nothing was written"),
        "{said}"
    );
}

/// The versions of ABC once 1.1 (id 10001) was renamed 1.2 and another version took its name.
fn renamed_and_replaced() -> Value {
    json!({"created": "ABC-9", "versions": {"ABC": [
        {"id": "10000", "name": "1.0", "released": true, "releaseDate": "2026-10-01"},
        {"id": "10001", "name": "1.2", "releaseDate": "2026-12-01"},
        {"id": "10009", "name": "1.1", "releaseDate": "2027-01-01"}
    ]}})
}

#[test]
fn each_request_writes_the_version_its_own_card_showed() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions(), "created": "ABC-9"}));
    let mcp = root_in(&user);

    let first = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "Sign in", "type": "Task", "milestone": "1.1"}),
    ));
    // Between the two cards, 1.1 (id 10001) is renamed and another version takes its name: the
    // second card shows that one, which the first request was never shown.
    user.acli_script(renamed_and_replaced());
    let second = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "Sign out", "type": "Task", "milestone": "1.1"}),
    ));
    let cards: Vec<String> = requests(&user).iter().map(Value::to_string).collect();
    assert!(cards[0].contains("(id 10001"), "{}", cards[0]);
    assert!(cards[1].contains("(id 10009"), "{}", cards[1]);

    let said = allow(&user, first);
    assert!(
        said.contains("what it does changed since the user allowed it")
            && said.contains("(id 10009")
            && !said.contains("and Flottille has now done it"),
        "{said}"
    );
    assert!(acli_writes(&user).is_empty(), "{:?}", acli_writes(&user));

    let said = allow(&user, second);
    assert!(said.contains("and Flottille has now done it"), "{said}");
    let writes = acli_writes(&user);
    assert_eq!(writes.len(), 1, "{writes:?}");
    let made = std::fs::read_to_string(user.path("acli/created.json")).unwrap();
    assert!(
        made.contains("Sign out") && !made.contains("Sign in"),
        "{made}"
    );
}

#[test]
fn a_creation_allowed_after_a_restart_is_checked_against_its_card() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"versions": versions(), "created": "ABC-9"}));
    let mcp = root_in(&user);

    let number = held(&mcp.call(
        "tickets",
        json!({"action": "create", "title": "Sign in", "type": "Task", "milestone": "1.1"}),
    ));
    // The request outlives the daemon; what it was shown is in the store, nowhere else.
    assert!(user.run(&["daemon", "stop"]).status.success());
    user.acli_script(renamed_and_replaced());

    let said = allow(&user, number);
    assert!(
        said.contains("what it does changed since the user allowed it")
            && said.contains("(id 10009"),
        "{said}"
    );
    assert!(acli_writes(&user).is_empty(), "{:?}", acli_writes(&user));
}
