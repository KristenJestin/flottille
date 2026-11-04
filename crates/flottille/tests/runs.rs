//! The scenarios of running commands (`docs/tools.md`, "Running commands the user sees"): the
//! agent woken when a command that outlived its call ends, the commands of a session listed,
//! stopped and dismissed by the user, and `flottille commands`. The agent is the fake ACP agent,
//! in a throwaway repository and temporary XDG folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

/// The tasks the scenarios run: scripts of the workspace, written by `session`.
const CATALOGUE: &str = "\n[commands.ends]\nline = \"./ends.sh\"\n\
    [commands.quick]\nline = \"./quick.sh\"\n\
    [commands.server]\nline = \"./server.sh\"\nserve = true\n\
    [commands.stubborn]\nline = \"./stubborn.sh\"\nserve = true\n\
    [commands.leak]\nline = \"./leak.sh\"\n";

const SCRIPTS: [(&str, &str); 5] = [
    // Outlives a call that waits 1 s, then fails with a coloured last line.
    (
        "ends.sh",
        "#!/bin/sh\necho start\nsleep 2\nprintf '\\033[31mred\\033[0m tail-line\\n'\nexit 3\n",
    ),
    ("quick.sh", "#!/bin/sh\necho quick\n"),
    // A server: its child is in its group, and its pid is kept for the test.
    (
        "server.sh",
        "#!/bin/sh\necho listening on 8080\nsleep 300 &\necho $! > child.pid\nwait\n",
    ),
    // Ignores the stop's first signal: stopping it takes the whole grace period.
    (
        "stubborn.sh",
        "#!/bin/sh\ntrap '' TERM\necho stubborn\nwhile :; do sleep 1; done\n",
    ),
    (
        "leak.sh",
        "#!/bin/sh\nsleep 2\necho token ghp_fakefakefakefakefakefakefake0002\n",
    ),
];

/// A project `app` over a throwaway repository, its workspace `w` with the scripts, and the
/// session of an agent working there: the agent's id, its MCP session, the workspace's folder.
fn session(user: &User) -> (String, Mcp, PathBuf) {
    session_of(user, false)
}

/// `session`, the project holding a second repository `web` when `with_web`.
fn session_of(user: &User, with_web: bool) -> (String, Mcp, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let folder = user.path("work/app");
    user.git_init(&folder);
    if with_web {
        let web = folder.join("web");
        user.git_init(&web);
        std::fs::write(web.join("README.md"), "web\n").unwrap();
        user.git(&web, &["add", "."]);
        user.git(&web, &["commit", "--quiet", "-m", "start"]);
        std::fs::write(folder.join(".gitignore"), "web/\n").unwrap();
        user.git(&folder, &["add", ".gitignore"]);
    }
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
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(CATALOGUE);
    std::fs::write(&file, text).unwrap();
    let workspace = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    let workspace = PathBuf::from(workspace["folder"].as_str().unwrap());
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    for (name, script) in SCRIPTS {
        let path = workspace.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "hello"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (id, Mcp::initialize(&url, &authorization), workspace)
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

/// The id of the run an answer of `run` names.
fn run_id(text: &str) -> String {
    text.strip_prefix("run ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("no run in {text}"))
        .to_owned()
}

/// Runs a task, leaving the call after one second: its run's id.
fn start(mcp: &Mcp, command: &str) -> String {
    run_id(&ok(mcp.call("run", json!({"task": command, "timeout": 1}))))
}

/// Every prompt the fake agent was sent, oldest first.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|p| {
            p["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// The first prompt holding every one of `parts`, once one was sent.
fn prompted(user: &User, parts: &[&str]) -> String {
    let mut found = None;
    eventually(&format!("a prompt with {parts:?}"), || {
        found = prompts(user)
            .into_iter()
            .find(|p| parts.iter().all(|part| p.contains(part)));
        found.is_some()
    });
    found.unwrap()
}

fn never_prompted(user: &User, part: &str) {
    std::thread::sleep(Duration::from_millis(800));
    let all = prompts(user);
    assert!(!all.iter().any(|p| p.contains(part)), "{part} in {all:?}");
}

/// The commands `run.list` gives for a session.
fn listed(user: &User, root: &str) -> Vec<Value> {
    user.call("run.list", json!({"tree": root, "ended": true}))
        .unwrap()["commands"]
        .as_array()
        .unwrap()
        .clone()
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

fn cli(user: &User, args: &[&str]) -> Result<String, String> {
    let output = user.run(args);
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

#[test]
fn a_long_command_delivers_command_ended_with_its_code_and_tail() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);

    let run = start(&mcp, "ends");

    let prompt = prompted(&user, &["[flottille:command-ended]", &run]);
    assert!(prompt.contains("./ends.sh"), "{prompt}");
    assert!(prompt.contains("exited with code 3"), "{prompt}");
    assert!(prompt.contains("tail-line"), "{prompt}");
    assert!(prompt.contains("start"), "{prompt}");
    // The program's own bytes: the window turns them into colours, the agent is told them as
    // they were written.
    assert!(prompt.contains("\u{1b}[31mred"), "{prompt:?}");
}

#[test]
fn a_command_that_ended_within_its_call_delivers_nothing() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);

    let answer = ok(mcp.call("run", json!({"task": "quick"})));

    assert!(answer.contains("exited with code 0"), "{answer}");
    never_prompted(&user, "[flottille:command-ended]");
}

#[test]
fn the_output_a_delivery_holds_is_masked() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);

    let run = start(&mcp, "leak");

    let prompt = prompted(&user, &["[flottille:command-ended]", &run]);
    assert!(!prompt.contains("ghp_fake"), "{prompt}");
    assert!(prompt.contains("exited with code 0"), "{prompt}");
}

