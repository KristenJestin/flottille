//! The scenarios of the project's skills (`docs/skills.md`, `docs/tools.md`): listed in an agent's
//! instructions without their content, read with `skill_read` through the gate, and managed with
//! `flottille skill`. Throwaway repositories and temporary XDG folders; no real
//! agent ever runs here.

mod common;

use std::path::PathBuf;

use common::User;
use common::mcp::{Answer, Mcp, server_of};
use serde_json::{Value, json};

const REACT_GUIDE: &str = "Use hooks, never classes.\nKeep components small.\n";
const TOKEN: &str = "ghp_fakefakefakefakefakefakefake0007";

/// A project `app` over a throwaway repository, with a one-file skill `style` and a folder skill
/// `react` in its folder, and a secret file next to them that no skill holds.
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

    std::fs::create_dir_all(folder.join("docs/react/deep")).unwrap();
    std::fs::write(folder.join("docs/react/hooks.md"), REACT_GUIDE).unwrap();
    std::fs::write(
        folder.join("docs/react/deep/keys.md"),
        format!("Keys. Token: {TOKEN}\n"),
    )
    .unwrap();
    std::fs::write(folder.join("docs/STYLE.md"), "Four spaces.\n").unwrap();
    std::fs::write(folder.join("secret.txt"), "outside every skill\n").unwrap();
    folder
}

