//! The review threads of a pull request (#359, `docs/forges.md`, "Review threads"): `forge show`
//! with `detail: full` gives each comment on a line with its file, line, side, replies, and
//! whether it is resolved or outdated; `forge reply {number, thread, body}` answers in one and
//! `forge resolve {number, thread, resolved}` marks it, both judged like `comment`. The forge is
//! the fake `gh` and `bkt`; the remotes are throwaway bare repositories.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A project `app` of one repository on `origin` (a throwaway bare remote whose address is then
/// the forge's), its ready workspace `login` on the branch `app/login`, pushed.
fn project(user: &User, origin: &str, forge: &str) {
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
    user.git(&folder, &["remote", "set-url", "origin", origin]);
    user.write_project_file(
        "app",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \".\"\nforge = {forge}\n",
            folder.display()
        ),
    );
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
}

fn on_github(user: &User) {
    project(
        user,
        "git@github.com:acme/app.git",
        "{ kind = \"github\", repository = \"acme/app\" }",
    );
    user.gh_script(github_script());
}

fn on_cloud(user: &User) {
    project(
        user,
        "git@bitbucket.org:acme/app.git",
        "{ kind = \"bitbucket-cloud\", repository = \"acme/app\" }",
    );
    user.bkt_script(cloud_script());
}

fn on_data_center(user: &User) {
    project(
        user,
        "ssh://git@bitbucket.example.com:7999/APP/app.git",
        "{ kind = \"bitbucket-data-center\", host = \"bitbucket.example.com\", \
         repository = \"APP/app\" }",
    );
    user.bkt_script(data_center_script());
}

/// A root agent started in the workspace `login`.
fn agent(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "workspace": "login", "message": "hello"}),
    )
    .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The request number of a call the gate holds.
fn held(answer: &Answer) -> u64 {
    assert!(!answer.error, "{}", answer.text);
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// What the user reads on the card of the request `number`.
fn says(user: &User, number: u64) -> Vec<String> {
    user.call("approval.list", json!({}))
        .unwrap()
        .get("requests")
        .and_then(Value::as_array)
        .and_then(|all| all.iter().find(|r| r["number"] == number))
        .cloned()
        .unwrap_or_else(|| panic!("no pending request #{number}"))["says"]
        .as_array()
        .unwrap()
        .iter()
        .map(|said| said.as_str().unwrap().to_owned())
        .collect()
}

/// A call the gate holds, allowed by the user: the answer the agent is handed for it.
fn allowed(user: &User, number: u64) -> Answer {
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let store = || Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || store().delivery(&id).unwrap().is_some());
    let text = store().delivery(&id).unwrap().unwrap().body;
    Answer {
        error: !text.contains("and Flottille has now done it"),
        text,
    }
}

/// The write `asked`, held, then allowed by the user.
fn written(user: &User, mcp: &Mcp, asked: Value) -> Answer {
    let number = held(&mcp.call("forge", asked));
    allowed(user, number)
}

fn args_of(call: &Value) -> Vec<String> {
    call["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_owned())
        .collect()
}

/// The calls of `gh api graphql`, each as its arguments.
fn graphql(user: &User) -> Vec<Vec<String>> {
    user.gh_received()
        .iter()
        .map(args_of)
        .filter(|args| args.len() > 1 && args[0] == "api" && args[1] == "graphql")
        .collect()
}

/// A GraphQL call's field `key` (`-f key=value` or `-F key=value`).
fn field<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
    args.windows(2)
        .filter(|pair| pair[0] == "-f" || pair[0] == "-F")
        .find_map(|pair| pair[1].strip_prefix(&format!("{key}=")))
}

/// The GraphQL mutations `gh` was asked, by name.
fn mutations(user: &User) -> Vec<String> {
    graphql(user)
        .iter()
        .filter_map(|args| {
            let query = field(args, "query")?;
            [
                "addPullRequestReviewThreadReply",
                "unresolveReviewThread",
                "resolveReviewThread",
            ]
            .into_iter()
            .find(|name| query.contains(name))
            .map(str::to_owned)
        })
        .collect()
}

