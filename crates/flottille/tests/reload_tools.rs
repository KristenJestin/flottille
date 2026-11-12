//! The scenarios of #503: the user reloads a session's tools, and its agent's process restarts on
//! its conversation with the tools its role has now (`docs/tools.md`, "When the tools change").
//! Against the fake ACP agent, with throwaway repositories and temporary XDG folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository whose `origin` is GitHub's `acme/app`, with no
/// forge and no tracker declared.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "git@github.com:acme/app.git"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

/// Starts a root in `app` with a first message: its id and the event number to follow it from.
fn run(user: &User, message: &str) -> (String, u64) {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        answer["after"].as_u64().unwrap(),
    )
}

/// The agent's events from `after`, up to the first one named `name`.
fn events_until(user: &User, agent: &str, after: u64, name: &str) -> Vec<Value> {
    let of = |event: &Value| event["data"]["agent"] == agent;
    user.watch(&["--after", &after.to_string()])
        .until(|e| of(e) && e["name"] == name)
        .into_iter()
        .filter(of)
        .collect()
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn state(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"].clone()
}

/// The number of the agent's last event: what follows it is what a reload did.
fn last_event(user: &User, agent: &str) -> u64 {
    let shown = user.call("agent.show", json!({"agent": agent})).unwrap();
    shown["events"].as_array().unwrap().last().unwrap()["seq"]
        .as_u64()
        .unwrap()
}

/// The prompts the fake agent was sent, as text.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|prompt| prompt["params"]["prompt"].to_string())
        .collect()
}

/// The agent's process, between turns, is reloaded; the message sent after starts the new one.
fn reload_then_send(user: &User, id: &str) -> String {
    let after = last_event(user, id);
    let answer = user
        .call("agent.reload_tools", json!({"agent": id}))
        .unwrap();
    assert_eq!(answer["after_turn"], false, "{answer}");
    let released = events_until(user, id, after, "agent.released");
    assert_eq!(released.last().unwrap()["data"]["reason"], "reload");
    let sent = prompts(user).len();
    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    eventually("the message is sent", || prompts(user).len() > sent);
    prompts(user)[sent].clone()
}

#[test]
fn reload_tools_restarts_the_process_on_the_same_conversation() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "recall", "do": [{"recall": true}]}]}));
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    let native =
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["native_id"].clone();

    // The command line's own door, as the user types it.
    let out = user.run(&["agent", "reload-tools", &id, "--json"]);
    assert!(out.status.success(), "{out:?}");
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(answer["agent"]["id"], id.as_str());
    assert_eq!(answer["after_turn"], false);
    events_until(&user, &id, after, "agent.released");
    assert_eq!(state(&user, &id), "idle", "the session goes on");

    let sent = user
        .call("agent.send", json!({"agent": id, "message": "recall"}))
        .unwrap();
    let events = events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );
    let pids: Vec<u64> = user
        .received()
        .iter()
        .filter(|e| e["started"] == true)
        .map(|e| e["pid"].as_u64().unwrap())
        .collect();
    assert_eq!(pids.len(), 2, "a new process");
    assert_ne!(pids[0], pids[1]);
    let resumed = user.received_requests("session/resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["params"]["sessionId"], native, "its own session");
    let said: String = events
        .iter()
        .filter(|e| e["name"] == "agent.message")
        .filter_map(|e| e["data"]["text"].as_str())
        .collect();
    assert!(
        said.starts_with("earlier you said:") && said.contains("hello"),
        "the conversation went on: {said}"
    );
}

/// The sentence every reload opens its delivery with.
const RELOADED: &str = "Your tools were reloaded. These are your tools now, whatever you read of \
                        them before:";

/// Turns Specs on, for every project.
fn specs_on(user: &User) {
    user.call(
        "extension.set",
        json!({"name": "specs", "enabled": true, "project": null}),
    )
    .unwrap();
}

/// The line of `tool` in a prompt's tools delivery, the first one that starts so.
fn line_of<'a>(prompt: &'a str, tool: &str) -> &'a str {
    let start = prompt
        .find(&format!("- {tool}: "))
        .unwrap_or_else(|| panic!("no line for {tool}: {prompt}"));
    // A prompt is its JSON, so its lines end with an escaped newline.
    prompt[start..].split("\\n").next().unwrap()
}

