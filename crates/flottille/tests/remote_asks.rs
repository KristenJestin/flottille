//! Where a question goes when the parent is off (`docs/remote.md`): a led session's ask, and a
//! child's ask and requests held by the gate, go to the agent that started them on another
//! machine while that machine is linked, and to the user of the machine they run on while it is
//! not; no agent allows a request, from any machine. A hub served in the test on 127.0.0.1 and
//! two daemons, each with its own temporary HOME and XDG folders and fake agents; a machine
//! "off" is its daemon stopped, then started again on the same folders.

use crate::common;
use crate::remote_deliveries::{answered, child_on_server, off, on, prompts_with, release, script};
use crate::remote_launch::{
    Machine, a_while, allowing, done, eventually, id_in, launched, pc_and_server, prompted,
    refused, restarted, sent_with, server_user, titled,
};

use common::mcp::Mcp;
use flottille_link::{Call, MachineMessage};
use serde_json::{Value, json};

const QUESTION: &str = "Which format first, CSV or JSON?";

/// A session led by `pc`'s root, started on `server` with `brief`: its id there, and the root's
/// MCP session.
/// The root is in `mode`: in Full, its `session_send` is let through without a judge on pc.
fn led_on_server(pc: &Machine, server: &Machine, brief: &str, mode: &str) -> (String, Mcp) {
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root(mode);
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export", "brief": brief}),
    );
    let session = id_in(done(&answer));
    prompted(server, brief);
    (session, mcp)
}

/// The questions in `machine`'s inbox, each entry whole.
fn questions(machine: &Machine) -> Vec<Value> {
    machine.user.call("inbox.list", json!({})).unwrap()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == "question")
        .cloned()
        .collect()
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

/// Waits until `machine` sees `name` offline.
fn offline(machine: &Machine, name: &str) {
    eventually(&format!("{name} offline"), || {
        machine.seen(name)["state"] != "online"
    });
}

/// The first of `pc`'s prompts holding `text`.
fn prompt_with(machine: &Machine, text: &str) -> String {
    prompted(machine, text);
    machine
        .prompts()
        .into_iter()
        .find(|prompt| prompt.contains(text))
        .unwrap()
}

/// The request of `machine` numbered `number`, as `approval.list` gives the user's.
fn users_request(machine: &Machine, number: u64) -> Option<Value> {
    machine
        .requests()
        .into_iter()
        .find(|request| request["number"] == number)
}

/// The numbers of the requests `machine`'s agents were told of as `child-asks`.
fn asked_of(machine: &Machine) -> Vec<u64> {
    let mut numbers: Vec<u64> = machine
        .prompts()
        .iter()
        .filter(|prompt| prompt.contains("[flottille:child-asks]"))
        .flat_map(|prompt| {
            prompt
                .match_indices("Request #")
                .map(|(at, _)| number_in(&prompt[at..].replacen("Request", "request", 1)))
                .collect::<Vec<_>>()
        })
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

/// The number of the request a tool answer of a child names (`request #n`).
fn number_in(text: &str) -> u64 {
    let at = text.find("request #").unwrap() + "request #".len();
    text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

#[test]
fn a_led_session_asks_the_user_when_its_pilot_s_machine_is_off() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "ask", "arguments": {"question": QUESTION}}}
        ]}]),
    );
    let (session, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    off(&pc);
    offline(&server, "pc");

    release(&server);

    let said = tool_answer(&server, "given to the user");
    assert!(said.contains("pc is offline"), "{said}");
    eventually("the question in server's inbox", || {
        !questions(&server).is_empty()
    });
    let question = questions(&server)[0].clone();
    assert_eq!(question["agent"], session.as_str());
    assert!(
        question["text"].as_str().unwrap().contains(QUESTION),
        "{question}"
    );
    // The user answers it there, as from any client of that machine.
    server
        .user
        .call(
            "agent.answer",
            json!({"agent": session, "question": question["seq"],
                "answers": [{"text": "JSON first."}]}),
        )
        .unwrap();
    prompted(&server, "JSON first.");
}

#[test]
fn while_the_pilot_s_machine_is_linked_the_ask_goes_to_the_pilot() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"mcp": {"tool": "ask", "arguments": {"question": QUESTION}}}
        ]}]),
    );

    let (session, mcp) = led_on_server(&pc, &server, "Finish the export.", "full");

    let said = tool_answer(&server, "Your question went to agent");
    assert!(said.contains("leads this session"), "{said}");
    let woken = prompt_with(&pc, QUESTION);
    assert!(woken.contains("it asked you a question"), "{woken}");
    assert!(woken.contains("session_send"), "{woken}");
    assert!(questions(&server).is_empty(), "{:#?}", questions(&server));
    eventually("the session waiting on its pilot", || {
        server
            .user
            .call("agent.tree", json!({"agent": session}))
            .unwrap()["agents"][0]["waiting_on"]
            == "pilot"
    });

    let answer = mcp.call(
        "session_send",
        json!({"session": session, "text": "CSV first, JSON next."}),
    );

    done(&answer);
    prompted(&server, "CSV first, JSON next.");
    eventually("the question answered", || {
        server.agent(&session)["waiting_on_answer"] == false
    });
    assert!(questions(&server).is_empty(), "{:#?}", questions(&server));
}

