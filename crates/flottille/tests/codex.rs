//! Codex agents end to end: the daemon starts Flottille's Codex bridge, which drives the fake
//! `codex` (`crates/flottille-fake-codex`). No real agent ever runs here.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A user with Codex alone enabled, on the fake `codex`, and a project `app` over a throwaway
/// repository with one commit.
fn user_with_codex() -> (User, PathBuf) {
    let user = User::with_fake_codex();
    user.write_providers("[claude]\nenabled = false\n[codex]\nenabled = true\n");
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    (user, folder)
}

fn run(user: &User, message: &str) -> (String, u64) {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        answer["after"].as_u64().unwrap(),
    )
}

fn events_until(user: &User, agent: &str, after: u64, name: &str) -> Vec<Value> {
    let of = |event: &Value| event["data"]["agent"] == agent;
    let watch = user.watch(&["--after", &after.to_string()]);
    watch
        .until(|e| of(e) && e["name"] == name)
        .into_iter()
        .filter(of)
        .collect()
}

fn agent(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
}

/// The messages the fake `codex` read, of one method.
fn sent(user: &User, method: &str) -> Vec<Value> {
    user.codex_received()
        .into_iter()
        .filter_map(|e| e.get("received").cloned())
        .filter(|m| m["method"] == method)
        .collect()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_daemon_runs_a_codex_agent_through_the_bridge_with_its_bare_session() {
    let (user, folder) = user_with_codex();
    user.codex_script(json!({"turns": [{"when": "hello", "do": [{"say": "Hello!"}]}]}));

    let (id, after) = run(&user, "hello");

    let events = events_until(&user, &id, after, "agent.turn_ended");
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "agent.message" && e["data"]["text"] == "Hello!"),
        "{events:?}"
    );
    assert_eq!(events.last().unwrap()["data"]["stop_reason"], "end_turn");
    let agent = agent(&user, &id);
    assert_eq!(agent["provider"], "codex");
    assert_eq!(agent["taken"]["model"], "gpt-fake");
    // What the daemon sent was taken: its instructions, its server with its header, its tools.
    let thread = &sent(&user, "thread/start")[0]["params"];
    assert_eq!(thread["cwd"], folder.to_str().unwrap());
    assert!(
        thread["baseInstructions"].as_str().unwrap().len() > 100,
        "{thread}"
    );
    let server = &thread["config"]["mcp_servers"]["flottille"];
    let url = server["url"].as_str().unwrap();
    assert!(
        url.starts_with("http://127.0.0.1:") && url.ends_with("/mcp"),
        "{url}"
    );
    let headers = server["http_headers"].as_object().unwrap();
    assert_eq!(headers.len(), 1, "{server}");
    assert!(
        headers["Authorization"]
            .as_str()
            .unwrap()
            .starts_with("Bearer "),
        "{server}"
    );
    let tools: Vec<&str> = server["enabled_tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    for tool in ["fs_read", "commands_run", "report", "ask_user"] {
        assert!(tools.contains(&tool), "{tools:?}");
    }
    assert_eq!(thread["environments"], json!([]));
    assert_eq!(
        agent["native_id"],
        sent(&user, "turn/start")[0]["params"]["threadId"]
    );
}

#[test]
fn a_codex_session_s_thread_is_deleted_when_flottille_forgets_it() {
    let (user, _) = user_with_codex();
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    let native = agent(&user, &id)["native_id"].as_str().unwrap().to_owned();
    let thread = user.path(&format!("codex/threads/{native}"));
    assert!(thread.exists());

    user.call("agent.stop", json!({"agent": id})).unwrap();

    eventually("the thread deleted", || !thread.exists());
    let deleted = sent(&user, "thread/delete");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0]["params"], json!({"threadId": native}));
}

#[test]
fn a_reply_streamed_in_many_deltas_reads_as_one_message() {
    let (user, _) = user_with_codex();
    let pieces = [
        "Available",
        " tool",
        " names",
        ":",
        " `",
        "fs",
        "_list",
        "`.",
    ];
    user.codex_script(json!({"turns": [{"when": "tools", "do": [{"say": pieces}]}]}));

    let (id, after) = run(&user, "tools");

    let events = events_until(&user, &id, after, "agent.turn_ended");
    // The stream, for `watch` and the window, is the reply as it settles.
    let streamed: String = events
        .iter()
        .filter(|e| e["name"] == "agent.message")
        .map(|e| e["data"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(streamed, pieces.concat(), "{events:?}");
    let read = user.call("agent.read", json!({"agent": id})).unwrap();
    let said: Vec<&str> = read["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.as_str())
        .filter(|l| l.contains(" said: "))
        .collect();
    assert_eq!(said.len(), 1, "{read}");
    assert!(
        said[0].ends_with(" said: Available tool names: `fs_list`."),
        "{said:?}"
    );
}