#[test]
fn a_reload_tells_the_agent_every_tool_it_has_with_its_actions_even_when_nothing_changed() {
    let user = User::with_fake_agent();
    project(&user);
    specs_on(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");

    let prompt = reload_then_send(&user, &id);

    assert!(prompt.contains("[flottille:tools]"), "{prompt}");
    assert!(prompt.contains("go on"), "with what woke it: {prompt}");
    assert!(prompt.contains(RELOADED), "{prompt}");
    assert!(!prompt.contains("nothing changed"), "{prompt}");
    assert!(!prompt.contains("Flottille was updated"), "{prompt}");
    let spec = line_of(&prompt, "spec");
    assert!(
        spec.starts_with("- spec: new, read, write, ask, questions, amend")
            && spec.contains(" — The specs of this project: what an agent needs to build a task"),
        "{spec}"
    );
    // A tool without actions has no actions part: its line is its name and what it does.
    let fs_read = line_of(&prompt, "fs_read");
    assert!(
        fs_read.starts_with("- fs_read: Reads a text file"),
        "{fs_read}"
    );
    // Said once: the process after holds the same list, and is told nothing.
    let sent = prompts(&user).len();
    user.call("agent.send", json!({"agent": id, "message": "and then"}))
        .unwrap();
    eventually("the message is sent", || prompts(&user).len() > sent);
    assert!(!prompts(&user)[sent].contains("[flottille:tools]"));
}

#[test]
fn a_reload_after_a_change_lists_every_tool_and_then_what_changed() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    // The project gains a tracker: the role's list has `tickets` now.
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();

    let prompt = reload_then_send(&user, &id);

    assert!(prompt.contains("go on"), "with what woke it: {prompt}");
    assert!(prompt.contains("[flottille:tools]"), "{prompt}");
    // The reload's own words, not those of an update a restart would give.
    assert!(prompt.contains(RELOADED), "{prompt}");
    assert!(!prompt.contains("Flottille was updated"), "{prompt}");
    assert!(!prompt.contains("nothing changed"), "{prompt}");
    // The whole list first, then what changed.
    let list = prompt.find("- fs_read: ").expect("the whole list");
    let changed = prompt
        .find("What changed since you last read them:")
        .expect("what changed, said after the list");
    assert!(list < changed, "{prompt}");
    assert!(
        line_of(&prompt, "tickets").starts_with("- tickets: "),
        "{prompt}"
    );
    assert!(prompt[changed..].contains("- tickets: new"), "{prompt}");
}

#[test]
fn an_update_without_a_reload_still_says_only_what_changed() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    user.call(
        "project.set_settings",
        json!({"name": "app",
            "repositories": [{"path": ".", "forge": {"kind": "github", "repository": "acme/app"}}],
            "tracker": {"kind": "github", "repository": "."}}),
    )
    .unwrap();
    // The next process starts on its own, no reload asked.
    user.call("agent.stop", json!({"agent": id})).unwrap();
    user.resume(&id);
    let sent = prompts(&user).len();
    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    eventually("the message is sent", || prompts(&user).len() > sent);

    let prompt = prompts(&user)[sent].clone();

    assert!(prompt.contains("Flottille was updated"), "{prompt}");
    assert!(prompt.contains("- tickets: new"), "{prompt}");
    assert!(!prompt.contains(RELOADED), "{prompt}");
    assert!(
        !prompt.contains("- fs_read: "),
        "only what changed, not the whole list: {prompt}"
    );
}

#[test]
fn a_reload_during_a_turn_waits_for_its_end() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "work", "do": [{"hold": "release"}, {"say": "Done."}]}
    ]}));
    project(&user);
    let (id, _) = run(&user, "work");
    eventually("the turn runs", || state(&user, &id) == "working");
    let after = last_event(&user, &id);

    let answer = user
        .call("agent.reload_tools", json!({"agent": id}))
        .unwrap();

    assert_eq!(answer["after_turn"], true, "{answer}");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(state(&user, &id), "working", "the turn is not cut");
    std::fs::write(user.path("fake/release"), "").unwrap();
    let events = events_until(&user, &id, after, "agent.released");
    let ended: Vec<&Value> = events
        .iter()
        .filter(|e| e["name"] == "agent.turn_ended")
        .collect();
    assert_eq!(ended.len(), 1, "released once the turn ended");
    assert_eq!(ended[0]["data"]["stop_reason"], "end_turn");
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "agent.message" && e["data"]["text"] == "Done.")
    );
    assert_eq!(events.last().unwrap()["data"]["reason"], "reload");
}

#[test]
fn an_ended_session_does_not_offer_reload_tools() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    user.call("agent.stop", json!({"agent": id})).unwrap();

    let refused = user
        .call("agent.reload_tools", json!({"agent": id}))
        .unwrap_err();

    assert_eq!(refused.0, "refused", "{refused:?}");
    let out = user.run(&["agent", "reload-tools", &id]);
    assert!(!out.status.success());
}

