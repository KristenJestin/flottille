//! The light copy of each machine's list, and the close kept for a machine that is off (#1150,
//! `docs/remote.md`, "One list of every machine"): a runner tells the machines it links to the
//! sessions it holds (title, project, state, last exchange, never the conversation), each keeps
//! it in its store, and a close the user asks of a session of a machine that is off is stored
//! before it is sent, then sent once when that machine links again. A hub served in the test on
//! 127.0.0.1 and two daemons, each with its own temporary HOME and XDG folders and fake agents; a
//! machine "off" is its daemon stopped, then started again on the same folders; throwaway
//! repositories.

use crate::common;
use crate::remote::{Machine, eventually, pc_and_server_on, signed};

use common::User;
use serde_json::{Value, json};

fn add_app(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// A session the user starts in `app` on `machine`, its first turn ended: its id.
fn session(machine: &Machine, message: &str) -> String {
    let started = machine
        .user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    let id = started["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || state(machine, &id) == "idle");
    id
}

fn state(machine: &Machine, agent: &str) -> String {
    machine
        .user
        .call("agent.show", json!({"agent": agent}))
        .map(|shown| {
            shown["agent"]["state"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

/// The sessions `machine` keeps of the machine `name`, as `remote.sessions` lists them.
fn kept_of(machine: &Machine, name: &str) -> Vec<Value> {
    let lists = machine.user.call("remote.sessions", json!({})).unwrap();
    lists["machines"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|list| list["name"] == name)
        .and_then(|list| list["sessions"].as_array().cloned())
        .unwrap_or_default()
}

fn kept_ids(machine: &Machine, name: &str) -> Vec<String> {
    kept_of(machine, name)
        .iter()
        .filter_map(|session| session["id"].as_str().map(str::to_owned))
        .collect()
}

fn off(machine: &Machine) {
    let stopped = machine.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", machine.log());
}

fn on(machine: &Machine) {
    machine.user.status();
}

/// A hub, `pc` its first member and `server` a runner, linked, each with the project `app`.
fn pc_and_server() -> (crate::remote::TestHub, Machine, Machine) {
    let (hub, pc, server) = pc_and_server_on(
        Machine::of(User::with_fake_agent()),
        User::with_fake_agent(),
    );
    add_app(&pc.user);
    add_app(&server.user);
    (hub, pc, server)
}

/// The close of `agent` on `machine` as this machine's client signs it.
fn close_of(pc: &Machine, machine: &str, agent: &str) -> Value {
    let line = signed(
        &pc.device(),
        "close-1",
        machine,
        "agent.stop",
        json!({"agent": agent}),
    );
    serde_json::from_str(&line).unwrap()
}

#[test]
fn a_runner_s_sessions_are_kept_by_the_other_machine_without_their_conversation() {
    let (_hub, pc, server) = pc_and_server();
    let agent = session(&server, "Ship the server part");

    eventually("pc keeps server's session", || {
        kept_ids(&pc, "server").contains(&agent)
    });
    let kept = kept_of(&pc, "server");
    let row = kept.iter().find(|row| row["id"] == agent.as_str()).unwrap();
    assert_eq!(row["project"], "app");
    assert_eq!(row["title"], "Ship the server part");
    assert!(row["exchanged_at"].as_u64().is_some(), "{row}");
    assert!(row["state"].is_string(), "{row}");
    // Light: what the row says, never the conversation.
    let mut keys: Vec<&str> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "created_at",
            "exchanged_at",
            "id",
            "pilot",
            "project",
            "state",
            "title"
        ],
        "{row}"
    );
    // This machine's own list is said too, for a client that reads the others'.
    let mine = session(&pc, "Fix the pc part");
    eventually("pc says its own list", || {
        pc.user.call("remote.sessions", json!({})).unwrap()["this"]
            .as_array()
            .is_some_and(|own| own.iter().any(|row| row["id"] == mine.as_str()))
    });
}

#[test]
fn the_copy_of_a_machine_that_is_off_stays_across_a_restart() {
    let (_hub, pc, server) = pc_and_server();
    let agent = session(&server, "Ship the server part");
    eventually("pc keeps server's session", || {
        kept_ids(&pc, "server").contains(&agent)
    });

    off(&server);
    pc.wait_for("server", "offline");
    off(&pc);
    on(&pc);

    assert!(kept_ids(&pc, "server").contains(&agent), "{}", pc.log());
}

#[test]
fn closing_a_session_of_a_machine_that_is_off_is_sent_when_it_comes_back() {
    let (_hub, pc, server) = pc_and_server();
    let agent = session(&server, "Ship the server part");
    eventually("pc keeps server's session", || {
        kept_ids(&pc, "server").contains(&agent)
    });
    off(&server);
    pc.wait_for("server", "offline");

    let request = close_of(&pc, "server", &agent);
    pc.user
        .call("remote.close", json!({"request": request}))
        .unwrap();
    // Kept once, however often it is asked.
    pc.user
        .call("remote.close", json!({"request": request}))
        .unwrap();

    // It leaves this machine's copy at once.
    assert!(!kept_ids(&pc, "server").contains(&agent));
    on(&server);
    pc.wait_for("server", "online");
    eventually("the session ended on server", || {
        state(&server, &agent) == "ended"
    });
    assert!(
        server.log().contains("agent.stop asked by pc"),
        "{}",
        server.log()
    );
    // Its list, told again, holds it no more.
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(!kept_ids(&pc, "server").contains(&agent));
}

#[test]
fn a_close_for_another_machine_or_of_another_method_is_refused() {
    let (_hub, pc, server) = pc_and_server();
    let agent = session(&server, "Ship the server part");

    let for_pc = close_of(&pc, "pc", &agent);
    let refused = pc
        .user
        .call("remote.close", json!({"request": for_pc}))
        .unwrap_err();
    assert!(refused.1.contains("another machine"), "{refused:?}");

    let line = signed(
        &pc.device(),
        "send-1",
        "server",
        "agent.send",
        json!({"agent": agent, "message": "hi"}),
    );
    let send: Value = serde_json::from_str(&line).unwrap();
    let refused = pc
        .user
        .call("remote.close", json!({"request": send}))
        .unwrap_err();
    assert!(refused.1.contains("agent.stop"), "{refused:?}");
    assert_eq!(state(&server, &agent), "idle");
}
