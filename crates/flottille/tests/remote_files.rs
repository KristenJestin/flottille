//! The pictures and videos of a session, seen from another device (`docs/remote.md`, "A file an
//! agent showed"): a hub served in the test on 127.0.0.1 and daemons, each with its own temporary
//! HOME and XDG folders and no keyring, paired as the user pairs them. The server's fake agent
//! shows files of its place with `web` `show_link`; a client of `pc` asks the server for them with
//! `agent.file`, signed with the device key and carried by `pc`'s daemon, chunk by chunk.

use crate::common;
use crate::remote;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use common::User;
use remote::{Machine, WAIT, error_of, pc_and_server, signed};
use serde_json::{Value, json};

/// The most of a file one answer carries.
const CHUNK: usize = 512 * 1024;

/// A picture, 1 pixel by 1.
const PNG: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// What the server's agent does: it leads, then shows what each message names.
fn script() -> Value {
    let file = |path: &str| json!({"mcp": {"tool": "web", "arguments": {"action": "show_link", "path": path}}});
    let link = |url: &str| json!({"mcp": {"tool": "web", "arguments": {"action": "show_link", "url": url}}});
    json!({"turns": [
        {"when": "lead the work", "do": [{"say": "On it."}]},
        {"when": "SHOW SHOT", "do": [file("shot.png")]},
        {"when": "SHOW NESTED", "do": [file("pics/shot.png")]},
        {"when": "SHOW CLIP", "do": [file("clip.mp4")]},
        {"when": "SHOW NOTES", "do": [file("notes.md")]},
        {"when": "SHOW PAGE", "do": [link("https://example.org/t/42")]}
    ]})
}

/// `pc` and `server` linked, the server with the project `app` and a session that leads in it.
fn machines() -> (remote::TestHub, Machine, Machine, String) {
    let user = User::with_fake_agent();
    user.fake_script(script());
    let (hub, pc, server) = pc_and_server(user);
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
    let agent = start(&server);
    (hub, pc, server, agent)
}

