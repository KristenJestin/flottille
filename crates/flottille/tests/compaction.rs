//! The scenarios of Flottille's own compaction (`docs/design.md` §5.8): the compactor, set like
//! the Auto judge, summarises an agent past its threshold at the end of a turn, and the agent goes
//! on in a new session from its brief, the summary and its latest messages. The agents and the
//! compactor are fake ACP agents (`crates/flottille-fake-agent`), in throwaway repositories and
//! temporary XDG folders.

mod common;

use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// What the scripted compactor answers.
const SUMMARY: &str = "## Goal\nAdd the login page.\n## Next steps\nWire the form to /api/login.";

/// The agent's turns, each played once (the new session's first prompt holds the words that
/// started them): `FULL` and `AGAIN` report a context past 70 % of its window, `HALF` 30 %. Each
/// says something worth keeping word for word, and long enough that a summary is smaller, with a
/// thought that is not kept.
fn script() -> Value {
    let details = format!(
        "Details: {}",
        "the form posts to the API and shows its errors. ".repeat(60)
    );
    json!({"turns": [
        {"when": "FULL", "times": 1, "do": [{"think": "a private thought"},
            {"say": "I edited src/login.rs and the tests pass.", "message": "m1"},
            {"say": details, "message": "m2"}, {"usage": {"used": 150_000, "size": 200_000}}]},
        {"when": "AGAIN", "times": 1, "do": [{"say": details}, {"usage": {"used": 160_000, "size": 200_000}}]},
        {"when": "HALF", "times": 1, "do": [{"say": details}, {"usage": {"used": 60_000, "size": 200_000}}]},
        {"when": "STILL", "do": [{"say": "Still at it."}, {"usage": {"used": 152_000, "size": 200_000}}]},
        {"when": "GROWN", "do": [{"say": details}, {"usage": {"used": 175_000, "size": 200_000}}]},
        {"when": "AFTER", "do": [{"say": "Back at it."}, {"usage": {"used": 25_000, "size": 200_000}}]},
        {"when": "HANG", "do": [{"hold": "release"}]},
        {"when": "earlier_conversation_summary", "do": [{"say": "Picking up."}, {"usage": {"used": 25_000, "size": 200_000}}]}
    ]})
}

/// A user whose agents are the fake agent playing `script()`, and whose compactor is OpenCode
/// standing on a fake ACP agent of its own (`fake-compactor`), each prompt of which plays `steps`.
/// `settings` is the rest of `compaction.toml`.
fn user_with(steps: Value, settings: &str) -> User {
    let user = User::with_fake_agent();
    user.fake_script(script());
    user.with_compactor(steps, settings)
}

fn user() -> User {
    user_with(json!([{"say": SUMMARY}]), "")
}

/// A project `app` over a throwaway repository, its ready workspace `w`, and a root agent working
/// there whose first message is `message`: its id.
fn session(user: &User, message: &str) -> String {
    started(user, message).0
}

/// `session`, and the number of sessions the fake agent opened before it.
fn started(user: &User, message: &str) -> (String, usize) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    user.call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": message}),
        )
        .unwrap();
    (answer["agent"]["id"].as_str().unwrap().to_owned(), opened)
}

/// `session`, and the agent's session of Flottille's MCP server, for a first turn that does not
/// compact it.
fn session_with_mcp(user: &User, message: &str) -> (String, Mcp) {
    let (id, opened) = started(user, message);
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization))
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The agent's events of that name, in order.
fn events(user: &User, agent: &str, name: &str) -> Vec<Value> {
    user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == name)
        .map(|e| e["data"].clone())
        .collect()
}

fn compacted(user: &User, agent: &str) -> Vec<Value> {
    events(user, agent, "agent.compacted")
}

fn failed(user: &User, agent: &str) -> Vec<Value> {
    events(user, agent, "agent.compaction_failed")
}

fn turns_ended(user: &User, agent: &str) -> usize {
    events(user, agent, "agent.turn_ended").len()
}

fn send(user: &User, agent: &str, message: &str) {
    user.call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
}

