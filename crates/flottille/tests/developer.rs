//! The scenarios of the developer mode (`docs/developer.md`): off, a judged call writes nothing;
//! on, the log holds the judge's input and verdict for the request it made, with the agent's id,
//! the events around it, and never a secret. The agents and the judge are fake ACP agents, in
//! throwaway repositories and temporary XDG folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// A token of the kind `secrets` recognises anywhere.
const TOKEN: &str = "ghp_fakefakefakefakefakefakefakefake0001";
/// The value of a variable whose name says it is a secret.
const PASSWORD: &str = "hunter2-hunter2";

/// A project `app` over a throwaway repository, and the session of a root agent working in its
/// folder, where every write is judged: the agent's id, its session, the folder.
fn session(user: &User) -> (String, Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "add the login page, then stop"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization), folder)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The judge's answer, as the fake judge says it.
fn says(verdict: &str, reason: &str) -> Value {
    json!([{"say": json!({"verdict": verdict, "reason": reason}).to_string()}])
}

/// The number of the request a held call made.
fn request_of(answer: &Answer) -> u64 {
    answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text))
}

/// The prompts the judge was given, in order.
fn judge_prompts(user: &User) -> Vec<String> {
    user.judge_received()
        .iter()
        .filter(|e| e["received"]["method"] == "session/prompt")
        .map(|e| {
            e["received"]["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn log_file(user: &User) -> PathBuf {
    user.path("state/flottille/developer.log")
}

/// The log's lines, each a JSON object.
fn log_lines(user: &User) -> Vec<Value> {
    std::fs::read_to_string(log_file(user))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

fn developer(user: &User, args: &[&str]) -> String {
    let mut all = vec!["developer"];
    all.extend(args);
    let output = user.run(&all);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn with_the_mode_off_a_judged_call_writes_nothing_to_the_log() {
    let user = User::with_fake_agent().with_judge(says("allow", "It fits the task."));
    let (_, mcp, folder) = session(&user);

    let answer = mcp.call(
        "fs_write",
        json!({"path": "notes.txt", "content": "notes\n"}),
    );

    assert!(!answer.error, "{}", answer.text);
    assert!(folder.join("notes.txt").exists());
    assert_eq!(judge_prompts(&user).len(), 1);
    assert!(!log_file(&user).exists(), "{:?}", log_lines(&user));
}

#[test]
fn with_the_mode_on_a_judged_call_writes_the_judge_s_input_and_verdict() {
    let user = User::with_fake_agent().with_judge(says("ask", "Nothing in the task asks for it."));
    assert!(developer(&user, &["on"]).starts_with("Developer mode is on"));
    let (id, mcp, _) = session(&user);

    let answer = mcp.call_keyed(
        "fs_write",
        json!({"path": "notes.txt", "content": "notes\n", "why": "to keep notes"}),
        "toolu_notes",
    );
    let request = request_of(&answer);

    let lines = log_lines(&user);
    let call = lines
        .iter()
        .find(|line| line["kind"] == "call" && line["tool"] == "fs_write")
        .unwrap_or_else(|| panic!("no call in {lines:?}"));
    assert_eq!(call["agent"], id.as_str());
    assert_eq!(call["call_key"], "toolu_notes");
    assert_eq!(call["request"], request);
    assert_eq!(call["gate"], "ask");
    assert_eq!(call["class"], "judge");
    assert!(
        call["reasons"].to_string().contains("project folder"),
        "{call}"
    );
    assert_eq!(call["outcome"], "held");
    let judge = &call["judge"];
    assert_eq!(judge_prompts(&user), [judge["input"].as_str().unwrap()]);
    assert_eq!(judge["provider"], "opencode");
    assert_eq!(judge["model"], Value::Null);
    assert_eq!(judge["verdict"], "ask");
    assert_eq!(judge["reason"], "Nothing in the task asks for it.");
    assert!(
        judge["answer"]
            .as_str()
            .unwrap()
            .contains("Nothing in the task asks for it."),
        "{judge}"
    );
    assert!(judge["duration_ms"].is_u64(), "{judge}");
    assert!(call["at"].is_u64(), "{call}");
    // The request it made, and what came of it, are there too, under the same number.
    user.call("approval.deny", json!({"number": request}))
        .unwrap();
    eventually("the answer is logged", || {
        log_lines(&user).iter().any(|line| {
            line["kind"] == "event"
                && line["name"] == "approval.decided"
                && line["data"]["number"] == request
                && line["agent"] == id.as_str()
        })
    });
    let lines = log_lines(&user);
    assert!(
        lines
            .iter()
            .any(|line| line["name"] == "approval.requested" && line["data"]["number"] == request),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line["name"] == "agent.started" && line["agent"] == id.as_str()),
        "{lines:?}"
    );
}

#[test]
fn a_secret_in_a_call_s_arguments_is_masked_in_the_log() {
    let user = User::with_fake_agent()
        .with_judge(says("ask", "Let the user see it."))
        .with_env("DB_PASSWORD", PASSWORD);
    developer(&user, &["on"]);
    let (_, mcp, _) = session(&user);

    let answer = mcp.call(
        "fs_write",
        json!({"path": "notes.txt", "content": format!("token = {TOKEN}\n"),
            "why": format!("the deploy needs {PASSWORD}")}),
    );
    request_of(&answer);

    let text = std::fs::read_to_string(log_file(&user)).unwrap();
    assert!(text.contains("\"kind\":\"call\""), "{text}");
    assert!(text.contains("[masked]"), "{text}");
    for secret in [TOKEN, PASSWORD] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
}

#[test]
fn the_mode_is_switched_from_the_command_line_and_says_where_its_log_is() {
    let user = User::new();
    let file = log_file(&user);

    let shown = developer(&user, &["show"]);
    assert_eq!(
        shown,
        format!("Developer mode is off. Its log: {}\n", file.display())
    );
    developer(&user, &["on"]);
    let shown: Value = serde_json::from_str(&developer(&user, &["show", "--json"])).unwrap();
    assert_eq!(shown, json!({"log": true, "file": file}));
    let config = std::fs::read_to_string(user.path("config/flottille/developer.toml")).unwrap();
    assert!(config.contains("log = true"), "{config}");
    assert_eq!(
        developer(&user, &["off"]),
        format!("Developer mode is off. Its log: {}\n", file.display())
    );
    let watch = user.watch(&["--after", "0"]);
    let seen = watch.until(|e| e["name"] == "developer.changed" && e["data"]["log"] == false);
    assert!(
        seen.iter()
            .any(|e| e["name"] == "developer.changed" && e["data"]["log"] == true),
        "{seen:?}"
    );
}
