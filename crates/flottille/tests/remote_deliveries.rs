//! Deliveries and the tree's calls across machines (`docs/remote.md`, "Deliveries across
//! machines", "Coming back"): what a session on one machine says to the agent that started it on
//! another, kept by the sender until the other machine takes it, and what that agent asks of it.
//! A hub served in the test on 127.0.0.1 and two daemons, each with its own temporary HOME and
//! XDG folders and fake agents; a machine "off" is its daemon stopped, then started again on the
//! same folders; throwaway repositories.

use crate::common;
use crate::remote_asks::{led_on_server, offline};
use crate::remote_launch::{
    Machine, a_while, allowing, done, eventually, folder_of, id_in, launched, pair, pc_and_server,
    prompted, refused, restarted, sent_with, server_user, titled,
};

use common::mcp::{Mcp, server_of};
use flottille_core::store::Store;
use flottille_link::{Call, Deliver, MachineMessage};
use serde_json::{Value, json};

/// The fake agents of `server` play `turns` (`crates/flottille-fake-agent`).
pub(crate) fn script(server: &Machine, turns: Value) {
    server.user.fake_script(json!({"turns": turns}));
}

/// A child of `pc`'s root started on `server` with `brief`: the child's id there, the launch's
/// name, and the root's MCP session.
pub(crate) fn child_on_server(
    pc: &Machine,
    server: &Machine,
    brief: &str,
) -> (String, String, Mcp) {
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");
    let answer = launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Write the tests", "brief": brief}),
    );
    let text = done(&answer).to_owned();
    let child = titled(server, "Write the tests");
    assert_eq!(id_in(&text), child["id"].as_str().unwrap(), "{text}");
    (
        child["id"].as_str().unwrap().to_owned(),
        launch_in(&text),
        mcp,
    )
}

/// The launch's name in the answer of a launch on `server`.
pub(crate) fn launch_in(text: &str) -> String {
    let marker = "refs/flottille/machines/server/";
    let at = text
        .find(marker)
        .unwrap_or_else(|| panic!("no ref in {text}"))
        + marker.len();
    text[at..].split('/').next().unwrap().to_owned()
}

/// `machine`'s daemon stopped: the machine is off until `on`.
pub(crate) fn off(machine: &Machine) {
    let stopped = machine.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", machine.log());
}

/// `machine`'s daemon started again on the same folders.
pub(crate) fn on(machine: &Machine) {
    machine.user.status();
}

/// Lets the turn the fake agents of `machine` hold go on.
pub(crate) fn release(machine: &Machine) {
    std::fs::write(machine.user.path("fake/release"), "").unwrap();
}

/// Waits until a fake agent of `machine` was answered `text` by one of its tools.
pub(crate) fn answered(machine: &Machine, text: &str) {
    eventually(&format!("a tool answering {text}"), || {
        machine
            .user
            .received()
            .iter()
            .any(|record| record["mcp"]["body"].to_string().contains(text))
    });
}

/// How many prompts `machine`'s fake agents were sent holding `text`.
pub(crate) fn prompts_with(machine: &Machine, text: &str) -> usize {
    machine
        .prompts()
        .iter()
        .map(|prompt| prompt.matches(text).count())
        .sum()
}

#[test]
fn a_report_written_while_the_parent_s_machine_is_off_reaches_it_when_it_is_back() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "The export tests are written."}}}
        ]}]),
    );
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");

    off(&pc);
    release(&server);
    answered(&server, "Your report went to your parent");
    eventually("the child ended with its report", || {
        server.agent(&child)["state"] == "ended"
    });
    a_while();
    on(&pc);

    pc.wait_for("server");
    prompted(&pc, "The export tests are written.");
    let report = pc
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains("The export tests are written."))
        .unwrap();
    assert!(report.contains("[flottille:child-report]"), "{report}");
    assert!(report.contains(&child), "{report}");
}

