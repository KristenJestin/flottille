//! The project's message checks (`docs/projects.md`, "Message checks"): the command lines the
//! user declares, each run on a file holding a message before an agent's commit, a pull
//! request's title or a squash message is made. They are the user's configuration, never the
//! repository's: git runs no hook for an agent (`runs::defend`), and a check is run by the
//! daemon, without a shell, in the place's run environment, never as a call of the agent.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use flottille_core::agent::new_id;
use flottille_core::command::split_line;
use flottille_core::project::MESSAGE;
use flottille_core::{ansi, secrets};

use crate::daemon::Daemon;
use crate::gate;
use crate::place::Place;

/// What a check may take before it is stopped, and counted as not run.
pub const TIME_LIMIT: Duration = Duration::from_secs(60);
/// The end of a check's output a refusal holds: a refusal is never cut as a result is, and stays
/// under 2,000 characters.
const OUTPUT_CHARS: usize = 1_200;
/// What a message file given with `git commit -F` may hold.
const FILE_BYTES: u64 = 64 * 1024;

/// What a message is, for the refusal's words.
#[derive(Clone, Copy)]
pub enum Checked {
    Commit,
    Title,
    Squash,
    /// A commit a push sends.
    Pushed,
}

impl Checked {
    fn what(self) -> &'static str {
        match self {
            Checked::Commit => "the commit message",
            Checked::Title => "the pull request's title",
            Checked::Squash => "the squash message",
            Checked::Pushed => "its message",
        }
    }

    fn nothing(self) -> &'static str {
        match self {
            Checked::Commit => "nothing was committed",
            Checked::Title => "nothing was written to the forge",
            Checked::Squash => "nothing was merged",
            Checked::Pushed => "nothing was pushed",
        }
    }
}

/// Where the checks run: the place, the repository whose folder they run in and that folder.
pub struct Where<'a> {
    pub daemon: &'a Daemon,
    pub place: &'a Place,
    pub repository: &'a str,
    pub folder: &'a Path,
}

/// Runs the project's checks on each commit the line `words` makes in `folder` (`git commit`,
/// `git merge`, `git commit-tree`, the user's alias of one): the refusal of the first that
/// fails, or of a commit whose message cannot be known before it is made. Nothing when the
/// project declares none.
pub async fn commits(at: &Where<'_>, words: &[String]) -> Result<(), String> {
    let gits = gate::gits(words);
    if gits.is_empty() {
        return Ok(());
    }
    let checks = declared(at.daemon, at.place, Checked::Commit)?;
    if checks.is_empty() {
        return Ok(());
    }
    let unknown = |why: String| {
        format!(
            "refused: the project's message checks read the commit message before the commit, \
             and {why}; {}",
            Checked::Commit.nothing()
        )
    };
    let env =
        crate::runs::environment(at.daemon, at.place, at.repository, false).map_err(unknown)?;
    for git in gits {
        let git = unaliased(git, at.folder, &env).await.map_err(unknown)?;
        if let Some(message) = message_of(&git, at.folder).map_err(unknown)? {
            run_all(at, &checks, Checked::Commit, &message).await?;
        }
    }
    Ok(())
}

/// Aliases followed at most, as git stops a loop of them.
const ALIASES_MAX: usize = 10;

/// A git command's words with the user's alias of its command expanded, as git expands it: an
/// alias of `ci` to `commit` makes `git ci -m x` a commit. A command git has, built in or a
/// `git-<command>` it finds, is never an alias; one that is neither is refused, since git may
/// run another in its place (`help.autocorrect`), which would commit unread.
async fn unaliased(mut git: Vec<String>, folder: &Path, env: &Env) -> Result<Vec<String>, String> {
    for _ in 0..ALIASES_MAX {
        let args = &git[1..];
        let (at, globals) = gate::git_globals(args);
        let Some(command) = args.get(at).cloned() else {
            return Ok(git);
        };
        if ["commit", "merge", "commit-tree", "revert"].contains(&command.as_str()) {
            return Ok(git);
        }
        // Where git would look for it: the line's folder and its own aliases.
        let mut dir = folder.to_path_buf();
        let mut asked: Vec<String> = Vec::new();
        for (option, value) in globals {
            let alias = value.is_some_and(|v| v.to_ascii_lowercase().starts_with("alias."));
            match (option, value) {
                ("-C", Some(value)) => dir = dir.join(value),
                ("-c", Some(value)) if alias => asked.extend(["-c".into(), value.into()]),
                ("--config-env", _) if alias => {
                    return Err(format!(
                        "git {command} is an alias set from the environment, whose commits \
                         cannot be read: commit with git commit -m"
                    ));
                }
                _ => {}
            }
        }
        asked.extend(["config".into(), "--get".into(), format!("alias.{command}")]);
        let unread = |why: String| format!("the alias git {command} could not be read ({why})");
        let found = crate::git::run(&dir, &asked, crate::git::Class::Read)
            .await
            .map_err(unread)?;
        // Where the run looks for a `git-<command>`: the `PATH` of its environment.
        let listed = ["git", "--list-cmds=builtins,main,others"];
        let commands = crate::git::run_in(&dir, &listed, env, None)
            .await
            .map_err(unread)?;
        if !commands.success {
            return Err(unread(commands.stderr.trim().to_owned()));
        }
        if commands.stdout.lines().any(|known| known == command) {
            return Ok(git);
        }
        if !found.success {
            return Err(format!(
                "git {command} is neither a git command, an alias nor a git-{command} on the \
                 PATH, and git may run another command in its place (help.autocorrect): name \
                 the command git runs"
            ));
        }
        let alias = found.stdout.trim_end_matches('\n');
        if alias.starts_with('!') {
            return Err(format!(
                "git {command} is the user's alias of a shell command, whose commits cannot be \
                 read: commit with git commit -m"
            ));
        }
        let expanded = split_line(alias).map_err(unread)?;
        let mut words = git[..=at].to_vec();
        words.extend(expanded);
        words.extend_from_slice(&git[at + 2..]);
        git = words;
    }
    Err(format!(
        "its aliases lead to more than {ALIASES_MAX} others: commit with git commit -m"
    ))
}

