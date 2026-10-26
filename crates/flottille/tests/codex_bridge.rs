//! Flottille's Codex bridge (`flottille bridge codex`), driven over ACP as the daemon drives it,
//! in front of the fake `codex` (`crates/flottille-fake-codex`). No real `codex` ever runs here.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::User;
use serde_json::{Value, json};

/// One bridge process, its ACP messages read line by line.
struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<Value>,
    next: u64,
}

impl Bridge {
    fn start(user: &User) -> Bridge {
        let mut child = user
            .command(&["bridge", "codex"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(serde_json::from_str(&line).unwrap()).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        let mut bridge = Bridge {
            child,
            stdin,
            lines,
            next: 0,
        };
        let (initialized, _) = bridge.request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {},
                "clientInfo": {"name": "test", "version": "0"}}),
        );
        let initialized = initialized.unwrap();
        assert_eq!(initialized["protocolVersion"], 1);
        assert_eq!(
            initialized["agentCapabilities"]["sessionCapabilities"],
            json!({"resume": {}})
        );
        bridge
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn next(&self) -> Value {
        self.lines
            .recv_timeout(Duration::from_secs(10))
            .expect("no message from the bridge within 10 s")
    }

    /// Sends a request and reads up to its answer: its result or its error, and every message
    /// before it.
    fn request(&mut self, method: &str, params: Value) -> (Result<Value, Value>, Vec<Value>) {
        self.next += 1;
        let id = self.next;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut seen = Vec::new();
        loop {
            let message = self.next();
            assert!(
                message.get("method").is_none() || message.get("id").is_none(),
                "the bridge asked {message}"
            );
            if message.get("method").is_none() && message["id"] == id {
                return match message.get("error") {
                    Some(error) => (Err(error.clone()), seen),
                    None => (Ok(message["result"].clone()), seen),
                };
            }
            seen.push(message);
        }
    }

    fn open(&mut self, user: &User, tools: &[&str]) -> (String, Value) {
        let (opened, _) = self.request("session/new", new_session(user, tools));
        let opened = opened.unwrap();
        (opened["sessionId"].as_str().unwrap().to_owned(), opened)
    }