#[test]
fn deliveries_kept_for_a_machine_arrive_in_order_and_once() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "ask", "arguments": {"question": "Which format first, CSV or JSON?"}}},
            {"mcp": {"tool": "report", "arguments": {"text": "Both formats are tested."}}}
        ]}]),
    );
    let (_child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
    // pc stays linked, so the question goes to it, but takes nothing until it starts again: both
    // wait on server, kept (a question asked while pc is off would go to server's user).
    std::fs::write(pc.user.path("stopping"), "").unwrap();

    release(&server);
    answered(&server, "Your report went to your parent");
    a_while();
    off(&pc);
    std::fs::remove_file(pc.user.path("stopping")).unwrap();
    on(&pc);

    pc.wait_for("server");
    prompted(&pc, "Both formats are tested.");
    let all = pc.prompts().join("\n");
    let question = all.find("Which format first, CSV or JSON?").expect(&all);
    let report = all.find("Both formats are tested.").expect(&all);
    assert!(
        question < report,
        "the question came after the report:\n{all}"
    );
    // Each once, even when both machines start again and link anew.
    restarted(&pc);
    restarted(&server);
    pc.wait_for("server");
    server.wait_for("pc");
    a_while();
    assert_eq!(prompts_with(&pc, "Which format first, CSV or JSON?"), 1);
    assert_eq!(prompts_with(&pc, "Both formats are tested."), 1);
}

#[test]
fn the_report_names_the_ref_the_branch_came_back_into() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "Tests committed."}}}
        ]}]),
    );
    let (child, launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
    // The child's work, committed in its place there.
    let there = std::path::PathBuf::from(server.agent(&child)["folder"].as_str().unwrap());
    std::fs::write(there.join("tests.txt"), "the tests\n").unwrap();
    server.git(&there, &["add", "tests.txt"]);
    server.git(&there, &["commit", "--quiet", "-m", "the export tests"]);

    release(&server);

    prompted(&pc, "Tests committed.");
    let reference = format!("refs/flottille/machines/server/{launch}/feat/export");
    let report = pc
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains("Tests committed."))
        .unwrap();
    assert!(report.contains(&reference), "{report}");
    assert_eq!(
        pc.git(&pc.folder(), &["log", "-1", "--format=%s", &reference]),
        "the export tests"
    );
    // Nothing moved the caller's branch.
    assert_eq!(pc.git(&pc.folder(), &["log", "-1", "--format=%s"]), "b");
}

#[test]
fn agent_send_to_an_offline_child_is_kept_and_delivered() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");

    off(&server);
    eventually("server offline", || pc.seen("server")["state"] != "online");
    let answer = mcp.call(
        "agent_send",
        json!({"agent_id": child, "message": "Cover the CSV export too."}),
    );

    let text = done(&answer);
    assert!(text.contains("server is offline since"), "{text}");
    assert!(text.contains("kept"), "{text}");
    on(&server);
    server.wait_for("pc");
    prompted(&server, "Cover the CSV export too.");
}

#[test]
fn agent_status_of_an_offline_child_is_refused_saying_since_when() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
    // Reached while its machine is on.
    let status = mcp.call("agent_status", json!({"agent_id": child}));
    assert!(done(&status).contains(&child), "{}", status.text);

    off(&server);
    eventually("server offline", || pc.seen("server")["state"] != "online");
    let answer = mcp.call("agent_status", json!({"agent_id": child}));

    let text = refused(&answer);
    assert!(text.contains("server is offline since"), "{text}");
    assert!(text.contains("woken"), "{text}");
}

#[test]
fn a_call_from_an_agent_that_did_not_start_the_session_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    let caller = pc
        .agents()
        .into_iter()
        .find(|agent| agent["parent"].is_null())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let key = pc.stand_in();
    let call = |id: &str, caller: &str, tool: &str| {
        MachineMessage::Call(Box::new(Call {
            id: id.into(),
            launch: launch.clone(),
            caller: caller.into(),
            agent: child.clone(),
            tool: tool.into(),
            arguments: json!({"agent_id": child, "message": "Stop everything."}).to_string(),
            // A read carries the caller's project, as any call to a session of another machine.
            project: "app".into(),
            ..Call::default()
        }))
    };

    let answers = sent_with(
        &key,
        &server,
        vec![
            call("c1", "aaaaaaaaaaaa", "agent_send"),
            call("c2", "aaaaaaaaaaaa", "agent_stop"),
            // The agent that started it is taken.
            call("c3", &caller, "agent_status"),
        ],
    );

    for answer in &answers[..2] {
        let refused = answer.clone().unwrap_err();
        assert!(refused.contains("did not start"), "{refused}");
    }
    assert!(answers[2].is_ok(), "{:?}", answers[2]);
    a_while();
    assert_eq!(prompts_with(&server, "Stop everything."), 0);
    assert_ne!(server.agent(&child)["state"], "ended");
}

