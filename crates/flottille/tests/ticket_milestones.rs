//! The milestones of the tickets tool (#758, `docs/tickets.md`, "Milestones"): a GitHub tracker's
//! milestones listed, made and changed, a ticket put in one or taken out, and the list and the
//! reading of a ticket that say it. The tracker is the fake `gh` (`crates/flottille-fake-gh`) and
//! the fake `acli`; the remotes are throwaway bare repositories; the escalating programs are
//! fakes, first on the daemon's `PATH`, and none runs.

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

fn on_github(user: &User) {
    fixture(user, json!({"kind": "github", "repository": "."}));
}

fn on_jira(user: &User) {
    fixture(
        user,
        json!({"kind": "jira", "site": SITE, "projects": ["ABC"]}),
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

fn gh_calls(user: &User) -> Vec<String> {
    user.gh_received()
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

/// The calls that wrote: `gh api` with a method, and the issues' writes.
fn gh_writes(user: &User) -> Vec<String> {
    gh_calls(user)
        .into_iter()
        .filter(|call| {
            call.contains("--method")
                || ["issue create", "issue edit"]
                    .iter()
                    .any(|write| call.starts_with(write))
        })
        .collect()
}

fn milestone(
    number: u64,
    title: &str,
    state: &str,
    due: Option<&str>,
    open: u64,
    closed: u64,
) -> Value {
    json!({"number": number, "title": title, "state": state, "description": "",
        "due_on": due, "open_issues": open, "closed_issues": closed,
        "html_url": format!("https://github.com/acme/app/milestone/{number}")})
}

/// Two open milestones and a closed one, as GitHub lists them.
fn milestones() -> Value {
    json!([
        milestone(1, "v0.1", "open", Some("2026-12-01T00:00:00Z"), 2, 1),
        milestone(2, "v0.2", "open", None, 4, 0),
        milestone(3, "v0.0", "closed", None, 0, 5),
    ])
}

fn issue(number: u64, title: &str, milestone: Option<(u64, &str)>) -> Value {
    json!({"number": number, "title": title, "state": "OPEN",
        "body": "Users cannot sign in.",
        "url": format!("https://github.com/acme/app/issues/{number}"),
        "labels": [], "assignees": [], "comments": [],
        "milestone": milestone.map(|(n, title)| json!({"number": n, "title": title}))})
}

/// The tracker: its milestones and, by number, its issues.
fn script(user: &User, issues: Value) {
    user.gh_script(json!({
        "milestones": {"acme/app": milestones()},
        "issues": {"acme/app": issues},
    }));
}

#[test]
fn milestones_lists_the_tracker_s_milestones_with_their_counts() {
    let user = user();
    on_github(&user);
    script(&user, json!({}));
    let mcp = root_in(&user);

    let open = read(&user, &mcp, json!({"action": "milestones"}));
    let all = read(&user, &mcp, json!({"action": "milestones", "state": "all"}));

    assert!(open.contains("acme/app"), "{open}");
    let line = open.lines().find(|l| l.contains("v0.1")).expect(&open);
    for said in [
        "open",
        "due 2026-12-01",
        "2 open",
        "1 closed",
        "https://github.com/acme/app/milestone/1",
    ] {
        assert!(line.contains(said), "{said} missing: {line}");
    }
    let line = open.lines().find(|l| l.contains("v0.2")).expect(&open);
    assert!(!line.contains("due"), "{line}");
    assert!(
        !open.contains("v0.0"),
        "a closed milestone is listed: {open}"
    );
    assert!(all.contains("v0.0") && all.contains("closed"), "{all}");
    // The tracker's repository, never one the agent names; one page after another.
    let asked = gh_calls(&user);
    assert!(
        asked.iter().any(
            |call| call.starts_with("api repos/acme/app/milestones?state=open")
                && call.contains("--paginate")
        ),
        "{asked:?}"
    );
    assert!(
        asked
            .iter()
            .any(|call| call.starts_with("api repos/acme/app/milestones?state=all")),
        "{asked:?}"
    );
}

#[test]
fn milestone_create_makes_one_and_refuses_a_duplicate_title() {
    let user = user();
    on_github(&user);
    script(&user, json!({}));
    let mcp = root_in(&user);

    let made = written(
        &user,
        &mcp,
        json!({"action": "milestone_create", "title": "v0.3",
            "description": "The sign-in work", "due": "2026-12-31"}),
    );

    assert!(made.contains("v0.3"), "{made}");
    assert!(
        made.contains("https://github.com/acme/app/milestone/4"),
        "{made}"
    );
    assert_eq!(
        gh_writes(&user),
        [
            "api repos/acme/app/milestones --hostname github.com --method POST -f title=v0.3 \
          -f description=The sign-in work -f due_on=2026-12-31T00:00:00Z"
        ]
    );
    // Closed or open, an existing title is not made again.
    for title in ["v0.1", "v0.0"] {
        let said = refused_at_once(
            &user,
            &mcp,
            json!({"action": "milestone_create", "title": title}),
        );
        assert!(said.contains(title) && said.contains("already"), "{said}");
    }
    assert_eq!(gh_writes(&user).len(), 1, "{:?}", gh_writes(&user));
    // A title, and a due date that is one.
    for (args, said) in [
        (json!({"action": "milestone_create"}), "needs title"),
        (
            json!({"action": "milestone_create", "title": "x", "due": "next week"}),
            "YYYY-MM-DD",
        ),
        (
            json!({"action": "milestone_create", "title": "x", "due": "2026-02-30"}),
            "YYYY-MM-DD",
        ),
    ] {
        let refused = refused_at_once(&user, &mcp, args.clone());
        assert!(refused.contains(said), "{args}: {refused}");
    }
}

#[test]
fn milestone_edit_closes_and_reopens_a_milestone() {
    let user = user();
    on_github(&user);
    script(&user, json!({}));
    let mcp = root_in(&user);

    // By its title, then by its number.
    written(
        &user,
        &mcp,
        json!({"action": "milestone_edit", "milestone": "v0.1", "state": "closed"}),
    );
    written(
        &user,
        &mcp,
        json!({"action": "milestone_edit", "milestone": "3", "state": "open",
            "title": "v0.0.1", "description": "Back", "due": "2027-01-15"}),
    );

    assert_eq!(
        gh_writes(&user),
        [
            "api repos/acme/app/milestones/1 --hostname github.com --method PATCH -f state=closed",
            "api repos/acme/app/milestones/3 --hostname github.com --method PATCH -f title=v0.0.1 \
             -f description=Back -f due_on=2027-01-15T00:00:00Z -f state=open",
        ]
    );
    for (args, said) in [
        (
            json!({"action": "milestone_edit", "milestone": "v0.1"}),
            "what changes",
        ),
        (
            json!({"action": "milestone_edit", "state": "closed"}),
            "needs milestone",
        ),
        (
            json!({"action": "milestone_edit", "milestone": "v0.1", "state": "all"}),
            "open or closed",
        ),
        (
            json!({"action": "milestone_edit", "milestone": "v0.1", "title": "v0.2"}),
            "already",
        ),
    ] {
        let refused = refused_at_once(&user, &mcp, args.clone());
        assert!(refused.contains(said), "{args}: {refused}");
    }
    assert_eq!(gh_writes(&user).len(), 2);
}

#[test]
fn create_and_edit_set_a_ticket_s_milestone_and_remove_it() {
    let user = user();
    on_github(&user);
    script(&user, json!({"1": issue(1, "The login fails", None)}));
    let mcp = root_in(&user);

    written(
        &user,
        &mcp,
        json!({"action": "create", "title": "Add search", "milestone": "v0.1"}),
    );
    // By number, passed to `gh` as the title it is: `gh issue edit` takes the name.
    written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "#1", "milestone": "2"}),
    );
    written(
        &user,
        &mcp,
        json!({"action": "edit", "ticket": "#1", "remove_milestone": true}),
    );

    let writes = gh_writes(&user);
    assert_eq!(writes.len(), 3, "{writes:?}");
    assert!(
        writes[0].starts_with("issue create -R github.com/acme/app")
            && writes[0].ends_with("--milestone v0.1"),
        "{}",
        writes[0]
    );
    assert_eq!(
        writes[1],
        "issue edit 1 -R github.com/acme/app --milestone v0.2"
    );
    assert_eq!(
        writes[2],
        "issue edit 1 -R github.com/acme/app --remove-milestone"
    );
    // A milestone and its removal together, and a removal off an edit, say nothing coherent.
    for (args, said) in [
        (
            json!({"action": "edit", "ticket": "#1", "milestone": "v0.1", "remove_milestone": true}),
            "remove_milestone",
        ),
        (
            json!({"action": "close", "ticket": "#1", "remove_milestone": true}),
            "is for edit",
        ),
        (
            json!({"action": "comment", "ticket": "#1", "body": "x", "milestone": "v0.1"}),
            "milestone is for",
        ),
    ] {
        let refused = refused_at_once(&user, &mcp, args.clone());
        assert!(refused.contains(said), "{args}: {refused}");
    }
}

