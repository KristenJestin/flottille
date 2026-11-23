//! A session led from another machine is a led session (`docs/remote.md`, "A tree across
//! machines"; `docs/tools.md`, "Sessions an agent starts"): the machine that runs it records its
//! pilot, the caller stood in for there; it wakes its pilot when it asks, reports or ends, is told
//! where its questions go, ends with its pilot, and is detached or unfollowed as on one machine.
//! A hub served in the test on 127.0.0.1 and two daemons, each with its own temporary HOME and
//! XDG folders and fake agents.

use crate::remote_deliveries::{answered, launch_in, release, script};
use crate::remote_launch::{
    Machine, TestHub, a_while, allowing, done, eventually, id_in, launched, pair, pc_and_server,
    prompted, refused, sent_with, server_user,
};

use crate::common::mcp::Mcp;
use flottille_core::store::{LaunchFilter, Store};
use flottille_link::{Call, MachineMessage};
use serde_json::{Value, json};

const QUESTION: &str = "Which format first, CSV or JSON?";

/// The hex of `machine`'s key, as a caller of it is stood in for on another machine.
fn hex_of(machine: &Machine) -> String {
    machine
        .key()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A session led by `pc`'s root (in `mode`), started on `server` with `brief`: its id there, the
/// root's id, the launch's answer and the root's MCP session.
fn led_on_server(
    pc: &Machine,
    server: &Machine,
    brief: &str,
    mode: &str,
) -> (String, String, String, Mcp) {
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root(mode);
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export", "brief": brief}),
    );
    let text = done(&answer).to_owned();
    let session = id_in(&text);
    prompted(server, brief);
    (session, root, text, mcp)
}

/// The first of `machine`'s prompts holding `text`, once there.
fn prompt_with(machine: &Machine, text: &str) -> String {
    prompted(machine, text);
    machine
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains(text))
        .unwrap()
}

/// The text a fake agent of `machine` was answered by its tool, the first one holding `text`.
fn tool_answer(machine: &Machine, text: &str) -> String {
    answered(machine, text);
    machine
        .user
        .received()
        .iter()
        .map(|record| record["mcp"]["body"].to_string())
        .find(|body| body.contains(text))
        .unwrap()
}

/// The questions in `machine`'s inbox.
fn questions(machine: &Machine) -> Vec<Value> {
    machine.user.call("inbox.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == "question")
        .cloned()
        .collect()
}

/// The followings of sessions `agent` of `machine` has, as `subscription.list` gives them.
fn followings(machine: &Machine, agent: &str) -> Vec<Value> {
    machine.user.call("subscription.list", json!({})).unwrap()["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["agent"] == agent && s["source"] == "session")
        .cloned()
        .collect()
}

/// `machine`'s project `app` as `register` writes it, its repository on a forge.
fn with_a_forge(machine: &Machine) {
    let text = format!(
        "folder = \"{}\"\nworkspaces_root = \"{}\"\nbranch_prefix = \"\"\n\
         [[repositories]]\npath = \".\"\nbase = [\"main\"]\n\
         forge = {{ kind = \"github\", repository = \"acme/app\" }}\n",
        machine.folder().display(),
        machine.user.path("work/app-work").display()
    );
    machine.user.write_project_file("app", &text);
    machine.user.call("project.list", json!({})).unwrap();
}

/// The launches `pc` shows under their pilot (`remote.launches`).
fn launches(pc: &Machine) -> Vec<Value> {
    pc.user.call("remote.launches", json!({})).unwrap()["launches"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_led_session_on_another_machine_wakes_its_pilot_when_it_asks() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"mcp": {"tool": "ask", "arguments": {"question": QUESTION}}}
        ]}]),
    );

    let (session, _root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    let woken = prompt_with(&pc, QUESTION);
    assert!(woken.contains("[flottille:session-turn-ended]"), "{woken}");
    assert!(
        woken.contains("it asked you a question with ask"),
        "{woken}"
    );
    assert!(woken.contains(&session), "{woken}");
}

#[test]
fn a_led_session_on_another_machine_wakes_its_pilot_when_it_reports() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "The export is finished."}}},
            {"say": "The export is finished, CSV and JSON."}
        ]}]),
    );

    let (session, _root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    let woken = prompt_with(&pc, "it reported");
    assert!(woken.contains("[flottille:session-turn-ended]"), "{woken}");
    assert!(woken.contains(&session), "{woken}");
    assert!(
        woken.contains("The export is finished, CSV and JSON."),
        "{woken}"
    );
}

