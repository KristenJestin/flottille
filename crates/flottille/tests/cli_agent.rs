//! The command line's side of driving agents from outside (`docs/cli.md`): the scenario of the
//! pilot door (`tests/pilot.rs`) through `flottille agent start|send|show|read|stop|list|tree` and
//! `flottille inbox`, all with `--json`. Against the real daemon with the fake ACP agent.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "LEAD-LAUNCHES", "do": [
            {"mcp": {"tool": "agent_launch", "arguments": {"title": "worker one",
                "brief": "HANG while the pilot looks", "workspace": "w"}, "keep": "w1"}},
            {"say": "launched the worker"}]},
        {"when": "HANG", "do": [{"hang": true}]},
        {"when": "ASK-USER", "do": [
            {"mcp": {"tool": "ask_user", "arguments": {"question": "which database?"}}}]},
        {"when": "REPORT-NOW", "do": [
            {"mcp": {"tool": "report", "arguments": {"text": "all done, tests pass"}}}]},
        {"when": "die-always", "do": [{"die": 4}]},
        {"when": "GREET", "do": [{"say": "hello from the fake"}]},
        {"when": "RECALL", "do": [{"recall": true}]},
        {"when": "ASK-FILE", "do": [
            {"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}},
            {"say": "waiting for the user's answer"}]}
    ]}));
    user
}

/// A project `app` over a throwaway repository, and its ready workspace `w`.
fn project(user: &User) {
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
    user.call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A command that succeeded: its standard output as JSON.
fn json_of(output: Output) -> Value {
    assert!(
        output.status.success(),
        "failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// A command that failed: its standard error.
fn failure_of(output: Output) -> String {
    assert!(!output.status.success(), "it succeeded");
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn the_cli_starts_a_root_agent_sends_it_a_message_reads_it_sees_its_child_reads_and_stops_it() {
    let user = user();
    project(&user);

    let started = json_of(user.run(&[
        "agent",
        "start",
        "app",
        "LEAD-LAUNCHES and wait",
        "--workspace",
        "w",
        "--json",
    ]));
    let root = started["agent"]["id"].as_str().unwrap().to_owned();
    assert!(started["after"].is_u64());
    assert_eq!(started["agent"]["role"], "lead");

    let mut child = String::new();
    eventually("the child", || {
        let tree = json_of(user.run(&["agent", "tree", &root, "--json"]));
        let agents = tree["agents"].as_array().unwrap();
        if let Some(found) = agents.iter().find(|a| a["agent"]["title"] == "worker one") {
            child = found["agent"]["id"].as_str().unwrap().to_owned();
            assert_eq!(found["agent"]["parent"], root.as_str());
            return true;
        }
        false
    });
    let tree = json_of(user.run(&["agent", "tree", &root, "--json"]));
    assert_eq!(
        tree["agents"][0]["agent"]["id"],
        root.as_str(),
        "the root comes first"
    );

    let listed = json_of(user.run(&["agent", "list", "--project", "app", "--json"]));
    assert_eq!(listed["agents"].as_array().unwrap().len(), 2);
    let listed = json_of(user.run(&["agent", "list", "--json"]));
    assert_eq!(listed["agents"].as_array().unwrap().len(), 2);

    let sent = json_of(user.run(&[
        "agent",
        "send",
        &root,
        "a second message from the user",
        "--json",
    ]));
    assert_eq!(sent["agent"]["id"], root.as_str());
    assert!(sent["after"].is_u64());

    let mut read = Value::Null;
    eventually("the second turn in the conversation", || {
        read = json_of(user.run(&["agent", "read", &root, "--json"]));
        read["lines"]
            .to_string()
            .contains("a second message from the user")
    });
    assert!(read["lines"].to_string().contains("launched the worker"));
    assert_eq!(read["more"], false);
    assert!(read["after"].as_u64().unwrap() > 0);
    let text = user.run(&["agent", "read", &root]);
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("said: launched the worker"), "{text}");

    let worker = json_of(user.run(&["agent", "read", &child, "--json"]));
    assert!(
        worker["lines"]
            .to_string()
            .contains("HANG while the pilot looks")
    );
    let shown = json_of(user.run(&["agent", "show", &child, "--json"]));
    assert_eq!(shown["agent"]["parent"], root.as_str());

    // A child belongs to its parent.
    let refused = failure_of(user.run(&["agent", "send", &child, "do it differently"]));
    assert!(
        refused.contains("only its parent") && refused.contains(&root),
        "{refused}"
    );
    let refused = failure_of(user.run(&["agent", "stop", &child]));
    assert!(
        refused.contains("only its parent") && refused.contains(&root),
        "{refused}"
    );

    let stopped = json_of(user.run(&["agent", "stop", &root, "--json"]));
    assert_eq!(stopped["agent"]["state"], "ended");
    let tree = json_of(user.run(&["agent", "tree", &root, "--json"]));
    for entry in tree["agents"].as_array().unwrap() {
        assert_eq!(entry["agent"]["state"], "ended");
    }
}

#[test]
fn agent_run_still_starts_an_agent_and_prints_its_reply() {
    let user = user();
    project(&user);
    let output = user.run(&["agent", "run", "app", "hello"]);
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("ok"));
}

#[test]
fn the_inbox_lists_a_root_s_report_its_question_and_its_failure_and_nothing_of_a_child() {
    let user = user();
    project(&user);
    let start = |message: &str| {
        let started = json_of(user.run(&["agent", "start", "app", message, "--json"]));
        started["agent"]["id"].as_str().unwrap().to_owned()
    };
    let reporter = start("REPORT-NOW");
    let asker = start("ASK-USER");
    let doomed = start("die-always");
    let lead = json_of(user.run(&[
        "agent",
        "start",
        "app",
        "LEAD-LAUNCHES",
        "--workspace",
        "w",
        "--json",
    ]))["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut entries = Vec::new();
    eventually("the three entries", || {
        let inbox = json_of(user.run(&["inbox", "--json"]));
        entries = inbox["entries"].as_array().unwrap().clone();
        entries.len() >= 3
    });
    let of = |agent: &str| entries.iter().find(|e| e["agent"] == agent).unwrap();
    assert_eq!(of(&reporter)["kind"], "report");
    assert_eq!(of(&reporter)["text"], "all done, tests pass");
    assert_eq!(of(&asker)["kind"], "question");
    assert_eq!(of(&asker)["text"], "which database?");
    assert_eq!(of(&doomed)["kind"], "failure");
    assert!(
        of(&doomed)["text"]
            .as_str()
            .unwrap()
            .contains("replacements"),
        "{entries:?}"
    );
    for entry in &entries {
        assert!(entry["seq"].is_u64() && entry["at"].is_u64());
        assert_eq!(entry["project"], "app");
        assert_ne!(entry["agent"], lead.as_str());
    }
    // A worker's own report goes to its lead, never to the user: the lead's worker hangs, and
    // nothing of it is in the inbox.
    assert_eq!(entries.len(), 3, "{entries:?}");

    // `--after` reads on from the last number seen.
    let last = entries.last().unwrap()["seq"].as_u64().unwrap();
    let inbox = json_of(user.run(&["inbox", "--json", "--after", &last.to_string()]));
    assert!(inbox["entries"].as_array().unwrap().is_empty());

    let text = user.run(&["inbox"]);
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        text.contains("all done, tests pass") && text.contains("which database?"),
        "{text}"
    );
    assert!(text.contains("failure of agent"), "{text}");
}

#[test]
fn an_empty_inbox_says_so() {
    let user = user();
    let text = user.run(&["inbox"]);
    assert!(text.status.success());
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "Nothing came for you.\n"
    );
    let inbox = json_of(user.run(&["inbox", "--json"]));
    assert_eq!(inbox, json!({"entries": []}));
}

/// A command of the user that runs while the test talks to it: its standard input, and its
/// standard output and error read line by line.
struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<(bool, String)>,
    out: String,
    err: String,
}

