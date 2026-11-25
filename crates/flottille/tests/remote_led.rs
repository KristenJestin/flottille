//! A session led from another machine is a led session (`docs/remote.md`, "A tree across
//! machines"; `docs/tools.md`, "Sessions an agent starts"): the machine that runs it records its
//! pilot, the caller stood in for there; it wakes its pilot when it asks, reports or ends, is told
//! where its questions go, ends with its pilot, and is detached or unfollowed as on one machine.
//! A hub served in the test on 127.0.0.1 and two daemons, each with its own temporary HOME and
//! XDG folders and fake agents.

use crate::remote_deliveries::{answered, launch_in, off, on, release, script};
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
pub(crate) fn led_on_server(
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
fn detaching_a_handed_session_from_another_machine_stores_nothing_for_its_stand_in() {
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

    server
        .user
        .call("agent.detach", json!({"agent": session}))
        .unwrap();

    // Its session is told it is the user's alone; the stand-in of its starter, which nobody reads
    // on this machine, is told nothing.
    prompt_with(&server, "detached this session");
    let stand_in = format!("{root}@{}", hex_of(&pc));
    let stored = Store::open_read_only(&server.user.path("state/flottille/flottille.db"))
        .unwrap()
        .delivery(&format!("detached-{stand_in}-from-{session}"))
        .unwrap();
    assert!(stored.is_none(), "{stored:?}");
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
    let folder = crate::remote_launch::folder_of(&server.agent(&session));
    std::fs::write(folder.join("notes.txt"), "draft\n").unwrap();

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
    // Asked there: what its workspace holds that is not saved, as that machine reads it.
    assert_eq!(sessions[0]["state"], "uncommitted", "{ending:#}");
    let losses = sessions[0]["losses"].to_string();
    assert!(losses.contains("notes.txt"), "{ending:#}");
    assert!(!losses.contains("unknown (on server)"), "{ending:#}");
}

#[test]
fn the_end_confirmation_says_unknown_when_the_other_machine_does_not_answer() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    off(&server);
    eventually("server offline", || pc.seen("server")["state"] != "online");

    let asked = std::time::Instant::now();
    let ending = pc
        .user
        .call("agent.ending", json!({"agent": root}))
        .unwrap();

    // Linked to no one now: said at once, the wait is not what answers.
    assert!(
        asked.elapsed() < std::time::Duration::from_secs(3),
        "{:?}",
        asked.elapsed()
    );
    let sessions = ending["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{ending:#}");
    assert_eq!(sessions[0]["id"], session.as_str(), "{ending:#}");
    assert_eq!(sessions[0]["state"], "unknown", "{ending:#}");
    assert_eq!(
        sessions[0]["losses"],
        json!(["unknown (on server)"]),
        "{ending:#}"
    );
}

#[test]
fn the_end_confirmation_gives_up_on_a_machine_that_answers_too_slowly() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    let slow = server.user.path("slow-call");
    std::fs::write(&slow, "").unwrap();

    let asked = std::time::Instant::now();
    let ending = pc
        .user
        .call("agent.ending", json!({"agent": root}))
        .unwrap();
    let waited = asked.elapsed();
    std::fs::remove_file(&slow).unwrap();

    // Linked, but not answering: it is waited for 3 seconds, not for as long as it takes.
    assert!(
        waited >= std::time::Duration::from_millis(2900),
        "{waited:?}"
    );
    assert!(waited < std::time::Duration::from_secs(6), "{waited:?}");
    let sessions = ending["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{ending:#}");
    assert_eq!(sessions[0]["id"], session.as_str(), "{ending:#}");
    assert_eq!(sessions[0]["state"], "unknown", "{ending:#}");
    assert_eq!(
        sessions[0]["losses"],
        json!(["unknown (on server)"]),
        "{ending:#}"
    );
}

#[test]
fn the_losses_from_another_machine_are_masked_of_the_secrets_of_the_pilot_s_place() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    std::fs::write(
        crate::remote_launch::folder_of(&pc.agent(&root)).join(".env"),
        "EXPORT_TOKEN=orchid-7731-lantern\n",
    )
    .unwrap();
    // A file the other machine names in what it holds unsaved, and that is a secret here.
    let folder = crate::remote_launch::folder_of(&server.agent(&session));
    std::fs::write(folder.join("orchid-7731-lantern.txt"), "draft\n").unwrap();

    let ending = pc
        .user
        .call("agent.ending", json!({"agent": root}))
        .unwrap();

    let losses = ending["sessions"][0]["losses"].to_string();
    assert!(losses.contains(".txt"), "{ending:#}");
    assert!(!losses.contains("orchid-7731-lantern"), "{ending:#}");
}

