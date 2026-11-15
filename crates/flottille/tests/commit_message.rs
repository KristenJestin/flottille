//! The scenarios of `tools/commit-message.sh` (AGENTS.md, "Git and pull requests"): the judge of
//! the commit-msg hook and of CI. It runs on throwaway message files; no repository of the user.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Whether the script accepts that message, and what it said when it did not.
fn judged(message: &str) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("COMMIT_EDITMSG");
    std::fs::write(&file, format!("{message}\n")).unwrap();
    let output = Command::new(root().join("tools/commit-message.sh"))
        .arg(&file)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn a_scope_nobody_listed_is_refused() {
    let (accepted, said) = judged("feat(nope): x");

    assert!(!accepted);
    // The refusal says which scopes there are.
    assert!(said.contains("scopes: ") && said.contains("link"), "{said}");
}

#[test]
fn the_scopes_of_the_remote_milestone_are_accepted() {
    for scope in ["link", "hub", "mobile", "android", "site", "protocol"] {
        let message = format!("feat({scope}): x");
        assert!(judged(&message).0, "{message}");
    }
}

#[test]
fn every_scope_agents_md_lists_is_accepted() {
    // The list is read from AGENTS.md, so that no second one can disagree: the scopes found
    // there are the ones the script takes, the old ones among them.
    let agents = std::fs::read_to_string(root().join("AGENTS.md")).unwrap();
    let mut lines = agents.lines().skip_while(|l| !l.starts_with("- Scopes:"));
    let first = lines.next().expect("a Scopes line");
    let listed: Vec<&str> = std::iter::once(first)
        .chain(lines.take_while(|l| l.starts_with("  ")))
        .flat_map(|l| l.split('`').skip(1).step_by(2))
        .collect();
    assert!(listed.len() >= 21, "{listed:?}");
    for scope in ["core", "gui", "ci", "repo", "tools"] {
        assert!(listed.contains(&scope), "{scope}");
    }
    for scope in listed {
        let message = format!("fix({scope}): x");
        assert!(judged(&message).0, "{message}");
    }
}

#[test]
fn the_rest_of_the_convention_still_holds() {
    for refused in [
        "feat: no scope",
        "feat(gui): Starts upper",
        "feat(gui): ends with a period.",
        "wip(gui): not a type",
        &format!("feat(gui): {}", "a".repeat(70)),
    ] {
        assert!(!judged(refused).0, "{refused}");
    }
    for accepted in [
        "fix(gui)!: breaking",
        "Merge branch 'dev'",
        "fixup! fix(gui): a",
    ] {
        assert!(judged(accepted).0, "{accepted}");
    }
}