#[test]
fn questions_open_to_the_pilot_go_to_the_user_when_its_machine_goes_off() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"mcp": {"tool": "ask", "arguments": {"question": QUESTION}}}
        ]}]),
    );
    let (session, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    tool_answer(&server, "Your question went to agent");
    prompted(&pc, QUESTION);
    assert!(questions(&server).is_empty(), "{:#?}", questions(&server));

    off(&pc);

    eventually("the question in server's inbox", || {
        !questions(&server).is_empty()
    });
    let question = questions(&server)[0].clone();
    assert_eq!(question["agent"], session.as_str());
    assert!(
        question["text"].as_str().unwrap().contains(QUESTION),
        "{question}"
    );
    let tree = server
        .user
        .call("agent.tree", json!({"agent": session}))
        .unwrap();
    assert_eq!(tree["agents"][0]["waiting_on"], "user", "{tree:#}");
}

#[test]
fn the_pilot_back_is_told_what_was_asked_and_answered() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "ask", "arguments": {"question": QUESTION}}}
        ]}]),
    );
    let (session, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    off(&pc);
    offline(&server, "pc");
    release(&server);
    tool_answer(&server, "given to the user");
    eventually("the question in server's inbox", || {
        !questions(&server).is_empty()
    });
    let question = questions(&server)[0]["seq"].clone();
    server
        .user
        .call(
            "agent.answer",
            json!({"agent": session, "question": question,
                "answers": [{"text": "JSON first, the users ask for it."}]}),
        )
        .unwrap();
    prompted(&server, "JSON first, the users ask for it.");

    on(&pc);
    pc.wait_for("server");

    let told = prompt_with(&pc, "JSON first, the users ask for it.");
    assert!(told.contains("[flottille:session-meanwhile]"), "{told}");
    assert!(told.contains(QUESTION), "{told}");
    assert!(told.contains(&session), "{told}");
    // Once, even when both machines start again and link anew.
    restarted(&pc);
    restarted(&server);
    pc.wait_for("server");
    server.wait_for("pc");
    a_while();
    assert_eq!(prompts_with(&pc, "[flottille:session-meanwhile]"), 1);
}

#[test]
fn a_child_s_held_request_goes_to_the_user_when_its_parent_s_machine_is_off() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}},
            {"hold": "again"},
            {"mcp": {"tool": "fs_read", "arguments": {"path": ".env.local"}}}
        ]}]),
    );
    let (_child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    // While pc is linked, its parent there holds it, and the user is asked nothing.
    let first = number_in(&tool_answer(&server, "your parent decides"));
    let asks = prompt_with(&pc, "[flottille:child-asks]");
    assert!(asks.contains(&format!("#{first}")), "{asks}");
    assert!(users_request(&server, first).is_none());

    off(&pc);

    eventually("the request held by the user", || {
        users_request(&server, first).is_some()
    });
    let request = users_request(&server, first).unwrap();
    assert_eq!(
        request["forwards"][0]["why"], "its parent's machine pc is offline",
        "{request:#}"
    );
    // One held while pc is off goes to the user at once, the same way.
    std::fs::write(server.user.path("fake/again"), "").unwrap();
    eventually("a second request held by the user", || {
        server
            .requests()
            .iter()
            .any(|request| request["number"] != first)
    });
    let request = server
        .requests()
        .into_iter()
        .find(|request| request["number"] != first)
        .unwrap();
    let said = tool_answer(&server, "the user's approval");
    assert!(said.contains(&format!("#{}", request["number"])), "{said}");
    assert_eq!(
        request["forwards"][0]["why"], "its parent's machine pc is offline",
        "{request:#}"
    );
}