#[test]
fn an_unknown_milestone_is_refused_listing_the_existing_ones() {
    let user = user();
    on_github(&user);
    script(&user, json!({"1": issue(1, "The login fails", None)}));
    let mcp = root_in(&user);

    for args in [
        json!({"action": "create", "title": "Add search", "milestone": "v9"}),
        json!({"action": "edit", "ticket": "#1", "milestone": "v9"}),
        json!({"action": "milestone_edit", "milestone": "v9", "state": "closed"}),
    ] {
        let said = refused_at_once(&user, &mcp, args.clone());
        assert!(said.contains("\"v9\""), "{args}: {said}");
        for existing in ["v0.1", "v0.2", "v0.0"] {
            assert!(said.contains(existing), "{existing} missing: {said}");
        }
        assert!(said.contains("nothing is created implicitly"), "{said}");
    }
    // A list is no write: refused too, answered at once.
    let listed = mcp.call("tickets", json!({"action": "list", "milestone": "v9"}));
    assert!(listed.error, "{}", listed.text);
    assert!(
        listed.text.starts_with("refused:") && listed.text.contains("v0.1"),
        "{}",
        listed.text
    );
    // Nothing was made implicitly.
    assert_eq!(gh_writes(&user), Vec::<String>::new());
}

#[test]
fn list_filters_by_milestone_and_show_says_it() {
    let user = user();
    on_github(&user);
    script(
        &user,
        json!({
            "1": issue(1, "The login fails", Some((1, "v0.1"))),
            "2": issue(2, "Add search", None),
            "3": issue(3, "Reset the password", Some((1, "v0.1"))),
            "4": issue(4, "Dark mode", Some((2, "v0.2"))),
        }),
    );
    let mcp = root_in(&user);

    let filtered = read(&user, &mcp, json!({"action": "list", "milestone": "v0.1"}));
    let by_number = read(&user, &mcp, json!({"action": "list", "milestone": "2"}));
    let searched = read(
        &user,
        &mcp,
        json!({"action": "search", "query": "password", "milestone": "v0.1"}),
    );
    let shown = read(&user, &mcp, json!({"action": "show", "ticket": "#1"}));
    let none = read(&user, &mcp, json!({"action": "show", "ticket": "#2"}));

    assert!(
        filtered.contains("in milestone v0.1"),
        "the list says its filter: {filtered}"
    );
    assert!(
        filtered.contains("#1") && filtered.contains("#3"),
        "{filtered}"
    );
    assert!(
        !filtered.contains("#2") && !filtered.contains("#4"),
        "{filtered}"
    );
    // A line shows the ticket's milestone.
    let line = filtered.lines().find(|l| l.contains("#3")).unwrap();
    assert!(line.contains("milestone: v0.1"), "{line}");
    assert!(
        by_number.contains("#4") && !by_number.contains("#1"),
        "{by_number}"
    );
    assert!(
        searched.contains("#3") && !searched.contains("#1"),
        "{searched}"
    );
    assert!(shown.contains("Milestone: v0.1"), "{shown}");
    assert!(!none.contains("Milestone"), "{none}");
    // The tracker is asked with the title, not the number the agent gave.
    assert!(
        gh_calls(&user)
            .iter()
            .any(|call| call.starts_with("issue list") && call.contains("--milestone v0.2")),
        "{:?}",
        gh_calls(&user)
    );
}

