//! The scenarios of what an agent does of what the window does (`docs/tools.md`, "The project"):
//! the project's procedures and settings read at once, every write held for the user unless the
//! project lets its family run on its own, and a line outside the catalogue in the project
//! folder held to be allowed once or always. The agents are the fake agent's sessions, driven
//! through their MCP server as their client would, on throwaway repositories and temporary XDG
//! folders.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use flottille_core::approval::Delivery;
use flottille_core::store::Store;
use serde_json::{Value, json};

/// A project `app` that is one repository with a docs folder, its file starting with a comment,
/// its procedure `ship` (push, asked), and its ready workspace `login`.
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
        "procedure.add",
        json!({"project": "app", "name": "ship", "description": "Ship the workspace",
            "instructions": "Push the branch.", "grants": {"push": "ask"}}),
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
    json!({"action": "add_reference", "name": "style", "description": "The code style",
        "path": "docs/STYLE.md"})
}

#[test]
fn each_family_s_read_runs_at_once() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, Some("login"));

    let listed = done(mcp.call("procedure", json!({"action": "list"})));
    assert!(listed.contains("- ship: Ship the workspace"), "{listed}");
    assert!(listed.contains("push (ask)"), "{listed}");
    let shown = done(mcp.call("procedure", json!({"action": "show", "name": "ship"})));
    assert!(shown.contains("Push the branch."), "{shown}");
    let settings = done(mcp.call("project", json!({"action": "settings"})));
    assert!(settings.contains("Project app."), "{settings}");
    assert!(settings.contains("Tracker: none"), "{settings}");
    assert!(
        settings
            .contains("What agents do on their own: procedures ask, settings ask, references ask"),
        "{settings}"
    );
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

    let number = held(mcp.call("project", add_style()));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "action");
    assert_eq!(
        asked["says"],
        json!(["Add the reference style (docs/STYLE.md): The code style"])
    );
    cli(&user, &["deny", &number.to_string()]).unwrap();
    assert!(
        settled(&user, number)
            .body
            .contains("was refused by the user")
    );
    assert!(!file(&user).contains("[references.style]"));

    let number = held(mcp.call("project", add_style()));
    cli(&user, &["approve", &number.to_string()]).unwrap();
    let body = settled(&user, number).body;
    assert!(
        body.starts_with(&format!(
            "Request #{number} (project) was approved by the user, and Flottille has now done it."
        )),
        "{body}"
    );
    let text = file(&user);
    assert!(text.contains("[references.style]"), "{text}");
    assert!(text.starts_with("# The user's own words.\n"), "{text}");
}

#[test]
fn a_change_of_the_project_is_allowed_once_never_for_the_tree() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, Some("login"));
    let number = held(mcp.call("project", add_style()));

    let error = cli(&user, &["approve", &number.to_string(), "--for-tree"]).unwrap_err();
    assert!(error.contains("never for the tree"), "{error}");
    let error = cli(&user, &["approve", &number.to_string(), "--always"]).unwrap_err();
    assert!(error.contains("only such a line"), "{error}");
    assert_eq!(request(&user, number)["state"], "pending");
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
fn a_family_set_to_allow_runs_at_once() {
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

    let added = done(mcp.call("project", add_style()));
    assert!(added.starts_with("Reference style added"), "{added}");
    assert!(file(&user).contains("[references.style]"));
    // Another family still asks.
    held(mcp.call("procedure", json!({"action": "start", "name": "ship"})));
}

#[test]
fn an_agent_s_procedure_start_is_held_confirmed_then_started_in_its_workspace() {
    let user = User::with_fake_agent();
    project(&user);
    let (lead, mcp) = root(&user, Some("login"));
    let agents = || {
        user.call("agent.list", json!({})).unwrap()["agents"]
            .as_array()
            .unwrap()
            .len()
    };
    let before = agents();

    let number = held(mcp.call("procedure", json!({"action": "start", "name": "ship"})));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "action");
    assert_eq!(
        asked["says"],
        json!([
            "Start the procedure ship in workspace login: Ship the workspace",
            "Its agents may push the workspace's branch to its remote (git push), after asking \
             you, each time",
            "The agents the run starts may do the same"
        ])
    );
    assert_eq!(agents(), before, "nothing starts before the user confirms");

    cli(&user, &["approve", &number.to_string()]).unwrap();
    let body = settled(&user, number).body;
    let started = body
        .split("its agent is ")
        .nth(1)
        .unwrap_or_else(|| panic!("{body}"))[..12]
        .to_owned();
    assert_ne!(started, lead);
    let run = user
        .call("procedure.of", json!({"agent": started}))
        .unwrap()["run"]
        .clone();
    assert_eq!(run["procedure"], "ship");
    assert_eq!(run["workspace"], "login");
    assert_eq!(run["grants"], json!({"push": "ask"}));
    // The lead holds none of it.
    assert!(user.call("procedure.of", json!({"agent": lead})).unwrap()["run"].is_null());
}