#[test]
fn no_agent_allows_a_request_across_machines() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}},
            {"hold": "again"},
            {"mcp": {"tool": "fs_read", "arguments": {"path": ".env.local"}}}
        ]}]),
    );
    let (child, launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    let first = number_in(&tool_answer(&server, "your parent decides"));
    let asks = prompt_with(&pc, &format!("Request #{first}"));
    assert!(asks.contains(r#"\"machine\": \"server\""#), "{asks}");

    // The parent cannot allow it; it forwards it, and the user of server holds it.
    let allow = mcp.call(
        "approval_answer",
        json!({"request_id": first, "machine": "server", "decision": "allow", "why": "fine"}),
    );
    assert!(refused(&allow).contains("forward"), "{}", allow.text);
    let forward = mcp.call(
        "approval_answer",
        json!({"request_id": first, "machine": "server", "decision": "forward",
            "why": "the tests need the database name"}),
    );
    done(&forward);
    eventually("the request held by the user", || {
        users_request(&server, first).is_some()
    });
    let request = users_request(&server, first).unwrap();
    assert_eq!(
        request["forwards"][0]["why"], "the tests need the database name",
        "{request:#}"
    );

    // Nor can a machine, for one its agent holds.
    std::fs::write(server.user.path("fake/again"), "").unwrap();
    eventually("a second request told to pc", || asked_of(&pc).len() > 1);
    let second = asked_of(&pc)
        .into_iter()
        .find(|number| *number != first)
        .unwrap();
    let parent = pc
        .agents()
        .into_iter()
        .find(|agent| agent["parent"].is_null())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let key = pc.stand_in();
    let call = |id: &str, decision: &str| {
        MachineMessage::Call(Box::new(Call {
            id: id.into(),
            launch: launch.clone(),
            caller: parent.clone(),
            agent: child.clone(),
            tool: "approval_answer".into(),
            arguments: json!({"request_id": second, "decision": decision, "why": "fine"})
                .to_string(),
        }))
    };
    let answers = sent_with(
        &key,
        &server,
        vec![call("c1", "allow"), call("c2", "approve")],
    );
    for answer in &answers {
        let refused = answer.clone().unwrap_err();
        assert!(refused.contains("forward"), "{refused}");
    }
    a_while();
    assert_eq!(prompts_with(&server, "approved"), 0);
    // Still not allowed: with pc gone, it went to the user, who decides.
    eventually("the request held by the user", || {
        users_request(&server, second).is_some()
    });
    assert_eq!(users_request(&server, second).unwrap()["state"], "pending");
}

/// A second child of `pc`'s root on `server`, titled `title`, started with `brief`: its id there.
fn another_child(pc_root: &Mcp, server: &Machine, title: &str, brief: &str) -> String {
    let answer = launched(
        pc_root,
        json!({"kind": "child", "machine": "server", "title": title, "brief": brief}),
    );
    let text = done(&answer).to_owned();
    let child = titled(server, title);
    assert_eq!(id_in(&text), child["id"].as_str().unwrap(), "{text}");
    child["id"].as_str().unwrap().to_owned()
}

#[test]
fn a_parent_with_two_children_on_one_machine_answers_the_second_s_request() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the docs of the export.", "times": 1, "do": [
            {"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}}
        ]}]),
    );
    // The first child asks nothing; the second, started after it, is held.
    let (_first, _launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    another_child(
        &mcp,
        &server,
        "Write the docs",
        "Write the docs of the export.",
    );
    let number = number_in(&tool_answer(&server, "your parent decides"));
    prompt_with(&pc, &format!("Request #{number}"));

    let forward = mcp.call(
        "approval_answer",
        json!({"request_id": number, "machine": "server", "decision": "forward",
            "why": "the docs need the database name"}),
    );

    done(&forward);
    eventually("the request held by the user", || {
        users_request(&server, number).is_some()
    });
    assert_eq!(
        users_request(&server, number).unwrap()["forwards"][0]["why"],
        "the docs need the database name"
    );
}

#[test]
fn an_approval_answer_aimed_at_another_tree_or_a_local_request_is_refused() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([
            {"when": "Write the docs of the export.", "times": 1, "do": [
                {"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}}
            ]},
            {"when": "hello", "times": 1, "do": [
                {"mcp": {"tool": "fs_read", "arguments": {"path": ".env.local"}}}
            ]}
        ]),
    );
    // The first root's child on server, which asks nothing.
    let (_child, _launch, mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    // Another root's child there, whose request that root holds.
    let (_other, other) = pc.root("auto");
    another_child(
        &other,
        &server,
        "Write the docs",
        "Write the docs of the export.",
    );
    let theirs = number_in(&tool_answer(&server, "your parent decides"));
    prompt_with(&pc, &format!("Request #{theirs}"));
    // A session of server's own, whose request its user holds.
    server.root("auto");
    eventually("server's own request", || !server.requests().is_empty());
    let local = server.requests()[0]["number"].as_u64().unwrap();

    for number in [theirs, local] {
        let answer = mcp.call(
            "approval_answer",
            json!({"request_id": number, "machine": "server", "decision": "deny",
                "why": "not needed"}),
        );
        assert!(refused(&answer).contains("not held by"), "{}", answer.text);
    }
    a_while();
    assert!(users_request(&server, theirs).is_none());
    assert_eq!(users_request(&server, local).unwrap()["state"], "pending");
}
