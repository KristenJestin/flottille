//! What an agent can do to a pull request after it opened it (#360, `docs/tools.md`, "The forge"):
//! reviewers and labels on `create`, the branch deleted at `merge`, and `edit` with a draft made
//! ready, reviewers, labels and a base. The forge is the fake `gh` and `bkt`; the remotes are
//! throwaway bare repositories; every write is held for the user and done once they allow it.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A project `app` of one repository on `origin` (a throwaway bare remote whose address is then
/// the forge's), its ready workspace `login` on the branch `app/login`, pushed. `forge` is the
/// repository's declaration, `repository` more keys of its table, `more` the tables after it.
fn project(user: &User, origin: &str, forge: &str, repository: &str, more: &str) -> PathBuf {
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
            "folder = \"{}\"\n\n[[repositories]]\npath = \".\"\nforge = {forge}\n{repository}\n\
             {more}",
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

fn on_github(user: &User, repository: &str, more: &str) {
    project(
        user,
        "git@github.com:acme/app.git",
        "{ kind = \"github\", repository = \"acme/app\" }",
        repository,
        more,
    );
}

fn on_bitbucket(user: &User, repository: &str, more: &str) {
    project(
        user,
        "git@bitbucket.org:acme/app.git",
        "{ kind = \"bitbucket-cloud\", repository = \"acme/app\" }",
        repository,
        more,
    );
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

/// A call the gate holds, allowed by the user: the answer the agent is handed for it, an error
/// when it was not done.
fn allowed(user: &User, answer: Answer) -> Answer {
    let number = held(&answer);
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let store = || Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let id = format!("approval-{number}");
    eventually("the answer", || store().delivery(&id).unwrap().is_some());
    let text = store().delivery(&id).unwrap().unwrap().body;
    Answer {
        error: !text.contains("and Flottille has now done it"),
        text,
    }
}

/// A write of `forge`, allowed by the user.
fn write(user: &User, mcp: &Mcp, args: Value) -> Answer {
    allowed(user, mcp.call("forge", args))
}

/// What the user reads on the card of the request `number`.
fn says(user: &User, number: u64) -> Vec<String> {
    let requests = user.call("approval.list", json!({})).unwrap();
    let request = requests["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .unwrap_or_else(|| panic!("no pending request #{number}"));
    request["says"]
        .as_array()
        .unwrap()
        .iter()
        .map(|said| said.as_str().unwrap().to_owned())
        .collect()
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

fn gh(user: &User) -> Vec<String> {
    calls(user.gh_received())
}

fn bkt(user: &User) -> Vec<String> {
    calls(user.bkt_received())
}

fn gh_starting(user: &User, start: &str) -> Vec<String> {
    gh(user)
        .into_iter()
        .filter(|call| call.starts_with(start))
        .collect()
}

fn bkt_starting(user: &User, start: &str) -> Vec<String> {
    bkt(user)
        .into_iter()
        .filter(|call| call.starts_with(start))
        .collect()
}

fn passing() -> Value {
    json!([{"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
        "conclusion": "SUCCESS"}])
}

/// GitHub's pull request #12 of `app/login`, open and passing, and the script around it.
fn github_pulls() -> Value {
    github_pulls_in("acme/app")
}

/// The same, in the repository `slug`.
fn github_pulls_in(slug: &str) -> Value {
    json!({"created": 12, "pulls": {slug: {"app/login": {"number": 12, "title": "Login",
        "state": "OPEN", "isDraft": true, "baseRefName": "main", "headRefName": "app/login",
        "isCrossRepository": false, "url": format!("https://github.com/{slug}/pull/12"),
        "statusCheckRollup": passing()}}}})
}

/// Bitbucket Cloud's pull request #12 of `app/login`, open, its checks passing, and what
/// `bkt api` answers for its page.
fn cloud_script() -> Value {
    json!({"created": 12,
        "pulls": {"acme/app": [{"id": 12, "title": "Login", "state": "OPEN",
            "source": {"branch": {"name": "app/login"}, "repository": {"full_name": "acme/app"}},
            "destination": {"branch": {"name": "main"}, "repository": {"full_name": "acme/app"}},
            "links": {"html": {"href": "https://bitbucket.org/acme/app/pull-requests/12"}}}]},
        "checks": {"acme/app": {"12": [{"state": "SUCCESSFUL", "name": "build"}]}},
        "api": {"/repositories/acme/app/pullrequests/12":
            {"id": 12, "state": "OPEN", "participants": []},
            "/repositories/acme/app/refs/branches/release": {"name": "release"}}})
}

/// A project `app` of the repositories `api` (its table gets `setting`) and `web`, both on GitHub,
/// and its ready workspace `login`, which takes `api` only, on the branch `app/login`, pushed.
fn api_and_web(user: &User, setting: &str) {
    let folder = user.path("work/app");
    let mut tables = String::new();
    for repository in ["api", "web"] {
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
        user.git(
            &dir,
            &[
                "remote",
                "set-url",
                "origin",
                &format!("git@github.com:acme/{repository}.git"),
            ],
        );
        let more = if repository == "api" { setting } else { "" };
        tables += &format!(
            "\n[[repositories]]\npath = \"{repository}\"\nforge = {{ kind = \"github\", \
             repository = \"acme/{repository}\" }}\n{more}\n"
        );
    }
    user.write_project_file(
        "app",
        &format!("folder = \"{}\"\n{tables}", folder.display()),
    );
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "repositories": ["api"], "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    let worktree = user.path("data/flottille/workspaces/app/login");
    for dir in [worktree.join("api"), worktree.clone()] {
        if dir.join(".git").exists() {
            user.git(
                &dir,
                &["update-ref", "refs/remotes/origin/app/login", "HEAD"],
            );
        }
    }
}

/// A project `app` on `origin` whose workspace `login` resumed the branch `app/login`, which
/// existed before it (the user's own), its repository's table holding `delete_branch_on_merge`.
fn resumed(user: &User, origin: &str, forge: &str) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(&folder, &["branch", "app/login"]);
    user.git(&folder, &["remote", "add", "origin", origin]);
    user.write_project_file(
        "app",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \".\"\nforge = {forge}\n\
             delete_branch_on_merge = true\n",
            folder.display()
        ),
    );
    let made = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login", "branch": "app/login", "offline": true}),
        )
        .unwrap();
    assert_eq!(made["state"], "ready", "{made}");
    user.git(
        &user.path("data/flottille/workspaces/app/login"),
        &["update-ref", "refs/remotes/origin/app/login", "HEAD"],
    );
}

/// A second workspace `other` recorded on the branch `app/login`: git lets only one worktree
/// hold a branch, so `login` lets it go for the time `other` is made, and takes it back.
fn another_workspace_on_the_branch(user: &User) {
    let login = user.path("data/flottille/workspaces/app/login");
    let other = user.path("data/flottille/workspaces/app/other");
    user.git(&login, &["switch", "--quiet", "--detach"]);
    let made = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "other", "branch": "app/login", "offline": true}),
        )
        .unwrap();
    assert_eq!(made["state"], "ready", "{made}");
    user.git(&other, &["switch", "--quiet", "--detach"]);
    user.git(&login, &["switch", "--quiet", "app/login"]);
}