/// The texts of the prompts the agent's fake received, in order.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|p| {
            p["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// The prompts the compactor was given, in order.
fn compactor_prompts(user: &User) -> Vec<String> {
    user.compactor_received()
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

/// Nothing more happens for a while: no compaction is under way.
fn quiet(user: &User, agent: &str) {
    std::thread::sleep(Duration::from_millis(1_500));
    assert!(compacted(user, agent).is_empty(), "it was compacted");
    assert!(
        compactor_prompts(user).is_empty(),
        "the compactor was asked"
    );
}

#[test]
fn an_agent_past_the_threshold_is_compacted_at_the_end_of_its_turn() {
    let user = user();
    let agent = session(&user, "add the login page FULL");

    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    let event = &compacted(&user, &agent)[0];
    assert_eq!(event["by"], "flottille", "{event}");
    assert_eq!(event["before"], 150_000, "{event}");
    assert!(event["after"].as_u64().unwrap() < 150_000, "{event}");
    assert_eq!(event["summary"], SUMMARY, "{event}");
    // After the turn's end, never in it.
    let shown = user.call("agent.show", json!({"agent": agent})).unwrap();
    let names: Vec<&str> = shown["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    let ended = names.iter().position(|n| *n == "agent.turn_ended").unwrap();
    let done = names.iter().position(|n| *n == "agent.compacted").unwrap();
    assert!(ended < done, "{names:?}");
    // A new session of its CLI.
    eventually("the new session", || {
        user.received_requests("session/new").len() == 2
    });
}

#[test]
fn the_maximum_size_compacts_before_the_threshold() {
    let user = user_with(json!([{"say": SUMMARY}]), "maximum = 50000");
    let agent = session(&user, "add the login page HALF");

    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    assert_eq!(compacted(&user, &agent)[0]["before"], 60_000);
}

#[test]
fn automatic_off_compacts_only_by_hand() {
    let user = user_with(json!([{"say": SUMMARY}]), "automatic = false");
    let agent = session(&user, "add the login page FULL");
    eventually("the turn's end", || turns_ended(&user, &agent) == 1);
    quiet(&user, &agent);

    let output = user.run(&["agent", "compact", &agent]);
    assert!(output.status.success(), "{output:?}");

    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    assert_eq!(compacted(&user, &agent)[0]["summary"], SUMMARY);
}

#[test]
fn the_compactor_is_given_the_transcript_the_previous_summary_and_the_guidance() {
    let user = user();
    user.call(
        "compaction.set_guidance",
        json!({"text": "Keep every ticket number."}),
    )
    .unwrap();
    let agent = session(&user, "add the login page FULL");
    eventually("the first compaction", || {
        compacted(&user, &agent).len() == 1
    });

    send(&user, &agent, "now the logout AGAIN");
    eventually("the second compaction", || {
        compacted(&user, &agent).len() == 2
    });

    // The guidance is in its instructions, the text OpenCode reads them from.
    let instructions =
        std::fs::read_to_string(user.path("runtime/flottille/opencode/compactor.md")).unwrap();
    assert!(
        instructions.contains("Keep every ticket number."),
        "{instructions}"
    );
    assert!(instructions.starts_with("You are Flottille's compactor."));
    let given = compactor_prompts(&user);
    assert_eq!(given.len(), 2, "{given:?}");
    let first = &given[0];
    assert!(first.contains("add the login page"), "{first}");
    assert!(
        first.contains("I edited src/login.rs and the tests pass."),
        "{first}"
    );
    assert!(!first.contains("a private thought"), "{first}");
    // The second updates the first summary, given the turns since.
    let second = &given[1];
    assert!(second.contains(SUMMARY), "{second}");
    assert!(second.contains("now the logout"), "{second}");
}

#[test]
fn a_summary_not_smaller_than_what_it_replaces_is_refused() {
    let long = "a summary longer than its conversation ".repeat(400);
    let user = user_with(json!([{"say": long}]), "");
    let agent = session(&user, "add the login page FULL");

    eventually("the failure", || !failed(&user, &agent).is_empty());
    let reason = failed(&user, &agent)[0]["reason"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(reason.contains("not smaller"), "{reason}");
    assert!(compacted(&user, &agent).is_empty());
    assert_eq!(user.received_requests("session/new").len(), 1);
}

#[test]
fn a_summary_cut_off_by_length_or_calling_a_tool_is_refused() {
    for (steps, why) in [
        (
            json!({"turns": [{"when": "", "do": [{"say": SUMMARY}], "stop": "max_tokens"}]}),
            "cut off",
        ),
        (
            json!({"turns": [{"when": "", "do": [{"update": {"sessionUpdate": "tool_call",
                "toolCallId": "t1", "title": "Read a file", "kind": "read", "status": "pending"}},
                {"say": SUMMARY}]}]}),
            "tool",
        ),
    ] {
        let user = user();
        std::fs::write(user.path("fake-compactor/script.json"), steps.to_string()).unwrap();
        let agent = session(&user, "add the login page FULL");

        eventually("the failure", || !failed(&user, &agent).is_empty());
        let reason = failed(&user, &agent)[0]["reason"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(reason.contains(why), "{reason}");
        assert!(compacted(&user, &agent).is_empty());
    }
}

#[test]
fn a_compacted_agent_keeps_its_id_children_subscriptions_and_pending_deliveries() {
    // The compactor takes its time, so that a message comes while it works.
    let user = user_with(json!([{"hold": "release"}, {"say": SUMMARY}]), "");
    let (agent, mcp) = session_with_mcp(&user, "lead the work");
    eventually("the first turn's end", || turns_ended(&user, &agent) == 1);
    let launched = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "tests", "brief": "write the tests HANG", "workspace": "w"}),
    );
    assert!(!launched.error, "{}", launched.text);
    let child = launched
        .text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| w.len() == 12 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap()
        .to_owned();
    let woken: Answer = mcp.call("wake", json!({"minutes": 30, "why": "look at the build"}));
    assert!(!woken.error, "{}", woken.text);

    send(&user, &agent, "go on FULL");
    eventually("the compactor at work", || {
        compactor_prompts(&user).len() == 1
    });
    // A message sent while it compacts waits for the new session.
    send(&user, &agent, "and mind the footer");
    std::fs::write(user.path("fake-compactor/release"), "").unwrap();
    eventually("the compaction", || !compacted(&user, &agent).is_empty());

    let shown = user.call("agent.show", json!({"agent": agent})).unwrap();
    assert_eq!(shown["agent"]["id"], agent.as_str());
    let tree = user.call("agent.tree", json!({"agent": agent})).unwrap();
    assert!(tree.to_string().contains(&child), "{tree}");
    let child_shown = user.call("agent.show", json!({"agent": child})).unwrap();
    assert_eq!(child_shown["agent"]["parent"], agent.as_str());
    let subscriptions = user
        .call("subscription.list", json!({"tree": agent}))
        .unwrap();
    assert!(
        subscriptions.to_string().contains("look at the build"),
        "{subscriptions}"
    );
    // The message held went to the new session, after its summary, never to the old one.
    eventually("the message held, then delivered", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("and mind the footer"))
    });
    let given = prompts(&user);
    let summary = given.iter().position(|p| p.contains(SUMMARY)).unwrap();
    let held = given
        .iter()
        .position(|p| p.contains("and mind the footer"))
        .unwrap();
    assert!(summary <= held, "{given:#?}");
}

#[test]
fn the_new_session_gets_the_brief_the_summary_and_the_latest_messages() {
    let user = user();
    let agent = session(&user, "add the login page FULL");
    eventually("the compaction", || !compacted(&user, &agent).is_empty());

    send(&user, &agent, "go on");
    eventually("the new session's first prompt", || {
        prompts(&user).iter().any(|p| p.contains(SUMMARY))
    });
    let first = prompts(&user)
        .into_iter()
        .find(|p| p.contains(SUMMARY))
        .unwrap();
    let brief = first.find("add the login page").expect("the brief");
    let summary = first.find(SUMMARY).unwrap();
    let kept = first
        .rfind("I edited src/login.rs and the tests pass.")
        .expect("the latest messages, word for word");
    assert!(brief < summary && summary < kept, "{first}");
    assert!(!first.contains("a private thought"), "{first}");
}

