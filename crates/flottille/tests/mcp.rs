//! The scenarios of the agents' MCP server and its tools, called over HTTP with the token the
//! daemon gave a fake agent's session (`crates/flottille-fake-agent`), with throwaway
//! repositories and temporary XDG folders. No real agent ever runs here.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, post, server_of};
use flottille_core::store::Store;
use rmcp::model::{CallToolRequestParams, ClientConfig, ProtocolVersion, SubscriptionFilter};
use rmcp::service::{NotificationContext, RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt};
use serde_json::{Value, json};

/// The limit every normal result stays under (`docs/reference/mcp.md` §4 rule 5).
const RESULT_LIMIT: usize = 40_000;

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

/// An agent of `app` started with `params`: its id, and the MCP server its session got.
fn agent(user: &User, params: Value) -> (String, String, String) {
    let mut params = params;
    params["project"] = "app".into();
    if params.get("message").is_none() {
        params["message"] = "hello".into();
    }
    let opened = user.received_requests("session/new").len();
    let answer = user.call("agent.run", params).unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, url, authorization)
}

/// A workspace `w` of `app`: its folder. The project folder is read-only for agents.
fn workspace(user: &User) -> PathBuf {
    let workspace = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    PathBuf::from(workspace["folder"].as_str().unwrap())
}

/// The session of an agent working in the workspace `w`, through rmcp's client, and its folder.
fn session(user: &User) -> (Mcp, PathBuf) {
    project(user);
    let folder = workspace(user);
    let (_, url, authorization) = agent(user, json!({"workspace": "w"}));
    (Mcp::initialize(&url, &authorization), folder)
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

/// A call the gate held: answered at once, not as an error, with the waiting text.
fn held(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        answer
            .text
            .starts_with("Waiting for the user's approval, request #"),
        "{}",
        answer.text
    );
    answer.text
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not an error: {}", answer.text);
    assert!(answer.text.chars().count() < 2_000, "{}", answer.text);
    answer.text
}

/// The id of the run an answer of `run` names.
fn run_id(text: &str) -> String {
    text.strip_prefix("run ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("no run in {text}"))
        .to_owned()
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

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

// Tokens and lists.

#[test]
fn a_wrong_token_gets_401() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, url, authorization) = agent(&user, json!({}));
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}}});

    for headers in [
        vec![("Authorization", "Bearer not-the-token")],
        vec![],
        vec![("Authorization", "Basic dXNlcjpwYXNz")],
    ] {
        let (status, body, _) = post(&url, &headers, &initialize);
        assert_eq!(status, 401, "{headers:?}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({"error": "unauthorized"})
        );
    }
    let (status, _, _) = post(&url, &[("Authorization", &authorization)], &initialize);
    assert_eq!(status, 200);
}

#[test]
fn a_request_for_another_host_or_from_another_origin_is_refused() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, url, authorization) = agent(&user, json!({}));
    let port = url.split(':').nth(2).unwrap().split('/').next().unwrap();
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}}});
    let auth = ("Authorization", authorization.as_str());

    // A page of another site that resolves its name to this machine is not this server's client.
    let elsewhere = format!("rebound.example:{port}");
    assert_eq!(
        post(&url, &[auth, ("Host", &elsewhere)], &initialize).0,
        403
    );
    assert_eq!(
        post(
            &url,
            &[auth, ("Origin", "http://rebound.example")],
            &initialize
        )
        .0,
        403
    );
    let local = format!("localhost:{port}");
    let origin = format!("http://localhost:{port}");
    assert_eq!(
        post(
            &url,
            &[auth, ("Host", &local), ("Origin", &origin)],
            &initialize
        )
        .0,
        200
    );
}

#[test]
fn a_token_minted_again_revokes_the_previous_one() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_IDLE_RELEASE_MS", "300");
    project(&user);
    let (id, url, first) = agent(&user, json!({}));
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}}});
    // Released, then started again by a new message: a new process, a new token.
    eventually("the release", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "agent.released")
    });
    user.call("agent.send", json!({"agent": id, "message": "again"}))
        .unwrap();
    eventually("a second session", || {
        user.received_requests("session/resume").len() == 1
    });
    let second = user.received_requests("session/resume")[0]["params"]["mcpServers"][0]["headers"]
        [0]["value"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_ne!(first, second);
    assert_eq!(post(&url, &[("Authorization", &first)], &initialize).0, 401);
    assert_eq!(
        post(&url, &[("Authorization", &second)], &initialize).0,
        200
    );
}

#[test]
fn a_session_sees_only_its_tools() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);

    // The tools that exist, in the order frozen in the agent; the agent's tools that come later
    // are not listed until they do.
    assert_eq!(
        mcp.names(),
        [
            "fs_read",
            "fs_list",
            "search",
            "fs_write",
            "fs_edit",
            "tasks",
            "run",
            "run_status",
            "skill_read",
            "workspace",
            "web",
            "services",
            "agent_launch",
            "agent_send",
            "agent_status",
            "agent_stop",
            "skills",
            "commands",
            "setup",
            "project",
            "wake",
            "report",
            "ask",
            "session_send",
            "session_read",
            "session_title",
            "approval_answer",
        ]
    );
    let text = refused(mcp.call("shell", json!({})));
    assert!(text.contains("has no tool shell"), "{text}");

    // Another agent's session is its own: what the first read and ran is not the second's.
    ok(mcp.call("fs_read", json!({"path": "README.md"})));
    let run = run_id(&ok(mcp.call("run", json!({"line": "true"}))));
    let (_, url, authorization) = agent(&user, json!({"workspace": "w"}));
    let other = Mcp::initialize(&url, &authorization);
    let text = refused(other.call("fs_write", json!({"path": "README.md", "content": "x"})));
    assert!(text.contains("has not read it"), "{text}");
    let text = refused(other.call("run_status", json!({"run": run})));
    assert!(text.contains("is not a run this agent started"), "{text}");
}

#[test]
fn tools_list_is_in_the_same_order_every_time_and_every_entry_carries_title_annotations_and_always_load()
 {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);

    let first = mcp.tools();
    for _ in 0..3 {
        assert_eq!(mcp.tools(), first);
    }
    for tool in &first {
        let name = tool["name"].as_str().unwrap();
        assert!(
            tool["title"].as_str().is_some_and(|t| !t.is_empty()),
            "{name}"
        );
        let description = tool["description"].as_str().unwrap();
        // Claude Code cuts an upfront tool's description at 4,096 characters, 2,048 before
        // v2.1.296 (https://code.claude.com/docs/en/mcp, "Scale with MCP tool search"): the
        // older figure, so no version cuts a text.
        assert!(
            (40..2_048).contains(&description.chars().count()),
            "{name}: {description}"
        );
        let annotations = &tool["annotations"];
        for hint in ["readOnlyHint", "idempotentHint", "destructiveHint"] {
            assert!(annotations[hint].is_boolean(), "{name}: {hint}");
        }
        assert_eq!(tool["_meta"]["anthropic/alwaysLoad"], true, "{name}");
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
    }
    let hints = |name: &str| {
        let tool = first.iter().find(|t| t["name"] == name).unwrap();
        (
            tool["annotations"]["readOnlyHint"] == true,
            tool["annotations"]["destructiveHint"] == true,
        )
    };
    assert_eq!(hints("fs_read"), (true, false));
    assert_eq!(hints("search"), (true, false));
    assert_eq!(hints("fs_write"), (false, true));
    assert_eq!(hints("run_status"), (false, true));
}

// Protocol revisions.