const GITHUB: &str = "{ kind = \"github\", repository = \"acme/app\" }";
const BITBUCKET: &str = "{ kind = \"bitbucket-cloud\", repository = \"acme/app\" }";

// create

#[test]
fn create_adds_reviewers_and_labels() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(
        &user,
        "",
        "[pull_requests]\nreviewers = [\"alice\", \"bob\"]\n",
    );
    user.gh_script(github_pulls());
    let mcp = agent(&user);

    // The user reads what is asked: the reviewers and the labels, whole.
    let asked = mcp.call(
        "forge",
        json!({"action": "create", "repository": ".", "title": "Add the login page",
            "reviewers": ["carol", "bob"], "labels": ["bug", "good first issue"]}),
    );
    let card = says(&user, held(&asked));
    assert!(
        card.contains(&"Its reviewers, besides the project's: carol, bob".to_owned()),
        "{card:?}"
    );
    assert!(
        card.contains(&"Its labels: bug, good first issue".to_owned()),
        "{card:?}"
    );

    let answer = write(
        &user,
        &mcp,
        json!({"action": "create", "repository": ".", "title": "Add the login page",
            "reviewers": ["carol", "bob"], "labels": ["bug", "good first issue"]}),
    );
    assert!(!answer.error, "{}", answer.text);
    // The project's reviewers first, those asked after them, none twice.
    assert_eq!(
        gh_starting(&user, "pr create"),
        [
            "pr create -R github.com/acme/app --head app/login --base main --title Add the login \
             page --body  --reviewer alice --reviewer bob --reviewer carol --label bug --label \
             good first issue"
        ]
    );
}

