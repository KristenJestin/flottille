//! The scenarios of the tickets (`docs/tickets.md`): a workspace linked to a GitHub issue or a Jira
//! work item, read through a fake `gh` (`crates/flottille-fake-gh`) and a fake `acli`
//! (`crates/flottille-fake-acli`) on the daemon's `PATH`, in throwaway repositories whose `origin`
//! looks like GitHub's. No test ever calls GitHub or Jira.

mod common;

use std::path::PathBuf;

use common::User;
use common::mcp::{Mcp, server_of};
use serde_json::{Value, json};

/// A project `app` of two repositories, `lib` first on another host and `api` on GitHub, and its
/// ready workspace `login`. Each repository is pushed to a bare remote of its own first, so that
/// the workspace's fetch works; its `origin` then says where the forge is.
fn fixture(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    for repository in ["lib", "api"] {
        let dir = folder.join(repository);
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), format!("{repository}\n")).unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
        let remote = user.path(&format!("remotes/{repository}.git"));
        user.git(
            &user.path(""),
            &["init", "--quiet", "--bare", remote.to_str().unwrap()],
        );
        user.git(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
        user.git(&dir, &["push", "--quiet", "origin", "main"]);
    }
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    for (repository, url) in [
        ("lib", "https://git.example.com/acme/lib.git"),
        ("api", "git@github.com:acme/api.git"),
    ] {
        user.git(
            &folder.join(repository),
            &["remote", "set-url", "origin", url],
        );
    }
    folder
}

/// What `gh issue view --json` answers for an issue of `acme/api`.
fn issue(number: u64, title: &str) -> Value {
    json!({
        "number": number,
        "title": title,
        "state": "OPEN",
        "body": "Users cannot sign in.\nSee the form.",
        "url": format!("https://github.com/acme/api/issues/{number}"),
        "comments": [
            {"author": {"login": "alice"}, "createdAt": "2026-10-01T10:00:00Z", "body": "I can reproduce"},
            {"author": {"login": "bob"}, "createdAt": "2026-10-02T09:30:00Z", "body": "Fixed on a branch"}
        ]
    })
}

fn issues(user: &User, issues: Value) {
    user.gh_script(json!({"issues": {"acme/api": issues}}));
}

/// A text field in Atlassian Document Format: one paragraph.
fn document(text: &str) -> Value {
    json!({"type": "doc", "version": 1, "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

/// What `acli jira workitem view ABC-123 --json` is assumed to print (`docs/tickets.md`).
fn work_item(key: &str, summary: &str) -> Value {
    json!({
        "key": key,
        "id": "10001",
        "self": "https://acme.atlassian.net/rest/api/3/issue/10001",
        "fields": {
            "summary": summary,
            "status": {"name": "In Progress", "statusCategory": {"key": "indeterminate"}},
            "description": {"type": "doc", "version": 1, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "Let users log in."}]},
                {"type": "bulletList", "content": [
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "a form"}]}]},
                    {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "a session"}]}]}
                ]}
            ]},
            "comment": {"comments": [
                {"author": {"displayName": "Alice"}, "created": "2026-10-01T10:00:00.000+0000", "body": document("Started")}
            ], "total": 1}
        }
    })
}

fn jira(user: &User, items: Value) {
    user.acli_script(json!({"items": items}));
}

fn ticket(user: &User) -> Value {
    user.call(
        "workspace.ticket",
        json!({"project": "app", "name": "login"}),
    )
    .unwrap()
}

fn link(user: &User, ticket: Option<&str>) -> Result<Value, (String, String)> {
    user.call(
        "workspace.link_ticket",
        json!({"project": "app", "name": "login", "ticket": ticket}),
    )
}

/// The calls of a fake, each as one line of its words.
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

/// An agent started in the workspace (the project folder for none): its MCP session.
fn agent_in(user: &User, workspace: Option<&str>) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "workspace": workspace, "message": "hello"}),
    )
    .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

