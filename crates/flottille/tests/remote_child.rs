//! A child an agent of another machine launched is a child on the machine it runs on
//! (`docs/remote.md`, "A tree across machines"): a child's tools, its parent's depth plus one, and
//! shown as a child of that agent, never as a session of the user's. A hub served in the test on
//! 127.0.0.1 and two daemons, each with its own temporary HOME and XDG folders and fake agents;
//! throwaway repositories.

use crate::common;
use crate::remote_deliveries::{answered, child_on_server, off, on, prompts_with, release, script};
use crate::remote_launch::{
    Machine, allowing, eventually, launch_ask, pc_and_server, prompted, sent_with, server_user,
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
    let (child, _launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
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
    // Ended, then resumed by the user: its tools are made again at its next start, which its
    // parent's message wakes, from its depth: a child's.
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
    server.wait_for("pc");
    let sent = mcp.call(
        "agent_send",
        json!({"agent_id": child, "message": "Cover the CSV export too."}),
    );
    assert!(!sent.error, "{}", sent.text);
    prompted(&server, "Cover the CSV export too.");
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

#[test]
fn a_child_on_another_machine_stops_with_its_parent() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
    let root = pc.agents()[0]["id"].as_str().unwrap().to_owned();

    pc.user.call("agent.stop", json!({"agent": root})).unwrap();

    eventually("the child ended on server", || {
        server.agent(&child)["state"] == "ended"
    });
    assert_eq!(
        server.agent(&child)["state_reason"],
        "its parent stopped",
        "{:#}",
        server.agent(&child)
    );
}

/// A child of `pc`'s root held in its first turn on `server`: its id.
fn working_child(pc: &Machine, server: &Machine) -> String {
    script(
        server,
        json!([{"when": "Write the tests of the export.", "times": 1,
            "do": [{"hold": "release"}]}]),
    );
    let (child, _launch, _mcp) = child_on_server(pc, server, "Write the tests of the export.");
    prompted(server, "Write the tests of the export.");
    child
}

/// What the user of `server` is told when `method` is called on a child of another machine's
/// agent: the same refusal a child here is given, which says whose it is and what only it does.
fn refused_as_a_child(server: &Machine, child: &str, method: &str, params: Value) -> String {
    let refused = server.user.call(method, params).unwrap_err().1;
    assert!(
        refused.contains(&format!("agent {child} is a child of agent")),
        "{refused}"
    );
    assert!(refused.contains("of pc"), "{refused}");
    assert!(refused.contains("only its parent"), "{refused}");
    refused
}

/// A message `pc`'s root sends its child on `server` while the child's first turn is held: the
/// child's id, and the id of the delivery it waits as on `server`.
fn queued_by_the_parent(pc: &Machine, server: &Machine) -> (String, String) {
    script(
        server,
        json!([{"when": "Write the tests of the export.", "times": 1,
            "do": [{"hold": "release"}]}]),
    );
    let (child, _launch, mcp) = child_on_server(pc, server, "Write the tests of the export.");
    prompted(server, "Write the tests of the export.");
    let sent = mcp.call(
        "agent_send",
        json!({"agent_id": child, "message": "Cover the CSV export too."}),
    );
    assert!(!sent.error, "{}", sent.text);
    eventually("the parent's message queued", || {
        queued(server, &child).is_some()
    });
    let delivery = queued(server, &child).unwrap();
    (child, delivery)
}

/// The delivery `agent` waits on `server`, if any.
fn queued(server: &Machine, agent: &str) -> Option<String> {
    let db = rusqlite::Connection::open_with_flags(
        server.user.path("state/flottille/flottille.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    db.query_row(
        "SELECT id FROM deliveries WHERE agent = ?1 AND state = 'queued'",
        [agent],
        |row| row.get(0),
    )
    .ok()
}

#[test]
fn agent_unqueue_keeps_the_message_the_parent_of_a_child_of_another_machine_queued() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, delivery) = queued_by_the_parent(&pc, &server);

    refused_as_a_child(
        &server,
        &child,
        "agent.unqueue",
        json!({"agent": child, "delivery": delivery}),
    );

    assert_eq!(queued(&server, &child), Some(delivery));
    release(&server);
    prompted(&server, "Cover the CSV export too.");
}

