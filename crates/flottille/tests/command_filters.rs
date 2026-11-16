//! The scenarios of filtering the output of the commands an agent runs (`docs/tools.md`,
//! "Filtered command output"): what a terminal hides is not read by the agent, a result that lost
//! lines says how to read them, `run_status` with `raw` gives them all, and no other tool is
//! filtered. The program is the fake `cargo`, which prints recorded outputs; the agent is the
//! fake ACP agent, in a throwaway repository and temporary XDG folders.

mod common;

use std::path::PathBuf;

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::json;

/// A project `app` over a throwaway repository holding `noise.txt`, a file that looks like the
/// output of a tool, and the session of a root agent working in its folder.
fn session(user: &User) -> (Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(
        folder.join("noise.txt"),
        "   Compiling app v0.3.1\n   Compiling app v0.3.1\n   Compiling app v0.3.1\n\
         \u{1b}[32mgreen\u{1b}[0m\ntest one ... ok\n",
    )
    .unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let started = user
        .call(
            "agent.run",
            json!({"project": "app", "message": "run the tests"}),
        )
        .unwrap();
    // Full, so that the gate holds no `cargo` for the user's word: the scenarios are about what
    // the agent reads of it.
    let id = started["agent"]["id"].as_str().unwrap();
    assert!(user.run(&["agent", "mode", id, "full"]).status.success());
    let (url, authorization) = server_of(user, opened);
    (Mcp::initialize(&url, &authorization), folder)
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

fn user() -> User {
    User::with_fake_agent().with_fake_cargo()
}

/// The id of the run an answer is about.
fn run_of(text: &str) -> String {
    text.strip_prefix("run ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("no run in {text}"))
        .to_owned()
}

#[test]
fn a_cargo_test_run_keeps_its_failures_and_summary() {
    let user = user();
    let (mcp, _) = session(&user);

    let text = ok(mcp.call("run", json!({"line": "cargo test"})));

    for kept in [
        "test total::adds_two_numbers ... FAILED",
        "thread 'total::adds_two_numbers' panicked at src/total.rs:18:9:",
        "left: 4",
        "test result: FAILED. 4 passed; 1 failed",
        "error: test failed, to rerun pass `-p app --lib`",
    ] {
        assert!(text.contains(kept), "{kept} is missing from {text}");
    }
    for gone in [
        "Compiling",
        "test parse::reads_a_number ... ok",
        "test total::adds_nothing ... ok",
    ] {
        assert!(!text.contains(gone), "{gone} is in {text}");
    }
}

#[test]
fn a_filtered_result_says_how_to_read_it_whole() {
    let user = user();
    let (mcp, _) = session(&user);

    let text = ok(mcp.call("run", json!({"line": "cargo test"})));

    let id = run_of(&text);
    let last = text.lines().last().unwrap();
    assert_eq!(
        last,
        format!(
            "[7 lines filtered out: read the whole output with run_status {{\"run\": \"{id}\", \
             \"start\": 1, \"raw\": true}}]"
        )
    );
    // Read back later, the filtered view says the same.
    let again = ok(mcp.call("run_status", json!({"run": id})));
    assert!(
        again.lines().last().unwrap().contains("\"raw\": true"),
        "{again}"
    );
}

#[test]
fn a_result_that_lost_nothing_has_no_such_line() {
    let user = user();
    let (mcp, _) = session(&user);

    let text = ok(mcp.call("run", json!({"line": "cargo publish"})));

    assert!(text.contains("error: no such subcommand"), "{text}");
    assert!(!text.contains("lines filtered out"), "{text}");
}

#[test]
fn run_status_raw_reads_the_unfiltered_lines() {
    let user = user();
    let (mcp, _) = session(&user);
    let id = run_of(&ok(mcp.call("run", json!({"line": "cargo test"}))));

    let raw = ok(mcp.call("run_status", json!({"run": id, "start": 1, "raw": true})));

    assert!(raw.contains("Compiling serde v1.0.219"), "{raw}");
    assert!(raw.contains("test parse::reads_a_number ... ok"), "{raw}");
    assert!(
        raw.contains("test total::adds_two_numbers ... FAILED"),
        "{raw}"
    );
    assert!(!raw.contains("lines filtered out"), "{raw}");
    // `raw` goes with reading a run's output, nothing else.
    let stop = mcp.call(
        "run_status",
        json!({"run": id, "action": "stop", "raw": true}),
    );
    assert!(stop.text.starts_with("refused:"), "{}", stop.text);
    let listing = mcp.call("run_status", json!({"raw": true}));
    assert!(listing.text.starts_with("refused:"), "{}", listing.text);
}

#[test]
fn fs_read_and_search_are_never_filtered() {
    let user = user();
    let (mcp, _) = session(&user);

    let file = ok(mcp.call("fs_read", json!({"path": "noise.txt"})));
    let found = ok(mcp.call("search", json!({"pattern": "Compiling"})));

    assert_eq!(file.matches("Compiling app v0.3.1").count(), 3, "{file}");
    assert!(file.contains("\u{1b}[32mgreen"), "{file}");
    assert!(file.contains("test one ... ok"), "{file}");
    assert_eq!(found.matches("Compiling app v0.3.1").count(), 3, "{found}");
}

#[test]
fn filters_off_returns_the_output_as_it_was() {
    let user = user();
    let (mcp, _) = session(&user);
    user.call("tool_results.set", json!({"filters": false}))
        .unwrap();

    let text = ok(mcp.call("run", json!({"line": "cargo test"})));

    assert!(text.contains("Compiling serde v1.0.219"), "{text}");
    assert!(text.contains("test parse::reads_a_number ... ok"), "{text}");
    assert!(!text.contains("lines filtered out"), "{text}");
}

#[test]
fn a_build_that_works_is_said_in_a_few_words() {
    let user = user();
    let (mcp, _) = session(&user);

    let text = ok(mcp.call("run", json!({"line": "cargo build"})));

    assert!(
        text.contains("\ncargo: ok\n[5 lines filtered out"),
        "{text}"
    );
}

#[test]
fn the_filters_follow_the_switch_while_a_session_runs() {
    let user = user();
    let (mcp, _) = session(&user);
    let id = run_of(&ok(mcp.call("run", json!({"line": "cargo test"}))));

    user.call("tool_results.set", json!({"filters": false}))
        .unwrap();
    let off = ok(mcp.call("run_status", json!({"run": id})));
    user.call("tool_results.set", json!({"filters": true}))
        .unwrap();
    let on = ok(mcp.call("run_status", json!({"run": id})));

    assert!(off.contains("Compiling"), "{off}");
    assert!(!on.contains("Compiling"), "{on}");
}
