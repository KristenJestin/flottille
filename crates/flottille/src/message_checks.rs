//! The project's message checks (`docs/projects.md`, "Message checks"): the command lines the
//! user declares, each run on a file holding a message before an agent's commit, a pull
//! request's title or a squash message is made. They are the user's configuration, never the
//! repository's: git runs no hook for an agent (`runs::defend`), and a check is run by the
//! daemon, without a shell, in the place's run environment, never as a call of the agent.

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
}

impl Checked {
    fn what(self) -> &'static str {
        match self {
            Checked::Commit => "the commit message",
            Checked::Title => "the pull request's title",
            Checked::Squash => "the squash message",
        }
    }

    fn nothing(self) -> &'static str {
        match self {
            Checked::Commit => "nothing was committed",
            Checked::Title => "nothing was written to the forge",
            Checked::Squash => "nothing was merged",
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

/// Runs the project's checks on each `git commit` the line `words` runs in `folder`: the
/// refusal of the first that fails, or of a commit whose message cannot be known before it is
/// made. Nothing when the project declares none.
pub async fn commits(at: &Where<'_>, words: &[String]) -> Result<(), String> {
    let commits = gate::commits(words);
    if commits.is_empty() {
        return Ok(());
    }
    let checks = declared(at.daemon, at.place, Checked::Commit)?;
    if checks.is_empty() {
        return Ok(());
    }
    for git in commits {
        let message = commit_message(&git, at.folder).map_err(|why| {
            format!(
                "refused: the project's message checks read the commit message before the \
                 commit, and {why}: give the message with -m or -F; {}",
                Checked::Commit.nothing()
            )
        })?;
        run_all(at, &checks, Checked::Commit, &message).await?;
    }
    Ok(())
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
        .map_err(&not_run)?
        .iter()
        .map(|word| word.replace(MESSAGE, &file))
        .collect();
    let env =
        crate::runs::environment(at.daemon, at.place, at.repository, false).map_err(&not_run)?;
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
    let child = command
        .spawn()
        .map_err(|e| not_run(format!("cannot run {}: {e}", words[0])))?;
    let pid = child.id();
    let output = match tokio::time::timeout(TIME_LIMIT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(not_run(e.to_string())),
        Err(_) => {
            if let Some(pid) = pid {
                crate::agents::signal_group(pid, libc::SIGKILL);
            }
            return Err(not_run(format!(
                "it took longer than {} s and was stopped",
                TIME_LIMIT.as_secs()
            )));
        }
    };
    // What it left in its group goes with it.
    if let Some(pid) = pid {
        crate::agents::signal_group(pid, libc::SIGKILL);
    }
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

/// What a `git commit` (its words from `git` on) commits as its message, run in `folder`, or
/// why it cannot be known before the commit: an editor writes it, or git takes it from another
/// commit.
pub fn commit_message(git: &[String], folder: &Path) -> Result<String, String> {
    let args = &git[1..];
    let (at, globals) = gate::git_globals(args);
    let mut folder = folder.to_path_buf();
    for (option, value) in globals {
        if let ("-C", Some(value)) = (option, value) {
            folder = folder.join(value);
        }
    }
    let mut messages: Vec<String> = Vec::new();
    let mut file: Option<String> = None;
    let mut edit = false;
    let mut words = args[at + 1..].iter().map(String::as_str);
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        if let Some(long) = word.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v)));
            let long = long_option(name)?;
            match long {
                Long::Message | Long::File | Long::Valued => {
                    let value = match value {
                        Some(value) => value,
                        None => words.next().ok_or("an option lacks its value")?,
                    };
                    match long {
                        Long::Message => messages.push(value.to_owned()),
                        Long::File => file = Some(value.to_owned()),
                        _ => {}
                    }
                }
                Long::Edit(on) => edit = on,
                Long::Elsewhere(why) => return Err(why.into()),
                Long::Flag => {}
            }
        } else if let Some(cluster) = word.strip_prefix('-').filter(|c| !c.is_empty()) {
            for (index, letter) in cluster.char_indices() {
                if letter == 'e' {
                    edit = true;
                    continue;
                }
                if !"mFCct".contains(letter) {
                    continue;
                }
                let rest = &cluster[index + letter.len_utf8()..];
                let value = match rest.is_empty() {
                    true => words.next().ok_or("an option lacks its value")?,
                    false => rest,
                };
                match letter {
                    'm' => messages.push(value.to_owned()),
                    'F' => file = Some(value.to_owned()),
                    'C' | 'c' => return Err("it takes the message of another commit".into()),
                    _ => {}
                }
                break;
            }
        }
    }
    if edit {
        return Err("it opens an editor on the message (--edit)".into());
    }
    match (messages.is_empty(), file) {
        (false, None) => Ok(messages.join("\n\n")),
        (false, Some(_)) => Err("it gives both -m and -F, which git refuses".into()),
        (true, Some(file)) if file == "-" => {
            Err("it reads the message from its standard input".into())
        }
        (true, Some(file)) => read_message(&folder.join(file)),
        (true, None) => Err("it gives no message: an editor would write it".into()),
    }
}

/// The long options of `git commit` that matter to its message. Git takes any unambiguous start
/// of one.
#[derive(Clone, Copy)]
enum Long {
    Message,
    File,
    /// One that takes a value, which is no message.
    Valued,
    Edit(bool),
    /// The message comes from elsewhere than the line: why.
    Elsewhere(&'static str),
    Flag,
}

fn long_option(name: &str) -> Result<Long, String> {
    const OPTIONS: [(&str, Long); 14] = [
        ("message", Long::Message),
        ("file", Long::File),
        ("author", Long::Valued),
        ("date", Long::Valued),
        ("cleanup", Long::Valued),
        ("trailer", Long::Valued),
        ("template", Long::Valued),
        ("pathspec-from-file", Long::Valued),
        ("edit", Long::Edit(true)),
        ("no-edit", Long::Edit(false)),
        (
            "reuse-message",
            Long::Elsewhere("it takes the message of another commit"),
        ),
        (
            "reedit-message",
            Long::Elsewhere("it takes the message of another commit"),
        ),
        (
            "fixup",
            Long::Elsewhere("git writes the message of a fixup"),
        ),
        (
            "squash",
            Long::Elsewhere("git writes the message of a squash commit"),
        ),
    ];
    if let Some((_, long)) = OPTIONS.iter().find(|(option, _)| *option == name) {
        return Ok(*long);
    }
    let started: Vec<Long> = OPTIONS
        .iter()
        .filter(|(option, _)| name.len() >= 3 && option.starts_with(name))
        .map(|(_, long)| *long)
        .collect();
    match started.as_slice() {
        [] => Ok(Long::Flag),
        [one] => Ok(*one),
        _ => Err(format!("--{name} is ambiguous")),
    }
}

fn read_message(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let opened = std::fs::File::open(path)
        .map_err(|e| format!("its message file {} cannot be read: {e}", path.display()))?;
    let mut text = String::new();
    opened
        .take(FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("its message file {} cannot be read: {e}", path.display()))?;
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

    fn message(line: &str) -> Result<String, String> {
        let commits = gate::commits(&words(line));
        assert_eq!(commits.len(), 1, "{line}");
        commit_message(&commits[0], Path::new("/nowhere"))
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
        let commits = gate::commits(&words("git -C api commit -F msg"));
        assert_eq!(
            commit_message(&commits[0], &folder).as_deref(),
            Ok("fix: a\n")
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_line_without_a_commit_has_none_to_check() {
        for line in ["git log -m", "echo git commit", "git status"] {
            assert!(gate::commits(&words(line)).is_empty(), "{line}");
        }
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