#[test]
fn agent_send_now_leaves_the_message_the_parent_of_a_child_of_another_machine_queued() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, delivery) = queued_by_the_parent(&pc, &server);

    refused_as_a_child(
        &server,
        &child,
        "agent.send_now",
        json!({"agent": child, "delivery": delivery}),
    );

    // Its turn goes on, the message waits it out.
    assert_eq!(server.agent(&child)["state"], "working");
    assert_eq!(queued(&server, &child), Some(delivery));
}

#[test]
fn agent_mode_does_not_set_the_mode_of_a_child_of_another_machine_apart_from_its_tree() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    let before = server.agent(&child)["gate_mode"].clone();

    let refused = server
        .user
        .call("agent.mode", json!({"agent": child, "mode": "full"}))
        .unwrap_err()
        .1;

    assert!(
        refused.contains(&format!("agent {child} is a child of agent")),
        "{refused}"
    );
    assert!(refused.contains("of pc"), "{refused}");
    assert!(refused.contains("mode"), "{refused}");
    assert_eq!(server.agent(&child)["gate_mode"], before);
}

#[test]
fn agent_cancel_does_not_cancel_the_turn_of_a_child_of_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let child = working_child(&pc, &server);

    refused_as_a_child(&server, &child, "agent.cancel", json!({"agent": child}));

    assert_eq!(server.agent(&child)["state"], "working");
}

#[test]
fn agent_rename_does_not_rename_a_child_of_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");

    refused_as_a_child(
        &server,
        &child,
        "agent.rename",
        json!({"agent": child, "title": "Mine now"}),
    );

    assert_eq!(server.agent(&child)["title"], "Write the tests");
}

#[test]
fn agent_quota_resume_is_refused_on_a_child_of_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");

    refused_as_a_child(
        &server,
        &child,
        "agent.quota_resume",
        json!({"agent": child, "resume": true}),
    );
}

#[test]
fn the_inactivity_sweep_leaves_a_child_of_another_machine_to_its_parent() {
    const DAY: u64 = 24 * 3_600_000;
    let (_hub, pc, server) = pc_and_server(
        server_user(allowing()).with_env("FLOTTILLE_TEST_CLOCK", "1"),
        Some(""),
    );
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    eventually("the child idle", || server.agent(&child)["state"] == "idle");
    // A session of the server's own, the one the sweep ends: the proof that it ran.
    let (local, _mcp) = server.root("auto");
    server
        .user
        .call("sessions.set", json!({"days": 1}))
        .unwrap();

    server
        .user
        .call("test.advance", json!({"ms": 2 * DAY}))
        .unwrap();

    eventually("the server's own session ended", || {
        server.agent(&local)["state"] == "ended"
    });
    assert_eq!(server.agent(&child)["state"], "idle");
}

#[test]
fn a_child_of_another_machine_s_agent_gives_the_user_no_review() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");

    server
        .user
        .call("extension.set", json!({"name": "review", "enabled": true}))
        .unwrap();

    let refused = server
        .user
        .call("review.start", json!({"agent": child}))
        .unwrap_err()
        .1;

    assert!(refused.contains("this agent is a child"), "{refused}");
}

#[test]
fn a_report_of_a_child_of_another_machine_is_no_notification_of_the_user() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "The export tests are written."}}}
        ]}]),
    );
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    answered(&server, "Your report went to your parent");

    let notified = server
        .user
        .call("notifications.list", json!({}))
        .unwrap()
        .to_string();
    let inbox = server
        .user
        .call("inbox.list", json!({}))
        .unwrap()
        .to_string();

    assert!(!notified.contains(&child), "{notified}");
    assert!(!inbox.contains(&child), "{inbox}");
}

