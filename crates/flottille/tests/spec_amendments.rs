//! The scenarios of a spec amended during its development (`docs/extensions.md`, "Specs",
//! "Developing"): the development session proposes, through `spec` `amend`, a `C` or a `D` added
//! or changed, with why; the user accepts or refuses it; the session is told the answer with a
//! delivery. An accepted one is part of the spec, marked "decided during development" with its
//! date, a changed item keeping its identifier and its old text. A spec sent back to Draft during
//! its development tells the session, and `amend` is refused while it is a Draft. A fake ACP
//! agent, a fake `gh`, a throwaway repository, temporary XDG folders; no test calls GitHub.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

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

/// A project `app` of one repository whose `origin` is GitHub's `acme/app`, its issues its
/// tracker.
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
    let issue = json!({
        "number": 5, "title": "Users cannot find their orders", "state": "OPEN", "body": "",
        "url": "https://github.com/acme/app/issues/5",
        "author": {"login": "other"}, "assignees": [], "labels": [],
        "updatedAt": "2020-01-01T00:00:00Z", "comments": []
    });
    user.gh_script(json!({"pulls": {"acme/app": {}}, "issues": {"acme/app": {"5": issue}}}));
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    user.call("items.refresh", json!({"project": "app"}))
        .unwrap();
}