/// The project's pull request template (#466), a Markdown text of several lines.
const TEMPLATE: &str = "## Why\n\n## How to verify\n";

#[test]
fn forge_create_without_a_body_uses_the_template() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(
        &user,
        "",
        "[pull_requests]\ntemplate = \"\"\"\n## Why\n\n## How to verify\n\"\"\"\n",
    );
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let create = json!({"action": "create", "repository": ".", "title": "Add the login page"});

    // The user reads the body the forge will show: the template.
    let card = says(&user, held(&mcp.call("forge", create.clone())));
    assert!(
        card.contains(&format!("Its body, the project's template: {TEMPLATE}")),
        "{card:?}"
    );

    let answer = write(&user, &mcp, create);
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr create"),
        [format!(
            "pr create -R github.com/acme/app --head app/login --base main --title Add the login \
             page --body {TEMPLATE}"
        )]
    );
}

#[test]
fn forge_create_with_a_body_keeps_it() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "[pull_requests]\ntemplate = \"## Why\\n\"\n");
    user.gh_script(github_pulls());
    let mcp = agent(&user);

    let answer = write(
        &user,
        &mcp,
        json!({"action": "create", "repository": ".", "title": "Add the login page",
            "body": "## Why\nThe login page was missing."}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr create"),
        [
            "pr create -R github.com/acme/app --head app/login --base main --title Add the login \
          page --body ## Why\nThe login page was missing."
        ]
    );
}

#[test]
fn create_refuses_a_reviewer_or_a_label_that_would_be_an_option() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);

    for (key, value, why) in [
        ("reviewers", "--admin", "reviewers"),
        ("reviewers", "a b", "reviewers"),
        ("labels", "-x", "labels"),
        ("labels", "a,b", "labels"),
        ("labels", " ", "labels"),
    ] {
        // Refused at once: the user is not asked for what cannot be done.
        let answer = mcp.call(
            "forge",
            json!({"action": "create", "repository": ".", "title": "T", key: [value]}),
        );
        assert!(
            answer.text.contains(&format!("refused: {why}")),
            "{key} {value:?}: {}",
            answer.text
        );
    }
    assert!(gh_starting(&user, "pr create").is_empty());
}

#[test]
fn create_refuses_delete_branch_on_merge_for_a_branch_it_would_not_delete() {
    let create = json!({"action": "create", "repository": ".", "title": "Login",
        "delete_branch_on_merge": true});
    let refused = |answer: &Answer, why: &str| {
        assert!(answer.error, "{}", answer.text);
        assert!(
            answer.text.contains(&format!(
                "refused: delete_branch_on_merge: the branch app/login is never deleted: {why}; \
                 nothing was opened"
            )),
            "{}",
            answer.text
        );
    };

    // A branch the workspace resumed, on GitHub and on Bitbucket.
    let user = User::with_fake_agent().with_fake_gh();
    resumed(&user, "git@github.com:acme/app.git", GITHUB);
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    refused(
        &write(&user, &mcp, create.clone()),
        "it existed before the workspace",
    );
    assert!(gh_starting(&user, "pr create").is_empty());

    let user = User::with_fake_agent().with_fake_bkt();
    resumed(&user, "git@bitbucket.org:acme/app.git", BITBUCKET);
    user.bkt_script(cloud_script());
    let mcp = agent(&user);
    refused(
        &write(&user, &mcp, create.clone()),
        "it existed before the workspace",
    );
    assert!(bkt_starting(&user, "pr create").is_empty());

    // A branch another workspace is on, on both.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    another_workspace_on_the_branch(&user);
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    refused(
        &write(&user, &mcp, create.clone()),
        "the workspace other is on it",
    );
    assert!(gh_starting(&user, "pr create").is_empty());

    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "", "");
    another_workspace_on_the_branch(&user);
    user.bkt_script(cloud_script());
    let mcp = agent(&user);
    refused(&write(&user, &mcp, create), "the workspace other is on it");
    assert!(bkt_starting(&user, "pr create").is_empty());
}

