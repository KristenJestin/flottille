//! An update replaces Flottille's file while the daemon runs (#1577): what the daemon starts from
//! its own program still starts, from the running image.

use crate::common;

use common::User;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository with one commit.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// A session of `app` prompted with `hello`: the text of its reply once its turn ended.
fn replied(user: &User) -> Vec<Value> {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let agent = answer["agent"]["id"].as_str().unwrap().to_owned();
    let after = answer["after"].as_u64().unwrap();
    user.watch(&["--after", &after.to_string()])
        .until(|e| {
            e["data"]["agent"] == agent.as_str()
                && matches!(
                    e["name"].as_str(),
                    Some("agent.turn_ended" | "agent.ended" | "agent.failed")
                )
        })
        .into_iter()
        .filter(|e| e["data"]["agent"] == agent.as_str())
        .collect()
}

fn said_hello(events: &[Value]) -> bool {
    events
        .iter()
        .any(|e| e["name"] == "agent.message" && e["data"]["text"] == "Hello!")
}

#[test]
fn a_session_starts_after_the_binary_was_replaced() {
    let claude = User::with_fake_claude();
    claude.claude_script(json!({"turns": [{"when": "hello", "do": [{"say": "Hello!"}]}]}));
    let _installed = claude.daemon_from_replaced_binary();
    project(&claude);
    let events = replied(&claude);
    assert!(said_hello(&events), "{events:?}");

    let codex = User::with_fake_codex();
    codex.write_providers("[claude]\nenabled = false\n[codex]\nenabled = true\n");
    codex.codex_script(json!({"turns": [{"when": "hello", "do": [{"say": "Hello!"}]}]}));
    let _installed = codex.daemon_from_replaced_binary();
    project(&codex);
    let events = replied(&codex);
    assert!(said_hello(&events), "{events:?}");
}