/// The `bkt api` calls with a method, each as `METHOD path` and the JSON of its `--input`.
fn bkt_writes(user: &User) -> Vec<(String, Value)> {
    user.bkt_received()
        .iter()
        .map(args_of)
        .filter(|args| args.first().map(String::as_str) == Some("api"))
        .filter_map(|args| {
            let option = |name: &str| {
                let at = args.iter().position(|a| a == name)?;
                args.get(at + 1).cloned()
            };
            let method = option("--method")?;
            let input = option("--input")
                .map(|input| serde_json::from_str(&input).unwrap())
                .unwrap_or(Value::Null);
            Some((format!("{method} {}", args[1]), input))
        })
        .collect()
}

fn show(mcp: &Mcp) -> String {
    let answer = mcp.call(
        "forge",
        json!({"action": "show", "repository": ".", "number": 12, "detail": "full"}),
    );
    assert!(!answer.error, "{}", answer.text);
    answer.text
}

fn reply(thread: Value) -> Value {
    json!({"action": "reply", "repository": ".", "number": 12, "thread": thread,
        "body": "Renamed."})
}

fn resolve(thread: Value, resolved: bool) -> Value {
    json!({"action": "resolve", "repository": ".", "number": 12, "thread": thread,
        "resolved": resolved})
}

/// GitHub's pull request #12 of `app/login`, open, with two review threads: one on a line of the
/// new file, unresolved, with a reply; one outdated and resolved on lines of the old file. A
/// thread of #13 too.
fn github_script() -> Value {
    let comment = |login: &str, body: &str, hour: u32, id: u32| {
        json!({"author": {"login": login}, "body": body,
            "createdAt": format!("2026-11-01T{hour}:00:00Z"),
            "url": format!("https://github.com/acme/app/pull/12#discussion_r{id}")})
    };
    json!({
        "pulls": {"acme/app": {
            "app/login": {"number": 12, "title": "Login", "state": "OPEN", "isDraft": false,
                "baseRefName": "main", "headRefName": "app/login",
                "headRefOid": "0a1b2c3d4e5f60718293a4b5c6d7e8f901234567",
                "url": "https://github.com/acme/app/pull/12", "comments": [], "reviews": []},
            "app/other": {"number": 13, "title": "Other", "state": "OPEN", "isDraft": false,
                "baseRefName": "main", "headRefName": "app/other",
                "url": "https://github.com/acme/app/pull/13"}}},
        "threads": {"acme/app": {
            "12": [
                {"id": "PRRT_open", "isResolved": false, "isOutdated": false,
                    "path": "src/login.rs", "line": 14, "startLine": null, "originalLine": 14,
                    "originalStartLine": null, "diffSide": "RIGHT",
                    "comments": {"nodes": [comment("alice", "Rename this.", 10, 101),
                        comment("bob", "Agreed.", 11, 102)]}},
                {"id": "PRRT_old", "isResolved": true, "isOutdated": true,
                    "path": "README.md", "line": null, "startLine": null, "originalLine": 3,
                    "originalStartLine": 2, "diffSide": "LEFT",
                    "comments": {"nodes": [comment("carol", "Typo.", 12, 103)]}}
            ],
            "13": [
                {"id": "PRRT_elsewhere", "isResolved": false, "isOutdated": false,
                    "path": "src/other.rs", "line": 1, "diffSide": "RIGHT",
                    "comments": {"nodes": []}}
            ]
        }}
    })
}

/// Bitbucket Cloud's pull request #12 of `app/login`: an inline comment on the new file's line 14
/// with a reply, an outdated and resolved one on the old file's line 3, and a general comment.
fn cloud_script() -> Value {
    let user = |name: &str| json!({"display_name": name, "nickname": name.to_lowercase()});
    json!({
        "contexts": [{"name": "cloud", "host": "bitbucket.org", "active": true}],
        "pulls": {"acme/app": [{"id": 12, "title": "Login", "state": "OPEN",
            "source": {"branch": {"name": "app/login"}, "repository": {"full_name": "acme/app"}},
            "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}},
            "links": {"html": {"href": "https://bitbucket.org/acme/app/pull-requests/12"}}}]},
        "api": {
            "/repositories/acme/app/pullrequests/12": {"id": 12, "state": "OPEN",
                "participants": []},
            "/repositories/acme/app/pullrequests/12/comments": {"values": [
                {"id": 101, "content": {"raw": "Rename this."}, "user": user("Alice"),
                    "created_on": "2026-11-01T10:00:00+00:00", "deleted": false,
                    "inline": {"path": "src/login.rs", "from": null, "to": 14}},
                {"id": 102, "content": {"raw": "Agreed."}, "user": user("Bob"),
                    "created_on": "2026-11-01T11:00:00+00:00", "deleted": false,
                    "parent": {"id": 101},
                    "inline": {"path": "src/login.rs", "from": null, "to": 14}},
                {"id": 103, "content": {"raw": "Typo."}, "user": user("Carol"),
                    "created_on": "2026-11-01T12:00:00+00:00", "deleted": false,
                    "inline": {"path": "README.md", "from": 3, "to": null, "outdated": true},
                    "resolution": {"type": "comment_resolution", "user": user("Carol"),
                        "created_on": "2026-11-01T12:30:00+00:00"}},
                {"id": 104, "content": {"raw": "Looks good overall."}, "user": user("Dan"),
                    "created_on": "2026-11-01T13:00:00+00:00", "deleted": false}
            ]}
        }
    })
}