#[test]
fn a_session_that_cannot_be_loaded_starts_from_the_summary_and_the_latest_messages() {
    let user = user();
    // Its session is lost once its process is gone: it cannot be loaded again.
    let mut lost = script();
    lost["load_error"] = "the session file is corrupt".into();
    lost["turns"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"when": "DIE", "times": 1, "do": [{"die": 0}]}));
    user.fake_script(lost);
    let agent = session(&user, "add the login page HALF");
    eventually("the first turn", || turns_ended(&user, &agent) == 1);
    let native =
        user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["native_id"].clone();

    send(&user, &agent, "DIE now");
    eventually("the new session's first prompt", || {
        prompts(&user).iter().any(|p| p.contains(SUMMARY))
    });

    let first = prompts(&user)
        .into_iter()
        .find(|p| p.contains(SUMMARY))
        .unwrap();
    assert!(first.contains("could not be loaded"), "{first}");
    assert!(first.contains("the session file is corrupt"), "{first}");
    let brief = first
        .find("add the login page HALF")
        .expect("the first message");
    let summary = first.find(SUMMARY).unwrap();
    let kept = first
        .rfind("the form posts to the API")
        .expect("the latest messages, word for word");
    assert!(brief < summary && summary < kept, "{first}");
    assert!(first.contains("DIE now"), "the message it died on: {first}");
    assert_eq!(compactor_prompts(&user).len(), 1);
    assert_eq!(user.received_requests("session/new").len(), 2);
    let shown = user.call("agent.show", json!({"agent": agent})).unwrap();
    assert_ne!(shown["agent"]["native_id"], native);
    assert_eq!(shown["agent"]["epoch"], 1);
}

#[test]
fn a_failed_compaction_leaves_the_agent_in_its_session() {
    let user = user_with(json!([{"die": 3}]), "");
    let agent = session(&user, "add the login page FULL");
    eventually("the failure", || !failed(&user, &agent).is_empty());

    send(&user, &agent, "go on HALF");
    eventually("the next turn", || turns_ended(&user, &agent) == 2);
    assert!(compacted(&user, &agent).is_empty());
    assert_eq!(user.received_requests("session/new").len(), 1);
    let last = prompts(&user).pop().unwrap();
    assert!(last.contains("go on") && !last.contains(SUMMARY), "{last}");
}

#[test]
fn a_user_who_never_chose_gets_a_compactor_picked_and_told_once() {
    // Claude installed (the fake `claude`) and signed in: the daemon picks its smallest model.
    let user = User::with_fake_claude();
    let shown = user.call("compaction.show", json!({})).unwrap();
    assert_eq!(
        shown["compactor"],
        json!({"provider": "claude", "model": "haiku", "picked": true}),
        "{shown}"
    );
    let text = std::fs::read_to_string(user.path("config/flottille/compaction.toml")).unwrap();
    assert!(text.contains("Picked by Flottille"), "{text}");

    user.call("compaction.noticed", json!({})).unwrap();
    let shown = user.call("compaction.show", json!({})).unwrap();
    assert_eq!(
        shown["compactor"],
        json!({"provider": "claude", "model": "haiku"})
    );
    // Said once: one event of the pick.
    let picked = user
        .watch(&["--after", "0"])
        .within(Duration::from_millis(500))
        .into_iter()
        .filter(|e| e["name"] == "compaction.changed" && e["data"]["compactor"]["picked"] == true)
        .count();
    assert_eq!(picked, 1);
}

#[test]
fn none_leaves_each_cli_to_compact_on_its_own() {
    let user = user();
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "compactor = \"none\"\n",
    )
    .unwrap();
    let agent = session(&user, "add the login page FULL");
    eventually("the turn's end", || turns_ended(&user, &agent) == 1);
    quiet(&user, &agent);
    let refused = user
        .call("agent.compact", json!({"agent": agent}))
        .unwrap_err();
    assert!(refused.1.contains("no compaction model"), "{refused:?}");
    let shown = user.call("compaction.show", json!({})).unwrap();
    assert_eq!(shown["compactor"], Value::Null, "{shown}");
}

#[test]
fn the_settings_are_kept_and_refused_out_of_range() {
    let user = user();
    let shown = user
        .call(
            "compaction.set",
            json!({"automatic": false, "threshold": 80, "maximum": 120000, "kept": 10000}),
        )
        .unwrap();
    assert_eq!(shown["automatic"], false);
    assert_eq!(shown["threshold"], 80);
    assert_eq!(shown["maximum"], 120000);
    assert_eq!(shown["kept"], 10000);
    assert_eq!(shown["compactor"]["provider"], "opencode");
    for wrong in [
        json!({"threshold": 5}),
        json!({"threshold": 40}),
        json!({"threshold": 100}),
        json!({"kept": -1}),
        json!({"kept": 60000}),
        json!({"maximum": 0}),
        json!({"maximum": 19999}),
    ] {
        let refused = user.call("compaction.set", wrong.clone()).unwrap_err();
        assert_eq!(refused.0, "invalid-params", "{wrong}: {refused:?}");
    }
    let shown = user
        .call("compaction.set", json!({"maximum": "none"}))
        .unwrap();
    assert_eq!(shown["maximum"], Value::Null);
}

#[test]
fn a_failed_compaction_is_not_tried_again_at_every_turn_end() {
    let user = user_with(json!([{"die": 3}]), "");
    let agent = session(&user, "add the login page FULL");
    eventually("the failure", || !failed(&user, &agent).is_empty());
    eventually("the first turn's end", || turns_ended(&user, &agent) == 1);

    // Still past the threshold, but not grown by a tenth of the window: not tried again.
    send(&user, &agent, "STILL one");
    eventually("the second turn's end", || turns_ended(&user, &agent) == 2);
    send(&user, &agent, "STILL two");
    eventually("the third turn's end", || turns_ended(&user, &agent) == 3);
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(compactor_prompts(&user).len(), 1, "tried again");
    assert_eq!(failed(&user, &agent).len(), 1);

    // Grown by more than that: tried again.
    send(&user, &agent, "GROWN");
    eventually("a second try", || compactor_prompts(&user).len() == 2);
}

