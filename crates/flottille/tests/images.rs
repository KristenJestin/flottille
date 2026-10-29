//! The scenarios of images sent with a message (#120), against the fake agent
//! (`crates/flottille-fake-agent`) and the fake Claude, with throwaway repositories and temporary
//! XDG folders. No real agent ever runs here.

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

fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// An agent in `app`, idle once its first turn ended.
fn idle_agent(user: &User) -> String {
    let answer = user
        .call("agent.start", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        show(user, &id)["agent"]["state"] == "idle"
    });
    id
}

fn show(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()
}

fn image(mime: &str, bytes: &[u8]) -> Value {
    json!({"mime": mime, "data": base64_of(bytes)})
}

#[test]
fn a_pasted_image_goes_to_the_agent_as_an_image_block_beside_the_text() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true}));
    project(&user);
    let id = idle_agent(&user);

    user.call(
        "agent.send",
        json!({"agent": id, "message": "what is this?", "images": [image("image/png", PNG)]}),
    )
    .unwrap();
    eventually("the second turn ends", || {
        user.received_requests("session/prompt").len() == 2
    });

    let prompt = &user.received_requests("session/prompt")[1]["params"]["prompt"];
    assert_eq!(prompt[0]["type"], "text");
    assert!(
        prompt[0]["text"]
            .as_str()
            .unwrap()
            .contains("what is this?"),
        "{prompt}"
    );
    assert_eq!(prompt[1]["type"], "image");
    assert_eq!(prompt[1]["mimeType"], "image/png");
    assert_eq!(prompt[1]["data"], base64_of(PNG));
    assert_eq!(prompt.as_array().unwrap().len(), 2);
}

#[test]
fn the_image_is_kept_in_the_data_folder_and_named_by_the_turn_s_event() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true}));
    project(&user);
    let id = idle_agent(&user);

    user.call(
        "agent.send",
        json!({"agent": id, "message": "look", "images": [image("image/png", PNG)]}),
    )
    .unwrap();
    eventually("the second turn started", || {
        show(&user, &id)["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["name"] == "agent.prompted")
            .count()
            == 2
    });

    let prompted: Vec<Value> = show(&user, &id)["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == "agent.prompted")
        .cloned()
        .collect();
    assert!(
        prompted[0]["data"].get("images").is_none(),
        "a message with no image names none"
    );
    let names = prompted[1]["data"]["images"].as_array().unwrap();
    assert_eq!(names.len(), 1);
    let name = names[0].as_str().unwrap();
    assert!(name.ends_with(".png"), "{name}");
    // In Flottille's data folder, under the agent's own folder, never in the project.
    let stored = user.path(&format!("data/flottille/images/{id}/{name}"));
    assert_eq!(std::fs::read(&stored).unwrap(), PNG);
    assert!(!user.path("work/app").join(name).exists());

    // The window asks for it again after a restart: by the name the event gave.
    let again = user
        .call("agent.image", json!({"agent": id, "name": name}))
        .unwrap();
    assert_eq!(again["mime"], "image/png");
    assert_eq!(again["data"], base64_of(PNG));
    let missing = user
        .call("agent.image", json!({"agent": id, "name": "../../x.png"}))
        .unwrap_err();
    assert_eq!(missing.0, "not-found", "{missing:?}");
}

#[test]
fn an_image_is_refused_with_a_sentence_for_an_agent_that_takes_none() {
    let user = User::with_fake_agent();
    // The fake agent says `image: false` unless its script says otherwise.
    project(&user);
    let id = idle_agent(&user);
    assert_eq!(show(&user, &id)["agent"]["images"], false);

    let refused = user
        .call(
            "agent.send",
            json!({"agent": id, "message": "look", "images": [image("image/png", PNG)]}),
        )
        .unwrap_err();

    assert_eq!(refused.0, "refused");
    assert!(refused.1.contains("takes no images"), "{}", refused.1);
    assert!(
        !user.path(&format!("data/flottille/images/{id}")).exists(),
        "nothing was stored"
    );
}

#[test]
fn an_image_that_is_not_one_the_daemon_takes_is_refused() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true}));
    project(&user);
    let id = idle_agent(&user);
    assert_eq!(show(&user, &id)["agent"]["images"], true);
    let send = |images: Value, message: &str| {
        user.call(
            "agent.send",
            json!({"agent": id, "message": message, "images": images}),
        )
        .unwrap_err()
    };

    let kind = send(json!([image("application/pdf", PNG)]), "x");
    assert!(kind.1.contains("application/pdf"), "{}", kind.1);
    let text = send(json!([{"mime": "image/png", "data": "***"}]), "x");
    assert!(text.1.contains("base64"), "{}", text.1);
    let empty = send(json!([image("image/png", b"")]), "x");
    assert!(empty.1.contains("empty or over"), "{}", empty.1);
    let many = send(json!(vec![image("image/png", PNG); 5]), "x");
    assert!(many.1.contains("4 images at most"), "{}", many.1);
    let nothing = send(json!([]), " ");
    assert!(nothing.1.contains("empty"), "{}", nothing.1);
}

#[test]
fn an_image_alone_is_a_message() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true}));
    project(&user);
    let id = idle_agent(&user);

    user.call(
        "agent.send",
        json!({"agent": id, "message": "", "images": [image("image/png", PNG)]}),
    )
    .unwrap();

    eventually("the second turn is played", || {
        user.received_requests("session/prompt").len() == 2
    });
}

#[test]
fn the_claude_bridge_hands_an_image_to_claude_in_the_same_user_message() {
    let user = User::with_fake_claude();
    project(&user);
    let id = idle_agent(&user);
    assert_eq!(show(&user, &id)["agent"]["images"], true);

    user.call(
        "agent.send",
        json!({"agent": id, "message": "what is this?", "images": [image("image/png", PNG)]}),
    )
    .unwrap();

    eventually("claude got the second message", || {
        user.claude_received()
            .iter()
            .filter(|m| m["received"]["type"] == "user")
            .count()
            == 2
    });
    let sent: Vec<Value> = user
        .claude_received()
        .into_iter()
        .filter(|m| m["received"]["type"] == "user")
        .collect();
    let content = &sent[1]["received"]["message"]["content"];
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["type"], "base64");
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    assert_eq!(content[1]["source"]["data"], base64_of(PNG));
}