/// The environment of a run.
type Env = BTreeMap<OsString, OsString>;

/// Runs the project's checks on each commit the push `words` sends from `folder`, with `env`,
/// the environment of its run: the refusal of the first that fails, naming it, or of a push
/// whose commits cannot be known. Nothing when the project declares none.
pub async fn pushed(at: &Where<'_>, words: &[String], env: &Env) -> Result<(), String> {
    let checks = declared(at.daemon, at.place, Checked::Pushed)?;
    if checks.is_empty() {
        return Ok(());
    }
    let unknown = |why: String| {
        let why = secrets::mask_also(&why, env, &at.place.secret_values());
        format!(
            "refused: the project's message checks read each commit a push sends before it \
             goes, and {why}; {}",
            Checked::Pushed.nothing()
        )
    };
    let at_push = gate::git_globals(&words[1..]).0 + 1;
    let git = &words[..at_push];
    for sha in sent(git, &words[at_push + 1..], at.folder, env)
        .await
        .map_err(unknown)?
    {
        let object = read_git(git, &["cat-file", "commit", &sha], at.folder, env, None)
            .await
            .map_err(unknown)?;
        let message = object.split_once("\n\n").map_or("", |(_, message)| message);
        let subject = crate::mcp::masked(message.lines().next().unwrap_or_default(), at.place);
        let short = &sha[..12.min(sha.len())];
        run_all(at, &checks, Checked::Pushed, message)
            .await
            .map_err(|refusal| {
                let said = refusal.strip_prefix("refused: ").unwrap_or(&refusal);
                format!("refused: the push sends the commit {short} ({subject}), and {said}")
            })?;
    }
    Ok(())
}

