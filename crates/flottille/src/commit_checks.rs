//! A repository's commit checks (`docs/projects.md`, "Commit checks"): the command lines the user
//! saves for a repository, run before an agent's `git commit` in it as its `pre-commit` and
//! `commit-msg` hooks would run for the user. Git still runs no hook for an agent
//! (`runs::defend`): a hook is a file the agent can write. A check is the user's own line, run by
//! the daemon without a shell, in the place's run environment, never read from the repository's
//! hooks; those are only read to propose lines (`proposals`), which nothing runs until the user
//! saved them.

use std::path::{Path, PathBuf};

use flottille_core::project::{MESSAGE_FILE, check_commit_check};
use flottille_core::secrets;
use serde::Serialize;

use crate::gate;
use crate::message_checks::{Check, MessageFile, Where, commit_of, run_check, unaliased};
use crate::place::Place;

const NOTHING: &str = "nothing was committed";

/// Runs the commit checks of the repository each `git commit` of the line `words` commits in
/// (the user's alias of one too): the refusal of the first that fails, of a commit that skips
/// them (`--no-verify`), or of one whose message its `commit-msg` checks cannot know. Nothing
/// when the commit's repository declares none.
pub async fn commits(at: &Where<'_>, words: &[String]) -> Result<(), String> {
    let gits = gate::gits(words);
    if gits.is_empty() {
        return Ok(());
    }
    let project =
        crate::projects::load_project(at.daemon, &at.place.project).map_err(|failure| {
            format!(
                "refused: the project's file cannot be read, so its commit checks cannot run ({}); \
             {NOTHING}",
                failure.message
            )
        })?;
    if project
        .repositories
        .iter()
        .all(|repository| repository.commit_checks.is_empty())
    {
        return Ok(());
    }
    let unknown = |why: String| {
        format!(
            "refused: the repositories' commit checks read each commit before it is made, and \
             {why}; {NOTHING}"
        )
    };
    let env =
        crate::runs::environment(at.daemon, at.place, at.repository, false).map_err(unknown)?;
    for git in gits {
        let git = unaliased(git, at.folder, &env).await.map_err(unknown)?;
        let Some(commit) = commit_of(&git, at.folder).map_err(unknown)? else {
            continue;
        };
        let Some((path, folder)) = repository_of(at.place, &commit.folder) else {
            continue;
        };
        let Some(checks) = project
            .repositories
            .iter()
            .find(|repository| repository.path == path)
            .map(|repository| &repository.commit_checks)
            .filter(|checks| !checks.is_empty())
        else {
            continue;
        };
        let of = format!(" of the repository {path}");
        if commit.no_verify {
            return Err(format!(
                "refused: --no-verify skips the commit checks{of}, which are the user's, not the \
                 agent's to skip: commit without it; {NOTHING}"
            ));
        }
        let at = Where {
            daemon: at.daemon,
            place: at.place,
            repository: path,
            folder,
        };
        for line in &checks.pre_commit {
            let check = Check {
                named: "the pre-commit check",
                of: of.clone(),
                what: "the commit",
                nothing: NOTHING,
                again: "fix what it says and try again",
                fill: None,
            };
            run_check(&at, line, &check).await?;
        }
        if checks.commit_msg.is_empty() {
            continue;
        }
        let message = commit.message.map_err(|why| {
            format!(
                "refused: the commit-msg checks{of} read the commit's message before the commit, \
                 and {why}; {NOTHING}"
            )
        })?;
        let file = MessageFile::write(at.daemon, &message).map_err(|why| {
            format!("refused: the message could not be written for the checks ({why}); {NOTHING}")
        })?;
        for line in &checks.commit_msg {
            let check = Check {
                named: "the commit-msg check",
                of: of.clone(),
                what: "the commit message",
                nothing: NOTHING,
                again: "change it and try again",
                fill: Some((MESSAGE_FILE, &file.0)),
            };
            run_check(&at, line, &check).await?;
        }
    }
    Ok(())
}

/// The repository of the place a folder is in: the deepest whose folder holds it.
fn repository_of<'a>(place: &'a Place, folder: &Path) -> Option<(&'a str, &'a Path)> {
    place
        .repositories
        .iter()
        .filter(|(_, root)| folder.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(path, root)| (path.as_str(), root.as_path()))
}

/// The hooks a commit check stands for.
pub const HOOKS: [&str; 2] = ["pre-commit", "commit-msg"];

/// A line read from a repository's hooks, for the user to save as one of its checks.
#[derive(Debug, PartialEq, Serialize)]
pub struct Proposal {
    /// `pre-commit` or `commit-msg`.
    pub hook: &'static str,
    pub line: String,
    /// The file it was read from, relative to the repository.
    pub from: String,
}

/// A command of a hook file that cannot be a check, and why: shell syntax, mostly.
#[derive(Debug, PartialEq, Serialize)]
pub struct LeftOut {
    pub hook: &'static str,
    pub line: String,
    pub from: String,
    pub why: String,
}

