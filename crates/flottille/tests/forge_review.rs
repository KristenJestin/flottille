//! A review an agent submits on a pull request (#364, `docs/tools.md`, "The forge"):
//! `forge review {number, verdict, body, comments?}`. A `comment` review goes like the other
//! writes, to judge; **`approve` and `request_changes` are held for the user in every mode, Full
//! included**, and the judge never decides them. The forge is the fake `gh` and `bkt`; the
//! remotes are throwaway bare repositories.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A project `app` of one repository on `origin` (a throwaway bare remote whose address is then
/// the forge's), its ready workspace `login` on the branch `app/login`, pushed.
fn project(user: &User, origin: &str, forge: &str) -> PathBuf {
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
    let worktree = user.path("data/flottille/workspaces/app/login");
    user.git(
        &worktree,
        &["update-ref", "refs/remotes/origin/app/login", "HEAD"],
    );
    folder
}

fn on_github(user: &User) {
    project(
        user,
        "git@github.com:acme/app.git",
        "{ kind = \"github\", repository = \"acme/app\" }",
    );
}

fn on_bitbucket(user: &User) {
    project(
        user,
        "git@bitbucket.org:acme/app.git",
        "{ kind = \"bitbucket-cloud\", repository = \"acme/app\" }",
    );
}

/// A root agent started in the workspace `login`: its id and its session.
fn agent(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let started = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "login", "message": "hello"}),
        )
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization))
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn set_mode(user: &User, agent: &str, mode: &str) {
    let output = user.run(&["agent", "mode", agent, mode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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

/// What the user is shown of the request `number`: the whole entry of `approval.list`.
fn request(user: &User, number: u64) -> Value {
    user.call("approval.list", json!({}))
        .unwrap()
        .get("requests")
        .and_then(Value::as_array)
        .and_then(|all| all.iter().find(|r| r["number"] == number))
        .cloned()
        .unwrap_or_else(|| panic!("no pending request #{number}"))
}

/// What the user reads on the card of the request `number`.
fn says(user: &User, number: u64) -> Vec<String> {
    request(user, number)["says"]
        .as_array()
        .unwrap()
        .iter()
        .map(|said| said.as_str().unwrap().to_owned())
        .collect()
}

/// The text the agent is handed once the request `number` is answered.
fn delivered(user: &User, number: u64) -> String {
    let store = || Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || store().delivery(&id).unwrap().is_some());
    store().delivery(&id).unwrap().unwrap().body
}

/// A call the gate holds, allowed by the user: the answer the agent is handed for it.
fn allowed(user: &User, number: u64) -> Answer {
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let text = delivered(user, number);
    Answer {
        error: !text.contains("and Flottille has now done it"),
        text,
    }
}

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

fn gh_starting(user: &User, start: &str) -> Vec<String> {
    calls(user.gh_received())
        .into_iter()
        .filter(|call| call.starts_with(start))
        .collect()
}

fn bkt_starting(user: &User, start: &str) -> Vec<String> {
    calls(user.bkt_received())
        .into_iter()
        .filter(|call| call.starts_with(start))
        .collect()
}

/// What each review sent to GitHub held: the fake `gh` records the file `--input` names.
fn github_reviews(user: &User) -> Vec<Value> {
    user.gh_received()
        .into_iter()
        .filter(|call| call["args"][0] == "api" && call["args"][1].to_string().contains("/reviews"))
        .map(|call| call["input"].clone())
        .collect()
}

/// GitHub's pull request #12 of `app/login`, open.
fn github_pulls() -> Value {
    json!({"pulls": {"acme/app": {"app/login": {"number": 12, "title": "Login",
        "state": "OPEN", "isDraft": false, "baseRefName": "main", "headRefName": "app/login",
        "isCrossRepository": false, "url": "https://github.com/acme/app/pull/12"}}}})
}

/// Bitbucket Cloud's pull request #12 of `app/login`, open.
fn cloud_script() -> Value {
    json!({"pulls": {"acme/app": [{"id": 12, "title": "Login", "state": "OPEN",
            "source": {"branch": {"name": "app/login"}, "repository": {"full_name": "acme/app"}},
            "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}},
            "links": {"html": {"href": "https://bitbucket.org/acme/app/pull-requests/12"}}}]},
        "api": {"/repositories/acme/app/pullrequests/12":
            {"id": 12, "state": "OPEN", "participants": []}}})
}

const BODY: &str = "Good direction, two nits.";

fn review(verdict: &str) -> Value {
    json!({"action": "review", "number": 12, "repository": ".", "verdict": verdict,
        "body": BODY,
        "comments": [{"path": "src/login.rs", "line": 14, "body": "Rename this."},
            {"path": "README.md", "line": 2, "body": "Typo."}]})
}

#[test]
fn a_review_comment_with_inline_comments_is_posted() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github_pulls());
    let (_, mcp) = agent(&user);

    let number = held(&mcp.call("forge", review("comment")));
    // The user reads the verdict, the body and every inline comment whole.
    let card = says(&user, number);
    for said in [
        "Its body: Good direction, two nits.",
        "Its verdict: comment",
        "Its inline comment on src/login.rs line 14: Rename this.",
        "Its inline comment on README.md line 2: Typo.",
    ] {
        assert!(card.contains(&said.to_owned()), "{said}: {card:?}");
    }
    assert!(github_reviews(&user).is_empty(), "posted before the answer");

    let answer = allowed(&user, number);
    assert!(!answer.error, "{}", answer.text);
    let posted = gh_starting(&user, "api repos/acme/app/pulls/12/reviews");
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert!(
        posted[0].starts_with(
            "api repos/acme/app/pulls/12/reviews --hostname github.com --method POST --input "
        ),
        "{posted:?}"
    );
    assert_eq!(
        github_reviews(&user),
        [json!({"event": "COMMENT", "body": BODY, "comments": [
            {"path": "src/login.rs", "line": 14, "side": "RIGHT", "body": "Rename this."},
            {"path": "README.md", "line": 2, "side": "RIGHT", "body": "Typo."}]})]
    );
}