/// The commits a push sends to branches and tags, oldest first: those its refs lead to that no
/// ref of the remote has. Git says which refs the push sends, as it reads the line and the
/// configuration (`--dry-run`); the remote says which it has (`ls-remote`): its remote-tracking
/// refs are the agent's to write. Notes and other refs are no message of the project's history.
async fn sent(
    git: &[String],
    rest: &[String],
    folder: &Path,
    env: &Env,
) -> Result<Vec<String>, String> {
    // Last among its options, so that none of the line's turns them off.
    let end = rest
        .iter()
        .position(|word| word == "--" || word == "--end-of-options")
        .unwrap_or(rest.len());
    let said =
        read(&rest[..end], &PUSH).map_err(|why| format!("its options cannot be read: {why}"))?;
    let recursing = |value: &str| ["on-demand", "only"].contains(&value);
    let recursing = match said.submodules {
        Some(said) => recursing(&said),
        // Git takes whichever of the two keys it reads last; submodule.recurse true is on-demand.
        None => {
            setting(git, &["push.recurseSubmodules"], folder, env)
                .await?
                .is_some_and(|value| recursing(&value))
                || setting(git, &["--type=bool", "submodule.recurse"], folder, env)
                    .await?
                    .is_some_and(|value| value == "true")
        }
    };
    if recursing {
        return Err(
            "it would push the commits of submodules too (--recurse-submodules, \
             push.recurseSubmodules or submodule.recurse), which no check reads: push each \
             submodule from its own folder"
                .into(),
        );
    }
    let mut dry: Vec<String> = git.to_vec();
    dry.push("push".into());
    dry.extend_from_slice(&rest[..end]);
    dry.extend(["--dry-run", "--porcelain", "--no-quiet"].map(String::from));
    dry.extend_from_slice(&rest[end..]);
    let tried = crate::git::run_in(folder, &dry, env, None).await?;
    let mut remotes: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in tried.stdout.lines() {
        if let Some(url) = line.strip_prefix("To ") {
            remotes.push((url, Vec::new()));
        } else if let (Some((_, sources)), [flag, refs, _]) = (
            remotes.last_mut(),
            line.splitn(3, '\t').collect::<Vec<_>>().as_slice(),
        ) && [" ", "+", "*"].contains(flag)
            && let Some((source, destination)) = refs.rsplit_once(':')
            && ["refs/heads/", "refs/tags/"]
                .iter()
                .any(|kind| destination.starts_with(kind))
        {
            sources.push(source);
        }
    }
    // A URL that did not answer has no block, and may answer at the real push.
    if !tried.success {
        return Err(format!(
            "git could not say what it sends ({})",
            tried.stderr.trim()
        ));
    }
    let mut commits: Vec<String> = Vec::new();
    for (url, sources) in remotes {
        if sources.is_empty() {
            continue;
        }
        let mut input = String::new();
        for source in sources {
            let sha = read_git(git, &["rev-parse", "--verify", source], folder, env, None).await?;
            input.push_str(sha.trim());
            input.push('\n');
        }
        input.push_str("--not\n");
        // `insteadOf` would make it read another remote than the one the push goes to.
        let read = read_git(
            git,
            &["ls-remote", "--get-url", "--end-of-options", url],
            folder,
            env,
            None,
        )
        .await?;
        let read = read.trim_end_matches('\n');
        if read != url {
            return Err(format!(
                "the remote's URL is rewritten for reads ({url} is read as {read}), so what it \
                 has cannot be known: push without the rewrite"
            ));
        }
        let had = read_git(
            git,
            &["ls-remote", "--end-of-options", url],
            folder,
            env,
            None,
        )
        .await?;
        for listed in had.lines() {
            input.push_str(listed.split('\t').next().unwrap_or_default());
            input.push('\n');
        }
        let listed = read_git(
            git,
            &["rev-list", "--reverse", "--ignore-missing", "--stdin"],
            folder,
            env,
            Some(input.into_bytes()),
        )
        .await?;
        for sha in listed.lines() {
            if !commits.iter().any(|seen| seen == sha) {
                commits.push(sha.to_owned());
            }
        }
    }
    Ok(commits)
}

/// A key of git's configuration (after the options of `git config --get`), as `git` (`git` and
/// the line's global options) reads it: nothing when it is not set.
async fn setting(
    git: &[String],
    key: &[&str],
    folder: &Path,
    env: &Env,
) -> Result<Option<String>, String> {
    let mut words = git.to_vec();
    words.extend(
        ["config", "--get"]
            .into_iter()
            .chain(key.iter().copied())
            .map(String::from),
    );
    let output = crate::git::run_in(folder, &words, env, None).await?;
    match (output.success, output.stderr.trim()) {
        (true, _) => Ok(Some(output.stdout.trim_end_matches('\n').to_owned())),
        (false, "") => Ok(None),
        (false, said) => Err(format!(
            "git config --get {} failed ({said})",
            key.join(" ")
        )),
    }
}

/// A git command of `git`'s (`git` and the line's global options): its output, or what git
/// said when it failed.
async fn read_git(
    git: &[String],
    command: &[&str],
    folder: &Path,
    env: &Env,
    input: Option<Vec<u8>>,
) -> Result<String, String> {
    let mut words = git.to_vec();
    words.extend(command.iter().map(|word| word.to_string()));
    let output = crate::git::run_in(folder, &words, env, input).await?;
    match output.success {
        true => Ok(output.stdout),
        false => Err(format!(
            "git {} failed ({})",
            command[0],
            output.stderr.trim()
        )),
    }
}

/// Runs the project's checks on `message`: the refusal of the first that fails.
pub async fn check(at: &Where<'_>, checked: Checked, message: &str) -> Result<(), String> {
    let checks = declared(at.daemon, at.place, checked)?;
    run_all(at, &checks, checked, message).await
}

/// The project's checks, as its file says them now.
fn declared(daemon: &Daemon, place: &Place, checked: Checked) -> Result<Vec<String>, String> {
    crate::projects::load_project(daemon, &place.project)
        .map(|project| project.message_checks)
        .map_err(|failure| {
            format!(
                "refused: the project's file cannot be read, so its message checks cannot run \
                 ({}); {}",
                failure.message,
                checked.nothing()
            )
        })
}

async fn run_all(
    at: &Where<'_>,
    checks: &[String],
    checked: Checked,
    message: &str,
) -> Result<(), String> {
    if checks.is_empty() {
        return Ok(());
    }
    let file = MessageFile::write(at.daemon, message).map_err(|why| {
        format!(
            "refused: the message could not be written for the checks ({why}); {}",
            checked.nothing()
        )
    })?;
    for line in checks {
        run_one(at, line, &file.0, checked).await?;
    }
    Ok(())
}

/// The file a check reads the message in, in the daemon's own state folder, removed once the
/// checks ran.
struct MessageFile(PathBuf);