/// Milliseconds of 2026-11-01 at `hour`:00 UTC.
fn at(hour: u64) -> u64 {
    1_793_527_200_000 + (hour - 10) * 3_600_000
}

/// Bitbucket Data Center's pull request #12 of `APP/app`, with the same threads as Cloud's: a
/// comment on the new file's line 14 with a reply, an orphaned and resolved one on a removed line,
/// and a general comment.
fn data_center_script() -> Value {
    let repository = json!({"slug": "app", "project": {"key": "APP"}});
    let author = |name: &str| json!({"displayName": name, "name": name.to_lowercase()});
    json!({
        "contexts": [{"name": "dc", "host": "bitbucket.example.com"}],
        "pulls": {"APP/app": [{"id": 12, "title": "Login", "state": "OPEN", "version": 1,
            "author": {"user": author("Alice")},
            "fromRef": {"id": "refs/heads/app/login", "displayId": "app/login",
                "latestCommit": "0a1b2c3d4e5f60718293a4b5c6d7e8f901234567",
                "repository": repository},
            "toRef": {"id": "refs/heads/main", "displayId": "main", "repository": repository},
            "reviewers": [], "participants": [],
            "links": {"self": [{"href":
                "https://bitbucket.example.com/projects/APP/repos/app/pull-requests/12"}]}}]},
        "api": {
            "/rest/api/1.0/projects/APP/repos/app/pull-requests/12/activities": {"values": [
                {"action": "COMMENTED", "commentAction": "ADDED",
                    "comment": {"id": 201, "version": 0, "text": "Rename this.",
                        "author": author("Alice"), "createdDate": at(10), "severity": "NORMAL",
                        "state": "OPEN", "threadResolved": false,
                        "comments": [{"id": 202, "version": 0, "text": "Agreed.",
                            "author": author("Bob"), "createdDate": at(11), "comments": []}]},
                    "commentAnchor": {"path": "src/login.rs", "line": 14, "lineType": "ADDED",
                        "fileType": "TO", "orphaned": false, "diffType": "EFFECTIVE"}},
                {"action": "COMMENTED", "commentAction": "ADDED",
                    "comment": {"id": 203, "version": 2, "text": "Typo.",
                        "author": author("Carol"), "createdDate": at(12), "severity": "NORMAL",
                        "state": "OPEN", "threadResolved": true, "comments": []},
                    "commentAnchor": {"path": "README.md", "line": 3, "lineType": "REMOVED",
                        "fileType": "FROM", "orphaned": true, "diffType": "EFFECTIVE"}},
                {"action": "COMMENTED", "commentAction": "ADDED",
                    "comment": {"id": 204, "version": 0, "text": "Looks good overall.",
                        "author": author("Dan"), "createdDate": at(13), "comments": []}}
            ]},
            "/rest/api/1.0/projects/APP/repos/app/pull-requests/12/comments/201": {"id": 201,
                "version": 0, "text": "Rename this.", "severity": "NORMAL", "state": "OPEN",
                "threadResolved": false},
            "/rest/api/1.0/projects/APP/repos/app/pull-requests/12/comments/203": {"id": 203,
                "version": 2, "text": "Typo.", "severity": "NORMAL", "state": "OPEN",
                "threadResolved": true}
        }
    })
}

/// What `show` says of the two threads of every script: the open one on the new file's line 14,
/// its comments in order with their author and date, the other resolved and outdated on the old
/// file.
fn assert_threads_shown(text: &str, open: &str, old: &str, (alice, bob): (&str, &str)) {
    for said in [
        format!("thread {open} on src/login.rs line 14, new side, unresolved:"),
        format!("    - {alice}, 2026-11-01T10:00:00Z: Rename this."),
        format!("    - {bob}, 2026-11-01T11:00:00Z: Agreed."),
        format!("thread {old} on README.md"),
    ] {
        assert!(text.contains(&said), "{said:?} in {text}");
    }
    let first = text.find("Rename this.").unwrap();
    assert!(first < text.find("Agreed.").unwrap(), "{text}");
}

