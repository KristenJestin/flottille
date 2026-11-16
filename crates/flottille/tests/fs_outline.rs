//! The scenarios of the outline `fs_read` gives (`docs/tools.md`, "Files"): the declarations of a
//! source file, each with its line and byte offset, which is not a read, which is refused as a
//! read is, and which is cut as any result. The parsing of each language is the unit tests of
//! `outline.rs`. The agent is the fake ACP agent, in a throwaway repository and temporary XDG
//! folders.

mod common;

use std::path::PathBuf;

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::json;

const API: &str = "//! Connections.

/// A connection.
pub struct Conn {
    host: String,
}

impl Conn {
    /// Opens it.
    pub fn open(host: &str) -> Conn {
        Conn { host: host.to_owned() }
    }
}

pub fn close(conn: Conn) {}
";

/// The line of `pub fn close` in `API`.
const CLOSE_LINE: usize = 15;

/// A project `app` over a throwaway repository holding `src/api.rs` and the files the scenarios
/// ask an outline of, and the session of an agent working in its workspace `w` (the project's own
/// folder is read-only for agents, so a write would be held for approval): the workspace's folder.
fn session(user: &User) -> (Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::create_dir_all(folder.join("src")).unwrap();
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join("src/api.rs"), API).unwrap();
    std::fs::write(folder.join("notes.md"), "# notes\n").unwrap();
    std::fs::write(folder.join("Makefile"), "all:\n").unwrap();
    let many: String = (1..=400)
        .map(|n| {
            format!(
                "pub fn declaration_number_{n:04}(first: u32, second: u32) -> u32 {{ first }}\n"
            )
        })
        .collect();
    std::fs::write(folder.join("src/many.rs"), many).unwrap();
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
    let workspace = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "workspace": "w", "message": "look at the code"}),
    )
    .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        Mcp::initialize(&url, &authorization),
        PathBuf::from(workspace["folder"].as_str().unwrap()),
    )
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    answer.text
}

fn outline(mcp: &Mcp, path: &str) -> Answer {
    mcp.call("fs_read", json!({"path": path, "outline": true}))
}

#[test]
fn an_outline_gives_each_declaration_with_its_line_and_offset_and_says_it_is_not_a_read() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);

    let text = ok(outline(&mcp, "src/api.rs"));

    let (head, lines) = text.split_once('\n').unwrap();
    assert!(head.starts_with("outline of src/api.rs (Rust, "), "{head}");
    assert!(head.contains("Not a read"), "{head}");
    let at = |needle: &str| {
        format!(
            "{}:{}",
            API[..API.find(needle).unwrap()].matches('\n').count() + 1,
            API.find(needle).unwrap()
        )
    };
    assert_eq!(
        lines,
        format!(
            "{}  pub struct Conn\n{}  impl Conn\n{}    pub fn open(host: &str) -> Conn\n{}  pub fn close(conn: Conn)\n",
            at("pub struct"),
            at("impl Conn"),
            at("pub fn open"),
            at("pub fn close"),
        )
    );
    assert!(!text.contains("A connection."), "{text}");
}

#[test]
fn the_offset_of_a_declaration_is_where_fs_read_goes_on_from() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);
    let text = ok(outline(&mcp, "src/api.rs"));
    let line = text
        .lines()
        .find(|l| l.ends_with("pub fn close(conn: Conn)"))
        .unwrap();
    let offset: u64 = line
        .split(':')
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();

    let page = ok(mcp.call(
        "fs_read",
        json!({"path": "src/api.rs", "range": {"offset": offset}}),
    ));

    assert!(
        page.starts_with(&format!("{CLOSE_LINE:6}\tpub fn close(conn: Conn) {{}}\n")),
        "{page}"
    );
}