#[test]
fn a_2025_11_25_client_with_initialize_lists_and_calls_the_tools() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, url, authorization) = agent(&user, json!({}));

    let mcp = Mcp::initialize(&url, &authorization);
    assert_eq!(mcp.names()[0], "fs_read");
    assert!(ok(mcp.call("fs_list", json!({"path": "."}))).contains("README.md"));

    // No protocol session: the token is the identity.
    let (status, body, headers) = post(
        &url,
        &[("Authorization", &authorization)],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}}}),
    );
    assert_eq!(status, 200, "{body}");
    assert!(!headers.contains_key("mcp-session-id"), "{headers:?}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(body["result"]["capabilities"]["tools"]["listChanged"], true);
    let (status, body, _) = post(
        &url,
        &[
            ("Authorization", &authorization),
            ("MCP-Protocol-Version", "2025-11-25"),
        ],
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "fs_read", "arguments": {"path": "README.md"}}}),
    );
    assert_eq!(status, 200, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert!(
        body["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("app"),
        "{body}"
    );
}

#[test]
fn a_2026_07_28_client_with_server_discover_lists_and_calls_the_tools() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, url, authorization) = agent(&user, json!({}));

    let mcp = Mcp::discover(&url, &authorization);
    assert_eq!(mcp.names().len(), 27);
    assert!(ok(mcp.call("fs_list", json!({"path": "."}))).contains("README.md"));

    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let (status, body, headers) = post(
        &url,
        &[
            ("Authorization", &authorization),
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "server/discover"),
        ],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover",
            "params": {"_meta": meta}}),
    );
    assert_eq!(status, 200, "{body}");
    assert!(!headers.contains_key("mcp-session-id"));
    let body: Value = serde_json::from_str(&body).unwrap();
    let versions = body["result"]["supportedVersions"].as_array().unwrap();
    for version in ["2025-11-25", "2026-07-28"] {
        assert!(versions.contains(&json!(version)), "{body}");
    }
    let (status, body, _) = post(
        &url,
        &[
            ("Authorization", &authorization),
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "tools/list"),
        ],
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": meta}}),
    );
    assert_eq!(status, 200, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["result"]["tools"][0]["name"], "fs_read", "{body}");
}

// Files.

#[test]
fn writing_a_file_never_read_is_refused() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);

    let text = refused(mcp.call(
        "fs_write",
        json!({"path": "README.md", "content": "overwritten\n"}),
    ));
    assert_eq!(
        text,
        "refused: README.md exists and this session has not read it: read it with fs_read first"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("README.md")).unwrap(),
        "app\n"
    );

    // A new file needs no read; once read, a file is written.
    ok(mcp.call(
        "fs_write",
        json!({"path": "src/new.txt", "content": "new\n"}),
    ));
    assert_eq!(
        std::fs::read_to_string(folder.join("src/new.txt")).unwrap(),
        "new\n"
    );
    ok(mcp.call("fs_read", json!({"path": "README.md"})));
    ok(mcp.call(
        "fs_write",
        json!({"path": "README.md", "content": "written\n"}),
    ));
    assert_eq!(
        std::fs::read_to_string(folder.join("README.md")).unwrap(),
        "written\n"
    );
    // What the session wrote is what it knows: it writes again without reading.
    ok(mcp.call(
        "fs_write",
        json!({"path": "README.md", "content": "again\n"}),
    ));
}

#[test]
fn a_file_changed_since_read_is_refused_with_a_diff() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::write(folder.join("notes.txt"), "one\ntwo\nthree\n").unwrap();
    ok(mcp.call("fs_read", json!({"path": "notes.txt"})));
    std::fs::write(folder.join("notes.txt"), "one\nTWO\nthree\n").unwrap();

    let text = refused(mcp.call(
        "fs_edit",
        json!({"path": "notes.txt", "edits": [{"old": "one", "new": "1"}]}),
    ));
    assert!(
        text.starts_with(
            "refused: notes.txt changed since this session read it; read it again before \
             writing. What changed:\n"
        ),
        "{text}"
    );
    assert!(text.contains("-two"), "{text}");
    assert!(text.contains("+TWO"), "{text}");
    assert!(!text.contains("three"), "only what changed: {text}");
    let text = refused(mcp.call("fs_write", json!({"path": "notes.txt", "content": "x"})));
    assert!(
        text.contains("changed since this session read it"),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("notes.txt")).unwrap(),
        "one\nTWO\nthree\n"
    );

    std::fs::remove_file(folder.join("notes.txt")).unwrap();
    let text = refused(mcp.call("fs_write", json!({"path": "notes.txt", "content": "x"})));
    assert_eq!(
        text,
        "refused: notes.txt was removed since this session read it; nothing was written"
    );
}

#[test]
fn an_edit_whose_anchor_appears_twice_changes_nothing() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::write(folder.join("code.txt"), "first\naaa\n").unwrap();
    ok(mcp.call("fs_read", json!({"path": "code.txt"})));

    // `aa` appears twice in `aaa`: overlapping occurrences count.
    let text = refused(mcp.call(
        "fs_edit",
        json!({"path": "code.txt", "edits": [{"old": "first", "new": "1st"}, {"old": "aa", "new": "b"}]}),
    ));
    assert_eq!(
        text,
        "refused: the anchor of edit 2 appears 2 time(s) in code.txt; it must appear exactly \
         once. Nothing was changed"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("code.txt")).unwrap(),
        "first\naaa\n"
    );

    // Spliced, never a pattern: `$&` is text.
    ok(mcp.call(
        "fs_edit",
        json!({"path": "code.txt", "edits": [{"old": "first", "new": "$& 1st"}, {"old": "aaa", "new": "b"}]}),
    ));
    assert_eq!(
        std::fs::read_to_string(folder.join("code.txt")).unwrap(),
        "$& 1st\nb\n"
    );
}

#[test]
fn a_pipe_is_refused_by_read_and_edit_and_skipped_by_search() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::write(folder.join("plain.txt"), "needle\n").unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(folder.join("pipe"))
        .status()
        .unwrap();
    assert!(made.success());
    // A read of a pipe waits for a writer forever: the call, if it hangs, ends the test.
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog = finished.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(30));
        if !watchdog.load(std::sync::atomic::Ordering::SeqCst) {
            eprintln!("a call on a pipe never ended");
            std::process::exit(101);
        }
    });

    let read = refused(mcp.call("fs_read", json!({"path": "pipe"})));
    let edit = refused(mcp.call(
        "fs_edit",
        json!({"path": "pipe", "edits": [{"old": "a", "new": "b"}]}),
    ));
    let found = ok(mcp.call("search", json!({"pattern": "needle"})));
    finished.store(true, std::sync::atomic::Ordering::SeqCst);

    assert!(read.contains("pipe is not a regular file"), "{read}");
    assert!(edit.contains("pipe is not a regular file"), "{edit}");
    assert!(found.contains("plain.txt:1: needle"), "{found}");
}

#[test]
fn dot_dot_notes_is_inside_and_dot_dot_slash_x_is_outside() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::write(folder.join("..notes"), "inside\n").unwrap();
    std::fs::write(folder.parent().unwrap().join("x"), "outside\n").unwrap();
    std::os::unix::fs::symlink(folder.parent().unwrap().join("x"), folder.join("link")).unwrap();
    std::fs::create_dir_all(folder.join("src")).unwrap();

    assert!(ok(mcp.call("fs_read", json!({"path": "..notes"}))).contains("inside"));
    for path in [
        "../x",
        "src/../../x",
        folder.parent().unwrap().join("x").to_str().unwrap(),
        "link",
    ] {
        // Outside the place, a call is held for the user.
        let text = held(mcp.call("fs_read", json!({"path": path})));
        assert!(!text.contains("outside\n"), "never read: {text}");
    }
    // Up out of a folder that is not there: the kernel would not find it either.
    let text = refused(mcp.call("fs_read", json!({"path": "missing/../../x"})));
    assert!(
        text.contains("names a folder that does not exist and then goes up out of it"),
        "{text}"
    );
    held(mcp.call("fs_write", json!({"path": "../y", "content": "y"})));
    assert!(!folder.parent().unwrap().join("y").exists());
    let text = refused(mcp.call("fs_read", json!({"path": "README.md", "repository": "web"})));
    assert_eq!(
        text,
        "refused: the project has no repository web: it has `.` (the project folder itself)"
    );
}

#[test]
fn a_folder_given_as_repository_is_refused_with_the_field_to_use() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::create_dir_all(folder.join("apps/front")).unwrap();
    let said = |field: &str| {
        format!(
            "refused: apps/front is a folder of the repository `.` (the project folder itself), \
             not a repository: give {field}"
        )
    };

    let text = refused(mcp.call("fs_list", json!({"path": ".", "repository": "apps/front"})));
    assert_eq!(text, said("`path: \"apps/front\"`"));
    let text = refused(mcp.call(
        "search",
        json!({"pattern": "x", "repository": "apps/front"}),
    ));
    assert_eq!(text, said("`glob: \"apps/front/**\"`"));
    let text = refused(mcp.call("run", json!({"line": "true", "repository": "apps/front"})));
    assert_eq!(
        text,
        said("the folder in the command line, as an argument of the program")
    );
}