#[test]
fn an_approval_waits_for_the_user_even_in_full() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github_pulls());
    let (id, mcp) = agent(&user);
    set_mode(&user, &id, "full");

    let mut asked = review("approve");
    asked["comments"][0]["body"] = json!("Use ghp_0123456789abcdefghijklmnopqrstuvwxyz to try it.");
    let number = held(&mcp.call("forge", asked));

    // Held as base, with the verdict, the body and the inline comments on the card, masked.
    let held_request = request(&user, number);
    assert_eq!(held_request["class"], "base", "{held_request}");
    let card = says(&user, number);
    for said in [
        "Its body: Good direction, two nits.",
        "Its verdict: approve",
        "Its inline comment on README.md line 2: Typo.",
    ] {
        assert!(card.contains(&said.to_owned()), "{said}: {card:?}");
    }
    let leaked = card.iter().any(|said| said.contains("ghp_0123456789"));
    assert!(!leaked, "a token on the card: {card:?}");
    assert!(
        card.iter()
            .any(|said| said.starts_with("Its inline comment on src/login.rs line 14: Use ")),
        "{card:?}"
    );
    assert!(github_reviews(&user).is_empty(), "approved before the user");

    let answer = allowed(&user, number);
    assert!(!answer.error, "{}", answer.text);
    let sent = github_reviews(&user);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["event"], "APPROVE");
}

#[test]
fn request_changes_waits_for_the_user() {
    let user = User::with_fake_agent().with_allowing_judge().with_fake_gh();
    on_github(&user);
    user.gh_script(github_pulls());
    let (id, mcp) = agent(&user);
    set_mode(&user, &id, "auto");

    let number = held(&mcp.call("forge", review("request_changes")));

    // The judge that allows everything is never asked: only the user decides it.
    let judged = user
        .judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .count();
    assert_eq!(judged, 0, "the judge was asked");
    assert_eq!(request(&user, number)["class"], "base");
    assert!(github_reviews(&user).is_empty());

    let answer = allowed(&user, number);
    assert!(!answer.error, "{}", answer.text);
    let sent = github_reviews(&user);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["event"], "REQUEST_CHANGES");
    assert_eq!(sent[0]["body"], BODY);
}

#[test]
fn an_approval_the_user_refused_is_not_posted() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github_pulls());
    let (_, mcp) = agent(&user);

    let number = held(&mcp.call("forge", review("approve")));
    user.call("approval.deny", json!({"number": number}))
        .unwrap();

    let text = delivered(&user, number);
    assert!(!text.contains("and Flottille has now done it"), "{text}");
    assert!(github_reviews(&user).is_empty(), "posted after a refusal");
    assert!(
        gh_starting(&user, "api repos/acme/app/pulls/12/reviews").is_empty(),
        "gh was asked to post"
    );
}

#[test]
fn an_approval_of_a_pull_request_that_closed_meanwhile_is_not_posted() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github_pulls());
    let (_, mcp) = agent(&user);
    let number = held(&mcp.call("forge", review("approve")));

    let mut closed = github_pulls();
    closed["pulls"]["acme/app"]["app/login"]["state"] = json!("CLOSED");
    user.gh_script(closed);
    let answer = allowed(&user, number);

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("is closed, not open"),
        "{}",
        answer.text
    );
    assert!(github_reviews(&user).is_empty());
}

