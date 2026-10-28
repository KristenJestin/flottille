//! The scenarios of agents run over ACP, end to end: the daemon starts Flottille's Claude bridge,
//! which drives the fake `claude` (`crates/flottille-fake-claude`). No real agent ever runs here.

mod common;

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

use common::User;
use serde_json::{Value, json};

/// A project `app` over a throwaway repository with one commit.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    folder
}

fn run(user: &User, message: &str) -> (String, u64) {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": message}))
        .unwrap();
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        answer["after"].as_u64().unwrap(),
    )
}

fn of(agent: &str) -> impl Fn(&Value) -> bool + '_ {
    move |event: &Value| event["data"]["agent"] == agent
}

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

fn argv(start: &Value) -> Vec<&str> {
    start["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect()
}

#[test]
fn through_the_bridge_a_session_is_opened_and_a_prompt_gets_its_reply() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "hello", "do": [
        {"think": "a greeting"},
        {"say": "Hello", "message": "msg_a"}, {"say": " there!", "message": "msg_a"}
    ]}]}));
    let folder = project(&user);

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
    let agent = &shown["agent"];
    assert_eq!(agent["state"], "idle");
    let events: Vec<Value> = shown["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| !e["name"].as_str().unwrap().starts_with("delivery."))
        .cloned()
        .collect();
    let names = names(&events);
    assert_eq!(
        names[..5],
        [
            "agent.created",
            "agent.started",
            "agent.prompted",
            "agent.thought",
            "agent.message",
        ]
    );
    assert_eq!(names.last(), Some(&"agent.turn_ended"));
    assert!(names.contains(&"agent.usage"), "{names:?}");
    assert_eq!(events[1]["data"]["opened"], "new");
    // Its two pieces, short enough to be held back whole until the message ends.
    assert_eq!(events[4]["data"]["text"], "Hello there!");
    assert_eq!(events.last().unwrap()["data"]["stop_reason"], "end_turn");
    // The session the daemon knows is the one `claude` was started on, in the project's folder.
    let start = &user.claude_starts()[0];
    let native = agent["native_id"].as_str().unwrap();
    assert!(argv(start).contains(&format!("--session-id={native}").as_str()));
    assert_eq!(start["cwd"], folder.to_str().unwrap());
    let config = &start["mcp_config"]["mcpServers"]["flottille"];
    assert_eq!(config["type"], "http");
    assert!(
        config["headers"]["Authorization"]
            .as_str()
            .unwrap()
            .starts_with("Bearer ")
    );
    let tools = argv(start)
        .into_iter()
        .find_map(|a| a.strip_prefix("--allowedTools="))
        .unwrap()
        .split(',')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(json!(tools), agent["tools"]);
}

#[test]
fn through_the_bridge_claude_gets_the_sign_in_and_none_of_the_daemon_s_secrets_or_settings() {
    let user = User::with_fake_claude()
        .with_env("GH_TOKEN", "forge-secret-1")
        .with_env("DEPLOY_PASSWORD", "forge-secret-2")
        .with_env("ANTHROPIC_MODEL", "some-model")
        .with_env("CLAUDE_CODE_PROMPT_CACHE_TTL", "5m")
        .with_env("FORCE_PROMPT_CACHING_5M", "1")
        .with_env("ANTHROPIC_API_KEY", "sign-in-key")
        .with_env("HTTPS_PROXY", "http://proxy.invalid:3128");
    project(&user);

    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");

    let env = &user.claude_starts()[0]["env"];
    for gone in [
        "GH_TOKEN",
        "DEPLOY_PASSWORD",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_PROMPT_CACHE_TTL",
        "FORCE_PROMPT_CACHING_5M",
    ] {
        assert!(env.get(gone).is_none(), "{gone} reached claude");
    }
    assert_eq!(env["ANTHROPIC_API_KEY"], "sign-in-key");
    assert_eq!(env["HTTPS_PROXY"], "http://proxy.invalid:3128");
    assert_eq!(env["CLAUDE_CODE_DISABLE_AUTO_MEMORY"], "1");
}