#[test]
fn an_inline_comment_comes_with_its_file_and_line_on_github() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let mcp = agent(&user);

    let text = show(&mcp);

    assert_threads_shown(&text, "PRRT_open", "PRRT_old", ("alice", "bob"));
    assert!(
        text.contains("thread PRRT_old on README.md lines 2 to 3, old side, resolved"),
        "{text}"
    );
    assert!(text.contains("review threads (2, 1 unresolved"), "{text}");
    // The pull request's threads, of the declared repository and number.
    let read = graphql(&user);
    assert_eq!(read.len(), 1, "{read:?}");
    assert_eq!(field(&read[0], "owner"), Some("acme"));
    assert_eq!(field(&read[0], "name"), Some("app"));
    assert_eq!(field(&read[0], "number"), Some("12"));
    assert!(
        read[0]
            .windows(2)
            .any(|p| p == ["--hostname", "github.com"])
    );
}

#[test]
fn an_inline_comment_comes_with_its_file_and_line_on_bitbucket_cloud() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_cloud(&user);
    let mcp = agent(&user);

    let text = show(&mcp);

    assert_threads_shown(&text, "101", "103", ("Alice", "Bob"));
    assert!(
        text.contains("thread 103 on README.md line 3, old side, resolved"),
        "{text}"
    );
    // The general comment stays among the comments; a thread's are said in their thread alone.
    assert!(
        text.contains("comments (1):\n  - Dan: Looks good overall."),
        "{text}"
    );
    assert_eq!(text.matches("Rename this.").count(), 1, "{text}");
}

#[test]
fn an_inline_comment_comes_with_its_file_and_line_on_bitbucket_data_center() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mcp = agent(&user);

    let text = show(&mcp);

    assert_threads_shown(&text, "201", "203", ("Alice", "Bob"));
    assert!(
        text.contains("thread 203 on README.md line 3, old side, resolved"),
        "{text}"
    );
    assert!(
        text.contains("comments (1):\n  - Dan: Looks good overall."),
        "{text}"
    );
}

#[test]
fn a_short_show_counts_the_threads_it_read() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_cloud(&user);
    let mcp = agent(&user);

    let answer = mcp.call(
        "forge",
        json!({"action": "show", "repository": ".", "number": 12}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("review threads: 2, 1 unresolved, read them with forge"),
        "{}",
        answer.text
    );
}

#[test]
fn an_outdated_thread_is_said_so_on_github() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let mcp = agent(&user);

    let text = show(&mcp);

    assert!(
        text.contains(
            "thread PRRT_old on README.md lines 2 to 3, old side, resolved, outdated: the code \
             changed since, the line is where it was written:"
        ),
        "{text}"
    );
    let open = text
        .lines()
        .find(|l| l.contains("thread PRRT_open"))
        .unwrap();
    assert!(!open.contains("outdated"), "{open}");
}

#[test]
fn an_outdated_thread_is_said_so_on_bitbucket_cloud() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_cloud(&user);
    let mcp = agent(&user);

    let text = show(&mcp);

    assert!(
        text.contains("thread 103 on README.md line 3, old side, resolved, outdated"),
        "{text}"
    );
    let open = text.lines().find(|l| l.contains("thread 101")).unwrap();
    assert!(!open.contains("outdated"), "{open}");
}

#[test]
fn an_outdated_thread_is_said_so_on_bitbucket_data_center() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mcp = agent(&user);

    let text = show(&mcp);

    assert!(
        text.contains("thread 203 on README.md line 3, old side, resolved, outdated"),
        "{text}"
    );
    let open = text.lines().find(|l| l.contains("thread 201")).unwrap();
    assert!(!open.contains("outdated"), "{open}");
}