#[test]
fn a_server_stopped_from_the_window_delivers_and_says_so() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);
    let run = start(&mcp, "server");

    user.call("run.stop", json!({"run": run})).unwrap();

    let prompt = prompted(&user, &["[flottille:command-ended]", &run]);
    assert!(prompt.contains("stopped by the user"), "{prompt}");
    assert!(prompt.contains("listening on 8080"), "{prompt}");
}

#[test]
fn a_command_the_agent_stops_itself_delivers_nothing() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);
    let run = start(&mcp, "server");

    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));

    never_prompted(&user, "[flottille:command-ended]");
}

#[test]
fn stop_stops_the_whole_process_group() {
    let user = User::with_fake_agent();
    let (_, mcp, workspace) = session(&user);
    let run = start(&mcp, "server");
    eventually("the child's pid", || workspace.join("child.pid").exists());
    let child: u64 = std::fs::read_to_string(workspace.join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(child));

    user.call("run.stop", json!({"run": run})).unwrap();

    eventually("the child to be gone", || !alive(child));
}

#[test]
fn the_session_lists_a_running_command_and_its_last_line() {
    let user = User::with_fake_agent();
    let (agent, mcp, _) = session(&user);
    let run = start(&mcp, "server");

    let commands = listed(&user, &agent);

    assert_eq!(commands.len(), 1, "{commands:?}");
    let command = &commands[0];
    assert_eq!(command["run"], run);
    assert_eq!(command["agent"], agent);
    assert_eq!(command["state"], "running");
    assert_eq!(command["name"], "server");
    assert_eq!(command["line"], "./server.sh");
    assert_eq!(command["project"], "app");
    assert_eq!(command["last_line"], "listening on 8080");
    assert!(command["started_at"].as_u64().unwrap() > 0);
    // By project, the same.
    let by_project = user.call("run.list", json!({"project": "app"})).unwrap();
    assert_eq!(by_project["commands"][0]["run"], run);
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[test]
fn stop_stops_a_command_and_it_stays_listed_with_its_end_until_dismissed() {
    let user = User::with_fake_agent();
    let (agent, mcp, _) = session(&user);
    let run = start(&mcp, "server");

    user.call("run.stop", json!({"run": run})).unwrap();

    let commands = listed(&user, &agent);
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(commands[0]["state"], "stopped");
    assert!(commands[0]["ended_at"].as_u64().is_some());
    assert_eq!(commands[0]["last_line"], "listening on 8080");

    user.call("run.dismiss", json!({"run": run})).unwrap();

    assert!(listed(&user, &agent).is_empty());
}

#[test]
fn a_command_that_ended_by_itself_stays_listed_with_its_exit_code() {
    let user = User::with_fake_agent();
    let (agent, mcp, _) = session(&user);
    let run = start(&mcp, "ends");
    prompted(&user, &["[flottille:command-ended]", &run]);

    let commands = listed(&user, &agent);

    assert_eq!(commands[0]["state"], "exited");
    assert_eq!(commands[0]["exit_code"], 3);
    assert_eq!(
        commands[0]["last_line"], "\u{1b}[31mred\u{1b}[0m tail-line",
        "what the program wrote, for the window to colour"
    );
}

#[test]
fn a_dismissed_command_is_refused_while_it_runs_and_a_stranger_is_not_found() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);
    let run = start(&mcp, "server");

    let refused = user.call("run.dismiss", json!({"run": run})).unwrap_err();
    assert_eq!(refused.0, "refused", "{refused:?}");
    let missing = user
        .call("run.stop", json!({"run": "nothing"}))
        .unwrap_err();
    assert_eq!(missing.0, "not-found", "{missing:?}");
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[test]
fn the_ended_commands_of_a_session_that_ended_are_no_longer_listed() {
    let user = User::with_fake_agent();
    let (agent, mcp, _) = session(&user);
    let run = start(&mcp, "server");
    user.call("run.stop", json!({"run": run})).unwrap();
    assert_eq!(listed(&user, &agent).len(), 1);

    user.call("agent.stop", json!({"agent": agent})).unwrap();

    assert!(listed(&user, &agent).is_empty());
}