#[test]
fn a_workspace_is_linked_to_a_ticket_at_its_creation_and_later_and_unlinked() {
    let user = User::new();
    fixture(&user);

    // At its creation, from the command line, the number typed with or without its hash.
    let output = user.run(&[
        "workspace",
        "create",
        "app",
        "second",
        "--offline",
        "--ticket",
        "123",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let made: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(made["ticket"], "#123");

    // Later, a Jira key in lower case, kept in upper case.
    assert_eq!(made["name"], "second");
    let linked = link(&user, Some("abc-7")).unwrap();
    assert_eq!(linked["ticket"], "ABC-7");
    let listed = user.call("workspace.list", json!({})).unwrap();
    let tickets: Vec<(&str, &Value)> = listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["name"].as_str().unwrap(), &w["ticket"]))
        .collect();
    assert_eq!(
        tickets,
        [("login", &json!("ABC-7")), ("second", &json!("#123"))]
    );

    // From the command line, then unlinked.
    let output = user.run(&["workspace", "ticket", "app", "login", "#9"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("Linked.\n"));
    assert_eq!(ticket(&user)["ticket"], "#9");
    let output = user.run(&["workspace", "ticket", "app", "login", "--clear"]);
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("is linked to no ticket"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(ticket(&user)["ticket"], Value::Null);
    assert!(
        !user
            .run(&["workspace", "ticket", "app", "login"])
            .status
            .success()
    );

    // Each change is an event.
    let output = user
        .command(&["watch", "--after", "0"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = output;
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    let said = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        said.matches("\"name\":\"workspace.ticket\"").count(),
        3,
        "{said}"
    );
}

#[test]
fn a_reference_that_is_no_ticket_is_refused_at_creation_and_when_linking() {
    let user = User::new();
    fixture(&user);

    for bad in ["", "--web", "ABC", "#0", "ABC-1 --web", "x"] {
        let error = link(&user, Some(bad)).unwrap_err();
        assert_eq!(error.0, "refused", "{bad}: {error:?}");
        assert!(
            error.1.contains("is not a ticket") || error.1.contains("not an issue"),
            "{error:?}"
        );
    }
    let error = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "other", "ticket": "nope"}),
        )
        .unwrap_err();
    assert_eq!(error.0, "refused");
    assert!(
        user.call("workspace.show", json!({"project": "app", "name": "other"}))
            .is_err(),
        "nothing was made"
    );
    assert_eq!(ticket(&user)["ticket"], Value::Null);
}

#[test]
fn a_github_issue_is_read_with_its_title_state_description_and_comments() {
    let user = User::new().with_fake_gh();
    fixture(&user);
    issues(&user, json!({"12": issue(12, "The login fails")}));
    link(&user, Some("#12")).unwrap();
    // Linking answers with the ticket read: that is one read, the next one is another.
    let linked = user.gh_received().len();

    let read = ticket(&user);

    assert_eq!(read["problem"], Value::Null, "{read}");
    assert_eq!(read["tracker"], "github");
    // The first repository on GitHub: `lib`, first of the workspace, is on another host.
    assert_eq!(read["repository"], "acme/api");
    assert_eq!(read["title"], "The login fails");
    assert_eq!(read["state"], "open");
    assert_eq!(read["url"], "https://github.com/acme/api/issues/12");
    assert_eq!(read["description"], "Users cannot sign in.\nSee the form.");
    assert_eq!(read["comments"][0]["author"], "alice");
    assert_eq!(read["comments"][1]["body"], "Fixed on a branch");
    assert_eq!(
        calls(user.gh_received())[linked..],
        [
            "auth status --hostname github.com",
            "issue view 12 -R acme/api --json number,title,state,body,comments,url"
        ]
    );
}