#[test]
fn a_reply_goes_into_its_thread_on_github() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let mcp = agent(&user);

    let number = held(&mcp.call("forge", reply(json!("PRRT_open"))));
    let card = says(&user, number);
    for said in [
        "It replies in the review thread PRRT_open of pull request #12",
        "Its body: Renamed.",
    ] {
        assert!(card.contains(&said.to_owned()), "{said}: {card:?}");
    }
    assert!(mutations(&user).is_empty(), "posted before the answer");
    let answer = allowed(&user, number);

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains(
            "The reply was posted in the review thread PRRT_open on src/login.rs line 14, new \
             side of the pull request #12 of acme/app."
        ),
        "{}",
        answer.text
    );
    assert_eq!(mutations(&user), ["addPullRequestReviewThreadReply"]);
    let sent = graphql(&user).pop().unwrap();
    assert_eq!(field(&sent, "thread"), Some("PRRT_open"));
    assert_eq!(field(&sent, "body"), Some("Renamed."));

    // Read back in its thread, the reply is the agent's.
    let mut script = github_script();
    script["threads"]["acme/app"]["12"][0]["comments"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"author": {"login": "someone"}, "body": "Renamed.",
            "createdAt": "2026-11-01T14:00:00Z",
            "url": "https://github.com/acme/app/pull/12#discussion_r1"}));
    user.gh_script(script);
    let text = show(&mcp);
    assert!(
        text.contains("    - someone (posted by an agent), 2026-11-01T14:00:00Z: Renamed."),
        "{text}"
    );
}

#[test]
fn a_reply_goes_into_its_thread_on_bitbucket_cloud() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_cloud(&user);
    let mcp = agent(&user);

    let answer = written(&user, &mcp, reply(json!(101)));

    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_writes(&user),
        [(
            "POST /repositories/acme/app/pullrequests/12/comments".to_owned(),
            json!({"content": {"raw": "Renamed."}, "parent": {"id": 101}})
        )]
    );
    let mut script = cloud_script();
    script["api"]["/repositories/acme/app/pullrequests/12/comments"]["values"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": 1, "content": {"raw": "Renamed."},
            "user": {"display_name": "Someone"}, "created_on": "2026-11-01T14:00:00+00:00",
            "deleted": false, "parent": {"id": 102},
            "inline": {"path": "src/login.rs", "from": null, "to": 14}}));
    user.bkt_script(script);
    let text = show(&mcp);
    let thread = &text[text.find("thread 101").unwrap()..text.find("thread 103").unwrap()];
    assert!(
        thread.contains("    - Someone (posted by an agent), 2026-11-01T14:00:00Z: Renamed."),
        "{text}"
    );
}

#[test]
fn a_reply_goes_into_its_thread_on_bitbucket_data_center() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mcp = agent(&user);

    let answer = written(&user, &mcp, reply(json!("201")));

    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_writes(&user),
        [(
            "POST /rest/api/1.0/projects/APP/repos/app/pull-requests/12/comments".to_owned(),
            json!({"text": "Renamed.", "parent": {"id": 201}})
        )]
    );
}

#[test]
fn a_thread_is_resolved_and_unresolved_on_github() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let mcp = agent(&user);

    let number = held(&mcp.call("forge", resolve(json!("PRRT_open"), true)));
    let card = says(&user, number);
    assert!(
        card.contains(&"It marks the review thread PRRT_open of pull request #12 resolved".into()),
        "{card:?}"
    );
    let resolved = allowed(&user, number);
    let unresolved = written(&user, &mcp, resolve(json!("PRRT_old"), false));

    assert!(!resolved.error, "{}", resolved.text);
    assert!(
        resolved.text.contains("is now resolved"),
        "{}",
        resolved.text
    );
    assert!(!unresolved.error, "{}", unresolved.text);
    assert!(
        unresolved.text.contains("is now unresolved"),
        "{}",
        unresolved.text
    );
    assert_eq!(
        mutations(&user),
        ["resolveReviewThread", "unresolveReviewThread"]
    );
    let sent: Vec<Option<String>> = graphql(&user)
        .iter()
        .filter(|args| field(args, "query").is_some_and(|q| q.starts_with("mutation")))
        .map(|args| field(args, "thread").map(str::to_owned))
        .collect();
    assert_eq!(
        sent,
        [Some("PRRT_open".to_owned()), Some("PRRT_old".to_owned())]
    );
}

#[test]
fn a_thread_already_resolved_is_said_so_and_nothing_is_changed_on_github() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let mcp = agent(&user);

    let answer = written(&user, &mcp, resolve(json!("PRRT_old"), true));

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("was already resolved"),
        "{}",
        answer.text
    );
    assert!(mutations(&user).is_empty(), "{:?}", mutations(&user));
}