#[test]
fn a_delivery_kept_thirty_days_is_dropped_with_an_inbox_entry() {
    let user = server_user(allowing()).with_env("FLOTTILLE_TEST_CLOCK", "1");
    let (_hub, pc, server) = pc_and_server(user, Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "Written, long ago."}}}
        ]}]),
    );
    let (_child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
    off(&pc);
    release(&server);
    answered(&server, "Your report went to your parent");

    server
        .user
        .call(
            "test.advance",
            json!({"ms": 30u64 * 24 * 3600 * 1000 + 60_000}),
        )
        .unwrap();

    let entries = server.user.call("inbox.list", json!({})).unwrap()["entries"].clone();
    let dropped = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "dropped")
        .unwrap_or_else(|| panic!("no entry of a dropped delivery: {entries:#}"))
        .clone();
    let text = dropped["text"].as_str().unwrap();
    assert!(text.contains("pc"), "{text}");
    assert!(text.contains("30 days"), "{text}");
    on(&pc);
    pc.wait_for("server");
    a_while();
    assert_eq!(prompts_with(&pc, "Written, long ago."), 0);
}

#[test]
fn the_pilot_stops_its_led_session_on_the_other_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    // In Full, its end is let through here as a session's it leads on this machine would be.
    let (_root, mcp) = pc.root("full");
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export of the report."}),
    );
    let session = id_in(done(&answer));
    prompted(&server, "Finish the export of the report.");

    let stopped = mcp.call("agent_stop", json!({"agent_id": session}));

    let text = done(&stopped);
    assert!(text.contains("Ended the session"), "{text}");
    eventually("the session ended on server", || {
        server.agent(&session)["state"] == "ended"
    });
    let reason = server.agent(&session)["state_reason"].clone();
    assert!(
        reason.as_str().unwrap_or_default().contains("led it"),
        "{reason}"
    );
}

#[test]
fn a_child_whose_report_came_back_no_longer_counts_in_its_parent_s_cap() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.register("max_children = 1");
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "Done with the tests."}}}
        ]}]),
    );
    let (_child, _launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    let full = launched(
        &mcp,
        json!({"kind": "child", "title": "Local", "brief": "Here."}),
    );
    assert!(refused(&full).contains("its cap"), "{}", full.text);

    release(&server);
    prompted(&pc, "Done with the tests.");

    let again = launched(
        &mcp,
        json!({"kind": "child", "title": "Local", "brief": "Here."}),
    );
    done(&again);
}

#[test]
fn a_delivery_refused_while_the_parent_s_machine_stops_arrives_once_it_is_back() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "The export tests pass."}}}
        ]}]),
    );
    let (_child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
    // pc answers every delivery as a daemon that is stopping does.
    std::fs::write(pc.user.path("stopping"), "").unwrap();

    release(&server);
    answered(&server, "Your report went to your parent");
    eventually("pc refused the report for now", || {
        pc.log().contains("the daemon is stopping")
    });
    assert_eq!(prompts_with(&pc, "The export tests pass."), 0);

    off(&pc);
    std::fs::remove_file(pc.user.path("stopping")).unwrap();
    on(&pc);
    pc.wait_for("server");
    prompted(&pc, "The export tests pass.");
}

#[test]
fn a_child_whose_process_died_tells_its_parent_on_the_other_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [{"die": 3}]}]),
    );
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");

    prompted(&pc, "[flottille:child-replaced]");
    let told = pc
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains("[flottille:child-replaced]"))
        .unwrap();
    assert!(told.contains(&child), "{told}");
    assert!(told.contains("it goes on in its session"), "{told}");
}

