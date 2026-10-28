//! Flottille's Claude bridge (`flottille bridge claude`), driven over ACP as the daemon drives it,
//! in front of the fake `claude` (`crates/flottille-fake-claude`). No real `claude` ever runs here.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::User;
use serde_json::{Value, json};

const BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";

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
            .command(&["bridge", "claude"])
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
        assert_eq!(initialized.unwrap()["protocolVersion"], 1);
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
    /// before it. A request of the bridge's own is answered by `answer`.
    fn request_with(
        &mut self,
        method: &str,
        params: Value,
        mut answer: impl FnMut(&Value) -> Value,
    ) -> (Result<Value, Value>, Vec<Value>) {
        self.next += 1;
        let id = self.next;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut seen = Vec::new();
        loop {
            let message = self.next();
            if message.get("method").is_some() && message.get("id").is_some() {
                let result = answer(&message);
                self.send(json!({"jsonrpc": "2.0", "id": message["id"], "result": result}));
                seen.push(message);
            } else if message.get("method").is_none() && message["id"] == id {
                return match message.get("error") {
                    Some(error) => (Err(error.clone()), seen),
                    None => (Ok(message["result"].clone()), seen),
                };
            } else {
                seen.push(message);
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> (Result<Value, Value>, Vec<Value>) {
        self.request_with(method, params, |request| panic!("unexpected {request}"))
    }

    fn open(&mut self, user: &User, tools: &[&str]) -> String {
        let (opened, _) = self.request("session/new", new_session(user, tools));
        opened.unwrap()["sessionId"].as_str().unwrap().to_owned()
    }

    fn prompt(&mut self, session: &str, text: &str) -> (Result<Value, Value>, Vec<Value>) {
        self.request(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]}),
        )
    }

    /// Closes the bridge's input and waits for it to end: its exit code.
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