#[test]
fn create_sets_bitbucket_close_source_branch() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "", "[pull_requests]\nreviewers = [\"alice\"]\n");
    user.bkt_script(cloud_script());
    let mcp = agent(&user);

    let asked = mcp.call(
        "forge",
        json!({"action": "create", "repository": ".", "title": "Login",
            "reviewers": ["carol"], "delete_branch_on_merge": true}),
    );
    let card = says(&user, held(&asked));
    assert!(
        card.contains(&"It deletes the branch when the pull request is merged".to_owned()),
        "{card:?}"
    );

    let answer = write(
        &user,
        &mcp,
        json!({"action": "create", "repository": ".", "title": "Login",
            "reviewers": ["carol"], "delete_branch_on_merge": true}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr create"),
        [
            "pr create --context cloud --workspace acme --repo app --source app/login --target \
             main --title Login --description  --reviewer alice --reviewer carol --close-source"
        ]
    );
}

#[test]
fn labels_are_refused_on_bitbucket_naming_why() {
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "", "");
    user.bkt_script(cloud_script());
    let mcp = agent(&user);

    let create = write(
        &user,
        &mcp,
        json!({"action": "create", "repository": ".", "title": "Login", "labels": ["bug"]}),
    );
    let edit = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "labels_add": ["bug"]}),
    );

    for refused in [&create.text, &edit.text] {
        assert!(
            refused.contains("refused: Bitbucket has no labels on pull requests"),
            "{refused}"
        );
        assert!(refused.contains("nothing was"), "{refused}");
    }
    assert!(bkt_starting(&user, "pr create").is_empty());
    assert!(bkt_starting(&user, "pr edit").is_empty());
}

#[test]
fn create_keeps_delete_branch_on_merge_for_a_github_pull_request_and_says_the_repository_s() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    let mut script = github_pulls();
    script["delete_head_branches"] = json!({"acme/app": true});
    user.gh_script(script);
    let mcp = agent(&user);

    let created = write(
        &user,
        &mcp,
        json!({"action": "create", "repository": ".", "title": "Login",
            "delete_branch_on_merge": true}),
    );
    assert!(!created.error, "{}", created.text);
    // Said: Flottille deletes it at the merge, and the repository does by itself.
    assert!(
        created
            .text
            .contains("Flottille will delete the branch app/login when it merges #12"),
        "{}",
        created.text
    );
    assert!(
        created
            .text
            .contains("the repository's own setting deletes head branches after a merge"),
        "{}",
        created.text
    );
    // No flag of `gh pr create` for it: it is Flottille's to apply.
    assert!(!gh_starting(&user, "pr create")[0].contains("delete"));

    // At the merge, left out, it follows the pull request.
    let merged = write(&user, &mcp, json!({"action": "merge", "repository": "."}));
    assert!(!merged.error, "{}", merged.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash --delete-branch"]
    );
    assert!(
        merged.text.contains("and its branch app/login was deleted"),
        "{}",
        merged.text
    );
}

// merge

#[test]
fn merge_deletes_the_branch_when_asked_or_set() {
    // Asked.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let asked = json!({"action": "merge", "repository": ".", "delete_branch": true});
    let card = says(&user, held(&mcp.call("forge", asked.clone())));
    assert!(
        card.contains(&"It deletes the source branch on the forge after the merge".to_owned()),
        "{card:?}"
    );
    let answer = write(&user, &mcp, asked);
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash --delete-branch"]
    );
    assert!(
        answer.text.contains("and its branch app/login was deleted"),
        "{}",
        answer.text
    );

    // Set by the project, for the repository, and said on the card.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "delete_branch_on_merge = true", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let card = says(
        &user,
        held(&mcp.call("forge", json!({"action": "merge", "repository": "."}))),
    );
    assert!(
        card.contains(
            &"It deletes the source branch on the forge after the merge (the project's setting \
              Delete the branch after merging)"
                .to_owned()
        ),
        "{card:?}"
    );
    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash --delete-branch"]
    );

    // Bitbucket: the strategy and the close of the source branch, both said.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "", "");
    user.bkt_script(cloud_script());
    let mcp = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "merge", "repository": ".", "delete_branch": true}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr merge"),
        [
            "pr merge 12 --context cloud --workspace acme --repo app --strategy squash \
          --close-source=true"
        ]
    );
}