#[test]
fn a_message_over_64000_characters_for_another_machine_is_refused_at_once() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");

    let answer = mcp.call(
        "agent_send",
        json!({"agent_id": child, "message": format!("Cover {}", "x".repeat(64_000))}),
    );

    let text = refused(&answer);
    assert!(text.contains("64000 characters at most"), "{text}");
    a_while();
    assert_eq!(prompts_with(&server, "Cover xxx"), 0);
}

#[test]
fn messages_for_a_machine_that_is_off_are_refused_past_100_waiting() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
    off(&server);
    eventually("server offline", || pc.seen("server")["state"] != "online");

    for n in 0..100 {
        let answer = mcp.call(
            "agent_send",
            json!({"agent_id": child, "message": format!("Note {n}.")}),
        );
        done(&answer);
    }
    let answer = mcp.call(
        "agent_send",
        json!({"agent_id": child, "message": "One more note."}),
    );

    let text = refused(&answer);
    assert!(text.contains("100 messages wait for server"), "{text}");
}

#[test]
fn a_delivery_for_an_unrelated_agent_or_another_launch_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    let parent = pc
        .agents()
        .into_iter()
        .find(|agent| agent["parent"].is_null())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (other, _) = pc.root("auto");
    let key = server.stand_in();
    let deliver = |id: &str, launch: &str, agent: &str, body: &str| {
        MachineMessage::Deliver(Box::new(Deliver {
            id: id.into(),
            launch: launch.into(),
            agent: agent.into(),
            kind: "child-question".into(),
            body: format!("Your child agent {child} asks: {body}"),
            urgency: "urgent".into(),
            ended: false,
        }))
    };

    let answers = sent_with(
        &key,
        &pc,
        vec![
            deliver("d1", &launch, &other, "Delete the main branch?"),
            deliver("d2", "another-launch", &parent, "Push to production?"),
            // The agent that started it is taken.
            deliver("d3", &launch, &parent, "Which format first?"),
        ],
    );

    for answer in &answers[..2] {
        let refused = answer.clone().unwrap_err();
        assert!(refused.contains("did not start launch"), "{refused}");
    }
    assert!(answers[2].is_ok(), "{:?}", answers[2]);
    prompted(&pc, "Which format first?");
    assert_eq!(prompts_with(&pc, "Delete the main branch?"), 0);
    assert_eq!(prompts_with(&pc, "Push to production?"), 0);
}

#[test]
fn a_delivery_named_after_an_extension_other_than_review_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (_child, launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    let parent = pc
        .agents()
        .into_iter()
        .find(|agent| agent["parent"].is_null())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let key = server.stand_in();
    let deliver = |id: &str, kind: &str, body: &str| {
        MachineMessage::Deliver(Box::new(Deliver {
            id: id.into(),
            launch: launch.clone(),
            agent: parent.clone(),
            kind: kind.into(),
            body: body.into(),
            urgency: "between-turns".into(),
            ended: false,
        }))
    };

    let answers = sent_with(
        &key,
        &pc,
        vec![
            // Only a review's result is ever sent across: another extension's name is no kind.
            deliver("d1", "autosubscriptions", "Comments woke you."),
            deliver("d2", "review", "The review of the export found nothing."),
        ],
    );

    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("no delivery of kind"), "{refused}");
    assert!(answers[1].is_ok(), "{:?}", answers[1]);
    prompted(&pc, "The review of the export found nothing.");
    assert_eq!(prompts_with(&pc, "Comments woke you."), 0);
}

