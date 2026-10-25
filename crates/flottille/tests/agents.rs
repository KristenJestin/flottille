//! The scenarios of agents run over ACP, against the fake agent (`crates/flottille-fake-agent`),
//! with throwaway repositories and temporary XDG folders. No real agent ever runs here.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository with one commit, pushed to a bare remote.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

/// Starts an agent in `app` with a first message; its id and the event number to follow it from.
fn run(user: &User, params: Value) -> (String, u64) {
    let mut params = params;
    params["project"] = "app".into();
    let answer = user.call("agent.run", params).unwrap();
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        answer["after"].as_u64().unwrap(),
    )
}

fn of(agent: &str) -> impl Fn(&Value) -> bool + '_ {
    move |event: &Value| event["data"]["agent"] == agent
}

/// The agent's events from `after`, up to the first one named `name`.
fn events_until(user: &User, agent: &str, after: u64, name: &str) -> Vec<Value> {
    let watch = user.watch(&["--after", &after.to_string()]);
    watch
        .until(|e| of(agent)(e) && e["name"] == name)
        .into_iter()
        .filter(of(agent))
        .collect()
}

fn names(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["name"].as_str().unwrap()).collect()
}

fn show(user: &User, agent: &str) -> Value {
    user.call("agent.show", json!({"agent": agent})).unwrap()
}

#[test]
fn a_session_is_opened_and_a_prompt_gets_its_reply_recorded_as_events() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "hello", "do": [{"think": "a greeting"}, {"say": "Hello"}, {"say": " there!"}]}]}));
    project(&user);

    let output = user.run(&["agent", "run", "app", "hello"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Hello there!"), "{stdout}");
    let id = stdout
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches(',');
    let shown = show(&user, id);
    assert_eq!(shown["agent"]["state"], "idle");
    // The message was a delivery: stored, then sent.
    let all = shown["events"].as_array().unwrap();
    let deliveries: Vec<&str> = names(all)
        .into_iter()
        .filter(|n| n.starts_with("delivery."))
        .collect();
    assert_eq!(deliveries, ["delivery.queued", "delivery.sent"]);
    let events: Vec<Value> = all
        .iter()
        .filter(|e| !e["name"].as_str().unwrap().starts_with("delivery."))
        .cloned()
        .collect();
    assert_eq!(
        names(&events),
        [
            "agent.created",
            "agent.started",
            "agent.prompted",
            "agent.thought",
            "agent.message",
            "agent.message",
            "agent.turn_ended"
        ]
    );
    assert_eq!(events[1]["data"]["opened"], "new");
    assert_eq!(events[2]["data"]["text"], "[flottille:user]\nhello");
    assert_eq!(events[4]["data"]["text"], "Hello");
    assert_eq!(events[4]["data"]["replay"], false);
    assert_eq!(events[6]["data"]["stop_reason"], "end_turn");
    let opened = user.received_requests("session/new");
    assert_eq!(opened.len(), 1);
    assert_eq!(
        opened[0]["params"]["cwd"],
        user.path("work/app").to_str().unwrap()
    );
    assert_eq!(
        shown["agent"]["native_id"],
        user.received()
            .iter()
            .rev()
            .find_map(|e| e["received"]["params"]["sessionId"]
                .as_str()
                .map(str::to_owned))
            .unwrap()
    );
}

#[test]
fn the_exact_claude_code_options_are_sent() {
    let user = User::with_fake_agent();
    project(&user);

    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let agent = &show(&user, &id)["agent"];
    let instructions = agent["instructions"].as_str().unwrap();
    let (before, after) = instructions
        .split_once("\n\n__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__\n\n")
        .expect("the instructions keep the cache boundary");
    assert!(after.contains("app"), "the session part names the project");
    let tools = [
        "fs_read",
        "fs_list",
        "search",
        "fs_write",
        "fs_edit",
        "commands_list",
        "commands_run",
        "commands_output",
        "commands_stop",
        "agent_launch",
        "agent_send",
        "agent_status",
        "agent_read",
        "agent_stop",
        "report",
        "ask_user",
        "approval_answer",
    ]
    .map(|tool| format!("mcp__flottille__{tool}"));
    let opened = &user.received_requests("session/new")[0]["params"];
    assert_eq!(
        opened["_meta"],
        json!({"claudeCode": {"options": {
            "tools": [],
            "allowedTools": tools,
            "settingSources": [],
            "strictMcpConfig": true,
            "systemPrompt": {
                "type": "custom",
                "prompt": [before, "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__", after],
                "snapshot": true
            },
            "env": {
                "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
                "ENABLE_CLAUDEAI_MCP_SERVERS": "false",
                "ENABLE_TOOL_SEARCH": "false",
                "MCP_TOOL_TIMEOUT": "600000",
                "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT": "600000"
            }
        }}})
    );
    assert_eq!(agent["tools"], json!(tools));
    let servers = opened["mcpServers"].as_array().unwrap();
    assert_eq!(servers.len(), 1, "only Flottille's MCP server: {servers:?}");
    assert_eq!(servers[0]["name"], "flottille");
    assert_eq!(servers[0]["type"], "http");
    let initialize = &user.received_requests("initialize")[0]["params"];
    assert_eq!(initialize["protocolVersion"], 1);
    let capabilities = &initialize["clientCapabilities"];
    let none = |key: &str, off: Value| capabilities.get(key).is_none_or(|v| v == &off);
    assert!(
        none("fs", json!({"readTextFile": false, "writeTextFile": false})),
        "{capabilities}"
    );
    assert!(none("terminal", json!(false)), "{capabilities}");
    assert!(capabilities.get("subagents").is_none(), "{capabilities}");
}

#[test]
fn a_second_prompt_after_the_idle_release_restarts_the_process_and_resumes_the_session() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    user.fake_script(json!({"turns": [{"when": "again", "do": [{"say": "Back."}]}]}));
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    let first = events_until(&user, &id, after, "agent.released");
    assert_eq!(first.last().unwrap()["data"]["reason"], "idle");
    assert_eq!(show(&user, &id)["agent"]["state"], "idle");

    let resent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();

    let second = events_until(
        &user,
        &id,
        resent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );
    let started: Vec<&Value> = second
        .iter()
        .filter(|e| e["name"] == "agent.started")
        .collect();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0]["data"]["opened"], "resumed");
    let pids: Vec<u64> = user
        .received()
        .iter()
        .filter(|e| e["started"] == true)
        .map(|e| e["pid"].as_u64().unwrap())
        .collect();
    assert_eq!(pids.len(), 2);
    assert_ne!(pids[0], pids[1]);
    let native = show(&user, &id)["agent"]["native_id"].clone();
    let resumed = user.received_requests("session/resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["params"]["sessionId"], native);
    assert!(user.received_requests("session/load").is_empty());
    assert_eq!(user.received_requests("session/new").len(), 1);
    assert!(second.iter().any(|e| e["data"]["text"] == "Back."));
}

