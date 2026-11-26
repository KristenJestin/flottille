//! What an agent launched on another machine closes and is tidied away as a local one (#1520,
//! `docs/remote.md`, "A row of a session on another machine"): the user's close goes to the
//! session's machine and runs there through the local path, its launch leaving this machine's
//! rows; a child that reported is an ended child, forgotten with its parent's tree; the lead closes
//! what it launched there without asking. A hub served in the test on 127.0.0.1 and two daemons,
//! each with its own temporary HOME and XDG folders and fake agents; throwaway repositories.

use crate::remote::{NoKeyring, signed};
use crate::remote_deliveries::{child_on_server, script};
use crate::remote_launch::{
    Machine, allowing, done, eventually, pc_and_server, pc_and_server_with, prompted, server_user,
};
use crate::remote_led::led_on_server;

use flottille_link::DeviceKey;
use serde_json::{Value, json};

/// The launches `pc` shows under their pilot (`remote.launches`).
fn launches(pc: &Machine) -> Vec<Value> {
    pc.user.call("remote.launches", json!({})).unwrap()["launches"]
        .as_array()
        .unwrap()
        .clone()
}

fn shown(pc: &Machine, agent: &str) -> bool {
    launches(pc).iter().any(|launch| launch["agent"] == agent)
}

/// The user's close of `agent` of `machine`, signed by `pc`'s device as the window signs it.
fn close_of(pc: &Machine, machine: &str, agent: &str) -> Value {
    let device = DeviceKey::load(&NoKeyring, &pc.user.path("state/flottille/remote"))
        .unwrap()
        .expect("pc's device key");
    let line = signed(
        &device,
        &format!("close-{agent}"),
        machine,
        "agent.stop",
        json!({"agent": agent}),
    );
    serde_json::from_str(&line).unwrap()
}

#[test]
fn a_session_launched_on_another_machine_closes_from_this_machine_as_a_local_one() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    let (session, _root, _, _mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    eventually("its turn ended there", || {
        server.agent(&session)["state"] == "idle"
    });
    assert!(shown(&pc, &session), "{:#?}", launches(&pc));

    // The window's close: the user's agent.stop, signed for server, carried by pc's daemon.
    pc.user
        .call(
            "remote.close",
            json!({"request": close_of(&pc, "server", &session)}),
        )
        .unwrap();

    eventually("the session ended on server", || {
        server.agent(&session)["state"] == "ended"
    });
    assert_eq!(
        server.agent(&session)["state_reason"],
        "stopped by the user",
        "{:#}",
        server.agent(&session)
    );
    // Its launch leaves pc's rows, as an ended local session leaves its pilot's.
    eventually("its launch leaves pc's rows", || !shown(&pc, &session));
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

#[test]
fn a_child_launched_on_another_machine_that_reported_goes_into_ended() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "The export tests are written."}}}
        ]}]),
    );
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&pc, "The export tests are written.");

    // An ended child, as one here: ended, its report the last word, kept beside its parent.
    eventually("its report is what was last heard", || {
        launches(&pc)
            .iter()
            .any(|launch| launch["agent"] == child.as_str() && launch["heard"] == "child-report")
    });
    let reported = launches(&pc)
        .into_iter()
        .find(|launch| launch["agent"] == child.as_str())
        .unwrap();
    assert_eq!(reported["state"], "ended", "{reported:#}");
    assert_eq!(server.agent(&child)["state"], "ended");
}

#[test]
fn an_ended_child_of_another_machine_can_be_forgotten() {
    let (_hub, pc, server) = pc_and_server(server_user(allowing()), Some(""));
    script(
        &server,
        json!([{"when": "Write the tests of the export.", "times": 1, "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "The export tests are written."}}}
        ]}]),
    );
    let (child, _launch, _mcp) = child_on_server(&pc, &server, "Write the tests of the export.");
    prompted(&pc, "The export tests are written.");
    eventually("its report is what was last heard", || {
        launches(&pc)
            .iter()
            .any(|launch| launch["agent"] == child.as_str() && launch["heard"] == "child-report")
    });
    let caller = launches(&pc)[0]["caller"].as_str().unwrap().to_owned();

    // Forgotten as an ended local child is: with its parent's tree.
    pc.user
        .call("agent.stop", json!({"agent": caller}))
        .unwrap();

    eventually("the ended child is forgotten here", || !shown(&pc, &child));
    // Its own machine had ended it already: nothing more is asked there.
    assert_eq!(server.agent(&child)["state"], "ended");
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
}

#[test]
fn a_lead_closes_what_it_launched_on_another_machine_without_asking() {
    // pc's judge allows, as server's: in Auto, a lead ends its own sessions without the user.
    let pc_user = server_user(allowing());
    let (_hub, pc, server) =
        pc_and_server_with("runner", pc_user, server_user(allowing()), Some(""));
    let (session, _root, _, mcp) = led_on_server(&pc, &server, "Finish the export.", "auto");
    let answer = mcp.call(
        "agent_launch",
        json!({"kind": "child", "machine": "server", "title": "Write the tests",
            "brief": "Write the tests of the export."}),
    );
    let child = crate::remote_launch::id_in(done(&answer));
    prompted(&server, "Write the tests of the export.");
    eventually("the session's turn ended there", || {
        server.agent(&session)["state"] == "idle"
    });

    let ended = mcp.call("agent_stop", json!({"agent_id": session}));
    let stopped = mcp.call("agent_stop", json!({"agent_id": child}));

    assert!(done(&ended).contains("Ended the session"), "{}", ended.text);
    assert!(done(&stopped).contains("Stopped"), "{}", stopped.text);
    eventually("both ended on server", || {
        server.agent(&session)["state"] == "ended" && server.agent(&child)["state"] == "ended"
    });
    // The user was asked nothing, on either machine.
    assert!(pc.requests().is_empty(), "{:#?}", pc.requests());
    assert!(server.requests().is_empty(), "{:#?}", server.requests());
    eventually("their launches leave pc's rows", || {
        !shown(&pc, &session) && !shown(&pc, &child)
    });
}
