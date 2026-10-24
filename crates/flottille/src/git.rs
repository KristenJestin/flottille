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
    let mut child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "git is not on the PATH".to_owned(),
        _ => format!("cannot run git: {error}"),
    })?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let ran = tokio::time::timeout(class.limit(), async {
        let (out, err, status) = tokio::join!(read(&mut stdout), read(&mut stderr), child.wait());
        (out, err, status)
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
        Ok((Ok(stdout), Ok(stderr), Ok(status))) => {
            if stdout.len() as u64 >= OUTPUT_LIMIT {
                return Err(cut("it wrote more than 32 MB"));
            }
            Ok(Output {
                success: status.success(),
                stdout,
                stderr,
            })
        }
        Ok((_, _, Err(error))) | Ok((Err(error), _, _)) | Ok((_, Err(error), _)) => {
            kill(&mut child).await;
            Err(format!("git {} in {}: {error}", shown(), dir.display()))
        }
        Err(_) => {
            kill(&mut child).await;
            Err(cut(&format!(
                "it took more than {} s",
                class.limit().as_secs()
            )))
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

async fn read(stream: &mut (impl AsyncRead + Unpin)) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    stream.take(OUTPUT_LIMIT).read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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
