//! The local ghost of each session of another machine (#1527, `docs/remote.md`, "Machines are
//! one", point 4): each machine keeps, in its store, the conversation, the tree and the waits of
//! every session another machine lists, as that machine shows them, kept current from its events
//! and caught up after a gap from where it stopped. The ghost only reflects: the session's machine
//! alone decides. A hub served in the test on 127.0.0.1 and two daemons, each with its own
//! temporary HOME and XDG folders and fake agents; a machine "off" is its daemon stopped, then
//! started again on the same folders; throwaway repositories.

use crate::common;
use crate::remote::{Machine, eventually, pc_and_server_on};

use common::User;
use serde_json::{Value, json};

/// What the server's agent says to a first message.
const REPLY: &str = "The notes are ready.";

fn script() -> Value {
    json!({"turns": [
        {"when": "LOOK", "do": [
            {"mcp": {"tool": "fs_list", "arguments": {}}},
            {"say": "I looked at the files."}
        ]},
        {"when": "ASK", "do": [
            {"mcp": {"tool": "ask", "arguments": {"question": "Which branch ships?"}}}
        ]},
        {"when": "", "do": [{"say": REPLY}]}
    ]})
}

fn add_app(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// A hub, `pc` its first member and `server` a runner, linked, each with the project `app`.
fn pc_and_server() -> (crate::remote::TestHub, Machine, Machine) {
    let server = User::with_fake_agent();
    server.fake_script(script());
    let (hub, pc, server) = pc_and_server_on(Machine::of(User::with_fake_agent()), server);
    add_app(&pc.user);
    add_app(&server.user);
    (hub, pc, server)
}

/// A session the user starts in `app` on `machine`, which answered: its id.
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

fn send(machine: &Machine, agent: &str, message: &str) {
    machine
        .user
        .call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
}

/// What `pc` keeps of the server's session `agent`, as `remote.ghost` says it.
fn ghost(pc: &Machine, agent: &str) -> Result<Value, (String, String)> {
    pc.user.call(
        "remote.ghost",
        json!({"machine": "server", "session": agent}),
    )
}

/// Whether the ghost of `agent` holds `text` in its conversation.
fn ghost_holds(pc: &Machine, agent: &str, text: &str) -> bool {
    ghost(pc, agent).is_ok_and(|kept| kept["events"].to_string().contains(text))
}

/// The ghost's tree entry of its root, as `agent.tree` gives it there.
fn ghost_root(pc: &Machine, agent: &str) -> Value {
    ghost(pc, agent)
        .ok()
        .and_then(|kept| {
            kept["view"]["tree"]
                .as_array()
                .and_then(|tree| tree.iter().find(|entry| entry["agent"]["id"] == agent))
                .cloned()
        })
        .unwrap_or_default()
}

/// Whether pc's ghost of `agent` holds the same events as the server shows of it.
fn in_step(pc: &Machine, server: &Machine, agent: &str) -> bool {
    let there = server
        .user
        .call("agent.show", json!({"agent": agent, "after": 0}))
        .unwrap();
    ghost(pc, agent).is_ok_and(|kept| {
        kept["events"] == there["events"]
            && kept["last"] == there["events"].as_array().unwrap().last().unwrap()["seq"]
    })
}

/// Waits until the server's own conversation of `agent` holds `text`, saying what it holds when
/// it never does.
fn holds(server: &Machine, agent: &str, text: &str) {
    let shown = || {
        server
            .user
            .call("agent.show", json!({"agent": agent, "after": 0}))
            .map(|shown| shown["events"].to_string())
            .unwrap_or_default()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !shown().contains(text) {
        assert!(
            std::time::Instant::now() < deadline,
            "the server's session never said {text:?}: {}",
            shown()
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn off(machine: &Machine) {
    let stopped = machine.user.run(&["daemon", "stop"]);
    assert!(stopped.status.success(), "{}", machine.log());
}

#[test]
fn the_ghost_follows_the_session_as_it_runs() {
    let (_hub, pc, server) = pc_and_server();
    let agent = session(&server, "Write the release notes");
    eventually("pc's ghost holds the first reply", || {
        ghost_holds(&pc, &agent, REPLY)
    });
    // Each event as the server stores it, its own numbers: the same conversation.
    eventually("the ghost holds the server's events", || {
        in_step(&pc, &server, &agent)
    });

    // A new message, its tool call and its reply come as the session runs.
    send(&server, &agent, "LOOK at the files");
    holds(&server, &agent, "I looked at the files.");
    eventually(
        "the ghost holds the message, its tool call and its reply",
        || in_step(&pc, &server, &agent),
    );
    let kept = ghost(&pc, &agent).unwrap()["events"].to_string();
    for said in ["LOOK at the files", "fs_list", "I looked at the files."] {
        assert!(kept.contains(said), "{said}: {kept}");
    }

    // A question to the user: the session waits, as its tree says it there.
    send(&server, &agent, "ASK which branch");
    eventually("the ghost says the session waits for the user", || {
        let root = ghost_root(&pc, &agent);
        root["needs_user"] == true && root.to_string().contains("Which branch ships?")
    });
    let there = server
        .user
        .call("agent.tree", json!({"agent": agent}))
        .unwrap();
    eventually("the ghost's tree is the server's", || {
        ghost(&pc, &agent).is_ok_and(|kept| kept["view"]["tree"] == there["agents"])
    });
}

#[test]
fn a_ghost_catches_up_after_a_gap_without_reloading_everything() {
    let (_hub, pc, server) = pc_and_server();
    let agent = session(&server, "Write the release notes");
    eventually("pc's ghost holds the first reply", || {
        ghost_holds(&pc, &agent, REPLY)
    });
    let before = ghost(&pc, &agent).unwrap();
    let cursor = before["last"].as_u64().unwrap();

    // pc is off while the session goes on.
    off(&pc);
    server.wait_for("pc", "offline");
    send(&server, &agent, "Cover the docs too");
    eventually("the second turn ends", || {
        server
            .user
            .call("agent.show", json!({"agent": agent, "after": cursor}))
            .is_ok_and(|shown| shown.to_string().contains(REPLY))
            && state(&server, &agent) == "idle"
    });
    // Back, it catches up from where it stopped: only what it missed comes.
    pc.user.status();
    pc.wait_for("server", "online");
    eventually("the ghost holds what came while pc was off", || {
        ghost_holds(&pc, &agent, "Cover the docs too")
    });
    // The server says it sent only what pc missed, from where pc's ghost stopped.
    let from_where = |log: &str| -> Option<(u64, usize)> {
        let said = format!("the ghost of session {agent} for pc caught up from event ");
        log.lines()
            .filter_map(|line| line.split_once(&said).map(|(_, rest)| rest))
            .filter_map(|rest| {
                let (from, count) = rest.split_once(": ")?;
                let count = count.strip_suffix(" events")?;
                Some((from.parse().ok()?, count.parse().ok()?))
            })
            .find(|(from, _)| *from != 0)
    };
    eventually(
        "the server caught pc's ghost up from where it stopped",
        || from_where(&server.log()).is_some(),
    );
    let (from, sent) = from_where(&server.log()).unwrap();
    let total = server
        .user
        .call("agent.show", json!({"agent": agent, "after": 0}))
        .unwrap()["events"]
        .as_array()
        .unwrap()
        .len();
    assert!(from >= cursor, "from {from}, the ghost held up to {cursor}");
    assert!(sent > 0, "what came while pc was off is sent");
    assert!(
        sent < total,
        "{sent} sent of {total}: not the whole conversation again"
    );
    eventually("the ghost holds the whole conversation, once", || {
        in_step(&pc, &server, &agent)
    });
}

#[test]
fn a_ghost_of_an_off_machine_still_opens_and_says_since_when() {
    let (_hub, pc, server) = pc_and_server();
    let agent = session(&server, "Write the release notes");
    eventually("pc's ghost holds the first reply", || {
        ghost_holds(&pc, &agent, REPLY)
    });
    off(&server);
    pc.wait_for("server", "offline");

    // Read from pc's own store: the machine that is off is asked nothing.
    assert!(ghost_holds(&pc, &agent, REPLY), "{}", pc.log());
    let machines = pc.remote();
    let server_state = machines["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|machine| machine["name"] == "server")
        .cloned()
        .unwrap();
    assert_eq!(server_state["state"], "offline");
    assert!(server_state["since"].as_u64().is_some(), "{server_state}");

    // And across a restart of pc: the ghost is kept in its store.
    off(&pc);
    pc.user.status();
    assert!(ghost_holds(&pc, &agent, REPLY), "{}", pc.log());
}

#[test]
fn a_ghost_goes_away_with_its_session() {
    let (_hub, pc, server) = pc_and_server();
    let agent = session(&server, "Write the release notes");
    let other = session(&server, "Fix the changelog");
    eventually("pc's ghosts hold both", || {
        ghost_holds(&pc, &agent, REPLY) && ghost_holds(&pc, &other, REPLY)
    });

    server
        .user
        .call("agent.stop", json!({"agent": agent}))
        .unwrap();
    eventually("the ended session's ghost is gone", || {
        ghost(&pc, &agent).is_err_and(|(code, _)| code == "not-found")
    });
    assert!(ghost_holds(&pc, &other, REPLY), "the other stays");
}

#[test]
fn the_ghost_decides_nothing() {
    let (_hub, pc, server) = pc_and_server();
    let agent = session(&server, "Write the release notes");
    eventually("pc's ghost holds the server's events", || {
        in_step(&pc, &server, &agent)
    });
    let kept = ghost(&pc, &agent).unwrap();

    // The ghost is no session of pc's: pc has no agent to write to, stop or answer for it.
    let (code, _) = pc
        .user
        .call("agent.send", json!({"agent": agent, "message": "Hello"}))
        .unwrap_err();
    assert_eq!(code, "not-found");
    let (code, _) = pc
        .user
        .call("agent.stop", json!({"agent": agent}))
        .unwrap_err();
    assert_eq!(code, "not-found");
    let listed = pc.user.call("agent.list", json!({})).unwrap();
    assert!(!listed.to_string().contains(&agent), "{listed}");
    // It is read, never written: `remote.ghost` takes nothing but what names it.
    let (code, _) = pc
        .user
        .call(
            "remote.ghost",
            json!({"machine": "server", "session": agent, "events": []}),
        )
        .unwrap_err();
    assert_eq!(code, "invalid-params");
    assert_eq!(ghost(&pc, &agent).unwrap(), kept, "nothing of it moved");
    // The session still runs there as it did, and goes on taking the user's messages.
    assert_eq!(state(&server, &agent), "idle");
    send(&server, &agent, "Cover the docs too");
    eventually("what the server decided comes back", || {
        ghost_holds(&pc, &agent, "Cover the docs too")
    });
}