#[test]
fn merge_keeps_the_branch_by_default() {
    // Nothing asked, nothing set.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash"]
    );
    assert!(
        !answer.text.contains("branch app/login was"),
        "{}",
        answer.text
    );

    // The project deletes, this merge says not to.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "delete_branch_on_merge = true", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "merge", "repository": ".", "delete_branch": false}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash"]
    );

    // Bitbucket keeps it as it always did.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "", "");
    user.bkt_script(cloud_script());
    let mcp = agent(&user);
    let answer = write(&user, &mcp, json!({"action": "merge", "repository": "."}));
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr merge"),
        [
            "pr merge 12 --context cloud --workspace acme --repo app --strategy squash \
          --close-source=false"
        ]
    );
}

#[test]
fn merge_never_deletes_a_branch_the_workspace_resumed_or_another_workspace_is_on() {
    let merge = json!({"action": "merge", "repository": ".", "delete_branch": true});

    // A branch that existed before the workspace was made is the user's.
    let user = User::with_fake_agent().with_fake_gh();
    resumed(&user, "git@github.com:acme/app.git", GITHUB);
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let answer = write(&user, &mcp, merge.clone());
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash"]
    );
    assert!(
        answer
            .text
            .contains("its branch app/login was left: it existed before the workspace"),
        "{}",
        answer.text
    );

    // So is one another workspace is on, whatever the project or the call says.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "delete_branch_on_merge = true", "");
    another_workspace_on_the_branch(&user);
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let answer = write(&user, &mcp, merge.clone());
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash"]
    );
    assert!(
        answer
            .text
            .contains("its branch app/login was left: the workspace other is on it"),
        "{}",
        answer.text
    );

    // Bitbucket says it to `bkt`, which closes the source branch unless told not to.
    let user = User::with_fake_agent().with_fake_bkt();
    resumed(&user, "git@bitbucket.org:acme/app.git", BITBUCKET);
    user.bkt_script(cloud_script());
    let mcp = agent(&user);
    let answer = write(&user, &mcp, merge.clone());
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr merge"),
        [
            "pr merge 12 --context cloud --workspace acme --repo app --strategy squash \
             --close-source=false"
        ]
    );
    assert!(
        answer
            .text
            .contains("its branch app/login was left: it existed before the workspace"),
        "{}",
        answer.text
    );

    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "delete_branch_on_merge = true", "");
    another_workspace_on_the_branch(&user);
    user.bkt_script(cloud_script());
    let mcp = agent(&user);
    let answer = write(&user, &mcp, merge);
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr merge"),
        [
            "pr merge 12 --context cloud --workspace acme --repo app --strategy squash \
             --close-source=false"
        ]
    );
    assert!(
        answer
            .text
            .contains("its branch app/login was left: the workspace other is on it"),
        "{}",
        answer.text
    );
}

#[test]
fn merge_says_a_github_repository_deletes_the_branch_flottille_left() {
    let user = User::with_fake_agent().with_fake_gh();
    resumed(&user, "git@github.com:acme/app.git", GITHUB);
    let mut script = github_pulls();
    script["delete_head_branches"] = json!({"acme/app": true});
    user.gh_script(script);
    let mcp = agent(&user);

    let answer = write(
        &user,
        &mcp,
        json!({"action": "merge", "repository": ".", "delete_branch": true}),
    );

    assert!(!answer.error, "{}", answer.text);
    // Flottille asked `gh` to leave it; it cannot stop the repository's own setting.
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/app --squash"]
    );
    assert!(
        answer.text.contains(
            "the repository's own setting deletes head branches: GitHub deletes app/login"
        ),
        "{}",
        answer.text
    );
    assert!(!answer.text.contains("was left"), "{}", answer.text);
}

