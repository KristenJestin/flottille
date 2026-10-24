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
    let events = shown["events"].as_array().unwrap();
    assert_eq!(
        names(events),
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
    assert_eq!(events[2]["data"]["text"], "hello");
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
fn a_load_replays_history_flagged_replay_and_not_counted_as_activity() {
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
    let replayed: Vec<&Value> = second
        .iter()
        .take_while(|e| e["name"] != "agent.started")
        .filter(|e| e["data"]["replay"] == true)
        .collect();
    assert_eq!(
        names(&replayed.into_iter().cloned().collect::<Vec<_>>()),
        ["agent.user_message", "agent.message"]
    );
    assert!(
        second
            .iter()
            .filter(
                |e| ["agent.user_message", "agent.message"].contains(&e["name"].as_str().unwrap())
            )
            .all(|e| e["data"]["replay"] == true),
        "only the replay brings messages here: {second:?}"
    );
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
fn a_death_is_detected_and_published() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "die", "do": [{"say": "Bye"}, {"die": 3}]}]}));
    project(&user);
    let (id, after) = run(&user, json!({"message": "die now"}));

    let events = events_until(&user, &id, after, "agent.died");

    let died = events.last().unwrap();
    assert_eq!(died["data"]["code"], 3);
    assert_eq!(died["data"]["during_turn"], true);
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

/// The adapter as `npm install` leaves it, and a `node` that runs the fake agent in its place,
/// writing down how it was started. The `bin` folder to put first in `PATH`, the folder of the
/// package, and the file the arguments are written to (the environment goes beside it, `.env`).
fn adapter_shim(user: &User) -> (PathBuf, PathBuf, PathBuf) {
    let package = user
        .path("data/flottille/adapters/claude/node_modules/@agentclientprotocol/claude-agent-acp");
    std::fs::create_dir_all(package.join("dist")).unwrap();
    std::fs::write(
        package.join("package.json"),
        json!({"name": "@agentclientprotocol/claude-agent-acp", "version": "0.95.0",
            "bin": {"claude-agent-acp": "dist/index.js"}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(package.join("dist/index.js"), "").unwrap();
    let bin = user.path("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(user.path("fake")).unwrap();
    let started = user.path("fake/node-started");
    write_script(
        &bin.join("node"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {started}\nenv > {started}.env\nexec {fake}\n",
            started = started.display(),
            fake = common::fake_agent().display()
        ),
    );
    write_script(
        &bin.join("claude"),
        "#!/bin/sh\necho '2.1.300 (Claude Code)'\n",
    );
    (bin, package, started)
}

#[test]
fn the_adapter_is_started_with_node_and_pointed_at_the_user_s_claude() {
    let user = User::new();
    let (bin, package, started) = adapter_shim(&user);
    let fake_dir = user.path("fake");
    let user = user
        .with_env("PATH", &format!("{}:/usr/bin:/bin", bin.display()))
        .with_env("FLOTTILLE_FAKE_AGENT_DIR", fake_dir.to_str().unwrap())
        .with_env("CLAUDE_CODE_PROMPT_CACHE_TTL", "5m")
        .with_env("FORCE_PROMPT_CACHING_5M", "1");
    project(&user);

    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let args = std::fs::read_to_string(&started).unwrap();
    assert_eq!(args.trim(), package.join("dist/index.js").to_str().unwrap());
    let env = std::fs::read_to_string(started.with_extension("env")).unwrap();
    assert!(
        env.lines()
            .any(|l| l == format!("CLAUDE_CODE_EXECUTABLE={}", bin.join("claude").display())),
        "{env}"
    );
    assert!(!env.contains("CLAUDE_CODE_PROMPT_CACHE_TTL"), "{env}");
    assert!(!env.contains("FORCE_PROMPT_CACHING_5M"), "{env}");
}

#[test]
fn the_adapter_gets_the_agent_s_sign_in_and_none_of_the_daemon_s_other_secrets() {
    let user = User::new();
    let (bin, _, started) = adapter_shim(&user);
    let fake_dir = user.path("fake");
    let user = user
        .with_env("PATH", &format!("{}:/usr/bin:/bin", bin.display()))
        .with_env("FLOTTILLE_FAKE_AGENT_DIR", fake_dir.to_str().unwrap())
        .with_env("GH_TOKEN", "forge-secret-1")
        .with_env("DEPLOY_PASSWORD", "forge-secret-2")
        .with_env("ANTHROPIC_MODEL", "some-model")
        .with_env("DISABLE_TELEMETRY", "1")
        .with_env("ANTHROPIC_API_KEY", "sign-in-key")
        .with_env("HTTPS_PROXY", "http://proxy.invalid:3128");
    project(&user);

    let (id, after) = run(&user, json!({"message": "hello"}));
    events_until(&user, &id, after, "agent.turn_ended");

    let env = std::fs::read_to_string(started.with_extension("env")).unwrap();
    for gone in [
        "GH_TOKEN=",
        "DEPLOY_PASSWORD=",
        "ANTHROPIC_MODEL=",
        "DISABLE_TELEMETRY=",
    ] {
        assert!(!env.contains(gone), "{gone} reached the agent: {env}");
    }
    for kept in [
        "ANTHROPIC_API_KEY=sign-in-key",
        "HTTPS_PROXY=http://proxy.invalid:3128",
    ] {
        assert!(env.lines().any(|l| l == kept), "{kept} is missing: {env}");
    }
}

fn write_script(path: &std::path::Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn an_agent_is_refused_while_the_adapter_is_not_installed() {
    let user = User::new();
    project(&user);

    let refused = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap_err();

    assert_eq!(refused.0, "refused");
    assert!(
        refused.1.contains("flottille agent install"),
        "{}",
        refused.1
    );
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