#[test]
fn a_failed_compaction_can_still_be_asked_for_by_hand() {
    let user = user_with(json!([{"die": 3}]), "");
    let agent = session(&user, "add the login page FULL");
    eventually("the failure", || !failed(&user, &agent).is_empty());
    eventually("the first turn's end", || turns_ended(&user, &agent) == 1);

    user.call("agent.compact", json!({"agent": agent})).unwrap();
    eventually("a second try", || failed(&user, &agent).len() == 2);
}

#[test]
fn a_new_session_that_starts_past_the_maximum_is_not_compacted_again() {
    let user = user_with(json!([{"say": SUMMARY}]), "maximum = 20000");
    let agent = session(&user, "add the login page FULL");
    eventually("the compaction", || compacted(&user, &agent).len() == 1);

    // Its first turn, on the brief and the summary, reports 25k: past the maximum already.
    send(&user, &agent, "AFTER one");
    eventually("its turns", || turns_ended(&user, &agent) >= 3);
    send(&user, &agent, "AFTER two");
    eventually("one more turn", || turns_ended(&user, &agent) >= 4);
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(compacted(&user, &agent).len(), 1, "compacted again");
    assert_eq!(compactor_prompts(&user).len(), 1);
}

#[test]
fn a_compaction_asked_while_one_runs_is_answered_that_one_runs() {
    let user = user_with(json!([{"hold": "release"}, {"say": SUMMARY}]), "");
    let agent = session(&user, "add the login page FULL");
    eventually("the compactor at work", || {
        compactor_prompts(&user).len() == 1
    });

    let refused = user
        .call("agent.compact", json!({"agent": agent}))
        .unwrap_err();
    assert!(refused.1.contains("a compaction runs"), "{refused:?}");
    std::fs::write(user.path("fake-compactor/release"), "").unwrap();
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(compacted(&user, &agent).len(), 1);
    assert_eq!(compactor_prompts(&user).len(), 1);
}

#[test]
fn a_picked_compactor_compacts_only_by_hand_until_its_notice_is_seen() {
    let user = user();
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "[compactor]\nprovider = \"opencode\"\npicked = true\n",
    )
    .unwrap();
    let agent = session(&user, "add the login page FULL");
    eventually("the turn's end", || turns_ended(&user, &agent) == 1);
    quiet(&user, &agent);

    user.call("compaction.noticed", json!({})).unwrap();
    send(&user, &agent, "now the logout AGAIN");
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
}

#[test]
fn the_summary_is_framed_as_data_a_model_wrote() {
    let user = user();
    let agent = session(&user, "add the login page FULL");
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    send(&user, &agent, "go on");
    eventually("the new session's first prompt", || {
        prompts(&user).iter().any(|p| p.contains(SUMMARY))
    });
    let first = prompts(&user)
        .into_iter()
        .find(|p| p.contains(SUMMARY))
        .unwrap();
    let opens = first
        .find("<earlier_conversation_summary>")
        .expect("its block");
    let closes = first.find("</earlier_conversation_summary>").unwrap();
    let summary = first.find(SUMMARY).unwrap();
    assert!(opens < summary && summary < closes, "{first}");
    assert!(
        first.contains("written by a model") && first.contains("not the user's words"),
        "{first}"
    );
}

#[test]
fn a_secret_in_the_summary_is_masked() {
    const SECRET: &str = "s3cr3t-deploy-value-9876";
    let said = format!("{SUMMARY}\nThe deploy key is {SECRET}.");
    let user = User::with_fake_agent().with_env("DEPLOY_SECRET", SECRET);
    user.fake_script(script());
    let user = user.with_compactor(json!([{"say": said}]), "");
    let agent = session(&user, "add the login page FULL");
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    let event = compacted(&user, &agent)[0].to_string();
    assert!(!event.contains(SECRET), "{event}");
    send(&user, &agent, "go on");
    eventually("the new session's first prompt", || {
        prompts(&user).iter().any(|p| p.contains("Wire the form"))
    });
    assert!(!prompts(&user).iter().any(|p| p.contains(SECRET)));
}

// The compaction around the expiry of the prompt cache (`docs/design.md` §5.8, "Around the
// cache's expiry"). The agents stand for Claude, signed in without an API key here: their cache
// lives an hour. The daemon's clock is moved ahead (`test.advance`).

const MINUTE: u64 = 60_000;

/// `user_with` a scripted summary, the threshold left out of the way and the daemon's clock moved
/// by the test.
fn clocked(settings: &str) -> User {
    user_with(
        json!([{"say": SUMMARY}]),
        &format!("automatic = false\n{settings}"),
    )
    .with_env("FLOTTILLE_TEST_CLOCK", "1")
}

/// An agent whose first turn (`HALF`, 60k tokens of context) ended: its id.
fn at_rest(user: &User) -> String {
    let agent = session(user, "add the login page HALF");
    eventually("the first turn's end", || turns_ended(user, &agent) == 1);
    agent
}

fn advance(user: &User, ms: u64) {
    user.call("test.advance", json!({"ms": ms})).unwrap();
}

#[test]
fn an_agent_at_rest_is_compacted_the_margin_before_its_cache_expires() {
    let user = clocked("before_expiry = true");
    let agent = at_rest(&user);

    advance(&user, 54 * MINUTE);
    quiet(&user, &agent);
    advance(&user, MINUTE);
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    assert_eq!(compacted(&user, &agent)[0]["before"], 60_000);
    // Once: its new session is not compacted again while nothing grew.
    eventually("the new session's first turn", || {
        prompts(&user).iter().any(|p| p.contains(SUMMARY))
    });
    advance(&user, 61 * MINUTE);
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(compacted(&user, &agent).len(), 1);
}

