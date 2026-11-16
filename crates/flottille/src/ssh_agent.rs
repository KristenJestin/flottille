//! The user's SSH agent, lent to a run that is itself a `git push` the gate let through, and to
//! the dry run made for its checks (`docs/vault.md`, "The runtime folder"): its environment names
//! the socket, and Landlock lets it connect to that socket alone. Every other run gets no
//! `SSH_AUTH_SOCK` (`runs::defend`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
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

/// The agent, and the `git` and `ssh` that speak to it, found once, at the daemon's start.
struct Found {
    /// What `SSH_AUTH_SOCK` named, perhaps a link.
    named: PathBuf,
    socket: Lent,
    /// The `git` a push lent the agent, and its reads, run.
    git: PathBuf,
    /// The `ssh` a push lent the agent runs.
    ssh: PathBuf,
    /// The `PATH` it runs with: the daemon's folders `program` would take a program from.
    search: OsString,
}

static FOUND: OnceLock<Option<Found>> = OnceLock::new();

/// Finds the user's SSH agent, its `git` and its `ssh`, at the daemon's start, before anything
/// runs.
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

/// The agent the daemon's environment names, and a `git` and an `ssh` for it; why not, when it
/// names one.
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
    let git = program(paths, "git").map_err(Some)?;
    let ssh = program(paths, "ssh").map_err(Some)?;
    let search = std::env::join_paths(system_folders(paths)).unwrap_or_default();
    Ok(Found {
        named,
        socket: Lent {
            path,
            id: (socket.dev(), socket.ino()),
        },
        git,
        ssh,
        search,
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

/// The user's own folders (the home, where an agent's folders and workspaces are) and
/// Flottille's, resolved.
fn theirs(paths: &Paths) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.iter()
        .chain([&paths.data, &paths.state, &paths.config, &paths.runtime])
        .map(|folder| folder.canonicalize().unwrap_or_else(|_| folder.clone()))
        .collect()
}

/// The folders of the daemon's `PATH`, resolved, that are none of `theirs` (`~/bin`,
/// `~/.local/bin`, `~/.cargo/bin`…) and that the daemon's user cannot write, nor any folder above
/// them: a run could put a program of its own there. The run's own `PATH`, which a project sets,
/// is not read.
fn system_folders(paths: &Paths) -> Vec<PathBuf> {
    let theirs = theirs(paths);
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|folder| folder.is_absolute())
        .filter_map(|folder| folder.canonicalize().ok())
        .filter(|folder| !theirs.iter().any(|their| folder.starts_with(their)))
        .filter(|folder| writable(folder).is_none())
        .collect()
}

/// The first `name` (`git`, `ssh`) of the daemon's `PATH` that is none of the user's own nor in
/// Flottille's folders (`theirs`), links followed; refused when the daemon's user can write it or
/// a folder above it (a Homebrew or a `/usr/local` of the user's), where a run could replace it.
fn program(paths: &Paths, name: &str) -> Result<PathBuf, String> {
    let theirs = theirs(paths);
    let found = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|folder| folder.is_absolute())
        .filter_map(|folder| folder.join(name).canonicalize().ok())
        .filter(|found| !theirs.iter().any(|folder| found.starts_with(folder)))
        .find(|found| {
            std::fs::metadata(found)
                .is_ok_and(|found| found.is_file() && found.permissions().mode() & 0o111 != 0)
        })
        .ok_or_else(|| {
            format!("no {name} on PATH outside the user's home and Flottille's folders")
        })?;
    match writable(&found) {
        None => Ok(found),
        Some(writable) => Err(format!(
            "the {name} found on PATH ({}) could be replaced: the user can write {}",
            found.display(),
            writable.display()
        )),
    }
}

/// The first of `path` (resolved) and the folders above it that the daemon's user can write;
/// none when it is none of them. A test build takes the folder `FLOTTILLE_TEST_TRUSTED_PROGRAMS`
/// names as one it cannot: its fakes live in temporary folders.
fn writable(path: &Path) -> Option<PathBuf> {
    if cfg!(debug_assertions)
        && let Some(trusted) = std::env::var_os("FLOTTILLE_TEST_TRUSTED_PROGRAMS")
        && let Ok(trusted) = Path::new(&trusted).canonicalize()
        && path.starts_with(&trusted)
    {
        return None;
    }
    path.ancestors()
        .find(|step| {
            let Ok(step) = std::ffi::CString::new(step.as_os_str().as_bytes()) else {
                return true;
            };
            // SAFETY: a valid C string; faccessat only reads it.
            let access = unsafe {
                libc::faccessat(libc::AT_FDCWD, step.as_ptr(), libc::W_OK, libc::AT_EACCESS)
            };
            access == 0
        })
        .map(Path::to_path_buf)
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

/// What runs with the agent of a push `words` lent it (`env` names it), pinned: the `git` found at
/// the start in place of the line's, the `ssh` found then (`GIT_SSH_COMMAND` wins over
/// `core.sshCommand` and `GIT_SSH`), sharing no connection with another ssh and running no
/// `LocalCommand`, SSH the only transport (no `ext::`, `fd::` or path, which run a program of the
/// machine), git's own `git-receive-pack` for every remote, and no maintenance left running after
/// the push. What `ssh` runs from its
/// configuration (`ProxyCommand`, `Match exec`, `KnownHostsCommand`) goes through `$SHELL -c`
/// and finds its programs on `PATH`: `/bin/sh`, and the system's folders alone. Settings and
/// global options on the line itself are refused before (`runs::run`): they would beat these.
pub async fn pin(env: &mut BTreeMap<OsString, OsString>, folder: &Path, words: &mut [String]) {
    let Some(found) = FOUND.get().and_then(Option::as_ref) else {
        return;
    };
    if lent_to(env).is_none() {
        return;
    }
    words[0] = found.git.to_string_lossy().into_owned();
    let ssh = found.ssh.to_string_lossy().replace('\'', "'\\''");
    // A master another ssh could reuse would outlive the push, the agent's connection with it.
    env.insert(
        "GIT_SSH_COMMAND".into(),
        format!("'{ssh}' -o ControlMaster=no -o ControlPath=none -o PermitLocalCommand=no").into(),
    );
    env.insert("GIT_ALLOW_PROTOCOL".into(), "ssh".into());
    // zsh reads `~/.zshenv` at every `-c`, and a run may write the home.
    env.insert("SHELL".into(), "/bin/sh".into());
    env.insert("PATH".into(), found.search.clone());
    // `git maintenance run --auto --detach` would go on after the push, with its environment.
    crate::runs::configure(env, "maintenance.auto", "false");
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