#[test]
fn a_load_replays_history_that_is_held_already_and_not_recorded_again() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    user.fake_script(json!({"resume": false, "turns": [
        {"when": "hello", "do": [{"say": "Hello!"}]},
        {"when": "quiet", "do": []}
    ]}));
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.released");
    let active = show(&user, &id)["agent"]["active_at"].as_u64().unwrap();

    let resent = user
        .call("agent.send", json!({"agent": id, "message": "quiet"}))
        .unwrap();
    let second = events_until(
        &user,
        &id,
        resent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );

    let started = second
        .iter()
        .find(|e| e["name"] == "agent.started")
        .unwrap();
    assert_eq!(started["data"]["opened"], "loaded");
    // The fake agent replayed its history; Flottille holds it already, as it happened.
    assert!(
        !second.iter().any(|e| e["data"]["replay"] == true
            || ["agent.user_message", "agent.message"].contains(&e["name"].as_str().unwrap())),
        "{second:?}"
    );
    let messages = show(&user, &id)["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == "agent.message")
        .count();
    assert_eq!(messages, 1, "Hello! once");
    assert_eq!(user.received_requests("session/load").len(), 1);
    assert_eq!(
        show(&user, &id)["agent"]["active_at"].as_u64().unwrap(),
        active,
        "the replay is no sign of activity"
    );
}

#[test]
fn a_cancel_ends_the_turn_with_cancelled() {
    let user = User::with_fake_agent();
    user.fake_script(
        json!({"turns": [{"when": "long", "do": [{"say": "Working on it"}, {"hang": true}]}]}),
    );
    project(&user);
    let (id, after) = run(&user, json!({"message": "a long task"}));
    events_until(&user, &id, after, "agent.message");

    let cancelled = user.call("agent.cancel", json!({"agent": id})).unwrap();

    assert_eq!(cancelled["cancelled"], true);
    let events = events_until(&user, &id, after, "agent.turn_ended");
    assert_eq!(events.last().unwrap()["data"]["stop_reason"], "cancelled");
    let cancels = user.received_requests("session/cancel");
    assert_eq!(cancels.len(), 1);
    assert!(cancels[0].get("id").is_none(), "a cancel is a notification");
    assert_eq!(show(&user, &id)["agent"]["state"], "idle");
}