/// The options the daemon gives every Claude session (`claude::session_meta`).
fn meta(tools: &[&str]) -> Value {
    json!({"claudeCode": {"options": {
        "tools": [],
        "allowedTools": tools,
        "settingSources": [],
        "strictMcpConfig": true,
        "systemPrompt": {"type": "custom",
            "prompt": ["the shared part", BOUNDARY, "the session of zebra"], "snapshot": true},
        "env": {
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
            "ENABLE_CLAUDEAI_MCP_SERVERS": "false",
            "ENABLE_TOOL_SEARCH": "false",
            "MCP_TOOL_TIMEOUT": "600000",
            "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT": "600000"
        }
    }}})
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

/// What the fake `claude` read on its standard input, of one type.
fn written(user: &User, kind: &str) -> Vec<Value> {
    user.claude_received()
        .into_iter()
        .filter_map(|e| e.get("received").cloned())
        .filter(|m| m["type"] == kind)
        .collect()
}

#[test]
fn the_bridge_gives_claude_exactly_the_bare_options_and_environment() {
    let user = User::with_fake_claude();
    let mut bridge = Bridge::start(&user);

    let session = bridge.open(
        &user,
        &["mcp__flottille__fs_read", "mcp__flottille__report"],
    );

    let started = &user.claude_starts()[0];
    let argv: Vec<&str> = started["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let config = argv[argv.iter().position(|a| *a == "--mcp-config").unwrap() + 1];
    assert_eq!(
        argv,
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json",
            "--include-partial-messages",
            "--thinking-display=summarized",
            "--permission-prompt-tool=stdio",
            "--tools=",
            "--setting-sources=",
            "--strict-mcp-config",
            "--allowedTools=mcp__flottille__fs_read,mcp__flottille__report",
            "--mcp-config",
            config,
            &format!("--session-id={session}"),
        ]
    );
    // The token stays out of the command line, which other users' processes can read.
    assert!(!argv.iter().any(|a| a.contains("invented-token-1")));
    assert!(config.starts_with(user.path("runtime/flottille").to_str().unwrap()));
    assert_eq!(
        started["mcp_config"],
        json!({"mcpServers": {"flottille": {"type": "http", "url": "http://127.0.0.1:9/mcp",
            "headers": {"Authorization": "Bearer invented-token-1"}, "alwaysLoad": true}}})
    );
    assert_eq!(started["cwd"], user.path("work").to_str().unwrap());
    let env = &started["env"];
    for (name, value) in [
        ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1"),
        ("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),
        ("ENABLE_TOOL_SEARCH", "false"),
        ("MCP_TOOL_TIMEOUT", "600000"),
        ("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT", "600000"),
        ("CLAUDE_CODE_ENTRYPOINT", "sdk-rs"),
    ] {
        assert_eq!(env[name], value, "{name}");
    }
    // The system prompt, with its cache boundary, goes in Claude Code's own `initialize`.
    let initialize = &written(&user, "control_request")[0]["request"];
    assert_eq!(
        initialize,
        &json!({"subtype": "initialize",
            "systemPrompt": ["the shared part", BOUNDARY, "the session of zebra"],
            "systemPromptSnapshot": true})
    );
    assert_eq!(bridge.close(), Some(0));
    assert!(
        !std::path::Path::new(config).exists(),
        "the file with the token is removed"
    );
}

#[test]
fn options_that_are_not_bare_are_refused() {
    let user = User::with_fake_claude();
    let mut bridge = Bridge::start(&user);
    for (key, value) in [
        ("tools", json!(["Bash"])),
        ("settingSources", json!(["user"])),
        ("strictMcpConfig", json!(false)),
        ("permissionMode", json!("bypassPermissions")),
    ] {
        let mut params = new_session(&user, &["mcp__flottille__report"]);
        params["_meta"]["claudeCode"]["options"][key] = value;

        let (refused, _) = bridge.request("session/new", params);

        assert!(refused.is_err(), "{key}");
    }
    assert!(user.claude_starts().is_empty());
}

#[test]
fn a_prompt_streams_text_thoughts_and_tool_calls_then_the_usage_and_its_stop_reason() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "hello", "do": [
        {"think": "a greeting"},
        {"say": "Hello"},
        {"say": " there!"},
        {"tool": {"id": "toolu_01", "name": "mcp__flottille__fs_list",
            "input": {"path": "."}, "result": "README.md"}}
    ]}]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &["mcp__flottille__fs_list"]);

    let (answer, seen) = bridge.prompt(&session, "hello");

    let answer = answer.unwrap();
    assert_eq!(answer["stopReason"], "end_turn");
    assert_eq!(
        answer["usage"],
        json!({"inputTokens": 100, "outputTokens": 20, "cachedReadTokens": 1000,
            "cachedWriteTokens": 50, "totalTokens": 1170})
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
            "agent_message_chunk",
            "agent_message_chunk",
            "tool_call",
            "tool_call_update",
            "usage_update"
        ],
        "the whole messages repeat nothing the stream gave: {updates:?}"
    );
    assert_eq!(
        updates[0]["content"],
        json!({"type": "text", "text": "a greeting"})
    );
    assert_eq!(updates[1]["content"]["text"], "Hello");
    assert_eq!(updates[2]["content"]["text"], " there!");
    // The call's id is the tool use's, which Claude Code gives Flottille's MCP server as
    // `claudecode/toolUseId`.
    assert_eq!(updates[3]["toolCallId"], "toolu_01");
    assert_eq!(updates[3]["title"], "mcp__flottille__fs_list");
    assert_eq!(updates[3]["status"], "in_progress");
    assert_eq!(updates[3]["rawInput"], json!({"path": "."}));
    assert_eq!(
        updates[3]["_meta"]["claudeCode"]["toolName"],
        "mcp__flottille__fs_list"
    );
    assert_eq!(updates[4]["toolCallId"], "toolu_01");
    assert_eq!(updates[4]["status"], "completed");
    assert_eq!(updates[4]["rawOutput"], "README.md");
    assert_eq!(
        updates[5],
        json!({"sessionUpdate": "usage_update", "used": 1170, "size": 200000,
            "cost": {"amount": 0.0125, "currency": "USD"}})
    );
    assert_eq!(
        written(&user, "user"),
        [
            json!({"type": "user", "session_id": session, "parent_tool_use_id": null,
            "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}})
        ]
    );
}

/// The `messageId` of each update that carries a reply or a thought, in order.
fn message_ids(updates: &[Value]) -> Vec<(String, Option<String>)> {
    updates
        .iter()
        .filter(|u| {
            ["agent_message_chunk", "agent_thought_chunk"]
                .contains(&u["sessionUpdate"].as_str().unwrap())
        })
        .map(|u| {
            (
                u["content"]["text"].as_str().unwrap().to_owned(),
                u["messageId"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

#[test]
fn a_reply_and_a_thought_carry_the_id_of_the_api_message_they_come_from() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "hello", "do": [
        {"think": "a greeting", "message": "msg_a"},
        {"say": "Hello", "message": "msg_a"},
        {"say": " there!", "message": "msg_a"},
        {"stream": ["Bye", " now"], "message": "msg_b"}
    ]}]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    let (_, seen) = bridge.prompt(&session, "hello");

    let ids = message_ids(&updates(&seen));
    let id = |text: &str, id: &str| (text.to_owned(), Some(id.to_owned()));
    assert_eq!(
        ids,
        [
            id("a greeting", "msg_a"),
            id("Hello", "msg_a"),
            id(" there!", "msg_a"),
            id("Bye", "msg_b"),
            id(" now", "msg_b"),
        ]
    );
}

#[test]
fn a_reply_the_api_message_of_which_is_unknown_has_no_message_id() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "work", "do": [{"limit": "five_hour"}]}]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    let (_, seen) = bridge.prompt(&session, "work");

    // Claude Code's own message about the limit has no API message behind it: nothing is invented.
    let ids = message_ids(&updates(&seen));
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert_eq!(ids[0].1, None);
}