#[test]
fn a_thread_of_another_pull_request_is_refused_and_nothing_is_written_on_github() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let mcp = agent(&user);

    let replied = written(&user, &mcp, reply(json!("PRRT_elsewhere")));
    let resolved = written(&user, &mcp, resolve(json!("PRRT_elsewhere"), true));

    for answer in [replied, resolved] {
        assert!(answer.error, "{}", answer.text);
        assert!(
            answer.text.contains("has no review thread PRRT_elsewhere")
                && answer.text.contains("nothing was written"),
            "{}",
            answer.text
        );
    }
    assert!(mutations(&user).is_empty(), "{:?}", mutations(&user));
}

#[test]
fn a_thread_is_resolved_and_unresolved_on_bitbucket_cloud() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_cloud(&user);
    let mcp = agent(&user);

    let resolved = written(&user, &mcp, resolve(json!(101), true));
    let unresolved = written(&user, &mcp, resolve(json!(103), false));

    assert!(!resolved.error, "{}", resolved.text);
    assert!(
        resolved.text.contains("is now resolved"),
        "{}",
        resolved.text
    );
    assert!(!unresolved.error, "{}", unresolved.text);
    assert!(
        unresolved.text.contains("is now unresolved"),
        "{}",
        unresolved.text
    );
    let writes: Vec<String> = bkt_writes(&user).into_iter().map(|(w, _)| w).collect();
    assert_eq!(
        writes,
        [
            "POST /repositories/acme/app/pullrequests/12/comments/101/resolve",
            "DELETE /repositories/acme/app/pullrequests/12/comments/103/resolve"
        ]
    );
}

#[test]
fn a_thread_is_resolved_and_unresolved_on_bitbucket_data_center() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mcp = agent(&user);

    let resolved = written(&user, &mcp, resolve(json!(201), true));
    let unresolved = written(&user, &mcp, resolve(json!(203), false));

    assert!(!resolved.error, "{}", resolved.text);
    assert!(!unresolved.error, "{}", unresolved.text);
    let path = "/rest/api/1.0/projects/APP/repos/app/pull-requests/12/comments";
    assert_eq!(
        bkt_writes(&user),
        [
            (
                format!("PUT {path}/201"),
                json!({"version": 0, "threadResolved": true})
            ),
            (
                format!("PUT {path}/203"),
                json!({"version": 2, "threadResolved": false})
            )
        ]
    );
}

#[test]
fn a_data_center_whose_comments_have_no_resolved_state_refuses_naming_why() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mut script = data_center_script();
    let path = "/rest/api/1.0/projects/APP/repos/app/pull-requests/12/comments/201";
    script["api"][path] = json!({"id": 201, "version": 0, "text": "Rename this.",
        "severity": "NORMAL", "state": "OPEN"});
    user.bkt_script(script);
    let mcp = agent(&user);

    let answer = written(&user, &mcp, resolve(json!(201), true));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("say no resolved state")
            && answer.text.contains("nothing was changed"),
        "{}",
        answer.text
    );
    assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_writes(&user));
}

#[test]
fn a_reply_or_a_resolve_missing_what_it_needs_is_refused_before_the_user_is_asked() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let mcp = agent(&user);

    for (asked, said) in [
        (
            json!({"action": "reply", "repository": ".", "number": 12, "body": "x"}),
            "needs thread",
        ),
        (
            json!({"action": "reply", "repository": ".", "number": 12, "thread": "PRRT_open"}),
            "needs body",
        ),
        (
            json!({"action": "resolve", "repository": ".", "number": 12, "thread": "PRRT_open"}),
            "needs resolved",
        ),
        (
            json!({"action": "reply", "repository": ".", "thread": "PRRT_open", "body": "x"}),
            "needs number",
        ),
        (
            json!({"action": "reply", "repository": ".", "number": 12, "thread": "--x",
                "body": "x"}),
            "is not a review thread's id",
        ),
        (
            json!({"action": "comment", "repository": ".", "number": 12, "thread": "PRRT_open",
                "body": "x"}),
            "takes no thread",
        ),
    ] {
        let answer = mcp.call("forge", asked.clone());
        assert!(answer.error, "{asked}: {}", answer.text);
        assert!(answer.text.contains(said), "{said}: {}", answer.text);
    }
    assert!(graphql(&user).is_empty(), "{:?}", graphql(&user));
}

