//! A child an agent of another machine launched is a child on the machine it runs on
//! (`docs/remote.md`, "A tree across machines"): a child's tools, its parent's depth plus one, and
//! shown as a child of that agent, never as a session of the user's. A hub served in the test on
//! 127.0.0.1 and two daemons, each with its own temporary HOME and XDG folders and fake agents;
//! throwaway repositories.

use crate::common;
use crate::remote_deliveries::{child_on_server, off, on, prompts_with, script};
use crate::remote_launch::{
    allowing, eventually, launch_ask, pc_and_server, sent_with, server_user,
};

use common::mcp::{Mcp, server_of};
use flottille_link::MachineMessage;
use serde_json::{Value, json};

/// The tools only a root has: the project's actions and the sessions it writes to.
const ROOT_TOOLS: [&str; 7] = [
    "skills",
    "commands",
    "setup",
    "project",
    "session_send",
    "session_read",
    "session_title",
];

/// The MCP session of the child the server opened first: the remote child's own.
fn child_s_mcp(server: &crate::remote_launch::Machine) -> Mcp {
    let (url, authorization) = server_of(&server.user, 0);
    Mcp::initialize(&url, &authorization)
}

#[test]
fn a_child_launched_from_another_machine_has_a_child_s_tools() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    let caller = pc.agents()[0]["id"].as_str().unwrap().to_owned();

    let names = child_s_mcp(&server).names();

    for tool in ROOT_TOOLS {
        assert!(
            !names.iter().any(|name| name == tool),
            "{tool} in {names:?}"
        );
    }
    // At its parent's depth plus one, the limit: it launches nothing.
    assert!(
        !names.iter().any(|name| name.starts_with("agent_")),
        "{names:?}"
    );
    assert!(names.iter().any(|name| name == "report"), "{names:?}");
    let shown = server.agent(&child);
    assert_eq!(shown["depth"], 1, "{shown}");
    assert_eq!(
        shown["launched_from"],
        json!({"machine": "pc", "agent": caller, "kind": "child"})
    );
    // The list says it too, as the window, the CLI and the phone read it.
    let listed = server
        .agents()
        .into_iter()
        .find(|agent| agent["id"] == child.as_str())
        .unwrap();
    assert_eq!(listed["launched_from"], shown["launched_from"]);
    assert_eq!(listed["depth"], 1);
    // Its tools are the same when they are made again for it.
    server
        .user
        .call("agent.reload_tools", json!({"agent": child}))
        .unwrap();
    let again = server.agent(&child);
    assert_eq!(again["tools"], shown["tools"], "{again}");
}

#[test]
fn a_child_launched_from_another_machine_cannot_start_a_session() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    child_on_server(&pc, &server, "Write the tests of the export.");
    let mcp = child_s_mcp(&server);

    for kind in ["led", "handed"] {
        let answer = mcp.call(
            "agent_launch",
            json!({"kind": kind, "title": "Another", "brief": "Do more."}),
        );
        assert!(
            answer.error || answer.text.starts_with("refused"),
            "{}",
            answer.text
        );
    }
    // Nothing started: the child alone runs on the server.
    let agents = server.agents();
    assert_eq!(agents.len(), 1, "{agents:?}");
    assert_eq!(agents[0]["title"], "Write the tests");
}

#[test]
fn the_depth_limit_holds_across_machines() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    // Its parent is at depth 0: it is at 1, the limit.
    assert_eq!(server.agent(&child)["depth"], 1);
    let caller = pc.agents()[0]["id"].as_str().unwrap().to_owned();
    let key = pc.stand_in();

    // A child asked for by an agent already at the limit on its own machine.
    let mut deep = launch_ask("deep", &caller);
    deep.kind = "child".into();
    deep.depth = 1;
    let answers = sent_with(&key, &server, vec![MachineMessage::Launch(Box::new(deep))]);

    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("depth limit"), "{refused}");
    let titles: Vec<Value> = server
        .agents()
        .iter()
        .map(|agent| agent["title"].clone())
        .collect();
    assert_eq!(titles, vec![json!("Write the tests")]);
    // A caller that sends no depth (a machine of before) is at depth 0, as only a root launched
    // there: the field is left out at 0, and its child is at 1.
    let sent = serde_json::to_value(launch_ask("plain", &caller)).unwrap();
    assert!(sent.get("depth").is_none(), "{sent}");
}