#[test]
fn an_outline_is_not_a_read_for_edit() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    ok(outline(&mcp, "src/api.rs"));

    let edit =
        json!({"path": "src/api.rs", "edits": [{"old": "pub fn close", "new": "pub fn shut"}]});
    let text = refused(mcp.call("fs_edit", edit.clone()));
    assert_eq!(
        text,
        "refused: src/api.rs exists and this session has not read it: read it with fs_read first"
    );
    let text = refused(mcp.call(
        "fs_write",
        json!({"path": "src/api.rs", "content": "replaced\n"}),
    ));
    assert!(text.contains("has not read it"), "{text}");
    assert_eq!(
        std::fs::read_to_string(folder.join("src/api.rs")).unwrap(),
        API
    );

    // A page read is what lets the session change the file, whatever the outline said.
    ok(mcp.call("fs_read", json!({"path": "src/api.rs"})));
    ok(mcp.call("fs_edit", edit));
    assert!(
        std::fs::read_to_string(folder.join("src/api.rs"))
            .unwrap()
            .contains("pub fn shut")
    );
}

#[test]
fn an_unknown_language_is_refused_naming_fs_read() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);

    assert_eq!(
        refused(outline(&mcp, "notes.md")),
        "refused: no outline for md files; read it with fs_read"
    );
    assert_eq!(
        refused(outline(&mcp, "Makefile")),
        "refused: no outline for files without an extension; read it with fs_read"
    );
}

#[test]
fn an_outline_is_refused_as_a_read_is() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    let outside = user.path("outside.rs");
    std::fs::write(&outside, "pub fn secret() {}\n").unwrap();
    std::os::unix::fs::symlink(&outside, folder.join("link.rs")).unwrap();
    let pipe = std::ffi::CString::new(folder.join("pipe.rs").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o644) }, 0);

    // A login's place of the user (the temporary HOME's), which no call reaches.
    let keys = user.path("home/.ssh");
    std::fs::create_dir_all(&keys).unwrap();
    std::fs::write(keys.join("x.rs"), "pub fn secret() {}\n").unwrap();
    let login = keys.join("x.rs").display().to_string();

    for path in [
        "../outside.rs",
        "link.rs",
        "pipe.rs",
        "src",
        "missing.rs",
        login.as_str(),
    ] {
        let read = mcp.call("fs_read", json!({"path": path}));
        let listed = outline(&mcp, path);
        assert_eq!(
            listed.error, read.error,
            "{path}: {} / {}",
            listed.text, read.text
        );
        assert_eq!(held(&listed.text), held(&read.text), "{path}");
        if !held(&read.text) {
            assert_eq!(listed.text, read.text, "{path}");
        }
        assert!(!listed.text.contains("secret"), "{path}: {}", listed.text);
    }
}

/// Whether a call was held for the user's approval, which a path outside the place is, for a
/// read and an outline alike.
fn held(text: &str) -> bool {
    text.starts_with("Waiting for the user's approval, request #")
}

#[test]
fn an_outline_never_shows_part_of_a_secret() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    // Each secret starts a few characters before the 160th of its signature, so that cutting
    // the signature there would show a part of it that no mask knows.
    let padding = |n: usize| "a".repeat(n);
    let source = format!(
        "pub fn url(first: &str, {}, link: &str = \"https://deploy:Hunter2Hunter2Hunter2@forge.example/x.git\") {{}}\n\
         pub fn token(first: &str, {}, token: &str = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\") {{}}\n",
        padding(110),
        padding(108),
    );
    std::fs::write(folder.join("src/secrets.rs"), source).unwrap();

    let text = ok(outline(&mcp, "src/secrets.rs"));

    for part in ["Hunter2", "ghp_", "abcdefghij", "deploy:"] {
        assert!(!text.contains(part), "{part:?} in {text}");
    }
    assert!(text.contains("pub fn url("), "{text}");
    assert!(text.contains("pub fn token("), "{text}");
}

#[test]
fn a_pathological_file_is_answered_in_bounded_time() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    // The most an outline takes, of nested brackets that cost a parser dearly. How long it takes
    // depends on the machine: either way it is answered at once, and when it was stopped the
    // answer says so and names fs_read (the stop itself is a unit test of `outline.rs`).
    std::fs::write(folder.join("src/nested.js"), "a=[".repeat(512 * 1024 / 3)).unwrap();

    let started = std::time::Instant::now();
    let answer = outline(&mcp, "src/nested.js");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
    if answer.error {
        assert_eq!(
            answer.text,
            "refused: src/nested.js took too long for an outline; read it with fs_read"
        );
    }
}

