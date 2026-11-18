//! The user's own `git`, spawned without a shell (`docs/reference/hemera.md` §6, "Git invocation
//! rules"): always `-C <folder>`, never a prompt, every call bounded in time and in output, and a
//! call cut at its limit killed with its group and waited for.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

// Called by the link once a launch sends a place's code to another machine (#815).
#[cfg_attr(not(test), allow(dead_code))]
pub mod bundle;

/// How long a call may take.
#[derive(Clone, Copy)]
pub enum Class {
    /// Status, rev-parse, log, the fetch of a few branches.
    Read,
    /// Anything that walks the whole tree: worktree add, remove, prune.
    Work,
}

const OUTPUT_LIMIT: u64 = 32 * 1024 * 1024;

impl Class {
    fn limit(self) -> Duration {
        match self {
            Class::Read => Duration::from_secs(30),
            Class::Work => Duration::from_secs(30 * 60),
        }
    }
}

/// What a call that ran to its end gave.
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `git -C <dir> <args>`. The error is a sentence: git missing, or the call cut.
pub async fn run<S: AsRef<OsStr>>(dir: &Path, args: &[S], class: Class) -> Result<Output, String> {
    run_with(dir, args, class, &[]).await
}

/// [`run`], with `env` set on git besides the daemon's own environment.
pub async fn run_with<S: AsRef<OsStr>>(
    dir: &Path,
    args: &[S],
    class: Class,
    env: &[(&str, &OsStr)],
) -> Result<Output, String> {
    let mut command = command(dir, args);
    command.envs(env.iter().copied());
    crate::landlock::confined(&mut command);
    bounded(command, dir, args, class.limit(), OUTPUT_LIMIT, None).await
}

/// [`output`] of `git <subcommand> <args>` that reaches `remote` (`ls-remote`, `fetch`): only the
/// protocol its URL uses, as git resolves it, git's own upload command over the remote's, and the
/// user's SSH agent lent when that protocol is SSH (`docs/vault.md`, "The runtime folder").
pub async fn output_remote(
    dir: &Path,
    remote: &str,
    subcommand: &str,
    args: &[&str],
    class: Class,
) -> Result<String, String> {
    let url = output(
        dir,
        &["ls-remote", "--get-url", "--end-of-options", remote],
        Class::Read,
    )
    .await?;
    let protocol = protocol(url.trim());
    // The line's own option, since the first `remote.<name>.uploadpack` git reads is the one it
    // takes: the repository's, before any given in the environment.
    let args: Vec<&str> = [subcommand, "--upload-pack=git-upload-pack"]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    let mut git = command(dir, &args);
    git.env("GIT_ALLOW_PROTOCOL", protocol);
    match crate::ssh_agent::socket().filter(|_| protocol == "ssh") {
        Some(socket) => crate::landlock::confined_lent(git.env(crate::ssh_agent::VARIABLE, socket)),
        None => crate::landlock::confined(&mut git),
    };
    said(bounded(git, dir, &args, class.limit(), OUTPUT_LIMIT, None).await?)
}

/// The name `GIT_ALLOW_PROTOCOL` knows the transport of `url` by, read as git reads it: a
/// helper's `<name>::`, a scheme (`ssh` for its other spellings), `ssh` for `host:path`, `file`
/// for a path.
fn protocol(url: &str) -> &str {
    let scheme = url
        .find(|c: char| !(c.is_ascii_alphanumeric() || "+.-".contains(c)))
        .map_or(url, |end| &url[..end]);
    let rest = &url[scheme.len()..];
    if !scheme.is_empty() && rest.starts_with("::") {
        return scheme;
    }
    if !scheme.is_empty() && rest.starts_with("://") {
        return match scheme {
            "git+ssh" | "ssh+git" => "ssh",
            scheme => scheme,
        };
    }
    match url.find(':') {
        Some(colon) if !url[..colon].contains('/') => "ssh",
        _ => "file",
    }
}

/// The `ssh` git runs: the one found at the daemon's start, quoted for the shell git reads
/// `GIT_SSH_COMMAND` with.
fn ssh_command() -> OsString {
    match crate::ssh_agent::ssh() {
        Some(ssh) => {
            let quoted = ssh.as_os_str().to_string_lossy().replace('\'', "'\\''");
            format!("'{quoted}'").into()
        }
        None => "ssh".into(),
    }
}