    fn prompt(&mut self, session: &str, text: &str) -> (Result<Value, Value>, Vec<Value>) {
        self.request(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]}),
        )
    }

    /// Sends a prompt without waiting for its answer: its id.
    fn prompt_later(&mut self, session: &str, text: &str) -> u64 {
        self.next += 1;
        let id = self.next;
        self.send(
            json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]}}),
        );
        id
    }

    fn answer_to(&self, id: u64) -> Value {
        loop {
            let message = self.next();
            if message["id"] == id {
                return message;
            }
        }
    }

    fn close(mut self) -> Option<i32> {
        drop(self.stdin.take());
        self.child.wait().unwrap().code()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The options the daemon gives every Codex session (`codex::session_meta`).
fn meta(tools: &[&str]) -> Value {
    json!({"codex": {"instructions": "You are the agent of zebra.", "tools": tools}})
}

fn server() -> Value {
    json!({"type": "http", "name": "flottille", "url": "http://127.0.0.1:9/mcp",
        "headers": [{"name": "Authorization", "value": "Bearer invented-token-1"}]})
}

fn new_session(user: &User, tools: &[&str]) -> Value {
    std::fs::create_dir_all(user.path("work")).unwrap();
    json!({"cwd": user.path("work"), "mcpServers": [server()], "_meta": meta(tools)})
}

fn updates(seen: &[Value]) -> Vec<Value> {
    seen.iter()
        .filter(|m| m["method"] == "session/update")
        .map(|m| m["params"]["update"].clone())
        .collect()
}

/// The messages the fake `codex` read, of one method.
fn sent(user: &User, method: &str) -> Vec<Value> {
    user.codex_received()
        .into_iter()
        .filter_map(|e| e.get("received").cloned())
        .filter(|m| m["method"] == method)
        .collect()
}

fn starts(user: &User) -> Vec<Value> {
    user.codex_received()
        .into_iter()
        .filter_map(|e| e.get("started").cloned())
        .collect()
}

fn failures(seen: &[Value]) -> Vec<(String, String, String)> {
    updates(seen)
        .iter()
        .filter_map(|u| {
            let failure = &u["_meta"]["jetbrains"]["air"]["sessionFailure"];
            failure.is_object().then(|| {
                let text = |key: &str| failure[key].as_str().unwrap_or_default().to_owned();
                (text("severity"), text("category"), text("message"))
            })
        })
        .collect()
}

#[test]
fn the_bridge_starts_codex_bare_and_its_thread_with_no_environment_and_flottille_s_server_only() {
    let user = User::with_fake_codex();
    let mut bridge = Bridge::start(&user);

    let (session, _) = bridge.open(
        &user,
        &["mcp__flottille__fs_read", "mcp__flottille__report"],
    );

    let started = &starts(&user)[0];
    let argv: Vec<&str> = started["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(argv[..2], ["-c", "approval_policy=\"never\""]);
    assert_eq!(
        argv[argv.len() - 3..],
        ["app-server", "--listen", "stdio://"]
    );
    assert!(argv.contains(&"features.shell_tool=false"));
    assert!(argv.contains(&"agents.enabled=false"));
    assert_eq!(started["cwd"], user.path("work").to_str().unwrap());
    let data = user.path("data/flottille/agents/codex");
    let env = &started["env"];
    for (name, value) in [
        ("CODEX_HOME", data.join("codex").to_str().unwrap()),
        ("HOME", data.join("home").to_str().unwrap()),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED", "1"),
    ] {
        assert_eq!(env[name], value, "{name}");
    }
    // The token goes through Codex's input only, never its command line or its environment.
    assert!(!started.to_string().contains("invented-token-1"));
    assert_eq!(
        sent(&user, "initialize")[0]["params"]["capabilities"],
        json!({"experimentalApi": true})
    );
    assert_eq!(sent(&user, "initialized").len(), 1);
    assert_eq!(sent(&user, "account/read").len(), 1);
    let thread = &sent(&user, "thread/start")[0]["params"];
    assert_eq!(
        thread,
        &json!({
            "cwd": user.path("work"),
            "approvalPolicy": "never",
            "sandbox": "read-only",
            "baseInstructions": "You are the agent of zebra.",
            "environments": [],
            "config": {"mcp_servers": {"flottille": {
                "url": "http://127.0.0.1:9/mcp",
                "http_headers": {"Authorization": "Bearer invented-token-1"},
                "enabled_tools": ["fs_read", "report"],
                "required": true,
                "default_tools_approval_mode": "approve",
                "omit_tools_from": ["deferred", "code_mode"],
                "tool_timeout_sec": 600,
            }}},
        })
    );
    assert!(session.starts_with("thread-"), "{session}");
    assert_eq!(bridge.close(), Some(0));
}

#[test]
fn options_that_are_not_bare_are_refused() {
    let user = User::with_fake_codex();
    let mut bridge = Bridge::start(&user);
    let mut cases = Vec::new();
    for (key, value) in [
        ("sandbox", json!("danger-full-access")),
        ("tools", json!(["shell"])),
    ] {
        let mut params = new_session(&user, &["mcp__flottille__report"]);
        params["_meta"]["codex"][key] = value;
        cases.push(params);
    }
    let mut no_meta = new_session(&user, &[]);
    no_meta["_meta"] = json!({});
    cases.push(no_meta);
    let mut another_server = new_session(&user, &[]);
    another_server["mcpServers"] = json!([server(), {"type": "http", "name": "more", "url": "u"}]);
    cases.push(another_server);

    for params in cases {
        let (refused, _) = bridge.request("session/new", params.clone());

        assert!(refused.is_err(), "{params}");
    }
    assert!(starts(&user).is_empty());
}