#[test]
fn the_output_of_a_run_is_read_masked_and_whole() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);
    let run = start(&mcp, "server");

    let read = user.call("run.output", json!({"run": run})).unwrap();

    assert_eq!(read["run"], run);
    assert_eq!(read["state"], "running");
    assert_eq!(read["output"], "listening on 8080");
    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
}

#[test]
fn flottille_runs_lists_the_running_commands_of_a_project_and_stops_one_and_commands_still_does() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);
    let run = start(&mcp, "server");

    let text = cli(&user, &["runs", "list", "app"]).unwrap();
    assert!(text.contains(&run), "{text}");
    assert!(text.contains("./server.sh"), "{text}");
    assert!(text.contains("listening on 8080"), "{text}");
    let json: Value =
        serde_json::from_str(&cli(&user, &["runs", "list", "app", "--json"]).unwrap()).unwrap();
    assert_eq!(json["commands"][0]["run"], run);

    let said = cli(&user, &["runs", "stop", &run]).unwrap();
    assert!(said.contains("stopped"), "{said}");
    let after = cli(&user, &["commands", "list", "app"]).unwrap();
    assert!(after.contains("No command is running"), "{after}");
}

#[test]
fn a_command_is_listed_with_the_tail_of_its_output() {
    let user = User::with_fake_agent();
    let (agent, mcp, _) = session(&user);
    let run = start(&mcp, "ends");
    prompted(&user, &["[flottille:command-ended]", &run]);

    let commands = listed(&user, &agent);

    assert_eq!(
        commands[0]["tail"],
        json!(["start", "\u{1b}[31mred\u{1b}[0m tail-line"]),
        "the lines the card of the window folds"
    );
    // A command that runs has its tail too: the card of its call follows it.
    let live = start(&mcp, "server");
    let running = listed(&user, &agent);
    let running = running.iter().find(|c| c["run"] == live).unwrap();
    assert_eq!(running["tail"], json!(["listening on 8080"]), "{running}");
    ok(mcp.call("run_status", json!({"run": live, "action": "stop"})));
}

#[test]
fn a_user_stop_while_the_call_waits_is_told_in_the_call_s_answer() {
    let user = User::with_fake_agent();
    let (agent, mcp, _) = session(&user);

    let answer = std::thread::scope(|scope| {
        let call = scope.spawn(|| mcp.call("run", json!({"task": "ends", "timeout": 30})));
        eventually("the command to run", || !listed(&user, &agent).is_empty());
        let run = listed(&user, &agent)[0]["run"].as_str().unwrap().to_owned();
        user.call("run.stop", json!({"run": run})).unwrap();
        call.join().unwrap()
    });

    assert!(!answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("was stopped by the user"),
        "{}",
        answer.text
    );
    // The agent's own stop says what it is: its own.
    let run = start(&mcp, "server");
    let own = ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));
    assert!(own.contains("was stopped after"), "{own}");
}

#[test]
fn the_runs_of_a_session_are_pruned_when_it_is_forgotten_for_good() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_CLOCK", "1");
    let (agent, mcp, _) = session(&user);
    let quick = run_id(&ok(mcp.call("run", json!({"task": "quick"}))));
    let server = start(&mcp, "server");
    user.call("run.stop", json!({"run": server})).unwrap();
    assert!(user.call("run.output", json!({"run": quick})).is_ok());

    user.call("agent.stop", json!({"agent": agent})).unwrap();
    user.call("test.advance", json!({"ms": 60_000})).unwrap();
    assert!(
        user.call("run.output", json!({"run": quick})).is_ok(),
        "the session can be resumed: its runs are kept"
    );
    // A session kept to be resumed is forgotten after 7 days.
    user.call("test.advance", json!({"ms": 7 * 24 * 3_600_000_u64}))
        .unwrap();

    for run in [&quick, &server] {
        let gone = user.call("run.output", json!({"run": run})).unwrap_err();
        assert_eq!(gone.0, "not-found", "{gone:?}");
    }
}

// Restarting a command (`run.restart`).

/// The run an answer of `run.restart` names.
fn restarted(user: &User, run: &str) -> Value {
    user.call("run.restart", json!({"run": run})).unwrap()["run"].clone()
}

