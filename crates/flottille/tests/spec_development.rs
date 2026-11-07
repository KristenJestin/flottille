//! The scenarios of a spec's development (`docs/extensions.md`, "Specs", "Developing"): "Launch
//! development" on a Ready spec starts a session for the user in a new workspace linked to the
//! spec's tickets, its first message the spec; and the statuses In progress, Review and Done are
//! read from what Flottille sees (the session, its pull requests), never set and forgotten. A
//! fake ACP agent, a fake `gh`, a throwaway repository whose fetch fails at once, temporary XDG
//! folders; no test calls GitHub.

mod common;

use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A time ahead of any cursor: what changed since the last round.
const NEW: &str = "2999-01-01T00:00:00Z";

/// A user whose daemon keeps the items through a fake `gh`, and whose `git` reaches no server.
fn user() -> User {
    let user = User::with_fake_agent()
        .with_fake_gh()
        .with_fetcher()
        .with_env("GIT_SSH_COMMAND", "false")
        .with_env("FLOTTILLE_TEST_CLOCK", "1");
    project(&user);
    user.call("extension.set", json!({"name": "specs", "enabled": true}))
        .unwrap();
    user
}

/// What `gh issue list --json` answers of an issue of `acme/app`.
fn issue(number: u64, title: &str) -> Value {
    json!({
        "number": number, "title": title, "state": "OPEN", "body": "",
        "url": format!("https://github.com/acme/app/issues/{number}"),
        "author": {"login": "other"}, "assignees": [], "labels": [],
        "updatedAt": "2020-01-01T00:00:00Z", "comments": []
    })
}

/// What `gh pr list --json` answers of the pull request `number` on `branch`.
fn pull(number: u64, branch: &str, state: &str) -> Value {
    json!({
        "number": number, "title": "Magic link sign-in", "state": state, "isDraft": false,
        "headRefName": branch, "baseRefName": "main",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "url": format!("https://github.com/acme/app/pull/{number}"),
        "author": {"login": "someone"}, "assignees": [], "labels": [],
        "updatedAt": NEW, "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED",
        "statusCheckRollup": [], "comments": [], "reviews": []
    })
}

/// A project `app` of one repository whose `origin` is GitHub's `acme/app`, its issues its
/// tracker, and its first round of the items done.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    pulls(user, json!({}));
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    refresh(user);
}

/// The fake `gh`'s pull requests of `acme/app`, by branch, beside issues 5 and 6.
fn pulls(user: &User, pulls: Value) {
    user.gh_script(json!({
        "pulls": {"acme/app": pulls},
        "issues": {"acme/app": {"5": issue(5, "Users cannot sign in"), "6": issue(6, "Typo")}}
    }));
}

fn refresh(user: &User) {
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

/// An internal spec of tickets 5 and 6, complete and made Ready.
fn ready_spec(user: &User) -> u64 {
    let spec = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "Magic link", "tickets": ["#5", "#6"]}),
        )
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    for (section, content) in [
        ("problem", json!("Users forget their passwords.")),
        (
            "behaviour",
            json!([{"text": "A link sent by mail signs the user in once."}]),
        ),
        (
            "verification",
            json!([{"id": "C1", "text": "Open the link twice: the second is refused."}]),
        ),
    ] {
        user.call(
            "spec.write",
            json!({"spec": spec, "section": section, "content": content}),
        )
        .unwrap();
    }
    user.call("spec.ready", json!({"spec": spec})).unwrap();
    spec
}

fn develop(user: &User, spec: u64) -> Value {
    user.call("spec.develop", json!({"spec": spec})).unwrap()
}