impl Running {
    fn start(user: &User, args: &[&str]) -> Running {
        let mut child = user
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (sender, lines) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        for (is_error, pipe) in [
            (false, Box::new(stdout) as Box<dyn std::io::Read + Send>),
            (true, Box::new(stderr)),
        ] {
            let sender = sender.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines() {
                    let Ok(line) = line else { break };
                    if sender.send((is_error, line)).is_err() {
                        break;
                    }
                }
            });
        }
        let stdin = child.stdin.take();
        Running {
            child,
            stdin,
            lines,
            out: String::new(),
            err: String::new(),
        }
    }

    fn type_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("standard input is closed");
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    /// Ctrl+D.
    fn close_input(&mut self) {
        self.stdin = None;
    }

    /// Ctrl+C: SIGINT to this process only.
    fn interrupt(&self) {
        // SAFETY: kill(2) on the pid of a child this test started and has not waited for.
        let sent = unsafe { libc::kill(self.child.id() as i32, libc::SIGINT) };
        assert_eq!(sent, 0);
    }

    fn take(&mut self, within: Duration) -> Option<()> {
        match self.lines.recv_timeout(within) {
            Ok((true, line)) => self.err.push_str(&format!("{line}\n")),
            Ok((false, line)) => self.out.push_str(&format!("{line}\n")),
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
        Some(())
    }

    /// Reads until the standard output (or the error) holds `needle`.
    fn wait_for(&mut self, needle: &str, on_error: bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let seen = if on_error { &self.err } else { &self.out };
            if seen.contains(needle) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                self.take(left).is_some(),
                "never saw {needle:?}; stdout {:?}, stderr {:?}",
                self.out,
                self.err
            );
        }
    }

    /// Waits for the command to end: how, what it printed, what it said.
    fn finish(mut self) -> (ExitStatus, String, String) {
        self.stdin = None;
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!(
                    "it never ended; stdout {:?}, stderr {:?}",
                    self.out, self.err
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        while self.take(Duration::from_millis(200)).is_some() {}
        (status, self.out, self.err)
    }
}