#[test]
fn restart_runs_the_same_line_in_the_same_place() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (agent, mcp, workspace) = session(&user);
    let first = run_id(&ok(mcp.call("run", json!({"task": "quick"}))));

    let new = restarted(&user, &first);

    let second = new["run"].as_str().unwrap();
    assert_ne!(second, first);
    assert_eq!(new["line"], "./quick.sh");
    assert_eq!(new["name"], "quick", "a task restarts as the same task");
    assert_eq!(new["project"], "app");
    assert_eq!(new["agent"], agent);
    assert_eq!(
        PathBuf::from(new["folder"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        workspace.canonicalize().unwrap(),
        "in the workspace it ran in"
    );
    eventually("its output", || {
        user.call("run.output", json!({"run": second})).unwrap()["output"] == "quick"
    });
    assert!(user.escalations().is_empty());
}

#[test]
fn restart_runs_a_task_the_user_ran_as_the_same_task_in_the_project_folder() {
    use std::os::unix::fs::PermissionsExt;
    let user = User::with_fake_agent().with_fake_escalation();
    session(&user);
    let script = user.path("work/app/quick.sh");
    std::fs::write(&script, "#!/bin/sh\necho in the project\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let first = user
        .call("task.run", json!({"project": "app", "name": "quick"}))
        .unwrap()["run"]
        .as_str()
        .unwrap()
        .to_owned();

    let new = restarted(&user, &first);

    assert_ne!(new["run"], first.as_str());
    assert_eq!(new["name"], "quick");
    assert_eq!(new["agent"], Value::Null, "the user's run stays the user's");
    assert_eq!(
        PathBuf::from(new["folder"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        user.path("work/app").canonicalize().unwrap()
    );
    let second = new["run"].as_str().unwrap().to_owned();
    eventually("its output", || {
        user.call("run.output", json!({"run": second})).unwrap()["output"] == "in the project"
    });
    assert!(user.escalations().is_empty());
}

#[test]
fn restart_stops_a_running_command_first() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (_, mcp, workspace) = session(&user);
    let first = start(&mcp, "server");
    eventually("the child's pid", || workspace.join("child.pid").exists());
    let child: u64 = std::fs::read_to_string(workspace.join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    let new = restarted(&user, &first);

    let before = user.call("run.output", json!({"run": first})).unwrap();
    assert_eq!(before["state"], "stopped", "{before}");
    assert_eq!(new["state"], "running");
    eventually("the old group to be gone", || !alive(child));
    user.call("run.stop", json!({"run": new["run"]})).unwrap();
}

#[test]
fn an_agent_s_restarted_command_stays_the_agent_s_and_it_is_told() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (agent, mcp, _) = session(&user);
    let first = start(&mcp, "server");

    let new = restarted(&user, &first);

    let second = new["run"].as_str().unwrap().to_owned();
    assert_eq!(new["agent"], agent, "it stays the agent's");
    let prompt = prompted(&user, &["[flottille:command-ended]", &first, &second]);
    assert!(prompt.contains("restarted by the user"), "{prompt}");
    // The agent reads the new run like its own.
    let status = ok(mcp.call("run_status", json!({"run": second})));
    assert!(status.contains("is running"), "{status}");
    // The new run's end wakes it as the first one's did.
    user.call("run.stop", json!({"run": second})).unwrap();
    prompted(
        &user,
        &["[flottille:command-ended]", &second, "stopped by the user"],
    );
}

#[test]
fn restart_of_a_stranger_is_not_found() {
    let user = User::with_fake_agent().with_fake_escalation();
    session(&user);

    let missing = user
        .call("run.restart", json!({"run": "nothing"}))
        .unwrap_err();

    assert_eq!(missing.0, "not-found", "{missing:?}");
}

#[test]
fn the_command_line_restarts_a_run() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (agent, mcp, _) = session(&user);
    let first = run_id(&ok(mcp.call("run", json!({"task": "quick"}))));

    let said = cli(&user, &["runs", "restart", &first]).unwrap();

    let all = listed(&user, &agent);
    let new = all.iter().find(|c| c["run"] != first.as_str()).unwrap();
    assert!(said.contains(new["run"].as_str().unwrap()), "{said}");
}

#[test]
fn an_agent_is_told_when_its_ended_command_is_restarted() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (_, mcp, _) = session(&user);
    let first = start(&mcp, "server");
    // The agent was already told of the end of this very run.
    user.call("run.stop", json!({"run": first})).unwrap();
    prompted(
        &user,
        &["[flottille:command-ended]", &first, "stopped by the user"],
    );

    let new = restarted(&user, &first);

    let second = new["run"].as_str().unwrap().to_owned();
    let prompt = prompted(&user, &["[flottille:command-ended]", &second]);
    assert!(prompt.contains("restarted by the user"), "{prompt}");
    user.call("run.stop", json!({"run": second})).unwrap();
}

#[test]
fn a_second_restart_of_the_same_run_is_told_too() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (_, mcp, _) = session(&user);
    let first = start(&mcp, "server");

    let once = restarted(&user, &first)["run"].as_str().unwrap().to_owned();
    let twice = restarted(&user, &first)["run"].as_str().unwrap().to_owned();

    assert_ne!(once, twice);
    for second in [&once, &twice] {
        let prompt = prompted(&user, &["[flottille:command-ended]", &first, second]);
        assert!(prompt.contains("restarted by the user"), "{prompt}");
    }
    for second in [once, twice] {
        user.call("run.stop", json!({"run": second})).unwrap();
    }
}

