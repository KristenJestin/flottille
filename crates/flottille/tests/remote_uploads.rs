//! The pictures and pasted texts of a message sent to a session of another machine, in chunks
//! (#1187, `docs/remote.md`, "Pictures sent to another machine"): a hub served in the test on
//! 127.0.0.1 and daemons, each with its own temporary HOME and XDG folders and no keyring, paired
//! as the user pairs them. A client of `pc` sends each chunk with `agent.upload`, signed with the
//! device key and carried by `pc`'s daemon, then the message that names the upload.

use crate::common;
use crate::remote;

use std::time::{Duration, Instant};

use common::User;
use remote::{Machine, WAIT, error_of, pc_and_server, signed};
use serde_json::{Value, json};

/// The most of a file one chunk carries.
const CHUNK: usize = 512 * 1024;

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A picture of `size` bytes, larger than a link's frame once in base64: a PNG's signature, then
/// bytes the daemon cannot decode, so that it keeps them as they came. No two chunks alike.
fn picture(size: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((bytes.len()..size).map(|n| (n % 251) as u8 ^ (n / CHUNK) as u8));
    bytes
}

/// `pc` and `server` linked, the server with the project `app` and an idle session in it whose
/// agent takes pictures.
fn machines(server_user: User) -> (remote::TestHub, Machine, Machine, String) {
    server_user.fake_script(json!({"images": true}));
    let (hub, pc, server) = pc_and_server(server_user);
    let folder = server.user.path("work/app");
    server.user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    server.user.git(&folder, &["add", "."]);
    server
        .user
        .git(&folder, &["commit", "--quiet", "-m", "start"]);
    server
        .user
        .call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let started = server
        .user
        .call("agent.start", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let agent = started["agent"]["id"].as_str().unwrap().to_owned();
    idle(&server, &agent);
    (hub, pc, server, agent)
}

fn idle(server: &Machine, agent: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let state = server
            .user
            .call("agent.show", json!({"agent": agent}))
            .unwrap()["agent"]["state"]
            .clone();
        if state == "idle" {
            return;
        }
        assert!(Instant::now() < deadline, "{agent} never went idle");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `method` with `params`, asked by a client of `pc` for the server.
fn ask(pc: &Machine, method: &str, params: Value) -> Value {
    pc.raw(&signed(&pc.device(), "u", "server", method, params))
}

/// One chunk of the file `file` of the upload `upload`, from `offset`.
fn chunk(
    upload: &str,
    file: u32,
    kind: (&str, Option<&str>),
    size: usize,
    offset: usize,
    data: &[u8],
) -> Value {
    let mut params = json!({"upload": upload, "file": file, "kind": kind.0, "size": size,
        "offset": offset, "data": base64_of(data)});
    if let Some(mime) = kind.1 {
        params["mime"] = json!(mime);
    }
    params
}

/// Every chunk of `bytes` as the file `file` of `upload`, each answered.
fn upload(pc: &Machine, upload: &str, file: u32, kind: (&str, Option<&str>), bytes: &[u8]) {
    for (at, part) in bytes.chunks(CHUNK).enumerate() {
        let params = chunk(upload, file, kind, bytes.len(), at * CHUNK, part);
        let answer = ask(pc, "agent.upload", params);
        assert_eq!(
            answer["result"]["received"],
            (at * CHUNK + part.len()) as u64,
            "{answer}"
        );
    }
}

const PNG: (&str, Option<&str>) = ("image", Some("image/png"));
const TEXT: (&str, Option<&str>) = ("text", None);

/// The folders of the uploads the server keeps.
fn kept(server: &Machine) -> usize {
    std::fs::read_dir(server.user.path("data/flottille/uploads"))
        .map(|folder| folder.count())
        .unwrap_or(0)
}

#[test]
fn a_picture_larger_than_a_frame_reaches_a_session_of_another_machine() {
    let (_hub, pc, server, agent) = machines(User::with_fake_agent());
    let shot = picture(CHUNK * 2 + 300_000);
    // Inline, it cannot be one request: the link's frame is 1 MiB.
    assert!(base64_of(&shot).len() > 1 << 20);
    let text = "a line of a long log\n".repeat(60_000);

    upload(&pc, "up1", 1, PNG, &shot);
    upload(&pc, "up1", 2, TEXT, text.as_bytes());
    let sent = ask(
        &pc,
        "agent.send",
        json!({"agent": agent, "message": "what is this? [Image 1: shot.png]", "upload": "up1"}),
    );
    assert!(sent["result"]["delivery"].is_string(), "{sent}");
    assert!(
        server.log().contains("agent.upload asked by pc"),
        "served as requests of pc's: {}",
        server.log()
    );

    let deadline = Instant::now() + WAIT;
    while server.user.received_requests("session/prompt").len() < 2 {
        assert!(
            Instant::now() < deadline,
            "the message never reached the agent"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let prompt = &server.user.received_requests("session/prompt")[1]["params"]["prompt"];
    let image = prompt
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["type"] == "image")
        .unwrap_or_else(|| panic!("no picture in {prompt}"));
    assert_eq!(image["mimeType"], "image/png");
    assert_eq!(
        image["data"],
        base64_of(&shot),
        "the whole picture, as it was"
    );
    // The text around the picture's block: before it, and after it.
    let said: String = prompt
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect();
    let said = said.as_str();
    assert!(said.contains("what is this?"), "{said}");
    // The pasted text is a file of the session's, which the message says where to read.
    let path = said
        .split("read it at ")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_else(|| panic!("no pasted text said in {said}"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
    // Taken by its message: nothing of the upload is left.
    assert_eq!(kept(&server), 0);
}

#[test]
fn the_limits_of_a_local_message_hold_for_one_sent_in_chunks() {
    let (_hub, pc, server, agent) = machines(User::with_fake_agent());
    let refused = |params: Value| error_of(&ask(&pc, "agent.upload", params));

    // A picture over 5 MiB, said by its size before any of it is kept.
    let why = refused(chunk("big", 1, PNG, 5 * 1024 * 1024 + 1, 0, b"x"));
    assert!(why.contains("picture 1") && why.contains("5 MiB"), "{why}");
    // A type a local message does not take.
    let why = refused(chunk("bmp", 2, ("image", Some("image/bmp")), 10, 0, b"x"));
    assert!(
        why.contains("picture 2") && why.contains("png, jpeg, gif and webp"),
        "{why}"
    );
    // A chunk is 512 KiB at most, and comes in order.
    let why = refused(chunk("c", 1, PNG, CHUNK * 2, 0, &vec![1; CHUNK + 1]));
    assert!(why.contains("512 KiB"), "{why}");
    let why = refused(chunk("c", 1, PNG, CHUNK * 2, CHUNK, &[1; 10]));
    assert!(
        why.contains("picture 1") && why.contains("offset 0"),
        "{why}"
    );
    // A name of the upload is the sender's word, never a path.
    let why = refused(chunk("../../x", 1, PNG, 10, 0, b"x"));
    assert!(why.contains("letters, digits"), "{why}");
    assert!(!why.contains("../"), "never said back: {why}");

    // 20 pictures, as a local message.
    for file in 1..=20 {
        ask(&pc, "agent.upload", chunk("many", file, PNG, 1, 0, b"x"));
    }
    let why = refused(chunk("many", 21, PNG, 1, 0, b"x"));
    assert!(why.contains("20 pictures"), "{why}");

    // A picture that arrived in part is no message's.
    ask(
        &pc,
        "agent.upload",
        chunk("part", 1, PNG, CHUNK + 10, 0, &vec![1; CHUNK]),
    );
    let why = error_of(&ask(
        &pc,
        "agent.send",
        json!({"agent": agent, "message": "look", "upload": "part"}),
    ));
    assert!(
        why.contains("picture 1") && why.contains("in part"),
        "{why}"
    );

    // Two uploads wait at most for one device's messages: the others are refused.
    let why = refused(chunk("third", 1, PNG, 1, 0, b"x"));
    assert!(why.contains("wait"), "{why}");

    // An upload is its sender's: a client of the server cannot take pc's.
    let (code, why) = server
        .user
        .call(
            "agent.send",
            json!({"agent": agent, "message": "look", "upload": "many"}),
        )
        .unwrap_err();
    assert_eq!(code, "not-found", "{why}");
    // Nothing reached the agent.
    assert_eq!(server.user.received_requests("session/prompt").len(), 1);
}

#[test]
fn chunks_without_their_message_are_dropped() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_UPLOAD_MS", "1500");
    let (_hub, pc, server, agent) = machines(user);
    let shot = picture(CHUNK + 100);
    upload(&pc, "late", 1, PNG, &shot);
    assert_eq!(kept(&server), 1, "kept while its message may come");

    let deadline = Instant::now() + WAIT;
    while kept(&server) > 0 {
        assert!(
            Instant::now() < deadline,
            "an upload without its message was kept"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let answer = ask(
        &pc,
        "agent.send",
        json!({"agent": agent, "message": "look", "upload": "late"}),
    );
    assert_eq!(answer["error"]["code"], "not-found", "{answer}");
    assert!(error_of(&answer).contains("dropped"), "{answer}");
}