#[test]
fn an_agent_with_a_turn_running_is_not_compacted_before_expiry() {
    let user = clocked("before_expiry = true");
    let agent = at_rest(&user);

    advance(&user, 54 * MINUTE);
    send(&user, &agent, "hold on HANG");
    eventually("the turn that runs", || {
        prompts(&user).iter().any(|p| p.contains("hold on"))
    });
    advance(&user, 2 * MINUTE);
    quiet(&user, &agent);
}

#[test]
fn a_message_to_an_agent_whose_cache_expired_compacts_first() {
    let user = clocked("");
    let agent = at_rest(&user);

    advance(&user, 61 * MINUTE);
    // Said once, for the composer's toggle, ticked from the setting.
    advance(&user, MINUTE);
    let expired = events(&user, &agent, "agent.cache_expired");
    assert_eq!(expired.len(), 1, "{expired:?}");
    assert_eq!(expired[0]["compact"], true);
    send(&user, &agent, "go on AFTER");
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    eventually("the message in the new session", || {
        prompts(&user).iter().any(|p| p.contains("go on AFTER"))
    });
    assert_eq!(user.received_requests("session/new").len(), 2);
    let given = prompts(&user);
    let summary = given.iter().position(|p| p.contains(SUMMARY)).unwrap();
    let message = given
        .iter()
        .position(|p| p.contains("go on AFTER"))
        .unwrap();
    assert!(summary <= message, "{given:#?}");
}

#[test]
fn a_message_before_the_expiry_is_sent_as_it_is() {
    let user = clocked("");
    let agent = at_rest(&user);

    advance(&user, 59 * MINUTE);
    send(&user, &agent, "go on AFTER");
    eventually("the next turn", || turns_ended(&user, &agent) == 2);
    quiet(&user, &agent);
    assert_eq!(user.received_requests("session/new").len(), 1);
}

#[test]
fn below_the_minimum_size_neither_trigger_compacts() {
    let user = clocked("before_expiry = true\nminimum = 100000");
    let agent = at_rest(&user);

    advance(&user, 61 * MINUTE);
    quiet(&user, &agent);
    send(&user, &agent, "go on AFTER");
    eventually("the next turn", || turns_ended(&user, &agent) == 2);
    quiet(&user, &agent);
}

#[test]
fn unticking_the_toggle_sends_without_compacting() {
    let user = clocked("");
    let agent = at_rest(&user);

    advance(&user, 61 * MINUTE);
    user.call(
        "agent.send",
        json!({"agent": agent, "message": "go on AFTER", "compact": false}),
    )
    .unwrap();
    eventually("the next turn", || turns_ended(&user, &agent) == 2);
    quiet(&user, &agent);
}

#[test]
fn ticking_the_toggle_compacts_even_with_the_setting_off() {
    let user = clocked("after_expiry = false");
    let agent = at_rest(&user);

    advance(&user, 61 * MINUTE);
    user.call(
        "agent.send",
        json!({"agent": agent, "message": "go on AFTER", "compact": true}),
    )
    .unwrap();
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
}

#[test]
fn a_parent_woken_by_its_child_follows_the_setting() {
    let user = clocked("");
    let (agent, mcp) = session_with_mcp(&user, "lead the work HALF");
    eventually("the first turn's end", || turns_ended(&user, &agent) == 1);

    advance(&user, 61 * MINUTE);
    // The child ends two turns without a report: its parent is told, by Flottille.
    let launched = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "tests", "brief": "write the tests", "workspace": "w"}),
    );
    assert!(!launched.error, "{}", launched.text);
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
}

#[test]
fn without_a_compactor_neither_trigger_runs() {
    let user = clocked("");
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "compactor = \"none\"\nautomatic = false\nbefore_expiry = true\n",
    )
    .unwrap();
    let agent = at_rest(&user);

    advance(&user, 61 * MINUTE);
    quiet(&user, &agent);
    send(&user, &agent, "go on AFTER");
    eventually("the next turn", || turns_ended(&user, &agent) == 2);
    quiet(&user, &agent);
}

#[test]
fn the_expiry_settings_and_the_lifetimes_are_shown_and_kept() {
    let user = user();
    let shown = user.call("compaction.show", json!({})).unwrap();
    assert_eq!(shown["before_expiry"], false, "{shown}");
    assert_eq!(shown["margin"], 5, "{shown}");
    assert_eq!(shown["after_expiry"], true, "{shown}");
    assert_eq!(shown["minimum"], 50_000, "{shown}");
    assert_eq!(shown["lifetimes"]["claude"], 3_600, "{shown}");
    assert_eq!(shown["lifetimes"]["codex"], Value::Null, "{shown}");
    assert_eq!(shown["lifetimes"]["opencode"], Value::Null, "{shown}");

    let set = user
        .call(
            "compaction.set",
            json!({"before_expiry": true, "margin": 10, "after_expiry": false, "minimum": 80_000}),
        )
        .unwrap();
    assert_eq!(set["before_expiry"], true, "{set}");
    assert_eq!(set["margin"], 10, "{set}");
    assert_eq!(set["after_expiry"], false, "{set}");
    assert_eq!(set["minimum"], 80_000, "{set}");
    let refused = user
        .call("compaction.set", json!({"margin": 0}))
        .unwrap_err();
    assert!(refused.1.contains("margin"), "{refused:?}");
}

// The summary written by the agent itself (#405): `summary = "agent"` in `compaction.toml`, no
// compactor needed. Flottille asks the agent for its handoff in a message of its own, then starts
// it again in a new session from its brief, the handoff and what Flottille holds of it.

/// What the scripted agent writes when Flottille asks for its handoff.
const HANDOFF: &str = "## Task\nAdd the login page.\n## About to do\nWire the form to /api/login.";

