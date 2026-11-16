//! The user's SSH agent, lent to a run that is itself a `git push` the gate let through, and to
//! the git reads made for it (`docs/vault.md`, "The runtime folder"): its environment names the
//! socket, and Landlock lets it connect to that socket alone. Every other run gets no
//! `SSH_AUTH_SOCK` (`runs::defend`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

pub const VARIABLE: &str = "SSH_AUTH_SOCK";

/// The socket of the user's SSH agent, as the daemon's own environment names it, resolved by the
/// daemon: never a path a run or an agent gives. One path, so that a run's own mount namespace can
/// bind it back.
#[derive(Clone, Debug)]
pub struct Lent(PathBuf);

impl Lent {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// The daemon's SSH agent, when its environment names a socket that exists.
pub fn lend() -> Option<Lent> {
    let named = std::env::var_os(VARIABLE)?;
    let socket = Path::new(&named).canonicalize().ok()?;
    let socket_file = std::fs::metadata(&socket).ok()?.file_type().is_socket();
    socket_file.then_some(Lent(socket))
}

/// The agent lent to a command of `env`: only `runs::environment` of a push the gate let through
/// names it there, with the path `lend` resolved. Any other value lends nothing.
pub fn lent_to(env: &BTreeMap<OsString, OsString>) -> Option<Lent> {
    let named = env.get(&OsString::from(VARIABLE))?;
    lend().filter(|lent| lent.path().as_os_str() == named)
}

/// Where an HTTP(S) push URL is sent in the dry run of `over_ssh`: a path that is no repository.
const HTTP_MARK: &str = "/dev/null/flottille-http-push/";

/// Refuses a push `words` (`git`, its global options, `push`, the rest) that goes to an HTTP(S)
/// URL: only the SSH agent is lent, so it could not sign in. Git says where the push goes itself,
/// in a dry run where each HTTP(S) push URL is rewritten to a path that says so and no transport
/// but a local one is allowed: nothing leaves the machine. A push it cannot tell goes on, and
/// fails or not as git says.
pub async fn over_ssh(
    folder: &Path,
    words: &[String],
    env: &BTreeMap<OsString, OsString>,
) -> Result<(), String> {
    let mut probe = env.clone();
    for scheme in ["https://", "http://"] {
        crate::runs::configure(
            &mut probe,
            &format!("url.{HTTP_MARK}.pushInsteadOf"),
            scheme,
        );
    }
    probe.insert("GIT_ALLOW_PROTOCOL".into(), "file".into());
    let at_push = crate::gate::git_globals(&words[1..]).0 + 1;
    let rest = &words[at_push + 1..];
    let end = rest
        .iter()
        .position(|word| word == "--" || word == "--end-of-options")
        .unwrap_or(rest.len());
    match crate::message_checks::dry_run(&words[..at_push], rest, end, folder, &probe).await {
        Err(why) if why.contains(HTTP_MARK) => Err(
            "refused: this push goes to an HTTP(S) URL, and a push signs in only through the \
             user's SSH agent: give the remote an SSH push URL (git remote set-url --push \
             <remote> git@<host>:<owner>/<repository>.git), or ask the user to push by hand; \
             nothing was pushed"
                .into(),
        ),
        _ => Ok(()),
    }
}