fn skill(user: &User, name: &str, description: &str, path: &str) {
    let output = user.run(&[
        "skill",
        "add",
        "app",
        name,
        "--path",
        path,
        "--description",
        description,
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn skills(user: &User) {
    skill(user, "react", "How we write React components", "docs/react");
    skill(user, "style", "The code style", "docs/STYLE.md");
}

/// A session of an agent working in the workspace `w`, and the agent's id.
fn session(user: &User) -> (Mcp, String) {
    user.call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "hello"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (Mcp::initialize(&url, &authorization), id)
}

fn ok(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    answer.text
}

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not an error: {}", answer.text);
    assert!(answer.text.starts_with("refused: "), "{}", answer.text);
    answer.text
}

fn stdout(user: &User, args: &[&str]) -> String {
    let output = user.run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn failure(user: &User, args: &[&str]) -> String {
    let output = user.run(args);
    assert!(!output.status.success(), "it should have failed");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_project_lists_adds_and_removes_its_skills() {
    let user = User::with_fake_agent();
    project(&user);

    assert!(stdout(&user, &["skill", "list", "app"]).contains("no skills"));
    skills(&user);

    let listed = stdout(&user, &["skill", "list", "app"]);
    assert!(
        listed.contains("react") && listed.contains("How we write React components"),
        "{listed}"
    );
    assert!(
        listed.contains("docs/react") && listed.contains("style"),
        "{listed}"
    );
    let json: Value =
        serde_json::from_str(&stdout(&user, &["skill", "list", "app", "--json"])).unwrap();
    assert_eq!(json["skills"][0]["name"], "react");
    assert_eq!(json["skills"][1]["path"], "docs/STYLE.md");
    assert_eq!(
        user.call("project.show", json!({"name": "app"})).unwrap()["skills"]["style"]["description"],
        "The code style"
    );

    let removed = stdout(&user, &["skill", "remove", "app", "react"]);
    assert!(removed.contains("react"), "{removed}");
    let left = stdout(&user, &["skill", "list", "app"]);
    assert!(!left.contains("React") && left.contains("style"), "{left}");
}

#[test]
fn editing_the_skills_keeps_the_rest_of_the_file_as_the_user_wrote_it() {
    let user = User::with_fake_agent();
    let folder = project(&user);
    let file = user.project_file("app");
    let written = std::fs::read_to_string(&file).unwrap();
    let handwritten = format!("# my notes on this project\n{written}\n# footer\n");
    std::fs::write(&file, &handwritten).unwrap();

    skill(&user, "style", "The code style", "docs/STYLE.md");
    let after = std::fs::read_to_string(&file).unwrap();
    assert!(after.starts_with("# my notes on this project\n"), "{after}");
    assert!(after.contains("# footer"), "{after}");
    assert!(after.contains("[skills.style]"), "{after}");

    stdout(&user, &["skill", "remove", "app", "style"]);
    let removed = std::fs::read_to_string(&file).unwrap();
    assert_eq!(removed.trim_end(), handwritten.trim_end());
    assert!(
        folder.join("docs/STYLE.md").is_file(),
        "the folder is never touched"
    );
}

#[test]
fn a_skill_that_cannot_be_used_is_refused_and_nothing_is_written() {
    let user = User::with_fake_agent();
    project(&user);
    let file = user.project_file("app");
    let before = std::fs::read_to_string(&file).unwrap();
    let add = |name: &str, path: &str, description: &str| {
        failure(
            &user,
            &[
                "skill",
                "add",
                "app",
                name,
                "--path",
                path,
                "--description",
                description,
            ],
        )
    };

    assert!(add("a b", "docs/STYLE.md", "d").contains("name"));
    assert!(add("x", "docs/missing.md", "d").contains("does not exist"));
    assert!(add("x", "../outside.md", "d").contains("inside the project folder"));
    assert!(add("x", "/etc/hostname", "d").contains("inside the project folder"));
    assert!(add("x", "docs/STYLE.md", "").contains("description"));
    assert!(add("x", ".git/config", "d").contains(".git"));
    std::os::unix::fs::symlink("/etc", user.path("work/app/etc-link")).unwrap();
    assert!(add("x", "etc-link/hostname", "d").contains("outside the project folder"));
    assert!(
        failure(
            &user,
            &[
                "skill",
                "add",
                "nobody",
                "x",
                "--path",
                "docs/STYLE.md",
                "--description",
                "d"
            ]
        )
        .contains("no project named nobody")
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);

    skill(&user, "style", "The code style", "docs/STYLE.md");
    assert!(add("style", "docs/STYLE.md", "again").contains("already"));
    assert!(
        failure(&user, &["skill", "remove", "app", "nothing"]).contains("no skill named nothing")
    );
}

#[test]
fn the_instructions_list_the_names_and_descriptions_and_never_the_content() {
    let user = User::with_fake_agent();
    project(&user);
    skills(&user);

    let (_mcp, id) = session(&user);

    let agent = user.call("agent.show", json!({"agent": id})).unwrap();
    let instructions = agent["agent"]["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("- react: How we write React components")
            && instructions.contains("- style: The code style"),
        "{instructions}"
    );
    assert!(instructions.contains("skill_read"), "{instructions}");
    assert!(
        !instructions.contains("Use hooks") && !instructions.contains("Four spaces"),
        "the content stays behind the tool"
    );
    let (before, after) = instructions
        .split_once("\n\n__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__\n\n")
        .unwrap();
    assert!(
        !before.contains("react") && after.contains("react"),
        "the list is part of the session, after the cache boundary"
    );
}

#[test]
fn a_project_without_skills_adds_nothing_to_the_instructions() {
    let user = User::with_fake_agent();
    project(&user);

    let (_mcp, id) = session(&user);

    let agent = user.call("agent.show", json!({"agent": id})).unwrap();
    let instructions = agent["agent"]["instructions"].as_str().unwrap();
    assert!(
        !instructions.contains("The project's skills"),
        "{instructions}"
    );
}

#[test]
fn skill_read_returns_a_file_skill_read_only_and_masked() {
    let user = User::with_fake_agent();
    project(&user);
    skills(&user);
    let (mcp, _) = session(&user);

    let tools = mcp.tools();
    let tool = tools.iter().find(|t| t["name"] == "skill_read").unwrap();
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    assert_eq!(tool["_meta"]["anthropic/alwaysLoad"], true);

    let style = ok(mcp.call("skill_read", json!({"name": "style"})));
    assert!(style.contains("Four spaces."), "{style}");
    let hooks = ok(mcp.call("skill_read", json!({"name": "react", "path": "hooks.md"})));
    assert!(hooks.contains("Use hooks, never classes."), "{hooks}");
    let keys = ok(mcp.call(
        "skill_read",
        json!({"name": "react", "path": "deep/keys.md"}),
    ));
    assert!(keys.contains("[masked]") && !keys.contains(TOKEN), "{keys}");
}

#[test]
fn skill_read_lists_a_folder_skill_and_reads_a_file_of_it_by_page() {
    let user = User::with_fake_agent();
    let folder = project(&user);
    let long: String = (1..=3000).map(|n| format!("line number {n}\n")).collect();
    std::fs::write(folder.join("docs/react/long.md"), &long).unwrap();
    skills(&user);
    let (mcp, _) = session(&user);

    let listing = ok(mcp.call("skill_read", json!({"name": "react"})));
    for file in ["hooks.md", "deep/keys.md", "long.md"] {
        assert!(listing.contains(file), "{listing}");
    }
    assert!(!listing.contains("Use hooks"), "a listing holds no content");

    let page = ok(mcp.call("skill_read", json!({"name": "react", "path": "long.md"})));
    assert!(page.contains("line number 1\n"), "{page}");
    let next = page
        .lines()
        .find_map(|l| l.split("the next page starts at offset ").nth(1))
        .map(|rest| rest.trim_end_matches(')').parse::<u64>().unwrap())
        .unwrap_or_else(|| panic!("no next page in {page}"));
    let second = ok(mcp.call(
        "skill_read",
        json!({"name": "react", "path": "long.md", "range": {"offset": next}}),
    ));
    assert!(!second.contains("line number 1\n"), "{second}");
}

#[test]
fn skill_read_refuses_a_name_that_is_not_in_the_list() {
    let user = User::with_fake_agent();
    project(&user);
    skills(&user);
    let (mcp, _) = session(&user);

    let message = refused(mcp.call("skill_read", json!({"name": "secrets"})));

    assert!(
        message.contains("no skill named secrets") && message.contains("react, style"),
        "{message}"
    );
    let message = refused(mcp.call("skill_read", json!({"name": "../react"})));
    assert!(message.contains("no skill named"), "{message}");
}

#[test]
fn skill_read_refuses_a_path_outside_the_skill() {
    let user = User::with_fake_agent();
    let folder = project(&user);
    skills(&user);
    std::os::unix::fs::symlink(folder.join("secret.txt"), folder.join("docs/react/link.md"))
        .unwrap();
    std::os::unix::fs::symlink("/etc", folder.join("docs/react/etc")).unwrap();
    std::fs::create_dir_all(folder.join("docs/react/.git")).unwrap();
    std::fs::write(folder.join("docs/react/.git/config"), "[core]\n").unwrap();
    std::fs::write(folder.join("docs/react/.env"), "KEY=value\n").unwrap();
    let (mcp, _) = session(&user);

    let read = |path: &str| refused(mcp.call("skill_read", json!({"name": "react", "path": path})));

    for escape in [
        "../STYLE.md",
        "../../secret.txt",
        "deep/../../STYLE.md",
        "/etc/hostname",
        "~/.bashrc",
        "link.md",
        "etc/hostname",
        "etc",
    ] {
        let message = read(escape);
        assert!(message.contains("outside the skill"), "{escape}: {message}");
    }
    assert!(read(".git/config").contains(".git"));
    assert!(read(".env").contains("secrets"));
    let message = refused(mcp.call(
        "skill_read",
        json!({"name": "style", "path": "../secret.txt"}),
    ));
    assert!(message.contains("is a file"), "{message}");
}

#[test]
fn skill_read_refuses_what_fs_read_holds_for_the_user() {
    let user = User::with_fake_agent();
    let folder = project(&user);
    skill(&user, "all", "The whole project", ".");
    std::fs::write(folder.join(".npmrc"), "//registry/:_authToken=x\n").unwrap();
    std::fs::write(folder.join(".netrc"), "machine example.com\n").unwrap();
    std::fs::write(folder.join(".git-credentials"), "https://u:p@example.com\n").unwrap();
    std::fs::create_dir_all(folder.join(".ssh")).unwrap();
    std::fs::write(folder.join(".ssh/id_ed25519"), "key\n").unwrap();
    std::os::unix::fs::symlink(folder.join(".npmrc"), folder.join("docs/npmrc.md")).unwrap();
    let (mcp, _) = session(&user);

    for path in [
        ".npmrc",
        ".netrc",
        ".git-credentials",
        ".ssh/id_ed25519",
        ".ssh",
        "docs/npmrc.md",
    ] {
        let message = refused(mcp.call("skill_read", json!({"name": "all", "path": path})));
        assert!(
            message.contains("sensitive place") && message.contains("fs_read"),
            "{path}: {message}"
        );
    }
    let listed = ok(mcp.call("skill_read", json!({"name": "all"})));
    assert!(listed.contains("README.md"), "{listed}");
    for hidden in [".npmrc", ".netrc", ".git-credentials", ".ssh"] {
        assert!(!listed.contains(hidden), "{hidden}: {listed}");
    }
    // A skill that is the sensitive file itself is refused too.
    skill(&user, "npm", "The registry", ".npmrc");
    let message = refused(mcp.call("skill_read", json!({"name": "npm"})));
    assert!(message.contains("sensitive place"), "{message}");
}

#[test]
fn skill_read_reads_the_project_folder_from_a_workspace_and_follows_its_edits() {
    let user = User::with_fake_agent();
    let folder = project(&user);
    skills(&user);
    let (mcp, _) = session(&user);
    // Not committed, so the workspace does not hold it: the skill is the project's.
    std::fs::write(folder.join("docs/STYLE.md"), "Two spaces.\n").unwrap();

    let style = ok(mcp.call("skill_read", json!({"name": "style"})));

    assert!(style.contains("Two spaces."), "{style}");
    stdout(&user, &["skill", "remove", "app", "style"]);
    let message = refused(mcp.call("skill_read", json!({"name": "style"})));
    assert!(message.contains("no skill named style"), "{message}");
}

#[test]
fn skill_read_is_recorded_as_a_tool_served_with_the_gate_allowing_it() {
    let user = User::with_fake_agent();
    project(&user);
    skills(&user);
    let (mcp, id) = session(&user);
    ok(mcp.call("skill_read", json!({"name": "style"})));

    let events = user.call("agent.show", json!({"agent": id})).unwrap()["events"]
        .as_array()
        .unwrap()
        .clone();

    let served = events
        .iter()
        .find(|e| e["name"] == "agent.tool_served" && e["data"]["tool"] == "skill_read")
        .unwrap_or_else(|| panic!("no tool_served in {events:?}"));
    assert_eq!(served["data"]["gate"], "allow");
    assert_eq!(served["data"]["outcome"], "ok");
}

#[test]
fn a_text_skill_is_kept_in_flottille_s_data_folder_and_read_with_skill_read() {
    let user = User::with_fake_agent();
    project(&user);
    let file = user.project_file("app");

    stdout(
        &user,
        &[
            "skill",
            "add",
            "app",
            "review",
            "--text",
            "Read the diff twice.\n",
            "--description",
            "How we review",
        ],
    );
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("[skills.review]"), "{written}");
    assert!(!written.contains("Read the diff"), "{written}");
    let kept = user.path("data/flottille/skills/app/review.md");
    assert_eq!(
        std::fs::read_to_string(&kept).unwrap(),
        "Read the diff twice.\n"
    );
    let (mcp, _) = session(&user);
    let read = ok(mcp.call("skill_read", json!({"name": "review"})));
    assert!(read.contains("Read the diff twice."), "{read}");
    let shown = user
        .call("skill.show", json!({"project": "app", "name": "review"}))
        .unwrap();
    assert_eq!(shown["kind"], "text");
    assert_eq!(shown["text"], "Read the diff twice.\n");

    // Made a path, it leaves no text behind; removed, nothing of it stays.
    stdout(
        &user,
        &["skill", "edit", "app", "review", "--path", "docs/STYLE.md"],
    );
    assert!(!kept.exists());
    let read = ok(mcp.call("skill_read", json!({"name": "review"})));
    assert!(read.contains("Four spaces."), "{read}");
    stdout(
        &user,
        &["skill", "edit", "app", "review", "--text", "Again."],
    );
    assert!(kept.exists());
    stdout(&user, &["skill", "remove", "app", "review"]);
    assert!(!kept.exists());
    assert!(!std::fs::read_to_string(&file).unwrap().contains("review"));
}

#[test]
fn a_text_skill_whose_file_is_refused_keeps_its_former_text() {
    use std::os::unix::fs::PermissionsExt;
    let user = User::with_fake_agent();
    project(&user);
    stdout(
        &user,
        &[
            "skill",
            "add",
            "app",
            "review",
            "--text",
            "Read the diff twice.\n",
            "--description",
            "How we review",
        ],
    );
    // The project's file cannot be replaced: its folder is read-only.
    let projects = user.project_file("app").parent().unwrap().to_owned();
    std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o555)).unwrap();

    let refused = failure(
        &user,
        &["skill", "edit", "app", "review", "--text", "Never mind.\n"],
    );

    std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!refused.is_empty());
    assert_eq!(
        std::fs::read_to_string(user.path("data/flottille/skills/app/review.md")).unwrap(),
        "Read the diff twice.\n"
    );
}

