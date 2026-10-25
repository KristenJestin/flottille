//! How `send --follow` picks the turn of its own message (`docs/cli.md`), against a daemon that
//! says exactly what the test scripts: another client's delivery is queued, and played, between
//! the numbers the command read and the turn of its own message. The real daemon cannot be made
//! to interleave two clients at a chosen instant.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};

use common::User;
use serde_json::{Value, json};

const AGENT: &str = "aaaaaaaaaaaa";

/// What the scripted daemon answers `agent.send`, and the events it streams once watched.
fn serve(listener: UnixListener, events: Vec<Value>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { return };
        let events = events.clone();
        std::thread::spawn(move || answer(stream, &events));
    }
}

fn answer(stream: UnixStream, events: &[Value]) {
    let mut writer = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let request: Value = serde_json::from_str(&line).unwrap();
        let result = match request["method"].as_str().unwrap() {
            "agent.send" => json!({"agent": {"id": AGENT}, "after": 7, "delivery": "user-mine"}),
            "agent.tree" => json!({"agents": [{"agent": {"id": AGENT}}]}),
            "agent.show" => json!({"agent": {"id": AGENT}, "events": []}),
            "events.watch" => json!({"after": 7}),
            other => panic!("unexpected {other}"),
        };
        writeln!(writer, "{}", json!({"id": request["id"], "result": result})).unwrap();
        if request["method"] == "events.watch" {
            for (seq, (name, data)) in events
                .iter()
                .map(|e| (e[0].clone(), e[1].clone()))
                .enumerate()
            {
                let event = json!({"event": {"seq": 8 + seq, "at": 0, "name": name, "data": data}});
                writeln!(writer, "{event}").unwrap();
            }
        }
    }
}

#[test]
fn send_follow_prints_only_the_reply_to_the_delivery_the_daemon_named_not_to_one_queued_before_it()
{
    let user = User::new();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(user.path("runtime/flottille"))
        .unwrap();
    let listener = UnixListener::bind(user.path("runtime/flottille/daemon.sock")).unwrap();
    let said =
        |text: &str| json!(["agent.message", {"agent": AGENT, "text": text, "replay": false}]);
    let ended = || json!(["agent.turn_ended", {"agent": AGENT, "stop_reason": "end_turn"}]);
    let sent = |delivery: &str| json!(["delivery.sent", {"agent": AGENT, "deliveries": [delivery], "how": "prompt"}]);
    // Another client's message was queued first and played first, then ours.
    let events = vec![
        json!(["delivery.queued", {"agent": AGENT, "delivery": "user-theirs", "kind": "user"}]),
        json!(["delivery.queued", {"agent": AGENT, "delivery": "user-mine", "kind": "user"}]),
        sent("user-theirs"),
        said("theirs"),
        ended(),
        sent("user-mine"),
        said("mine"),
        ended(),
    ];
    std::thread::spawn(move || serve(listener, events));

    let output = user.run(&["agent", "send", AGENT, "hello", "--follow"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "mine\n");
}