#[test]
fn milestone_writes_go_through_the_gate_as_other_ticket_writes() {
    let user = user();
    on_github(&user);
    script(&user, json!({"1": issue(1, "The login fails", None)}));
    let mcp = root_in(&user);

    let create = held(&mcp.call(
        "tickets",
        json!({"action": "milestone_create", "title": "v0.3", "description": "Sign-in",
            "due": "2026-12-31"}),
    ));
    let edit = held(&mcp.call(
        "tickets",
        json!({"action": "milestone_edit", "milestone": "v0.1", "state": "closed"}),
    ));
    let put = held(&mcp.call(
        "tickets",
        json!({"action": "edit", "ticket": "#1", "milestone": "v0.1"}),
    ));

    // Held like a create: the user reads what it does, and nothing ran before they allowed it.
    let cards: Vec<String> = requests(&user).iter().map(Value::to_string).collect();
    assert_eq!(cards.len(), 3);
    assert!(cards[0].contains("creates a milestone"), "{}", cards[0]);
    for said in ["v0.3", "Sign-in", "2026-12-31"] {
        assert!(cards[0].contains(said), "{said}: {}", cards[0]);
    }
    assert!(
        cards[1].contains("changes a milestone") && cards[1].contains("closes it"),
        "{}",
        cards[1]
    );
    assert!(cards[2].contains("milestone \\\"v0.1\\\""), "{}", cards[2]);
    assert_eq!(gh_writes(&user), Vec::<String>::new());
    for number in [create, edit, put] {
        let done = allow(&user, number);
        assert!(done.contains("and Flottille has now done it"), "{done}");
    }
    assert_eq!(gh_writes(&user).len(), 3, "{:?}", gh_writes(&user));
    // A refused answer of the user changes nothing either.
    let denied = held(&mcp.call(
        "tickets",
        json!({"action": "milestone_create", "title": "v0.4"}),
    ));
    user.call("approval.deny", json!({"number": denied}))
        .unwrap();
    eventually("the denial", || {
        store(&user)
            .delivery(&format!("approval-{denied}"))
            .unwrap()
            .is_some()
    });
    assert_eq!(gh_writes(&user).len(), 3);
}