#[test]
fn a_jira_work_item_is_read_through_acli_and_its_description_comes_as_plain_text() {
    let user = User::new().with_fake_acli();
    fixture(&user);
    jira(
        &user,
        json!({"ABC-123": work_item("ABC-123", "Add the login page")}),
    );
    link(&user, Some("ABC-123")).unwrap();
    let linked = user.acli_received().len();

    let read = ticket(&user);

    assert_eq!(read["problem"], Value::Null, "{read}");
    assert_eq!(read["tracker"], "jira");
    assert_eq!(read["repository"], Value::Null);
    assert_eq!(read["title"], "Add the login page");
    assert_eq!(read["state"], "In Progress");
    assert_eq!(read["url"], "https://acme.atlassian.net/browse/ABC-123");
    assert_eq!(
        read["description"],
        "Let users log in.\n\n- a form\n- a session"
    );
    assert_eq!(read["comments"][0]["author"], "Alice");
    assert_eq!(read["comments"][0]["body"], "Started");
    assert_eq!(
        calls(user.acli_received())[linked..],
        [
            "jira auth status",
            "jira workitem view ABC-123 --json --fields summary,status,description,comment"
        ]
    );
}

#[test]
fn workspace_show_holds_the_ticket_in_its_text_and_its_json() {
    let user = User::new().with_fake_gh();
    fixture(&user);
    issues(&user, json!({"12": issue(12, "The login fails")}));
    link(&user, Some("#12")).unwrap();

    let text = String::from_utf8_lossy(&user.run(&["workspace", "show", "app", "login"]).stdout)
        .into_owned();
    let output = user.run(&["workspace", "show", "app", "login", "--json"]);
    let shown: Value = serde_json::from_slice(&output.stdout).unwrap();

    for said in [
        "Ticket #12, open: The login fails\n  https://github.com/acme/api/issues/12\n",
        "  Description:\n    Users cannot sign in.\n    See the form.\n",
        "  Comments (2):\n    - alice, 2026-10-01T10:00:00Z: I can reproduce\n",
    ] {
        assert!(text.contains(said), "{said} in {text}");
    }
    assert_eq!(shown["ticket"], "#12");
    assert_eq!(shown["ticket_read"]["title"], "The login fails");

    // A workspace with no ticket asks nothing of the tracker.
    link(&user, None).unwrap();
    let calls = user.gh_received().len();
    let text = String::from_utf8_lossy(&user.run(&["workspace", "show", "app", "login"]).stdout)
        .into_owned();
    assert!(!text.contains("Ticket"), "{text}");
    assert_eq!(user.gh_received().len(), calls);
}

#[test]
fn what_a_tracker_says_is_masked_like_any_output() {
    let token = "ghp_fakefakefakefakefakefakefake0079";
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fake_acli()
        .with_env("GH_TOKEN", token);
    fixture(&user);
    let mut leaking = issue(12, &format!("Leak {token}"));
    leaking["comments"][0]["body"] = json!(format!("my token is {token}"));
    issues(&user, json!({"12": leaking}));
    let mut item = work_item("ABC-1", "Jira");
    item["fields"]["description"] = document(&format!("use {token} here"));
    jira(&user, json!({"ABC-1": item}));
    let mcp = {
        link(&user, Some("#12")).unwrap();
        agent_in(&user, Some("login"))
    };

    let github = ticket(&user);
    let tool = mcp.call("ticket_read", json!({}));
    link(&user, Some("ABC-1")).unwrap();
    let jira = ticket(&user);

    assert!(!github.to_string().contains(token), "{github}");
    assert_eq!(github["title"], "Leak [masked]");
    assert_eq!(github["comments"][0]["body"], "my token is [masked]");
    assert!(!tool.text.contains(token), "{}", tool.text);
    assert!(tool.text.contains("Leak [masked]"), "{}", tool.text);
    assert!(!jira.to_string().contains(token), "{jira}");
    assert_eq!(jira["description"], "use [masked] here");
}