#[test]
fn a_led_session_on_another_machine_wakes_its_pilot_when_it_ends() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, _root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    server
        .user
        .call("agent.stop", json!({"agent": session}))
        .unwrap();

    let woken = prompt_with(&pc, "Your following of it is over.");
    assert!(woken.contains("[flottille:session-turn-ended]"), "{woken}");
    assert!(woken.contains(&session), "{woken}");
    assert!(woken.contains("ended"), "{woken}");
}

#[test]
fn a_led_session_from_another_machine_is_led_by_its_pilot_there() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));

    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    let pilot = format!("{root}@{}", hex_of(&pc));
    let shown = server.agent(&session);
    assert_eq!(shown["launched_by"], pilot.as_str(), "{shown:#}");
    assert_eq!(shown["led_by"], pilot.as_str(), "{shown:#}");
    let listed = server
        .agents()
        .into_iter()
        .find(|agent| agent["id"] == session.as_str())
        .unwrap();
    assert_eq!(listed["led_by"], pilot.as_str(), "{listed:#}");
    // The stand-in is no session of the server's: its light list still holds the session, and
    // every machine takes that list.
    eventually("the session in pc's list of server's", || {
        pc.user
            .call("remote.sessions", json!({}))
            .unwrap()
            .to_string()
            .contains(&session)
    });
    let lists = pc.user.call("remote.sessions", json!({})).unwrap();
    assert!(
        !lists.to_string().contains(&format!("@{}", hex_of(&pc))),
        "{lists:#}"
    );
}

#[test]
fn a_led_session_from_another_machine_is_told_where_its_questions_go() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));

    let (_session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    let first = prompt_with(&server, "Finish the export.");
    assert!(first.contains("Where a question goes:"), "{first}");
    assert!(
        first.contains(&format!("this session is led by agent {root} of pc")),
        "{first}"
    );
}

#[test]
fn the_window_shows_a_session_led_from_another_machine_without_the_agent_only_paragraphs() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (_session, _root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    // The fake agent keeps the prompt as the ACP content blocks it was sent.
    let first: String =
        serde_json::from_str::<Vec<Value>>(&prompt_with(&server, "Finish the export."))
            .unwrap()
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect();

    // What the daemon really sent, folded as the window folds it: a session led here shows its
    // brief alone, and so does this one.
    let mut conversation =
        flottille_window::conversation::Conversation::new("session".into(), None);
    conversation.apply(&flottille_protocol::Event {
        seq: 1,
        at: 1,
        name: "agent.prompted".into(),
        data: json!({ "text": first }),
    });
    let shown = conversation
        .blocks()
        .iter()
        .find_map(|block| match block {
            flottille_window::conversation::Block::Message { from, text, .. }
                if from.starts_with("started by") =>
            {
                Some(text.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no started-by message in {:?}", conversation.blocks()));
    assert_eq!(shown.trim(), "Finish the export.", "{shown}");
}

#[test]
fn the_answer_of_a_led_launch_on_another_machine_says_it_wakes_its_pilot() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));

    let (_session, _root, text, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    assert!(text.contains("Started a session led by you"), "{text}");
    assert!(text.contains("[flottille:session-turn-ended]"), "{text}");
    assert!(text.contains("session_send"), "{text}");
    assert!(text.contains("forge unsubscribe"), "{text}");
}

#[test]
fn a_held_launch_on_another_machine_says_what_follows_once_allowed() {
    // No judge on server: each launch is held for its user.
    let (_hub, pc, server) = pc_and_server(server_user(None), Some(""));
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");

    let led = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export."}),
    );
    let child = launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": "Write the tests."}),
    );

    let led = done(&led);
    assert!(led.contains("Waiting for the user's approval"), "{led}");
    assert!(
        led.contains(
            "Once the user allows it there, it runs as a session led by you: each end of its \
             turns wakes you as a local led session's does, and session_send reaches it."
        ),
        "{led}"
    );
    assert!(!led.contains("told nothing more"), "{led}");
    let child = done(&child);
    assert!(
        child.contains(
            "Once the user allows it there, it runs as your child: its report, questions and \
             failure come to you as a local child's do, and agent_send reaches it."
        ),
        "{child}"
    );
    assert!(!child.contains("told nothing more"), "{child}");
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn the_pilot_reaches_its_led_session_on_another_machine_without_a_leave() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, _root, _, mcp) = led_on_server(&pc, &server, "Finish the export.", "full");

    done(&mcp.call(
        "session_send",
        json!({"session": session, "text": "CSV first, JSON next."}),
    ));
    prompted(&server, "CSV first, JSON next.");
    let read = mcp.call("session_read", json!({"session": session}));
    assert!(done(&read).contains(&session), "{}", read.text);
    // As here, a session it leads is no child: agent_status reads its children only.
    let status = mcp.call("agent_status", json!({"agent_id": session}));
    assert!(
        refused(&status).contains("not one of your children"),
        "{}",
        status.text
    );
    // No leave was asked of the user there, as for a session it leads here.
    assert!(server.requests().is_empty(), "{:#?}", server.requests());

    let stopped = mcp.call("agent_stop", json!({"agent_id": session}));

    assert!(
        done(&stopped).contains("Ended the session"),
        "{}",
        stopped.text
    );
    eventually("the session ended on server", || {
        server.agent(&session)["state"] == "ended"
    });
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