#[test]
fn a_procedure_edited_after_it_was_asked_is_not_started() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, Some("login"));
    let number = held(mcp.call("procedure", json!({"action": "start", "name": "ship"})));
    user.call(
        "procedure.edit",
        json!({"project": "app", "name": "ship", "grants": {"push": "allow"}}),
    )
    .unwrap();

    cli(&user, &["approve", &number.to_string()]).unwrap();

    let body = settled(&user, number).body;
    assert!(
        body.starts_with(&format!(
            "Request #{number} (procedure) was not executed: what it does changed since the user \
             allowed it"
        )),
        "{body}"
    );
    assert!(body.contains("on their own"), "{body}");
}

#[test]
fn an_agent_never_gives_a_grant_sets_what_runs_on_its_own_nor_reaches_another_project() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, None);

    for (tool, args) in [
        (
            "procedure",
            json!({"action": "start", "name": "ship", "workspace": "login",
                "grants": {"push": "allow"}}),
        ),
        (
            "procedure",
            json!({"action": "start", "name": "ship", "workspace": "login", "project": "other"}),
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
    // A procedure starts only in a workspace, the agent's own when it has one.
    let said = refused(mcp.call("procedure", json!({"action": "start", "name": "ship"})));
    assert!(said.contains("runs in a workspace"), "{said}");
    // A reference stays inside the project folder.
    let outside = refused(mcp.call(
        "project",
        json!({"action": "add_reference", "name": "x", "description": "x", "path": "../x"}),
    ));
    assert!(outside.contains("references.x"), "{outside}");
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
fn a_line_outside_the_catalogue_in_main_is_held_and_once_runs_it_without_changing_the_file() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, None);
    let before = file(&user);

    let number = held(mcp.call("commands_run", json!({"line": "cat README.md"})));
    let asked = request(&user, number);
    assert_eq!(asked["kind"], "catalogue");
    assert_eq!(
        asked["says"],
        json!([
            "Run cat README.md in the project folder",
            "Always adds it to the project's catalogue as cat-readme-md, a read-only check its \
             agents run there without asking"
        ])
    );
    let error = cli(&user, &["approve", &number.to_string(), "--for-tree"]).unwrap_err();
    assert!(error.contains("never"), "{error}");
    cli(&user, &["approve", &number.to_string()]).unwrap();

    let body = settled(&user, number).body;
    assert!(body.contains("and Flottille has now done it"), "{body}");
    assert!(body.contains("exited with code 0"), "{body}");
    assert_eq!(file(&user), before);
    // Once: the next run asks again.
    held(mcp.call("commands_run", json!({"line": "cat README.md"})));
}

#[test]
fn always_runs_it_and_writes_it_in_the_catalogue() {
    let user = User::with_fake_agent();
    project(&user);
    let (_, mcp) = root(&user, None);

    let number = held(mcp.call("commands_run", json!({"line": "cat README.md"})));
    let said = cli(&user, &["approve", &number.to_string(), "--always"]).unwrap();
    assert!(said.contains("catalogue"), "{said}");

    let body = settled(&user, number).body;
    assert!(
        body.starts_with(&format!(
            "Request #{number} (commands_run) was approved by the user, who added it to the \
             project's catalogue as cat-readme-md"
        )),
        "{body}"
    );
    assert!(body.contains("exited with code 0"), "{body}");
    let text = file(&user);
    assert!(text.starts_with("# The user's own words.\n"), "{text}");
    assert!(
        text.contains(
            "[commands.cat-readme-md]\nline = \"cat README.md\"\ncheck = true\nread_only = true\n"
        ),
        "{text}"
    );
    // From then on it runs there without asking, by its line or its name.
    let ran = done(mcp.call("commands_run", json!({"line": "cat README.md"})));
    assert!(ran.contains("app"), "{ran}");
    done(mcp.call("commands_run", json!({"command": "cat-readme-md"})));
}

#[test]
fn a_never_command_or_shell_syntax_in_main_is_still_refused() {
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

    let never = refused(mcp.call("commands_run", json!({"line": "deploy now"})));
    assert!(never.contains("never"), "{never}");
    // Refused in the words a workspace's run is refused in.
    let shell = mcp.call("commands_run", json!({"line": "cat README.md | wc -l"}));
    assert!(shell.error, "{}", shell.text);
    assert!(shell.text.contains("is shell syntax"), "{}", shell.text);
    let push = refused(mcp.call("commands_run", json!({"line": "git push origin main"})));
    assert!(push.starts_with("refused: "), "{push}");
    assert!(
        user.call("approval.list", json!({})).unwrap()["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
