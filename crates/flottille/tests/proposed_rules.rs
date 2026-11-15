//! The scenarios of the rules proposed from the failures agents keep making (`docs/projects.md`,
//! "Proposed rules"): the same refusal in several sessions of a project makes one proposal, written
//! by the judge (a fake ACP agent with a scripted rule), which the user adds to the project's rules
//! or dismisses. The sessions are the fake agent's, whose scripted turns make the refused calls, on
//! a throwaway repository and temporary XDG folders.

mod common;

use std::time::{Duration, Instant};

use common::User;
use flottille_core::store::Store;
use serde_json::{Value, json};

/// The rule the fake judge writes.
const RULE: &str = "Never read the user's login folders such as ~/.ssh: ask the user for what \
                    you need from them.";

/// A token of the kind `secrets` recognises anywhere: never shown to the judge.
const TOKEN: &str = "ghp_fakefakefakefakefakefakefakefake0001";

/// How many letters lead the token in the long path: the refusal, cut at 484 characters, keeps the
/// first fourteen of the token.
const LONG: usize = 453;

/// A user whose sessions make, on their message `read the key <n>`, a call of `fs_read` refused
/// for a login's place, a different file each time; on `write it`, a `run` refused for shell
/// syntax. The judge, when there is one, says `judge`. Their judge is asked at every failure.
fn user_judged_by(judge: Option<Value>) -> User {
    let user = User::with_fake_agent().with_env("FLOTTILLE_RULES_EVERY_MS", "0");
    let user = match judge {
        Some(steps) => user.with_judge(steps),
        None => user,
    };
    let mut turns: Vec<Value> = (1..=9)
        .map(|n| {
            json!({"when": format!("read the key {n}"), "do": [
                {"mcp": {"tool": "fs_read", "arguments": {"path": format!("~/.ssh/id_{n}")}}},
                {"say": "done"}
            ]})
        })
        .collect();
    // A path so long that the refusal naming it is cut where the token ends: the cut falls in the
    // middle of the token.
    let long = format!("~/.ssh/{}/{TOKEN}", "a".repeat(LONG));
    turns.push(json!({"when": "read the long key", "do": [
        {"mcp": {"tool": "fs_read", "arguments": {"path": long}}},
        {"say": "done"}
    ]}));
    turns.push(json!({"when": "write it", "do": [
        {"mcp": {"tool": "run", "arguments": {"line": format!("echo {TOKEN} | tee notes.txt")}}},
        {"say": "done"}
    ]}));
    user.fake_script(json!({ "turns": turns }));
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user
}

/// `user_judged_by`, the judge writing `RULE`, or no judge at all.
fn user(judge: bool) -> User {
    user_judged_by(judge.then(|| json!([{"say": RULE}])))
}

/// A new session of the project whose first message is `message`, once its first turn ended: its
/// root's id.
fn session(user: &User, message: &str) -> String {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    ended(user, &id, 1);
    id
}

/// Waits for the agent's `turns`th turn to end.
fn ended(user: &User, agent: &str, turns: usize) {
    eventually("the turn's end", || {
        user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["name"] == "agent.turn_ended")
            .count()
            >= turns
    });
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn proposals(user: &User) -> Vec<Value> {
    user.call("rules.proposals", json!({"project": "app"}))
        .unwrap()["proposals"]
        .as_array()
        .unwrap()
        .clone()
}

