//! The scenarios of a project's tasks and setup (`docs/tools.md`, "Tasks" and "Setup"): an agent
//! reads them at once, a root's write is judged (held for the user here, where no judge is
//! chosen), a child never writes them, a line a task or a step sets is judged as a run of it, the
//! user's Run of a task from the window, a setup changed there played by the next workspace, and
//! the tools' old names still served. The agents are the fake agent's sessions, driven through
//! their MCP server, on throwaway repositories and temporary XDG folders; a fake of each
//! escalating program stands first on the runs' `PATH`.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::approval::Delivery;
use flottille_core::store::Store;
use serde_json::{Value, json};

fn user() -> User {
    User::with_fake_agent().with_fake_escalation()
}

/// A project `app` that is one repository holding `say.sh`, which prints two lines, and `.env`
/// left out of git.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    let say = folder.join("say.sh");
    std::fs::write(&say, "#!/bin/sh\necho first\necho second\n").unwrap();
    std::fs::set_permissions(&say, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(folder.join(".gitignore"), ".env\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    std::fs::write(folder.join(".env"), "TOKEN=invented\n").unwrap();
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

/// A root agent the user started in the project folder: its id and MCP session.
fn root(user: &User) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let (url, authorization) = server_of(user, opened);
    (
        answer["agent"]["id"].as_str().unwrap().to_owned(),
        Mcp::initialize(&url, &authorization),
    )
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn held(answer: Answer) -> u64 {
    assert!(!answer.error, "an error: {}", answer.text);
    let rest = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    rest.split('.').next().unwrap().parse().unwrap()
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    assert!(answer.text.starts_with("refused: "), "{}", answer.text);
    answer.text
}

fn request(user: &User, number: u64) -> Value {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == number)
        .cloned()
        .unwrap_or_else(|| panic!("no pending request #{number}"))
}

fn no_request(user: &User) -> bool {
    user.call("approval.list", json!({})).unwrap()["requests"]
        .as_array()
        .unwrap()
        .is_empty()
}

fn approve(user: &User, number: u64) -> Delivery {
    let output = user.run(&["approve", &number.to_string()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id = format!("approval-{number}");
    let store = || Store::open(&user.path("state/flottille/flottille.db")).unwrap();
    eventually("the answer", || store().delivery(&id).unwrap().is_some());
    store().delivery(&id).unwrap().unwrap()
}

fn file(user: &User) -> String {
    std::fs::read_to_string(user.project_file("app")).unwrap()
}

fn add_say() -> Value {
    json!({"project": "app", "name": "say", "line": "./say.sh", "check": true})
}

#[test]
fn the_tasks_and_the_setup_are_read_at_once() {
    let user = user();
    project(&user);
    user.call("task.add", add_say()).unwrap();
    user.call(
        "setup.add",
        json!({"project": "app", "step": {"copy": [".env"], "for_each_repository": true}}),
    )
    .unwrap();
    let (_, mcp) = root(&user);

    let listed = done(mcp.call("tasks", json!({"action": "list"})));
    assert!(listed.contains("- say: `./say.sh` (a check)"), "{listed}");
    let shown = done(mcp.call("tasks", json!({"action": "show", "name": "say"})));
    assert!(shown.contains("./say.sh"), "{shown}");
    let setup = done(mcp.call("setup", json!({"action": "list"})));
    assert!(
        setup.contains("1. copy .env in each repository, leaves first"),
        "{setup}"
    );
    assert!(no_request(&user));
}

#[test]
fn a_task_a_root_adds_edits_or_removes_is_held_then_written_on_approval() {
    let user = user();
    project(&user);
    let (_, mcp) = root(&user);

    let number = held(mcp.call(
        "tasks",
        json!({"action": "add", "name": "say", "line": "./say.sh", "repository": "."}),
    ));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "action");
    assert_eq!(asked["class"], "judge");
    assert!(
        asked["says"]
            .to_string()
            .contains("Add the task say: `./say.sh` (in .)"),
        "{asked}"
    );
    assert!(!file(&user).contains("[tasks.say]"));
    approve(&user, number);
    let text = file(&user);
    assert!(text.contains("[tasks.say]"), "{text}");
    assert!(text.contains("repository = \".\""), "{text}");

    let number = held(mcp.call(
        "tasks",
        json!({"action": "edit", "name": "say", "repository": null, "serve": true}),
    ));
    let says = request(&user, number)["says"].to_string();
    assert!(says.contains("It was: `./say.sh` (in .)"), "{says}");
    approve(&user, number);
    let text = file(&user);
    assert!(
        text.contains("serve = true") && !text.contains("repository"),
        "{text}"
    );

    let number = held(mcp.call("tasks", json!({"action": "remove", "name": "say"})));
    approve(&user, number);
    assert!(!file(&user).contains("say"), "{}", file(&user));
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn a_setup_step_is_added_moved_edited_and_removed_on_approval() {
    let user = user();
    project(&user);
    let (_, mcp) = root(&user);

    for step in [
        json!({"copy": ["say.sh"]}),
        json!({"copy": ["dist"], "for_each_repository": true}),
    ] {
        let number = held(mcp.call("setup", json!({"action": "add", "step": step})));
        assert_eq!(request(&user, number)["class"], "judge");
        approve(&user, number);
    }
    let number = held(mcp.call("setup", json!({"action": "move", "number": 2, "to": 1})));
    let says = request(&user, number)["says"].to_string();
    assert!(says.contains("copy dist"), "{says}");
    approve(&user, number);
    let text = file(&user);
    assert!(
        text.find("\"dist\"").unwrap() < text.find("\"say.sh\"").unwrap(),
        "{text}"
    );

    let number = held(mcp.call(
        "setup",
        json!({"action": "edit", "number": 2, "step": {"copy": ["say.sh", "docs"]}}),
    ));
    approve(&user, number);
    assert!(file(&user).contains(r#""docs""#), "{}", file(&user));

    let number = held(mcp.call("setup", json!({"action": "remove", "number": 1})));
    approve(&user, number);
    let text = file(&user);
    assert!(
        !text.contains("dist") && text.contains("[[setup]]"),
        "{text}"
    );

    let unknown = refused(mcp.call("setup", json!({"action": "remove", "number": 9})));
    assert!(unknown.contains("its steps are 1 to 1"), "{unknown}");
}

#[test]
fn a_line_a_task_or_a_setup_step_sets_that_runs_sudo_is_base_and_never_runs() {
    let user = user();
    project(&user);
    let (_, mcp) = root(&user);

    let task = held(mcp.call(
        "tasks",
        json!({"action": "add", "name": "install", "line": "sudo make install"}),
    ));
    let step = held(mcp.call(
        "setup",
        json!({"action": "add", "step": {"run": "git push --force origin main"}}),
    ));
    for (number, class) in [(task, "base"), (step, "base")] {
        let asked = request(&user, number);
        assert_eq!(asked["class"], class, "{asked}");
        assert!(
            asked["reasons"].to_string().contains("the line it sets"),
            "{asked}"
        );
    }
    let shell = refused(mcp.call(
        "setup",
        json!({"action": "add", "step": {"run": "npm ci && npm test"}}),
    ));
    assert!(shell.contains("shell"), "{shell}");
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn a_child_reads_the_tasks_but_never_writes_them_nor_sees_the_setup() {
    let user = user();
    project(&user);
    user.call("task.add", add_say()).unwrap();
    let (_, lead) = root(&user);
    let opened = user.received_requests("session/new").len();
    let launched = lead.call(
        "agent_launch",
        json!({"title": "worker", "brief": "Write the tests."}),
    );
    assert!(!launched.error, "{}", launched.text);
    let (url, authorization) = server_of(&user, opened);
    let child = Mcp::initialize(&url, &authorization);

    let listed = done(child.call("tasks", json!({"action": "list"})));
    assert!(listed.contains("say"), "{listed}");
    let write = refused(child.call(
        "tasks",
        json!({"action": "add", "name": "lint", "line": "cargo clippy"}),
    ));
    assert!(write.contains("ask_parent"), "{write}");
    let setup = refused(child.call("setup", json!({"action": "list"})));
    assert!(setup.contains("has no tool setup"), "{setup}");
    assert!(no_request(&user));
}

/// The user's runs of `app`, as the project page lists them.
fn user_runs(user: &User) -> Vec<Value> {
    user.call(
        "run.list",
        json!({"project": "app", "user": true, "ended": true}),
    )
    .unwrap()["commands"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn the_user_s_run_of_a_task_runs_in_the_project_folder_and_its_output_is_listed() {
    let user = user();
    let folder = project(&user);
    user.call("task.add", add_say()).unwrap();

    let answer = user
        .call("task.run", json!({"project": "app", "name": "say"}))
        .unwrap();
    let run = answer["run"].as_str().unwrap().to_owned();
    eventually("the run's end", || {
        user_runs(&user)
            .iter()
            .any(|r| r["run"] == run.as_str() && r["state"] != "running")
    });
    let listed = user_runs(&user);
    let row = listed.iter().find(|r| r["run"] == run.as_str()).unwrap();
    assert_eq!(row["agent"], Value::Null, "{row}");
    assert_eq!(row["project"], "app");
    assert_eq!(row["name"], "say");
    assert_eq!(row["folder"], folder.to_str().unwrap());
    assert_eq!(row["last_line"], "second", "{row}");
    let output = user.call("run.output", json!({"run": run})).unwrap();
    assert_eq!(output["output"], "first\nsecond", "{output}");
    // The project's list holds it too; no agent's tree does.
    let project = user.call("run.list", json!({"project": "app", "ended": true}));
    assert!(project.unwrap().to_string().contains(&run));

    let shell = user
        .call(
            "task.add",
            json!({"project": "app", "name": "log", "line": "./say.sh > log"}),
        )
        .unwrap_err();
    assert!(shell.1.contains("shell syntax"), "{shell:?}");
    let unknown = user
        .call("task.run", json!({"project": "app", "name": "nope"}))
        .unwrap_err();
    assert!(unknown.1.contains("no task named nope"), "{unknown:?}");
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn a_setup_edited_in_the_window_is_played_by_a_new_workspace() {
    let user = user();
    project(&user);
    user.call("task.add", add_say()).unwrap();
    user.call(
        "setup.add",
        json!({"project": "app", "step": {"task": "say"}}),
    )
    .unwrap();
    user.call(
        "setup.add",
        json!({"project": "app", "step": {"copy": [".env"]}, "at": 1}),
    )
    .unwrap();
    let listed = user.call("setup.list", json!({"project": "app"})).unwrap();
    assert_eq!(listed["steps"][0]["copy"], json!([".env"]), "{listed}");
    assert_eq!(listed["steps"][1]["number"], 2, "{listed}");
    let removal = user
        .call("task.remove", json!({"project": "app", "name": "say"}))
        .unwrap_err();
    assert!(removal.1.contains("step 2 of the setup"), "{removal:?}");

    user.call(
        "workspace.create",
        json!({"project": "app", "name": "login"}),
    )
    .unwrap();
    let show = || {
        user.call("workspace.show", json!({"project": "app", "name": "login"}))
            .unwrap()
    };
    eventually("the setup's end", || show()["state"] != "preparing");
    let workspace = show();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    let kinds: Vec<&Value> = workspace["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["kind"])
        .collect();
    assert_eq!(kinds, ["worktree", "copy", "run"], "{workspace}");
    let folder = PathBuf::from(workspace["folder"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(folder.join(".env")).unwrap(),
        "TOKEN=invented\n"
    );
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn the_old_tool_names_still_work_under_their_new_ones() {
    let user = user();
    project(&user);
    user.call("task.add", add_say()).unwrap();
    let (_, mcp) = root(&user);

    let listed = done(mcp.call("commands_list", json!({})));
    assert!(listed.contains("- say:"), "{listed}");
    // In the project folder a task is judged: held, then run on approval.
    let number = held(mcp.call("commands_run", json!({"command": "say", "timeout": 10})));
    assert_eq!(request(&user, number)["tool"], "run");
    let body = approve(&user, number).body;
    assert!(body.contains("second"), "{body}");
    let ran = body.split("Its result:\n\n").nth(1).unwrap_or(&body);
    let run = ran
        .strip_prefix("run ")
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .to_owned();
    let status = done(mcp.call("commands_status", json!({"run": run})));
    assert!(status.contains("second"), "{status}");
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

/// A user whose sessions start in Auto with a judge that allows every call to judge: what is
/// still held is base.
fn judged_user() -> User {
    User::with_fake_agent()
        .with_fake_escalation()
        .with_allowing_judge()
}

/// The request a held call made is base, with a reason holding `why`.
fn base_held(user: &User, answer: Answer, why: &str) {
    let number = held(answer);
    let asked = request(user, number);
    assert_eq!(asked["class"], "base", "{asked}");
    assert!(asked["reasons"].to_string().contains(why), "{why}: {asked}");
}

/// The project's never list becomes `never`, written as the user would.
fn never(user: &User, never: &str) {
    let file = user.project_file("app");
    let text: String = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("never"))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&file, format!("never = {never}\n{text}")).unwrap();
}

#[test]
fn an_agent_in_auto_setting_a_line_that_copies_a_home_key_is_refused_for_a_task_and_a_step() {
    let user = judged_user();
    project(&user);
    let key = user.path("home/.ssh/id_ed25519");
    std::fs::create_dir_all(key.parent().unwrap()).unwrap();
    std::fs::write(&key, "invented\n").unwrap();
    let line = format!("cp {} k.txt", key.display());
    let (_, mcp) = root(&user);

    for answer in [
        mcp.call("setup", json!({"action": "add", "step": {"run": line}})),
        mcp.call(
            "tasks",
            json!({"action": "add", "name": "steal", "line": line}),
        ),
    ] {
        assert!(answer.error, "not refused: {}", answer.text);
        assert!(
            answer.text.starts_with("refused: ") && answer.text.contains(".ssh"),
            "{}",
            answer.text
        );
    }
    assert!(!file(&user).contains("id_ed25519"), "{}", file(&user));
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn a_setup_step_naming_a_never_listed_or_force_pushing_task_is_base() {
    let user = judged_user();
    project(&user);
    user.call("task.add", add_say()).unwrap();
    user.call(
        "task.add",
        json!({"project": "app", "name": "ship", "line": "git push --force origin main"}),
    )
    .unwrap();
    never(&user, r#"[["say"]]"#);
    let (_, mcp) = root(&user);

    base_held(
        &user,
        mcp.call("setup", json!({"action": "add", "step": {"task": "say"}})),
        "never list holds say",
    );
    base_held(
        &user,
        mcp.call("setup", json!({"action": "add", "step": {"task": "ship"}})),
        "the line it sets",
    );
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn a_task_cloning_a_never_listed_or_ask_before_running_task_is_base_to_set_and_to_run() {
    let user = judged_user();
    project(&user);
    user.call("task.add", add_say()).unwrap();
    user.call(
        "task.add",
        json!({"project": "app", "name": "careful", "line": "./say.sh first",
               "ask_before_running": true}),
    )
    .unwrap();
    // The user's own copy of the never-listed line, under another name.
    user.call(
        "task.add",
        json!({"project": "app", "name": "again", "line": "./say.sh"}),
    )
    .unwrap();
    never(&user, r#"[["say"]]"#);
    let (_, mcp) = root(&user);

    base_held(
        &user,
        mcp.call(
            "tasks",
            json!({"action": "add", "name": "copy", "line": "./say.sh"}),
        ),
        "never list holds say",
    );
    base_held(
        &user,
        mcp.call(
            "tasks",
            json!({"action": "add", "name": "hasty", "line": "./say.sh first"}),
        ),
        "the task careful",
    );
    // Its copy still asks: allowed as a task like the original would.
    done(mcp.call(
        "tasks",
        json!({"action": "add", "name": "fine", "line": "./say.sh first",
               "ask_before_running": true}),
    ));
    base_held(
        &user,
        mcp.call("run", json!({"task": "again"})),
        "never list holds say",
    );
    assert_eq!(user.escalations(), Vec::<String>::new(), "a call ran");
}

#[test]
fn turning_off_asking_before_a_task_runs_is_base() {
    let user = judged_user();
    project(&user);
    user.call(
        "task.add",
        json!({"project": "app", "name": "careful", "line": "./say.sh",
               "ask_before_running": true}),
    )
    .unwrap();
    let (_, mcp) = root(&user);

    base_held(
        &user,
        mcp.call(
            "tasks",
            json!({"action": "edit", "name": "careful", "ask_before_running": false}),
        ),
        "asking before the task runs",
    );
}

#[test]
fn an_agent_adding_a_link_or_a_root_entries_step_is_base() {
    let user = judged_user();
    project(&user);
    let (_, mcp) = root(&user);

    for step in [
        json!({"link": "node_modules"}),
        json!({"root_entries": true}),
    ] {
        base_held(
            &user,
            mcp.call("setup", json!({"action": "add", "step": step})),
            "the user's checkout",
        );
    }
    user.call(
        "setup.add",
        json!({"project": "app", "step": {"copy": [".env.example"]}}),
    )
    .unwrap();
    base_held(
        &user,
        mcp.call(
            "setup",
            json!({"action": "edit", "number": 1, "step": {"link": "data"}}),
        ),
        "the user's checkout",
    );
}

#[test]
fn a_copy_step_whose_glob_matches_a_sensitive_name_is_base() {
    let user = judged_user();
    project(&user);
    let (_, mcp) = root(&user);

    for glob in [".env", ".env*", "*.pem", "config/.npmrc", ".ssh"] {
        base_held(
            &user,
            mcp.call(
                "setup",
                json!({"action": "add", "step": {"copy": ["README.md", glob]}}),
            ),
            "sensitive",
        );
    }
    // A template is no secret: judged, and the judge allows it.
    let added = done(mcp.call(
        "setup",
        json!({"action": "add", "step": {"copy": [".env.example"]}}),
    ));
    assert!(added.contains("copy .env.example"), "{added}");
}

#[test]
fn an_agent_marking_a_task_a_check_or_read_only_is_base() {
    let user = judged_user();
    project(&user);
    user.call(
        "task.add",
        json!({"project": "app", "name": "say", "line": "./say.sh"}),
    )
    .unwrap();
    let (_, mcp) = root(&user);

    for (flag, why) in [("check", "a check"), ("read_only", "changes nothing")] {
        let mut add = json!({"action": "add", "name": format!("new-{flag}"), "line": "./say.sh x"});
        add[flag] = true.into();
        base_held(&user, mcp.call("tasks", add), why);
        let mut edit = json!({"action": "edit", "name": "say"});
        edit[flag] = true.into();
        base_held(&user, mcp.call("tasks", edit), why);
    }
}