/// A session of the server's that leads in `app`, idle after its first turn.
fn start(server: &Machine) -> String {
    let started = server
        .user
        .call(
            "agent.run",
            json!({"project": "app", "message": "lead the work"}),
        )
        .unwrap();
    let agent = started["agent"]["id"].as_str().unwrap().to_owned();
    idle(server, &agent);
    agent
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

/// Every event the server stored, read with `events.watch` from the start.
fn events(server: &Machine) -> Vec<Value> {
    let mut stream =
        UnixStream::connect(server.user.path("runtime/flottille/daemon.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(800)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        json!({"id": "w", "method": "events.watch", "params": {"after": 0}})
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let mut seen = Vec::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return seen,
            Ok(_) => {
                let message: Value = serde_json::from_str(&line).unwrap();
                if message["event"].is_object() {
                    seen.push(message["event"].clone());
                }
            }
        }
    }
}

/// The agent shows what `message` names: the number of the event that says it.
fn show(server: &Machine, agent: &str, message: &str) -> u64 {
    let before = events(server)
        .iter()
        .filter(|event| event["name"] == "agent.link_shown")
        .count();
    server
        .user
        .call("agent.send", json!({"agent": agent, "message": message}))
        .unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        let shown: Vec<Value> = events(server)
            .into_iter()
            .filter(|event| event["name"] == "agent.link_shown" && event["data"]["agent"] == agent)
            .collect();
        if shown.len() > before {
            idle(server, agent);
            return shown.last().unwrap()["seq"].as_u64().unwrap();
        }
        assert!(Instant::now() < deadline, "{message} was never shown");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `agent.file` with `params`, asked by a client of `pc` for the server.
fn ask(pc: &Machine, params: Value) -> Value {
    pc.raw(&signed(&pc.device(), "f", "server", "agent.file", params))
}

fn bytes_of(answer: &Value) -> Vec<u8> {
    use base64::Engine as _;
    let data = answer["result"]["data"]
        .as_str()
        .unwrap_or_else(|| panic!("no bytes: {answer}"));
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap()
}

/// A video of `size` bytes no two chunks of which are alike.
fn clip(size: usize) -> Vec<u8> {
    (0..size)
        .map(|n| (n % 251) as u8 ^ (n / CHUNK) as u8)
        .collect()
}

#[test]
fn a_picture_a_session_showed_is_served_to_another_machine() {
    let (_hub, pc, server, agent) = machines();
    std::fs::write(server.user.path("work/app/shot.png"), PNG).unwrap();
    let shown = show(&server, &agent, "SHOW SHOT");

    let answer = ask(&pc, json!({"agent": agent, "shown": shown}));
    assert_eq!(answer["result"]["mime"], "image/png", "{answer}");
    assert_eq!(answer["result"]["size"], PNG.len(), "{answer}");
    assert_eq!(answer["result"]["offset"], 0, "{answer}");
    assert_eq!(bytes_of(&answer), PNG);
    assert!(
        server.log().contains("agent.file asked by pc"),
        "served as a request of pc's: {}",
        server.log()
    );
}

#[test]
fn a_video_plays_while_it_arrives_and_seeks() {
    let (_hub, pc, server, agent) = machines();
    let video = clip(CHUNK * 2 + CHUNK / 2);
    std::fs::write(server.user.path("work/app/clip.mp4"), &video).unwrap();
    let shown = show(&server, &agent, "SHOW CLIP");

    // The first chunk comes alone, with the whole size: a player starts on it.
    let first = ask(&pc, json!({"agent": agent, "shown": shown}));
    assert_eq!(first["result"]["mime"], "video/mp4", "{first}");
    assert_eq!(first["result"]["size"], video.len());
    assert_eq!(bytes_of(&first), video[..CHUNK]);

    // A seek asks for the range it needs, with nothing before it read.
    let at = CHUNK * 2 + 100;
    let sought = ask(
        &pc,
        json!({"agent": agent, "shown": shown, "offset": at, "length": 4096}),
    );
    assert_eq!(sought["result"]["offset"], at);
    assert_eq!(bytes_of(&sought), video[at..at + 4096]);

    // No answer carries more than a chunk, whatever is asked; past the end there is nothing.
    let large = ask(
        &pc,
        json!({"agent": agent, "shown": shown, "offset": CHUNK, "length": CHUNK * 8}),
    );
    assert_eq!(bytes_of(&large), video[CHUNK..CHUNK * 2]);
    let past = ask(
        &pc,
        json!({"agent": agent, "shown": shown, "offset": video.len() + 10}),
    );
    assert!(bytes_of(&past).is_empty(), "{past}");
}

#[test]
fn a_machine_serves_no_file_its_session_did_not_show() {
    let (_hub, pc, server, agent) = machines();
    let place = server.user.path("work/app");
    std::fs::write(place.join("shot.png"), PNG).unwrap();
    std::fs::write(place.join("notes.md"), "# notes\n").unwrap();
    std::fs::create_dir_all(place.join("pics")).unwrap();
    std::fs::write(place.join("pics/shot.png"), PNG).unwrap();
    // Never shown, in the place and outside it.
    let secret = b"\x89PNG not to be seen";
    std::fs::write(place.join("secret.png"), secret).unwrap();
    std::fs::create_dir_all(server.user.path("home/private")).unwrap();
    std::fs::write(server.user.path("home/private/shot.png"), secret).unwrap();

    let shot = show(&server, &agent, "SHOW SHOT");
    let nested = show(&server, &agent, "SHOW NESTED");
    let notes = show(&server, &agent, "SHOW NOTES");
    let page = show(&server, &agent, "SHOW PAGE");
    let other = start(&server);

    let refused = |params: Value, what: &str| {
        let answer = ask(&pc, params);
        assert!(answer["error"].is_object(), "{what} was served: {answer}");
        assert!(
            !answer.to_string().contains("not to be seen"),
            "{what}: {answer}"
        );
        error_of(&answer)
    };

    // A path is never taken: only the session and the reference it showed the file with.
    refused(
        json!({"agent": agent, "shown": shot, "path": place.join("secret.png")}),
        "a path",
    );
    refused(
        json!({"agent": agent, "path": place.join("secret.png")}),
        "a path alone",
    );
    // An event that is no file shown: a link of the web, any other event of the session.
    let why = refused(json!({"agent": agent, "shown": page}), "a link of the web");
    assert!(why.contains("showed no file"), "{why}");
    refused(json!({"agent": agent, "shown": shot - 1}), "another event");
    refused(
        json!({"agent": agent, "shown": 999_999}),
        "an event that is not",
    );
    // The file another session showed is not this one's.
    refused(json!({"agent": other, "shown": shot}), "another session's");
    // A file shown that is no picture or video.
    let why = refused(json!({"agent": agent, "shown": notes}), "a document");
    assert!(why.contains("picture or a video"), "{why}");

    // The file shown, put back as a link to another one: not followed.
    std::fs::remove_file(place.join("shot.png")).unwrap();
    std::os::unix::fs::symlink(place.join("secret.png"), place.join("shot.png")).unwrap();
    refused(
        json!({"agent": agent, "shown": shot}),
        "a link put in its place",
    );
    // Its folder, put back as a link to a folder outside the place: not followed either.
    std::fs::rename(place.join("pics"), place.join("pics-before")).unwrap();
    std::os::unix::fs::symlink(server.user.path("home/private"), place.join("pics")).unwrap();
    refused(
        json!({"agent": agent, "shown": nested}),
        "a folder put back as a link",
    );
}

/// One signed line written to a daemon's socket on a thread of its own: its answer, and how long
/// it took.
fn send_apart(
    socket: std::path::PathBuf,
    line: String,
) -> std::thread::JoinHandle<(Value, Duration)> {
    std::thread::spawn(move || {
        let began = Instant::now();
        let mut stream = UnixStream::connect(socket).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        writeln!(stream, "{line}").unwrap();
        let answer = common::read_line_patiently(stream);
        (serde_json::from_str(&answer).unwrap(), began.elapsed())
    })
}

#[test]
fn one_device_asking_without_end_does_not_starve_another() {
    let (_hub, pc, server, agent) = machines();
    std::fs::write(server.user.path("work/app/clip.mp4"), clip(CHUNK * 2)).unwrap();
    let shown = show(&server, &agent, "SHOW CLIP");
    let socket = pc.user.path("runtime/flottille/daemon.sock");
    let params = json!({"agent": agent, "shown": shown});
    // One device asks forty chunks at once: each is one more call waiting for the machine's rate.
    let lines: Vec<String> = (0..40)
        .map(|n| {
            signed(
                &pc.device(),
                &format!("burst-{n}"),
                "server",
                "agent.file",
                params.clone(),
            )
        })
        .collect();
    // The link to the server is up before the burst, as it is for a window that has shown it
    // (dials are limited, apart from this test: a cold burst would be refused by the link).
    let first = ask(&pc, params.clone());
    assert!(first["result"].is_object(), "{first}");

    let began = Instant::now();
    let burst: Vec<_> = lines
        .into_iter()
        .map(|line| send_apart(socket.clone(), line))
        .collect();
    std::thread::sleep(Duration::from_millis(300));

    // Another asker, meanwhile: it may be told to retry, and it is served within a few seconds.
    let mut served = None;
    while began.elapsed() < Duration::from_secs(5) {
        let answer = ask(&pc, params.clone());
        if answer["result"].is_object() {
            served = Some(began.elapsed());
            break;
        }
        let why = error_of(&answer);
        assert!(why.contains("busy"), "a refusal that says to retry: {why}");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        served.is_some(),
        "another asker got no chunk within 5 s of a burst of 40 calls"
    );

    let (mut refused, mut served_in_burst) = (0, 0);
    for call in burst {
        let (answer, took) = call.join().unwrap();
        if answer["result"].is_object() {
            served_in_burst += 1;
            assert!(took < Duration::from_secs(5), "a chunk waited {took:?}");
        } else {
            refused += 1;
            let why = error_of(&answer);
            assert!(why.contains("busy"), "{why}");
            assert!(
                took < Duration::from_secs(2),
                "a refusal came after {took:?}"
            );
        }
    }
    assert!(refused > 0, "no call of the burst was refused");
    assert!(served_in_burst > 0, "the burst got nothing");
}

#[test]
fn a_file_that_is_a_hard_link_or_another_file_than_the_one_shown_is_not_served() {
    let (_hub, pc, server, agent) = machines();
    let place = server.user.path("work/app");
    // A file outside the place, and a hard link to it made in the place: its real path is in
    // the place, its bytes are the outside file's.
    let outside = server.user.path("home/private");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.png"), b"\x89PNG not to be seen").unwrap();
    std::fs::hard_link(outside.join("secret.png"), place.join("shot.png")).unwrap();
    let linked = show(&server, &agent, "SHOW SHOT");
    let answer = ask(&pc, json!({"agent": agent, "shown": linked}));
    assert!(
        answer["error"].is_object(),
        "a hard link was served: {answer}"
    );
    assert!(!answer.to_string().contains("not to be seen"), "{answer}");

    // A file shown, then another file put in its place (a rename over it): not the one shown.
    std::fs::write(place.join("clip.mp4"), clip(1000)).unwrap();
    std::fs::write(place.join("other.mp4"), b"not to be seen either").unwrap();
    let shown = show(&server, &agent, "SHOW CLIP");
    let before = ask(&pc, json!({"agent": agent, "shown": shown}));
    assert_eq!(bytes_of(&before), clip(1000), "{before}");
    std::fs::rename(place.join("other.mp4"), place.join("clip.mp4")).unwrap();
    let answer = ask(&pc, json!({"agent": agent, "shown": shown}));
    assert!(
        answer["error"].is_object(),
        "another file was served: {answer}"
    );
    assert!(!answer.to_string().contains("either"), "{answer}");
}