#[test]
fn an_inline_skill_stays_inline_when_it_is_edited() {
    let user = User::with_fake_agent();
    project(&user);
    let file = user.project_file("app");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!(
            "{text}\n[skills]\nstyle = {{ description = \"Style\", path = \"docs/STYLE.md\" }}\n"
        ),
    )
    .unwrap();

    stdout(
        &user,
        &[
            "skill",
            "edit",
            "app",
            "style",
            "--description",
            "The code style",
        ],
    );

    let edited = std::fs::read_to_string(&file).unwrap();
    assert!(
        edited.contains("style = { description = \"The code style\", path = \"docs/STYLE.md\" }"),
        "{edited}"
    );
    assert!(!edited.contains("[skills.style]"), "{edited}");
}

#[test]
fn a_text_skill_whose_file_is_a_link_is_read_where_it_leads() {
    let user = User::with_fake_agent();
    project(&user);
    stdout(
        &user,
        &[
            "skill",
            "add",
            "app",
            "review",
            "--text",
            "Read the diff twice.\n",
            "--description",
            "How we review",
        ],
    );
    // The user keeps the text elsewhere and links it.
    let kept = user.path("data/flottille/skills/app/review.md");
    let elsewhere = user.path("notes/review.md");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    std::fs::write(&elsewhere, "Read it where it is kept.\n").unwrap();
    std::fs::remove_file(&kept).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &kept).unwrap();

    let (mcp, _) = session(&user);
    let read = ok(mcp.call("skill_read", json!({"name": "review"})));
    assert!(read.contains("Read it where it is kept."), "{read}");
}

#[test]
fn a_text_skill_whose_file_is_a_dangling_link_says_it_leads_nowhere() {
    let user = User::with_fake_agent();
    project(&user);
    stdout(
        &user,
        &[
            "skill",
            "add",
            "app",
            "review",
            "--text",
            "Read the diff twice.\n",
            "--description",
            "How we review",
        ],
    );
    let kept = user.path("data/flottille/skills/app/review.md");
    std::fs::remove_file(&kept).unwrap();
    std::os::unix::fs::symlink(user.path("notes/gone.md"), &kept).unwrap();

    let (mcp, _) = session(&user);
    let message = refused(mcp.call("skill_read", json!({"name": "review"})));
    assert!(
        message.contains("the skill's file is a link that leads nowhere"),
        "{message}"
    );
}