impl MessageFile {
    fn write(daemon: &Daemon, message: &str) -> Result<MessageFile, String> {
        let folder = daemon.paths.state.join("messages");
        flottille_core::paths::ensure_private_dir(&folder).map_err(|e| e.to_string())?;
        let path = folder.join(new_id().map_err(|e| e.to_string())?);
        let mut text = message.to_owned();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(MessageFile(path))
    }
}

impl Drop for MessageFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The process group of a check, killed when it is dropped.
struct Group(Option<u32>);

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            crate::agents::signal_group(pid, libc::SIGKILL);
        }
    }
}

/// One check: split without a shell, `{message}` filled with the file, run in the place's run
/// environment, bounded in time.
async fn run_one(at: &Where<'_>, line: &str, file: &Path, checked: Checked) -> Result<(), String> {
    let shown = crate::mcp::masked(line, at.place);
    let not_run = |why: String| {
        format!(
            "refused: the message check `{shown}` could not run ({why}); a check that does not \
             run never passes: {}",
            checked.nothing()
        )
    };
    let file = file.to_string_lossy();
    let words: Vec<String> = split_line(line)
        .map_err(not_run)?
        .iter()
        .map(|word| word.replace(MESSAGE, &file))
        .collect();
    let env =
        crate::runs::environment(at.daemon, at.place, at.repository, false).map_err(not_run)?;
    let mut command = tokio::process::Command::new(&words[0]);
    command
        .args(&words[1..])
        .env_clear()
        .envs(&env)
        .current_dir(at.folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let child = crate::children::spawn(&mut command)
        .map_err(|e| not_run(format!("cannot run {}: {e}", words[0])))?;
    // Its whole group goes once it ended, was stopped at its limit, or its call was dropped:
    // `kill_on_drop` alone stops only its first process.
    let _group = Group(child.id());
    let output = match tokio::time::timeout(TIME_LIMIT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(not_run(e.to_string())),
        Err(_) => {
            return Err(not_run(format!(
                "it took longer than {} s and was stopped",
                TIME_LIMIT.as_secs()
            )));
        }
    };
    if output.status.success() {
        return Ok(());
    }
    let ended = match (
        output.status.code(),
        std::os::unix::process::ExitStatusExt::signal(&output.status),
    ) {
        (Some(code), _) => format!("it exited with code {code}"),
        (None, Some(signal)) => format!("it was killed by signal {signal}"),
        (None, None) => "it failed".to_owned(),
    };
    let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
    said.push_str(&String::from_utf8_lossy(&output.stderr));
    let said = secrets::mask_also(&ansi::strip(&said), &env, &at.place.secret_values());
    Err(format!(
        "refused: the message check `{shown}` refused {} ({ended}); {}: change it and try \
         again. Its output:\n{}",
        checked.what(),
        checked.nothing(),
        end_of(said.trim())
    ))
}

/// The end of a text, from a line's start when one is near, saying what was left out.
fn end_of(text: &str) -> String {
    let count = text.chars().count();
    if count <= OUTPUT_CHARS {
        return text.to_owned();
    }
    let start = text
        .char_indices()
        .nth(count - OUTPUT_CHARS)
        .map_or(0, |(at, _)| at);
    let start = text[start..]
        .find('\n')
        .filter(|newline| *newline < OUTPUT_CHARS / 2)
        .map_or(start, |newline| start + newline + 1);
    format!(
        "[the first {} characters are left out]\n{}",
        text[..start].chars().count(),
        &text[start..]
    )
}

/// What a git command (its words from `git` on) run in `folder` commits as its message: `None`
/// when it makes no commit, or why the message cannot be known before the commit, with what to
/// do instead: an editor writes it, git takes it from another commit or writes it itself.
pub fn message_of(git: &[String], folder: &Path) -> Result<Option<String>, String> {
    let args = &git[1..];
    let (at, globals) = gate::git_globals(args);
    let mut folder = folder.to_path_buf();
    for (option, value) in globals {
        if let ("-C", Some(value)) = (option, value) {
            folder = folder.join(value);
        }
    }
    let Some(command) = args.get(at) else {
        return Ok(None);
    };
    let rest = &args[at + 1..];
    match command.as_str() {
        "commit" => commit(read(rest, &COMMIT)?, &folder)
            .map(Some)
            .map_err(|why| format!("{why}: give the message with -m or -F")),
        "merge" => merge(read(rest, &MERGE)?, &folder).map_err(|why| {
            format!(
                "{why}: give the message with -m or -F, or merge with --no-commit and commit with \
                 git commit -m"
            )
        }),
        "commit-tree" => joined(read(rest, &COMMIT_TREE)?, &folder)
            .and_then(|message| {
                message.ok_or_else(|| "it reads its message from its standard input".to_owned())
            })
            .map(Some)
            .map_err(|why| {
                format!("{why}: give the message with -m or -F, or commit with git commit -m")
            }),
        "revert" => revert(&read(rest, &REVERT)?),
        _ => Ok(None),
    }
}

/// `git commit`: its `-m`, as git joins them, or its last `-F` file.
fn commit(mut said: Said, folder: &Path) -> Result<String, String> {
    if said.edit {
        return Err("it opens an editor on the message (--edit)".into());
    }
    let files = said
        .parts
        .iter()
        .filter(|p| matches!(p, Part::File(_)))
        .count();
    match (said.parts.len() - files, files) {
        (0, 0) => Err("it gives no message: an editor would write it".into()),
        (0, _) => {
            said.parts.drain(..files - 1);
            joined(said, folder).map(Option::unwrap_or_default)
        }
        (_, 0) => joined(said, folder).map(Option::unwrap_or_default),
        _ => Err("it gives both -m and -F, which git refuses".into()),
    }
}

/// `git merge`: none when it makes no commit, its `-m` and `-F` when it does.
fn merge(said: Said, folder: &Path) -> Result<Option<String>, String> {
    let (mut squash, mut no_commit, mut ff_only) = (false, false, false);
    for switch in &said.switches {
        match *switch {
            "abort" | "quit" => return Ok(None),
            "continue" => return Err("git writes the message of the merge it concludes".into()),
            "squash" | "no-squash" => squash = *switch == "squash",
            "commit" | "no-commit" => no_commit = *switch == "no-commit",
            _ => ff_only = *switch == "ff-only",
        }
    }
    if squash || no_commit || ff_only {
        return Ok(None);
    }
    if said.edit {
        return Err("it opens an editor on the message (--edit)".into());
    }
    match joined(said, folder)? {
        Some(message) => Ok(Some(message)),
        None => Err("git writes the message of the merge (Merge branch …)".into()),
    }
}

/// `git revert`: none with `--no-commit`, refused otherwise, git writes its message.
fn revert(said: &Said) -> Result<Option<String>, String> {
    let mut no_commit = false;
    for switch in &said.switches {
        match *switch {
            "abort" | "quit" => return Ok(None),
            "continue" | "skip" => no_commit = false,
            _ => no_commit = *switch == "no-commit",
        }
    }
    match no_commit {
        true => Ok(None),
        false => Err(
            "git writes the message of a revert (Revert \"…\"): revert with --no-commit, then \
             commit with git commit -m"
                .into(),
        ),
    }
}

/// The messages and files a command gives, in their order, a paragraph each: `None` when it
/// gives neither.
fn joined(said: Said, folder: &Path) -> Result<Option<String>, String> {
    let mut paragraphs = Vec::new();
    for part in said.parts {
        paragraphs.push(match part {
            Part::Text(text) => text,
            Part::File(file) if file == "-" => {
                return Err("it reads the message from its standard input".into());
            }
            Part::File(file) => read_message(&folder.join(file))?,
        });
    }
    Ok((!paragraphs.is_empty()).then(|| paragraphs.join("\n\n")))
}

/// What a command's options say of its message.
#[derive(Default)]
struct Said {
    /// Its `-m` and `-F`, in their order.
    parts: Vec<Part>,
    edit: bool,
    /// The switches that say whether a commit is made, in their order.
    switches: Vec<&'static str>,
    /// The last `--recurse-submodules` of a push (empty for `--no-recurse-submodules`).
    submodules: Option<String>,
}

enum Part {
    Text(String),
    File(String),
}

/// How a git command reads one of its options, for its message.
#[derive(Clone, Copy)]
enum Opt {
    /// A paragraph of the message.
    Message,
    /// A file holding the message.
    File,
    /// Drops the messages, or the files, given before it.
    NoMessage,
    NoFile,
    /// Takes a value, which is no message.
    Valued,
    /// Takes a value only stuck to it (`-Skey`), which ends a cluster of letters.
    Stuck,
    Edit(bool),
    /// The message comes from elsewhere than the line: why.
    Elsewhere(&'static str),
    /// Says whether a commit is made: its name.
    Switch(&'static str),
    /// Says which submodules a push sends too: whether it takes a value.
    Submodules(bool),
    Flag,
}

/// A git command's options: its long ones as `--git-completion-helper-all` says them (git
/// 2.39, a value marked `=`), those of them that matter to the message, and its letters that do.
struct Options {
    longs: &'static str,
    matter: &'static [(&'static str, Opt)],
    letters: &'static [(char, Opt)],
}

const OTHER_COMMIT: Opt = Opt::Elsewhere("it takes the message of another commit");

const COMMIT: Options = Options {
    longs: "quiet verbose file= author= date= message= reedit-message= reuse-message= fixup= \
        squash= reset-author trailer= signoff template= edit cleanup= status gpg-sign all \
        include interactive patch only no-verify dry-run short branch ahead-behind porcelain \
        long null amend no-post-rewrite untracked-files pathspec-from-file= pathspec-file-nul \
        allow-empty allow-empty-message verify post-rewrite no-quiet no-verbose no-file \
        no-author no-date no-message no-reedit-message no-reuse-message no-fixup no-squash \
        no-reset-author no-signoff no-template no-edit no-cleanup no-status no-gpg-sign no-all \
        no-include no-interactive no-patch no-only no-dry-run no-short no-branch \
        no-ahead-behind no-porcelain no-long no-null no-amend no-untracked-files \
        no-pathspec-from-file no-pathspec-file-nul no-allow-empty no-allow-empty-message",
    matter: &[
        ("message", Opt::Message),
        ("file", Opt::File),
        ("no-message", Opt::NoMessage),
        ("no-file", Opt::NoFile),
        ("edit", Opt::Edit(true)),
        ("no-edit", Opt::Edit(false)),
        ("reuse-message", OTHER_COMMIT),
        ("reedit-message", OTHER_COMMIT),
        ("fixup", Opt::Elsewhere("git writes the message of a fixup")),
        (
            "squash",
            Opt::Elsewhere("git writes the message of a squash commit"),
        ),
    ],
    letters: &[
        ('m', Opt::Message),
        ('F', Opt::File),
        ('C', OTHER_COMMIT),
        ('c', OTHER_COMMIT),
        ('t', Opt::Valued),
        ('e', Opt::Edit(true)),
        ('S', Opt::Stuck),
        ('u', Opt::Stuck),
    ],
};

const MERGE: Options = Options {
    longs: "stat summary log squash commit edit cleanup= ff ff-only rerere-autoupdate \
        verify-signatures strategy= strategy-option= message= file= into-name= verbose quiet \
        abort quit continue allow-unrelated-histories progress gpg-sign autostash \
        overwrite-ignore signoff no-verify verify no-stat no-summary no-log no-squash no-commit \
        no-edit no-cleanup no-ff no-rerere-autoupdate no-verify-signatures no-strategy \
        no-strategy-option no-message no-into-name no-verbose no-quiet no-abort no-quit \
        no-continue no-allow-unrelated-histories no-progress no-gpg-sign no-autostash \
        no-overwrite-ignore no-signoff",
    matter: &[
        ("message", Opt::Message),
        ("file", Opt::File),
        ("no-message", Opt::NoMessage),
        ("edit", Opt::Edit(true)),
        ("no-edit", Opt::Edit(false)),
        ("squash", Opt::Switch("squash")),
        ("no-squash", Opt::Switch("no-squash")),
        ("commit", Opt::Switch("commit")),
        ("no-commit", Opt::Switch("no-commit")),
        ("ff", Opt::Switch("ff")),
        ("no-ff", Opt::Switch("no-ff")),
        ("ff-only", Opt::Switch("ff-only")),
        ("abort", Opt::Switch("abort")),
        ("quit", Opt::Switch("quit")),
        ("continue", Opt::Switch("continue")),
    ],
    letters: &[
        ('m', Opt::Message),
        ('F', Opt::File),
        ('e', Opt::Edit(true)),
        ('s', Opt::Valued),
        ('X', Opt::Valued),
        ('S', Opt::Stuck),
    ],
};

const COMMIT_TREE: Options = Options {
    longs: "gpg-sign no-gpg-sign",
    matter: &[],
    letters: &[
        ('p', Opt::Valued),
        ('m', Opt::Message),
        ('F', Opt::File),
        ('S', Opt::Stuck),
    ],
};

const REVERT: Options = Options {
    longs: "quit continue abort skip cleanup= no-commit edit signoff mainline= \
        rerere-autoupdate strategy= strategy-option= gpg-sign reference commit no-cleanup \
        no-edit no-signoff no-mainline no-rerere-autoupdate no-strategy no-strategy-option \
        no-gpg-sign no-reference",
    matter: &[
        ("no-commit", Opt::Switch("no-commit")),
        ("commit", Opt::Switch("commit")),
        ("abort", Opt::Switch("abort")),
        ("quit", Opt::Switch("quit")),
        ("continue", Opt::Switch("continue")),
        ("skip", Opt::Switch("skip")),
    ],
    letters: &[
        ('n', Opt::Switch("no-commit")),
        ('m', Opt::Valued),
        ('X', Opt::Valued),
        ('S', Opt::Stuck),
    ],
};

const PUSH: Options = Options {
    longs: "verbose quiet repo= all branches mirror delete tags dry-run porcelain force \
        force-with-lease force-if-includes recurse-submodules= thin receive-pack= exec= \
        set-upstream progress prune no-verify follow-tags signed atomic push-option= ipv4 ipv6 \
        verify no-verbose no-quiet no-repo no-all no-branches no-mirror no-delete no-tags \
        no-dry-run no-porcelain no-force no-force-with-lease no-force-if-includes \
        no-recurse-submodules no-thin no-receive-pack no-exec no-set-upstream no-progress \
        no-prune no-follow-tags no-signed no-atomic no-push-option",
    matter: &[
        ("recurse-submodules", Opt::Submodules(true)),
        ("no-recurse-submodules", Opt::Submodules(false)),
    ],
    letters: &[('o', Opt::Valued)],
};

/// The options of a command's words, read as git reads them, up to `--`.
fn read(words: &[String], options: &Options) -> Result<Said, String> {
    let mut said = Said::default();
    let mut words = words.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        if let Some(long) = word.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v)));
            let opt = long_option(options, name)?;
            let value = match (opt, value) {
                (Opt::Message | Opt::File | Opt::Valued | Opt::Submodules(true), None) => {
                    Some(words.next().ok_or("an option lacks its value")?)
                }
                _ => value,
            };
            take(&mut said, opt, value.unwrap_or_default())?;
        } else if let Some(cluster) = word.strip_prefix('-').filter(|c| !c.is_empty()) {
            for (index, letter) in cluster.char_indices() {
                let opt = options
                    .letters
                    .iter()
                    .find(|(known, _)| *known == letter)
                    .map_or(Opt::Flag, |(_, opt)| *opt);
                match opt {
                    Opt::Stuck => break,
                    Opt::Message | Opt::File | Opt::Valued => {
                        let rest = &cluster[index + letter.len_utf8()..];
                        let value = match rest.is_empty() {
                            true => words.next().ok_or("an option lacks its value")?,
                            false => rest,
                        };
                        take(&mut said, opt, value)?;
                        break;
                    }
                    _ => take(&mut said, opt, "")?,
                }
            }
        }
    }
    Ok(said)
}