#[test]
fn the_pilot_writes_to_its_led_session_on_another_machine_in_auto() {
    // pc's judge too allows: in Auto the message is judged, then checked again.
    let (_hub, pc, server) = crate::remote_launch::pc_and_server_with(
        "runner",
        server_user(allowing()),
        server_user(allowing()),
        Some(""),
    );
    let (session, _root, _, mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    let sent = mcp.call(
        "session_send",
        json!({"session": session, "text": "CSV first, JSON next."}),
    );

    assert!(!sent.error, "{}", sent.text);
    prompted(&server, "CSV first, JSON next.");
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
    assert!(pc.requests().is_empty(), "{:#?}", pc.requests());
}

#[test]
fn a_led_session_on_another_machine_ends_with_its_pilot() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    pc.user.call("agent.stop", json!({"agent": root})).unwrap();

    eventually("the session ended on server", || {
        server.agent(&session)["state"] == "ended"
    });
    assert_eq!(
        server.agent(&session)["state_reason"],
        "its pilot ended",
        "{:#}",
        server.agent(&session)
    );
}

#[test]
fn detaching_a_led_session_from_another_machine_tells_its_pilot() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    assert_eq!(
        followings(&pc, &root).len(),
        1,
        "{:#?}",
        followings(&pc, &root)
    );

    let detached = server
        .user
        .call("agent.detach", json!({"agent": session}))
        .unwrap();

    let pilot = format!("{root}@{}", hex_of(&pc));
    assert!(detached["agent"]["led_by"].is_null(), "{detached:#}");
    assert_eq!(
        detached["agent"]["detached_from"],
        pilot.as_str(),
        "{detached:#}"
    );
    let told = prompt_with(&pc, "the user detached the session");
    assert!(told.contains("[flottille:detached]"), "{told}");
    assert!(told.contains(&session), "{told}");
    eventually("the pilot's following ended", || {
        followings(&pc, &root).is_empty()
    });
    // It is the user's now: no longer a row under its pilot here.
    eventually("the launch no longer shown under its pilot", || {
        launches(&pc)
            .iter()
            .all(|launch| launch["agent"] != session.as_str())
    });
    let session_told = prompt_with(&server, "detached this session");
    assert!(session_told.contains("the user's alone"), "{session_told}");
}

#[test]
fn the_pilot_unsubscribes_from_its_led_session_on_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "ask", "arguments": {"question": QUESTION}}}
        ]}]),
    );
    // A forge declared: the root has the forge tool, whose unsubscribe ends a following.
    with_a_forge(&pc);
    let (session, root, _, mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    let following = followings(&pc, &root);
    assert_eq!(following.len(), 1, "{following:#?}");
    assert_eq!(following[0]["target"]["session"], session.as_str());

    let answer = mcp.call(
        "forge",
        json!({"action": "unsubscribe", "session": session}),
    );

    assert!(
        done(&answer).contains("no longer follow"),
        "{}",
        answer.text
    );
    assert!(followings(&pc, &root).is_empty());
    // Told between its turns, as one led here is: its turn still runs.
    eventually("the session told on server", || {
        Store::open_read_only(&server.user.path("state/flottille/flottille.db"))
            .unwrap()
            .deliveries(&session, "queued")
            .unwrap()
            .iter()
            .any(|delivery| delivery.kind == "unfollowed")
    });
    // Its questions now go to the user there; the pilot is not woken by them.
    release(&server);
    let said = tool_answer(&server, "given to the user");
    assert!(!said.contains("offline"), "{said}");
    eventually("the question in server's inbox", || {
        !questions(&server).is_empty()
    });
    let told = prompt_with(&server, "no longer follows it");
    assert!(told.contains("[flottille:unfollowed]"), "{told}");
    a_while();
    assert!(
        !pc.prompts().iter().any(|prompt| prompt.contains(QUESTION)),
        "{:#?}",
        pc.prompts()
    );
}