fn agent_state(user: &User, agent: &str) -> String {
    let shown = json_of(user.run(&["agent", "show", agent, "--json"]));
    shown["agent"]["state"].as_str().unwrap().to_owned()
}

/// A root agent of project `app`, its first turn over.
fn idle_agent(user: &User) -> String {
    project(user);
    let started = json_of(user.run(&["agent", "start", "app", "hello", "--json"]));
    let agent = started["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn to end", || {
        agent_state(user, &agent) == "idle"
    });
    agent
}

/// Whether the fake agent was given a prompt holding this text.
fn prompted_with(user: &User, text: &str) -> bool {
    user.received_requests("session/prompt")
        .iter()
        .any(|request| request.to_string().contains(text))
}

/// A child of a root that launched one, and its root.
fn child_and_root(user: &User) -> (String, String) {
    project(user);
    let root = json_of(user.run(&[
        "agent",
        "start",
        "app",
        "LEAD-LAUNCHES and wait",
        "--workspace",
        "w",
        "--json",
    ]))["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut child = String::new();
    eventually("the child", || {
        let tree = json_of(user.run(&["agent", "tree", &root, "--json"]));
        match tree["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["agent"]["parent"] == root.as_str())
        {
            Some(found) => {
                child = found["agent"]["id"].as_str().unwrap().to_owned();
                true
            }
            None => false,
        }
    });
    (child, root)
}

const REPLY: &str = "hello from the fake\n";

#[test]
fn send_follow_prints_the_reply_and_exits_when_the_turn_ends() {
    let user = user();
    let agent = idle_agent(&user);

    let output = user.run(&["agent", "send", &agent, "GREET", "--follow"]);

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), REPLY);
    assert_eq!(agent_state(&user, &agent), "idle", "the agent stays alive");
}

#[test]
fn send_follow_prints_a_reply_as_agent_run_prints_it() {
    let user = user();
    project(&user);
    let ran = user.run(&["agent", "run", "app", "GREET"]);
    let ran = String::from_utf8(ran.stdout).unwrap();
    let agent = ran
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches(',')
        .to_owned();
    assert_eq!(ran.lines().last(), Some(REPLY.trim_end()), "{ran}");

    let output = user.run(&["agent", "send", &agent, "GREET", "-f"]);

    assert!(output.status.success());
    let printed = String::from_utf8(output.stdout).unwrap();
    assert_eq!(printed, REPLY);
    assert!(ran.ends_with(&printed));
}

#[test]
fn send_follow_waits_for_the_turn_of_its_own_message_not_for_the_one_that_runs() {
    let user = user();
    project(&user);
    let agent = json_of(user.run(&["agent", "start", "app", "HANG", "--json"]))["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the turn to run", || prompted_with(&user, "HANG"));
    let following = Running::start(&user, &["agent", "send", &agent, "GREET", "--follow"]);
    eventually("the message to be queued", || {
        let shown = json_of(user.run(&["agent", "show", &agent, "--json"]));
        shown["events"].to_string().contains("\"kind\":\"user\"")
    });

    // The turn that hangs ends cancelled; the command is not about it.
    user.call("agent.cancel", json!({"agent": agent})).unwrap();

    let (status, out, _) = following.finish();
    assert!(status.success());
    assert_eq!(out, REPLY);
}

#[test]
fn send_without_follow_returns_at_once_while_a_turn_runs() {
    let user = user();
    project(&user);
    let agent = json_of(user.run(&["agent", "start", "app", "HANG", "--json"]))["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually("the turn to run", || prompted_with(&user, "HANG"));

    let output = user.run(&["agent", "send", &agent, "GREET"]);

    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("Sent to agent")
    );
}

#[test]
fn send_follow_with_json_prints_the_answer_with_the_agent_once_its_turn_ended() {
    let user = user();
    let agent = idle_agent(&user);

    let output = user.run(&["agent", "send", &agent, "GREET", "--follow", "--json"]);

    let printed = json_of(output);
    assert_eq!(printed["agent"]["id"], agent.as_str());
    assert_eq!(printed["agent"]["state"], "idle");
    assert!(printed["after"].is_u64());
}

#[test]
fn send_follow_on_a_child_is_refused_naming_its_root_and_sends_nothing() {
    let user = user();
    let (child, root) = child_and_root(&user);

    let output = user.run(&["agent", "send", &child, "GREET", "--follow"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let refused = String::from_utf8(output.stderr).unwrap();
    assert!(
        refused.contains("only its parent") && refused.contains(&root),
        "{refused}"
    );
    assert!(!prompted_with(&user, "GREET"), "nothing reached the child");
}

#[test]
fn send_follow_with_an_empty_message_is_refused_and_follows_nothing() {
    let user = user();
    let agent = idle_agent(&user);

    let output = user.run(&["agent", "send", &agent, "", "--follow"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let refused = String::from_utf8(output.stderr).unwrap();
    assert!(
        refused.starts_with("flottille: ") && refused.contains("empty"),
        "{refused}"
    );
}