#[test]
fn a_place_with_one_repository_needs_no_repository_argument() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::write(folder.join("a.rs"), "fn needle() {}\n").unwrap();

    assert!(ok(mcp.call("fs_list", json!({"path": "."}))).contains("a.rs"));
    assert!(ok(mcp.call("search", json!({"pattern": "needle"}))).contains("a.rs:1"));
    let run = run_id(&ok(mcp.call("run", json!({"line": "true"}))));
    assert!(!run.is_empty());
}

#[test]
fn listing_and_searching_find_the_files_of_the_place() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::create_dir_all(folder.join("src/deep")).unwrap();
    std::fs::write(folder.join("src/a.rs"), "fn Main() {}\n").unwrap();
    std::fs::write(folder.join("src/deep/b.rs"), "// the main thing\n").unwrap();
    std::fs::write(folder.join("ignored.log"), "main\n").unwrap();
    std::fs::write(folder.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(folder.join("blob.bin"), b"main\0\x01").unwrap();

    let listed = ok(mcp.call("fs_list", json!({"path": ".", "depth": 3})));
    assert!(listed.contains("src/deep/b.rs"), "{listed}");
    assert!(listed.contains("src/  folder"), "{listed}");
    assert!(!listed.contains(".git/"), "{listed}");

    let found = ok(mcp.call("search", json!({"pattern": "MAIN"})));
    assert!(found.starts_with("2 match(es) for \"MAIN\""), "{found}");
    assert!(found.contains("src/a.rs:1: fn Main() {}"), "{found}");
    assert!(
        found.contains("src/deep/b.rs:1: // the main thing"),
        "{found}"
    );
    assert!(!found.contains("ignored.log:"), "{found}");
    assert!(
        found.contains("blob.bin"),
        "the binary file is named: {found}"
    );
    let only = ok(mcp.call("search", json!({"pattern": "main", "glob": "**/b.rs"})));
    assert!(only.starts_with("1 match(es)"), "{only}");
}

// Sizes.

#[test]
fn reading_a_1_mib_file_returns_a_page_under_40000_characters_that_names_the_next_offset() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    let mut text = String::new();
    let mut n = 0;
    while text.len() < 1024 * 1024 {
        n += 1;
        text.push_str(&format!("line {n} of the big file\n"));
    }
    std::fs::write(folder.join("big.txt"), &text).unwrap();
    // A file of empty lines numbers a line for every byte.
    std::fs::write(folder.join("empty-lines.txt"), "\n".repeat(1024 * 1024)).unwrap();
    // And one line with no end.
    std::fs::write(folder.join("one-line.txt"), "x".repeat(1024 * 1024)).unwrap();

    let page = ok(mcp.call("fs_read", json!({"path": "big.txt"})));
    assert!(page.chars().count() < RESULT_LIMIT, "{}", page.len());
    assert!(
        page.starts_with("     1\tline 1 of the big file\n"),
        "{}",
        &page[..80]
    );
    let next = page
        .split("the next page starts at offset ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .unwrap_or_else(|| panic!("no next offset: {}", &page[page.len() - 300..]))
        .parse::<u64>()
        .unwrap();
    assert!(next > 0);
    let second = ok(mcp.call(
        "fs_read",
        json!({"path": "big.txt", "range": {"offset": next}}),
    ));
    let first_line = &text[next as usize..].lines().next().unwrap().to_owned();
    assert!(second.contains(first_line), "{}", &second[..80]);
    let last_number = page
        .lines()
        .rev()
        .find_map(|l| l.split('\t').next()?.trim().parse::<u64>().ok())
        .unwrap();
    assert!(
        second.starts_with(&format!("{:6}\t", last_number + 1)),
        "the numbering goes on: {}",
        &second[..40]
    );

    for file in ["empty-lines.txt", "one-line.txt"] {
        let page = ok(mcp.call("fs_read", json!({"path": file})));
        assert!(
            page.chars().count() < RESULT_LIMIT,
            "{file}: {}",
            page.len()
        );
        assert!(page.contains("the next page starts at offset"), "{file}");
    }
    let asked = refused(mcp.call(
        "fs_read",
        json!({"path": "big.txt", "range": {"length": 10_000_000}}),
    ));
    assert!(asked.contains("length"), "{asked}");
}

#[test]
fn a_search_with_thousands_of_matches_stays_under_the_limit_and_says_how_to_read_on() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::create_dir(folder.join("many")).unwrap();
    for file in 0..50 {
        let lines: String = (0..100)
            .map(|line| format!("needle {file}-{line} {}\n", "x".repeat(300)))
            .collect();
        std::fs::write(folder.join(format!("many/{file:02}.txt")), lines).unwrap();
    }

    let found = ok(mcp.call("search", json!({"pattern": "needle"})));
    assert!(found.chars().count() < RESULT_LIMIT, "{}", found.len());
    let cursor = found
        .split("cursor ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no cursor: {}", &found[..400]))
        .trim_matches(|c| c == '"' || c == ':' || c == ')' || c == ',')
        .to_owned();
    let last = found.lines().last().unwrap().to_owned();
    let more = ok(mcp.call("search", json!({"pattern": "needle", "cursor": cursor})));
    assert!(more.chars().count() < RESULT_LIMIT);
    let first_more = more
        .lines()
        .find(|l| l.starts_with("many/"))
        .unwrap()
        .to_owned();
    assert_ne!(first_more, last, "the next page starts after the last one");
    assert!(!found.contains(&format!("{first_more}\n")));
}

#[test]
fn a_command_printing_megabytes_stays_under_the_limit_and_says_how_to_read_on() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, _) = session(&user);

    for line in ["seq 1 500000", "seq -s , 1 400000"] {
        let answer = ok(mcp.call("run", json!({"line": line})));
        assert!(
            answer.chars().count() < RESULT_LIMIT,
            "{line}: {}",
            answer.len()
        );
        assert!(answer.contains("exited with code 0"), "{line}: {answer}");
        let run = run_id(&answer);
        let output = ok(mcp.call("run_status", json!({"run": run, "tail": 1000})));
        assert!(
            output.chars().count() < RESULT_LIMIT,
            "{line}: {}",
            output.len()
        );
        assert!(
            output.contains("earlier lines: run_status"),
            "{line}: {}",
            &output[output.len().saturating_sub(400)..]
        );
    }
    let answer = ok(mcp.call("run", json!({"line": "seq 1 500000"})));
    let output = ok(mcp.call("run_status", json!({"run": run_id(&answer)})));
    assert!(output.contains("500000"), "the tail is the end: {output}");
}

// Commands.

#[test]
fn shell_syntax_is_refused_with_the_sentence() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);

    let script = "or put the line in a script of the repository and run the script.";
    for (line, token, instead) in [
        (
            "echo a | cat",
            "|",
            format!(
                "run starts one program and hands its output to no other: read a file with \
                 fs_read, search the project with search, {script}"
            ),
        ),
        (
            "true && false",
            "&&",
            format!("run starts one program: run each one with its own call, {script}"),
        ),
        (
            "echo $(x)",
            "$(",
            format!(
                "a shell would run a command there and none does here: run that command with \
                 its own call to run and write its result in the line, {script}"
            ),
        ),
        (
            "echo a > f",
            ">",
            format!(
                "run starts one program and keeps its output for you to read with run_status: \
                 write a file with fs_write, read one with fs_read, {script}"
            ),
        ),
        (
            "ls *",
            "*",
            "put the word in single quotes, so that the program receives it as it is: no shell \
             expands it here. This line passes: ls '*'"
                .into(),
        ),
    ] {
        let text = refused(mcp.call("run", json!({"line": line})));
        assert_eq!(
            text,
            format!(
                "Flottille runs a command without a shell, and “{token}” is shell syntax: \
                 {instead}"
            ),
            "{line}"
        );
    }
    assert!(!folder.join("f").exists());
    let text = refused(mcp.call("run", json!({"task": "nope"})));
    assert_eq!(
        text,
        "refused: the project has no task nope: list them with tasks {\"action\": \"list\"}"
    );
    let text = refused(mcp.call("run", json!({"task": "nope", "line": "ls"})));
    assert!(text.contains("exactly one"), "{text}");
}