#[test]
fn a_task_run_in_a_repository_restarts_in_that_repository() {
    use std::os::unix::fs::PermissionsExt;
    let user = User::with_fake_agent().with_fake_escalation();
    let (_, mcp, workspace) = session_of(&user, true);
    let script = workspace.join("web/quick.sh");
    std::fs::write(&script, "#!/bin/sh\necho in web\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let first = run_id(&ok(
        mcp.call("run", json!({"task": "quick", "repository": "web"}))
    ));

    let new = restarted(&user, &first);

    assert_eq!(new["name"], "quick");
    assert_eq!(
        PathBuf::from(new["folder"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        workspace.join("web").canonicalize().unwrap(),
        "in the repository it ran in"
    );
    let second = new["run"].as_str().unwrap().to_owned();
    eventually("its output", || {
        user.call("run.output", json!({"run": second})).unwrap()["output"] == "in web"
    });
}

#[test]
fn a_double_restart_starts_one_run() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (agent, mcp, _) = session(&user);
    let first = start(&mcp, "stubborn");

    let (one, other) = std::thread::scope(|scope| {
        let one = scope.spawn(|| user.call("run.restart", json!({"run": first})));
        // The first restart waits for the stop's grace period: the second comes during it.
        std::thread::sleep(Duration::from_millis(700));
        let other = user.call("run.restart", json!({"run": first}));
        (one.join().unwrap(), other)
    });

    let refused = other.unwrap_err();
    assert_eq!(refused.0, "refused", "{refused:?}");
    assert!(refused.1.contains("is being restarted"), "{refused:?}");
    let new = one.unwrap()["run"].clone();
    let all = listed(&user, &agent);
    assert_eq!(all.len(), 2, "the first and one new run: {all:?}");
    user.call("run.stop", json!({"run": new["run"]})).unwrap();
}

#[test]
fn a_restart_that_fails_after_the_stop_tells_the_agent() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (_, mcp, workspace) = session(&user);
    let first = start(&mcp, "server");
    std::fs::remove_file(workspace.join("server.sh")).unwrap();

    let failed = user.call("run.restart", json!({"run": first})).unwrap_err();

    assert_eq!(failed.0, "refused", "{failed:?}");
    let before = user.call("run.output", json!({"run": first})).unwrap();
    assert_eq!(before["state"], "stopped", "the stop happened: {before}");
    let prompt = prompted(&user, &["[flottille:command-ended]", &first, "restart"]);
    assert!(prompt.contains("stopped by the user"), "{prompt}");
    assert!(prompt.contains("could not"), "{prompt}");
    assert!(prompt.contains("./server.sh"), "{prompt}");
}