#[test]
fn through_the_bridge_a_second_prompt_after_the_idle_release_resumes_the_session() {
    let user = User::with_fake_claude().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    user.claude_script(json!({"turns": [{"when": "again", "do": [{"say": "Back."}]}]}));
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.released");

    let resent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
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
    assert_eq!(started["data"]["opened"], "resumed");
    assert!(second.iter().any(|e| e["data"]["text"] == "Back."));
    let native = show(&user, &id)["agent"]["native_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let starts = user.claude_starts();
    assert_eq!(starts.len(), 2);
    assert!(argv(&starts[1]).contains(&format!("--resume={native}").as_str()));
}

#[test]
fn through_the_bridge_a_load_replays_history_that_is_not_recorded_again() {
    let user = User::with_fake_claude()
        .with_env("FLOTTILLE_IDLE_RELEASE_MS", "300")
        .with_env("FLOTTILLE_BRIDGE_NO_RESUME", "1");
    user.claude_script(json!({"turns": [
        {"when": "hello", "do": [{"say": "Hello!"}]},
        {"when": "quiet", "do": []}
    ]}));
    project(&user);
    let (id, after) = run(&user, "hello");
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
    assert_eq!(
        show(&user, &id)["agent"]["active_at"].as_u64().unwrap(),
        active,
        "the replay is no sign of activity"
    );
}

#[test]
fn through_the_bridge_a_cancel_ends_the_turn_with_cancelled() {
    let user = User::with_fake_claude();
    user.claude_script(
        // Long enough for its start to be shown before the turn ends.
        json!({"turns": [{"when": "long", "do": [{"say": "Working on it. ".repeat(20)}, {"hang": true}]}]}),
    );
    project(&user);
    let (id, after) = run(&user, "a long task");
    events_until(&user, &id, after, "agent.message");

    let cancelled = user.call("agent.cancel", json!({"agent": id})).unwrap();

    assert_eq!(cancelled["cancelled"], true);
    let events = events_until(&user, &id, after, "agent.turn_ended");
    assert_eq!(events.last().unwrap()["data"]["stop_reason"], "cancelled");
    assert_eq!(show(&user, &id)["agent"]["state"], "idle");
}

#[test]
fn through_the_bridge_a_permission_request_for_a_tool_that_is_not_flottille_s_is_refused() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "ask", "do": [
        {"permission": {"tool": "Bash", "input": {"command": "ls"}}},
        {"permission": {"tool": "mcp__flottille__fs_list", "input": {"path": "."}}},
        {"permission": {"tool": "mcp__other__fs_list", "input": {}}}
    ]}]}));
    project(&user);
    let (id, after) = run(&user, "ask");

    let events = events_until(&user, &id, after, "agent.turn_ended");

    let behaviours: Vec<Value> = user
        .claude_received()
        .into_iter()
        .filter_map(|e| e.get("permission").cloned())
        .map(|answer| answer["response"]["behavior"].clone())
        .collect();
    assert_eq!(behaviours, [json!("deny"), json!("allow"), json!("deny")]);
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
fn through_the_bridge_a_death_of_claude_is_the_agent_s_death() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "die", "do": [{"say": "Bye"}, {"die": 3}]}]}));
    project(&user);
    let (id, after) = run(&user, "die now");

    let events = events_until(&user, &id, after, "agent.failed");

    let died: Vec<&Value> = events
        .iter()
        .filter(|e| e["name"] == "agent.died")
        .collect();
    assert_eq!(died.len(), 3);
    assert_eq!(died[0]["data"]["code"], 3);
    assert_eq!(died[0]["data"]["during_turn"], true);
    let agent = &show(&user, &id)["agent"];
    assert_eq!(agent["state"], "failed");
    assert!(
        agent["state_reason"]
            .as_str()
            .unwrap()
            .contains("exit code 3")
    );
}

#[test]
fn through_the_bridge_a_provider_limit_is_reported_and_the_agent_lives_on() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "work", "do": [{"limit": "five_hour"}]}]}));
    project(&user);
    let (id, after) = run(&user, "work");

    let events = events_until(&user, &id, after, "agent.turn_ended");

    let limit = events
        .iter()
        .find(|e| e["name"] == "agent.provider_limit")
        .expect("an agent.provider_limit");
    assert!(
        limit["data"]["message"]
            .as_str()
            .unwrap()
            .contains("hit your limit"),
        "{limit}"
    );
    assert_eq!(events.last().unwrap()["data"]["stop_reason"], "end_turn");
    assert_eq!(show(&user, &id)["agent"]["state"], "idle");
}

#[test]
fn through_the_bridge_a_provider_error_is_reported_and_the_agent_lives_on() {
    let user = User::with_fake_claude();
    user.claude_script(
        json!({"turns": [{"when": "work", "do": [], "error": "API Error: 500 the provider failed"}]}),
    );
    project(&user);
    let (id, after) = run(&user, "work");

    let events = events_until(&user, &id, after, "agent.turn_ended");

    let error = events
        .iter()
        .find(|e| e["name"] == "agent.provider_error")
        .expect("an agent.provider_error");
    assert!(
        error["data"]["message"]
            .as_str()
            .unwrap()
            .contains("the provider failed"),
        "{error}"
    );
    assert_eq!(show(&user, &id)["agent"]["state"], "idle");
}

