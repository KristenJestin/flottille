//! The state a machine's light list gives each of its sessions (`docs/remote.md`, "One list of
//! every machine"): `needs you` on the rule the session's own window follows (`agent.trees`'
//! `needs_user`): a question to the user, a request of its tree held for the user, an agent's ask
//! for a vault entry; never a question to its pilot, a request its parent or the judge holds, nor
//! what waits in another tree. A hub served in the test on 127.0.0.1 and two daemons, each with
//! its own temporary HOME and XDG folders and fake agents; a machine "off" is its daemon stopped.

use crate::common;
use crate::remote_deliveries::{answered, off, script};
use crate::remote_launch::{
    Machine, allowing, done, eventually, id_in, launched, pc_and_server, pc_and_server_with,
    runner_user, server_user, sign_in,
};

use common::mcp::server_of;
use serde_json::json;

/// The state `machine` lists its session `agent` in, for the other machines.
fn listed(machine: &Machine, agent: &str) -> Option<String> {
    let lists = machine.user.call("remote.sessions", json!({})).ok()?;
    lists["this"]
        .as_array()?
        .iter()
        .find(|session| session["id"] == agent)
        .and_then(|session| session["state"].as_str())
        .map(str::to_owned)
}

#[test]
fn a_question_to_the_pilot_does_not_list_as_needs_you() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"mcp": {"tool": "ask", "arguments": {"question": "CSV or JSON first?"}}}
        ]}]),
    );
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export."}),
    );
    let session = id_in(done(&answer));
    answered(&server, "Your question went to agent");
    eventually("the session, its turn over, waiting on its pilot", || {
        server.agent(&session)["state"] == "idle"
            && server
                .user
                .call("agent.tree", json!({"agent": session}))
                .unwrap()["agents"][0]["waiting_on"]
                == "pilot"
    });

    // Its pilot answers it, not the user: its window shows no mark, its list says so too.
    assert_eq!(listed(&server, &session).as_deref(), Some("idle"));

    // Its pilot's machine off, the question goes to the user: now it needs them.
    off(&pc);
    eventually("server lists it as needing the user", || {
        listed(&server, &session).as_deref() == Some("needs you")
    });
}

#[test]
fn a_request_held_by_a_parent_does_not_list_as_needs_you() {
    let (_hub, pc, _server) = pc_and_server(server_user(allowing()), Some(""));
    let (root, mcp) = pc.root("auto");
    let opened = pc.user.received_requests("session/new").len();
    done(&mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "reader", "brief": "Read the settings."}),
    ));
    let (url, authorization) = server_of(&pc.user, opened);
    let child = common::mcp::Mcp::initialize(&url, &authorization);

    let held = child.call("fs_read", json!({"path": ".env"}));
    assert!(held.text.contains("your parent decides"), "{}", held.text);

    // Its parent decides, not the user.
    assert!(pc.requests().is_empty(), "{:#?}", pc.requests());
    let state = listed(&pc, &root);
    assert!(state.is_some(), "the root is listed");
    assert_ne!(state.as_deref(), Some("needs you"));
}

#[test]
fn a_request_the_judge_holds_does_not_list_as_needs_you() {
    let pc_user = runner_user().with_judge(json!([
        {"hold": "judged"},
        {"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}
    ]));
    sign_in(&pc_user);
    let (_hub, pc, _server) =
        pc_and_server_with("runner", pc_user, server_user(allowing()), Some(""));
    let (root, mcp) = pc.root("auto");

    let call = std::thread::spawn(move || mcp.call("run", json!({"line": "touch made.txt"})));
    eventually("the judge judging the call", || {
        !pc.user.judge_received().is_empty()
    });

    // The judge decides, not the user.
    let state = listed(&pc, &root);
    assert!(state.is_some(), "the root is listed");
    assert_ne!(state.as_deref(), Some("needs you"));
    std::fs::write(pc.user.path("fake-judge/judged"), "").unwrap();
    assert!(!call.join().unwrap().error);
}

#[test]
fn a_request_in_another_tree_does_not_list_as_needs_you() {
    let (_hub, pc, _server) = pc_and_server(server_user(allowing()), Some(""));
    let (asking, mcp) = pc.root("ask");
    let (other, _other_mcp) = pc.root("ask");

    let held = mcp.call("run", json!({"line": "touch made.txt"}));
    assert!(!held.error, "{}", held.text);
    eventually("the request held for the user", || {
        !pc.requests().is_empty()
    });

    assert_eq!(listed(&pc, &asking).as_deref(), Some("needs you"));
    assert_eq!(listed(&pc, &other).as_deref(), Some("idle"));
}

#[test]
fn an_ask_for_a_vault_entry_lists_as_needs_you() {
    let (_hub, pc, _server) = pc_and_server(server_user(allowing()), Some(""));
    let (root, mcp) = pc.root("auto");
    assert_eq!(listed(&pc, &root).as_deref(), Some("idle"));

    let asked = mcp.call(
        "vault",
        json!({"action": "ask", "name": "staging-login", "why": "to sign in to staging",
            "url": "https://staging.example.com/login"}),
    );
    assert!(!asked.error, "{}", asked.text);

    // It waits for the user as a held request does, as its own window shows it.
    assert_eq!(listed(&pc, &root).as_deref(), Some("needs you"));
}