#[test]
fn a_missing_or_signed_out_cli_is_said_with_what_to_do_and_nothing_else_breaks() {
    // A PATH with git and nothing else: no `gh` and no `acli` at all, the machine's own included.
    let user = User::new();
    std::fs::create_dir(user.path("bin")).unwrap();
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    let path = user.path("bin");
    let user = user.with_env("PATH", path.to_str().unwrap());
    fixture(&user);

    link(&user, Some("#12")).unwrap();
    let github = ticket(&user);
    link(&user, Some("ABC-1")).unwrap();
    let jira = ticket(&user);

    let said = github["problem"].as_str().expect("a problem is said");
    assert!(
        said.contains("gh, GitHub's command line, is not on"),
        "{said}"
    );
    assert!(said.contains("gh auth login"), "{said}");
    let said = jira["problem"].as_str().expect("a problem is said");
    assert!(
        said.contains("acli, Atlassian's command line, is not on"),
        "{said}"
    );
    assert!(said.contains("acli jira auth login"), "{said}");
    assert!(said.contains("the rest of Flottille works"), "{said}");
    // The rest works: the workspace is shown, by the daemon and by the command line.
    let output = user.run(&["workspace", "show", "app", "login"]);
    assert!(output.status.success());
    let shown = String::from_utf8_lossy(&output.stdout);
    assert!(
        shown.contains("Ticket ABC-1: it could not be read: acli"),
        "{shown}"
    );
    assert!(shown.contains("Steps:"), "{shown}");

    // Signed out: `acli jira auth status` ends with 1, and the work item is never asked.
    let user = User::new().with_fake_gh().with_fake_acli();
    fixture(&user);
    user.acli_script(json!({"signed_in": false}));
    user.gh_script(json!({"signed_in": false}));
    let jira = link(&user, Some("ABC-1")).unwrap();
    let github = link(&user, Some("#12")).unwrap();
    assert!(
        jira["problem"]
            .as_str()
            .unwrap()
            .contains("acli jira auth login")
    );
    assert_eq!(calls(user.acli_received()), ["jira auth status"]);
    assert!(
        github["problem"]
            .as_str()
            .unwrap()
            .contains("gh auth login")
    );
    assert_eq!(
        calls(user.gh_received()),
        ["auth status --hostname github.com"]
    );
}

#[test]
fn a_ticket_the_tracker_does_not_know_or_a_workspace_with_no_github_repository_says_so() {
    let user = User::new().with_fake_gh().with_fake_acli();
    let folder = fixture(&user);
    issues(&user, json!({}));
    jira(&user, json!({}));

    link(&user, Some("#99")).unwrap();
    let github = ticket(&user);
    link(&user, Some("ABC-99")).unwrap();
    let jira = ticket(&user);
    assert!(
        github["problem"]
            .as_str()
            .unwrap()
            .contains("Could not resolve"),
        "{github}"
    );
    assert!(
        jira["problem"].as_str().unwrap().contains("does not exist"),
        "{jira}"
    );

    // No repository of the workspace on GitHub: no issue to read.
    user.git(
        &folder.join("api"),
        &[
            "remote",
            "set-url",
            "origin",
            "https://git.example.com/a/api.git",
        ],
    );
    link(&user, Some("#1")).unwrap();
    let none = ticket(&user);
    assert!(
        none["problem"].as_str().unwrap().contains("is on GitHub"),
        "{none}"
    );
}