fn status(user: &User, spec: u64) -> String {
    user.call("spec.show", json!({"spec": spec})).unwrap()["status"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The texts of the prompts the fake agent was sent, in order.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|request| {
            request["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

#[test]
fn launch_development_starts_a_session_in_a_new_workspace_linked_to_the_tickets() {
    let user = user();
    let spec = ready_spec(&user);

    let answer = develop(&user, spec);

    let agent = answer["agent"].as_str().unwrap().to_owned();
    let workspace = answer["workspace"].as_str().unwrap().to_owned();
    let shown = user
        .call(
            "workspace.show",
            json!({"project": "app", "name": workspace}),
        )
        .unwrap();
    // A workspace has one ticket: the spec's first.
    assert_eq!(shown["ticket"], "#5", "{shown}");
    let tree = user.call("agent.tree", json!({"agent": agent})).unwrap();
    let entry = &tree["agents"][0];
    // A session for the user: a root, neither led nor handed by an agent.
    assert_eq!(entry["agent"]["workspace"], workspace.as_str(), "{entry}");
    assert_eq!(entry["agent"]["parent"], Value::Null, "{entry}");
    assert_eq!(entry["agent"]["launched_by"], Value::Null, "{entry}");
    assert_eq!(entry["agent"]["role"], "lead", "{entry}");
    // The extension's fact on the agent: the spec it develops.
    assert_eq!(entry["extensions"]["specs"]["spec"], spec, "{entry}");
    assert_eq!(
        entry["extensions"]["specs"]["work"], "developing",
        "{entry}"
    );
    // Its first message is sent once the workspace is prepared.
    eventually("the first message", || !prompts(&user).is_empty());
}

#[test]
fn its_first_message_is_the_spec_labelled_as_such() {
    let user = user();
    let spec = ready_spec(&user);
    develop(&user, spec);

    eventually("the first message", || !prompts(&user).is_empty());
    let first = prompts(&user)[0].clone();
    assert!(
        first.contains("This first message is Flottille's"),
        "{first}"
    );
    assert!(
        first.contains(&format!("The spec {spec}, as the user made it Ready")),
        "{first}"
    );
    assert!(first.contains("Users forget their passwords."), "{first}");
    assert!(
        first.contains("**C1** A link sent by mail signs the user in once."),
        "{first}"
    );
    // What Flottille asks of the work: the pull request's description.
    assert!(first.contains("pull request"), "{first}");
    assert!(first.contains("the Problem"), "{first}");
    assert!(first.contains("how each is verified"), "{first}");
    // An internal spec is linked by its number and its title.
    assert!(
        first.contains(&format!("spec {spec}, \"Magic link\"")),
        "{first}"
    );
}

#[test]
fn a_draft_spec_cannot_be_developed() {
    let user = user();
    let spec = user
        .call("spec.new", json!({"project": "app", "title": "Magic link"}))
        .unwrap()["spec"]
        .as_u64()
        .unwrap();

    let (code, message) = user
        .call("spec.develop", json!({"spec": spec}))
        .unwrap_err();

    assert_eq!(code, "refused", "{message}");
    assert!(message.contains("Draft"), "{message}");
    assert!(message.contains("Ready"), "{message}");
    let workspaces = user
        .call("workspace.list", json!({"project": "app"}))
        .unwrap();
    assert_eq!(workspaces["workspaces"], json!([]), "{workspaces}");
    assert!(user.received_requests("session/new").is_empty());
    assert_eq!(status(&user, spec), "draft");
}

#[test]
fn a_second_development_of_the_same_spec_is_refused() {
    let user = user();
    let spec = ready_spec(&user);
    let first = develop(&user, spec);

    let (code, message) = user
        .call("spec.develop", json!({"spec": spec}))
        .unwrap_err();

    assert_eq!(code, "refused", "{message}");
    assert!(message.contains("one development"), "{message}");
    assert!(
        message.contains(first["agent"].as_str().unwrap()),
        "{message}"
    );
    let workspaces = user
        .call("workspace.list", json!({"project": "app"}))
        .unwrap();
    assert_eq!(
        workspaces["workspaces"].as_array().unwrap().len(),
        1,
        "{workspaces}"
    );
}

#[test]
fn a_running_development_puts_the_spec_in_progress() {
    let user = user();
    let spec = ready_spec(&user);

    let answer = develop(&user, spec);

    assert_eq!(answer["spec"]["status"], "in_progress", "{answer}");
    assert_eq!(status(&user, spec), "in_progress");
    let listed = user
        .call(
            "spec.list",
            json!({"project": "app", "status": "in_progress"}),
        )
        .unwrap();
    assert_eq!(listed["specs"][0]["spec"], spec, "{listed}");
    // The development cannot be stopped by a write: the spec stays locked.
    let refused = user.call(
        "spec.write",
        json!({"spec": spec, "section": "notes", "content": "More."}),
    );
    assert!(refused.is_err(), "{refused:?}");
}

#[test]
fn its_pull_request_opened_puts_the_spec_in_review() {
    let user = user();
    let spec = ready_spec(&user);
    let answer = develop(&user, spec);
    let branch = answer["branch"].as_str().unwrap().to_owned();

    // Someone else's pull request moves nothing.
    pulls(&user, json!({"other": pull(3, "other", "OPEN")}));
    refresh(&user);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(status(&user, spec), "in_progress");

    pulls(
        &user,
        json!({"other": pull(3, "other", "OPEN"), branch.as_str(): pull(4, &branch, "OPEN")}),
    );
    refresh(&user);

    eventually("the spec in review", || status(&user, spec) == "review");
    // The session ending with its pull request open leaves it in review.
    let agent = answer["agent"].as_str().unwrap();
    user.call("agent.stop", json!({"agent": agent})).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(status(&user, spec), "review");
}

#[test]
fn its_pull_requests_merged_make_the_spec_done() {
    let user = user();
    let spec = ready_spec(&user);
    let answer = develop(&user, spec);
    let branch = answer["branch"].as_str().unwrap().to_owned();
    pulls(&user, json!({branch.as_str(): pull(4, &branch, "OPEN")}));
    refresh(&user);
    eventually("the spec in review", || status(&user, spec) == "review");

    let mut merged = pull(4, &branch, "MERGED");
    merged["updatedAt"] = json!("2999-01-02T00:00:00Z");
    pulls(&user, json!({branch.as_str(): merged}));
    refresh(&user);

    eventually("the spec done", || status(&user, spec) == "done");
}

#[test]
fn a_development_that_ends_without_a_pull_request_brings_the_spec_back_to_ready() {
    let user = user();
    let spec = ready_spec(&user);
    let answer = develop(&user, spec);
    assert_eq!(status(&user, spec), "in_progress");

    let agent = answer["agent"].as_str().unwrap();
    user.call("agent.stop", json!({"agent": agent})).unwrap();

    eventually("the spec back to ready", || status(&user, spec) == "ready");
    // Ready again, it can be developed again, in a workspace of its own, from the command line.
    let output = user.run(&["spec", "develop", &spec.to_string()]);
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        said.contains(&format!("develops it in workspace spec-{spec}-2")),
        "{said}"
    );
    assert_ne!(answer["workspace"], format!("spec-{spec}-2"));
    assert_eq!(status(&user, spec), "in_progress");
}

#[test]
fn nothing_launches_a_development_on_its_own_at_ready() {
    let user = user();
    let spec = ready_spec(&user);
    refresh(&user);

    std::thread::sleep(Duration::from_millis(500));

    assert_eq!(status(&user, spec), "ready");
    assert!(user.received_requests("session/new").is_empty());
    let agents = user.call("agent.list", json!({"project": "app"})).unwrap();
    assert_eq!(agents["agents"], json!([]), "{agents}");
    let workspaces = user
        .call("workspace.list", json!({"project": "app"}))
        .unwrap();
    assert_eq!(workspaces["workspaces"], json!([]), "{workspaces}");
}

/// The spec's workspaces, by name.
fn workspaces(user: &User) -> Vec<String> {
    user.call("workspace.list", json!({"project": "app"}))
        .unwrap()["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_resumed_development_puts_the_spec_back_in_progress() {
    let user = user();
    let spec = ready_spec(&user);
    let answer = develop(&user, spec);
    let agent = answer["agent"].as_str().unwrap();
    user.call("agent.stop", json!({"agent": agent})).unwrap();
    eventually("the spec back to ready", || status(&user, spec) == "ready");

    user.call("agent.resume", json!({"agent": agent})).unwrap();

    eventually("the spec in progress again", || {
        status(&user, spec) == "in_progress"
    });
    // Its session ending again brings it back, as the first time.
    user.call("agent.stop", json!({"agent": agent})).unwrap();
    eventually("the spec back to ready", || status(&user, spec) == "ready");
}

#[test]
fn a_development_that_ended_while_specs_was_off_does_not_leave_the_spec_in_progress() {
    let user = user();
    let spec = ready_spec(&user);
    let answer = develop(&user, spec);
    let agent = answer["agent"].as_str().unwrap();
    user.call("extension.set", json!({"name": "specs", "enabled": false}))
        .unwrap();
    user.call("agent.stop", json!({"agent": agent})).unwrap();

    user.call("extension.set", json!({"name": "specs", "enabled": true}))
        .unwrap();

    // The spec is read as what Flottille sees: no session runs behind it.
    assert_eq!(status(&user, spec), "ready");
    let listed = user.call("spec.list", json!({"project": "app"})).unwrap();
    assert_eq!(listed["specs"][0]["status"], "ready", "{listed}");
}

#[test]
fn a_done_spec_stays_done() {
    let user = user();
    let spec = ready_spec(&user);
    let answer = develop(&user, spec);
    let branch = answer["branch"].as_str().unwrap().to_owned();
    let agent = answer["agent"].as_str().unwrap();
    pulls(&user, json!({branch.as_str(): pull(4, &branch, "OPEN")}));
    refresh(&user);
    eventually("the spec in review", || status(&user, spec) == "review");
    let mut merged = pull(4, &branch, "MERGED");
    merged["updatedAt"] = json!("2999-01-02T00:00:00Z");
    pulls(&user, json!({branch.as_str(): merged}));
    refresh(&user);
    eventually("the spec done", || status(&user, spec) == "done");

    // A forge unset deletes the items of the project at the next round, the merged pull request
    // with them.
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": null}], "tracker": null}),
    )
    .unwrap();
    user.call("test.advance", json!({"ms": 0})).unwrap();
    eventually("the merged pull request forgotten", || {
        let listed = user
            .call("items.list", json!({"project": "app", "kind": "pull"}))
            .unwrap();
        listed["items"] == json!([])
    });
    user.call("agent.stop", json!({"agent": agent})).unwrap();
    std::thread::sleep(Duration::from_millis(500));

    assert_eq!(status(&user, spec), "done");
}

#[test]
fn a_draft_spec_is_untouched_by_pull_requests_and_by_sessions_ending() {
    let user = user();
    let spec = ready_spec(&user);
    let answer = develop(&user, spec);
    let branch = answer["branch"].as_str().unwrap().to_owned();
    let agent = answer["agent"].as_str().unwrap();
    user.call("agent.stop", json!({"agent": agent})).unwrap();
    eventually("the spec back to ready", || status(&user, spec) == "ready");
    user.call("spec.draft", json!({"spec": spec})).unwrap();

    // Its branch gets a pull request.
    pulls(&user, json!({branch.as_str(): pull(4, &branch, "OPEN")}));
    refresh(&user);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(status(&user, spec), "draft");

    // Its session starts again and ends again.
    user.call("agent.resume", json!({"agent": agent})).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(status(&user, spec), "draft");
    user.call("agent.stop", json!({"agent": agent})).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(status(&user, spec), "draft");
}

#[test]
fn two_launches_at_the_same_moment_start_one_development() {
    let user = user();
    let spec = ready_spec(&user);

    let answers: Vec<_> = std::thread::scope(|scope| {
        let launches: Vec<_> = (0..2)
            .map(|_| scope.spawn(|| user.call("spec.develop", json!({"spec": spec}))))
            .collect();
        launches
            .into_iter()
            .map(|launch| launch.join().unwrap())
            .collect()
    });

    assert_eq!(
        answers.iter().filter(|answer| answer.is_ok()).count(),
        1,
        "{answers:?}"
    );
    let refused = answers.iter().find_map(|answer| answer.as_ref().err());
    assert_eq!(refused.unwrap().0, "refused", "{answers:?}");
    let agents = user.call("agent.list", json!({"project": "app"})).unwrap();
    assert_eq!(agents["agents"].as_array().unwrap().len(), 1, "{agents}");
    // No workspace is left behind by the launch that was refused.
    assert_eq!(workspaces(&user), [format!("spec-{spec}")]);
}

#[test]
fn a_workspace_named_like_the_spec_does_not_block_its_development() {
    let user = user();
    let spec = ready_spec(&user);
    user.call(
        "workspace.create",
        json!({"project": "app", "name": format!("spec-{spec}")}),
    )
    .unwrap();

    let answer = develop(&user, spec);

    assert_eq!(answer["workspace"], format!("spec-{spec}-2"), "{answer}");
}

#[test]
fn a_failed_start_leaves_a_workspace_that_the_error_names_and_blocks_no_later_launch() {
    let user = user();
    let spec = ready_spec(&user);
    // Every provider turned off: the session cannot start once the workspace is made.
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/providers.toml"),
        "[claude]\nenabled = false\n\n[opencode]\nenabled = false\n\n[codex]\nenabled = false\n",
    )
    .unwrap();

    let (code, message) = user
        .call("spec.develop", json!({"spec": spec}))
        .unwrap_err();

    assert_eq!(code, "refused", "{message}");
    assert!(message.contains(&format!("spec-{spec}")), "{message}");
    assert_eq!(workspaces(&user), [format!("spec-{spec}")]);
    assert_eq!(status(&user, spec), "ready");

    std::fs::remove_file(user.path("config/flottille/providers.toml")).unwrap();
    let answer = develop(&user, spec);
    assert_eq!(answer["workspace"], format!("spec-{spec}-2"), "{answer}");
}