#[test]
fn a_reload_of_an_idle_agent_says_its_new_tools_come_with_its_next_message() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");

    let out = user.run(&["agent", "reload-tools", &id]);

    assert!(out.status.success(), "{out:?}");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("at its next message"), "{said}");
    assert!(
        !said.contains("restarts"),
        "nothing restarts before it: {said}"
    );
}

#[test]
fn a_pending_reload_is_shown_and_said_until_the_new_process_takes_it() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "work", "do": [{"hold": "release"}, {"say": "Done."}]}
    ]}));
    project(&user);
    let (id, _) = run(&user, "work");
    eventually("the turn runs", || state(&user, &id) == "working");
    let after = last_event(&user, &id);
    let pending = |user: &User| -> (Value, Value) {
        let shown =
            user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["tools_reload"].clone();
        let tree = user.call("agent.tree", json!({"agent": id})).unwrap()["agents"][0]["agent"]
            ["tools_reload"]
            .clone();
        (shown, tree)
    };
    assert_eq!(pending(&user), (json!(false), json!(false)));

    user.call("agent.reload_tools", json!({"agent": id}))
        .unwrap();

    let asked = events_until(&user, &id, after, "agent.tools_reload");
    assert_eq!(asked.last().unwrap()["data"]["pending"], true);
    assert_eq!(pending(&user), (json!(true), json!(true)));
    std::fs::write(user.path("fake/release"), "").unwrap();
    events_until(&user, &id, after, "agent.released");
    assert_eq!(
        pending(&user),
        (json!(true), json!(true)),
        "until the next process takes it"
    );
    let sent = user
        .call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    let taken = events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.tools_reload",
    );
    assert_eq!(taken.last().unwrap()["data"]["pending"], false);
    assert_eq!(pending(&user), (json!(false), json!(false)));
}

#[test]
fn a_message_sent_during_the_turn_reaches_the_new_process() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "work", "do": [{"hold": "release"}, {"say": "Done."}]}
    ]}));
    project(&user);
    let (id, _) = run(&user, "work");
    eventually("the turn runs", || state(&user, &id) == "working");
    user.call("agent.reload_tools", json!({"agent": id}))
        .unwrap();

    user.call(
        "agent.send",
        json!({"agent": id, "message": "and then this"}),
    )
    .unwrap();
    std::fs::write(user.path("fake/release"), "").unwrap();

    eventually("the message is sent", || {
        prompts(&user).iter().any(|p| p.contains("and then this"))
    });
    let received = user.received();
    let pid_of = |text: &str| {
        received
            .iter()
            .find(|e| {
                e["received"]["method"] == "session/prompt"
                    && e["received"]["params"]["prompt"].to_string().contains(text)
            })
            .map(|e| e["pid"].as_u64().unwrap())
            .unwrap()
    };
    assert_ne!(
        pid_of("work"),
        pid_of("and then this"),
        "played by the new process"
    );
    let prompt = prompts(&user)
        .into_iter()
        .find(|p| p.contains("and then this"))
        .unwrap();
    assert!(prompt.contains("[flottille:tools]"), "{prompt}");
}

/// A project `team` whose ready workspace `w` a lead launches a child in.
fn team(user: &User) {
    let folder = user.path("work/team");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "team\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/team.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "team"}))
        .unwrap();
    user.call("workspace.create", json!({"project": "team", "name": "w"}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "team", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
}

#[test]
fn a_reload_of_a_child_is_refused() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "LAUNCH", "do": [
            {"mcp": {"tool": "agent_launch", "arguments": {"kind": "child", "title": "worker one",
                "brief": "HANG here", "workspace": "w"}}}]},
        {"when": "HANG", "do": [{"hang": true}]}
    ]}));
    team(&user);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "team", "workspace": "w", "message": "LAUNCH"}),
        )
        .unwrap();
    let lead = answer["agent"]["id"].as_str().unwrap().to_owned();
    let child_of = || {
        user.call("agent.tree", json!({"agent": lead})).unwrap()["agents"][1]["agent"]["id"]
            .as_str()
            .map(str::to_owned)
    };
    eventually("the child exists", || child_of().is_some());
    let child = child_of().unwrap();

    let refused = user
        .call("agent.reload_tools", json!({"agent": child}))
        .unwrap_err();

    assert_eq!(refused.0, "refused", "{refused:?}");
    assert!(refused.1.contains(&lead), "names its root: {refused:?}");
    assert!(
        !user.call("agent.show", json!({"agent": child})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "agent.tools_reload"),
        "nothing pending"
    );
    user.call("agent.stop", json!({"agent": lead})).unwrap();
}