#[test]
fn a_loaded_session_replays_its_history_with_the_message_ids() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "hello", "do": [
        {"think": "a greeting", "message": "msg_a"},
        {"say": "Hello!", "message": "msg_a"}
    ]}]}));
    let mut first = Bridge::start(&user);
    let session = first.open(&user, &[]);
    first.prompt(&session, "hello").0.unwrap();
    assert_eq!(first.close(), Some(0));
    // A line written before the ids were kept has none, and replays with none.
    let history = user.path(&format!("state/flottille/bridge/claude/{session}.jsonl"));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(history)
        .unwrap();
    writeln!(file, "{}", json!({"agent": "an old reply"})).unwrap();

    let mut second = Bridge::start(&user);
    let mut params = new_session(&user, &[]);
    params["sessionId"] = session.into();
    let (loaded, seen) = second.request("session/load", params);
    loaded.unwrap();

    let ids = message_ids(&updates(&seen));
    let id = |text: &str, id: Option<&str>| (text.to_owned(), id.map(str::to_owned));
    assert_eq!(
        ids,
        [
            id("a greeting", Some("msg_a")),
            id("Hello!", Some("msg_a")),
            id("an old reply", None),
        ]
    );
}

#[test]
fn a_thought_claude_sends_without_its_text_gives_no_empty_thought() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "hello", "do": [
        {"think_hidden": true},
        {"think": "a plan"},
        {"say": "Done"}
    ]}]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    let (answer, seen) = bridge.prompt(&session, "hello");

    answer.unwrap();
    let thoughts: Vec<Value> = updates(&seen)
        .into_iter()
        .filter(|u| u["sessionUpdate"] == "agent_thought_chunk")
        .map(|u| u["content"]["text"].clone())
        .collect();
    assert_eq!(thoughts, [json!("a plan")]);
    let history = std::fs::read_to_string(
        user.path(&format!("state/flottille/bridge/claude/{session}.jsonl")),
    )
    .unwrap();
    assert!(!history.contains(r#"{"thought":""}"#), "{history}");
}

#[test]
fn a_compaction_is_reported_and_the_context_used_drops() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "long", "do": [{"compaction": true}]}]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    let (answer, seen) = bridge.prompt(&session, "a long one");

    assert_eq!(answer.unwrap()["stopReason"], "end_turn");
    let updates = updates(&seen);
    let compacted = updates
        .iter()
        .find(|u| u["sessionUpdate"] == "tool_call")
        .unwrap();
    assert_eq!(compacted["status"], "completed");
    assert!(
        !compacted["_meta"]["contextCompaction"].is_null(),
        "{compacted}"
    );
    assert!(
        updates
            .iter()
            .any(|u| u["sessionUpdate"] == "usage_update" && u["used"] == 12000)
    );
}