#[test]
fn the_merge_card_names_the_deletion_in_the_workspace_s_only_repository() {
    // `api` deletes on merge, the workspace takes `api` only: no repository needs naming.
    let user = User::with_fake_agent().with_fake_gh();
    api_and_web(&user, "delete_branch_on_merge = true");
    user.gh_script(github_pulls_in("acme/api"));
    let mcp = agent(&user);

    let card = says(&user, held(&mcp.call("forge", json!({"action": "merge"}))));
    assert!(
        card.iter()
            .any(|said| said.contains("Delete the branch after merging")),
        "{card:?}"
    );

    let answer = write(&user, &mcp, json!({"action": "merge"}));
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr merge"),
        ["pr merge 12 -R github.com/acme/api --squash --delete-branch"]
    );
}

#[test]
fn the_merge_card_names_the_deletion_a_create_asked_for() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let created = write(
        &user,
        &mcp,
        json!({"action": "create", "repository": ".", "title": "Login",
            "delete_branch_on_merge": true}),
    );
    assert!(!created.error, "{}", created.text);

    let card = says(
        &user,
        held(&mcp.call("forge", json!({"action": "merge", "repository": "."}))),
    );

    assert!(
        card.contains(
            &"It deletes the source branch on the forge after the merge if the pull request \
              asked for it when it was opened"
                .to_owned()
        ),
        "{card:?}"
    );
}

#[test]
fn the_cards_do_not_say_more_than_will_happen() {
    // A merge that leaves a resumed branch does not say it deletes it.
    let user = User::with_fake_agent().with_fake_gh();
    resumed(&user, "git@github.com:acme/app.git", GITHUB);
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let card = says(
        &user,
        held(&mcp.call(
            "forge",
            json!({"action": "merge", "repository": ".", "delete_branch": true}),
        )),
    );
    assert!(
        !card
            .iter()
            .any(|said| said.starts_with("It deletes the source branch")),
        "{card:?}"
    );
    assert!(
        card.contains(
            &"It asks to delete the source branch, but the merge leaves it: it existed before \
              the workspace"
                .to_owned()
        ),
        "{card:?}"
    );

    // On GitHub only a merge by Flottille deletes the branch a create asked to delete.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);
    let card = says(
        &user,
        held(&mcp.call(
            "forge",
            json!({"action": "create", "repository": ".", "title": "Login",
                "delete_branch_on_merge": true}),
        )),
    );
    assert!(
        card.contains(
            &"Flottille deletes the branch when it merges the pull request (GitHub has no such \
              flag: a merge by anyone else leaves it)"
                .to_owned()
        ),
        "{card:?}"
    );
}

// edit

#[test]
fn a_draft_is_marked_ready() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);

    let ready = json!({"action": "edit", "number": 12, "draft": false});
    let card = says(&user, held(&mcp.call("forge", ready.clone())));
    assert!(
        card.contains(&"It marks it ready for review".to_owned()),
        "{card:?}"
    );
    let answer = write(&user, &mcp, ready);
    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("Pull request #12 of acme/app was changed."),
        "{}",
        answer.text
    );
    // Only the readiness: `gh pr edit` was not asked for nothing.
    assert_eq!(
        gh_starting(&user, "pr ready"),
        ["pr ready 12 -R github.com/acme/app"]
    );
    assert!(gh_starting(&user, "pr edit").is_empty());

    // And back to a draft.
    let answer = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "draft": true}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr ready")[1],
        "pr ready 12 -R github.com/acme/app --undo"
    );

    // Bitbucket publishes it.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "", "");
    user.bkt_script(cloud_script());
    let mcp = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "draft": false}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr publish"),
        ["pr publish 12 --context cloud --workspace acme --repo app"]
    );
}

#[test]
fn reviewers_are_added_and_removed() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);

    let edit = json!({"action": "edit", "number": 12, "title": "feat: login",
        "reviewers_add": ["carol", "dave"], "reviewers_remove": ["alice"]});
    let card = says(&user, held(&mcp.call("forge", edit.clone())));
    for said in [
        "It adds the reviewers: carol, dave",
        "It removes the reviewers: alice",
    ] {
        assert!(card.contains(&said.to_owned()), "{said}: {card:?}");
    }
    let answer = write(&user, &mcp, edit);
    assert!(!answer.error, "{}", answer.text);
    // One call for everything `gh pr edit` takes.
    assert_eq!(
        gh_starting(&user, "pr edit"),
        [
            "pr edit 12 -R github.com/acme/app --title feat: login --add-reviewer carol \
          --add-reviewer dave --remove-reviewer alice"
        ]
    );

    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "", "");
    user.bkt_script(cloud_script());
    let mcp = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "reviewers_add": ["carol"],
            "reviewers_remove": ["alice"]}),
    );
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        bkt_starting(&user, "pr edit"),
        [
            "pr edit 12 --context cloud --workspace acme --repo app --reviewer carol \
          --remove-reviewer alice"
        ]
    );
}