#[test]
fn a_review_without_a_verdict_or_a_body_or_a_line_is_refused_before_anything_is_asked() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github_pulls());
    let (_, mcp) = agent(&user);

    let mut no_body = review("request_changes");
    no_body["body"] = json!("  ");
    let mut no_line = review("comment");
    no_line["comments"][0]["line"] = json!(0);
    let mut dashed = review("comment");
    dashed["comments"][0]["path"] = json!("--file");
    let mut no_number = review("comment");
    no_number.as_object_mut().unwrap().remove("number");
    for (what, asked) in [
        (
            "a verdict that is none",
            json!({"action": "review", "number": 12, "verdict": "lgtm", "body": "x"}),
        ),
        (
            "no verdict",
            json!({"action": "review", "number": 12, "body": "x"}),
        ),
        ("no body on a change request", no_body),
        ("line 0", no_line),
        ("a path that is an option", dashed),
        ("no number", no_number),
        (
            "a verdict on a comment",
            json!({"action": "comment", "number": 12, "body": "x", "verdict": "approve"}),
        ),
    ] {
        let answer = mcp.call("forge", asked);
        assert!(answer.error, "{what}: {}", answer.text);
        assert!(
            answer.text.starts_with("refused"),
            "{what}: {}",
            answer.text
        );
    }
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a refused call was asked of the user"
    );
}

#[test]
fn an_approval_with_no_body_is_held_and_posted_with_none() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user);
    user.gh_script(github_pulls());
    let (_, mcp) = agent(&user);

    let number = held(&mcp.call(
        "forge",
        json!({"action": "review", "number": 12, "verdict": "approve"}),
    ));
    let answer = allowed(&user, number);

    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        github_reviews(&user),
        [json!({"event": "APPROVE", "body": "", "comments": []})]
    );
}

#[test]
fn a_bitbucket_review_comment_posts_the_body_then_each_inline_comment() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user);
    user.bkt_script(cloud_script());
    let (_, mcp) = agent(&user);

    let number = held(&mcp.call("forge", review("comment")));
    let answer = allowed(&user, number);

    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr comment"),
        [
            "pr comment 12 --context cloud --workspace acme --repo app --text Good direction, \
             two nits.",
            "pr comment 12 --context cloud --workspace acme --repo app --text Rename this. \
             --file src/login.rs --to-line 14",
            "pr comment 12 --context cloud --workspace acme --repo app --text Typo. --file \
             README.md --to-line 2"
        ]
    );
    assert!(bkt_starting(&user, "pr approve").is_empty());
}

#[test]
fn a_bitbucket_approval_waits_for_the_user_and_approves_after_its_comments() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user);
    user.bkt_script(cloud_script());
    let (id, mcp) = agent(&user);
    set_mode(&user, &id, "full");

    let number = held(&mcp.call("forge", review("approve")));
    assert_eq!(request(&user, number)["class"], "base");
    assert!(bkt_starting(&user, "pr approve").is_empty());
    let answer = allowed(&user, number);

    assert!(!answer.error, "{}", answer.text);
    let all: Vec<String> = calls(user.bkt_received())
        .into_iter()
        .filter(|call| call.starts_with("pr comment") || call.starts_with("pr approve"))
        .collect();
    assert_eq!(all.len(), 4, "{all:?}");
    // The verdict comes last: a comment that failed leaves the pull request unapproved.
    assert_eq!(
        all[3],
        "pr approve 12 --context cloud --workspace acme --repo app"
    );
}

#[test]
fn a_bitbucket_cloud_change_request_goes_through_the_request_changes_endpoint() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user);
    user.bkt_script(cloud_script());
    let (_, mcp) = agent(&user);

    let number = held(&mcp.call("forge", review("request_changes")));
    let answer = allowed(&user, number);

    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(
            &user,
            "api /repositories/acme/app/pullrequests/12/request-changes"
        ),
        [
            "api /repositories/acme/app/pullrequests/12/request-changes --context cloud --method \
          POST"
        ]
    );
}

#[test]
fn a_bitbucket_review_that_fails_half_way_says_what_was_already_posted() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user);
    let mut script = cloud_script();
    script["step_fail"] = json!({"acme/app": {"approve": "Error: 502 Bad Gateway"}});
    user.bkt_script(script);
    let (_, mcp) = agent(&user);

    let number = held(&mcp.call("forge", review("approve")));
    let answer = allowed(&user, number);

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("had already changed the pull request"),
        "{}",
        answer.text
    );
}

#[test]
fn a_data_center_change_request_is_refused_saying_bkt_cannot() {
    let user = User::with_fake_agent().with_fake_bkt();
    project(
        &user,
        "ssh://git@bitbucket.example.com:7999/APP/app.git",
        "{ kind = \"bitbucket-data-center\", host = \"bitbucket.example.com\", \
         repository = \"APP/app\" }",
    );
    user.bkt_script(json!({
        "contexts": [{"name": "dc", "host": "bitbucket.example.com"}]}));
    let (_, mcp) = agent(&user);

    let number = held(&mcp.call("forge", review("request_changes")));
    let answer = allowed(&user, number);

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("Data Center") && answer.text.contains("nothing was posted"),
        "{}",
        answer.text
    );
    assert!(bkt_starting(&user, "pr comment").is_empty());
}
