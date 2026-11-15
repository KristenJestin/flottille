//! A repository's commit checks (`docs/projects.md`, "Commit checks"): the command lines the user
//! saves for a repository, run before an agent's `git commit` in it as its `pre-commit` and
//! `commit-msg` hooks would run for the user. Git still runs no hook for an agent
//! (`runs::defend`): a hook is a file the agent can write. A check is the user's own line, run by
//! the daemon without a shell, in the place's run environment, never read from the repository's
//! hooks; those are only read to propose lines (`proposals`), which nothing runs until the user
//! saved them.

use std::path::Path;
use std::time::Duration;

use flottille_core::project::{MESSAGE_FILE, check_commit_check};
use flottille_core::secrets;
use serde::Serialize;

use crate::gate;
use crate::message_checks::{Check, MessageFile, Where, commit_of, run_check, unaliased};
use crate::place::Place;

const NOTHING: &str = "nothing was committed";
/// What a commit check may take before it is stopped, and counted as not run: longer than a
/// message check, as the first `pre-commit run` installs the environments of its hooks.
pub const TIME_LIMIT: Duration = Duration::from_secs(300);

/// Runs the commit checks of the repository each `git commit` of the line `words` commits in
/// (the user's alias of one too): the refusal of the first that fails, of a commit that skips
/// them (`--no-verify`), or of one whose message its `commit-msg` checks cannot know. Nothing
/// when the commit's repository declares none.
pub async fn commits(at: &Where<'_>, words: &[String]) -> Result<(), String> {
    let gits = gate::gits(at.place, at.folder, words);
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
        let words = unaliased(git.words, &git.folder, &env)
            .await
            .map_err(unknown)?;
        let Some(commit) = commit_of(&words, &git.folder).map_err(unknown)? else {
            continue;
        };
        // Where git commits is then another repository than its folder's, whose checks it skips.
        if let Some(pointer) = commit.elsewhere.map(str::to_owned).or(git.elsewhere) {
            return Err(format!(
                "refused: {pointer} points the commit at a repository other than its folder's, \
                 whose commit checks would not run, and the project's repositories have commit \
                 checks: commit in the repository's folder, with git -C <folder> if it is \
                 another one; {NOTHING}"
            ));
        }
        let Some((path, folder)) = repository_of(at.place, &git.folder) else {
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
                limit: TIME_LIMIT,
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
                limit: TIME_LIMIT,
            };
            run_check(&at, line, &check).await?;
        }
    }
    Ok(())
}

/// The repository of the place a folder (real, `gate::gits`) is in: the deepest whose folder
/// holds it.
fn repository_of<'a>(place: &'a Place, folder: &Path) -> Option<(&'a str, &'a Path)> {
    place
        .repositories
        .iter()
        .filter(|(_, root)| {
            folder.starts_with(root)
                || crate::place::real_path(root).is_ok_and(|real| folder.starts_with(real))
        })
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
    let masked = masker(folder);
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

/// Masks a text as other shown text is: the daemon's variables, the values of the repository's
/// files of secrets (`.env`), the vault's.
fn masker(folder: &Path) -> impl Fn(&str) -> String {
    let mut values = Vec::new();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        if entry.file_name().to_str().is_some_and(secrets::secret_file)
            && let Some(text) = small_file(&entry.path())
        {
            values.extend(secrets::file_values(&text));
        }
    }
    let known = secrets::Secrets::new(&std::env::vars_os().collect(), &values);
    move |text| known.mask(text)
}

/// What a hook file or a hook manager's file may hold to be read.
const FILE_BYTES: u64 = 256 * 1024;

