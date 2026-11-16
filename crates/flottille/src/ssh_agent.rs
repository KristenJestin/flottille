//! The user's SSH agent, lent to a run that is itself a `git push` the gate let through, and to
//! the git reads made for it (`docs/vault.md`, "The runtime folder"): its environment names the
//! socket, and Landlock lets it connect to that socket alone. Every other run gets no
//! `SSH_AUTH_SOCK` (`runs::defend`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use flottille_core::paths::Paths;

pub const VARIABLE: &str = "SSH_AUTH_SOCK";

/// The socket of the user's SSH agent, as the daemon's own environment named it at its start,
/// resolved then: never a path a run or an agent gives, nor one a link leads to later. Its device
/// and inode with it, so that a socket put in its place is not lent.
#[derive(Clone, Debug)]
pub struct Lent {
    path: PathBuf,
    id: (u64, u64),
}

impl Lent {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The socket's device and inode.
    pub fn id(&self) -> (u64, u64) {
        self.id
    }
}

/// The agent and the `ssh` that speaks to it, found once, at the daemon's start.
struct Found {
    /// What `SSH_AUTH_SOCK` named, perhaps a link.
    named: PathBuf,
    socket: Lent,
    /// The `ssh` a push lent the agent runs.
    ssh: PathBuf,
}

static FOUND: OnceLock<Option<Found>> = OnceLock::new();

/// Finds the user's SSH agent and its `ssh`, at the daemon's start, before anything runs.
pub fn find(paths: &Paths) {
    FOUND.get_or_init(|| match found(paths) {
        Ok(found) => Some(found),
        Err(why) => {
            if let Some(why) = why {
                eprintln!("flottille daemon: no push is lent the SSH agent: {why}");
            }
            None
        }
    });
}

/// The agent the daemon's environment names, and an `ssh` for it; why not, when it names one.
fn found(paths: &Paths) -> Result<Found, Option<String>> {
    let named = PathBuf::from(std::env::var_os(VARIABLE).ok_or(None)?);
    let unusable = || Some(format!("{VARIABLE} ({}) is no socket", named.display()));
    let path = named.canonicalize().map_err(|_| unusable())?;
    let socket = std::fs::symlink_metadata(&path).map_err(|_| unusable())?;
    if !socket.file_type().is_socket() {
        return Err(unusable());
    }
    // The session bus leads to the keyring, where the vault is.
    if buses().contains(&path) {
        return Err(Some(format!(
            "{VARIABLE} leads to the session bus ({})",
            path.display()
        )));
    }
    let ssh = ssh(paths).ok_or_else(|| {
        Some("no ssh on PATH outside the user's home and Flottille's folders".to_owned())
    })?;
    Ok(Found {
        named,
        socket: Lent {
            path,
            id: (socket.dev(), socket.ino()),
        },
        ssh,
    })
}

/// The session bus's sockets: the runtime folder's `bus`, and the path of its address.
fn buses() -> Vec<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(|folder| Path::new(&folder).join("bus"));
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let addressed = address
        .split(';')
        .filter_map(|address| address.strip_prefix("unix:"))
        .flat_map(|keys| keys.split(','))
        .filter_map(|key| key.strip_prefix("path="))
        .map(PathBuf::from);
    runtime
        .into_iter()
        .chain(addressed)
        .filter_map(|bus| bus.canonicalize().ok())
        .collect()
}

/// The first `ssh` of the daemon's `PATH` that is none of the user's own (`~/bin`,
/// `~/.local/bin`, anything under the home, where an agent's folders and workspaces are) nor in
/// Flottille's folders, links followed: a program an agent could write is never run with the
/// agent's socket.
fn ssh(paths: &Paths) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let theirs: Vec<PathBuf> = home
        .iter()
        .chain([&paths.data, &paths.state, &paths.config, &paths.runtime])
        .map(|folder| folder.canonicalize().unwrap_or_else(|_| folder.clone()))
        .collect();
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|folder| folder.is_absolute())
        .filter_map(|folder| folder.join("ssh").canonicalize().ok())
        .filter(|ssh| !theirs.iter().any(|folder| ssh.starts_with(folder)))
        .find(|ssh| {
            std::fs::metadata(ssh)
                .is_ok_and(|ssh| ssh.is_file() && ssh.permissions().mode() & 0o111 != 0)
        })
}