/// The agent's turns: the handoff, `FULL` past the threshold (72 % of the window, under the
/// threshold + 5), `RUSH` far past it (80 %) and still running, and the new session's first turn,
/// with the handoff or without it. `handoff` is what the agent does when asked for it.
fn handoff_script(handoff: Value) -> Value {
    json!({"models": ["fake-small"], "turns": [
        {"when": "flottille:handoff", "do": handoff},
        {"when": "FULL", "times": 1, "do": [{"say": "I edited src/login.rs."},
            {"usage": {"used": 144_000, "size": 200_000}}]},
        {"when": "RUSH", "times": 1, "do": [{"say": "Working on it."},
            {"usage": {"used": 160_000, "size": 200_000}}, {"hold": "release"}]},
        {"when": "your_handoff", "do": [{"say": "Picking up."}, {"usage": {"used": 9_000, "size": 200_000}}]},
        {"when": "summary is missing", "do": [{"say": "Starting again."}, {"usage": {"used": 9_000, "size": 200_000}}]}
    ]})
}

/// A user whose agents are the fake agent playing `handoff_script(handoff)` and whose summaries
/// the agents write themselves.
fn handing_user(handoff: Value) -> User {
    let user = User::with_fake_agent();
    user.fake_script(handoff_script(handoff));
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "summary = \"agent\"\n",
    )
    .unwrap();
    user
}

#[test]
fn the_claude_process_starts_with_its_compaction_off() {
    let user = User::with_fake_claude();
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "summary = \"agent\"\ncompactor = \"none\"\n",
    )
    .unwrap();
    session(&user, "hello");
    eventually("claude's start", || !user.claude_starts().is_empty());
    let env = &user.claude_starts()[0]["env"];
    assert_eq!(env["DISABLE_COMPACT"], "1", "{env}");
    assert!(
        env.get("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE").is_none(),
        "{env}"
    );
    // Flottille's compaction off (automatic off): Claude compacts on its own again.
    user.call("compaction.set", json!({"automatic": false}))
        .unwrap();
    let again = session_in(&user, "hello again");
    eventually("claude's second start", || user.claude_starts().len() == 2);
    let env = &user.claude_starts()[1]["env"];
    assert!(env.get("DISABLE_COMPACT").is_none(), "{again}: {env}");
}

/// A second root agent in the workspace `w` of `session`.
fn session_in(user: &User, message: &str) -> String {
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": message}),
        )
        .unwrap();
    answer["agent"]["id"].as_str().unwrap().to_owned()
}

#[test]
fn an_agent_past_the_threshold_hands_off_at_the_end_of_its_turn() {
    let user = handing_user(json!([{"say": HANDOFF}]));
    let agent = session(&user, "add the login page FULL");

    eventually("the handoff", || !compacted(&user, &agent).is_empty());
    let event = &compacted(&user, &agent)[0];
    assert_eq!(event["by"], "flottille", "{event}");
    assert_eq!(event["source"], "agent", "{event}");
    assert_eq!(event["before"], 144_000, "{event}");
    assert_eq!(event["summary"], HANDOFF, "{event}");
    // Asked at the end of the turn, never in it, in a message of Flottille's.
    let given = prompts(&user);
    assert!(given.len() >= 2, "{given:#?}");
    assert!(
        given[1].starts_with("[flottille:handoff]\n"),
        "{}",
        given[1]
    );
    for section in [
        "Task",
        "Decisions",
        "Files touched",
        "Done",
        "Left",
        "About to do",
    ] {
        assert!(given[1].contains(section), "{section}: {}", given[1]);
    }
    assert_eq!(
        events(&user, &agent, "agent.turn_ended")[0]["stop_reason"],
        "end_turn"
    );
    // No compactor was asked: there is none.
    assert_eq!(
        user.call("compaction.show", json!({})).unwrap()["compactor"],
        Value::Null
    );
    eventually("the new session", || {
        user.received_requests("session/new").len() == 2
    });
}

#[test]
fn a_turn_far_past_the_threshold_is_interrupted_for_the_handoff() {
    let user = handing_user(json!([{"say": HANDOFF}]));
    let agent = session(&user, "add the login page RUSH");

    eventually("the handoff", || !compacted(&user, &agent).is_empty());
    // The turn was cut (its hold never released), then the handoff asked, saying so.
    let ended = events(&user, &agent, "agent.turn_ended");
    assert_eq!(ended[0]["stop_reason"], "cancelled", "{ended:?}");
    let asked = prompts(&user)
        .into_iter()
        .find(|p| p.starts_with("[flottille:handoff]"))
        .unwrap();
    assert!(asked.contains("interrupted"), "{asked}");
    // The new session goes on from what it was about to do.
    eventually("the new session's first prompt", || {
        prompts(&user).iter().any(|p| p.contains("your_handoff"))
    });
    let first = prompts(&user)
        .into_iter()
        .find(|p| p.contains("your_handoff"))
        .unwrap();
    assert!(first.contains("about to do"), "{first}");
}

#[test]
fn the_new_process_gets_the_brief_the_summary_and_the_state_of_the_store() {
    let user = handing_user(json!([{"say": HANDOFF}]));
    let (agent, mcp) = session_with_mcp(&user, "lead the work");
    eventually("the first turn's end", || turns_ended(&user, &agent) == 1);
    let woken: Answer = mcp.call("wake", json!({"minutes": 30, "why": "look at the build"}));
    assert!(!woken.error, "{}", woken.text);

    send(&user, &agent, "go on FULL");
    eventually("the new session's first prompt", || {
        prompts(&user).iter().any(|p| p.contains("your_handoff"))
    });
    let first = prompts(&user)
        .into_iter()
        .find(|p| p.contains("your_handoff"))
        .unwrap();
    let brief = first.find("lead the work").expect("the brief");
    let handoff = first.find(HANDOFF).expect("the handoff");
    let state = first.find("<flottille_state>").expect("the state");
    assert!(brief < handoff && handoff < state, "{first}");
    assert!(
        first.contains("look at the build"),
        "the subscription: {first}"
    );
    assert!(first.contains("Workspace branch"), "the workspace: {first}");
}