#[test]
fn milestone_writes_are_refused_on_jira_where_acli_makes_no_version() {
    let user = user();
    on_jira(&user);
    user.acli_script(json!({"items": {}}));
    let mcp = root_in(&user);

    // A Jira milestone is a version (`ticket_versions.rs`): read, and a work item created in one;
    // acli makes, changes and releases none, and its edit moves none.
    for args in [
        json!({"action": "milestone_create", "title": "v1"}),
        json!({"action": "milestone_edit", "milestone": "v1", "state": "closed"}),
        json!({"action": "edit", "ticket": "ABC-1", "milestone": "v1"}),
        json!({"action": "edit", "ticket": "ABC-1", "remove_milestone": true}),
    ] {
        let said = refused_at_once(&user, &mcp, args.clone());
        assert!(
            said.contains("acli") && said.contains("Jira") && said.contains("nothing was written"),
            "{args}: {said}"
        );
    }
    // Nothing was run: not `acli`, not `gh`.
    assert!(
        user.acli_received().is_empty(),
        "{:?}",
        user.acli_received()
    );
    assert!(gh_writes(&user).is_empty());
}

/// The arguments of every `gh` call that wrote through the API, as `gh` received them.
fn api_writes(user: &User) -> Vec<Vec<String>> {
    user.gh_received()
        .iter()
        .map(|call| {
            call["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        })
        .filter(|args| args.contains(&"--method".to_owned()))
        .collect()
}

#[test]
fn injection_shaped_milestone_input_stays_one_value() {
    let user = user();
    on_github(&user);
    script(&user, json!({"1": issue(1, "The login fails", None)}));
    let mcp = root_in(&user);
    let described = "first line\n-f state=closed\nsecond line";

    // What a title or a description could be taken for: an option, a query string, a file that
    // `-F` would read, a second field. `-f` sends each as the text it is.
    for title in ["-x", "a=b&c?d", "@/etc/passwd"] {
        written(
            &user,
            &mcp,
            json!({"action": "milestone_create", "title": title, "description": described}),
        );
    }

    let writes = api_writes(&user);
    assert_eq!(writes.len(), 3, "{writes:?}");
    for (write, title) in writes.iter().zip(["-x", "a=b&c?d", "@/etc/passwd"]) {
        assert_eq!(
            write[..6],
            [
                "api",
                "repos/acme/app/milestones",
                "--hostname",
                "github.com",
                "--method",
                "POST"
            ],
            "{write:?}"
        );
        assert_eq!(
            write[6..],
            [
                "-f".to_owned(),
                format!("title={title}"),
                "-f".to_owned(),
                format!("description={described}")
            ],
            "{write:?}"
        );
        assert!(
            !write.iter().any(|a| a == "-F" || a == "--field"),
            "{write:?}"
        );
    }
    // A milestone named like a path is none: refused listing the existing ones, nothing sent.
    for args in [
        json!({"action": "milestone_edit", "milestone": "1/../x", "state": "closed"}),
        json!({"action": "edit", "ticket": "#1", "milestone": "1/../x"}),
    ] {
        let said = refused_at_once(&user, &mcp, args.clone());
        assert!(
            said.contains("v0.1") && said.contains("v0.2"),
            "{args}: {said}"
        );
    }
    assert_eq!(api_writes(&user).len(), 3);
    assert_eq!(gh_writes(&user).len(), 3, "{:?}", gh_writes(&user));
}

#[test]
fn milestone_create_refuses_a_state() {
    let user = user();
    on_github(&user);
    script(&user, json!({}));
    let mcp = root_in(&user);

    let said = refused_at_once(
        &user,
        &mcp,
        json!({"action": "milestone_create", "title": "v0.3", "state": "closed"}),
    );

    assert!(
        said.contains("milestone_create takes no state") && said.contains("milestone_edit"),
        "{said}"
    );
    assert_eq!(gh_writes(&user), Vec::<String>::new());
}