#[test]
fn a_permission_request_for_a_tool_that_is_not_flottille_s_is_rejected() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "ask", "do": [
        {"permission": {"tool": "Bash", "title": "ls"}},
        {"permission": {"tool": "mcp__flottille__fs_list", "title": "fs_list"}},
        {"permission": {"tool": "mcp__other__fs_list", "title": "fs_list"}}
    ]}]}));
    project(&user);
    let (id, after) = run(&user, json!({"message": "ask"}));

    let events = events_until(&user, &id, after, "agent.turn_ended");

    let answers: Vec<Value> = user
        .received()
        .into_iter()
        .filter_map(|e| e.get("permission").cloned())
        .collect();
    assert_eq!(
        answers,
        [
            json!({"outcome": {"outcome": "selected", "optionId": "reject"}}),
            json!({"outcome": {"outcome": "selected", "optionId": "allow"}}),
            json!({"outcome": {"outcome": "selected", "optionId": "reject"}}),
        ]
    );
    let answered: Vec<(&str, bool)> = events
        .iter()
        .filter(|e| e["name"] == "agent.permission_answered")
        .map(|e| {
            (
                e["data"]["tool"].as_str().unwrap(),
                e["data"]["allowed"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        answered,
        [
            ("Bash", false),
            ("mcp__flottille__fs_list", true),
            ("mcp__other__fs_list", false)
        ]
    );
}

#[test]
fn a_permission_request_with_only_a_title_shaped_like_a_flottille_tool_is_rejected() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "ask", "do": [
        {"permission": {"title": "mcp__flottille__fs_list", "untyped": true}}
    ]}]}));
    project(&user);
    let (id, after) = run(&user, json!({"message": "ask"}));

    let events = events_until(&user, &id, after, "agent.turn_ended");

    let answers: Vec<Value> = user
        .received()
        .into_iter()
        .filter_map(|e| e.get("permission").cloned())
        .collect();
    assert_eq!(
        answers,
        [json!({"outcome": {"outcome": "selected", "optionId": "reject"}})]
    );
    let answered = events
        .iter()
        .find(|e| e["name"] == "agent.permission_answered")
        .unwrap();
    assert_eq!(answered["data"]["allowed"], false);
}

#[test]
fn a_death_is_detected_and_published() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "die", "do": [{"say": "Bye"}, {"die": 3}]}]}));
    project(&user);
    let (id, after) = run(&user, json!({"message": "die now"}));

    // Each death replaces the session, which dies again: the third fails the agent.
    let events = events_until(&user, &id, after, "agent.failed");

    let died: Vec<&Value> = events
        .iter()
        .filter(|e| e["name"] == "agent.died")
        .collect();
    assert_eq!(died.len(), 3);
    assert_eq!(died[0]["data"]["code"], 3);
    assert_eq!(died[0]["data"]["during_turn"], true);
    assert_eq!(
        events
            .iter()
            .filter(|e| e["name"] == "agent.replaced")
            .count(),
        2
    );
    let ended = events
        .iter()
        .find(|e| e["name"] == "agent.turn_ended")
        .unwrap();
    assert_eq!(ended["data"]["stop_reason"], "interrupted");
    let agent = &show(&user, &id)["agent"];
    assert_eq!(agent["state"], "failed");
    assert!(
        agent["state_reason"]
            .as_str()
            .unwrap()
            .contains("exit code 3")
    );
    let refused = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap_err();
    assert_eq!(refused.0, "refused");
}