#[test]
fn the_agent_keeps_its_id_its_place_in_the_tree_and_its_workspace() {
    let user = handing_user(json!([{"say": HANDOFF}]));
    let (agent, mcp) = session_with_mcp(&user, "lead the work");
    eventually("the first turn's end", || turns_ended(&user, &agent) == 1);
    let launched = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "tests", "brief": "write the tests HANG", "workspace": "w"}),
    );
    assert!(!launched.error, "{}", launched.text);
    let before = user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone();

    send(&user, &agent, "go on FULL");
    eventually("the handoff", || !compacted(&user, &agent).is_empty());

    let after = user.call("agent.show", json!({"agent": agent})).unwrap()["agent"].clone();
    assert_eq!(after["id"], agent.as_str());
    for key in ["workspace", "folder", "parent", "provider", "model"] {
        assert_eq!(after[key], before[key], "{key}");
    }
    let tree = user.call("agent.tree", json!({"agent": agent})).unwrap();
    assert!(tree.to_string().contains("tests"), "{tree}");
    // Not a replacement: neither counted nor told.
    assert!(
        events(&user, &agent, "agent.replaced")
            .iter()
            .all(|e| e["counted"] != true)
    );
}

#[test]
fn a_failed_handoff_starts_from_the_store_and_says_the_summary_is_missing() {
    // Asked for its handoff, the agent answers nothing.
    let user = handing_user(json!([]));
    let agent = session(&user, "start the work");
    eventually("the first turn's end", || turns_ended(&user, &agent) == 1);
    send(&user, &agent, "go on FULL");

    eventually("the new session", || {
        prompts(&user)
            .iter()
            .any(|p| p.contains("summary is missing"))
    });
    let first = prompts(&user)
        .into_iter()
        .find(|p| p.contains("summary is missing"))
        .unwrap();
    assert!(first.contains("<flottille_state>"), "{first}");
    assert!(
        first.contains("go on FULL"),
        "the last user message: {first}"
    );
    let event = &compacted(&user, &agent)[0];
    assert_eq!(event["missing"], true, "{event}");
    assert_eq!(event["summary"], Value::Null, "{event}");
    assert!(!failed(&user, &agent).is_empty());
}

#[test]
fn the_threshold_is_read_from_settings_and_refused_outside_50_to_95() {
    let user = handing_user(json!([{"say": HANDOFF}]));
    let shown = user.call("compaction.show", json!({})).unwrap();
    assert_eq!(shown["summary"], "agent", "{shown}");
    assert_eq!(shown["threshold"], 70, "{shown}");
    let shown = user
        .call(
            "compaction.set",
            json!({"threshold": 80, "summary": "compactor"}),
        )
        .unwrap();
    assert_eq!(shown["threshold"], 80);
    assert_eq!(shown["summary"], "compactor");
    for wrong in [
        json!({"threshold": 49}),
        json!({"threshold": 96}),
        json!({"summary": "someone"}),
    ] {
        let refused = user.call("compaction.set", wrong.clone()).unwrap_err();
        assert_eq!(refused.0, "invalid-params", "{wrong}: {refused:?}");
    }
    // At 80 %, a context at 72 % is no handoff.
    user.call("compaction.set", json!({"summary": "agent"}))
        .unwrap();
    let agent = session(&user, "add the login page FULL");
    eventually("the turn's end", || turns_ended(&user, &agent) == 1);
    std::thread::sleep(Duration::from_millis(1_000));
    assert!(compacted(&user, &agent).is_empty());
    assert_eq!(prompts(&user).len(), 1);
    // The file says it too, and refuses what is out of range.
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "summary = \"agent\"\nthreshold = 99\n",
    )
    .unwrap();
    let refused = user.call("compaction.show", json!({})).unwrap_err();
    assert!(refused.1.contains("threshold"), "{refused:?}");
}

#[test]
fn a_codex_agent_and_an_opencode_agent_hand_off_the_same_way() {
    let user = User::with_fake_providers();
    user.write_providers("[opencode]\nenabled = true\n[codex]\nenabled = true\n");
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/compaction.toml"),
        "summary = \"agent\"\n",
    )
    .unwrap();
    for provider in ["codex", "opencode"] {
        user.provider_script(provider, handoff_script(json!([{"say": HANDOFF}])));
    }
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    for provider in ["codex", "opencode"] {
        let agent = user
            .call(
                "agent.run",
                json!({"project": "app", "message": "add the login page FULL", "provider": provider}),
            )
            .unwrap()["agent"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        eventually("the handoff", || !compacted(&user, &agent).is_empty());
        let event = &compacted(&user, &agent)[0];
        assert_eq!(event["source"], "agent", "{provider}: {event}");
        assert_eq!(event["summary"], HANDOFF, "{provider}: {event}");
        eventually("the new session's first prompt", || {
            user.provider_received(provider).iter().any(|r| {
                r["received"]["method"] == "session/prompt"
                    && r.to_string().contains("your_handoff")
            })
        });
    }
}

/// The agent's entry in its own tree, as `agent.tree` gives it.
fn entry(user: &User, agent: &str) -> Value {
    user.call("agent.tree", json!({"agent": agent})).unwrap()["agents"][0].clone()
}

#[test]
fn a_compaction_says_it_started_and_the_tree_says_it_runs() {
    let user = user_with(json!([{"hold": "release"}, {"say": SUMMARY}]), "");
    let agent = session(&user, "add the login page FULL");
    eventually("the compactor at work", || {
        compactor_prompts(&user).len() == 1
    });

    let started = events(&user, &agent, "agent.compacting");
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0]["source"], "compactor", "{started:?}");
    assert_eq!(started[0]["before"], 150_000, "{started:?}");
    assert!(compacted(&user, &agent).is_empty(), "it is not over yet");
    let running = entry(&user, &agent);
    assert_eq!(running["agent"]["compacting"], true, "{running}");
    assert!(
        running["doing"]
            .as_str()
            .unwrap()
            .starts_with("compacting the conversation"),
        "{running}"
    );

    std::fs::write(user.path("fake-compactor/release"), "").unwrap();
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    assert_eq!(events(&user, &agent, "agent.compacting").len(), 1);
    eventually("the tree says it is over", || {
        entry(&user, &agent)["agent"]["compacting"] == false
    });
}