/// Runs `words` (`git`, its global options, its command) in `dir` with the environment `env` and
/// no other, `input` on its standard input: a git line of an agent's, read before it runs, in
/// the environment of its run. Unlike `command`, it adds no `-c` of its own: the run's
/// environment already sets `core.hooksPath`, and the line's global options, approved with the
/// line, apply to what is read for it as they apply to the line itself.
pub async fn run_in<S: AsRef<OsStr>>(
    dir: &Path,
    words: &[S],
    env: &BTreeMap<OsString, OsString>,
    input: Option<Vec<u8>>,
) -> Result<Output, String> {
    let mut command = read_in(dir, words, env, input.is_some());
    // It runs the line's own options and its transport (a remote's helper, its ssh command), as
    // the run would: kept from what a run is kept from.
    crate::landlock::confined(&mut command);
    let limit = Class::Read.limit();
    bounded(command, dir, &words[1..], limit, OUTPUT_LIMIT, input).await
}

/// [`run_in`] outside the sandbox: for what a push the gate let through reads of its remote
/// before it goes (its dry run), which signs in as the push itself does (`runs::lifted`).
pub async fn run_outside<S: AsRef<OsStr>>(
    dir: &Path,
    words: &[S],
    env: &BTreeMap<OsString, OsString>,
) -> Result<Output, String> {
    let command = read_in(dir, words, env, false);
    let limit = Class::Read.limit();
    bounded(command, dir, &words[1..], limit, OUTPUT_LIMIT, None).await
}

/// The command of [`run_in`] and [`run_outside`]: never a prompt, nowhere to answer it.
fn read_in<S: AsRef<OsStr>>(
    dir: &Path,
    words: &[S],
    env: &BTreeMap<OsString, OsString>,
    input: bool,
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&words[0]);
    command
        .args(&words[1..])
        .current_dir(dir)
        .env_clear()
        .envs(env)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(if input { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    command
}

fn command<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        // A repository's own config may name programs (a file system monitor, hooks): the daemon
        // reads and edits repositories of the user's, it never runs what they say.
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "fetch.recurseSubmodules=false",
            "-c",
            "submodule.recurse=false",
        ])
        .args(args)
        // A fetch that needs credentials fails at once; stored credentials still work.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        // No SSH agent but where `output_remote` lends it, and the `ssh` found at the start, over
        // a repository's `core.sshCommand`.
        .env_remove(crate::ssh_agent::VARIABLE)
        .env("GIT_SSH_COMMAND", ssh_command())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    // The crate's own tests never read the configuration of the machine they run on.
    #[cfg(test)]
    command
        .env("HOME", tests::home())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

