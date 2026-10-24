//! The scenarios of the agents' MCP server and its tools, called over HTTP with the token the
//! daemon gave a fake agent's session (`crates/flottille-fake-agent`), with throwaway
//! repositories and temporary XDG folders. No real agent ever runs here.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, post, server_of};
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

/// The agent's session in the project folder, through rmcp's client.
fn session(user: &User) -> (Mcp, PathBuf) {
    let folder = project(user);
    let (_, url, authorization) = agent(user, json!({}));
    (Mcp::initialize(&url, &authorization), folder)
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not an error: {}", answer.text);
    assert!(answer.text.chars().count() < 2_000, "{}", answer.text);
    answer.text
}

/// The id of the run an answer of `commands_run` names.
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
            "commands_list",
            "commands_run",
            "commands_output",
            "commands_stop",
        ]
    );
    let text = refused(mcp.call("agent_launch", json!({"title": "x", "brief": "y"})));
    assert!(text.contains("has no tool agent_launch"), "{text}");
    let text = refused(mcp.call("shell", json!({})));
    assert!(text.contains("has no tool shell"), "{text}");

    // Another agent's session is its own: what the first read and ran is not the second's.
    ok(mcp.call("fs_read", json!({"path": "README.md"})));
    let run = run_id(&ok(mcp.call("commands_run", json!({"line": "true"}))));
    let (_, url, authorization) = agent(&user, json!({}));
    let other = Mcp::initialize(&url, &authorization);
    let text = refused(other.call("fs_write", json!({"path": "README.md", "content": "x"})));
    assert!(text.contains("has not read it"), "{text}");
    let text = refused(other.call("commands_output", json!({"run": run})));
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
        assert!(
            (40..1_000).contains(&description.chars().count()),
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
    assert_eq!(hints("commands_stop"), (false, true));
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
    assert_eq!(
        body["result"]["capabilities"]["tools"]["listChanged"],
        false
    );
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
    assert_eq!(mcp.names().len(), 9);
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
fn dot_dot_notes_is_inside_and_dot_dot_slash_x_is_outside() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::write(folder.join("..notes"), "inside\n").unwrap();
    std::fs::write(folder.parent().unwrap().join("x"), "outside\n").unwrap();
    std::os::unix::fs::symlink(folder.parent().unwrap().join("x"), folder.join("link")).unwrap();

    assert!(ok(mcp.call("fs_read", json!({"path": "..notes"}))).contains("inside"));
    for path in [
        "../x",
        "src/../../x",
        folder.parent().unwrap().join("x").to_str().unwrap(),
        "link",
    ] {
        let text = refused(mcp.call("fs_read", json!({"path": path})));
        assert!(text.contains("outside"), "{path}: {text}");
        assert!(!text.contains("outside\n"), "never read: {text}");
    }
    let text = refused(mcp.call("fs_write", json!({"path": "../y", "content": "y"})));
    assert!(text.contains("outside"), "{text}");
    assert!(!folder.parent().unwrap().join("y").exists());
    let text = refused(mcp.call("fs_read", json!({"path": "README.md", "repository": "web"})));
    assert_eq!(text, "refused: the project has no repository web: it has .");
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
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);

    for line in ["seq 1 500000", "seq -s , 1 400000"] {
        let answer = ok(mcp.call("commands_run", json!({"line": line})));
        assert!(
            answer.chars().count() < RESULT_LIMIT,
            "{line}: {}",
            answer.len()
        );
        assert!(answer.contains("exited with code 0"), "{line}: {answer}");
        let run = run_id(&answer);
        let output = ok(mcp.call("commands_output", json!({"run": run, "tail": 1000})));
        assert!(
            output.chars().count() < RESULT_LIMIT,
            "{line}: {}",
            output.len()
        );
        assert!(
            output.contains("earlier lines: commands_output"),
            "{line}: {}",
            &output[output.len().saturating_sub(400)..]
        );
    }
    let answer = ok(mcp.call("commands_run", json!({"line": "seq 1 500000"})));
    let output = ok(mcp.call("commands_output", json!({"run": run_id(&answer)})));
    assert!(output.contains("500000"), "the tail is the end: {output}");
}

// Commands.

#[test]
fn shell_syntax_is_refused_with_the_sentence() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);

    for (line, token) in [
        ("echo a | cat", "|"),
        ("true && false", "&&"),
        ("echo $(x)", "$("),
        ("echo a > f", ">"),
        ("ls *", "*"),
    ] {
        let text = refused(mcp.call("commands_run", json!({"line": line})));
        assert_eq!(
            text,
            format!(
                "Flottille runs a command without a shell, and “{token}” is shell syntax: put it \
                 in a script of the repository and run the script."
            ),
            "{line}"
        );
    }
    assert!(!folder.join("f").exists());
    let text = refused(mcp.call("commands_run", json!({"command": "nope"})));
    assert_eq!(
        text,
        "refused: the catalogue has no command nope: list them with commands_list"
    );
    let text = refused(mcp.call("commands_run", json!({"command": "nope", "line": "ls"})));
    assert!(text.contains("exactly one"), "{text}");
}