#[test]
fn shell_syntax_inside_quotes_reaches_the_program_as_its_text() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, folder) = session(&user);

    let answer = ok(mcp.call(
        "run",
        json!({"line": r#"printf "[%s]\n" "a | b && c > d" 'e; f >> g' "*""#}),
    ));
    assert!(answer.contains("exited with code 0"), "{answer}");
    let output = ok(mcp.call("run_status", json!({"run": run_id(&answer)})));
    assert!(
        output.contains("[a | b && c > d]\n[e; f >> g]\n[*]"),
        "{output}"
    );
    for written in ["d", "g"] {
        assert!(!folder.join(written).exists(), "{written}");
    }
}

#[test]
fn a_long_command_answers_after_its_timeout_with_it_keeps_going_and_stop_ends_its_tree() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, folder) = session(&user);

    let started = Instant::now();
    std::fs::write(
        folder.join("tree.sh"),
        "sleep 300 & echo $! > child.pid; echo started; wait\n",
    )
    .unwrap();
    let answer = ok(mcp.call("run", json!({"line": "sh tree.sh", "timeout": 1})));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(answer.contains("is running"), "{answer}");
    assert!(
        answer.contains("it keeps going: read it with run_status"),
        "{answer}"
    );
    let run = run_id(&answer);
    eventually("the child's pid", || folder.join("child.pid").exists());
    let child: u64 = std::fs::read_to_string(folder.join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(child));

    let output = ok(mcp.call("run_status", json!({"run": run})));
    assert!(output.contains("is running"), "{output}");
    assert!(output.contains("started"), "{output}");

    let stopped = ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
    assert!(stopped.contains("was stopped"), "{stopped}");
    eventually("the child's end", || !alive(child));
    let output = ok(mcp.call("run_status", json!({"run": run})));
    assert!(output.contains("was stopped"), "{output}");

    let text = refused(mcp.call("run_status", json!({"run": "000000000000"})));
    assert_eq!(
        text,
        "refused: 000000000000 is not a run this agent started"
    );
}

/// A run that ignores `SIGTERM`, as its group inherits the ignoring: only the kill ends it.
fn stubborn(folder: &Path, seconds: u32) -> Value {
    std::fs::write(
        folder.join("stubborn.sh"),
        "trap '' TERM; echo $$ > pid-$1; sleep $1\n",
    )
    .unwrap();
    json!({"line": format!("sh stubborn.sh {seconds}"), "timeout": 1})
}

#[test]
fn stopping_a_run_kills_after_the_grace_one_that_ignores_sigterm() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, folder) = session(&user);
    let run = run_id(&ok(mcp.call("run", stubborn(&folder, 300))));
    eventually("the shell's pid", || folder.join("pid-300").exists());
    let pid: u64 = std::fs::read_to_string(folder.join("pid-300"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));

    let asked = Instant::now();
    let stopped = ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));

    // It answers once the run ended: not before the 5 s of grace, since the term was ignored.
    assert!(
        asked.elapsed() >= Duration::from_secs(4),
        "{:?}",
        asked.elapsed()
    );
    assert!(
        asked.elapsed() < Duration::from_secs(15),
        "{:?}",
        asked.elapsed()
    );
    assert!(stopped.contains("was stopped"), "{stopped}");
    assert!(!alive(pid));
}

#[test]
fn a_daemon_that_stops_stops_its_runs_together_not_one_after_another() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, folder) = session(&user);
    let watch = user.watch(&["--after", "0"]);
    let runs: Vec<String> = [301, 302, 303]
        .into_iter()
        .map(|seconds| run_id(&ok(mcp.call("run", stubborn(&folder, seconds)))))
        .collect();
    eventually("the shells", || {
        [301, 302, 303]
            .iter()
            .all(|n| folder.join(format!("pid-{n}")).exists())
    });

    let asked = Instant::now();
    let stop = user.run(&["daemon", "stop"]);
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    let events = watch.until(|e| e["name"] == "daemon.stopping");

    // Each takes the 5 s of grace: one after another would be 15 s.
    assert!(
        asked.elapsed() < Duration::from_secs(12),
        "{:?}",
        asked.elapsed()
    );
    for run in &runs {
        let ended = events
            .iter()
            .find(|e| e["name"] == "run.ended" && e["data"]["run"] == run.as_str())
            .unwrap_or_else(|| panic!("run {run} did not end before the daemon said it stops"));
        assert_eq!(ended["data"]["state"], "interrupted");
    }
}

#[test]
fn a_retry_with_the_same_call_key_does_not_run_twice() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, folder) = session(&user);
    std::fs::write(
        folder.join("count.sh"),
        "echo once >> count.txt; echo done\n",
    )
    .unwrap();
    let line = json!({"line": "sh count.sh"});

    let first = ok(mcp.call_keyed("run", line.clone(), "toolu_01"));
    let again = ok(mcp.call_keyed("run", line.clone(), "toolu_01"));
    assert_eq!(first, again);
    assert_eq!(
        std::fs::read_to_string(folder.join("count.txt")).unwrap(),
        "once\n"
    );
    ok(mcp.call_keyed("run", line, "toolu_02"));
    assert_eq!(
        std::fs::read_to_string(folder.join("count.txt")).unwrap(),
        "once\nonce\n"
    );
}

#[test]
fn gh_token_is_empty_in_a_run() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_env("GH_TOKEN", "ghp_fakefakefakefakefakefakefake0001")
        .with_env("GITLAB_TOKEN", "glpat-fakefakefakefakefake");
    let (mcp, _) = session(&user);

    let answer = ok(mcp.call("run", json!({"line": "env"})));
    let output = ok(mcp.call("run_status", json!({"run": run_id(&answer), "tail": 1000})));
    let value = |name: &str| {
        output
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .map(str::to_owned)
    };
    assert_eq!(value("GH_TOKEN").as_deref(), Some(""), "{output}");
    assert_eq!(value("GITLAB_TOKEN").as_deref(), Some(""), "{output}");
    assert!(!output.contains("fakefake"), "{output}");
    let config = value("GH_CONFIG_DIR").expect("GH_CONFIG_DIR is set");
    assert!(Path::new(&config).is_dir());
    assert_eq!(std::fs::read_dir(&config).unwrap().count(), 0);
    assert_eq!(value("GLAB_CONFIG_DIR").as_deref(), Some(config.as_str()));
    assert_eq!(value("GIT_CONFIG_COUNT").as_deref(), Some("2"));
    // No hook of the repository, which the agent writes, runs; and a push from a line that is
    // not itself a push the gate let through goes nowhere.
    assert_eq!(
        value("GIT_CONFIG_KEY_0").as_deref(),
        Some("core.hooksPath"),
        "{output}"
    );
    assert_eq!(
        value("GIT_CONFIG_VALUE_0").as_deref(),
        Some("/dev/null"),
        "{output}"
    );
    assert_eq!(
        value("GIT_CONFIG_KEY_1").as_deref(),
        Some("url./dev/null/flottille-refused-push/.pushInsteadOf"),
        "{output}"
    );
}

#[test]
fn a_run_in_a_workspace_gets_its_variables() {
    let user = User::with_fake_agent();
    let folder = project(&user);
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(
        "\n[variables]\nSTORE = \"{workspace.path}/.store\"\nSTACK = \"app-{workspace}\"\n\n\
         [commands.where]\nline = \"printenv STORE STACK\"\n",
    );
    std::fs::write(&file, text).unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    let workspace = PathBuf::from(workspace["folder"].as_str().unwrap());
    let (_, url, authorization) = agent(&user, json!({"workspace": "login"}));
    let mcp = Mcp::initialize(&url, &authorization);

    let listed = ok(mcp.call("tasks", json!({"action": "list"})));
    assert!(
        listed.contains("- where: `printenv STORE STACK`"),
        "{listed}"
    );
    let answer = ok(mcp.call("run", json!({"task": "where"})));
    assert!(answer.contains("exited with code 0"), "{answer}");
    assert!(
        answer.contains(&format!("{}/.store\napp-login", workspace.display())),
        "{answer}"
    );
    // The place is the workspace: the main checkout is outside it, and asked for.
    held(mcp.call(
        "fs_read",
        json!({"path": folder.join("README.md").to_str().unwrap()}),
    ));
}

// Records.