#[test]
fn a_failed_compaction_said_it_started_first_and_the_tree_stops_saying_it_runs() {
    let user = user_with(json!([{"die": 3}]), "");
    let agent = session(&user, "add the login page FULL");
    eventually("the failure", || !failed(&user, &agent).is_empty());

    assert_eq!(events(&user, &agent, "agent.compacting").len(), 1);
    let seqs = |name: &str| -> u64 {
        user.call("agent.show", json!({"agent": agent})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .map(|e| e["seq"].as_u64().unwrap())
            .unwrap()
    };
    assert!(seqs("agent.compacting") < seqs("agent.compaction_failed"));
    assert_eq!(entry(&user, &agent)["agent"]["compacting"], false);
}

#[test]
fn an_agent_writing_its_handoff_is_compacting_too() {
    let user = handing_user(json!([{"say": HANDOFF}]));
    let agent = session(&user, "add the login page FULL");
    eventually("the handoff", || !compacted(&user, &agent).is_empty());

    let started = events(&user, &agent, "agent.compacting");
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0]["source"], "agent", "{started:?}");
}

// What Flottille sends the new session after a compaction (#613) is never a message of the user's.

/// Waits until a prompt of the agent held `text`, and every prompt it was given has had its turn.
fn settled(user: &User, agent: &str, text: &str) {
    eventually(text, || {
        let prompted = events(user, agent, "agent.prompted");
        prompted
            .iter()
            .any(|e| e["text"].as_str().unwrap().contains(text))
            && turns_ended(user, agent) == prompted.len()
            && user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"] == "idle"
    });
}

/// What the agent's prompts say the user and Flottille sent, as a conversation shows it: the
/// sections of every `agent.prompted` event but those that replay the conversation after a
/// compaction, as `(kind, text)`.
fn sent(user: &User, agent: &str) -> Vec<(String, String)> {
    let shown = user.call("agent.show", json!({"agent": agent})).unwrap();
    shown["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == "agent.prompted")
        .flat_map(|e| {
            let text = flottille_core::prompt::without_replay(&e["data"]);
            flottille_core::prompt::sections(&text)
        })
        .collect()
}

/// The texts of what the user wrote, in order: their first message and those after.
fn written(user: &User, agent: &str) -> Vec<String> {
    sent(user, agent)
        .into_iter()
        .filter(|(kind, _)| matches!(kind.as_str(), "" | "user" | "brief"))
        .map(|(_, text)| text)
        .collect()
}

#[test]
fn a_compaction_by_the_compactor_replays_nothing_as_a_message_of_the_user() {
    // The compactor takes its time, so that the user writes while it works, and again after.
    let user = user_with(json!([{"hold": "release"}, {"say": SUMMARY}]), "");
    let agent = session(&user, "add the login page FULL");
    eventually("the compactor at work", || {
        compactor_prompts(&user).len() == 1
    });
    send(&user, &agent, "and mind the footer");
    std::fs::write(user.path("fake-compactor/release"), "").unwrap();
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    settled(&user, &agent, "and mind the footer");
    send(&user, &agent, "one more thing");
    settled(&user, &agent, "one more thing");

    assert_eq!(
        written(&user, &agent),
        [
            "add the login page FULL",
            "and mind the footer",
            "one more thing"
        ],
        "{:?}",
        sent(&user, &agent)
    );
    // The new session did receive the brief and the summary: only the conversation leaves them out.
    assert!(prompts(&user).iter().any(|p| p.contains(SUMMARY)));
    let all = sent(&user, &agent);
    assert!(
        all.iter().all(|(_, text)| !text.contains(SUMMARY)),
        "{all:?}"
    );
}

#[test]
fn a_compaction_by_the_agent_replays_nothing_as_a_message_of_the_user() {
    let user = handing_user(json!([{"say": HANDOFF}]));
    let agent = session(&user, "add the login page FULL");
    eventually("the handoff", || !compacted(&user, &agent).is_empty());
    send(&user, &agent, "go on");
    settled(&user, &agent, "go on");

    assert_eq!(
        written(&user, &agent),
        ["add the login page FULL", "go on"],
        "{:?}",
        sent(&user, &agent)
    );
    // Its own handoff is Flottille's word to it, asked for in the old session, and nothing else.
    let kinds: Vec<String> = sent(&user, &agent).into_iter().map(|(k, _)| k).collect();
    assert!(kinds.iter().all(|k| k != "resume"), "{kinds:?}");
}

#[test]
fn the_text_of_an_agent_read_by_the_cli_has_the_brief_once_after_a_compaction() {
    let user = user();
    let agent = session(&user, "add the login page FULL");
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    send(&user, &agent, "go on");
    settled(&user, &agent, "go on");

    let output = user.run(&["agent", "show", &agent]);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(text.matches("add the login page").count(), 1, "{text}");
    assert_eq!(text.matches("go on").count(), 1, "{text}");
}

#[test]
fn a_session_read_by_another_agent_has_the_brief_once_after_a_compaction() {
    let user = user();
    let (reader, mcp) = session_with_mcp(&user, "lead the work");
    eventually("the reader's turn", || turns_ended(&user, &reader) == 1);
    user.call("agent.mode", json!({"agent": reader, "mode": "full"}))
        .unwrap();
    let agent = session_in(&user, "add the login page FULL");
    eventually("the compaction", || !compacted(&user, &agent).is_empty());
    send(&user, &agent, "go on");
    settled(&user, &agent, "go on");

    let page = mcp.call("session_read", json!({"session": agent}));
    assert!(!page.error, "{}", page.text);
    assert_eq!(
        page.text.matches("add the login page").count(),
        1,
        "{}",
        page.text
    );
    assert_eq!(page.text.matches("go on").count(), 1, "{}", page.text);
}

#[test]
fn a_compaction_turned_on_again_picks_a_compactor_as_for_a_new_user() {
    let user = User::with_fake_claude();
    user.call("compaction.set", json!({"compactor": "none"}))
        .unwrap();

    let shown = user
        .call("compaction.set", json!({"compactor": "pick"}))
        .unwrap();

    assert_eq!(
        shown["compactor"],
        json!({"provider": "claude", "model": "haiku"}),
        "{shown}"
    );
}