fn take(said: &mut Said, opt: Opt, value: &str) -> Result<(), String> {
    match opt {
        Opt::Message => said.parts.push(Part::Text(value.to_owned())),
        Opt::File => said.parts.push(Part::File(value.to_owned())),
        Opt::NoMessage => said.parts.retain(|p| !matches!(p, Part::Text(_))),
        Opt::NoFile => said.parts.retain(|p| !matches!(p, Part::File(_))),
        Opt::Edit(on) => said.edit = on,
        Opt::Elsewhere(why) => return Err(why.into()),
        Opt::Switch(name) => said.switches.push(name),
        Opt::Submodules(_) => said.submodules = Some(value.to_owned()),
        Opt::Valued | Opt::Stuck | Opt::Flag => {}
    }
    Ok(())
}

/// A long option as git finds it: its whole name, or the start of one name alone, however short
/// (`--m` is `--message`); a start of several is ambiguous, which git refuses too. One git does
/// not have is left to git to refuse.
fn long_option(options: &Options, name: &str) -> Result<Opt, String> {
    let of = |long: &'static str| {
        let (long, valued) = long
            .strip_suffix('=')
            .map_or((long, false), |long| (long, true));
        let opt = options
            .matter
            .iter()
            .find(|(known, _)| *known == long)
            .map(|(_, opt)| *opt);
        (
            long,
            opt.unwrap_or(if valued { Opt::Valued } else { Opt::Flag }),
        )
    };
    let longs = || options.longs.split_whitespace().map(of);
    if let Some((_, opt)) = longs().find(|(long, _)| *long == name) {
        return Ok(opt);
    }
    let started: Vec<(&str, Opt)> = longs().filter(|(long, _)| long.starts_with(name)).collect();
    match started.as_slice() {
        [] => Ok(Opt::Flag),
        [(_, opt)] => Ok(*opt),
        _ => Err(format!("--{name} is ambiguous, which git refuses")),
    }
}