#[test]
fn ticket_read_returns_the_workspace_s_ticket_or_another_of_its_tracker_and_never_writes() {
    let user = User::with_fake_agent().with_fake_gh().with_fake_acli();
    fixture(&user);
    issues(
        &user,
        json!({"12": issue(12, "The login fails"), "13": issue(13, "The logout fails")}),
    );
    jira(
        &user,
        json!({"ABC-123": work_item("ABC-123", "Add the login page")}),
    );
    link(&user, Some("#12")).unwrap();
    let mcp = agent_in(&user, Some("login"));

    assert!(mcp.names().contains(&"ticket_read".to_owned()));
    let own = mcp.call("ticket_read", json!({}));
    assert!(!own.error, "{}", own.text);
    for said in [
        "Ticket #12 of acme/api (GitHub), open: The login fails",
        "https://github.com/acme/api/issues/12",
        "Users cannot sign in.",
        "- alice, 2026-10-01T10:00:00Z: I can reproduce",
        "- bob, 2026-10-02T09:30:00Z: Fixed on a branch",
    ] {
        assert!(own.text.contains(said), "{said} in {}", own.text);
    }

    // Another ticket of the same tracker, by its key.
    let other = mcp.call("ticket_read", json!({"ticket": "13"}));
    assert!(!other.error, "{}", other.text);
    assert!(
        other
            .text
            .contains("Ticket #13 of acme/api (GitHub), open: The logout fails")
    );

    // Another tracker's: refused, and never asked.
    let jira_calls = user.acli_received().len();
    let refused = mcp.call("ticket_read", json!({"ticket": "ABC-123"}));
    assert!(refused.error);
    assert!(
        refused.text.starts_with(
            "refused: ABC-123 is a Jira ticket, and this workspace's tracker is GitHub issues"
        ),
        "{}",
        refused.text
    );
    assert_eq!(user.acli_received().len(), jira_calls);
    let refused = mcp.call("ticket_read", json!({"ticket": "--web"}));
    assert!(
        refused.error && refused.text.starts_with("refused:"),
        "{}",
        refused.text
    );

    // A Jira workspace reads Jira, and not GitHub.
    link(&user, Some("ABC-123")).unwrap();
    let jira_agent = agent_in(&user, Some("login"));
    let own = jira_agent.call("ticket_read", json!({}));
    assert!(!own.error, "{}", own.text);
    assert!(
        own.text
            .contains("Ticket ABC-123 (Jira), In Progress: Add the login page"),
        "{}",
        own.text
    );
    assert!(own.text.contains("- a form"), "{}", own.text);
    let github_calls = user.gh_received().len();
    let refused = jira_agent.call("ticket_read", json!({"ticket": "#12"}));
    assert!(refused.error, "{}", refused.text);
    assert_eq!(user.gh_received().len(), github_calls);

    // No ticket linked: nothing to read by default, and GitHub's issues of its repositories by key.
    link(&user, None).unwrap();
    let none = agent_in(&user, Some("login"));
    let refused = none.call("ticket_read", json!({}));
    assert!(refused.error);
    assert!(
        refused.text.contains("is linked to no ticket"),
        "{}",
        refused.text
    );
    assert!(!none.call("ticket_read", json!({"ticket": "#13"})).error);
    assert!(none.call("ticket_read", json!({"ticket": "ABC-123"})).error);

    // Read only: every call either CLI got is one of the reads, and the gate held nothing.
    for call in calls(user.acli_received()) {
        assert!(
            call == "jira auth status" || call.starts_with("jira workitem view "),
            "{call}"
        );
    }
    for call in calls(user.gh_received()) {
        assert!(
            call.starts_with("auth status") || call.starts_with("issue view "),
            "{call}"
        );
    }
    let held = user.call("approval.list", json!({})).unwrap();
    assert!(held["requests"].as_array().unwrap().is_empty());
}

#[test]
fn an_agent_in_the_project_folder_has_no_ticket_read() {
    let user = User::with_fake_agent().with_fake_gh();
    fixture(&user);
    let mcp = agent_in(&user, None);

    assert!(!mcp.names().contains(&"ticket_read".to_owned()));
}

#[test]
fn an_agent_cannot_run_acli_by_itself_the_gate_refuses_it() {
    let user = User::with_fake_agent().with_fake_acli();
    fixture(&user);
    jira(&user, json!({"ABC-1": work_item("ABC-1", "Jira")}));
    let mcp = agent_in(&user, Some("login"));

    for line in [
        "acli jira workitem view ABC-1 --json",
        "acli jira workitem comment create --key ABC-1 --body hi",
    ] {
        let answer = mcp.call("commands_run", json!({"line": line}));
        assert!(answer.error, "{line}: {}", answer.text);
        assert!(answer.text.contains("acli"), "{}", answer.text);
    }
    assert!(user.acli_received().is_empty(), "the fake was never run");
}