#[test]
fn through_the_bridge_an_agent_takes_the_model_and_the_effort_asked() {
    let user = User::with_fake_claude();
    project(&user);
    let answer = user
        .call(
            "agent.start",
            json!({"project": "app", "message": "hello", "model": "fable", "effort": "max"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();

    events_until(
        &user,
        &id,
        answer["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );

    let taken = &show(&user, &id)["agent"]["taken"];
    assert_eq!(taken["model"], "fable");
    assert_eq!(taken["effort"], "max");
    let asked: Vec<Value> = user
        .claude_received()
        .into_iter()
        .filter_map(|e| e.get("received").cloned())
        .filter(|m| m["type"] == "control_request")
        .map(|m| m["request"].clone())
        .filter(|r| r["subtype"] != "initialize")
        .collect();
    assert_eq!(
        asked,
        [
            json!({"subtype": "set_model", "model": "fable"}),
            json!({"subtype": "apply_flag_settings", "settings": {"effortLevel": "max"}})
        ]
    );
}

#[test]
fn through_the_bridge_claude_s_models_are_listed_by_their_names_once_a_session_opened() {
    let user = User::with_fake_claude();
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");

    let listed = user.call("provider.list", Value::Null).unwrap();
    let claude = listed["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "claude")
        .unwrap()
        .clone();
    let models = claude["models"].as_array().unwrap();
    let named: Vec<(&str, &str)> = models
        .iter()
        .map(|m| (m["id"].as_str().unwrap(), m["name"].as_str().unwrap()))
        .collect();
    assert_eq!(
        named,
        [
            ("default", "Default (recommended)"),
            ("fable", "Fable"),
            ("haiku", "Haiku")
        ]
    );
    assert_eq!(models[0]["default"], true);
    assert_eq!(claude["models_outdated"], false);
}

#[test]
fn an_agent_asked_for_a_mode_is_refused_with_a_sentence() {
    let user = User::with_fake_claude();
    project(&user);

    let refused = user
        .call(
            "agent.start",
            json!({"project": "app", "message": "hello", "mode": "plan"}),
        )
        .unwrap_err();

    assert_eq!(refused.0, "refused");
    assert!(refused.1.contains("mode"), "{}", refused.1);
    let listed = user.call("agent.list", Value::Null).unwrap();
    assert_eq!(listed["agents"], json!([]), "an agent was created");
    assert!(user.claude_starts().is_empty());
}

/// The bridge's history of the agent's session, once its session is open.
fn history(user: &User, agent: &str) -> PathBuf {
    let native = show(user, agent)["agent"]["native_id"]
        .as_str()
        .unwrap()
        .to_owned();
    user.path(&format!("state/flottille/bridge/claude/{native}.jsonl"))
}

/// Whether `gone` comes true within 10 s.
fn within_10_s(gone: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !gone() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    gone()
}

#[test]
fn through_the_bridge_a_session_s_history_goes_with_its_replacement() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "boom", "do": [{"die": 3}]}]}));
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    let first = history(&user, &id);
    assert!(first.exists());

    // Given back, the message kills each new session too, until the agent fails.
    let sent = user
        .call("agent.send", json!({"agent": id, "message": "boom"}))
        .unwrap();
    let after = sent["after"].as_u64().unwrap();
    events_until(&user, &id, after, "agent.replaced");
    assert!(!first.exists(), "the replaced session's history is removed");
    events_until(&user, &id, after, "agent.failed");

    let folder = user.path("state/flottille/bridge/claude");
    assert!(within_10_s(|| std::fs::read_dir(&folder)
        .unwrap()
        .next()
        .is_none()));
}

#[test]
fn through_the_bridge_a_stopped_root_keeps_its_history_and_resumes_on_it() {
    let user = User::with_fake_claude();
    project(&user);
    let (id, after) = run(&user, "hello");
    events_until(&user, &id, after, "agent.turn_ended");
    let history = history(&user, &id);
    assert!(history.exists());

    user.call("agent.stop", json!({"agent": id})).unwrap();
    events_until(&user, &id, after, "agent.ended");
    // Its process is gone, and what would have removed the history ran with it.
    assert!(!within_10_s(|| !history.exists()), "kept to be resumed");

    user.call("agent.resume", json!({"agent": id})).unwrap();
    let sent = user
        .call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    let events = events_until(
        &user,
        &id,
        sent["after"].as_u64().unwrap(),
        "agent.turn_ended",
    );
    let started = events
        .iter()
        .find(|e| e["name"] == "agent.started")
        .unwrap();
    assert_eq!(started["data"]["opened"], "resumed");
}