#[test]
fn a_file_over_the_limit_is_refused_naming_it_and_fs_read() {
    let user = User::with_fake_agent();
    let (mcp, folder) = session(&user);
    let big = "pub fn f() {}\n".repeat(40_000);
    assert!(big.len() > 512 * 1024);
    std::fs::write(folder.join("src/big.rs"), &big).unwrap();

    let text = refused(outline(&mcp, "src/big.rs"));

    assert!(text.contains("512 KiB at most"), "{text}");
    assert!(text.contains("fs_read"), "{text}");
}

#[test]
fn an_outline_is_refused_with_again_as_with_a_range() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);

    let text = refused(mcp.call(
        "fs_read",
        json!({"path": "src/api.rs", "outline": true, "again": true}),
    ));

    assert!(
        text.starts_with("refused: an outline is not a page: give no again"),
        "{text}"
    );
}

#[test]
fn an_outline_takes_no_range() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);

    let text = refused(mcp.call(
        "fs_read",
        json!({"path": "src/api.rs", "outline": true, "range": {"offset": 4}}),
    ));

    assert!(
        text.starts_with("refused: an outline lists the whole file"),
        "{text}"
    );
}

#[test]
fn a_long_outline_is_cut_as_any_result() {
    let user = User::with_fake_agent().with_cut_results();
    let (mcp, _) = session(&user);

    let text = ok(outline(&mcp, "src/many.rs"));

    assert!(
        text.chars().count() < 8_000,
        "{} characters",
        text.chars().count()
    );
    assert!(text.starts_with("outline of src/many.rs"), "{text}");
    assert!(text.contains("declaration_number_0001"), "{text}");
    assert!(text.contains("declaration_number_0400"), "{text}");
    assert!(!text.contains("declaration_number_0200"), "{text}");
    let at = text
        .find("run_status {\"run\": \"")
        .expect("no way to read it whole");
    let id = text[at + "run_status {\"run\": \"".len()..]
        .split('"')
        .next()
        .unwrap();
    // The whole outline is read by ranges, as any cut result.
    let whole = ok(mcp.call("run_status", json!({"run": id, "start": 1})));
    assert!(whole.contains("declaration_number_0"), "{whole}");
}

#[test]
fn the_switch_is_on_until_the_user_turns_it_off_and_a_refused_outline_names_fs_read() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);
    let shown = user.call("tool_results.show", json!({})).unwrap();
    assert_eq!(shown["outline"], true, "{shown}");
    ok(outline(&mcp, "src/api.rs"));

    let shown = user
        .call("tool_results.set", json!({"outline": false}))
        .unwrap();
    assert_eq!(shown["outline"], false, "{shown}");
    let text = refused(outline(&mcp, "src/api.rs"));
    assert_eq!(
        text,
        "refused: outlines are turned off by the user; read it with fs_read"
    );
    // A page is read as ever.
    ok(mcp.call("fs_read", json!({"path": "src/api.rs"})));

    // The choice is kept with the others and read back.
    let shown = user.call("tool_results.show", json!({})).unwrap();
    assert_eq!(shown["outline"], false, "{shown}");
    let shown = user
        .call(
            "tool_results.set",
            json!({"outline": true, "unchanged": true}),
        )
        .unwrap();
    assert_eq!(shown["outline"], true, "{shown}");
    assert_eq!(shown["unchanged"], true, "{shown}");
    ok(outline(&mcp, "src/api.rs"));
}

#[test]
fn fs_read_says_outline_in_its_description() {
    let user = User::with_fake_agent();
    let (mcp, _) = session(&user);

    let tools = mcp.tools();
    let read = tools.iter().find(|tool| tool["name"] == "fs_read").unwrap();

    assert!(read["description"].as_str().unwrap().contains("outline"));
    assert_eq!(
        read["inputSchema"]["properties"]["outline"]["type"],
        "boolean"
    );
}