#[test]
fn a_prompt_streams_text_thoughts_and_tool_calls_then_the_usage_and_its_stop_reason() {
    let user = User::with_fake_codex();
    user.codex_script(json!({"turns": [{"when": "hello", "do": [
        {"think": "a greeting"},
        {"tool": {"id": "call-1", "tool": "fs_read", "arguments": {"path": "README.md"},
            "result": [{"type": "text", "text": "# App"}]}},
        {"tool": {"id": "call-2", "tool": "fs_read", "arguments": {"path": "nope"},
            "error": "no such file"}},
        {"say": "Hello!"},
    ]}]}));
    let mut bridge = Bridge::start(&user);
    let (session, _) = bridge.open(&user, &["mcp__flottille__fs_read"]);

    let (answer, seen) = bridge.prompt(&session, "hello there");

    let answer = answer.unwrap();
    assert_eq!(answer["stopReason"], "end_turn");
    assert_eq!(
        answer["usage"],
        json!({"inputTokens": 200, "outputTokens": 100, "cachedReadTokens": 1000,
            "totalTokens": 1300})
    );
    let updates = updates(&seen);
    let kinds: Vec<&str> = updates
        .iter()
        .map(|u| u["sessionUpdate"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "agent_thought_chunk",
            "tool_call",
            "tool_call_update",
            "tool_call",
            "tool_call_update",
            "agent_message_chunk",
            "usage_update",
        ]
    );
    assert_eq!(updates[0]["content"]["text"], "a greeting");
    assert_eq!(
        updates[1],
        json!({"sessionUpdate": "tool_call", "toolCallId": "call-1",
            "title": "mcp__flottille__fs_read", "kind": "other", "status": "in_progress",
            "rawInput": {"path": "README.md"}})
    );
    assert_eq!(updates[2]["status"], "completed");
    assert_eq!(
        updates[2]["rawOutput"],
        json!([{"type": "text", "text": "# App"}])
    );
    assert_eq!(updates[4]["status"], "failed");
    assert_eq!(updates[5]["content"]["text"], "Hello!");
    assert_eq!(
        updates[6],
        json!({"sessionUpdate": "usage_update", "used": 1300, "size": 272000})
    );
    let turn = &sent(&user, "turn/start")[0]["params"];
    assert_eq!(
        turn,
        &json!({"threadId": session,
            "input": [{"type": "text", "text": "hello there", "text_elements": []}],
            "model": "gpt-fake", "effort": "medium", "environments": []})
    );
}

#[test]
fn a_cancel_interrupts_codex_and_the_turn_ends_cancelled() {
    let user = User::with_fake_codex();
    user.codex_script(
        json!({"turns": [{"when": "long", "do": [{"say": "Working on it"}, {"hang": true}]}]}),
    );
    let mut bridge = Bridge::start(&user);
    let (session, _) = bridge.open(&user, &[]);
    let id = bridge.prompt_later(&session, "a long task");
    while bridge.next()["params"]["update"]["content"]["text"] != "Working on it" {}

    bridge.send(
        json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}),
    );

    assert_eq!(bridge.answer_to(id)["result"]["stopReason"], "cancelled");
    let interrupts = sent(&user, "turn/interrupt");
    assert_eq!(interrupts.len(), 1);
    assert_eq!(interrupts[0]["params"]["threadId"], session.as_str());
    assert_eq!(interrupts[0]["params"]["turnId"], "turn-1");
}

#[test]
fn a_tool_of_codex_s_own_interrupts_the_turn_and_fails_the_prompt() {
    let user = User::with_fake_codex();
    user.codex_script(json!({"turns": [
        {"when": "shell", "do": [
            {"item": {"type": "commandExecution", "id": "c-1", "command": "ls"}},
            {"hang": true}]},
        {"when": "other", "do": [
            {"item": {"type": "mcpToolCall", "id": "m-1", "server": "elsewhere", "tool": "run",
                "status": "inProgress", "arguments": {}}},
            {"hang": true}]},
    ]}));
    let mut bridge = Bridge::start(&user);
    let (session, _) = bridge.open(&user, &[]);

    let (shell, _) = bridge.prompt(&session, "shell");
    let (other, _) = bridge.prompt(&session, "other");

    let shell = shell.unwrap_err()["message"].as_str().unwrap().to_owned();
    assert!(
        shell.contains("Codex started its own commandExecution"),
        "{shell}"
    );
    assert!(shell.contains("bare mode"), "{shell}");
    let other = other.unwrap_err()["message"].as_str().unwrap().to_owned();
    assert!(
        other.contains("a tool of the MCP server elsewhere"),
        "{other}"
    );
    assert_eq!(sent(&user, "turn/interrupt").len(), 2);
}