/// Runs `command`, cut after `time` or past `output` bytes on either stream.
async fn bounded<S: AsRef<OsStr>>(
    mut command: tokio::process::Command,
    dir: &Path,
    args: &[S],
    time: Duration,
    output: u64,
    input: Option<Vec<u8>>,
) -> Result<Output, String> {
    let mut child = crate::children::spawn(&mut command).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "git is not on the PATH".to_owned(),
        _ => format!("cannot run git: {error}"),
    })?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stdin = child.stdin.take();
    // Written beside the reads: git may write before it has read all of it.
    let write = async move {
        if let (Some(mut stdin), Some(input)) = (stdin, input) {
            use tokio::io::AsyncWriteExt;
            // A git that exits before reading it all is answered by its exit, not by this.
            let _ = stdin.write_all(&input).await;
        }
        Ok::<(), Cut>(())
    };
    // The first stream past its limit stops the call: git would block on a full pipe.
    let ran = tokio::time::timeout(time, async {
        tokio::try_join!(
            read(&mut stdout, output),
            read(&mut stderr, output),
            write,
            async { child.wait().await.map_err(Cut::Failed) },
        )
    })
    .await;
    let shown = || {
        args.iter()
            .map(|arg| arg.as_ref().to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let cut = |what: &str| format!("git {} in {} was stopped: {what}", shown(), dir.display());
    match ran {
        Ok(Ok((stdout, stderr, (), status))) => Ok(Output {
            success: status.success(),
            stdout,
            stderr,
        }),
        Ok(Err(Cut::TooLong)) => {
            kill(&mut child).await;
            Err(cut(&format!("it wrote more than {}", size(output))))
        }
        Ok(Err(Cut::Failed(error))) => {
            kill(&mut child).await;
            Err(format!("git {} in {}: {error}", shown(), dir.display()))
        }
        Err(_) => {
            kill(&mut child).await;
            Err(cut(&format!("it took more than {} s", time.as_secs())))
        }
    }
}

/// Runs git and fails, with git's own words, unless it succeeds: its standard output.
pub async fn output<S: AsRef<OsStr>>(
    dir: &Path,
    args: &[S],
    class: Class,
) -> Result<String, String> {
    output_with(dir, args, class, &[]).await
}

/// [`output`], with `env` set on git besides the daemon's own environment.
pub async fn output_with<S: AsRef<OsStr>>(
    dir: &Path,
    args: &[S],
    class: Class,
    env: &[(&str, &OsStr)],
) -> Result<String, String> {
    said(run_with(dir, args, class, env).await?)
}

/// What a call that ran to its end gave on success, or its error in git's own words, masked.
fn said(output: Output) -> Result<String, String> {
    if output.success {
        return Ok(output.stdout);
    }
    let said = output.stderr.trim();
    Err(if said.is_empty() {
        "git failed and said nothing".to_owned()
    } else {
        let env = std::env::vars_os().collect();
        flottille_core::secrets::mask(said, &env)
    })
}

/// Why a call stopped before its end, other than its time.
enum Cut {
    TooLong,
    Failed(std::io::Error),
}

/// One stream to its end, failing once it is past `limit` bytes.
async fn read(stream: &mut (impl AsyncRead + Unpin), limit: u64) -> Result<String, Cut> {
    let mut bytes = Vec::new();
    stream
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(Cut::Failed)?;
    if bytes.len() as u64 > limit {
        return Err(Cut::TooLong);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes.is_multiple_of(MB) {
        format!("{} MB", bytes / MB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Kills the call and its group, and waits until it has exited.
async fn kill(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        // SAFETY: kill has no precondition; the group is the child's own (process_group(0)).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An empty home of the tests' own, for every git they run.
    pub(crate) fn home() -> &'static Path {
        static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        HOME.get_or_init(|| tempfile::tempdir().expect("a temporary home"))
            .path()
    }

    /// `git` with no configuration of the user's.
    fn isolated(dir: &Path, args: &[&str]) -> tokio::process::Command {
        let mut command = command(dir, args);
        command
            .env("HOME", dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        command
    }

    async fn git(dir: &Path, args: &[&str]) -> String {
        let output = bounded(
            isolated(dir, args),
            dir,
            args,
            Duration::from_secs(30),
            1 << 20,
            None,
        )
        .await
        .unwrap();
        assert!(output.success, "{args:?}: {}", output.stderr);
        output.stdout.trim().to_owned()
    }

    #[test]
    fn the_protocol_of_a_remote_is_read_as_git_reads_its_url() {
        for (url, protocol) in [
            ("git@forge.invalid:acme/app.git", "ssh"),
            ("forge.invalid:acme/app.git", "ssh"),
            ("ssh://git@forge.invalid/acme/app.git", "ssh"),
            ("git+ssh://forge.invalid/acme/app.git", "ssh"),
            ("https://forge.invalid/acme/app.git", "https"),
            ("http://127.0.0.1:8080/app.git", "http"),
            ("git://forge.invalid/app.git", "git"),
            ("file:///srv/app.git", "file"),
            ("/srv/app.git", "file"),
            ("../app.git", "file"),
            ("./a:b", "file"),
            ("ext::ssh -W %h:%p forge.invalid", "ext"),
        ] {
            assert_eq!(super::protocol(url), protocol, "{url}");
        }
    }

    #[tokio::test]
    async fn an_output_past_its_limit_stops_git_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        git(dir, &["init", "--quiet"]).await;
        // More than a pipe holds: git blocks on writing once nobody reads.
        std::fs::write(dir.join("big"), "x".repeat(300 * 1024)).unwrap();
        let blob = git(dir, &["hash-object", "-w", "big"]).await;
        let args = ["cat-file", "-p", blob.as_str()];

        let started = std::time::Instant::now();
        let cut = bounded(
            isolated(dir, &args),
            dir,
            &args,
            Duration::from_secs(5),
            1024,
            None,
        )
        .await;

        let message = cut.err().expect("the call is cut");
        assert!(
            message.ends_with("it wrote more than 1024 bytes"),
            "{message}"
        );
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[tokio::test]
    async fn an_error_output_past_its_limit_is_cut_too() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        git(dir, &["init", "--quiet"]).await;
        // Git names the revision it cannot find: its error is longer than the limit.
        let missing = "a".repeat(5000);
        let args = ["show", "--end-of-options", missing.as_str()];

        let cut = bounded(
            isolated(dir, &args),
            dir,
            &args,
            Duration::from_secs(5),
            1024,
            None,
        )
        .await;

        let message = cut.err().expect("the call is cut");
        assert!(
            message.ends_with("it wrote more than 1024 bytes"),
            "{message}"
        );
    }
}