#[test]
fn every_call_is_recorded_with_its_arguments_masked() {
    let user = User::with_fake_agent();
    project(&user);
    workspace(&user);
    let (id, url, authorization) = agent(&user, json!({"workspace": "w"}));
    let mcp = Mcp::initialize(&url, &authorization);
    ok(mcp.call("fs_read", json!({"path": "README.md"})));
    refused(mcp.call(
        "run",
        json!({"line": "echo ghp_fakefakefakefakefakefakefake0002 | cat"}),
    ));
    let answer = ok(mcp.call("run", json!({"line": "true"})));

    let events = user.call("agent.show", json!({"agent": id})).unwrap()["events"]
        .as_array()
        .unwrap()
        .clone();
    let served: Vec<&Value> = events
        .iter()
        .filter(|e| e["name"] == "agent.tool_served")
        .collect();
    assert_eq!(served.len(), 3, "{events:?}");
    assert_eq!(served[0]["data"]["tool"], "fs_read");
    assert_eq!(served[0]["data"]["outcome"], "ok");
    assert_eq!(served[0]["data"]["arguments"], json!({"path": "README.md"}));
    assert!(served[0]["data"]["duration_ms"].is_u64());
    assert_eq!(served[1]["data"]["outcome"], "refused");
    assert!(
        served[1]["data"]["arguments"]["line"]
            .as_str()
            .unwrap()
            .contains("[masked]"),
        "{:?}",
        served[1]
    );
    let run = run_id(&answer);
    let all = user
        .watch(&["--after", "0"])
        .until(|e| e["name"] == "run.ended" && e["data"]["run"] == run.as_str());
    let started = all
        .iter()
        .find(|e| e["name"] == "run.started" && e["data"]["run"] == run.as_str())
        .expect("run.started");
    assert_eq!(started["data"]["line"], "true");
    let ended = all.last().unwrap();
    assert_eq!(ended["data"]["agent"], id.as_str());
    assert_eq!(ended["data"]["state"], "exited");
    assert_eq!(ended["data"]["exit_code"], 0);
}

#[test]
fn a_run_s_line_is_masked_in_its_record_its_event_and_its_answers() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);
    let token = "ghp_fakefakefakefakefakefakefake0003";

    let answer = ok(mcp.call("run", json!({"line": format!("echo {token}")})));
    let run = run_id(&answer);

    let all = user
        .watch(&["--after", "0"])
        .until(|e| e["name"] == "run.ended" && e["data"]["run"] == run.as_str());
    let started = all
        .iter()
        .find(|e| e["name"] == "run.started" && e["data"]["run"] == run.as_str())
        .expect("run.started");
    let line = started["data"]["line"].as_str().unwrap();
    assert!(line.contains("[masked]") && !line.contains(token), "{line}");
    assert!(!answer.contains(token), "{answer}");
    // Once ended, the answer is read from the store.
    let stored = ok(mcp.call("run_status", json!({"run": run})));
    assert!(
        stored.contains("[masked]") && !stored.contains(token),
        "{stored}"
    );
}

#[test]
fn every_call_goes_through_the_gate_stopping_a_run_and_reading_its_output_included() {
    let user = User::with_fake_agent().with_allowing_judge();
    project(&user);
    workspace(&user);
    let (id, url, authorization) = agent(&user, json!({"workspace": "w"}));
    let mcp = Mcp::initialize(&url, &authorization);
    let answer = ok(mcp.call("run", json!({"line": "sleep 300", "timeout": 1})));
    let run = run_id(&answer);
    ok(mcp.call("run_status", json!({"run": run})));
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
    ok(mcp.call("tasks", json!({"action": "list"})));
    held(mcp.call("fs_read", json!({"path": ".env"})));
    refused(mcp.call("run", json!({"line": "ls | wc"})));

    let events = user.call("agent.show", json!({"agent": id})).unwrap()["events"]
        .as_array()
        .unwrap()
        .clone();
    let verdicts: Vec<(String, String, String)> = events
        .iter()
        .filter(|e| e["name"] == "agent.tool_served")
        .map(|e| {
            let data = &e["data"];
            let word = |field: &str| data[field].as_str().unwrap_or("-").to_owned();
            (word("tool"), word("gate"), word("outcome"))
        })
        .collect();
    let row = |tool: &str, gate: &str, outcome: &str| {
        (tool.to_owned(), gate.to_owned(), outcome.to_owned())
    };
    assert_eq!(
        verdicts,
        [
            row("run", "judged", "ok"),
            row("run_status", "allow", "ok"),
            row("run_status", "allow", "ok"),
            row("tasks", "allow", "ok"),
            row("fs_read", "ask", "held"),
            row("run", "deny", "refused"),
        ]
    );
}

// From an agent's session.

#[test]
fn an_agent_calls_a_tool_over_http_from_its_session() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "look", "do": [
        {"mcp": {"tool": "fs_list", "arguments": {"path": "."}}},
        {"say": "seen"}
    ]}]}));
    project(&user);
    let (id, _, _) = agent(&user, json!({"message": "look around"}));

    eventually("the call", || {
        user.received().iter().any(|e| e.get("mcp").is_some())
    });
    let call = user
        .received()
        .into_iter()
        .find_map(|e| e.get("mcp").cloned())
        .unwrap();
    assert_eq!(call["status"], 200, "{call}");
    assert!(
        call["body"]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("README.md"),
        "{call}"
    );
    eventually("the record", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "agent.tool_served" && e["data"]["tool"] == "fs_list")
    });
}

// Clients an agent started (issue #91): the daemon's socket is the user's, never an agent's.

const REFUSED_CLIENT: &str =
    "refused: this client was started by an agent; agents act only through their tools";

/// A script asking the daemon to stop, as the user's own command line would, from a run: the
/// gate does not read a script, and the run's environment no longer says where the socket is.
fn stop_script(user: &User, out: &str) -> String {
    format!(
        "XDG_RUNTIME_DIR='{}' exec '{}' daemon stop > {out} 2>&1\n",
        user.path("runtime").display(),
        env!("CARGO_BIN_EXE_flottille")
    )
}

fn read_when_done(path: &Path) -> String {
    eventually(&format!("{}", path.display()), || {
        std::fs::read_to_string(path).is_ok_and(|text| !text.is_empty())
    });
    // What the client prints comes in one write before it exits.
    std::thread::sleep(Duration::from_millis(100));
    std::fs::read_to_string(path).unwrap()
}

/// The pid of the process's parent, from its `stat` (`ppid` comes after the name).
fn parent_of(stat: &str) -> u64 {
    let (_, after) = stat.rsplit_once(')').unwrap();
    after.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// The daemon's children that ended and that nobody waited for.
fn zombies_of(daemon: u64) -> Vec<u64> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u64>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                let state = stat.rsplit_once(')').unwrap().1.trim_start();
                state.starts_with('Z') && parent_of(&stat) == daemon
            })
        })
        .collect()
}

#[test]
fn a_client_started_through_run_is_refused_and_nothing_it_sent_is_done() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, folder) = session(&user);
    let pid = user.status()["pid"].as_u64().unwrap();
    std::fs::write(folder.join("client.sh"), stop_script(&user, "client.out")).unwrap();

    let answer = ok(mcp.call("run", json!({"line": "sh client.sh"})));
    assert!(answer.contains("exited with code 1"), "{answer}");
    let said = std::fs::read_to_string(folder.join("client.out")).unwrap();
    assert!(said.contains(REFUSED_CLIENT), "{said}");

    // It asked the daemon to stop: the same daemon still runs.
    assert_eq!(user.status()["pid"].as_u64(), Some(pid));
    // Said once in the daemon's log.
    let log = std::fs::read_to_string(user.path("state/flottille/daemon.log")).unwrap();
    assert_eq!(
        log.matches("refused a client started by an agent").count(),
        1,
        "{log}"
    );
}