#[test]
fn a_call_from_a_third_machine_for_a_launch_it_did_not_make_is_refused() {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let laptop = Machine::of(server_user(None));
    pair(&hub, &laptop, "laptop", "runner");
    let (child, launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    let caller = pc
        .agents()
        .into_iter()
        .find(|agent| agent["parent"].is_null())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server.wait_for("laptop");
    let key = laptop.stand_in();
    let call = |id: &str, tool: &str| {
        MachineMessage::Call(Box::new(Call {
            id: id.into(),
            launch: launch.clone(),
            caller: caller.clone(),
            agent: child.clone(),
            tool: tool.into(),
            arguments: json!({"agent_id": child, "message": "Stop everything."}).to_string(),
            ..Call::default()
        }))
    };

    let answers = sent_with(
        &key,
        &server,
        vec![
            call("c1", "agent_send"),
            call("c2", "agent_status"),
            call("c3", "agent_stop"),
        ],
    );

    for answer in &answers {
        let refused = answer.clone().unwrap_err();
        assert!(refused.contains("did not start"), "{refused}");
    }
    a_while();
    assert_eq!(prompts_with(&server, "Stop everything."), 0);
    assert_ne!(server.agent(&child)["state"], "ended");
}

/// What `pc` lists of the sessions its agents started elsewhere (`remote.launches`).
fn launches(pc: &Machine) -> Vec<Value> {
    pc.user.call("remote.launches", json!({})).unwrap()["launches"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn the_pilot_s_machine_lists_its_launches_with_what_it_last_heard() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "The export tests are written."}}}
        ]}]),
    );
    let (child, launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");

    let listed = launches(&pc);
    assert_eq!(listed.len(), 1, "{listed:#?}");
    let first = &listed[0];
    assert_eq!(first["machine"], "server");
    assert_eq!(first["launch"], launch);
    assert_eq!(first["agent"], child);
    assert_eq!(first["kind"], "child");
    assert_eq!(first["title"], "Write the tests");
    assert_eq!(first["state"], "started");
    assert_eq!(first["heard"], Value::Null, "nothing heard yet");

    release(&server);
    prompted(&pc, "The export tests are written.");
    eventually("its report is what was last heard", || {
        launches(&pc)
            .first()
            .is_some_and(|launch| launch["heard"] == "child-report")
    });
    let reported = &launches(&pc)[0];
    assert_eq!(reported["state"], "ended", "{reported:#}");
    assert!(reported["heard_at"].as_u64().is_some(), "{reported:#}");
}

#[test]
fn an_old_ended_launch_is_not_listed() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "The export tests are written."}}}
        ]}]),
    );
    let (_child, launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&pc, "The export tests are written.");
    // Ended with a report, it stays listed while the agent that started it lives.
    eventually("its report is what was last heard", || {
        launches(&pc)
            .first()
            .is_some_and(|listed| listed["heard"] == "child-report" && listed["state"] == "ended")
    });
    let caller = launches(&pc)[0]["caller"].as_str().unwrap().to_owned();

    pc.user
        .call("agent.stop", json!({"agent": caller}))
        .unwrap();
    eventually("the ended launch leaves the list", || {
        launches(&pc)
            .iter()
            .all(|listed| listed["launch"] != launch.as_str())
    });
}

/// The plain end of a turn, as the pilot is told it: JSON, so its line end is escaped.
const PLAIN_END: &str = "ended a turn.\\nIt runs on";

/// How many turns of `session` ended on `machine`.
fn turns_of(machine: &Machine, session: &str) -> usize {
    Store::open_read_only(&machine.user.path("state/flottille/flottille.db"))
        .unwrap()
        .last_agent_events(session, "agent.turn_ended", 300)
        .unwrap()
        .len()
}

#[test]
fn a_plain_turn_end_of_a_led_session_wakes_its_pilot_on_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "do": [{"say": "Halfway through."}]}]),
    );

    let (_session, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    prompted(&pc, "Halfway through.");
    let told = pc
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains("Halfway through."))
        .unwrap();
    assert!(told.contains("[flottille:session-turn-ended]"), "{told}");
    assert!(told.contains(PLAIN_END), "{told}");
    assert!(!told.contains("it reported"), "{told}");
}

#[test]
fn plain_turn_ends_kept_for_a_machine_never_push_out_a_report() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([
            {"when": "Finish the export.", "do": [{"say": "Started."}]},
            {"when": "More.", "do": [{"say": "More done."}]},
            {"when": "Report now.", "do": [
                {"mcp": {"tool": "report", "arguments": {"text": "Done."}}},
                {"say": "The export is finished."}
            ]}
        ]),
    );
    let (session, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    prompted(&pc, "Started.");
    off(&pc);
    offline(&server, "pc");

    // More turns than the messages that may wait for one machine.
    for turn in 2..=106 {
        server
            .user
            .call("agent.send", json!({"agent": session, "message": "More."}))
            .unwrap();
        eventually("a turn of the session", || {
            turns_of(&server, &session) == turn
        });
    }
    server
        .user
        .call(
            "agent.send",
            json!({"agent": session, "message": "Report now."}),
        )
        .unwrap();
    eventually("its report", || turns_of(&server, &session) == 107);
    on(&pc);

    pc.wait_for("server");
    prompted(&pc, "The export is finished.");
    a_while();
    // The one it was told of before the machine went off, and the latest after.
    assert_eq!(prompts_with(&pc, PLAIN_END), 2, "{:#?}", pc.prompts());
}