/// Whether a list of tools, as `agent.show` gives it, holds one only a root has.
fn has_a_root_s_tool(tools: &Value) -> bool {
    tools.as_array().unwrap().iter().any(|tool| {
        let tool = tool.as_str().unwrap();
        ROOT_TOOLS
            .iter()
            .any(|root| tool.rsplit("__").next() == Some(root))
    })
}

#[test]
fn a_child_stored_at_depth_zero_before_the_upgrade_is_a_child_after_a_restart_and_a_resume() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    assert!(!has_a_root_s_tool(&server.agent(&child)["tools"]));
    // As a Flottille of before stored it: no parent, at depth 0, a root's tools.
    off(&server);
    let db = rusqlite::Connection::open(server.user.path("state/flottille/flottille.db")).unwrap();
    db.execute(
        "UPDATE agents SET depth = 0, tools = json_insert(tools, '$[#]', \
         'mcp__flottille__session_send') WHERE id = ?1",
        [&child],
    )
    .unwrap();
    drop(db);

    on(&server);

    let shown = server.agent(&child);
    assert_eq!(shown["depth"], 1, "{shown}");
    // Made again at its next start, from its depth: a child's.
    server
        .user
        .call("agent.stop", json!({"agent": child}))
        .unwrap();
    eventually("the child ended", || {
        server.agent(&child)["state"] == "ended"
    });
    server
        .user
        .call("agent.resume", json!({"agent": child}))
        .unwrap();
    eventually("the child back", || server.agent(&child)["state"] == "idle");
    let resumed = server.agent(&child);
    assert_eq!(resumed["depth"], 1, "{resumed}");
    assert!(!has_a_root_s_tool(&resumed["tools"]), "{resumed}");
}

#[test]
fn a_caller_below_the_root_cannot_lead_or_hand_a_session_across_machines() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (caller, _mcp) = pc.root("auto");
    let key = pc.stand_in();

    let mut answers = Vec::new();
    for kind in ["led", "handed"] {
        // Asked for by a child of that root: one depth below it.
        let mut ask = launch_ask(&format!("deep-{kind}"), &caller);
        ask.kind = kind.into();
        ask.depth = 1;
        answers.extend(sent_with(
            &key,
            &server,
            vec![MachineMessage::Launch(Box::new(ask))],
        ));
    }

    for answer in answers {
        let refused = answer.unwrap_err();
        assert!(refused.contains("only a root"), "{refused}");
    }
    assert!(server.agents().is_empty(), "{:?}", server.agents());
}

#[test]
fn the_user_writes_to_a_child_of_another_machine_s_agent_only_as_to_a_child() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");

    let sent = server
        .user
        .call(
            "agent.send",
            json!({"agent": child, "message": "Do it my way."}),
        )
        .unwrap_err()
        .1;
    assert!(sent.contains("only its parent writes to it"), "{sent}");
    // The ways out stay while its parent's machine is off: an answer to its question, its stop.
    let answered = server
        .user
        .call(
            "agent.answer",
            json!({"agent": child, "question": 1, "answers": []}),
        )
        .unwrap_err()
        .1;
    assert!(!answered.contains("is a child of"), "{answered}");
    server
        .user
        .call("agent.stop", json!({"agent": child}))
        .unwrap();
    eventually("the child ended", || {
        server.agent(&child)["state"] == "ended"
    });
}

#[test]
fn a_child_of_another_machine_s_agent_gets_its_brief_again_after_a_compaction() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1,
            "do": [{"compaction": true}]}]),
    );
    child_on_server(&pc, &server, "Write the tests of the export.");

    eventually("the brief again", || {
        prompts_with(&server, "Your brief, again") == 1
    });
}