/// The prompts the judge was given, in order.
fn judge_prompts(user: &User) -> Vec<String> {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .map(|e| {
            e["received"]["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn store(user: &User) -> Store {
    Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap()
}

/// Three sessions, each making the refused call of `fs_read` once: one pattern, seen three times
/// in three sessions.
fn three_sessions_refused(user: &User) -> Vec<String> {
    vec![
        session(user, "read the key 1"),
        session(user, "read the key 2"),
        session(user, "read the key 3"),
    ]
}

#[test]
fn three_same_refusals_in_two_sessions_make_one_proposal() {
    let user = user(true);
    let first = session(&user, "read the key 1");
    let second = session(&user, "read the key 2");
    // Twice: not yet worth a rule, and the judge is not asked.
    std::thread::sleep(Duration::from_millis(500));
    assert!(proposals(&user).is_empty());
    assert!(judge_prompts(&user).is_empty());

    user.call(
        "agent.send",
        json!({"agent": first, "message": "read the key 3"}),
    )
    .unwrap();
    ended(&user, &first, 2);

    eventually("the proposal", || proposals(&user).len() == 1);
    let proposal = &proposals(&user)[0];
    assert_eq!(proposal["rule"], RULE);
    assert_eq!(proposal["tool"], "fs_read");
    assert_eq!(proposal["count"], 3);
    assert_eq!(proposal["sessions"], 2);
    let calls = proposal["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 3);
    // Each failure names its session and its call, which the window opens.
    assert_eq!(calls[0]["session"], first);
    assert_eq!(calls[1]["session"], second);
    assert!(calls.iter().all(|call| call["seq"].as_u64().is_some()));
    assert_eq!(judge_prompts(&user).len(), 1);

    // The inbox says it, and the command line lists it.
    let inbox = user.call("inbox.list", json!({})).unwrap();
    let entry = inbox["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "proposed_rule")
        .cloned()
        .unwrap();
    assert_eq!(entry["project"], "app");
    assert_eq!(entry["proposal"], proposal["id"]);
    assert!(entry["text"].as_str().unwrap().contains(RULE), "{entry}");
    let listed = user.run(&["rules", "proposals"]);
    assert!(listed.status.success());
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.contains(RULE), "{listed}");
    assert!(listed.contains("3 times in 2 sessions"), "{listed}");
    let json = user.run(&["rules", "proposals", "--json"]);
    let json: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(json["proposals"][0]["id"], proposal["id"]);

    // A fourth failure of the pattern proposes it no more.
    session(&user, "read the key 4");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(proposals(&user).len(), 1);
    assert_eq!(judge_prompts(&user).len(), 1);

    // What the judge used is recorded as a turn of its own.
    let usage = store(&user).turn_usage().unwrap();
    let rules: Vec<_> = usage.iter().filter(|row| row.role == "rules").collect();
    assert_eq!(rules.len(), 1, "{usage:?}");
    assert_eq!(rules[0].project, "app");
    assert_eq!(rules[0].provider, "opencode");
}

#[test]
fn a_dismissed_pattern_is_never_proposed_again() {
    let user = user(true);
    three_sessions_refused(&user);
    eventually("the proposal", || proposals(&user).len() == 1);
    let id = proposals(&user)[0]["id"].clone();

    let dismissed = user
        .call("rules.proposal.dismiss", json!({"proposal": id}))
        .unwrap();
    assert_eq!(dismissed["state"], "dismissed");
    assert!(proposals(&user).is_empty());
    let entry = user.call("inbox.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "proposed_rule")
        .cloned()
        .unwrap();
    assert_eq!(entry["resolved"], true);

    // The same failure again, and another one that repeats: only the other is proposed.
    session(&user, "read the key 4");
    session(&user, "write it");
    session(&user, "write it");
    session(&user, "write it");
    eventually("the other proposal", || proposals(&user).len() == 1);
    assert_eq!(proposals(&user)[0]["tool"], "run");
    assert_eq!(judge_prompts(&user).len(), 2);
    // Answered once, it is answered.
    let again = user.call("rules.proposal.dismiss", json!({"proposal": id}));
    assert!(again.is_err(), "{again:?}");
}

#[test]
fn no_judge_no_proposal() {
    let user = user(false);
    three_sessions_refused(&user);
    std::thread::sleep(Duration::from_millis(1000));
    assert!(proposals(&user).is_empty());
    assert!(
        store(&user)
            .turn_usage()
            .unwrap()
            .iter()
            .all(|row| row.role != "rules")
    );
}

#[test]
fn a_failing_judge_proposes_nothing_and_is_asked_again_at_the_next_failure() {
    // An answer that is no rule.
    let user = user_judged_by(Some(json!([{"say": ""}])));
    three_sessions_refused(&user);
    eventually("the judge asked", || judge_prompts(&user).len() == 1);
    std::thread::sleep(Duration::from_millis(500));
    assert!(proposals(&user).is_empty());
    // Asked once for that failure, never in a loop.
    assert_eq!(judge_prompts(&user).len(), 1);

    session(&user, "read the key 4");
    eventually("the judge asked again", || judge_prompts(&user).len() == 2);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(judge_prompts(&user).len(), 2);
}

#[test]
fn an_accepted_rule_applies_to_the_next_session_not_the_running_one() {
    let user = user(true);
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("rules = \"Write tests first.\"\n{text}")).unwrap();
    let sessions = three_sessions_refused(&user);
    eventually("the proposal", || proposals(&user).len() == 1);
    let id = proposals(&user)[0]["id"].clone();
    let running = &sessions[0];
    let before = instructions(&user, running);

    let edited = "Never read ~/.ssh: ask the user instead.";
    let accepted = user
        .call(
            "rules.proposal.accept",
            json!({"proposal": id, "text": edited}),
        )
        .unwrap();
    assert_eq!(accepted["state"], "accepted");
    assert_eq!(accepted["rule"], edited);
    // At the end of the project's rules.
    let shown = user.call("rules.show", json!({"project": "app"})).unwrap();
    assert_eq!(shown["text"], format!("Write tests first.\n\n{edited}"));
    assert!(proposals(&user).is_empty());

    // The running session keeps the instructions it started with, for the prompt cache.
    user.call("agent.send", json!({"agent": running, "message": "hello"}))
        .unwrap();
    ended(&user, running, 2);
    assert_eq!(instructions(&user, running), before);
    assert!(!before.contains(edited));
    let next = session(&user, "hello");
    assert!(instructions(&user, &next).contains(edited));
}

/// The instructions an agent was given.
fn instructions(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["instructions"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn the_judge_never_sees_an_unmasked_secret() {
    let user = user(true);
    session(&user, "write it");
    session(&user, "write it");
    session(&user, "write it");
    eventually("the proposal", || proposals(&user).len() == 1);
    let prompts = judge_prompts(&user);
    assert_eq!(prompts.len(), 1);
    let prompt = &prompts[0];
    assert!(prompt.contains("shell syntax"), "{prompt}");
    // The call's line is there, its token masked.
    assert!(prompt.contains("echo [masked] | tee notes.txt"), "{prompt}");
    assert!(!prompt.contains(TOKEN), "{prompt}");
    assert!(!prompt.contains("fakefakefake"), "{prompt}");
    // Nor is the secret in what the user is shown.
    let shown = proposals(&user)[0].to_string();
    assert!(!shown.contains(TOKEN), "{shown}");
}

#[test]
fn a_refusal_s_stored_error_never_holds_part_of_a_secret() {
    let user = user(true);
    for _ in 0..3 {
        session(&user, "read the long key");
    }
    eventually("the proposal", || proposals(&user).len() == 1);
    // The cut leaves `ghp_` and ten letters of the token: too few for the mask to know it, and a
    // part of a secret all the same.
    let part = &TOKEN[..14];
    let stored = store(&user).refused_calls("app", 0).unwrap();
    assert_eq!(stored.len(), 3);
    for (event, _) in &stored {
        let error = event.data["error"].as_str().unwrap();
        // The refusal is the long one, cut: the test cuts where it means to.
        assert!(error.contains("‹+"), "{error}");
        assert!(!error.contains("ghp_"), "{error}");
        assert!(!error.contains(part), "{error}");
        assert!(
            !event.data.to_string().contains("fakefakefake"),
            "{}",
            event.data
        );
    }
    let shown = proposals(&user)[0].to_string();
    assert!(!shown.contains("fakefakefake"), "{shown}");
    let prompts = judge_prompts(&user);
    assert!(!prompts[0].contains("fakefakefake"), "{}", prompts[0]);
}
