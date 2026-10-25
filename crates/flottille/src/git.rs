//! The user's own `git`, spawned without a shell (`docs/reference/hemera.md` §6, "Git invocation
//! rules"): always `-C <folder>`, never a prompt, every call bounded in time and in output, and a
//! call cut at its limit killed with its group and waited for.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

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
    bounded(command(dir, args), dir, args, class.limit(), OUTPUT_LIMIT).await
}

fn command<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        // A fetch that needs credentials fails at once; stored credentials still work.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    command
}

/// Runs `command`, cut after `time` or past `output` bytes on either stream.
async fn bounded<S: AsRef<OsStr>>(
    mut command: tokio::process::Command,
    dir: &Path,
    args: &[S],
    time: Duration,
    output: u64,
) -> Result<Output, String> {
    let mut child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "git is not on the PATH".to_owned(),
        _ => format!("cannot run git: {error}"),
    })?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    // The first stream past its limit stops the call: git would block on a full pipe.
    let ran = tokio::time::timeout(time, async {
        tokio::try_join!(
            read(&mut stdout, output),
            read(&mut stderr, output),
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
        Ok(Ok((stdout, stderr, status))) => Ok(Output {
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
    let output = run(dir, args, class).await?;
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
mod tests {
    use super::*;

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
        )
        .await
        .unwrap();
        assert!(output.success, "{args:?}: {}", output.stderr);
        output.stdout.trim().to_owned()
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
        )
        .await;

        let message = cut.err().expect("the call is cut");
        assert!(
            message.ends_with("it wrote more than 1024 bytes"),
            "{message}"
        );
    }
}