#[test]
fn the_losses_of_a_session_are_not_read_by_a_machine_that_did_not_start_it() {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, text, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    let (_laptop, key) = laptop_of(&hub, &server);

    let answers = sent_with(
        &key,
        &server,
        vec![call_of(
            "call-l1",
            &launch_in(&text),
            &root,
            &session,
            "session_losses",
            json!({}),
        )],
    );

    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("did not start"), "{refused}");
}

#[test]
fn the_losses_of_a_child_are_not_read_by_its_parent() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.wait_for_offer("server", 12);
    let (root, mcp) = pc.root("auto");
    let text = done(&launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": "Write the tests of the export."}),
    ))
    .to_owned();
    let child = id_in(&text);
    prompted(&server, "Write the tests of the export.");
    let key = pc.stand_in();

    let answers = sent_with(
        &key,
        &server,
        vec![call_of(
            "call-l2",
            &launch_in(&text),
            &root,
            &child,
            "session_losses",
            json!({}),
        )],
    );

    let refused = answers[0].clone().unwrap_err();
    assert!(
        refused.contains("only a session the caller leads"),
        "{refused}"
    );
}

#[test]
fn the_losses_of_a_handed_session_are_not_read_by_the_machine_that_handed_it() {
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
            "call-l3",
            &launch_in(&text),
            &root,
            &session,
            "session_losses",
            json!({}),
        )],
    );

    let refused = answers[0].clone().unwrap_err();
    assert!(refused.contains("handed to the user"), "{refused}");
}

#[test]
fn a_pilot_ended_while_the_machine_is_off_ends_them_when_it_is_back() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, root, _, mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    let child = id_in(done(&launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": "Write the tests of the export."}),
    )));
    prompted(&server, "Write the tests of the export.");
    off(&server);
    eventually("server offline", || pc.seen("server")["state"] != "online");

    pc.user.call("agent.stop", json!({"agent": root})).unwrap();
    on(&server);
    server.wait_for("pc");

    eventually("both ended on server", || {
        server.agent(&session)["state"] == "ended" && server.agent(&child)["state"] == "ended"
    });
    assert_eq!(
        server.agent(&session)["state_reason"],
        "its pilot ended",
        "{:#}",
        server.agent(&session)
    );
    assert_eq!(
        server.agent(&child)["state_reason"],
        "its parent stopped",
        "{:#}",
        server.agent(&child)
    );
}

#[test]
fn every_machine_is_told_the_pilot_of_another_machine_of_a_led_session() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));

    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    // Server tells the machines it links to the pilot of its session, by its machine's name and
    // its id there: never in the list's `pilot`, which every machine takes as it did.
    let told = || {
        pc.user.call("remote.sessions", json!({})).unwrap()["machines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|machine| machine["name"] == "server")
            .flat_map(|machine| machine["sessions"].as_array().cloned().unwrap_or_default())
            .find(|listed| listed["id"] == session.as_str())
    };
    eventually("pc is told the pilot of server's session", || {
        told().is_some_and(|listed| !listed["pilot_elsewhere"].is_null())
    });
    let listed = told().unwrap();
    assert_eq!(
        listed["pilot_elsewhere"],
        json!({"machine": "pc", "agent": root}),
        "{listed:#}"
    );
    assert!(listed["pilot"].is_null(), "{listed:#}");
    // Server says it of its own list too, as its window reads it while another machine is in
    // front.
    let this = server.user.call("remote.sessions", json!({})).unwrap()["this"].clone();
    let own = this
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["id"] == session.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("{this:#}"));
    assert_eq!(
        own["pilot_elsewhere"],
        json!({"machine": "pc", "agent": root}),
        "{own:#}"
    );
}

#[test]
fn a_failed_pilot_does_not_end_its_led_session_on_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    pc.user
        .fake_script(json!({"load_error": "the session is lost", "turns": [
        {"when": "die now", "do": [{"die": 3}]}]}));
    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");

    pc.user
        .call("agent.send", json!({"agent": root, "message": "die now"}))
        .unwrap();

    eventually("the pilot failed on pc", || {
        pc.agent(&root)["state"] == "failed"
    });
    // It can be resumed: what it leads goes on, there as here.
    a_while();
    a_while();
    let there = server.agent(&session);
    assert!(
        !matches!(there["state"].as_str(), Some("ended" | "failed")),
        "{there:#}"
    );
}