#[test]
fn a_provider_retry_is_a_provider_wait() {
    let user = User::with_fake_claude();
    user.claude_script(
        json!({"turns": [{"when": "busy", "do": [{"retry": true}, {"say": "Done"}]}]}),
    );
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    let (_, seen) = bridge.prompt(&session, "busy day");

    let wait = updates(&seen)
        .into_iter()
        .find(|u| u["sessionUpdate"] == "session_info_update")
        .unwrap();
    assert_eq!(
        wait["_meta"]["jetbrains"]["air"]["sessionFailure"]["severity"],
        "warning"
    );
}

#[test]
fn a_cancel_interrupts_claude_and_the_turn_ends_cancelled() {
    let user = User::with_fake_claude();
    user.claude_script(
        json!({"turns": [{"when": "long", "do": [{"say": "Working on it"}, {"hang": true}]}]}),
    );
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);
    bridge.next += 1;
    let id = bridge.next;
    bridge.send(
        json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "a long task"}]}}),
    );
    while bridge.next()["params"]["update"]["content"]["text"] != "Working on it" {}

    bridge.send(
        json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}),
    );

    let answer = loop {
        let message = bridge.next();
        if message["id"] == id {
            break message;
        }
    };
    assert_eq!(answer["result"]["stopReason"], "cancelled");
    let interrupts: Vec<Value> = written(&user, "control_request")
        .into_iter()
        .filter(|m| m["request"]["subtype"] == "interrupt")
        .collect();
    assert_eq!(interrupts.len(), 1);
}

#[test]
fn claude_s_permission_requests_are_asked_of_the_client_and_its_answer_given_to_claude() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "ask", "do": [
        {"permission": {"tool": "Bash", "input": {"command": "ls"}}},
        {"permission": {"tool": "mcp__flottille__fs_list", "input": {"path": "."}}}
    ]}]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &["mcp__flottille__fs_list"]);

    let (answer, seen) = bridge.request_with(
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": "ask"}]}),
        |request| {
            assert_eq!(request["method"], "session/request_permission");
            let call = &request["params"]["toolCall"];
            let tool = call["_meta"]["claudeCode"]["toolName"].as_str().unwrap();
            let kind = if tool == "Bash" {
                "reject_once"
            } else {
                "allow_once"
            };
            let option = request["params"]["options"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["kind"] == kind)
                .unwrap();
            json!({"outcome": {"outcome": "selected", "optionId": option["optionId"]}})
        },
    );

    assert_eq!(answer.unwrap()["stopReason"], "end_turn");
    let asked: Vec<&Value> = seen
        .iter()
        .filter(|m| m["method"] == "session/request_permission")
        .collect();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0]["params"]["sessionId"], session.as_str());
    assert_eq!(
        asked[0]["params"]["toolCall"]["toolCallId"],
        "toolu_permission-0"
    );
    assert_eq!(
        asked[0]["params"]["toolCall"]["rawInput"],
        json!({"command": "ls"})
    );
    let answers: Vec<Value> = user
        .claude_received()
        .into_iter()
        .filter_map(|e| e.get("permission").cloned())
        .collect();
    assert_eq!(answers[0]["response"]["behavior"], "deny");
    assert!(answers[0]["response"]["message"].is_string());
    assert_eq!(
        answers[1]["response"],
        json!({"behavior": "allow", "updatedInput": {"path": "."}})
    );
}