/// The message file of `-F`, read at once: a FIFO or a device would hold the read, or the open,
/// until something writes to it.
fn read_message(path: &Path) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let unread =
        |e: std::io::Error| format!("its message file {} cannot be read: {e}", path.display());
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(unread)?;
    if !opened.metadata().map_err(unread)?.is_file() {
        return Err(format!(
            "its message file {} is not a regular file",
            path.display()
        ));
    }
    let mut text = String::new();
    opened
        .take(FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(unread)?;
    if text.len() as u64 > FILE_BYTES {
        return Err(format!(
            "its message file {} is longer than {FILE_BYTES} bytes",
            path.display()
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        split_line(line).unwrap()
    }

    /// What the line's one git command commits as its message: `None` when it commits none.
    fn committed(line: &str) -> Result<Option<String>, String> {
        let gits = gate::gits(&words(line));
        assert_eq!(gits.len(), 1, "{line}");
        message_of(&gits[0], Path::new("/nowhere"))
    }

    fn message(line: &str) -> Result<String, String> {
        committed(line).map(|message| message.unwrap_or_else(|| panic!("no commit: {line}")))
    }

    #[test]
    fn the_message_of_a_commit_line_is_read_from_its_options() {
        for (line, said) in [
            ("git commit -m 'fix: a'", "fix: a"),
            ("git commit -am 'fix: a'", "fix: a"),
            ("git commit -mfix", "fix"),
            ("git commit --message='fix: a' -m body", "fix: a\n\nbody"),
            ("git commit --mess 'fix: a'", "fix: a"),
            ("git commit --amend -m 'fix: a'", "fix: a"),
            ("git commit --author 'A <a@b>' -m 'fix: a' -- -m", "fix: a"),
            ("git -C api commit -m 'fix: a'", "fix: a"),
            ("env A=1 git commit -m 'fix: a'", "fix: a"),
            ("sh -c \"git commit -m 'fix: a'\"", "fix: a"),
            ("git commit -m 'fix: a' -e --no-edit", "fix: a"),
        ] {
            assert_eq!(message(line).as_deref(), Ok(said), "{line}");
        }
    }

    #[test]
    fn a_commit_whose_message_is_written_elsewhere_has_none() {
        for line in [
            "git commit",
            "git commit --amend --no-edit",
            "git commit -m 'fix: a' -e",
            "git commit -m 'fix: a' --edit",
            "git commit -C HEAD",
            "git commit --reuse-message=HEAD",
            "git commit --fixup HEAD",
            "git commit --squash=HEAD",
            "git commit -F -",
            "git commit -m 'fix: a' -F msg",
            "git commit -m",
        ] {
            assert!(message(line).is_err(), "{line}");
        }
    }

    #[test]
    fn a_message_file_is_read_where_git_runs() {
        let folder = std::env::temp_dir().join(format!("flottille-message-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("api")).unwrap();
        std::fs::write(folder.join("api/msg"), "fix: a\n").unwrap();
        let gits = gate::gits(&words("git -C api commit -F msg"));
        assert_eq!(
            message_of(&gits[0], &folder),
            Ok(Some("fix: a\n".to_owned()))
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_line_without_a_commit_has_none_to_check() {
        for line in ["git log -m", "git status", "git merge --ff-only -m x main"] {
            assert_eq!(committed(line), Ok(None), "{line}");
        }
        assert!(gate::gits(&words("echo git commit")).is_empty());
    }

    #[test]
    fn a_long_option_of_any_length_is_read_as_git_reads_it() {
        for (line, said) in [
            (
                "git commit --m 'not conventional' -m 'fix: ok'",
                "not conventional\n\nfix: ok",
            ),
            ("git commit --me 'not conventional'", "not conventional"),
            ("git commit --m=one --no-m -m two", "two"),
        ] {
            assert_eq!(message(line).as_deref(), Ok(said), "{line}");
        }
        // `--f` starts --file and --fixup, `--re` three options: git refuses both.
        for line in ["git commit --f msg", "git commit --re HEAD -m 'fix: a'"] {
            let refused = message(line).unwrap_err();
            assert!(refused.contains("ambiguous"), "{line}: {refused}");
        }
        // `--e` is --edit, `--fix` is --fixup.
        assert!(message("git commit -m 'fix: a' --e").is_err());
        assert!(message("git commit --fix HEAD").is_err());
    }

    #[test]
    fn the_message_of_a_merge_a_tree_commit_and_a_revert() {
        for (line, said) in [
            ("git merge --no-ff -m 'bad merge' other", "bad merge"),
            ("git merge --mes 'a' -m b other", "a\n\nb"),
            (
                "git commit-tree -m 'bad tree' -p HEAD HEAD^{tree}",
                "bad tree",
            ),
        ] {
            assert_eq!(message(line).as_deref(), Ok(said), "{line}");
        }
        for line in [
            "git merge --squash other",
            "git merge --no-commit other",
            "git merge --abort",
            "git revert --no-commit HEAD",
            "git revert -n HEAD",
            "git revert --abort",
        ] {
            assert_eq!(committed(line), Ok(None), "{line}");
        }
        for line in [
            "git merge other",
            "git merge -m x -e other",
            "git merge --continue",
            "git commit-tree HEAD^{tree}",
            "git revert HEAD",
            "git revert --no-edit -m 1 HEAD",
        ] {
            let refused = committed(line).unwrap_err();
            assert!(refused.contains("git commit -m"), "{line}: {refused}");
        }
    }

    #[test]
    fn a_message_file_that_is_not_a_regular_file_is_refused() {
        let folder = std::env::temp_dir().join(format!("flottille-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let fifo = folder.join("msg");
        let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let gits = gate::gits(&words("git commit -F msg"));
        // A FIFO with no writer would hold an open for reading forever.
        let (sender, answer) = std::sync::mpsc::channel();
        let read = folder.clone();
        std::thread::spawn(move || sender.send(message_of(&gits[0], &read)));
        let refused = answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the read of a FIFO returns")
            .unwrap_err();
        assert!(refused.contains("not a regular file"), "{refused}");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_long_output_keeps_its_end() {
        let text: String = (0..400).map(|n| format!("line {n}\n")).collect();
        let cut = end_of(text.trim());
        assert!(cut.starts_with("[the first "), "{cut}");
        assert!(cut.ends_with("line 399"), "{cut}");
        assert!(cut.chars().count() < OUTPUT_CHARS + 60, "{}", cut.len());
        assert_eq!(end_of("short"), "short");
    }
}