// A lead reviews the session it leads on another machine as one it leads here (#1279): the review
// is started there, judged there, its findings go to that session and its pilot is told here.

/// pc's root leading a session on server, Review on on both machines; the server's review agents
/// hang in their turn, so that the test acts as its pilot. The session's id there and the root's
/// MCP session.
fn led_with_review(pc: &Machine, server: &Machine) -> (String, Mcp) {
    for machine in [pc, server] {
        machine
            .user
            .call("extension.set", json!({"name": "review", "enabled": true}))
            .unwrap();
    }
    script(
        server,
        json!([{"when": "of the change of workspace", "do": [{"hang": true}]}]),
    );
    let (session, _root, _, mcp) = led_on_server(pc, server, "Finish the export.", "auto");
    eventually("the session's first turn", || {
        server.agent(&session)["state"] == "idle"
    });
    (session, mcp)
}

/// The reviews of `session` on `machine`.
fn reviews_of(machine: &Machine, session: &str) -> Vec<Value> {
    machine
        .user
        .call("review.list", json!({"session": session}))
        .unwrap()["reviews"]
        .as_array()
        .unwrap()
        .clone()
}

/// The MCP session of the pilot of a review on `machine`, as its fake agent was given it.
fn pilot_there(machine: &Machine) -> Mcp {
    let find = || {
        machine
            .user
            .received_requests("session/new")
            .into_iter()
            .find(|opened| {
                opened["params"]["_meta"]["claudeCode"]["options"]["systemPrompt"]["prompt"][2]
                    .as_str()
                    .is_some_and(|prompt| prompt.contains("You are the pilot of a review"))
            })
    };
    eventually("the pilot's session", || find().is_some());
    let found = find();
    let server = found.unwrap()["params"]["mcpServers"][0].clone();
    Mcp::initialize(
        server["url"].as_str().unwrap(),
        server["headers"][0]["value"].as_str().unwrap(),
    )
}

#[test]
fn a_lead_reviews_its_led_session_on_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, mcp) = led_with_review(&pc, &server);

    let started = mcp.call("review", json!({"action": "start", "agent": session}));

    let text = done(&started);
    assert!(text.contains("round 1"), "{text}");
    assert!(text.contains(&session), "names the session: {text}");
    let all = reviews_of(&server, &session);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["state"], "running");
    // Its pilot is the reviewed session's child there, as on one machine.
    let pilot = server.agent(all[0]["pilot"].as_str().unwrap());
    assert_eq!(pilot["parent"], session.as_str(), "{pilot:#}");
    // Started for the session the root leads, which needs no leave of the user there.
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

#[test]
fn the_findings_of_a_review_there_are_told_to_the_pilot_here() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, mcp) = led_with_review(&pc, &server);
    done(&mcp.call("review", json!({"action": "start", "agent": session})));
    let pilot = pilot_there(&server);

    let reported = pilot.call(
        "review",
        json!({"action": "report", "summary": "Read the export.", "findings": [
            {"severity": "high", "confidence": "confirmed", "file": "a.txt", "lines": "1",
                "concerns": "code", "text": "The export drops the last line.",
                "case": "export a file of two lines"}
        ]}),
    );

    assert!(!reported.error, "{}", reported.text);
    // The findings go to the session reviewed, there.
    prompted(&server, "The review of your change");
    // Its pilot is told here, as of a session it leads on this machine.
    let told = prompt_with(&pc, "the session you lead");
    assert!(told.contains("[flottille:review]"), "{told}");
    assert!(told.contains(&session), "names it: {told}");
    assert!(told.contains("The export drops the last line."), "{told}");
}

/// `review` with `args`, as pc's daemon sends it for `root`'s launch `launch` of `session`.
fn review_call(id: &str, launch: &str, root: &str, session: &str, args: Value) -> MachineMessage {
    MachineMessage::Call(Box::new(Call {
        id: id.into(),
        launch: launch.into(),
        caller: root.into(),
        agent: session.into(),
        tool: "review".into(),
        arguments: args.to_string(),
        project: "app".into(),
        mode: "auto".into(),
        ..Call::default()
    }))
}

/// What the review tool of the machine says to a call that is not the review of a led session.
const LED_ONLY: &str = "review only starts the review of a session its pilot leads here";