/// A secret only pc knows, in a secret file of its project's folder: the place of its root.
pub(crate) const PC_SECRET: &str = "pc-only-q8v3m1x7k2";

/// `PC_SECRET` written in the `.env` of pc's project's folder.
pub(crate) fn with_pc_secret(pc: &Machine) {
    std::fs::write(
        pc.folder().join(".env"),
        format!("EXPORT_KEY={PC_SECRET}\n"),
    )
    .unwrap();
}

/// How many deliveries `machine` stored, and prompts its agents were sent, holding `text`.
pub(crate) fn stored_or_prompted(machine: &Machine, text: &str) -> usize {
    let store = rusqlite::Connection::open(machine.user.path("state/flottille/flottille.db"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM deliveries WHERE instr(body, ?1) > 0",
            [text],
            |row| row.get::<_, usize>(0),
        )
        .unwrap();
    store + prompts_with(machine, text)
}

#[test]
fn a_report_from_a_child_on_another_machine_is_masked_with_the_caller_s_env() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {
                "text": format!("The export tests sign with {PC_SECRET} now.")
            }}}
        ]}]),
    );
    let (_child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    with_pc_secret(&pc);
    prompted(&server, "Write the tests of the export.");

    release(&server);
    prompted(&pc, "The export tests sign with");

    assert!(
        prompts_with(&pc, "sign with [masked] now.") > 0,
        "{:#?}",
        pc.prompts()
    );
    assert_eq!(stored_or_prompted(&pc, PC_SECRET), 0, "{:#?}", pc.prompts());
}

#[test]
fn a_secret_cut_in_half_in_a_delivery_from_another_machine_is_masked() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    with_pc_secret(&pc);
    let parent = pc
        .agents()
        .into_iter()
        .find(|agent| agent["parent"].is_null())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let key = server.stand_in();
    let deliver = |id: &str, body: String| {
        MachineMessage::Deliver(Box::new(Deliver {
            id: id.into(),
            launch: launch.clone(),
            agent: parent.clone(),
            kind: "child-question".into(),
            body,
            urgency: "urgent".into(),
            ended: false,
        }))
    };
    let half = &PC_SECRET[..12];
    // Cut by the other machine as `across::fitted` cuts a long delivery, saying so.
    let fitted = format!(
        "First question, cut: x{half}\n\n[cut: 900 more characters; session_read \
         {{\"session\": \"{child}\"}} on server reads the rest]"
    );
    // Cut at 64,000 characters without a word, as the answer of a request is.
    let head = "Second question, cut: ";
    let unmarked = format!(
        "{head}{}{half}",
        "x".repeat(64_000 - head.len() - half.len())
    );
    assert_eq!(unmarked.chars().count(), 64_000);

    let answers = sent_with(
        &key,
        &pc,
        vec![deliver("d1", fitted), deliver("d2", unmarked)],
    );

    for answer in &answers {
        assert!(answer.is_ok(), "{answer:?}");
    }
    prompted(&pc, "First question, cut: x[masked]");
    prompted(&pc, "Second question, cut: ");
    assert!(prompts_with(&pc, "x[masked]\\n\\n[cut: 900 more characters") > 0);
    assert_eq!(
        stored_or_prompted(&pc, &PC_SECRET[..6]),
        0,
        "{:#?}",
        pc.prompts()
    );
}

/// A secret only the server knows, in a secret file of the place of its agents.
pub(crate) const SERVER_SECRET: &str = "server-only-d4w9z6r1b8";

/// `SERVER_SECRET` written in the `.env` of the folder of `agent` of `server`.
pub(crate) fn with_server_secret(server: &Machine, agent: &str) {
    std::fs::write(
        folder_of(&server.agent(agent)).join(".env"),
        format!("EXPORT_KEY={SERVER_SECRET}\n"),
    )
    .unwrap();
}