#[test]
fn a_resumed_session_runs_claude_with_resume_and_a_loaded_one_replays_its_history() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "hello", "do": [{"say": "Hello!"}]}]}));
    let mut first = Bridge::start(&user);
    let session = first.open(&user, &[]);
    first.prompt(&session, "hello").0.unwrap();
    assert_eq!(first.close(), Some(0));

    let mut second = Bridge::start(&user);
    let mut params = new_session(&user, &[]);
    params["sessionId"] = session.clone().into();
    let (resumed, seen) = second.request("session/resume", params.clone());
    resumed.unwrap();
    assert!(updates(&seen).is_empty(), "a resume replays nothing");
    assert_eq!(second.close(), Some(0));

    let mut third = Bridge::start(&user);
    let (loaded, seen) = third.request("session/load", params);
    loaded.unwrap();
    let replayed: Vec<(String, String)> = updates(&seen)
        .iter()
        .map(|u| {
            (
                u["sessionUpdate"].as_str().unwrap().to_owned(),
                u["content"]["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        replayed,
        [
            ("user_message_chunk".to_owned(), "hello".to_owned()),
            ("agent_message_chunk".to_owned(), "Hello!".to_owned())
        ]
    );
    let starts = user.claude_starts();
    assert_eq!(starts.len(), 3);
    for start in &starts[1..] {
        assert!(
            start["argv"]
                .as_array()
                .unwrap()
                .contains(&json!(format!("--resume={session}"))),
            "{start}"
        );
    }
}

#[test]
fn the_history_is_masked_and_kept_one_line_per_message() {
    let user = User::with_fake_claude().with_env("ANTHROPIC_API_KEY", "invented-key-123");
    user.claude_script(json!({"turns": [{"when": "hello", "do": [
        {"stream": ["Hel", "lo, ", "invented-key-123"]},
        {"think": "a greeting"}
    ]}]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    bridge
        .prompt(&session, "hello, my key is invented-key-123")
        .0
        .unwrap();

    let history = std::fs::read_to_string(
        user.path(&format!("state/flottille/bridge/claude/{session}.jsonl")),
    )
    .unwrap();
    let lines: Vec<Value> = history
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            json!({"user": "hello, my key is [masked]"}),
            json!({"agent": "Hello, [masked]", "id": "msg_fake_1"}),
            json!({"thought": "a greeting", "id": "msg_fake_2"})
        ]
    );
}

#[test]
fn a_session_claude_does_not_know_cannot_be_resumed() {
    let user = User::with_fake_claude();
    let mut bridge = Bridge::start(&user);
    let mut params = new_session(&user, &[]);
    params["sessionId"] = "00000000-0000-4000-8000-000000000000".into();

    let (refused, _) = bridge.request("session/resume", params);

    assert!(refused.is_err());
}

#[test]
fn the_bridge_offers_load_and_resume_and_resume_can_be_withheld() {
    let user = User::with_fake_claude();
    let (answer, _) = Bridge::start(&user).request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    );
    let capabilities = &answer.unwrap()["agentCapabilities"];
    assert_eq!(capabilities["loadSession"], true);
    assert!(capabilities["sessionCapabilities"]["resume"].is_object());

    let user = user.with_env("FLOTTILLE_BRIDGE_NO_RESUME", "1");
    let (answer, _) = Bridge::start(&user).request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    );
    let capabilities = &answer.unwrap()["agentCapabilities"];
    assert_eq!(capabilities["loadSession"], true);
    assert!(capabilities["sessionCapabilities"]["resume"].is_null());
}

/// The values an option of the answer offers, by its id.
fn offered(answer: &Value, id: &str) -> Option<Vec<String>> {
    let option = answer["configOptions"]
        .as_array()?
        .iter()
        .find(|o| o["id"] == id)?;
    Some(
        option["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["value"].as_str().unwrap().to_owned())
            .collect(),
    )
}

fn current(answer: &Value, id: &str) -> Value {
    answer["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == id)
        .map(|o| o["currentValue"].clone())
        .unwrap_or_default()
}

#[test]
fn the_models_offered_are_claude_s_own_and_an_unknown_one_is_refused_with_them() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"models": [
        {"value": "default", "displayName": "Default (recommended)", "description": "d"},
        {"value": "fable", "displayName": "Fable", "description": "f"},
        {"value": "haiku", "displayName": "Haiku", "description": "h"}
    ]}));
    let mut bridge = Bridge::start(&user);
    let (opened, _) = bridge.request("session/new", new_session(&user, &[]));
    let opened = opened.unwrap();
    let session = opened["sessionId"].as_str().unwrap().to_owned();
    let model = &opened["configOptions"][0];
    assert_eq!(model["category"], "model");
    assert_eq!(model["currentValue"], "default");
    assert_eq!(
        offered(&opened, "model").unwrap(),
        ["default", "fable", "haiku"]
    );
    assert_eq!(model["options"][1]["name"], "Fable");

    let (set, _) = bridge.request(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "model", "value": "fable"}),
    );

    assert_eq!(current(&set.unwrap(), "model"), "fable");
    let asked: Vec<Value> = written(&user, "control_request")
        .into_iter()
        .filter(|m| m["request"]["subtype"] == "set_model")
        .collect();
    assert_eq!(asked[0]["request"]["model"], "fable");
    let (refused, _) = bridge.request(
        "session/set_config_option",
        json!({"sessionId": session, "configId": "model", "value": "opus"}),
    );
    let refused = refused.unwrap_err();
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("default, fable, haiku"),
        "{refused}"
    );
}

