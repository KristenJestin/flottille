//! The scenarios of what an agent does of what the window does (`docs/tools.md`, "The project"):
//! the project's skills, commands and settings read at once, every write of the project to judge (held
//! for the user here, where no judge is chosen), a change of the tracker base, and the lines an
//! agent runs in the project folder classed as anywhere else. The agents are the fake agent's
//! sessions, driven through their MCP server as their client would, on throwaway repositories and
//! temporary XDG folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::approval::Delivery;
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A project `app` that is one repository with a docs folder, its file starting with a comment,
/// its command `ship`, and its ready workspace `login`.
fn project(user: &User) -> PathBuf {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::create_dir_all(folder.join("docs")).unwrap();
    std::fs::write(folder.join("docs/STYLE.md"), "Write short.\n").unwrap();
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
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("# The user's own words.\n{text}")).unwrap();
    user.call(
        "command.add",
        json!({"project": "app", "name": "ship", "description": "Ship the workspace",
            "instructions": "Push the branch."}),
    )
    .unwrap();
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    folder
}

/// A root agent the user started, in `login` or in the project folder: its id and MCP session.
fn root(user: &User, workspace: Option<&str>) -> (String, Mcp) {
    let opened = user.received_requests("session/new").len();
    let mut params = json!({"project": "app", "message": "hello"});
    if let Some(workspace) = workspace {
        params["workspace"] = workspace.into();
    }
    let answer = user.call("agent.run", params).unwrap();
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

/// The number of the request a held call's answer names.
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

fn store(user: &User) -> Store {
    Store::open(&user.path("state/flottille/flottille.db")).unwrap()
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

fn cli(user: &User, args: &[&str]) -> Result<String, String> {
    let output = user.run(args);
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// The answer stored for the agent once the request is settled.
fn settled(user: &User, number: u64) -> Delivery {
    let id = format!("approval-{number}");
    eventually("the answer", || {
        store(user).delivery(&id).unwrap().is_some()
    });
    store(user).delivery(&id).unwrap().unwrap()
}

fn file(user: &User) -> String {
    std::fs::read_to_string(user.project_file("app")).unwrap()
}

fn add_style() -> Value {
    json!({"action": "add", "name": "style", "description": "The code style",
        "path": "docs/STYLE.md"})
}

#[test]
fn each_family_s_read_runs_at_once() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, Some("login"));

    user.call(
        "skill.add",
        json!({"project": "app", "name": "style", "description": "The code style",
            "text": "Write short."}),
    )
    .unwrap();

    let listed = done(mcp.call("commands", json!({"action": "list"})));
    assert!(listed.contains("- /ship: Ship the workspace"), "{listed}");
    let shown = done(mcp.call("commands", json!({"action": "show", "name": "ship"})));
    assert!(shown.contains("Push the branch."), "{shown}");
    let listed = done(mcp.call("skills", json!({"action": "list"})));
    assert!(listed.contains("style"), "{listed}");
    let shown = done(mcp.call("skills", json!({"action": "show", "name": "style"})));
    assert!(shown.contains("Write short."), "{shown}");
    let settings = done(mcp.call("project", json!({"action": "settings"})));
    assert!(settings.contains("Project app."), "{settings}");
    assert!(settings.contains("Tracker: none"), "{settings}");
    assert!(!settings.contains("on their own"), "{settings}");
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_write_is_held_then_done_on_approval_and_not_done_on_refusal() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, Some("login"));

    let number = held(mcp.call("skills", add_style()));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "action");
    assert_eq!(
        asked["says"],
        json!(["Add the skill style (docs/STYLE.md): The code style"])
    );
    cli(&user, &["deny", &number.to_string()]).unwrap();
    assert!(
        settled(&user, number)
            .body
            .contains("was refused by the user")
    );
    assert!(!file(&user).contains("[skills.style]"));

    let number = held(mcp.call("skills", add_style()));
    cli(&user, &["approve", &number.to_string()]).unwrap();
    let body = settled(&user, number).body;
    assert!(
        body.starts_with(&format!(
            "Request #{number} (skills) was approved by the user, and Flottille has now done it."
        )),
        "{body}"
    );
    let text = file(&user);
    assert!(text.contains("[skills.style]"), "{text}");
    assert!(text.starts_with("# The user's own words.\n"), "{text}");
}