#[test]
fn a_long_command_answers_after_its_timeout_with_it_keeps_going_and_stop_ends_its_tree() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);

    let started = Instant::now();
    let answer = ok(mcp.call(
        "commands_run",
        json!({"line": "sh -c 'sleep 300 & echo $! > child.pid; echo started; wait'", "timeout": 1}),
    ));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(answer.contains("is running"), "{answer}");
    assert!(
        answer.contains("it keeps going: read it with commands_output, stop it with commands_stop"),
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

    let output = ok(mcp.call("commands_output", json!({"run": run})));
    assert!(output.contains("is running"), "{output}");
    assert!(output.contains("started"), "{output}");

    let stopped = ok(mcp.call("commands_stop", json!({"run": run})));
    assert!(stopped.contains("was stopped"), "{stopped}");
    eventually("the child's end", || !alive(child));
    let output = ok(mcp.call("commands_output", json!({"run": run})));
    assert!(output.contains("was stopped"), "{output}");

    let text = refused(mcp.call("commands_output", json!({"run": "000000000000"})));
    assert_eq!(
        text,
        "refused: 000000000000 is not a run this agent started"
    );
}

#[test]
fn a_retry_with_the_same_call_key_does_not_run_twice() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    let line = json!({"line": "sh -c 'echo once >> count.txt; echo done'"});

    let first = ok(mcp.call_keyed("commands_run", line.clone(), "toolu_01"));
    let again = ok(mcp.call_keyed("commands_run", line.clone(), "toolu_01"));
    assert_eq!(first, again);
    assert_eq!(
        std::fs::read_to_string(folder.join("count.txt")).unwrap(),
        "once\n"
    );
    ok(mcp.call_keyed("commands_run", line, "toolu_02"));
    assert_eq!(
        std::fs::read_to_string(folder.join("count.txt")).unwrap(),
        "once\nonce\n"
    );
}

#[test]
fn git_push_from_a_run_fails() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    std::fs::write(folder.join("more.txt"), "more\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "more"]);
    let remote = user.path("remotes/app.git");
    let before = user.git_output(&remote, &["rev-parse", "main"]);

    let answer = ok(mcp.call("commands_run", json!({"line": "git push origin main"})));

    assert!(!answer.contains("exited with code 0"), "{answer}");
    assert!(answer.contains("exited with code"), "{answer}");
    assert_eq!(user.git_output(&remote, &["rev-parse", "main"]), before);
}

#[test]
fn gh_token_is_empty_in_a_run() {
    let user = User::with_fake_agent()
        .with_env("GH_TOKEN", "ghp_fakefakefakefakefakefakefake0001")
        .with_env("GITLAB_TOKEN", "glpat-fakefakefakefakefake");
    let (mcp, _) = session(&user);

    let answer = ok(mcp.call("commands_run", json!({"line": "env"})));
    let output = ok(mcp.call(
        "commands_output",
        json!({"run": run_id(&answer), "tail": 1000}),
    ));
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
    assert_eq!(value("GIT_CONFIG_COUNT").as_deref(), Some("1"));
    assert!(
        value("GIT_CONFIG_KEY_0")
            .unwrap()
            .ends_with(".pushInsteadOf"),
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

    let listed = ok(mcp.call("commands_list", json!({})));
    assert!(listed.contains("where: printenv STORE STACK"), "{listed}");
    let answer = ok(mcp.call("commands_run", json!({"command": "where"})));
    assert!(answer.contains("exited with code 0"), "{answer}");
    assert!(
        answer.contains(&format!("{}/.store\napp-login", workspace.display())),
        "{answer}"
    );
    // The place is the workspace: the main checkout is outside it.
    let text = refused(mcp.call(
        "fs_read",
        json!({"path": folder.join("README.md").to_str().unwrap()}),
    ));
    assert!(text.contains("outside"), "{text}");
}

// Records.

#[test]
fn every_call_is_recorded_with_its_arguments_masked() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, url, authorization) = agent(&user, json!({}));
    let mcp = Mcp::initialize(&url, &authorization);
    ok(mcp.call("fs_read", json!({"path": "README.md"})));
    refused(mcp.call(
        "commands_run",
        json!({"line": "echo ghp_fakefakefakefakefakefakefake0002 | cat"}),
    ));
    let answer = ok(mcp.call("commands_run", json!({"line": "true"})));

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