#[test]
fn the_effort_is_an_option_of_the_model_set_on_claude() {
    let user = User::with_fake_claude();
    let mut bridge = Bridge::start(&user);
    let (opened, _) = bridge.request("session/new", new_session(&user, &[]));
    let opened = opened.unwrap();
    let session = opened["sessionId"].as_str().unwrap().to_owned();
    assert_eq!(
        offered(&opened, "effort").unwrap(),
        ["default", "low", "medium", "high", "xhigh"]
    );
    let effort = opened["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "effort")
        .unwrap()
        .clone();
    assert_eq!(effort["category"], "thought_level");
    assert_eq!(effort["currentValue"], "default");
    let set = |bridge: &mut Bridge, id: &str, value: &str| {
        bridge
            .request(
                "session/set_config_option",
                json!({"sessionId": session, "configId": id, "value": value}),
            )
            .0
    };
    let flags = || -> Vec<Value> {
        written(&user, "control_request")
            .into_iter()
            .filter(|m| m["request"]["subtype"] == "apply_flag_settings")
            .map(|m| m["request"]["settings"].clone())
            .collect()
    };

    let high = set(&mut bridge, "effort", "high").unwrap();

    assert_eq!(current(&high, "effort"), "high");
    assert_eq!(flags(), [json!({"effortLevel": "high"})]);
    assert!(
        set(&mut bridge, "effort", "max").is_err(),
        "not the default model's"
    );
    let fable = set(&mut bridge, "model", "fable").unwrap();
    assert_eq!(current(&fable, "effort"), "high", "fable takes it too");
    assert!(
        offered(&fable, "effort")
            .unwrap()
            .contains(&"max".to_owned())
    );
    // Haiku has no effort: the option goes, and the level set goes back to Claude's own.
    let haiku = set(&mut bridge, "model", "haiku").unwrap();
    assert!(offered(&haiku, "effort").is_none());
    assert_eq!(
        flags(),
        [json!({"effortLevel": "high"}), json!({"effortLevel": null})]
    );
    assert!(set(&mut bridge, "effort", "low").is_err());
}

#[test]
fn a_model_change_is_answered_once_claude_took_the_effort_reset() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"slow_controls": 300}));
    let mut bridge = Bridge::start(&user);
    let (opened, _) = bridge.request("session/new", new_session(&user, &[]));
    let session = opened.unwrap()["sessionId"].as_str().unwrap().to_owned();
    let mut set = |id: &str, value: &str| {
        bridge
            .request(
                "session/set_config_option",
                json!({"sessionId": session, "configId": id, "value": value}),
            )
            .0
            .unwrap()
    };
    set("effort", "high");

    set("model", "haiku");

    let resets: Vec<Value> = written(&user, "control_request")
        .into_iter()
        .filter(|m| m["request"]["subtype"] == "apply_flag_settings")
        .map(|m| m["request"]["settings"].clone())
        .collect();
    assert_eq!(
        resets,
        [json!({"effortLevel": "high"}), json!({"effortLevel": null})]
    );
}

#[test]
fn the_stop_reason_follows_claude_s_result() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [
        {"when": "refuse", "do": [], "stop_reason": "refusal"},
        {"when": "long", "do": [{"say": "and"}], "stop_reason": "max_tokens"},
        {"when": "loop", "do": [], "subtype": "error_max_turns"}
    ]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    for (text, stop) in [
        ("refuse", "refusal"),
        ("long", "max_tokens"),
        ("loop", "max_turn_requests"),
    ] {
        let (answer, _) = bridge.prompt(&session, text);
        assert_eq!(answer.unwrap()["stopReason"], stop, "{text}");
    }
}