/// The text of a regular file of `FILE_BYTES` at most: not a link, which may lead out of the
/// repository, nor a FIFO, whose read would wait for a writer.
fn small_file(path: &Path) -> Option<String> {
    std::fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.len() <= FILE_BYTES)
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
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
        let Some(text) = small_file(&folder.join(file)) else {
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
    let text = small_file(&folder.join(file))?;
    // Its hooks run at commit unless they say another stage; one at commit-msg names it.
    match message {
        true => names_commit_msg(&text).then(|| {
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
            // A link may lead out of the repository; a hook of git's own is a plain file.
            small_file(&folder.join(&from)).map(|text| (from, text))
        })
}

/// Whether a `.pre-commit-config.yaml` runs a hook at `commit-msg`: one of its `stages`,
/// `default_stages` or `default_install_hook_types` lists it, inline (`[commit-msg]`) or as the
/// items below the key.
fn names_commit_msg(text: &str) -> bool {
    const KEYS: [&str; 3] = ["stages", "default_stages", "default_install_hook_types"];
    let item = |word: &str| word.trim().trim_matches(['"', '\'']) == "commit-msg";
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.split('#').next().unwrap_or_default();
        let indent = line.len() - line.trim_start().len();
        let entry = line.trim_start().trim_start_matches("- ");
        let Some((key, value)) = entry.split_once(':') else {
            continue;
        };
        if !KEYS.contains(&key.trim()) {
            continue;
        }
        let value = value.trim();
        if !value.is_empty() {
            if value.trim_matches(['[', ']']).split(',').any(item) {
                return true;
            }
            continue;
        }
        // A list below the key: its items, indented more than it or as much.
        while let Some(next) = lines.peek() {
            let next = next.split('#').next().unwrap_or_default();
            let deeper = next.len() - next.trim_start().len() >= indent;
            match next.trim_start().strip_prefix("- ") {
                Some(word) if deeper => {
                    if item(word) {
                        return true;
                    }
                    lines.next();
                }
                _ => break,
            }
        }
    }
    false
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

    /// A throwaway repository folder holding `files`.
    fn folder_of(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("flottille-{name}-{}", std::process::id()));
        for (file, text) in files {
            let path = folder.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        folder
    }

    fn hooks(proposed: &[Proposal]) -> Vec<&str> {
        proposed.iter().map(|proposal| proposal.hook).collect()
    }

    #[test]
    fn a_pre_commit_config_s_commit_msg_is_read_from_its_stages() {
        let config = |name: &str, text: &str| {
            let folder = folder_of(name, &[(".pre-commit-config.yaml", text)]);
            let (proposed, _) = proposals(&folder);
            std::fs::remove_dir_all(folder).unwrap();
            hooks(&proposed).contains(&"commit-msg")
        };
        // The word alone, in a comment or a hook's id, is no stage.
        assert!(!config(
            "no-stage",
            "# no commit-msg hook here\nrepos:\n- repo: local\n  hooks:\n  - id: no-commit-msg\n    \
             entry: lint\n    language: system\n"
        ));
        for (name, text) in [
            (
                "inline",
                "repos:\n- repo: local\n  hooks:\n  - id: message\n    entry: lint\n    \
                 stages: [pre-push, \"commit-msg\"]\n",
            ),
            (
                "block",
                "repos:\n- repo: local\n  hooks:\n  - id: message\n    stages:\n    - \
                 commit-msg\n    entry: lint\n",
            ),
            (
                "installed",
                "default_install_hook_types: [pre-commit, commit-msg]\nrepos: []\n",
            ),
        ] {
            assert!(config(name, text), "{name}");
        }
    }

    #[test]
    fn a_proposal_is_masked_with_the_vault_s_values_and_the_repository_s_secrets() {
        secrets::add_vault_value("vaulted-orchard-77");
        let folder = folder_of(
            "masked",
            &[
                (".env", "DEPLOY=riverstone-lane-42\n"),
                (
                    ".githooks/pre-commit",
                    "lint --with vaulted-orchard-77 --and riverstone-lane-42\n",
                ),
            ],
        );
        let (proposed, _) = proposals(&folder);
        std::fs::remove_dir_all(folder).unwrap();
        assert_eq!(proposed.len(), 1, "{proposed:?}");
        assert_eq!(
            proposed[0].line,
            format!("lint --with {0} --and {0}", secrets::MASK)
        );
    }

    #[test]
    fn a_hook_file_s_commands_leave_its_comments_out() {
        let text = "#!/bin/sh\n. \"$(dirname -- \"$0\")/_/husky.sh\"\n\n# lint\nlint-staged\n";
        assert_eq!(commands(text), ["lint-staged"]);
    }
}