/// What a repository's hooks propose (`folder`, its checkout): for each hook, the entry point of
/// its hook manager when it has one (`lefthook`, `pre-commit`), so that the whole hook runs as the
/// project wrote it; else its hook file (`.husky`, `.githooks`, `.git/hooks`, not a sample), run
/// itself when it is a program of the repository (a `#!` line), or the commands read in it, those
/// with shell syntax left out and said. Nothing is run; every line is masked.
pub fn proposals(folder: &Path) -> (Vec<Proposal>, Vec<LeftOut>) {
    let (mut proposed, mut left) = (Vec::new(), Vec::new());
    for hook in HOOKS {
        let message = hook == "commit-msg";
        if let Some((line, from)) = manager(folder, hook) {
            proposed.push(Proposal { hook, line, from });
            continue;
        }
        let Some((from, text)) = hook_file(folder, hook) else {
            continue;
        };
        // A program of the repository's own, in its checkout: run as it is.
        let tracked = !from.starts_with(".git/");
        if tracked && text.starts_with("#!") && executable(&folder.join(&from)) {
            let mut line = format!("./{from}");
            if message {
                line.push(' ');
                line.push_str(MESSAGE_FILE);
            }
            proposed.push(Proposal { hook, line, from });
            continue;
        }
        for command in commands(&text) {
            let line = match message {
                true => message_argument(&command),
                false => command.clone(),
            };
            match check_commit_check(&line, message) {
                Ok(()) => proposed.push(Proposal {
                    hook,
                    line: masked(&line),
                    from: from.clone(),
                }),
                Err(why) => left.push(LeftOut {
                    hook,
                    line: masked(&command),
                    from: from.clone(),
                    why: masked(&why),
                }),
            }
        }
    }
    (proposed, left)
}

fn masked(text: &str) -> String {
    secrets::mask(text, &std::env::vars_os().collect())
}

/// The entry point of the hook manager the repository configures for `hook`, and its file.
fn manager(folder: &Path, hook: &str) -> Option<(String, String)> {
    let message = hook == "commit-msg";
    for file in [
        "lefthook.yml",
        "lefthook.yaml",
        ".lefthook.yml",
        ".lefthook.yaml",
    ] {
        let Ok(text) = std::fs::read_to_string(folder.join(file)) else {
            continue;
        };
        // A hook of lefthook's is a key at the top of its file.
        if text
            .lines()
            .any(|line| line.trim_end() == format!("{hook}:"))
        {
            let line = match message {
                true => format!("lefthook run {hook} {MESSAGE_FILE}"),
                false => format!("lefthook run {hook}"),
            };
            return Some((line, file.to_owned()));
        }
    }
    let file = ".pre-commit-config.yaml";
    let text = std::fs::read_to_string(folder.join(file)).ok()?;
    // Its hooks run at commit unless they say another stage; one at commit-msg names it.
    match message {
        true => text.contains("commit-msg").then(|| {
            format!("pre-commit run --hook-stage commit-msg --commit-msg-filename {MESSAGE_FILE}")
        }),
        false => Some("pre-commit run --hook-stage pre-commit".to_owned()),
    }
    .map(|line| (line, file.to_owned()))
}

/// The first hook file of `hook` the repository has, relative to it, and its text.
fn hook_file(folder: &Path, hook: &str) -> Option<(String, String)> {
    [".husky", ".githooks", ".git/hooks"]
        .iter()
        .map(|dir| format!("{dir}/{hook}"))
        .find_map(|from| {
            let path: PathBuf = folder.join(&from);
            // A link may lead out of the repository; a hook of git's own is a plain file.
            let plain = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file());
            plain
                .then(|| std::fs::read_to_string(&path).ok())
                .flatten()
                .map(|text| (from, text))
        })
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

/// The commands of a hook file: its lines, comments, blank lines and husky's own loading line
/// left out.
fn commands(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.contains("husky.sh"))
        .map(str::to_owned)
        .collect()
}

/// A `commit-msg` hook's command with its argument, the message's file (`$1`), as
/// `{message_file}`.
fn message_argument(command: &str) -> String {
    ["\"$1\"", "'$1'", "\"${1}\"", "${1}", "$1"]
        .iter()
        .fold(command.to_owned(), |line, argument| {
            line.replace(argument, MESSAGE_FILE)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hook_s_message_argument_is_the_message_file() {
        assert_eq!(
            message_argument("npx --no -- commitlint --edit \"$1\""),
            "npx --no -- commitlint --edit {message_file}"
        );
        assert_eq!(
            message_argument("check ${1} $1"),
            "check {message_file} {message_file}"
        );
    }

    #[test]
    fn a_hook_file_s_commands_leave_its_comments_out() {
        let text = "#!/bin/sh\n. \"$(dirname -- \"$0\")/_/husky.sh\"\n\n# lint\nlint-staged\n";
        assert_eq!(commands(text), ["lint-staged"]);
    }
}