/// A comment of Bitbucket Cloud's pull request #13, which is not one of #12's.
fn cloud_script_with_another_pull() -> Value {
    let mut script = cloud_script();
    script["api"]["/repositories/acme/app/pullrequests/13/comments"] = json!({"values": [
        {"id": 301, "content": {"raw": "Elsewhere."}, "user": {"display_name": "Eve"},
            "created_on": "2026-11-01T14:00:00+00:00", "deleted": false,
            "inline": {"path": "src/other.rs", "from": null, "to": 1}}
    ]});
    script
}

#[test]
fn a_thread_of_another_pull_request_is_refused_on_bitbucket_cloud() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_cloud(&user);
    user.bkt_script(cloud_script_with_another_pull());
    let mcp = agent(&user);

    // 301 is a comment of #13, 104 a general comment of #12, 102 a reply and not a thread.
    for thread in [301, 104, 102] {
        for asked in [reply(json!(thread)), resolve(json!(thread), true)] {
            let answer = written(&user, &mcp, asked);
            assert!(answer.error, "{thread}: {}", answer.text);
            assert!(
                answer
                    .text
                    .contains(&format!("has no review thread {thread}"))
                    && answer.text.contains("nothing was written"),
                "{thread}: {}",
                answer.text
            );
        }
    }
    assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_writes(&user));
}

#[test]
fn a_thread_of_another_pull_request_is_refused_on_bitbucket_data_center() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mut script = data_center_script();
    let path = "/rest/api/1.0/projects/APP/repos/app/pull-requests";
    // 301 is a comment of #13, 204 a general comment of #12, 202 a reply and not a thread.
    script["api"][format!("{path}/13/comments/301")] = json!({"id": 301, "version": 0,
        "text": "Elsewhere.", "severity": "NORMAL", "state": "OPEN", "threadResolved": false});
    for (id, text) in [(204, "Looks good overall."), (202, "Agreed.")] {
        script["api"][format!("{path}/12/comments/{id}")] = json!({"id": id, "version": 0,
            "text": text, "severity": "NORMAL", "state": "OPEN", "threadResolved": false});
    }
    user.bkt_script(script);
    let mcp = agent(&user);

    for thread in [301, 204, 202] {
        for asked in [reply(json!(thread)), resolve(json!(thread), true)] {
            let answer = written(&user, &mcp, asked);
            assert!(answer.error, "{thread}: {}", answer.text);
            assert!(
                answer
                    .text
                    .contains(&format!("has no review thread {thread}"))
                    && answer.text.contains("nothing was written"),
                "{thread}: {}",
                answer.text
            );
        }
    }
    assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_writes(&user));
}

#[test]
fn a_thread_already_resolved_is_said_so_and_nothing_is_changed_on_bitbucket_cloud() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_cloud(&user);
    let mcp = agent(&user);

    let answer = written(&user, &mcp, resolve(json!(103), true));

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("was already resolved"),
        "{}",
        answer.text
    );
    assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_writes(&user));
}

#[test]
fn a_thread_already_resolved_is_said_so_and_nothing_is_changed_on_bitbucket_data_center() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mcp = agent(&user);

    let answer = written(&user, &mcp, resolve(json!(203), true));

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("was already resolved"),
        "{}",
        answer.text
    );
    assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_writes(&user));
}

#[test]
fn a_bitbucket_thread_that_is_not_a_number_is_refused_before_the_user_is_asked() {
    for (user, script) in [
        (User::with_fake_agent().with_fake_bkt(), "cloud"),
        (User::with_fake_agent().with_fake_bkt(), "data center"),
    ] {
        match script {
            "cloud" => on_cloud(&user),
            _ => on_data_center(&user),
        }
        let mcp = agent(&user);
        for asked in [reply(json!("PRRT_open")), resolve(json!("abc"), true)] {
            let answer = mcp.call("forge", asked.clone());
            assert!(answer.error, "{script}: {asked}: {}", answer.text);
            assert!(
                answer.text.contains("is not a thread of Bitbucket")
                    && !answer.text.contains("Waiting for the user's approval"),
                "{script}: {}",
                answer.text
            );
        }
        assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_writes(&user));
    }
}

