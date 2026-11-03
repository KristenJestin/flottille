//! The messages the user wrote to the root while it works and that wait for it (#223): taken
//! back, taken back to be edited, sent at once, and the race with the root reading them. Against
//! the real daemon with the fake ACP agent; no real agent ever runs here.

mod common;

use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A PNG's signature and a little more: the daemon keeps the bytes, it does not decode them.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot a real picture";

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A root whose first turn hangs until it is cancelled, still working when this returns.
fn busy_root(user: &User) -> String {
    user.fake_script(json!({
        "images": true,
        "turns": [
            {"when": "HOLD", "do": [{"hang": true}]},
            {"when": "FIRST", "do": [{"say": "one"}]},
            {"when": "SECOND", "do": [{"say": "two"}]},
            {"when": "THIRD", "do": [{"say": "three"}]}
        ]
    }));
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.start", json!({"project": "app", "message": "HOLD"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn runs", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "working"
    });
    id
}

/// Writes to the root; the id of the delivery that waits.
fn write(user: &User, root: &str, message: &str) -> String {
    user.call("agent.send", json!({"agent": root, "message": message}))
        .unwrap()["delivery"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// What waits for the root, oldest first: the deliveries the tree lists as queued.
fn waiting(user: &User, root: &str) -> Vec<String> {
    let tree = user.call("agent.tree", json!({"agent": root})).unwrap();
    tree["agents"][0]["queued"]
        .as_array()
        .unwrap()
        .iter()
        .map(|queued| queued["delivery"].as_str().unwrap().to_owned())
        .collect()
}

/// The text of each prompt the fake agent got, oldest first.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|request| {
            request["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

fn cancel(user: &User, root: &str) {
    user.call("agent.cancel", json!({"agent": root})).unwrap();
}

fn settled(user: &User, root: &str) {
    eventually("the root to be idle", || {
        user.call("agent.show", json!({"agent": root})).unwrap()["agent"]["state"] == "idle"
    });
}

#[test]
fn cancelling_a_waiting_message_means_the_root_never_receives_it_and_it_leaves_the_queue() {
    let user = User::with_fake_agent();
    let root = busy_root(&user);
    let mistake = write(&user, &root, "azdsqdsq");
    let kept = write(&user, &root, "FIRST");
    assert_eq!(waiting(&user, &root), [mistake.clone(), kept.clone()]);

    let answer = user
        .call("agent.unqueue", json!({"agent": root, "delivery": mistake}))
        .unwrap();

    assert_eq!(answer["delivery"], mistake.as_str());
    assert_eq!(answer["text"], "azdsqdsq");
    assert_eq!(waiting(&user, &root), [kept]);
    cancel(&user, &root);
    eventually("the other message to be read", || prompts(&user).len() == 2);
    settled(&user, &root);
    assert!(
        prompts(&user)
            .iter()
            .all(|prompt| !prompt.contains("azdsqdsq")),
        "{:?}",
        prompts(&user)
    );
    let read = user.call("agent.read", json!({"agent": root})).unwrap();
    assert!(!read["lines"].to_string().contains("azdsqdsq"), "{read}");
}

#[test]
fn editing_a_waiting_message_gives_back_its_text_and_its_pictures_and_it_leaves_the_queue() {
    let user = User::with_fake_agent();
    let root = busy_root(&user);
    let delivery = user
        .call(
            "agent.send",
            json!({"agent": root, "message": "look at this",
                "images": [{"mime": "image/png", "data": base64_of(PNG)}]}),
        )
        .unwrap()["delivery"]
        .as_str()
        .unwrap()
        .to_owned();

    let answer = user
        .call(
            "agent.unqueue",
            json!({"agent": root, "delivery": delivery, "images": true}),
        )
        .unwrap();

    assert_eq!(answer["text"], "look at this");
    assert_eq!(
        answer["images"],
        json!([{"mime": "image/png", "data": base64_of(PNG)}])
    );
    assert!(waiting(&user, &root).is_empty());
}

#[test]
fn a_message_the_root_already_took_cannot_be_withdrawn_and_nothing_is_lost() {
    let user = User::with_fake_agent();
    let root = busy_root(&user);
    let delivery = write(&user, &root, "FIRST");
    // The root takes it first: its turn ends, and the next prompt holds the message.
    cancel(&user, &root);
    eventually("the root to read it", || {
        prompts(&user).iter().any(|prompt| prompt.contains("FIRST"))
    });

    let refused = user
        .call(
            "agent.unqueue",
            json!({"agent": root, "delivery": delivery}),
        )
        .unwrap_err();

    assert_eq!(refused.0, "refused");
    assert!(
        refused.1.contains("already read by the agent"),
        "{refused:?}"
    );
    settled(&user, &root);
    let read = user.call("agent.read", json!({"agent": root})).unwrap();
    assert!(read["lines"].to_string().contains("FIRST"), "{read}");
}

#[test]
fn sending_a_waiting_message_now_stops_the_turn_and_the_root_receives_it_next_before_the_others() {
    let user = User::with_fake_agent();
    let root = busy_root(&user);
    let first = write(&user, &root, "FIRST");
    let second = write(&user, &root, "SECOND");
    let third = write(&user, &root, "THIRD");

    user.call("agent.send_now", json!({"agent": root, "delivery": second}))
        .unwrap();

    // Its turn was cancelled, the next prompt is this message alone, and the others follow, in
    // their order.
    eventually("the three messages to be read", || {
        prompts(&user).len() == 3
    });
    settled(&user, &root);
    let prompts = prompts(&user);
    assert!(prompts[0].contains("HOLD"));
    assert!(
        prompts[1].contains("SECOND") && !prompts[1].contains("FIRST"),
        "{prompts:?}"
    );
    assert!(
        prompts[2].contains("FIRST") && prompts[2].contains("THIRD"),
        "{prompts:?}"
    );
    let _ = (first, third);
}

#[test]
fn sending_now_a_message_the_root_already_took_is_refused_and_stops_nothing() {
    let user = User::with_fake_agent();
    let root = busy_root(&user);
    let delivery = write(&user, &root, "FIRST");
    cancel(&user, &root);
    eventually("the root to read it", || {
        prompts(&user).iter().any(|prompt| prompt.contains("FIRST"))
    });
    settled(&user, &root);
    let before = prompts(&user).len();

    let refused = user
        .call(
            "agent.send_now",
            json!({"agent": root, "delivery": delivery}),
        )
        .unwrap_err();

    assert_eq!(refused.0, "refused");
    assert!(
        refused.1.contains("already read by the agent"),
        "{refused:?}"
    );
    assert_eq!(prompts(&user).len(), before);
}

fn json_of(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON")
}

#[test]
fn the_cli_withdraws_a_waiting_message_and_json_answers_it() {
    let user = User::with_fake_agent();
    let root = busy_root(&user);
    let delivery = write(&user, &root, "azdsqdsq");

    let answered = json_of(user.run(&["agent", "unqueue", &root, &delivery, "--json"]));

    assert_eq!(answered["delivery"], delivery.as_str());
    assert_eq!(answered["text"], "azdsqdsq");
    assert!(waiting(&user, &root).is_empty());
    // Taken back already: the CLI says so and fails.
    let again = user.run(&["agent", "unqueue", &root, &delivery]);
    assert!(!again.status.success());
}

#[test]
fn the_cli_sends_a_waiting_message_now() {
    let user = User::with_fake_agent();
    let root = busy_root(&user);
    write(&user, &root, "FIRST");
    let second = write(&user, &root, "SECOND");

    let answered = json_of(user.run(&["agent", "send-now", &root, &second, "--json"]));

    assert_eq!(answered["delivery"], second.as_str());
    eventually("the message to be read next", || prompts(&user).len() == 3);
    assert!(prompts(&user)[1].contains("SECOND"));
}