#[test]
fn a_client_that_outlives_its_command_and_its_parent_is_still_refused() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, folder) = session(&user);
    let pid = user.status()["pid"].as_u64().unwrap();
    // A double fork out of the run's session: `setsid -f` starts `late.sh` in a session of its
    // own, which leaves `connect.sh` behind and ends. The run's group is killed when the command
    // ends, and `setsid -f` forks before it detaches: `orphan.sh` ends only once `late.sh` runs
    // (so is detached), and `connect.sh` connects only once the command is gone and `late.sh`
    // has left it behind, each seen as it happens rather than after a delay.
    std::fs::write(
        folder.join("orphan.sh"),
        "setsid -f sh late.sh $$\n\
         for i in $(seq 200); do [ -e detached ] && break; sleep 0.05; done\n",
    )
    .unwrap();
    std::fs::write(
        folder.join("late.sh"),
        "sh connect.sh $$ $1 &\ntouch detached\n",
    )
    .unwrap();
    std::fs::write(
        folder.join("connect.sh"),
        format!(
            "late=$1\n\
             command=$2\n\
             for i in $(seq 200); do [ -e /proc/$command ] || break; sleep 0.05; done\n\
             for i in $(seq 200); do\n\
             read -r line < /proc/$$/stat\n\
             set -- ${{line##*) }}\n\
             [ \"$2\" != \"$late\" ] && break\n\
             sleep 0.05\n\
             done\n\
             cat /proc/$$/stat > stat\n{}",
            stop_script(&user, "orphan.out")
        ),
    )
    .unwrap();

    let answer = ok(mcp.call("run", json!({"line": "sh orphan.sh"})));
    assert!(answer.contains("exited with code 0"), "{answer}");
    let said = read_when_done(&folder.join("orphan.out"));
    assert!(said.contains(REFUSED_CLIENT), "{said}");
    // Orphaned, it was taken in by the daemon, not by init: still its descendant.
    let stat = std::fs::read_to_string(folder.join("stat")).unwrap();
    assert_eq!(parent_of(&stat), pid);
    assert_eq!(user.status()["pid"].as_u64(), Some(pid));
    // And the daemon waits for what it took in: no child of another's is left a zombie.
    eventually("the strays reaped", || zombies_of(pid).is_empty());
}

#[test]
fn run_flottille_project_list_is_refused_by_the_gate() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);

    for line in ["flottille project list", "env flottille daemon stop"] {
        let text = refused(mcp.call("run", json!({"line": line})));
        assert!(
            text.starts_with("refused: the flottille command drives Flottille"),
            "{line}: {text}"
        );
    }
}

#[test]
fn a_run_s_environment_does_not_say_where_the_daemon_is() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, _) = session(&user);

    let answer = ok(mcp.call("run", json!({"line": "env"})));
    let output = ok(mcp.call("run_status", json!({"run": run_id(&answer)})));
    assert!(output.contains("HOME="), "{output}");
    assert!(!output.contains("XDG_RUNTIME_DIR="), "{output}");
}

#[test]
fn a_client_started_outside_the_agents_works_as_before() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);
    ok(mcp.call("run", json!({"line": "true"})));

    assert!(user.call("daemon.status", json!({})).is_ok());
    let listed = user.run(&["project", "list", "--json"]);
    assert!(listed.status.success(), "{listed:?}");
    assert!(String::from_utf8_lossy(&listed.stdout).contains("app"));
}

#[test]
fn run_status_reads_by_default_stops_on_request_and_refuses_what_does_not_fit() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, _) = session(&user);
    let answer = ok(mcp.call("run", json!({"line": "sleep 300", "timeout": 1})));
    let run = run_id(&answer);
    assert!(
        answer.contains(&format!(
            "run_status {{\"run\": \"{run}\", \"action\": \"stop\"}}"
        )),
        "the way to stop it: {answer}"
    );

    let state = ok(mcp.call("run_status", json!({"run": run})));
    assert!(state.contains("is running"), "{state}");
    for (arguments, said) in [
        (
            json!({"run": run, "action": "stop", "tail": 5}),
            "refused: tail and start are for action output; action stop only stops the run and \
             answers with the end of its output",
        ),
        (
            json!({"run": run, "action": "kill"}),
            "refused: run_status has no action \"kill\": its actions are output (the default), \
             stop and notify",
        ),
        (json!({"run": run, "tail": 5000}), "tail must be 1 to 1000"),
        (
            json!({"run": "000000000000"}),
            "is not a run this agent started",
        ),
    ] {
        let text = refused(mcp.call("run_status", arguments.clone()));
        assert!(text.contains(said), "{arguments}: {text}");
    }
    // Nothing was stopped by the refusals.
    let state = ok(mcp.call("run_status", json!({"run": run})));
    assert!(state.contains("is running"), "{state}");
    let stopped = ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
    assert!(stopped.contains("was stopped"), "{stopped}");
    // The two tools it replaces are gone.
    for old in ["commands_output", "commands_stop"] {
        let text = refused(mcp.call(old, json!({"run": run})));
        assert!(text.contains(&format!("has no tool {old}")), "{text}");
    }
}

// Tools that change while a session runs (`docs/tools.md`, "When the tools change").

/// How many `notifications/tools/list_changed` reached a client.
#[derive(Clone, Default)]
struct Told(Arc<AtomicUsize>);

impl ClientHandler for Told {
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        info
    }

    async fn on_tool_list_changed(&self, _: NotificationContext<RoleClient>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// rmcp's client, as an agent's own would be, counting the notices it gets.
struct Noticing {
    runtime: tokio::runtime::Runtime,
    client: RunningService<RoleClient, Told>,
    told: Told,
}

impl Noticing {
    fn connect(url: &str, authorization: &str, lifecycle: ClientLifecycleMode) -> Noticing {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let token = authorization.strip_prefix("Bearer ").unwrap();
        let config = StreamableHttpClientTransportConfig::with_uri(url).auth_header(token);
        let told = Told::default();
        let client = runtime.block_on(async {
            told.clone()
                .serve_with_lifecycle(
                    StreamableHttpClientTransport::from_config(config),
                    lifecycle,
                )
                .await
                .expect("the client connects")
        });
        Noticing {
            runtime,
            client,
            told,
        }
    }

    fn told(&self) -> usize {
        // Notices are handled on the client's runtime.
        self.runtime
            .block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        self.told.0.load(Ordering::SeqCst)
    }

    fn names(&self) -> Vec<String> {
        self.runtime
            .block_on(self.client.list_all_tools())
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    fn call(&self, tool: &str, arguments: Value) -> Answer {
        let params = CallToolRequestParams::new(tool.to_owned())
            .with_arguments(arguments.as_object().cloned().unwrap_or_default());
        let result = self
            .runtime
            .block_on(self.client.call_tool(params))
            .unwrap();
        let result = serde_json::to_value(result).unwrap();
        Answer {
            text: result["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            error: result["isError"] == true,
        }
    }
}

/// The project declares its forge and a tracker, or neither: its sessions gain or lose `forge`
/// and `tickets`.
fn track(user: &User, tracked: bool) {
    declare(user, tracked, tracked);
}

/// The project declares its forge, a tracker, both or neither.
fn declare(user: &User, forge: bool, tracker: bool) {
    let forge = match forge {
        true => json!({"kind": "github", "repository": "acme/app"}),
        false => Value::Null,
    };
    let tracker = match tracker {
        true => json!({"kind": "github", "repository": "."}),
        false => Value::Null,
    };
    // A forge is declared on a remote that is on it.
    user.git(
        &user.path("work/app"),
        &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
    );
    user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [{"path": ".", "forge": forge}],
            "tracker": tracker}),
    )
    .unwrap();
}

fn shown(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"].clone()
}

#[test]
fn a_running_session_is_told_its_tools_changed() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, url, authorization) = agent(&user, json!({}));

    // A 2025-11-25 client keeps no stream open: the notice comes with its next answer, once.
    let old = Noticing::connect(&url, &authorization, ClientLifecycleMode::Initialize);
    let (_, body, _) = post(
        &url,
        &[("Authorization", &authorization)],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}}}),
    );
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["result"]["capabilities"]["tools"]["listChanged"], true);
    assert!(!old.names().contains(&"tickets".to_owned()));
    track(&user, true);
    ok(old.call("fs_list", json!({"path": "."})));
    assert_eq!(old.told(), 1);
    ok(old.call("fs_list", json!({"path": "."})));
    assert_eq!(old.told(), 1, "told once");

    // A 2026-07-28 client that listens is told on its stream, without a request.
    let (_, url, authorization) = agent(&user, json!({}));
    let new = Noticing::connect(
        &url,
        &authorization,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    );
    let filter: SubscriptionFilter =
        serde_json::from_value(json!({"toolsListChanged": true})).unwrap();
    let mut listening = new
        .runtime
        .block_on(new.client.peer().listen(filter))
        .expect("the server accepts a listen for its tools");
    track(&user, false);
    let notice = new
        .runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(10), listening.next()).await })
        .expect("a notice within 10 s")
        .expect("the stream works")
        .expect("the stream is open");
    let notice = serde_json::to_value(notice).unwrap();
    assert_eq!(
        notice["method"], "notifications/tools/list_changed",
        "{notice}"
    );
}