#[test]
fn a_thread_past_the_first_hundred_can_be_replied_to() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let mut script = github_script();
    let threads = script["threads"]["acme/app"]["12"].as_array_mut().unwrap();
    for n in 0..100 {
        threads.push(json!({"id": format!("PRRT_more{n}"), "isResolved": false,
            "isOutdated": false, "path": "src/more.rs", "line": n + 1, "diffSide": "RIGHT",
            "comments": {"nodes": []}}));
    }
    user.gh_script(script);
    let mcp = agent(&user);

    // The 102nd thread of the pull request: past the page of 100 a read gives.
    let answer = written(&user, &mcp, reply(json!("PRRT_more99")));

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("src/more.rs line 100, new side"),
        "{}",
        answer.text
    );
    assert_eq!(mutations(&user), ["addPullRequestReviewThreadReply"]);
    let checked = graphql(&user)
        .into_iter()
        .find(|args| field(args, "query").is_some_and(|q| q.contains("node(id")))
        .expect("the thread was checked by its id");
    assert_eq!(field(&checked, "thread"), Some("PRRT_more99"));
}

#[test]
fn a_data_center_read_cut_short_says_the_forge_has_more_threads() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mut script = data_center_script();
    let path = "/rest/api/1.0/projects/APP/repos/app/pull-requests/12/activities";
    let activities = script["api"][path]["values"].as_array_mut().unwrap();
    for n in 0..120 {
        activities.push(json!({"action": "COMMENTED", "commentAction": "ADDED",
            "comment": {"id": 1000 + n, "version": 0, "text": "General.",
                "author": {"displayName": "Dan", "name": "dan"}, "createdDate": at(13),
                "comments": []}}));
    }
    user.bkt_script(script);
    let mcp = agent(&user);

    let text = show(&mcp);

    assert!(
        text.contains("review threads (2 read, the forge has more"),
        "{text}"
    );
}

#[test]
fn a_thread_with_more_comments_than_read_says_so_on_github() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    let mut script = github_script();
    script["threads"]["acme/app"]["12"][0]["comments"]["totalCount"] = json!(130);
    user.gh_script(script);
    let mcp = agent(&user);

    let text = show(&mcp);

    let open = text
        .lines()
        .find(|l| l.contains("thread PRRT_open"))
        .unwrap();
    assert!(
        open.contains("130 comments, only the first 2 shown"),
        "{open}"
    );
    let old = text
        .lines()
        .find(|l| l.contains("thread PRRT_old"))
        .unwrap();
    assert!(!old.contains("comments, only"), "{old}");
}

const DC_COMMENT: &str = "/rest/api/1.0/projects/APP/repos/app/pull-requests/12/comments/201";

#[test]
fn a_data_center_comment_without_a_version_is_never_resolved_with_a_zero() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mut script = data_center_script();
    script["api"][DC_COMMENT]
        .as_object_mut()
        .unwrap()
        .remove("version");
    user.bkt_script(script);
    let mcp = agent(&user);

    let answer = written(&user, &mcp, resolve(json!(201), true));

    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("no version"), "{}", answer.text);
    assert!(bkt_writes(&user).is_empty(), "{:?}", bkt_writes(&user));
}

#[test]
fn a_data_center_comment_changed_between_the_read_and_the_write_is_read_again_once() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mut script = data_center_script();
    // Read as version 0, edited since: the server holds version 1.
    script["api"][DC_COMMENT]["version"] = json!(1);
    script["versions"] = json!({DC_COMMENT: [0, 1]});
    user.bkt_script(script);
    let mcp = agent(&user);

    let answer = written(&user, &mcp, resolve(json!(201), true));

    assert!(!answer.error, "{}", answer.text);
    assert!(answer.text.contains("is now resolved"), "{}", answer.text);
    assert_eq!(
        bkt_writes(&user),
        [
            (
                format!("PUT {DC_COMMENT}"),
                json!({"version": 0, "threadResolved": true})
            ),
            (
                format!("PUT {DC_COMMENT}"),
                json!({"version": 1, "threadResolved": true})
            )
        ]
    );
}

#[test]
fn a_data_center_comment_that_keeps_changing_says_to_try_again() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_data_center(&user);
    let mut script = data_center_script();
    script["api"][DC_COMMENT]["version"] = json!(1);
    script["versions"] = json!({DC_COMMENT: [0, 0]});
    user.bkt_script(script);
    let mcp = agent(&user);

    let answer = written(&user, &mcp, resolve(json!(201), true));

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("the comment changed meanwhile: try again"),
        "{}",
        answer.text
    );
    assert_eq!(bkt_writes(&user).len(), 2, "{:?}", bkt_writes(&user));
}