/// An internal spec of ticket 5, with one C and one D, made Ready.
fn ready_spec(user: &User) -> u64 {
    let spec = user
        .call(
            "spec.new",
            json!({"project": "app", "title": "Order filters", "tickets": ["#5"]}),
        )
        .unwrap()["spec"]
        .as_u64()
        .unwrap();
    for (section, content) in [
        ("problem", json!("Users cannot find their orders.")),
        (
            "behaviour",
            json!([{"text": "The orders list filters by status."}]),
        ),
        (
            "decisions",
            json!([{"question": "Where do the filters go?", "decision": "Above the list.",
                "set_aside": ["In a side panel"]}]),
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

/// "Launch development": the session's id, and its MCP client.
fn develop(user: &User, spec: u64) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user.call("spec.develop", json!({"spec": spec})).unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        answer["agent"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

/// A spec under development, and its development session.
fn developed(user: &User) -> (u64, String, Mcp) {
    let spec = ready_spec(user);
    let (agent, mcp) = develop(user, spec);
    eventually("the first message", || !prompts(user).is_empty());
    (spec, agent, mcp)
}

fn show(user: &User, spec: u64) -> Value {
    user.call("spec.show", json!({"spec": spec})).unwrap()
}

fn amendments(user: &User, spec: u64) -> Vec<Value> {
    user.call("spec.amendments", json!({"spec": spec})).unwrap()["amendments"]
        .as_array()
        .unwrap()
        .clone()
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    answer.text
}

/// A C added: filters as chips.
fn chips() -> Value {
    json!({"section": "behaviour", "content": {"text": "The filters are chips, one per status."},
        "why": "The user chose chips rather than fields while trying the first version."})
}

fn amend(mcp: &Mcp, spec: u64, amendment: Value) -> Answer {
    let mut args = json!({"action": "amend", "spec": spec});
    args.as_object_mut()
        .unwrap()
        .extend(amendment.as_object().unwrap().clone());
    mcp.call("spec", args)
}

/// The user's answer on the command line.
fn answer(user: &User, spec: u64, amendment: u64, answer: &str) {
    let output = user.run(&[
        "spec",
        "amendment",
        answer,
        &spec.to_string(),
        &amendment.to_string(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
fn only_the_development_session_can_propose_an_amendment() {
    let user = user();
    let (spec, developer, mcp) = developed(&user);
    let before = show(&user, spec);

    // Another root of the project, the user's own session: it has `spec`, not `amend` of it.
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "message": "lead the work"}),
    )
    .unwrap();
    let (url, authorization) = server_of(&user, opened);
    let other = Mcp::initialize(&url, &authorization);
    let text = refused(amend(&other, spec, chips()));
    assert!(text.starts_with("refused:"), "{text}");
    assert!(text.contains(&developer), "{text}");

    // A child of the development session has no `spec` at all.
    let opened = user.received_requests("session/new").len();
    done(mcp.call(
        "agent_launch",
        json!({"title": "reader", "brief": "read the code", "kind": "child"}),
    ));
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);
    let text = refused(amend(&child, spec, chips()));
    assert!(text.contains("has no tool spec"), "{text}");

    // The pilot door has no `spec` either.
    let pilot = Mcp::pilot(&user);
    let text = refused(amend(&pilot, spec, chips()));
    assert!(text.contains("spec"), "{text}");

    assert_eq!(amendments(&user, spec), Vec::<Value>::new());
    assert_eq!(show(&user, spec)["behaviour"], before["behaviour"]);

    // The development session proposes it: it waits for the user, the spec unchanged.
    let text = done(amend(&mcp, spec, chips()));
    assert!(text.contains("delivery"), "{text}");
    let listed = amendments(&user, spec);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["state"], "pending", "{listed:?}");
    assert_eq!(listed[0]["agent"], developer.as_str(), "{listed:?}");
    assert_eq!(
        listed[0]["why"],
        "The user chose chips rather than fields while trying the first version."
    );
    assert_eq!(show(&user, spec)["behaviour"], before["behaviour"]);
    assert_eq!(show(&user, spec)["amendments"].as_array().unwrap().len(), 1);
}

#[test]
fn an_accepted_amendment_is_part_of_the_spec_marked_with_its_date() {
    let user = user();
    let (spec, _, mcp) = developed(&user);
    done(amend(&mcp, spec, chips()));
    let n = amendments(&user, spec)[0]["amendment"].as_u64().unwrap();

    answer(&user, spec, n, "accept");

    let shown = show(&user, spec);
    let behaviour = shown["behaviour"].as_array().unwrap();
    assert_eq!(behaviour.len(), 2, "{shown}");
    let added = &behaviour[1];
    assert_eq!(added["id"], "C2", "{shown}");
    assert_eq!(added["text"], "The filters are chips, one per status.");
    let marked = &added["amended"][0];
    assert!(marked["at"].as_u64().is_some_and(|at| at > 0), "{shown}");
    assert_eq!(marked["was"], Value::Null, "{shown}");
    assert_eq!(amendments(&user, spec)[0]["state"], "accepted");
    assert_eq!(amendments(&user, spec)[0]["became"], "C2");
    // The spec is still under development: an amendment is the one write past Draft.
    assert_eq!(shown["status"], "in_progress", "{shown}");

    // Whoever reads it afterwards reads it as it is now, marked.
    let output = user.run(&["spec", "show", &spec.to_string()]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("C2: The filters are chips, one per status. (decided during development, "),
        "{text}"
    );
    let read = done(mcp.call("spec", json!({"action": "read", "spec": spec})));
    assert!(read.contains("The filters are chips"), "{read}");
}

#[test]
fn a_changed_item_keeps_its_identifier_and_its_old_text() {
    let user = user();
    let (spec, _, mcp) = developed(&user);
    done(amend(
        &mcp,
        spec,
        json!({"section": "behaviour",
            "content": {"id": "C1", "text": "The orders list filters by status with chips."},
            "why": "Chips rather than fields."}),
    ));
    done(amend(
        &mcp,
        spec,
        json!({"section": "decisions",
            "content": {"id": "D1", "question": "Where do the filters go?",
                "decision": "Above the list, as chips.", "set_aside": ["Fields"]},
            "why": "Chips rather than fields."}),
    ));
    for amendment in amendments(&user, spec) {
        answer(
            &user,
            spec,
            amendment["amendment"].as_u64().unwrap(),
            "accept",
        );
    }

    let shown = show(&user, spec);
    let c = &shown["behaviour"][0];
    assert_eq!(c["id"], "C1", "{shown}");
    assert_eq!(c["text"], "The orders list filters by status with chips.");
    assert_eq!(c["amended"][0]["was"], "The orders list filters by status.");
    let d = &shown["decisions"][0];
    assert_eq!(d["id"], "D1", "{shown}");
    assert_eq!(d["decision"], "Above the list, as chips.");
    assert_eq!(d["amended"][0]["was"]["decision"], "Above the list.");
    assert_eq!(shown["behaviour"].as_array().unwrap().len(), 1);

    // A new one afterwards takes the next identifier.
    done(amend(&mcp, spec, chips()));
    let n = amendments(&user, spec)[2]["amendment"].as_u64().unwrap();
    answer(&user, spec, n, "accept");
    assert_eq!(show(&user, spec)["behaviour"][1]["id"], "C2");
}

#[test]
fn a_refused_amendment_leaves_the_spec_unchanged() {
    let user = user();
    let (spec, _, mcp) = developed(&user);
    let before = show(&user, spec);
    done(amend(&mcp, spec, chips()));
    let n = amendments(&user, spec)[0]["amendment"].as_u64().unwrap();

    answer(&user, spec, n, "refuse");

    let after = show(&user, spec);
    for section in ["behaviour", "decisions", "problem", "status"] {
        assert_eq!(after[section], before[section], "{section}");
    }
    // Kept for the page, answered.
    let listed = amendments(&user, spec);
    assert_eq!(listed[0]["state"], "refused", "{listed:?}");
    assert!(listed[0]["answered_at"].is_u64(), "{listed:?}");
    // An answered amendment is not answered again.
    let (code, message) = user
        .call(
            "spec.amendment",
            json!({"spec": spec, "amendment": n, "answer": "accept"}),
        )
        .unwrap_err();
    assert_eq!(code, "refused", "{message}");
    assert_eq!(show(&user, spec)["behaviour"], before["behaviour"]);
}

#[test]
fn the_session_is_told_the_answer_with_a_delivery() {
    let user = user();
    let (spec, _, mcp) = developed(&user);
    done(amend(&mcp, spec, chips()));
    done(amend(
        &mcp,
        spec,
        json!({"section": "behaviour", "content": {"text": "Filters are remembered."},
            "why": "The user asked for it."}),
    ));
    let listed = amendments(&user, spec);
    let (first, second) = (
        listed[0]["amendment"].as_u64().unwrap(),
        listed[1]["amendment"].as_u64().unwrap(),
    );
    let before = prompts(&user).len();

    answer(&user, spec, first, "accept");
    eventually("the acceptance delivered", || {
        prompts(&user)[before..].iter().any(|p| {
            p.contains("[flottille:specs]")
                && p.contains(&format!("accepted amendment {first}"))
                && p.contains("C2")
        })
    });
    let before = prompts(&user).len();
    answer(&user, spec, second, "refuse");
    eventually("the refusal delivered", || {
        prompts(&user)[before..].iter().any(|p| {
            p.contains("[flottille:specs]") && p.contains(&format!("refused amendment {second}"))
        })
    });
}

#[test]
fn back_to_draft_during_the_development_tells_the_session() {
    let user = user();
    let (spec, _, mcp) = developed(&user);
    done(amend(&mcp, spec, chips()));
    assert_eq!(show(&user, spec)["status"], "in_progress");
    let before = prompts(&user).len();

    let output = user.run(&["spec", "draft", &spec.to_string()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(show(&user, spec)["status"], "draft");
    eventually("the development session told", || {
        prompts(&user)[before..].iter().any(|p| {
            p.contains("[flottille:specs]")
                && p.contains("back to Draft")
                && p.contains("development")
        })
    });
    // Its pending amendment stays for the user.
    let listed = amendments(&user, spec);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["state"], "pending", "{listed:?}");
}

#[test]
fn amend_is_refused_while_the_spec_is_draft() {
    let user = user();
    let (spec, _, mcp) = developed(&user);
    user.call("spec.draft", json!({"spec": spec})).unwrap();

    let text = refused(amend(&mcp, spec, chips()));

    assert!(
        text.starts_with(&format!(
            "refused: the spec is back to draft: spec {spec} is a Draft"
        )),
        "{text}"
    );
    assert_eq!(amendments(&user, spec), Vec::<Value>::new());
}

#[test]
fn a_session_whose_development_ended_cannot_amend() {
    let user = user();
    let (spec, first, _) = developed(&user);
    user.call("agent.stop", json!({"agent": first})).unwrap();
    eventually("the spec back to ready", || {
        show(&user, spec)["status"] == "ready"
    });
    let (second, mcp) = develop(&user, spec);

    // The user resumes the first session while the second develops the spec.
    let resumed = user.received_requests("session/resume").len();
    user.call("agent.resume", json!({"agent": first})).unwrap();
    user.call("agent.send", json!({"agent": first, "message": "go on"}))
        .unwrap();
    eventually("the first session resumed", || {
        user.received_requests("session/resume").len() > resumed
    });
    let server = &user.received_requests("session/resume")[resumed]["params"]["mcpServers"][0];
    let old = Mcp::initialize(
        server["url"].as_str().unwrap(),
        server["headers"][0]["value"].as_str().unwrap(),
    );

    let text = refused(amend(&old, spec, chips()));

    assert!(
        text.starts_with(&format!(
            "refused: spec {spec} is developed by session {second}"
        )),
        "{text}"
    );
    assert_eq!(amendments(&user, spec), Vec::<Value>::new());
    // The session developing it now proposes it.
    done(amend(&mcp, spec, chips()));
}

/// A C1 changed: the amendment's number.
fn change_c1(user: &User, mcp: &Mcp, spec: u64, text: &str) -> u64 {
    done(amend(
        mcp,
        spec,
        json!({"section": "behaviour", "content": {"id": "C1", "text": text},
            "why": "The user said so."}),
    ));
    amendments(user, spec).last().unwrap()["amendment"]
        .as_u64()
        .unwrap()
}

#[test]
fn accepting_an_amendment_whose_item_changed_since_is_refused() {
    let user = user();
    let (spec, _, mcp) = developed(&user);

    // Two amendments of C1: the newer accepted first.
    let older = change_c1(
        &user,
        &mcp,
        spec,
        "The orders list filters by status with fields.",
    );
    let newer = change_c1(
        &user,
        &mcp,
        spec,
        "The orders list filters by status with chips.",
    );
    answer(&user, spec, newer, "accept");
    let (code, message) = user
        .call(
            "spec.amendment",
            json!({"spec": spec, "amendment": older, "answer": "accept"}),
        )
        .unwrap_err();
    assert_eq!(code, "refused", "{message}");
    assert_eq!(
        message,
        format!(
            "C1 of spec {spec} changed since amendment {older} was proposed: it would undo what \
             was written since; refuse it"
        )
    );
    let shown = show(&user, spec);
    assert_eq!(
        shown["behaviour"][0]["text"], "The orders list filters by status with chips.",
        "{shown}"
    );
    assert_eq!(amendments(&user, spec)[0]["state"], "pending");

    // C1 rewritten while the spec was back to Draft.
    let pending = change_c1(&user, &mcp, spec, "The orders list filters by date.");
    user.call("spec.draft", json!({"spec": spec})).unwrap();
    user.call(
        "spec.write",
        json!({"spec": spec, "section": "behaviour",
            "content": [{"id": "C1", "text": "The orders list filters by status and date."}]}),
    )
    .unwrap();
    user.call("spec.ready", json!({"spec": spec})).unwrap();
    let (code, message) = user
        .call(
            "spec.amendment",
            json!({"spec": spec, "amendment": pending, "answer": "accept"}),
        )
        .unwrap_err();
    assert_eq!(code, "refused", "{message}");
    assert!(
        message.starts_with(&format!("C1 of spec {spec} changed since")),
        "{message}"
    );
    assert_eq!(
        show(&user, spec)["behaviour"][0]["text"],
        "The orders list filters by status and date."
    );
}

#[test]
fn accepting_an_amendment_while_the_spec_is_in_draft_is_refused() {
    let user = user();
    let (spec, _, mcp) = developed(&user);
    done(amend(&mcp, spec, chips()));
    let n = amendments(&user, spec)[0]["amendment"].as_u64().unwrap();
    user.call("spec.draft", json!({"spec": spec})).unwrap();
    let before = show(&user, spec);

    let (code, message) = user
        .call(
            "spec.amendment",
            json!({"spec": spec, "amendment": n, "answer": "accept"}),
        )
        .unwrap_err();

    assert_eq!(code, "refused", "{message}");
    assert_eq!(
        message,
        format!(
            "spec {spec} is back to Draft: make it Ready again before you accept an amendment, \
             or refuse it"
        )
    );
    assert_eq!(show(&user, spec)["behaviour"], before["behaviour"]);
    assert_eq!(amendments(&user, spec)[0]["state"], "pending");
    // Refusing it stays possible.
    answer(&user, spec, n, "refuse");
}

#[test]
fn a_changed_decision_keeps_its_question_and_set_aside_options() {
    let user = user();
    let (spec, _, mcp) = developed(&user);
    done(amend(
        &mcp,
        spec,
        json!({"section": "decisions",
            "content": {"id": "D1", "question": "How are the filters shown?",
                "decision": "As chips.", "set_aside": ["As fields"]},
            "why": "Chips rather than fields."}),
    ));
    let n = amendments(&user, spec)[0]["amendment"].as_u64().unwrap();

    answer(&user, spec, n, "accept");

    let d = &show(&user, spec)["decisions"][0];
    assert_eq!(d["question"], "How are the filters shown?", "{d}");
    assert_eq!(
        d["amended"][0]["was"],
        json!({"question": "Where do the filters go?", "decision": "Above the list.",
            "set_aside": ["In a side panel"]}),
        "{d}"
    );
}

#[test]
fn an_amended_item_rewritten_in_draft_is_no_longer_marked() {
    let user = user();
    let (spec, _, mcp) = developed(&user);
    let n = change_c1(
        &user,
        &mcp,
        spec,
        "The orders list filters by status with chips.",
    );
    done(amend(&mcp, spec, chips()));
    answer(&user, spec, n, "accept");
    let added = amendments(&user, spec)[1]["amendment"].as_u64().unwrap();
    answer(&user, spec, added, "accept");
    user.call("spec.draft", json!({"spec": spec})).unwrap();

    // C1 rewritten, C2 sent again as it is.
    user.call(
        "spec.write",
        json!({"spec": spec, "section": "behaviour", "content": [
            {"id": "C1", "text": "The orders list filters by date."},
            {"id": "C2", "text": "The filters are chips, one per status."}]}),
    )
    .unwrap();

    let shown = show(&user, spec);
    assert_eq!(shown["behaviour"][0]["amended"], Value::Null, "{shown}");
    assert_eq!(
        shown["behaviour"][1]["amended"][0]["was"],
        Value::Null,
        "{shown}"
    );
    assert!(
        shown["behaviour"][1]["amended"][0]["at"].is_u64(),
        "{shown}"
    );
}