#[test]
fn an_action_of_an_agent_is_to_judge_and_a_change_of_the_tracker_is_base() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, None);

    let skill = held(mcp.call("skills", add_style()));
    let command = held(mcp.call(
        "commands",
        json!({"action": "edit", "name": "ship", "instructions": "Push it."}),
    ));
    let removal = held(mcp.call("commands", json!({"action": "remove", "name": "ship"})));
    let forge = held(mcp.call(
        "project",
        json!({"action": "set_settings", "repositories": [{"path": ".", "forge": null}]}),
    ));
    let tracker = held(mcp.call(
        "project",
        json!({"action": "set_settings", "tracker": {"kind": "github", "repository": "acme/app"}}),
    ));

    for number in [skill, command, removal, forge] {
        assert_eq!(request(&user, number)["class"], "judge", "#{number}");
        assert_eq!(request(&user, number)["kind"], "action", "#{number}");
    }
    assert_eq!(request(&user, tracker)["class"], "base");
    let error = cli(&user, &["approve", &skill.to_string(), "--for-tree"]).unwrap_err();
    assert!(error.contains("--for-tree"), "{error}");
    assert!(!file(&user).contains("[tracker]"));
}

#[test]
fn a_settings_change_says_what_changes_and_keeps_the_file_s_comments() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, None);
    let jira = json!({"kind": "jira", "site": "acme.atlassian.net", "projects": ["ABC"]});

    let number = held(mcp.call(
        "project",
        json!({"action": "set_settings", "tracker": jira}),
    ));
    assert_eq!(
        request(&user, number)["says"],
        json!(["Set the tracker to Jira on acme.atlassian.net (ABC)"])
    );
    cli(&user, &["approve", &number.to_string()]).unwrap();
    let body = settled(&user, number).body;
    assert!(body.contains("Saved."), "{body}");
    let text = file(&user);
    assert!(text.contains("[tracker]"), "{text}");
    assert!(text.starts_with("# The user's own words.\n"), "{text}");
    // A tracker left out is kept.
    let number = held(mcp.call(
        "project",
        json!({"action": "set_settings", "repositories": [{"path": ".", "forge": null}]}),
    ));
    cli(&user, &["approve", &number.to_string()]).unwrap();
    settled(&user, number);
    assert!(file(&user).contains("[tracker]"));
}

#[test]
fn an_old_agent_actions_table_lets_nothing_run_on_its_own() {
    let user = User::with_fake_agent();
    project(&user);
    let path = user.project_file("app");
    let text = file(&user);
    std::fs::write(
        &path,
        format!("{text}\n[agent_actions]\nreferences = \"allow\"\n"),
    )
    .unwrap();
    let (_, mcp) = root(&user, Some("login"));

    held(mcp.call("skills", add_style()));
    assert!(!file(&user).contains("[skills.style]"));
}

#[test]
fn the_daemon_s_start_removes_grants_families_and_always_from_the_project_s_file_keeping_the_rest()
{
    let user = User::with_fake_agent();
    project(&user);
    assert!(user.run(&["daemon", "stop"]).status.success());
    // The file as the release before wrote it: `ship` a procedure, `ls` in the catalogue.
    let old = file(&user).replace("[commands.ship]", "[procedures.ship]")
        + "\n[agent_actions]\nprocedures = \"allow\"\n\
           \n[procedures.ship.grants]\npush = \"allow\"\n\
           \n[commands.ls]\nline = \"ls\"\nadded_by = \"approval\"\n";
    std::fs::write(user.project_file("app"), &old).unwrap();

    user.status();

    let text = file(&user);
    assert!(!text.contains("agent_actions"), "{text}");
    assert!(
        !text.contains("grants") && !text.contains("push = "),
        "{text}"
    );
    assert!(!text.contains("added_by"), "{text}");
    assert!(text.starts_with("# The user's own words.\n"), "{text}");
    assert!(
        text.contains("[commands.ship]") && text.contains("[tasks.ls]"),
        "{text}"
    );
    assert!(!text.contains("procedures"), "{text}");
    let shown = user
        .call("project.settings", json!({"name": "app"}))
        .unwrap();
    assert_eq!(shown["obsolete"], json!([]));
}

#[test]
fn a_text_skill_is_created_by_an_agent_and_read_back() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, Some("login"));

    let number = held(mcp.call(
        "skills",
        json!({"action": "add", "name": "style", "description": "The code style",
            "text": "Short functions, no abbreviations."}),
    ));
    assert_eq!(
        request(&user, number)["says"],
        json!([
            "Add the skill style (a text Flottille keeps): The code style",
            "Its text: Short functions, no abbreviations.",
        ])
    );
    cli(&user, &["approve", &number.to_string()]).unwrap();
    settled(&user, number);

    let read = done(mcp.call("skill_read", json!({"name": "style"})));
    assert!(
        read.contains("Short functions, no abbreviations."),
        "{read}"
    );
    let text = file(&user);
    assert!(text.contains("[skills.style]"), "{text}");
    assert!(
        !text.contains("Short functions"),
        "the text is Flottille's: {text}"
    );
}