#[test]
fn the_new_list_is_served_after_the_change() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, url, authorization) = agent(&user, json!({}));
    let mcp = Mcp::initialize(&url, &authorization);
    let text = refused(mcp.call("tickets", json!({"action": "list"})));
    assert!(text.contains("has no tool tickets"), "{text}");

    track(&user, true);
    let names = mcp.names();
    let at = names.iter().position(|name| name == "tickets");
    assert_eq!(
        at,
        Some(10),
        "after skill_read, as a new session lists it: {names:?}"
    );
    let answer = mcp.call("tickets", json!({"action": "list"}));
    assert!(!answer.text.contains("has no tool"), "{}", answer.text);
    let tools = shown(&user, &id)["tools"].clone();
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t.as_str().unwrap().ends_with("tickets")),
        "{tools}"
    );

    // And back: a tool the role lost is no longer listed nor called. The client keeps the list
    // it read until it is told, with the answer to its next call.
    track(&user, false);
    let text = refused(mcp.call("tickets", json!({"action": "list"})));
    assert!(text.contains("has no tool tickets"), "{text}");
    assert!(!mcp.names().contains(&"tickets".to_owned()));
}

#[test]
fn a_session_started_before_an_update_gets_the_new_list_at_its_next_process() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, _, _) = agent(&user, json!({}));
    eventually("the first turn ends", || {
        shown(&user, &id)["state"] == "idle"
    });
    // An older version gave its sessions no `web` and no `wake`.
    assert!(user.run(&["daemon", "stop"]).status.success());
    {
        let mut store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
        let older: Vec<String> = store
            .agent(&id)
            .unwrap()
            .unwrap()
            .tools
            .into_iter()
            .filter(|tool| !tool.ends_with("web") && !tool.ends_with("wake"))
            .collect();
        store
            .set_agent_tools(&id, &older, json!({"agent": id}))
            .unwrap();
    }

    let reopened = user.received_requests("session/resume").len();
    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let server = loop {
        if let Some(resumed) = user.received_requests("session/resume").get(reopened) {
            break resumed["params"]["mcpServers"][0].clone();
        }
        assert!(Instant::now() < deadline, "the session was not resumed");
        std::thread::sleep(Duration::from_millis(20));
    };
    let authorization = server["headers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"] == "Authorization")
        .unwrap()["value"]
        .as_str()
        .unwrap()
        .to_owned();
    let mcp = Mcp::initialize(server["url"].as_str().unwrap(), &authorization);
    let names = mcp.names();
    assert!(names.contains(&"web".to_owned()), "{names:?}");
    assert!(names.contains(&"wake".to_owned()), "{names:?}");
    let tools = shown(&user, &id)["tools"].clone();
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t.as_str().unwrap().ends_with("web")),
        "{tools}"
    );
}

#[test]
fn a_session_whose_client_ignores_the_change_is_said_to_run_an_older_list() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, url, authorization) = agent(&user, json!({}));
    // rmcp's client does nothing with the notice: it lists nothing again.
    let mcp = Mcp::initialize(&url, &authorization);
    mcp.names();
    assert_eq!(shown(&user, &id)["tools_outdated"], false);

    track(&user, true);
    ok(mcp.call("fs_list", json!({"path": "."})));
    // Told with that answer, it calls again on its older list.
    ok(mcp.call("fs_list", json!({"path": "."})));
    eventually("the session is said to run an older list", || {
        shown(&user, &id)["tools_outdated"] == true
    });
    let tree = user.call("agent.tree", json!({"agent": id})).unwrap();
    assert_eq!(tree["agents"][0]["agent"]["tools_outdated"], true, "{tree}");
    let out = user.run(&["agent", "show", &id]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(
            "runs an older list of tools than its role has now: reload them (`flottille agent \
             reload-tools`) for the new one"
        ),
        "{text}"
    );

    // Once it lists them, it has the new list.
    assert!(mcp.names().contains(&"tickets".to_owned()));
    eventually("the session has the new list", || {
        shown(&user, &id)["tools_outdated"] == false
    });
}

#[test]
fn a_session_whose_agent_cannot_take_new_tools_is_said_to_run_an_older_list_at_once() {
    let user = User::with_fake_providers();
    project(&user);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "hello", "provider": "opencode"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("its process runs", || shown(&user, &id)["state"] == "idle");
    assert_eq!(shown(&user, &id)["tools_outdated"], false);

    // OpenCode's permissions, written when its process started, deny the tools it gains.
    track(&user, true);
    assert_eq!(shown(&user, &id)["tools_outdated"], true);
}

/// The `outdated` of each `agent.tools_outdated` of the agent, in order.
fn said_outdated(user: &User, id: &str) -> Vec<bool> {
    let shown = user.call("agent.show", json!({"agent": id})).unwrap();
    shown["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == "agent.tools_outdated")
        .map(|e| e["data"]["outdated"].as_bool().unwrap())
        .collect()
}

#[test]
fn a_session_that_ignored_a_change_lists_after_the_next_and_is_no_longer_behind() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, url, authorization) = agent(&user, json!({}));
    let mcp = Mcp::initialize(&url, &authorization);
    mcp.names();

    // It ignores change A: told with an answer, it calls again without listing.
    track(&user, true);
    ok(mcp.call("fs_list", json!({"path": "."})));
    ok(mcp.call("fs_list", json!({"path": "."})));
    assert_eq!(shown(&user, &id)["tools_outdated"], true);
    // Change B comes, to a list it does not hold either (the forge alone): it is still behind.
    declare(&user, true, false);
    ok(mcp.call("fs_list", json!({"path": "."})));
    assert_eq!(shown(&user, &id)["tools_outdated"], true);

    // It lists: it has the latest list, and that is said, so the window's line goes.
    let names = mcp.names();
    assert!(names.contains(&"forge".to_owned()), "{names:?}");
    assert!(!names.contains(&"tickets".to_owned()), "{names:?}");
    assert_eq!(shown(&user, &id)["tools_outdated"], false);
    assert_eq!(said_outdated(&user, &id), [true, false]);
}

#[test]
fn a_session_told_of_a_change_and_of_its_undoing_holds_its_list_and_is_not_behind() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, url, authorization) = agent(&user, json!({}));
    let mcp = Mcp::initialize(&url, &authorization);
    mcp.names();

    track(&user, true);
    ok(mcp.call("fs_list", json!({"path": "."})));
    track(&user, false);
    ok(mcp.call("fs_list", json!({"path": "."})));
    // Told twice, it calls without listing: what it holds is what it is served.
    ok(mcp.call("fs_list", json!({"path": "."})));
    assert_eq!(shown(&user, &id)["tools_outdated"], false);
    assert_eq!(said_outdated(&user, &id), Vec::<bool>::new());
}

#[test]
fn a_codex_session_is_behind_while_its_list_is_not_the_one_it_started_with() {
    let user = User::with_fake_providers();
    user.write_providers("[codex]\nenabled = true\n");
    project(&user);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "hello", "provider": "codex"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("its process runs", || shown(&user, &id)["state"] == "idle");
    assert_eq!(shown(&user, &id)["tools_outdated"], false);

    // Its `enabled_tools`, set when its thread started, leave out the tools it gains.
    track(&user, true);
    assert_eq!(shown(&user, &id)["tools_outdated"], true);
    // Back to the list it started with: it is no longer behind.
    track(&user, false);
    assert_eq!(shown(&user, &id)["tools_outdated"], false);
    assert_eq!(said_outdated(&user, &id), [true, false]);
}

// A new build of Flottille whose tools changed (`docs/tools.md`, "When the tools change").

/// The tool list the agent was last given, as stored with it.
fn given(user: &User, id: &str) -> Value {
    let store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    let given = store.agent_tools_given(id).unwrap().expect("a list given");
    serde_json::from_str(&given).unwrap()
}

/// The daemon stops, as an update stops it; `older` (when given) rewrites what the agent was last
/// given, as an older build gave it.
fn update(user: &User, id: &str, older: Option<&dyn Fn(&mut Value)>) {
    eventually("the first turn ends", || shown(user, id)["state"] == "idle");
    let mut was = given(user, id);
    assert!(user.run(&["daemon", "stop"]).status.success());
    if let Some(older) = older {
        older(&mut was);
        let mut store = Store::open(&user.path("state/flottille/flottille.db")).unwrap();
        store.set_agent_tools_given(id, &was.to_string()).unwrap();
    }
}