#[test]
fn codex_s_own_requests_are_refused() {
    let user = User::with_fake_codex();
    user.codex_script(json!({"turns": [{"when": "ask", "do": [
        {"request": "item/commandExecution/requestApproval"},
        {"request": "item/tool/requestUserInput"},
        {"say": "done"},
    ]}]}));
    let mut bridge = Bridge::start(&user);
    let (session, _) = bridge.open(&user, &[]);

    let (answer, _) = bridge.prompt(&session, "ask");

    assert_eq!(answer.unwrap()["stopReason"], "end_turn");
    let answered: Vec<Value> = user
        .codex_received()
        .into_iter()
        .filter_map(|e| e.get("answered").cloned())
        .collect();
    assert_eq!(answered.len(), 2);
    for answer in answered {
        assert!(answer["result"].is_null(), "{answer}");
        assert!(
            answer["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Flottille's tools only"),
            "{answer}"
        );
    }
}

fn offered(answer: &Value, id: &str) -> Option<Vec<String>> {
    let option = answer["configOptions"]
        .as_array()?
        .iter()
        .find(|o| o["id"] == id)?;
    Some(
        option["options"]
            .as_array()?
            .iter()
            .map(|v| v["value"].as_str().unwrap().to_owned())
            .collect(),
    )
}

fn current(answer: &Value, id: &str) -> Value {
    answer["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == id)
        .unwrap()["currentValue"]
        .clone()
}

#[test]
fn the_models_and_efforts_offered_are_codex_s_and_go_with_the_next_turn() {
    let user = User::with_fake_codex();
    let mut bridge = Bridge::start(&user);
    let (session, opened) = bridge.open(&user, &[]);
    assert_eq!(
        offered(&opened, "model").unwrap(),
        ["gpt-fake", "gpt-fake-mini"]
    );
    assert_eq!(current(&opened, "model"), "gpt-fake");
    assert_eq!(
        offered(&opened, "effort").unwrap(),
        ["low", "medium", "high"]
    );
    assert_eq!(current(&opened, "effort"), "medium");
    let category = opened["configOptions"][1]["category"].clone();
    assert_eq!(category, "thought_level");

    let (unknown, _) = bridge.request(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "model", "value": "gpt-fake-hidden"}),
    );
    let unknown = unknown.unwrap_err()["message"].as_str().unwrap().to_owned();
    assert!(unknown.contains("gpt-fake, gpt-fake-mini"), "{unknown}");
    let (mini, _) = bridge.request(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "model", "value": "gpt-fake-mini"}),
    );
    // The effort set goes back to the model's own when the new one does not offer it.
    let mini = mini.unwrap();
    assert_eq!(current(&mini, "effort"), "low");
    let (high, _) = bridge.request(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "effort", "value": "high"}),
    );
    assert!(high.is_err());
    let (minimal, _) = bridge.request(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "effort", "value": "minimal"}),
    );
    assert_eq!(current(&minimal.unwrap(), "effort"), "minimal");
    bridge.prompt(&session, "hello").0.unwrap();

    let turn = &sent(&user, "turn/start")[0]["params"];
    assert_eq!(turn["model"], "gpt-fake-mini");
    assert_eq!(turn["effort"], "minimal");
}

#[test]
fn a_workspace_plan_s_sign_in_or_none_is_refused_before_any_thread() {
    for (account, says) in [
        (
            json!({"type": "chatgpt", "email": "a@example.com", "planType": "enterprise"}),
            "ChatGPT enterprise plan",
        ),
        (Value::Null, "Codex is not signed in"),
    ] {
        let user = User::with_fake_codex();
        user.codex_script(json!({"account": account}));
        let mut bridge = Bridge::start(&user);

        let (refused, _) = bridge.request("session/new", new_session(&user, &[]));

        let refused = refused.unwrap_err()["message"].as_str().unwrap().to_owned();
        assert!(refused.contains(says), "{refused}");
        assert!(sent(&user, "thread/start").is_empty());
        // The bridge goes on: the next open starts another `codex`.
        user.codex_script(json!({}));
        bridge.open(&user, &[]);
        assert_eq!(starts(&user).len(), 2);
    }
}