#[test]
fn labels_are_added_and_removed_on_github() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);

    let edit = json!({"action": "edit", "number": 12, "labels_add": ["bug", "good first issue"],
        "labels_remove": ["wip"]});
    let card = says(&user, held(&mcp.call("forge", edit.clone())));
    for said in [
        "It adds the labels: bug, good first issue",
        "It removes the labels: wip",
    ] {
        assert!(card.contains(&said.to_owned()), "{said}: {card:?}");
    }
    let answer = write(&user, &mcp, edit);
    assert!(!answer.error, "{}", answer.text);
    assert_eq!(
        gh_starting(&user, "pr edit"),
        [
            "pr edit 12 -R github.com/acme/app --add-label bug --add-label good first issue \
          --remove-label wip"
        ]
    );
}

#[test]
fn the_base_is_changed_and_an_unknown_base_refused() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    let mut script = github_pulls();
    script["branches"] = json!({"acme/app": ["main", "release"]});
    user.gh_script(script);
    let mcp = agent(&user);

    let card = says(
        &user,
        held(&mcp.call(
            "forge",
            json!({"action": "edit", "number": 12, "base": "release"}),
        )),
    );
    assert!(
        card.contains(&"It moves it onto the base release".to_owned()),
        "{card:?}"
    );
    let moved = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "base": "release"}),
    );
    assert!(!moved.error, "{}", moved.text);
    assert_eq!(
        gh_starting(&user, "pr edit"),
        ["pr edit 12 -R github.com/acme/app --base release"]
    );

    // Not on the remote: refused before anything is edited.
    let unknown = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "base": "nowhere"}),
    );
    assert!(
        unknown
            .text
            .contains("refused: acme/app has no branch nowhere on the forge; nothing was written"),
        "{}",
        unknown.text
    );
    assert_eq!(gh_starting(&user, "pr edit").len(), 1);

    // Bitbucket Cloud moves the destination through its API.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "", "");
    user.bkt_script(cloud_script());
    let mcp = agent(&user);
    let moved = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "base": "release"}),
    );
    assert!(!moved.error, "{}", moved.text);
    assert_eq!(
        bkt_starting(
            &user,
            "api /repositories/acme/app/pullrequests/12 --context cloud --method PUT"
        ),
        [
            "api /repositories/acme/app/pullrequests/12 --context cloud --method PUT --input \
          {\"destination\":{\"branch\":{\"name\":\"release\"}}}"
        ]
    );
    let unknown = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "base": "nowhere"}),
    );
    assert!(
        unknown
            .text
            .contains("refused: acme/app has no branch nowhere on the forge; nothing was written"),
        "{}",
        unknown.text
    );
}

#[test]
fn a_base_that_is_not_a_branch_name_is_refused_before_the_forge_is_asked() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);

    for base in [
        "x/../../../../user",
        "a..b",
        "a?b",
        "a#b",
        "a~1",
        "a^",
        "a:b",
        "a*",
        "a[b",
        "a\\b",
        "a@{1}",
        "a//b",
        "/a",
        "a/",
        "a.",
        "a.lock",
        "a\u{7f}b",
    ] {
        // Refused at once, the user is not asked; and the forge is not read for it.
        let answer = mcp.call(
            "forge",
            json!({"action": "edit", "number": 12, "base": base}),
        );
        assert!(
            answer.text.contains("refused: base:") && answer.text.contains("branch name"),
            "{base:?}: {}",
            answer.text
        );
    }
    assert!(gh_starting(&user, "api").is_empty());
    assert!(gh_starting(&user, "pr edit").is_empty());
}