#[test]
fn a_command_of_an_ended_agent_restarts_as_the_user_s() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (agent, mcp, workspace) = session(&user);
    let first = run_id(&ok(mcp.call("run", json!({"task": "quick"}))));
    user.call("agent.stop", json!({"agent": agent})).unwrap();

    let answer = user.call("run.restart", json!({"run": first})).unwrap();

    let new = &answer["run"];
    assert_eq!(new["agent"], Value::Null, "it is the user's now");
    assert_eq!(new["name"], "quick");
    assert_eq!(
        PathBuf::from(new["folder"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        workspace.canonicalize().unwrap()
    );
    let said = answer["notice"].as_str().unwrap_or_default();
    assert!(said.contains(&agent) && said.contains("ended"), "{answer}");
    let second = new["run"].as_str().unwrap().to_owned();
    eventually("its output", || {
        user.call("run.output", json!({"run": second})).unwrap()["output"] == "quick"
    });
}

#[test]
fn a_restarted_command_that_ends_at_once_tells_its_agent_its_end() {
    let user = User::with_fake_agent().with_fake_escalation();
    let (_, mcp, _) = session(&user);
    let first = run_id(&ok(mcp.call("run", json!({"task": "quick"}))));

    let new = restarted(&user, &first);

    let second = new["run"].as_str().unwrap().to_owned();
    let prompt = prompted(
        &user,
        &["[flottille:command-ended]", &second, "exited with code 0"],
    );
    assert!(!prompt.contains("restarted by the user"), "{prompt}");
}

// Tasks that say when they are ready and where they answer (`docs/projects.md`, "Tasks").

/// Adds `tasks` (TOML) to the project `app`, and each script to the workspace's folder.
fn with_tasks(user: &User, workspace: &std::path::Path, tasks: &str, scripts: &[(&str, &str)]) {
    use std::os::unix::fs::PermissionsExt;
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(tasks);
    std::fs::write(&file, text).unwrap();
    for (name, script) in scripts {
        let path = workspace.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The run as `run.list` gives it for the session of `agent`.
fn listed_run(user: &User, agent: &str, run: &str) -> Value {
    listed(user, agent)
        .into_iter()
        .find(|c| c["run"] == run)
        .unwrap_or_else(|| panic!("{run} is not listed"))
}

/// A port nothing listens on, for the fake server to take.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A server that prints a line, then a second one a second later, and keeps running.
const BOOTING: (&str, &str) = (
    "booting.sh",
    "#!/bin/sh\necho booting\nsleep 1\necho 'ready in 12 ms'\nexec sleep 300\n",
);

#[test]
fn a_task_is_ready_on_its_line() {
    let user = User::with_fake_agent();
    let (agent, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.web]\nline = \"./booting.sh\"\nserve = true\nready = { line = \"ready in\" }\n",
        &[BOOTING],
    );

    let answer = ok(mcp.call("run", json!({"task": "web"})));

    let run = run_id(&answer);
    assert!(answer.contains("starting"), "{answer}");
    assert_eq!(listed_run(&user, &agent, &run)["ready"], "starting");
    eventually("ready", || {
        listed_run(&user, &agent, &run)["ready"] == "ready"
    });
    let status = ok(mcp.call("run_status", json!({"run": run})));
    assert!(status.contains("ready"), "{status}");
    assert!(!status.contains("starting"), "{status}");
}

#[test]
fn a_task_is_ready_when_its_url_answers() {
    let user = User::with_fake_agent();
    let (agent, mcp, workspace) = session(&user);
    let port = free_port();
    let server = common::fake_server();
    with_tasks(
        &user,
        &workspace,
        &format!(
            "\n[tasks.api]\nline = \"./api.sh\"\nserve = true\n\
             ready = {{ url = \"http://127.0.0.1:{port}/health\" }}\n"
        ),
        &[(
            "api.sh",
            &format!(
                "#!/bin/sh\nexec {} --print booting --wait 1500 --listen {port}\n",
                server.display()
            ),
        )],
    );

    let run = run_id(&ok(mcp.call("run", json!({"task": "api"}))));

    assert_eq!(listed_run(&user, &agent, &run)["ready"], "starting");
    eventually("ready", || {
        listed_run(&user, &agent, &run)["ready"] == "ready"
    });
}

#[test]
fn a_task_is_ready_when_its_command_exits_0() {
    let user = User::with_fake_agent();
    let (agent, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.db]\nline = \"./db.sh\"\nserve = true\nready = { command = \"./ping.sh {workspace}\" }\n",
        &[
            (
                "db.sh",
                "#!/bin/sh\nrm -f up\nsleep 1\ntouch up\nexec sleep 300\n",
            ),
            // Fails until the server is up, and says for which workspace it was asked.
            ("ping.sh", "#!/bin/sh\necho \"$1\" > pinged\ntest -f up\n"),
        ],
    );

    let run = run_id(&ok(mcp.call("run", json!({"task": "db"}))));

    assert_eq!(listed_run(&user, &agent, &run)["ready"], "starting");
    eventually("ready", || {
        listed_run(&user, &agent, &run)["ready"] == "ready"
    });
    assert_eq!(
        std::fs::read_to_string(workspace.join("pinged")).unwrap(),
        "w\n"
    );
}

#[test]
fn a_task_without_ready_has_no_ready_state() {
    let user = User::with_fake_agent();
    let (agent, mcp, _) = session(&user);

    let answer = ok(mcp.call("run", json!({"task": "server"})));

    let run = run_id(&answer);
    let listed = listed_run(&user, &agent, &run);
    assert_eq!(listed["ready"], Value::Null);
    assert_eq!(listed["url"], Value::Null);
    assert!(answer.contains("is running"), "{answer}");
    assert!(!answer.contains("starting"), "{answer}");
    assert!(!answer.contains("ready"), "{answer}");
}

#[test]
fn a_run_ending_before_ready_says_so() {
    let user = User::with_fake_agent();
    let (agent, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.web]\nline = \"./crash.sh\"\nserve = true\nready = { line = \"ready in\" }\n",
        &[(
            "crash.sh",
            "#!/bin/sh\necho booting\nsleep 1\necho 'port taken'\nexit 2\n",
        )],
    );

    let run = run_id(&ok(mcp.call("run", json!({"task": "web"}))));

    let prompt = prompted(&user, &["[flottille:command-ended]", &run]);
    assert!(prompt.contains("ended before it was ready"), "{prompt}");
    assert!(prompt.contains("exited with code 2"), "{prompt}");
    assert_eq!(listed_run(&user, &agent, &run)["ready"], "missed");
    let status = ok(mcp.call("run_status", json!({"run": run})));
    assert!(status.contains("ended before it was ready"), "{status}");
}