#[test]
fn the_pilot_of_another_machine_starts_no_other_review_there() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, mcp) = led_with_review(&pc, &server);
    let (root, text) = {
        let launch = &launches(&pc)[0];
        (
            launch["caller"].as_str().unwrap().to_owned(),
            launch["launch"].as_str().unwrap().to_owned(),
        )
    };
    // A child of the same root there: a launch of its own, which is not a led one.
    let started = done(&launched(
        &mcp,
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": "Write the tests of the export."}),
    ))
    .to_owned();
    let (child, child_launch) = (id_in(&started), launch_in(&started));
    drop(mcp);
    let key = pc.stand_in();

    // Its own change, a pull request, a pass, a report, a field more: none is the review of the
    // session it leads there, and none is served; that review alone is.
    let mut calls: Vec<MachineMessage> = [
        json!({"action": "start"}),
        json!({"action": "start", "pull_request": 7}),
        json!({"action": "start", "agent": session, "pull_request": 7}),
        json!({"action": "start", "agent": session, "x": 1}),
        json!({"action": "start", "agent": "0123456789ab"}),
        json!({"action": "run_pass", "pass": "bugs"}),
        json!({"action": "report", "summary": "Nothing.", "findings": []}),
    ]
    .into_iter()
    .enumerate()
    .map(|(n, args)| review_call(&format!("call-r{n}"), &text, &root, &session, args))
    .collect();
    // Sent over the launch of a child, a session it does not lead.
    calls.push(review_call(
        "call-child",
        &child_launch,
        &root,
        &child,
        json!({"action": "start", "agent": child}),
    ));
    calls.push(review_call(
        "call-ok",
        &text,
        &root,
        &session,
        json!({"action": "start", "agent": session}),
    ));

    let answers = sent_with(&key, &server, calls);

    let (refused, started) = answers.split_at(answers.len() - 1);
    for answer in refused {
        // Each is refused by the guard itself, not by what the review tool would say further on.
        let why = answer.as_ref().unwrap_err();
        assert!(why.contains(LED_ONLY), "{why}");
    }
    let started = started[0].as_ref().unwrap();
    assert_eq!(started["outcome"], "ok", "{started}");
    let all = reviews_of(&server, &session);
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(reviews_of(&server, &child).is_empty());
}

#[test]
fn a_review_with_no_launch_is_not_served_there() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, mcp) = led_with_review(&pc, &server);
    let root = launches(&pc)[0]["caller"].as_str().unwrap().to_owned();
    drop(mcp);
    let key = pc.stand_in();

    // The review of a session of this machine is called as any other session's tool is, by the
    // stand-in's tools, which hold no review.
    let answers = sent_with(
        &key,
        &server,
        vec![review_call(
            "call-empty",
            "",
            &root,
            &session,
            json!({"action": "start", "agent": session}),
        )],
    );

    let why = answers[0].as_ref().unwrap_err();
    assert!(why.contains("is not a tool another machine calls"), "{why}");
    assert!(reviews_of(&server, &session).is_empty());
}

#[test]
fn a_viewer_machine_cannot_start_the_review_of_its_led_session() {
    let (hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, mcp) = led_with_review(&pc, &server);
    hub.make_viewer("pc");
    eventually("pc a viewer on server", || {
        server.seen("pc")["role"] == "viewer"
    });

    let started = mcp.call("review", json!({"action": "start", "agent": session}));

    let text = refused(&started);
    assert!(text.contains("may only view"), "{text}");
    assert!(reviews_of(&server, &session).is_empty());
}

#[test]
fn a_review_while_the_machine_is_off_says_to_call_again_when_it_is_back() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, mcp) = led_with_review(&pc, &server);
    off(&server);
    eventually("server offline", || pc.seen("server")["state"] != "online");

    let started = mcp.call("review", json!({"action": "start", "agent": session}));

    let text = refused(&started);
    // Nothing comes back from a review that was not started: the lead calls again.
    assert!(!text.contains("woken"), "{text}");
    assert!(text.contains("call again once server is back"), "{text}");
    // The session is named as `forward` names it.
    let named = format!("the session \"Finish the export\" (agent {session})");
    assert!(text.contains(&named), "{text}");
    on(&server);
}

/// The deliveries of `kind` stored for `agent` of `machine`: their body.
fn delivered_there(machine: &Machine, agent: &str, kind: &str) -> Vec<String> {
    Store::open_read_only(&machine.user.path("state/flottille/flottille.db"))
        .unwrap()
        .last_deliveries(agent, 64)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == kind)
        .map(|delivery| delivery.body)
        .collect()
}

