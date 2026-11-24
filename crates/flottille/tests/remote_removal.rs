//! The workspace of a session or child started on another machine (`docs/remote.md`, "Coming
//! back"): its commits come back when it ends, and the agent that started it removes it as it
//! removes one of this machine, judged there by the same checks. A hub served in the test on
//! 127.0.0.1 and two daemons, each with its own temporary HOME and XDG folders and fake agents;
//! throwaway repositories.

use crate::common::mcp::Answer;
use crate::remote_deliveries::{launch_in, release, script};
use crate::remote_launch::{
    Machine, allowing, done, eventually, folder_of, id_in, launched, pc_and_server, prompted,
    refused, server_user,
};

use serde_json::{Value, json};

/// A session led by a root of `pc` in Auto, started on `server` with `brief`: its id there, the
/// launch's name and the root's MCP session.
fn led_on_server(
    pc: &Machine,
    server: &Machine,
    brief: &str,
) -> (String, String, crate::common::mcp::Mcp) {
    pc.wait_for_offer("server", 12);
    let (_root, mcp) = pc.root("auto");
    let answer = launched(
        &mcp,
        json!({"kind": "led", "machine": "server", "title": "Finish the export", "brief": brief}),
    );
    let text = done(&answer).to_owned();
    prompted(server, brief);
    (id_in(&text), launch_in(&text), mcp)
}

/// Everything in `dir`, committed as `message`.
fn commit_all(machine: &Machine, dir: &std::path::Path, file: &str, message: &str) {
    std::fs::write(dir.join(file), format!("{message}\n")).unwrap();
    machine.git(dir, &["add", "--all"]);
    machine.git(dir, &["commit", "--quiet", "-m", message]);
}

/// The subject of the last commit of `reference` in `pc`'s repository.
fn last_of(pc: &Machine, reference: &str) -> String {
    pc.git(&pc.folder(), &["log", "-1", "--format=%s", reference])
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

/// The number of the request a held answer names.
fn held(answer: &Answer) -> u64 {
    let text = done(answer);
    let at = text
        .find("request #")
        .unwrap_or_else(|| panic!("not held: {text}"))
        + "request #".len();
    text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

/// The pending request of `machine`'s user numbered `number`.
fn request(machine: &Machine, number: u64) -> Value {
    machine
        .requests()
        .into_iter()
        .find(|request| request["number"] == number)
        .unwrap_or_else(|| panic!("no request #{number}: {:#?}", machine.requests()))
}

/// The session `session` of `server` ended by its user, once it has.
fn ended_there(server: &Machine, session: &str) {
    server
        .user
        .call("agent.stop", json!({"agent": session}))
        .unwrap();
    eventually("the session ended", || {
        server.agent(session)["state"] == "ended"
    });
}

#[test]
fn the_pilot_removes_the_workspace_of_its_led_session_on_another_machine() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, launch, mcp) = led_on_server(&pc, &server, "Finish the export.");
    let agent = server.agent(&session);
    let there = folder_of(&agent);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    // Its work, and the caller's uncommitted work laid there, committed in its place there.
    commit_all(&server, &there, "export.txt", "the export");
    ended_there(&server, &session);

    let answer = mcp.call(
        "workspace",
        json!({"action": "remove", "name": workspace, "machine": "server"}),
    );

    // Held for the user of the machine it is on, as a removal is on one machine; its commits went
    // back first, so nothing would be lost.
    let number = held(&answer);
    let asked = request(&server, number);
    assert!(
        asked["says"].to_string().contains("Nothing would be lost"),
        "{asked:#}"
    );
    let reference = format!("refs/flottille/machines/server/{launch}/feat/export");
    assert_eq!(last_of(&pc, &reference), "the export");
    assert!(there.exists());
    server
        .user
        .call("approval.approve", json!({"number": number}))
        .unwrap();
    eventually("the workspace removed there", || !there.exists());
    let approved = prompt_with(&pc, &format!("Request #{number} on server"));
    assert!(approved.contains("was approved by the user"), "{approved}");
    assert!(approved.contains("is removed"), "{approved}");
}

#[test]
fn commits_made_after_the_last_report_come_back_when_the_session_ends() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Finish the export.", "times": 1, "do": [
            {"hold": "release"},
            {"mcp": {"tool": "report", "arguments": {"text": "The export is reported."}}}
        ]}]),
    );
    let (session, launch, _mcp) = led_on_server(&pc, &server, "Finish the export.");
    let there = folder_of(&server.agent(&session));
    commit_all(&server, &there, "export.txt", "the export");
    release(&server);
    // Its pilot is woken by its report, the commits it made so far come back with it.
    prompted(&pc, "it reported");
    let reference = format!("refs/flottille/machines/server/{launch}/feat/export");
    assert_eq!(last_of(&pc, &reference), "the export");
    // Work committed after its report, which no report brings back.
    commit_all(&server, &there, "late.txt", "the late fix");

    ended_there(&server, &session);

    let woken = prompt_with(&pc, "Your following of it is over.");
    assert!(woken.contains(&reference), "{woken}");
    assert_eq!(last_of(&pc, &reference), "the late fix");
}

#[test]
fn a_workspace_on_another_machine_is_removed_only_by_the_agent_that_started_its_session() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, _launch, _mcp) = led_on_server(&pc, &server, "Finish the export.");
    let agent = server.agent(&session);
    let workspace = agent["workspace"].as_str().unwrap().to_owned();
    ended_there(&server, &session);
    let (_other, other) = pc.root("auto");

    let answer = other.call(
        "workspace",
        json!({"action": "remove", "name": workspace, "machine": "server"}),
    );

    let text = refused(&answer);
    assert!(text.contains("started no session"), "{text}");
    assert!(folder_of(&agent).exists());
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}