#[test]
fn a_templated_url_names_the_workspace() {
    let user = User::with_fake_agent();
    let (agent, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.web]\nline = \"./booting.sh\"\nserve = true\n\
         url = \"https://{workspace}.{project}.localhost\"\n",
        &[BOOTING],
    );

    let answer = ok(mcp.call("run", json!({"task": "web"})));

    let run = run_id(&answer);
    assert!(answer.contains("https://w.app.localhost"), "{answer}");
    let listed = listed_run(&user, &agent, &run);
    assert_eq!(listed["url"], "https://w.app.localhost");
    assert_eq!(
        listed["ready"],
        Value::Null,
        "no ready said, no ready state"
    );
}

#[test]
fn a_url_command_gives_the_address() {
    let user = User::with_fake_agent();
    let (agent, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.web]\nline = \"./booting.sh\"\nserve = true\n\
         url = { command = \"./where.sh {workspace}\" }\nready = { line = \"ready in\" }\n",
        &[
            BOOTING,
            (
                "where.sh",
                "#!/bin/sh\necho\necho \"https://$1.web.localhost\"\necho more\n",
            ),
        ],
    );

    let answer = ok(mcp.call("run", json!({"task": "web", "notify": "ready"})));

    let run = run_id(&answer);
    assert_eq!(
        listed_run(&user, &agent, &run)["url"],
        "https://w.web.localhost"
    );
    let prompt = prompted(&user, &["[flottille:command-ready]", &run]);
    assert!(
        prompt.contains("is ready at https://w.web.localhost"),
        "{prompt}"
    );
}

#[test]
fn a_failing_url_command_says_why() {
    let user = User::with_fake_agent();
    let (agent, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.web]\nline = \"./booting.sh\"\nserve = true\nurl = { command = \"./where.sh\" }\n\
         [tasks.odd]\nline = \"./booting.sh\"\nserve = true\nurl = { command = \"./odd.sh\" }\n",
        &[
            BOOTING,
            (
                "where.sh",
                "#!/bin/sh\necho 'no route for this worktree' >&2\nexit 1\n",
            ),
            ("odd.sh", "#!/bin/sh\necho 'localhost:3000'\n"),
        ],
    );

    let answer = ok(mcp.call("run", json!({"task": "web"})));
    let odd = run_id(&ok(mcp.call("run", json!({"task": "odd"}))));

    let run = run_id(&answer);
    let listed = listed_run(&user, &agent, &run);
    assert_eq!(listed["url"], Value::Null);
    let why = listed["url_error"].as_str().unwrap();
    assert!(why.contains("exited with code 1"), "{why}");
    assert!(why.contains("no route for this worktree"), "{why}");
    assert!(answer.contains(why), "{answer}");
    assert_eq!(listed_run(&user, &agent, &run)["state"], "running");
    let odd = listed_run(&user, &agent, &odd);
    assert!(
        odd["url_error"]
            .as_str()
            .unwrap()
            .contains("not an http:// or https:// address"),
        "{odd}"
    );
}

#[test]
fn notify_ready_tells_the_agent_when_ready() {
    let user = User::with_fake_agent();
    let (_, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.web]\nline = \"./booting.sh\"\nserve = true\nready = { line = \"ready in\" }\n\
         url = \"http://{workspace}.localhost:5173\"\n",
        &[BOOTING],
    );

    let answer = ok(mcp.call("run", json!({"task": "web", "notify": "ready"})));

    let run = run_id(&answer);
    assert!(answer.contains("starting"), "{answer}");
    let prompt = prompted(&user, &["[flottille:command-ready]", &run]);
    assert!(
        prompt.contains(&format!(
            "run {run}: web is ready at http://w.localhost:5173"
        )),
        "{prompt}"
    );
    never_prompted(&user, "[flottille:command-ended]");
}