#[test]
fn through_the_bridge_an_agent_runs_on_a_path_of_claude_and_git_alone() {
    let user = User::with_fake_claude();
    // No `node`, `npm` or anything else of the machine's: what runs needs nothing more.
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    let path = user.path("bin");
    let user = user.with_env("PATH", path.to_str().unwrap());
    user.claude_script(json!({"turns": [{"when": "hello", "do": [{"say": "Hello!"}]}]}));
    project(&user);
    let (id, after) = run(&user, "hello");

    let events = events_until(&user, &id, after, "agent.turn_ended");

    assert!(events.iter().any(|e| e["data"]["text"] == "Hello!"));
    assert_eq!(events.last().unwrap()["data"]["stop_reason"], "end_turn");
    assert_eq!(
        user.claude_starts()[0]["env"]["PATH"],
        path.to_str().unwrap()
    );
}

/// The MCP configuration files of the bridges, with the token each holds.
fn token_files(user: &User) -> Vec<PathBuf> {
    match std::fs::read_dir(user.path("runtime/flottille/bridge")) {
        Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn through_the_bridge_the_token_file_of_a_killed_bridge_is_removed() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "long", "do": [{"hang": true}]}]}));
    project(&user);
    let (id, after) = run(&user, "a long task");
    events_until(&user, &id, after, "agent.prompted");
    let files = token_files(&user);
    assert_eq!(files.len(), 1, "{files:?}");
    let pid: i32 = files[0]
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_suffix(".mcp.json")
        .unwrap()
        .parse()
        .unwrap();

    // SAFETY: a signal to the bridge this test's daemon started.
    unsafe { libc::kill(pid, libc::SIGKILL) };

    events_until(&user, &id, after, "agent.died");
    assert!(within_10_s(|| !files[0].exists()));
}

#[test]
fn a_token_file_a_bridge_left_is_removed_when_the_daemon_starts() {
    let user = User::with_fake_claude();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(user.path("runtime/flottille/bridge"))
        .unwrap();
    let left = user.path("runtime/flottille/bridge/4242.mcp.json");
    std::fs::write(&left, "{}").unwrap();

    user.status();

    assert!(!left.exists());
}

#[test]
fn an_agent_is_refused_while_claude_is_not_on_path() {
    let user = User::new();
    // `git` alone: whatever else the machine has, no `claude` can be found.
    std::fs::create_dir(user.path("bin")).unwrap();
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    let path = user.path("bin");
    let user = user.with_env("PATH", path.to_str().unwrap());
    project(&user);

    let refused = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap_err();

    assert_eq!(refused.0, "refused");
    assert!(refused.1.contains("claude"), "{}", refused.1);
    assert!(
        !refused.1.contains("flottille agent install"),
        "{}",
        refused.1
    );
}

#[test]
fn through_the_bridge_a_reply_split_by_a_tool_served_reads_as_one_line() {
    // The first piece ends on the start of a secret, which is held back: what came before it is
    // recorded, then the tool, then the rest.
    let user = User::with_fake_claude().with_env("API_TOKEN", "s3cr3t-token-value");
    user.claude_script(json!({"turns": [{"when": "go", "do": [
        {"say": "I will look at the files now, the token is s3cr3t-", "message": "msg_a"},
        {"mcp": {"tool": "fs_list", "arguments": {}}},
        {"say": "token-value, keep it quiet", "message": "msg_a"}
    ]}]}));
    project(&user);
    let (id, after) = run(&user, "go");
    events_until(&user, &id, after, "agent.turn_ended");

    let page = user
        .call("agent.read", json!({"agent": id, "after": 0}))
        .unwrap();

    let lines: Vec<&str> = page["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect();
    let text = lines.join("\n");
    // The tool was served between the two pieces, and they are still one line.
    assert!(lines.iter().any(|l| l.contains("fs_list (")), "{text}");
    let replies: Vec<&str> = lines
        .iter()
        .filter_map(|line| Some(line.split_once(" said: ")?.1))
        .collect();
    assert_eq!(
        replies,
        ["I will look at the files now, the token is [masked], keep it quiet"],
        "{text}"
    );
}