/// The agent is sent a message after the update: the MCP server of its new process.
fn next_process(user: &User, id: &str, provider: Option<&str>) -> (String, String) {
    let resumes = || -> Vec<Value> {
        let received = match provider {
            Some(provider) => user.provider_received(provider),
            None => user.received(),
        };
        received
            .into_iter()
            .filter(|entry| entry["received"]["method"] == "session/resume")
            .map(|entry| entry["received"].clone())
            .collect()
    };
    let reopened = resumes().len();
    user.call("agent.send", json!({"agent": id, "message": "go on"}))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let server = loop {
        if let Some(resumed) = resumes().get(reopened) {
            break resumed["params"]["mcpServers"][0].clone();
        }
        assert!(Instant::now() < deadline, "the session was not resumed");
        std::thread::sleep(Duration::from_millis(20));
    };
    let authorization = server["headers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"] == "Authorization")
        .unwrap()["value"]
        .as_str()
        .unwrap()
        .to_owned();
    (server["url"].as_str().unwrap().to_owned(), authorization)
}

#[test]
fn an_agent_started_on_an_older_tool_list_is_told_they_changed() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, _, _) = agent(&user, json!({}));
    // Same names, other descriptions or schemas: only the digest tells.
    update(
        &user,
        &id,
        Some(&|given: &mut Value| given["digest"] = "older".into()),
    );

    let (url, authorization) = next_process(&user, &id, None);
    let client = Noticing::connect(&url, &authorization, ClientLifecycleMode::Initialize);
    ok(client.call("fs_list", json!({"path": "."})));
    assert_eq!(client.told(), 1, "told with the answer to its next call");
    ok(client.call("fs_list", json!({"path": "."})));
    assert_eq!(client.told(), 1, "told once");
    assert_ne!(given(&user, &id)["digest"], "older", "the new list is kept");
    // Only the digest changed, so no action is named and no delivery is sent.
    eventually("the message is sent", || {
        user.received_requests("session/prompt").len() == 2
    });
    let prompts = user.received_requests("session/prompt");
    assert!(
        !prompts[1].to_string().contains("[flottille:tools]"),
        "{}",
        prompts[1]
    );
}

#[test]
fn an_agent_whose_tools_did_not_change_is_not_told() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, _, _) = agent(&user, json!({}));
    update(&user, &id, None);

    let (url, authorization) = next_process(&user, &id, None);
    let client = Noticing::connect(&url, &authorization, ClientLifecycleMode::Initialize);
    ok(client.call("fs_list", json!({"path": "."})));
    assert_eq!(client.told(), 0);
}

#[test]
fn the_delivery_names_the_actions_added() {
    let user = User::with_fake_providers();
    user.write_providers("[codex]\nenabled = true\n");
    project(&user);
    declare(&user, true, false);
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "hello", "provider": "codex"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    // The build before gave `forge` no CI actions, and an action since gone.
    update(
        &user,
        &id,
        Some(&|given: &mut Value| {
            given["digest"] = "older".into();
            let forge = given["actions"]["forge"].as_array_mut().unwrap();
            forge.retain(|action| action != "pipelines" && action != "run");
            forge.push("rerun".into());
        }),
    );

    next_process(&user, &id, Some("codex"));
    let prompts = || -> Vec<String> {
        user.provider_received("codex")
            .iter()
            .filter(|entry| entry["received"]["method"] == "session/prompt")
            .map(|entry| entry["received"]["params"]["prompt"].to_string())
            .collect()
    };
    eventually("the message is sent", || prompts().len() == 2);
    let prompt = &prompts()[1];
    assert!(prompt.contains("go on"), "with what woke it: {prompt}");
    assert!(prompt.contains("[flottille:tools]"), "{prompt}");
    assert!(
        prompt.contains("forge: added pipelines, run; removed rerun"),
        "{prompt}"
    );
    assert!(!prompt.contains("fs_list"), "only what changed: {prompt}");
}

#[test]
fn a_claude_agent_is_told_the_actions_added_though_it_lists_again() {
    // Seen on a real Claude session: it did not list its tools again, and the daemon accepted an
    // action it had not been told of.
    let user = User::with_fake_agent();
    project(&user);
    declare(&user, true, false);
    let (id, _, _) = agent(&user, json!({}));
    update(
        &user,
        &id,
        Some(&|given: &mut Value| {
            given["digest"] = "older".into();
            let forge = given["actions"]["forge"].as_array_mut().unwrap();
            forge.retain(|action| action != "pipelines" && action != "run");
            forge.push("rerun".into());
        }),
    );

    next_process(&user, &id, None);
    eventually("the message is sent", || {
        user.received_requests("session/prompt").len() == 2
    });
    let prompt = user.received_requests("session/prompt")[1].to_string();
    assert!(prompt.contains("[flottille:tools]"), "{prompt}");
    assert!(
        prompt.contains("forge: added pipelines, run; removed rerun"),
        "{prompt}"
    );
}

#[test]
fn a_permission_asked_after_a_change_is_answered_from_the_current_list() {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [{"when": "ask", "do": [
        {"permission": {"tool": "mcp__flottille__tickets", "title": "tickets"}}
    ]}]}));
    project(&user);
    let (id, _, _) = agent(&user, json!({}));
    eventually("the first turn ends", || {
        shown(&user, &id)["state"] == "idle"
    });

    // Its process runs on: the tool it gains is allowed by the answer to its permission request.
    track(&user, true);
    user.call("agent.send", json!({"agent": id, "message": "ask"}))
        .unwrap();
    let answered = || {
        let shown = user.call("agent.show", json!({"agent": id})).unwrap();
        shown["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "agent.permission_answered")
            .map(|e| e["data"].clone())
    };
    eventually("the permission is answered", || answered().is_some());
    let answered = answered().unwrap();
    assert_eq!(answered["tool"], "mcp__flottille__tickets");
    assert_eq!(answered["allowed"], true, "{answered}");
    assert_eq!(
        user.received_requests("session/new").len(),
        1,
        "one process"
    );
}

// A replaced agent's late calls (`docs/design.md` §5.5).

/// Flottille's word for a call of a session that was replaced meanwhile.
const REPLACED: &str = "this session has been replaced";

/// A root agent of `app` working in the project folder, where a write goes to the judge, whose
/// process dies once the test creates `die-now` in the fake agent's folder: its id, the URL and
/// the authorization of its session's MCP server.
fn dying_root(user: &User) -> (String, String, String) {
    project(user);
    // Its session is lost with its process: a new one replaces it.
    user.fake_script(json!({"load_error": "the session is lost", "turns": [
        {"when": "hello", "times": 1, "do": [{"hold": "die-now"}, {"die": 3}]}
    ]}));
    agent(user, json!({}))
}

fn epoch(user: &User, id: &str) -> Value {
    user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["epoch"].clone()
}

#[test]
fn a_call_judged_while_its_agent_is_replaced_is_refused() {
    let user = User::with_fake_agent().with_judge(json!([
        {"hold": "release"},
        {"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}
    ]));
    let (id, url, authorization) = dying_root(&user);
    let folder = user.path("work/app");
    let call = std::thread::spawn(move || {
        Mcp::initialize(&url, &authorization).call(
            "fs_write",
            json!({"path": "notes.txt", "content": "the old session's\n"}),
        )
    });
    eventually("the judgement", || {
        user.judge_received()
            .iter()
            .any(|e| e["received"]["method"] == "session/prompt")
    });

    std::fs::write(user.path("fake/die-now"), "").unwrap();
    eventually("the replacement", || epoch(&user, &id) == 1);
    std::fs::write(user.path("fake-judge/release"), "").unwrap();

    let text = refused(call.join().unwrap());
    assert!(text.contains(REPLACED), "{text}");
    assert!(!folder.join("notes.txt").exists());
}

#[test]
fn a_call_of_the_current_epoch_still_passes() {
    let user = User::with_fake_agent().with_judge(json!([
        {"hold": "release"},
        {"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}
    ]));
    let (id, url, authorization) = dying_root(&user);
    let folder = user.path("work/app");
    std::fs::write(user.path("fake-judge/release"), "").unwrap();

    ok(Mcp::initialize(&url, &authorization).call(
        "fs_write",
        json!({"path": "notes.txt", "content": "the session's\n"}),
    ));

    assert_eq!(epoch(&user, &id), 0);
    assert_eq!(
        std::fs::read_to_string(folder.join("notes.txt")).unwrap(),
        "the session's\n"
    );
}