#[test]
fn a_resumed_session_resumes_its_thread_with_a_fresh_server_and_an_unknown_one_fails() {
    let user = User::with_fake_codex();
    let mut first = Bridge::start(&user);
    let (session, _) = first.open(&user, &["mcp__flottille__report"]);
    assert_eq!(first.close(), Some(0));

    let mut second = Bridge::start(&user);
    let mut params = new_session(&user, &["mcp__flottille__report"]);
    params["sessionId"] = session.clone().into();
    params["mcpServers"][0]["headers"][0]["value"] = "Bearer invented-token-2".into();
    let (resumed, _) = second.request("session/resume", params.clone());

    let resumed = resumed.unwrap();
    assert!(resumed.get("sessionId").is_none());
    assert_eq!(current(&resumed, "model"), "gpt-fake");
    let resume = &sent(&user, "thread/resume")[0]["params"];
    assert_eq!(resume["threadId"], session.as_str());
    assert!(resume.get("environments").is_none());
    assert_eq!(
        resume["config"]["mcp_servers"]["flottille"]["http_headers"],
        json!({"Authorization": "Bearer invented-token-2"})
    );
    second.prompt(&session, "hello").0.unwrap();
    assert_eq!(
        sent(&user, "turn/start")[0]["params"]["environments"],
        json!([])
    );

    let mut third = Bridge::start(&user);
    params["sessionId"] = "thread-unknown".into();
    let (unknown, _) = third.request("session/resume", params);
    assert!(unknown.is_err());
}

#[test]
fn a_failed_turn_ends_with_a_failure_and_only_a_sign_in_error_fails_the_prompt() {
    let user = User::with_fake_codex();
    user.codex_script(json!({"turns": [
        {"when": "limit", "do": [], "error": {"message": "You've hit your usage limit.",
            "codexErrorInfo": "usageLimitExceeded"}},
        {"when": "broken", "do": [], "error": {"message": "stream disconnected",
            "codexErrorInfo": {"responseStreamDisconnected": {"httpStatusCode": 502}}}},
        {"when": "signed", "do": [], "error": {"message": "401 Unauthorized",
            "codexErrorInfo": "unauthorized"}},
        {"when": "busy", "do": [{"retry": "server overloaded"}, {"say": "ok"}]},
        {"when": "long", "do": [{"compaction": true}, {"say": "ok"}]},
    ]}));
    let mut bridge = Bridge::start(&user);
    let (session, _) = bridge.open(&user, &[]);

    let (limit, limit_seen) = bridge.prompt(&session, "limit");
    let (broken, broken_seen) = bridge.prompt(&session, "broken");
    let (busy, busy_seen) = bridge.prompt(&session, "busy");
    let (long, long_seen) = bridge.prompt(&session, "long");
    let (signed, _) = bridge.prompt(&session, "signed");

    assert_eq!(limit.unwrap()["stopReason"], "end_turn");
    assert_eq!(
        failures(&limit_seen),
        [(
            "error".into(),
            "limit".into(),
            "You've hit your usage limit.".into()
        )]
    );
    assert_eq!(broken.unwrap()["stopReason"], "end_turn");
    assert_eq!(failures(&broken_seen)[0].1, "service");
    assert_eq!(busy.unwrap()["stopReason"], "end_turn");
    assert_eq!(
        failures(&busy_seen),
        [(
            "warning".into(),
            String::new(),
            "the provider is retrying: server overloaded".into()
        )]
    );
    long.unwrap();
    let compaction = updates(&long_seen)
        .into_iter()
        .find(|u| u["_meta"]["contextCompaction"].is_object())
        .unwrap();
    assert_eq!(compaction["status"], "completed");
    assert_eq!(signed.unwrap_err()["message"], "401 Unauthorized");
}

#[test]
fn a_death_of_codex_ends_the_bridge_with_its_exit_code() {
    let user = User::with_fake_codex();
    user.codex_script(json!({"turns": [{"when": "die", "do": [{"say": "Bye"}, {"die": 3}]}]}));
    let mut bridge = Bridge::start(&user);
    let (session, _) = bridge.open(&user, &[]);
    bridge.prompt_later(&session, "die now");

    let status = bridge.child.wait().unwrap();

    assert_eq!(status.code(), Some(3));
}