/// The socket found at the start, which every command but a push lent it is kept from.
pub fn socket() -> Option<&'static Path> {
    Some(FOUND.get()?.as_ref()?.socket.path())
}

/// The daemon's SSH agent, when what its environment named still leads to the socket found at
/// the start, and that socket is still there: a link moved since, or a socket put in its place,
/// lends nothing.
pub fn lend() -> Option<Lent> {
    let found = FOUND.get()?.as_ref()?;
    let same = |socket: std::fs::Metadata| {
        socket.file_type().is_socket() && (socket.dev(), socket.ino()) == found.socket.id
    };
    let named = std::fs::metadata(&found.named).is_ok_and(same);
    let there = std::fs::symlink_metadata(found.socket.path()).is_ok_and(same);
    (named && there).then(|| found.socket.clone())
}

/// The agent lent to a command of `env`: only `runs::environment` of a push the gate let through
/// names it there, with the path `lend` resolved. Any other value lends nothing.
pub fn lent_to(env: &BTreeMap<OsString, OsString>) -> Option<Lent> {
    let named = env.get(&OsString::from(VARIABLE))?;
    lend().filter(|lent| lent.path().as_os_str() == named)
}

/// `env` without the agent: for what a push's run starts besides the push (its probes).
pub fn withheld(env: &BTreeMap<OsString, OsString>) -> BTreeMap<OsString, OsString> {
    let mut env = env.clone();
    env.remove(&OsString::from(VARIABLE));
    env
}

/// What runs with the agent of a push `words` lent it (`env` names it), pinned: the `ssh` found at
/// the start (`GIT_SSH_COMMAND` wins over `core.sshCommand` and `GIT_SSH`), SSH the only
/// transport (no `ext::`, `fd::` or path, which run a program of the machine), and git's own
/// `git-receive-pack` for every remote. Settings on the line itself are refused before
/// (`runs::run`): they would beat these.
pub async fn pin(env: &mut BTreeMap<OsString, OsString>, folder: &Path, words: &[String]) {
    let Some(found) = FOUND.get().and_then(Option::as_ref) else {
        return;
    };
    if lent_to(env).is_none() {
        return;
    }
    let ssh = found.ssh.to_string_lossy().replace('\'', "'\\''");
    env.insert("GIT_SSH_COMMAND".into(), format!("'{ssh}'").into());
    env.insert("GIT_ALLOW_PROTOCOL".into(), "ssh".into());
    let at_push = crate::gate::git_globals(&words[1..]).0 + 1;
    let mut read: Vec<String> = words[..at_push].to_vec();
    read.extend(
        [
            "config",
            "--name-only",
            "--get-regexp",
            r"^remote\..*\.(url|pushurl)$",
        ]
        .map(String::from),
    );
    let remotes = crate::git::run_in(folder, &read, &withheld(env), None)
        .await
        .map(|read| read.stdout)
        .unwrap_or_default();
    let mut names: Vec<&str> = remotes
        .lines()
        .filter_map(|key| key.strip_prefix("remote."))
        .filter_map(|key| key.rsplit_once('.').map(|(name, _)| name))
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        crate::runs::configure(
            env,
            &format!("remote.{name}.receivepack"),
            "git-receive-pack",
        );
    }
}

/// Where an HTTP(S) push URL is sent in the dry run of `over_ssh`: a path that is no repository.
const HTTP_MARK: &str = "/dev/null/flottille-http-push/";

/// Refuses a push `words` (`git`, its global options, `push`, the rest) that goes to an HTTP(S)
/// URL: only the SSH agent is lent, so it could not sign in. Git says where the push goes itself,
/// in a dry run where each HTTP(S) push URL is rewritten to a path that says so and no transport
/// but a local one is allowed: nothing leaves the machine. A push it cannot tell goes on, and
/// fails or not as git says; so does one whose options do not read, never dry run.
pub async fn over_ssh(
    folder: &Path,
    words: &[String],
    env: &BTreeMap<OsString, OsString>,
) -> Result<(), String> {
    // A path is allowed here, so a repository of the agent's may answer, running its own config's
    // commands (`core.alternateRefsCommand`): the dry run needs no agent and is lent none.
    let mut probe = withheld(env);
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
    // No dry run of a line whose options do not read: it could be the push itself.
    if !crate::message_checks::push_options_read(&rest[..end]) {
        return Ok(());
    }
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