#[test]
fn a_lead_on_another_machine_is_told_the_user_answered() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    server
        .user
        .call(
            "extension.set",
            json!({"name": "connections", "enabled": true}),
        )
        .unwrap();
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"mcp": {"tool": "vault", "arguments": {"action": "ask", "name": "staging-login",
                "why": "to sign in to staging"}}},
            {"say": "Waiting for the user's answer on the vault entry."}
        ]}]),
    );
    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    prompted(&pc, "a vault entry asked: staging-login");
    let number = server.user.call("vault.asks", json!({})).unwrap()["asks"][0]["ask"]
        .as_u64()
        .unwrap();

    server
        .user
        .call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();

    eventually("the lead there is told", || {
        !delivered_there(&pc, &root, "session-request-answered").is_empty()
    });
    a_while();
    let told = delivered_there(&pc, &root, "session-request-answered");
    assert_eq!(told.len(), 1, "{told:?}");
    for word in [
        session.as_str(),
        &format!("request #{number}"),
        "staging-login",
        "dismissed",
    ] {
        assert!(told[0].contains(word), "{word}: {}", told[0]);
    }
}

/// The kind of the delivery that tells a lead how the user answered a request.
const ANSWERED: &str = "session-request-answered";

/// The deliveries of `kind` stored for `agent` of `machine`: their id and their state.
fn delivered_states(machine: &Machine, agent: &str, kind: &str) -> Vec<(String, String)> {
    Store::open_read_only(&machine.user.path("state/flottille/flottille.db"))
        .unwrap()
        .last_deliveries(agent, 64)
        .unwrap()
        .into_iter()
        .filter(|delivery| delivery.kind == kind)
        .map(|delivery| (delivery.id, delivery.state))
        .collect()
}

/// A vault ask of the entry `name`, as a fake agent plays it.
fn vault_ask(name: &str) -> Value {
    json!({"mcp": {"tool": "vault", "arguments": {"action": "ask", "name": name,
        "why": "to sign in to staging"}}})
}

/// The open vault asks of `machine`.
fn vault_asks(machine: &Machine) -> Vec<Value> {
    machine.user.call("vault.asks", json!({})).unwrap()["asks"]
        .as_array()
        .unwrap()
        .clone()
}

/// Connections on, on `machine`: its led sessions have the tool `vault`.
fn with_vault(machine: &Machine) {
    machine
        .user
        .call(
            "extension.set",
            json!({"name": "connections", "enabled": true}),
        )
        .unwrap();
}

#[test]
fn the_user_s_answer_there_rings_nobody_and_goes_with_the_lead_s_next_wake() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    with_vault(&server);
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            vault_ask("staging-login"), {"say": "Waiting for the vault entry."}]}]),
    );
    let (_, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    prompted(&pc, "a vault entry asked: staging-login");
    eventually("the root at rest", || pc.agent(&root)["state"] == "idle");
    let number = vault_asks(&server)[0]["ask"].as_u64().unwrap();

    server
        .user
        .call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();

    eventually("the lead there is told", || {
        !delivered_states(&pc, &root, ANSWERED).is_empty()
    });
    let (id, _) = delivered_states(&pc, &root, ANSWERED).remove(0);
    // Taken with the wake of the session's next turn (it is told of the answer), never alone.
    eventually("the next wake takes it", || {
        delivered_states(&pc, &root, ANSWERED)[0].1 == "sent"
    });
    let prompts = Store::open_read_only(&pc.user.path("state/flottille/flottille.db"))
        .unwrap()
        .last_agent_events(&root, "agent.prompted", 64)
        .unwrap();
    let with = prompts
        .iter()
        .find(|prompt| {
            prompt.data["deliveries"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|i| i == id.as_str()))
        })
        .unwrap_or_else(|| panic!("no prompt took {id}"));
    let ids = with.data["deliveries"].as_array().unwrap();
    assert!(ids.len() > 1, "played alone: {ids:?}");
}