#[test]
fn notify_on_a_task_without_ready_is_refused() {
    let user = User::with_fake_agent();
    let (_, mcp, _) = session(&user);

    let refused = mcp.call("run", json!({"task": "server", "notify": "ready"}));
    let line = mcp.call("run", json!({"line": "./quick.sh", "notify": "ready"}));
    let run = start(&mcp, "server");
    let later = mcp.call(
        "run_status",
        json!({"run": run, "action": "notify", "when": "ready"}),
    );

    for answer in [refused, line, later] {
        assert!(answer.error, "{}", answer.text);
        assert!(answer.text.contains("says no ready"), "{}", answer.text);
    }
}

#[test]
fn run_status_notify_on_a_started_run() {
    let user = User::with_fake_agent();
    let (_, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.web]\nline = \"./booting.sh\"\nserve = true\nready = { line = \"ready in\" }\n",
        &[BOOTING],
    );
    let run = run_id(&ok(mcp.call("run", json!({"task": "web"}))));

    let answer = ok(mcp.call(
        "run_status",
        json!({"run": run, "action": "notify", "when": "ready"}),
    ));

    assert!(answer.contains("starting"), "{answer}");
    let prompt = prompted(&user, &["[flottille:command-ready]", &run]);
    assert!(
        prompt.contains(&format!("run {run}: web is ready")),
        "{prompt}"
    );
    let again = ok(mcp.call(
        "run_status",
        json!({"run": run, "action": "notify", "when": "ready"}),
    ));
    assert!(again.contains("is ready"), "{again}");
}

#[test]
fn a_failed_address_stops_a_ready_url_that_uses_it() {
    let user = User::with_fake_agent();
    let (agent, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.api]\nline = \"./booting.sh\"\nserve = true\nurl = { command = \"./where.sh\" }\n\
         ready = { url = \"{url}/health\" }\n",
        &[
            BOOTING,
            (
                "where.sh",
                "#!/bin/sh\necho 'no route for TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789' >&2\nexit 1\n",
            ),
        ],
    );

    let answer = ok(mcp.call("run", json!({"task": "api"})));

    let run = run_id(&answer);
    let listed = listed_run(&user, &agent, &run);
    assert_eq!(listed["ready"], Value::Null, "{listed}");
    assert_eq!(listed["state"], "running");
    let why = listed["url_error"].as_str().unwrap();
    assert!(why.contains("cannot tell when it is ready"), "{why}");
    assert!(why.contains("no route"), "{why}");
    assert!(!why.contains("ghp_abcdef"), "{why}");
    assert!(answer.contains("cannot tell when it is ready"), "{answer}");
    assert!(!answer.contains("starting"), "{answer}");
}

#[test]
fn the_address_is_masked() {
    let user = User::with_fake_agent();
    let (agent, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.web]\nline = \"./booting.sh\"\nserve = true\nurl = { command = \"./where.sh\" }\n",
        &[
            BOOTING,
            (
                "where.sh",
                "#!/bin/sh\necho 'https://dev:hunter2pass@w.localhost/?t=ghp_abcdefghijklmnopqrstuvwxyz0123456789'\n",
            ),
        ],
    );

    let answer = ok(mcp.call("run", json!({"task": "web"})));

    let run = run_id(&answer);
    let url = listed_run(&user, &agent, &run)["url"].to_string();
    for shown in [&url, &answer] {
        assert!(shown.contains("w.localhost"), "{shown}");
        assert!(!shown.contains("hunter2pass"), "{shown}");
        assert!(!shown.contains("ghp_abcdef"), "{shown}");
    }
}

#[test]
fn a_probe_s_children_are_killed_when_the_run_ends() {
    let user = User::with_fake_agent();
    let (_, mcp, workspace) = session(&user);
    with_tasks(
        &user,
        &workspace,
        "\n[tasks.db]\nline = \"./db.sh\"\nserve = true\nready = { command = \"./ping.sh\" }\n",
        &[
            ("db.sh", "#!/bin/sh\nexec sleep 300\n"),
            // A probe that leaves a child of its own behind and waits for it.
            (
                "ping.sh",
                "#!/bin/sh\nsh -c 'echo $$ > child.pid; exec sleep 300' &\nwait\n",
            ),
        ],
    );
    let run = run_id(&ok(mcp.call("run", json!({"task": "db"}))));
    let pid = workspace.join("child.pid");
    eventually("the probe's child", || {
        std::fs::read_to_string(&pid).is_ok_and(|pid| pid.trim().parse().is_ok_and(alive))
    });
    let child: u64 = std::fs::read_to_string(&pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    ok(mcp.call("run_status", json!({"run": run, "action": "stop"})));

    // Well before the probe's own limit (5 s) would have killed it.
    let deadline = Instant::now() + Duration::from_millis(2500);
    while alive(child) {
        assert!(
            Instant::now() < deadline,
            "the probe's child {child} still runs"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