/// The session failures of a turn's updates: severity, category and message.
fn failures(seen: &[Value]) -> Vec<(String, String, String)> {
    updates(seen)
        .iter()
        .filter(|u| u["sessionUpdate"] == "session_info_update")
        .map(|u| {
            let failure = &u["_meta"]["jetbrains"]["air"]["sessionFailure"];
            let text = |key: &str| failure[key].as_str().unwrap_or_default().to_owned();
            (text("severity"), text("category"), text("message"))
        })
        .collect()
}

#[test]
fn a_rejected_rate_limit_ends_the_turn_with_a_limit_failure() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "work", "do": [{"limit": "five_hour"}]}]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    let (answer, seen) = bridge.prompt(&session, "work");

    assert_eq!(answer.unwrap()["stopReason"], "end_turn");
    let failures = failures(&seen);
    assert_eq!(failures.len(), 1, "{failures:?}");
    let (severity, category, message) = &failures[0];
    assert_eq!((severity.as_str(), category.as_str()), ("error", "limit"));
    assert!(message.contains("hit your limit"), "{message}");
    // The session goes on: the next prompt is played.
    assert_eq!(
        bridge.prompt(&session, "again").0.unwrap()["stopReason"],
        "end_turn"
    );
}

#[test]
fn a_provider_error_ends_the_turn_with_a_failure_and_only_a_sign_in_error_fails_the_prompt() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [
        {"when": "broken", "do": [], "error": "API Error: 500 the provider failed"},
        {"when": "signed", "do": [], "error": "Invalid API key · Please run /login"}
    ]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    let (answer, seen) = bridge.prompt(&session, "broken");

    assert_eq!(answer.unwrap()["stopReason"], "end_turn");
    let failures = failures(&seen);
    assert_eq!(failures.len(), 1, "{failures:?}");
    let (severity, category, message) = &failures[0];
    assert_eq!((severity.as_str(), category.as_str()), ("error", "service"));
    assert!(message.contains("the provider failed"), "{message}");
    let (failed, _) = bridge.prompt(&session, "signed out");
    let failed = failed.unwrap_err();
    assert!(
        failed["message"].as_str().unwrap().contains("/login"),
        "{failed}"
    );
}

#[test]
fn a_line_of_claude_s_that_is_not_text_is_skipped() {
    let user = User::with_fake_claude();
    user.claude_script(
        json!({"turns": [{"when": "noise", "do": [{"garbage": true}, {"say": "after"}]}]}),
    );
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    let (answer, seen) = bridge.prompt(&session, "noise");

    assert_eq!(answer.unwrap()["stopReason"], "end_turn");
    assert!(
        updates(&seen)
            .iter()
            .any(|u| u["content"]["text"] == "after")
    );
}

#[test]
fn a_prompt_s_text_blocks_are_parted_by_a_blank_line_and_a_resource_is_refused() {
    let user = User::with_fake_claude();
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);

    let (answer, _) = bridge.request(
        "session/prompt",
        json!({"sessionId": session, "prompt": [
            {"type": "text", "text": "first"}, {"type": "text", "text": "second"}]}),
    );
    answer.unwrap();
    let (refused, _) = bridge.request(
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": "see"},
            {"type": "resource", "resource": {"uri": "file:///notes.md", "text": "notes"}}]}),
    );

    assert!(refused.is_err());
    let prompts: Vec<Value> = written(&user, "user")
        .into_iter()
        .map(|m| m["message"]["content"][0]["text"].clone())
        .collect();
    assert_eq!(prompts, [json!("first\n\nsecond")]);
}

#[test]
fn a_death_of_claude_ends_the_bridge_with_its_exit_code() {
    let user = User::with_fake_claude();
    user.claude_script(json!({"turns": [{"when": "die", "do": [{"say": "Bye"}, {"die": 3}]}]}));
    let mut bridge = Bridge::start(&user);
    let session = bridge.open(&user, &[]);
    bridge.send(
        json!({"jsonrpc": "2.0", "id": 99, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "die now"}]}}),
    );

    let status = bridge.child.wait().unwrap();

    assert_eq!(status.code(), Some(3));
}
