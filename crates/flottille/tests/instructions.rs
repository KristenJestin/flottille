//! The scenarios of the user's own instructions for every agent (`docs/agents.md`, "Roles"):
//! `instructions.md` in Flottille's configuration folder, added to every agent's instructions
//! after the cache boundary and before the project's own files, read when the agent is created,
//! cut at the rules' limit, and shown and set from the command line. The agents are the fake
//! agent's sessions, on throwaway repositories and temporary XDG folders.

mod common;

use common::User;
use serde_json::{Value, json};

const BOUNDARY: &str = "\n\n__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__\n\n";
const HEADING: &str = "# The user's instructions, for every project";

/// A project `app` that is one repository holding an `AGENTS.md`.
fn project(user: &User) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    std::fs::write(folder.join("AGENTS.md"), "The project says: use tabs.\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

fn write_instructions(user: &User, text: &str) {
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(user.path("config/flottille/instructions.md"), text).unwrap();
}

/// The instructions of a root agent started now, whole.
fn started(user: &User) -> String {
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap();
    let shown = user.call("agent.show", json!({"agent": id})).unwrap();
    shown["agent"]["instructions"].as_str().unwrap().to_owned()
}

#[test]
fn the_user_s_instructions_come_before_the_project_s_files() {
    let user = User::with_fake_agent();
    project(&user);
    write_instructions(&user, "Name branches after the issue.\n");

    let text = started(&user);

    let (before, after) = text.split_once(BOUNDARY).expect("the cache boundary");
    // After the boundary, so that the shared part stays the same for every user's cache.
    assert!(!before.contains("Name branches"), "{before}");
    let at = |needle: &str| {
        after
            .find(needle)
            .unwrap_or_else(|| panic!("no {needle}: {after}"))
    };
    assert!(at(HEADING) < at("Name branches after the issue."));
    assert!(at("Name branches after the issue.") < at("# The project's own instructions"));
    assert!(at("# The project's own instructions") < at("The project says: use tabs."));
}

#[test]
fn an_empty_or_missing_instructions_file_adds_nothing() {
    let user = User::with_fake_agent();
    project(&user);

    let missing = started(&user);
    assert!(!missing.contains("The user's instructions"), "{missing}");

    write_instructions(&user, "  \n\n");
    let empty = started(&user);
    assert!(!empty.contains("The user's instructions"), "{empty}");

    let shown = user.call("instructions.show", json!({})).unwrap();
    assert_eq!(shown["text"], "  \n\n");
    assert!(shown["cut"].is_null(), "{shown}");
}

#[test]
fn a_change_to_the_instructions_applies_to_agents_started_after() {
    let user = User::with_fake_agent();
    project(&user);
    write_instructions(&user, "Answer in French.");
    let earlier = started(&user);

    let shown = user
        .call("instructions.set", json!({"text": "Answer in Spanish.\n"}))
        .unwrap();
    assert_eq!(shown["text"], "Answer in Spanish.\n");
    let later = started(&user);

    assert!(earlier.contains("Answer in French.") && !earlier.contains("Spanish"));
    assert!(later.contains("Answer in Spanish.") && !later.contains("French"));
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/instructions.md")).unwrap(),
        "Answer in Spanish.\n"
    );
    // A text longer than the limit is refused and the file stays as it was.
    let error = user
        .call("instructions.set", json!({"text": "x".repeat(8_001)}))
        .unwrap_err();
    assert!(error.1.contains("8000"), "{error:?}");
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/instructions.md")).unwrap(),
        "Answer in Spanish.\n"
    );
    // An empty text removes the file: nothing is added again.
    user.call("instructions.set", json!({"text": ""})).unwrap();
    assert!(!user.path("config/flottille/instructions.md").exists());
    assert!(!started(&user).contains("The user's instructions"));
}

#[test]
fn an_oversized_instructions_file_is_cut_with_a_note() {
    let user = User::with_fake_agent();
    project(&user);
    let long = format!("{}TAIL", "x".repeat(8_000));
    write_instructions(&user, &long);

    let text = started(&user);

    assert!(
        text.contains(&"x".repeat(8_000)),
        "the first 8000 are given"
    );
    assert!(!text.contains("TAIL"), "past the limit is cut");
    let shown = user.call("instructions.show", json!({})).unwrap();
    let cut = shown["cut"].as_str().unwrap_or_else(|| panic!("{shown}"));
    assert!(
        cut.contains("8004 characters") && cut.contains("8000") && cut.contains("instructions.md"),
        "{cut}"
    );
    // The file is the user's: it is read, never rewritten.
    assert_eq!(
        std::fs::read_to_string(user.path("config/flottille/instructions.md")).unwrap(),
        long
    );
}

fn json_of(user: &User, args: &[&str]) -> Value {
    let output = user.run(args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
}

#[test]
fn config_instructions_shows_and_sets_the_file() {
    let user = User::with_fake_agent();
    let file = user.path("config/flottille/instructions.md");

    // Nothing yet: the file is said, and empty.
    let shown = json_of(&user, &["config", "instructions", "--json"]);
    assert_eq!(shown["text"], "");
    assert_eq!(shown["path"], file.to_str().unwrap());
    let plain = user.run(&["config", "instructions", "--show"]);
    assert!(plain.status.success());
    assert!(String::from_utf8_lossy(&plain.stdout).contains("instructions.md"));

    // --file reads a text and keeps it.
    let source = user.path("mine.md");
    std::fs::write(&source, "Commit small.\n").unwrap();
    let set = json_of(
        &user,
        &[
            "config",
            "instructions",
            "--file",
            source.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(set["text"], "Commit small.\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Commit small.\n");
    assert_eq!(
        json_of(&user, &["config", "instructions", "--show", "--json"])["text"],
        "Commit small.\n"
    );

    // --edit opens the user's editor, without a shell, on a copy; what it leaves is kept.
    let editor = user.path("editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nprintf 'Answer briefly.\\n' >> \"$1\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut edit = user.command(&["config", "instructions", "--edit", "--json"]);
    edit.env("EDITOR", &editor).env_remove("VISUAL");
    let output = edit.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "Commit small.\nAnswer briefly.\n"
    );

    // Refused when too long, the file untouched; a missing file is an error too.
    let long = user.path("long.md");
    std::fs::write(&long, "x".repeat(8_001)).unwrap();
    let refused = user.run(&["config", "instructions", "--file", long.to_str().unwrap()]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("8000"));
    let absent = user.run(&[
        "config",
        "instructions",
        "--file",
        "/nonexistent/instructions.md",
    ]);
    assert!(!absent.status.success());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "Commit small.\nAnswer briefly.\n"
    );
}