#[test]
fn a_report_for_a_removed_workspace_never_holds_the_deliveries_behind_it() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    pc.user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace ready", || {
        pc.user
            .call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    // A parent that works in the workspace `w` of pc, with a child on the server.
    let opened = pc.user.received_requests("session/new").len();
    let parent = pc
        .user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "hello"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (url, authorization) = server_of(&pc.user, opened);
    let mcp = Mcp::initialize(&url, &authorization);
    eventually("the parent's first turn", || {
        pc.agent(&parent)["state"] == "idle"
    });
    pc.user
        .call("agent.mode", json!({"agent": parent, "mode": "auto"}))
        .unwrap();
    let answer = launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": "Write the tests of the export."}),
    );
    let launch = launch_in(done(&answer));
    // The parent ends and the user removes its workspace: nothing is left of its place to read.
    pc.user
        .call("agent.stop", json!({"agent": parent}))
        .unwrap();
    eventually("the parent ended", || {
        pc.agent(&parent)["state"] != "idle" && pc.agent(&parent)["state"] != "running"
    });
    let removed = pc.user.run(&["workspace", "remove", "app", "w"]);
    assert!(removed.status.success(), "{}", pc.log());

    let deliver = |id: &str, body: &str| {
        MachineMessage::Deliver(Box::new(Deliver {
            id: id.into(),
            launch: launch.clone(),
            agent: parent.clone(),
            kind: "child-report".into(),
            body: body.into(),
            urgency: "urgent".into(),
            ended: false,
        }))
    };
    let answers = sent_with(
        &server.stand_in(),
        &pc,
        vec![
            deliver("d1", "First report, for a place that is gone."),
            deliver("d2", "Second report, behind the first."),
        ],
    );

    // Neither is kept for later: the first would hold the second, and all that follows it.
    for answer in &answers {
        let answer = answer.as_ref().unwrap_or_else(|why| panic!("{why}"));
        assert!(answer["later"].is_null(), "{answer}");
    }
    for text in ["First report", "Second report"] {
        assert!(stored_or_prompted(&pc, text) > 0, "{text}: {}", pc.log());
    }
}

#[test]
fn an_answer_for_an_unknown_agent_is_dropped_not_kept() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);

    let answers = sent_with(
        &server.stand_in(),
        &pc,
        vec![MachineMessage::Deliver(Box::new(Deliver {
            id: "approval-7".into(),
            launch: "request-7".into(),
            agent: "aaaaaaaaaaaa".into(),
            kind: "approval".into(),
            body: "Approved.".into(),
            urgency: "urgent".into(),
            ended: false,
        }))],
    );

    // Refused for good, so that its sender forgets it and nothing waits behind it.
    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("waits for no answer"), "{refused}");
}

#[test]
fn a_message_sent_from_another_machine_is_masked_with_the_receiver_s_place() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (child, _launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&server, "Write the tests of the export.");
    with_server_secret(&server, &child);

    let sent = mcp.call(
        "agent_send",
        json!({"agent_id": child, "message": format!("Sign the export with {SERVER_SECRET} now.")}),
    );

    done(&sent);
    prompted(&server, "Sign the export with [masked] now.");
    assert_eq!(
        stored_or_prompted(&server, SERVER_SECRET),
        0,
        "{:#?}",
        server.prompts()
    );
}

#[test]
fn a_brief_from_another_machine_is_masked_with_the_receiver_s_place() {
    // The setup puts the secret in the workspace each child works in, as a copied `.env` is.
    let user = server_user(allowing());
    let file = user.path("export.env");
    std::fs::write(&file, format!("EXPORT_KEY={SERVER_SECRET}\n")).unwrap();
    let setup = format!("\n[[setup]]\nrun = \"cp {} .env\"\n", file.display());
    let (_hub, pc, server) = pc_and_server(user, Some(&setup));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");

    let answer = launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": format!("Sign the export tests with {SERVER_SECRET}.")}),
    );

    done(&answer);
    prompted(&server, "Sign the export tests with [masked].");
    assert_eq!(
        stored_or_prompted(&server, SERVER_SECRET),
        0,
        "{:#?}",
        server.prompts()
    );
}