#[test]
fn a_later_step_that_fails_says_what_the_earlier_ones_did() {
    // GitHub: the reviewer was asked, then `gh pr ready` failed.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    let mut script = github_pulls();
    script["step_fail"] = json!({"acme/app": {"ready": "HTTP 502: Bad Gateway"}});
    user.gh_script(script);
    let mcp = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "reviewers_add": ["carol"], "draft": false}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("HTTP 502"), "{}", answer.text);
    assert!(
        answer
            .text
            .contains("gh pr edit had already changed the pull request"),
        "{}",
        answer.text
    );

    // Bitbucket: the reviewer was asked, then `bkt pr publish` failed.
    let user = User::with_fake_agent().with_fake_bkt();
    on_bitbucket(&user, "", "");
    let mut script = cloud_script();
    script["step_fail"] = json!({"acme/app": {"publish": "Error: 502 Bad Gateway"}});
    user.bkt_script(script);
    let mcp = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "reviewers_add": ["carol"], "draft": false}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(answer.text.contains("502"), "{}", answer.text);
    assert!(
        answer
            .text
            .contains("bkt pr edit had already changed the pull request"),
        "{}",
        answer.text
    );

    // Nothing was done before a failure of the first step: nothing is said of it.
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    let mut script = github_pulls();
    script["step_fail"] = json!({"acme/app": {"edit": "HTTP 502: Bad Gateway"}});
    user.gh_script(script);
    let mcp = agent(&user);
    let answer = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "reviewers_add": ["carol"], "draft": false}),
    );
    assert!(answer.error, "{}", answer.text);
    assert!(!answer.text.contains("had already"), "{}", answer.text);
}

#[test]
fn an_edit_that_changes_nothing_is_refused_and_misplaced_arguments_before_the_gate() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    user.gh_script(github_pulls());
    let mcp = agent(&user);

    let nothing = mcp.call(
        "forge",
        json!({"action": "edit", "number": 12, "reviewers_add": []}),
    );
    assert!(
        nothing.text.contains(
            "refused: forge edit needs at least one of title, body, draft, reviewers_add, \
             reviewers_remove, labels_add, labels_remove or base"
        ),
        "{}",
        nothing.text
    );
    // An argument of another action is refused at once, nothing held.
    for (args, said) in [
        (
            json!({"action": "create", "title": "T", "reviewers_add": ["a"]}),
            "refused: forge create takes no reviewers_add: reviewers_add is for edit",
        ),
        (
            json!({"action": "edit", "number": 12, "title": "T", "reviewers": ["a"]}),
            "refused: forge edit takes no reviewers: reviewers is for create",
        ),
        (
            json!({"action": "merge", "labels": ["a"]}),
            "refused: forge merge takes no labels: labels is for create",
        ),
        (
            json!({"action": "edit", "number": 12, "title": "T", "delete_branch_on_merge": true}),
            "refused: forge edit takes no delete_branch_on_merge: delete_branch_on_merge is for \
             create",
        ),
        (
            json!({"action": "create", "title": "T", "base": "main"}),
            "refused: forge create takes no base: base is for edit",
        ),
        (
            json!({"action": "edit", "number": 12, "title": "T", "delete_branch": true}),
            "refused: forge edit takes no delete_branch: delete_branch is for close and merge",
        ),
        (
            json!({"action": "merge", "draft": true}),
            "refused: forge merge takes no draft: draft is for create and edit",
        ),
    ] {
        let answer = mcp.call("forge", args.clone());
        assert!(answer.text.contains(said), "{args}: {}", answer.text);
    }
    assert!(gh_starting(&user, "pr edit").is_empty());
}

#[test]
fn an_edit_of_a_pull_request_that_is_not_open_changes_nothing() {
    let user = User::with_fake_agent().with_fake_gh();
    on_github(&user, "", "");
    let mut script = github_pulls();
    script["pulls"]["acme/app"]["app/login"]["state"] = json!("MERGED");
    user.gh_script(script);
    let mcp = agent(&user);

    let answer = write(
        &user,
        &mcp,
        json!({"action": "edit", "number": 12, "draft": false, "labels_add": ["bug"]}),
    );

    assert!(
        answer
            .text
            .contains("is merged, not open; nothing was written"),
        "{}",
        answer.text
    );
    assert!(gh_starting(&user, "pr edit").is_empty());
    assert!(gh_starting(&user, "pr ready").is_empty());
}