#[test]
fn two_sessions_led_from_another_machine_that_join_one_ask_each_tell_their_lead_once() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    with_vault(&server);
    script(
        &server,
        json!([
            {"when": "Finish the export.", "times": 1, "do": [
                vault_ask("staging-login"), {"say": "Waiting for the vault entry."}]},
            {"when": "Finish the import.", "times": 1, "do": [
                vault_ask("staging-login"), {"say": "Waiting for it too."}]}
        ]),
    );
    let (export, first, _, _one) = led_on_server(&pc, &server, "Finish the export.", "auto");
    eventually("the first ask", || vault_asks(&server).len() == 1);
    let (import, second, _, _two) = led_on_server(&pc, &server, "Finish the import.", "auto");
    eventually("the second joined it", || {
        vault_asks(&server)[0]["joined"]
            .as_array()
            .is_some_and(|joined| !joined.is_empty())
    });
    let number = vault_asks(&server)[0]["ask"].as_u64().unwrap();

    server
        .user
        .call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();

    for (root, session) in [(&first, &export), (&second, &import)] {
        eventually("each lead there is told", || {
            !delivered_there(&pc, root, ANSWERED).is_empty()
        });
        a_while();
        let told = delivered_there(&pc, root, ANSWERED);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].contains(session.as_str()), "{}", told[0]);
        assert!(
            told[0].contains(&format!("request #{number}")),
            "{}",
            told[0]
        );
    }
}

#[test]
fn the_answers_while_the_lead_s_machine_is_off_wait_there_as_one_message() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    with_vault(&server);
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            vault_ask("staging-login"), vault_ask("billing-login"), vault_ask("mail-login"),
            {"say": "Waiting for the vault entries."}]}]),
    );
    let (_, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    prompted(&pc, "a vault entry asked: mail-login");
    let numbers: Vec<u64> = vault_asks(&server)
        .iter()
        .map(|ask| ask["ask"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers.len(), 3, "{numbers:?}");
    off(&pc);
    eventually("pc offline", || server.seen("pc")["state"] != "online");

    for number in &numbers {
        server
            .user
            .call("vault.ask_dismiss", json!({"ask": number}))
            .unwrap();
    }

    let waiting = || {
        Store::open_read_only(&server.user.path("state/flottille/flottille.db"))
            .unwrap()
            .outgoing(None)
            .unwrap()
            .into_iter()
            .filter(|kept| kept.id.starts_with(ANSWERED))
            .count()
    };
    a_while();
    assert_eq!(waiting(), 1, "one message keeps every answer");
    on(&pc);
    eventually("the lead there is told", || {
        !delivered_there(&pc, &root, ANSWERED).is_empty()
    });
    a_while();
    let told = delivered_there(&pc, &root, ANSWERED);
    assert_eq!(told.len(), 1, "{told:?}");
    for number in &numbers {
        assert!(
            told[0].contains(&format!("request #{number},")),
            "{number}: {}",
            told[0]
        );
    }
}

#[test]
fn a_child_s_request_of_a_session_led_from_another_machine_is_said_and_answered_there() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    with_vault(&server);
    // The project there sees an entry: the child has the tool `vault`.
    server
        .user
        .call(
            "vault.set",
            json!({"name": "deploy-key", "secret": "d3ploy-fake-value", "project": "app"}),
        )
        .unwrap();
    script(
        &server,
        json!([
            {"when": "Ask the vault.", "times": 1, "do": [
                vault_ask("staging-login"), {"say": "Waiting for the user."}]},
            {"when": "Finish the export.", "times": 1, "do": [
                {"mcp": {"tool": "agent_launch", "arguments": {"kind": "child",
                    "title": "Reader", "brief": "Ask the vault."}}},
                {"say": "Started a reader."}]},
            {"when": "Go on.", "do": [{"say": "Going on."}]}
        ]),
    );
    let (session, root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    eventually("the child's ask", || vault_asks(&server).len() == 1);
    let ask = vault_asks(&server).remove(0);
    let (number, child) = (
        ask["ask"].as_u64().unwrap(),
        ask["agent"].as_str().unwrap().to_owned(),
    );
    assert_ne!(child, session);
    eventually("the session at rest", || {
        server.agent(&session)["state"] == "idle"
    });

    server
        .user
        .call("agent.send", json!({"agent": session, "message": "Go on."}))
        .unwrap();

    // The prompts are read as JSON: the child is found by its id, its title's quotes escaped.
    prompted(
        &pc,
        &format!("(agent {child}): a vault entry asked: staging-login"),
    );
    server
        .user
        .call("vault.ask_dismiss", json!({"ask": number}))
        .unwrap();
    eventually("the lead there is told", || {
        !delivered_there(&pc, &root, ANSWERED).is_empty()
    });
    let told = delivered_there(&pc, &root, ANSWERED).remove(0);
    for word in [
        &format!("vault request #{number} of its child \"Reader\""),
        "dismissed it",
    ] {
        assert!(told.contains(word), "{word}: {told}");
    }
}