#[test]
fn an_agent_never_passes_a_grant_sets_what_runs_on_its_own_nor_reaches_another_project() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, None);

    for (tool, args) in [
        (
            "commands",
            json!({"action": "add", "name": "go", "description": "Go", "instructions": "Go.",
                "grants": {"push": "allow"}}),
        ),
        (
            "commands",
            json!({"action": "remove", "name": "ship", "project": "other"}),
        ),
        (
            "skills",
            json!({"action": "add", "name": "x", "description": "x", "text": "x",
                "project": "other"}),
        ),
        (
            "project",
            json!({"action": "set_settings", "agent_actions": {"procedures": "allow"}}),
        ),
        (
            "project",
            json!({"action": "set_settings", "project": "other", "tracker": null}),
        ),
    ] {
        let said = refused(mcp.call(tool, args.clone()));
        assert!(said.contains("the arguments do not fit"), "{args}: {said}");
    }
    // A skill stays inside the project folder.
    let outside = refused(mcp.call(
        "skills",
        json!({"action": "add", "name": "x", "description": "x", "path": "../x"}),
    ));
    assert!(outside.contains("skills.x"), "{outside}");
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty(),
        "nothing was asked"
    );
    assert!(!file(&user).contains("agent_actions"));
}

#[test]
fn in_main_a_known_read_runs_at_once_and_a_write_is_held_then_run_once() {
    let user = User::with_fake_agent();
    let folder = project(&user);
    let (_, mcp) = root(&user, None);
    let before = file(&user);

    let read = done(mcp.call("commands_run", json!({"line": "cat README.md"})));
    assert!(read.contains("exited with code 0"), "{read}");
    let number = held(mcp.call("commands_run", json!({"line": "touch notes.txt"})));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "call");
    assert_eq!(asked["class"], "judge");
    assert!(!folder.join("notes.txt").exists());
    cli(&user, &["approve", &number.to_string()]).unwrap();

    let body = settled(&user, number).body;
    assert!(body.contains("and Flottille has now done it"), "{body}");
    assert!(body.contains("exited with code 0"), "{body}");
    assert!(folder.join("notes.txt").exists());
    assert_eq!(file(&user), before, "nothing is added to the tasks");
    // Once: the next run asks again.
    held(mcp.call("commands_run", json!({"line": "touch notes.txt"})));
}

#[test]
fn in_main_shell_syntax_is_still_refused_and_a_never_command_is_base() {
    let user = User::with_fake_agent();
    project(&user);
    let path = user.project_file("app");
    let text: String = file(&user)
        .lines()
        .filter(|line| !line.starts_with("never"))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&path, format!("never = [[\"deploy\"]]\n{text}")).unwrap();
    let (_, mcp) = root(&user, None);

    // Refused in the words a workspace's run is refused in.
    let shell = mcp.call("commands_run", json!({"line": "cat README.md | wc -l"}));
    assert!(shell.error, "{}", shell.text);
    assert!(shell.text.contains("is shell syntax"), "{}", shell.text);
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let never = held(mcp.call("commands_run", json!({"line": "deploy now"})));
    assert_eq!(request(&user, never)["class"], "base");
    let push = held(mcp.call("commands_run", json!({"line": "git push origin main"})));
    assert_eq!(request(&user, push)["class"], "judge");
}

#[test]
fn an_agent_s_skill_on_the_project_folder_or_a_repository_is_refused() {
    let user = User::with_fake_agent();
    let folder = project(&user);
    std::os::unix::fs::symlink(&folder, folder.join("docs/all")).unwrap();
    let (_, mcp) = root(&user, Some("login"));

    for path in [".", "./", "docs/all"] {
        let said = refused(mcp.call(
            "skills",
            json!({"action": "add", "name": "all", "description": "Everything",
                "path": path}),
        ));
        assert!(said.contains("the project folder"), "{path}: {said}");
    }
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty(),
        "nothing was asked"
    );
    assert!(!file(&user).contains("[skills.all]"));
}