#[test]
fn a_child_left_by_an_adapter_that_died_does_not_outlive_it() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "die", "do": [{"child": true}, {"die": 3}]}]}));
    project(&user);
    let (id, after) = run(&user, json!({"message": "die now"}));

    events_until(&user, &id, after, "agent.died");

    let child = user
        .received()
        .iter()
        .find_map(|e| e["child"].as_u64())
        .expect("the fake agent started a child");
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(child) {
        assert!(
            Instant::now() < deadline,
            "the child of the dead adapter still runs (pid {child})"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_daemon_stopping_does_not_leave_the_group_of_an_agent_that_would_not_stop() {
    let user = User::with_fake_agent();
    // Stuck while it opens its session, it never sees the order to stop.
    user.fake_script(json!({"stall": true, "child": true}));
    project(&user);
    let daemon = {
        let (id, after) = run(&user, json!({"message": "hello"}));
        events_until(&user, &id, after, "agent.created");
        user.status()["pid"].as_u64().unwrap()
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let child = loop {
        if let Some(pid) = user.received().iter().find_map(|e| e["child"].as_u64()) {
            break pid;
        }
        assert!(Instant::now() < deadline, "the fake agent did not start");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(alive(child));

    user.run(&["daemon", "stop"]);

    let deadline = Instant::now() + Duration::from_secs(20);
    while alive(daemon) || alive(child) {
        assert!(
            Instant::now() < deadline,
            "the daemon ({daemon}) or the agent's child ({child}) still runs"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_model_and_an_effort_are_set_and_checked_and_a_model_not_offered_fails() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}));
    project(&user);

    let (id, after) = run(
        &user,
        json!({"message": "hello", "model": "fake-large", "effort": "high"}),
    );
    events_until(&user, &id, after, "agent.turn_ended");
    let (missing, missing_after) = run(&user, json!({"message": "hello", "model": "fake-huge"}));
    let failed = events_until(&user, &missing, missing_after, "agent.failed");

    let agent = &show(&user, &id)["agent"];
    assert_eq!(agent["chosen"]["model"], "fake-large");
    assert_eq!(agent["taken"]["model"], "fake-large");
    assert_eq!(agent["taken"]["effort"], "high");
    let set: Vec<(String, String)> = user
        .received_requests("session/set_config_option")
        .iter()
        .map(|r| {
            (
                r["params"]["configId"].as_str().unwrap().to_owned(),
                r["params"]["value"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        set[..2],
        [
            ("model".into(), "fake-large".into()),
            ("effort".into(), "high".into())
        ]
    );
    let reason = failed.last().unwrap()["data"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("fake-huge") && reason.contains("fake-small, fake-large"),
        "{reason}"
    );
    assert_eq!(show(&user, &missing)["agent"]["state"], "failed");
}

#[test]
fn an_agent_in_a_workspace_works_in_its_folder() {
    let user = User::with_fake_agent();
    project(&user);
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();

    let (id, after) = run(&user, json!({"message": "hello", "workspace": "login"}));
    events_until(&user, &id, after, "agent.turn_ended");

    assert_eq!(
        user.received_requests("session/new")[0]["params"]["cwd"],
        workspace["folder"]
    );
    assert_eq!(show(&user, &id)["agent"]["workspace"], "login");
    let missing = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "nope", "message": "hi"}),
        )
        .unwrap_err();
    assert_eq!(missing.0, "not-found");
}

fn alive(pid: u64) -> bool {
    // A zombie is gone for this purpose: it runs nothing.
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat
            .rsplit_once(')')
            .unwrap()
            .1
            .trim_start()
            .starts_with('Z'),
        Err(_) => false,
    }
}

#[test]
fn orphan_processes_are_killed_at_daemon_start() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"linger": true}));
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");
    let fake = user
        .received()
        .iter()
        .find(|e| e["started"] == true)
        .unwrap()["pid"]
        .as_u64()
        .unwrap();
    let daemon = user.status()["pid"].as_u64().unwrap();

    // A crash: the daemon goes without stopping its agents, which linger.
    // SAFETY: kill has no memory precondition.
    unsafe { libc::kill(daemon as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(daemon) {
        assert!(Instant::now() < deadline, "the daemon did not die");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(alive(fake), "the fake agent lingers as an orphan");
    user.status();

    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(fake) {
        assert!(Instant::now() < deadline, "the orphan was not killed");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn agent_show_prints_the_conversation() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "hello", "do": [{"say": "Hello there!"}]}]}));
    project(&user);
    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let output = user.run(&["agent", "show", &id]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("app"), "{stdout}");
    assert!(stdout.contains("hello"), "{stdout}");
    assert!(stdout.contains("Hello there!"), "{stdout}");
    let json: Value =
        serde_json::from_slice(&user.run(&["agent", "show", &id, "--json"]).stdout).unwrap();
    assert_eq!(json["agent"]["id"], id.as_str());
}