#[test]
fn a_remote_child_stored_by_an_older_release_is_put_above_its_local_children() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    // A child of the server's own session, which an older release had started under the remote
    // child (it allowed it): its parent is the remote child, one depth below a root.
    let (local, mcp) = server.root("auto");
    let launched = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "Check the export", "brief": "Check the CSV export."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let grandchild = server
        .agents()
        .into_iter()
        .find(|agent| agent["parent"] == local.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    off(&server);
    let db = rusqlite::Connection::open(server.user.path("state/flottille/flottille.db")).unwrap();
    db.execute("UPDATE agents SET depth = 0 WHERE id = ?1", [&child])
        .unwrap();
    db.execute(
        "UPDATE agents SET parent = ?1, depth = 1 WHERE id = ?2",
        [&child, &grandchild],
    )
    .unwrap();
    drop(db);

    on(&server);

    let depths = (
        server.agent(&child)["depth"].clone(),
        server.agent(&grandchild)["depth"].clone(),
        server.agent(&local)["depth"].clone(),
    );
    // The server's own session, a root, stays one.
    assert_eq!(depths, (json!(1), json!(2), json!(0)), "{depths:?}");
}

/// A child of `pc`'s root on `server`, and a child of the server's own that sits under it, as an
/// older release, or a deeper depth limit, let the remote child start: their ids.
fn child_with_a_local_child(pc: &Machine, server: &Machine) -> (String, String) {
    let (child, _launch, _mcp) = child_on_server(pc, server, "Write the tests of the export.");
    let (local, mcp) = server.root("auto");
    let launched = mcp.call(
        "agent_launch",
        json!({"kind": "child", "title": "Check the export", "brief": "Check the CSV export."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let grandchild = server
        .agents()
        .into_iter()
        .find(|agent| agent["parent"] == local.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    off(server);
    let db = rusqlite::Connection::open(server.user.path("state/flottille/flottille.db")).unwrap();
    db.execute(
        "UPDATE agents SET parent = ?1, depth = 2 WHERE id = ?2",
        [&child, &grandchild],
    )
    .unwrap();
    drop(db);
    on(server);
    (child, grandchild)
}

#[test]
fn agent_mode_does_not_set_the_mode_of_a_local_child_of_a_child_of_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, grandchild) = child_with_a_local_child(&pc, &server);
    let before = server.agent(&child)["gate_mode"].clone();

    let refused = server
        .user
        .call("agent.mode", json!({"agent": grandchild, "mode": "full"}))
        .unwrap_err()
        .1;

    assert!(refused.contains("of pc"), "{refused}");
    assert!(refused.contains("mode"), "{refused}");
    assert_eq!(server.agent(&child)["gate_mode"], before);
}

#[test]
fn a_local_child_of_a_child_of_another_machine_is_refused_naming_where_the_user_writes() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, grandchild) = child_with_a_local_child(&pc, &server);

    let refused = server
        .user
        .call(
            "agent.send",
            json!({"agent": grandchild, "message": "Check the PDF export too."}),
        )
        .unwrap_err()
        .1;

    assert!(
        refused.contains(&format!("agent {grandchild} is a child of agent {child}")),
        "{refused}"
    );
    // Not the remote child, which the user does not write to either: its root, on pc.
    assert!(
        !refused.contains(&format!("its root, agent {child}")),
        "{refused}"
    );
    assert!(refused.contains("on pc"), "{refused}");
}

#[test]
fn agent_reload_tools_is_refused_on_a_child_of_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");

    refused_as_a_child(
        &server,
        &child,
        "agent.reload_tools",
        json!({"agent": child}),
    );
}

#[test]
fn agent_resume_is_refused_on_an_ended_child_of_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    // The user's way out, while its parent's machine is off.
    server
        .user
        .call("agent.stop", json!({"agent": child}))
        .unwrap();
    eventually("the child ended", || {
        server.agent(&child)["state"] == "ended"
    });

    refused_as_a_child(&server, &child, "agent.resume", json!({"agent": child}));

    assert_eq!(server.agent(&child)["state"], "ended");
}