#[test]
fn a_command_whose_instructions_changed_after_it_was_asked_is_not_written() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, Some("login"));
    let number = held(mcp.call(
        "commands",
        json!({"action": "edit", "name": "ship", "instructions": "Push it, then open the PR."}),
    ));
    assert!(
        request(&user, number)["says"]
            .as_array()
            .unwrap()
            .contains(&json!("They were: Push the branch."))
    );
    user.call(
        "command.edit",
        json!({"project": "app", "name": "ship", "instructions": "Push it, then force-push."}),
    )
    .unwrap();

    cli(&user, &["approve", &number.to_string()]).unwrap();

    let body = settled(&user, number).body;
    assert!(
        body.starts_with(&format!(
            "Request #{number} (commands) was not executed: what it does changed since the user \
             allowed it"
        )),
        "{body}"
    );
    assert!(body.contains("then force-push"), "{body}");
    let shown = user
        .call("command.show", json!({"project": "app", "name": "ship"}))
        .unwrap();
    assert_eq!(shown["instructions"], "Push it, then force-push.");
}

#[test]
fn a_line_equal_to_a_catalogue_command_is_that_command_and_is_judged_in_main() {
    let user = User::with_fake_agent();
    project(&user);
    let path = user.project_file("app");
    let text = file(&user);
    std::fs::write(
        &path,
        format!(
            "{text}\n[tasks.build]\nline = \"touch built.txt\"\n\n\
             [tasks.status]\nline = \"git status\"\ncheck = true\nread_only = true\n\
             ask_before_running = true\n"
        ),
    )
    .unwrap();
    let (_, mcp) = root(&user, None);
    let before = file(&user);

    // In the project folder, the user's own checkout: judged, as that command.
    let build = held(mcp.call("commands_run", json!({"line": "touch  built.txt"})));
    let asked = request(&user, build);
    assert_eq!(asked["class"], "judge");
    assert!(
        asked["reasons"]
            .to_string()
            .contains("the catalogue command build runs in the project folder"),
        "{asked}"
    );
    // Marked to ask: held as that command.
    let status = held(mcp.call("commands_run", json!({"line": "git  status"})));
    assert!(
        request(&user, status)["reasons"]
            .to_string()
            .contains("marked to ask before it runs"),
        "{}",
        request(&user, status)
    );
    assert_eq!(file(&user), before, "nothing duplicated");
}

#[test]
fn in_main_a_line_that_discards_the_user_s_work_is_base() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, None);

    for line in [
        "git checkout -- README.md",
        "git restore README.md",
        "git clean -fd",
        "git stash drop",
        "git reset --hard",
        "git worktree remove ../w",
        "git branch -D old",
    ] {
        let number = held(mcp.call("commands_run", json!({"line": line})));
        let asked = request(&user, number);
        assert_eq!(asked["class"], "base", "{line}");
        assert!(
            asked["reasons"]
                .to_string()
                .contains("the user's own checkout"),
            "{line}: {asked}"
        );
    }
}

#[test]
fn a_sensitive_line_is_base_and_allowed_only_once() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, None);
    let number = held(mcp.call("commands_run", json!({"line": "cat .env"})));
    assert_eq!(request(&user, number)["class"], "base");

    let error = cli(&user, &["approve", &number.to_string(), "--always"]).unwrap_err();
    assert!(error.contains("--always"), "{error}");
    assert_eq!(request(&user, number)["state"], "pending");
    assert!(!file(&user).contains("[tasks."));
}

#[test]
fn an_agent_can_never_change_its_mode_nor_choose_the_judge() {
    let user = User::with_fake_agent();
    project(&user);
    let (id, mcp) = root(&user, None);

    // No tool of an agent sets either.
    for name in mcp.names() {
        assert!(!name.contains("mode") && !name.contains("judge"), "{name}");
    }
    for args in [
        json!({"action": "set_settings", "gate_mode": "ask"}),
        json!({"action": "set_settings", "judge": {"provider": "claude"}}),
    ] {
        let said = refused(mcp.call("project", args.clone()));
        assert!(said.contains("the arguments do not fit"), "{args}: {said}");
    }
    // Nor through a command line: the daemon's own command is refused, and Flottille's own
    // folder is base, which only the user allows.
    let said = refused(mcp.call(
        "commands_run",
        json!({"line": format!("flottille agent mode {id} ask")}),
    ));
    assert!(said.contains("flottille drives the daemon"), "{said}");
    let settings = user.path("config/flottille/providers.toml");
    let number = held(mcp.call(
        "fs_write",
        json!({"path": settings, "content": "[judge]\nprovider = \"claude\"\n"}),
    ));
    assert_eq!(request(&user, number)["class"], "base");
    assert_eq!(
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["gate_mode"],
        "auto"
    );
}
