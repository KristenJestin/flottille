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

/// A picture of that many bytes (a PNG's signature, then zeros).
fn picture(len: usize) -> Value {
    let mut bytes = PNG.to_vec();
    bytes.resize(len, 0);
    image("image/png", &bytes)
}

/// An agent in `app` whose first turn lasts `ms`, still working when this returns.
fn busy_agent(user: &User, ms: u64) -> String {
    user.fake_script(json!({
        "images": true,
        "turns": [{"when": "hello", "do": [{"wait": ms}]}]
    }));
    project(user);
    let answer = user
        .call("agent.start", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn runs", || {
        show(user, &id)["agent"]["state"] == "working"
    });
    id
}

/// The image blocks of each prompt the fake agent got, oldest first.
fn images_per_prompt(user: &User) -> Vec<usize> {
    user.received_requests("session/prompt")
        .iter()
        .map(|request| {
            request["params"]["prompt"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["type"] == "image")
                .count()
        })
        .collect()
}

#[test]
fn an_image_queued_before_the_first_start_of_an_agent_without_images_is_not_sent() {
    let user = User::with_fake_agent();
    // `initialize` answers late: the daemon does not yet know that the agent takes no images, so
    // it accepts the message.
    user.fake_script(json!({"init_wait": 1500}));
    project(&user);
    let answer = user
        .call("agent.start", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    assert!(show(&user, &id)["agent"]["images"].is_null());
    user.call(
        "agent.send",
        json!({"agent": id, "message": "look at this",
            "images": [image("image/png", PNG), image("image/png", PNG)]}),
    )
    .unwrap();

    let sentence = "[2 images not sent: this agent takes no images]";
    eventually("the message reaches the agent", || {
        user.received_requests("session/prompt")
            .iter()
            .any(|request| {
                request["params"]["prompt"]
                    .to_string()
                    .contains("look at this")
            })
    });
    let request = user
        .received_requests("session/prompt")
        .into_iter()
        .find(|request| {
            request["params"]["prompt"]
                .to_string()
                .contains("look at this")
        })
        .unwrap();
    let blocks = request["params"]["prompt"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "no image block: {blocks:?}");
    assert_eq!(blocks[0]["type"], "text");
    let text = blocks[0]["text"].as_str().unwrap();
    assert!(
        text.contains("look at this") && text.contains(sentence),
        "{text}"
    );

    // The event carries the sentence, for the window, and names no image.
    eventually("the turn is recorded", || {
        show(&user, &id)["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| {
                e["name"] == "agent.prompted" && e["data"]["text"].to_string().contains(sentence)
            })
    });
    let events = show(&user, &id)["events"].clone();
    let prompted = events
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "agent.prompted" && e["data"]["text"].to_string().contains(sentence))
        .unwrap();
    assert!(prompted["data"].get("images").is_none(), "{prompted}");
}

#[test]
fn a_prompt_carries_four_images_at_most_and_the_rest_waits_for_the_next_turn() {
    let user = User::with_fake_agent();
    let id = busy_agent(&user, 4000);
    let two = || json!([image("image/png", PNG), image("image/png", PNG)]);
    for message in ["first", "second", "third"] {
        user.call(
            "agent.send",
            json!({"agent": id, "message": message, "images": two()}),
        )
        .unwrap();
    }

    eventually("the three messages are played", || {
        user.received_requests("session/prompt").len() == 3
    });

    // `hello`, then four images of two messages, then the two of the third.
    assert_eq!(images_per_prompt(&user), vec![0, 4, 2]);
}

#[test]
fn a_prompt_carries_ten_mib_of_images_at_most_and_the_rest_waits_for_the_next_turn() {
    let user = User::with_fake_agent();
    let id = busy_agent(&user, 6000);
    let five_mib = 5 * 1024 * 1024;
    for (message, len) in [("first", five_mib), ("second", five_mib), ("third", 1024)] {
        user.call(
            "agent.send",
            json!({"agent": id, "message": message, "images": [picture(len)]}),
        )
        .unwrap();
    }

    eventually("the three messages are played", || {
        user.received_requests("session/prompt").len() == 3
    });

    // Three pictures are under the count, but 10 MiB and one more byte are over the bytes.
    assert_eq!(images_per_prompt(&user), vec![0, 2, 1]);
}

#[test]
fn an_image_that_is_gone_when_the_turn_starts_leaves_a_placeholder() {
    let user = User::with_fake_agent();
    let id = busy_agent(&user, 4000);
    user.call(
        "agent.send",
        json!({"agent": id, "message": "look", "images": [image("image/png", PNG)]}),
    )
    .unwrap();
    let folder = user.path(&format!("data/flottille/images/{id}"));
    let name = std::fs::read_dir(&folder)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .to_string_lossy()
        .into_owned();
    std::fs::remove_file(folder.join(&name)).unwrap();

    eventually("the message is played", || {
        user.received_requests("session/prompt").len() == 2
    });

    let prompt = &user.received_requests("session/prompt")[1]["params"]["prompt"];
    assert_eq!(prompt.as_array().unwrap().len(), 1, "{prompt}");
    let text = prompt[0]["text"].as_str().unwrap();
    assert!(
        text.contains(&format!("[image {name} is missing]")),
        "{text}"
    );
}

#[test]
fn the_images_of_an_agent_go_with_it_when_it_ends_for_good() {
    let user = User::with_fake_agent();
    // No session to resume: stopping it forgets it at once.
    user.fake_script(json!({"images": true, "load": false, "resume": false}));
    project(&user);
    let id = idle_agent(&user);
    user.call(
        "agent.send",
        json!({"agent": id, "message": "look", "images": [image("image/png", PNG)]}),
    )
    .unwrap();
    eventually("the second turn ends", || {
        user.received_requests("session/prompt").len() == 2
    });
    let folder = user.path(&format!("data/flottille/images/{id}"));
    assert!(folder.exists());

    user.call("agent.stop", json!({"agent": id})).unwrap();

    eventually("its images are gone", || !folder.exists());
}

#[test]
fn a_request_far_over_the_size_is_refused_before_it_is_decoded() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"images": true}));
    project(&user);
    let id = idle_agent(&user);

    // Not base64 at all: decoding it first would say so; the size is judged first.
    let huge = "*".repeat(5 * 1024 * 1024 * 4 / 3 + 5);
    let refused = user
        .call(
            "agent.send",
            json!({"agent": id, "message": "x", "images": [{"mime": "image/png", "data": huge}]}),
        )
        .unwrap_err();

    assert!(refused.1.contains("over 5 MiB"), "{}", refused.1);
}