/// A call of `tool` with `args` as pc's daemon sends it for `caller`'s launch `launch` of `agent`.
fn call_of(
    id: &str,
    launch: &str,
    caller: &str,
    agent: &str,
    tool: &str,
    args: Value,
) -> MachineMessage {
    MachineMessage::Call(Box::new(Call {
        id: id.into(),
        launch: launch.into(),
        caller: caller.into(),
        agent: agent.into(),
        tool: tool.into(),
        arguments: args.to_string(),
        ..Call::default()
    }))
}

/// What server stored of the following of the launch of `session`.
fn following_there(server: &Machine, session: &str) -> Value {
    Store::open_read_only(&server.user.path("state/flottille/flottille.db"))
        .unwrap()
        .launches("in", None, LaunchFilter::Agent(session))
        .unwrap()
        .pop()
        .unwrap()
        .asked["following"]
        .clone()
}

/// `laptop`, a third machine of the user's, linked to `server`, and the key it links with.
fn laptop_of(hub: &TestHub, server: &Machine) -> (Machine, std::path::PathBuf) {
    let laptop = Machine::of(server_user(None));
    pair(hub, &laptop, "laptop", "runner");
    server.wait_for("laptop");
    let key = laptop.stand_in();
    (laptop, key)
}

#[test]
fn a_following_from_a_third_machine_is_refused() {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, text, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    let (_laptop, key) = laptop_of(&hub, &server);

    let answers = sent_with(
        &key,
        &server,
        vec![call_of(
            "call-f1",
            &launch_in(&text),
            &root,
            &session,
            "following",
            json!({"state": "ended"}),
        )],
    );

    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("did not start"), "{refused}");
    assert!(following_there(&server, &session).is_null());
}

#[test]
fn a_following_of_a_handed_session_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("auto");
    let text = done(&launched(
        &mcp,
        json!({"kind": "handed", "machine": "server", "title": "Finish the export",
            "brief": "Finish the export."}),
    ))
    .to_owned();
    let session = id_in(&text);
    prompted(&server, "Finish the export.");
    let key = pc.stand_in();

    let answers = sent_with(
        &key,
        &server,
        vec![call_of(
            "call-f2",
            &launch_in(&text),
            &root,
            &session,
            "following",
            json!({"state": "ended"}),
        )],
    );

    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("handed to the user"), "{refused}");
    assert!(following_there(&server, &session).is_null());
}

#[test]
fn a_following_of_a_state_that_is_none_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, text, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    let key = pc.stand_in();

    let answers = sent_with(
        &key,
        &server,
        vec![call_of(
            "call-f3",
            &launch_in(&text),
            &root,
            &session,
            "following",
            json!({"state": "forgotten"}),
        )],
    );

    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("on, paused or ended"), "{refused}");
    assert!(following_there(&server, &session).is_null());
}

#[test]
fn a_pilot_ended_stop_from_a_machine_that_did_not_start_the_session_is_refused() {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, text, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    let (_laptop, key) = laptop_of(&hub, &server);

    let answers = sent_with(
        &key,
        &server,
        vec![call_of(
            "call-s1",
            &launch_in(&text),
            &root,
            &session,
            "agent_stop",
            json!({"agent_id": session, "why": "pilot-ended"}),
        )],
    );

    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("did not start"), "{refused}");
    a_while();
    assert_ne!(server.agent(&session)["state"], "ended");
}

#[test]
fn ending_a_pilot_lists_its_led_sessions_on_other_machines() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    let ending = pc
        .user
        .call("agent.ending", json!({"agent": root}))
        .unwrap();

    let sessions = ending["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{ending:#}");
    assert_eq!(sessions[0]["id"], session.as_str(), "{ending:#}");
    assert_eq!(sessions[0]["title"], "Finish the export", "{ending:#}");
    assert_eq!(sessions[0]["machine"], "server", "{ending:#}");
    assert_eq!(sessions[0]["project"], "app", "{ending:#}");
}
