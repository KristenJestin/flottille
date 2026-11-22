//! Signing the agents and the forge programs in (#1185, `docs/agents.md`, "Signing in"): each
//! program says whether it is signed in by its own status, and the user starts its **own**
//! sign-in from the window, finished in their browser. Flottille provides no login of its own and
//! keeps nothing of one: of a status it reads the exit code and the account's name; of a sign-in,
//! the address to open and the one-time code, held in memory while the flow runs and given in
//! the answers only (never in an event, the store or a log); a pasted code is written once to the
//! program's input and dropped. Every program is spawned without a shell.
//!
//! The methods are the user's (`drive`, `docs/protocol.md`, "The sign-in methods"): no tool of an
//! agent reaches them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use flottille_core::declared::{self, ProjectTracker, RepositoryForge};
use flottille_core::registry::Registry;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWriteExt as _, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::Notify;

use crate::daemon::Daemon;
use crate::forge::masked;
use crate::projects::Failure;

type Answer = Result<Value, Failure>;

/// A status or a sign-out is given this long.
const STATUS_TIME: Duration = Duration::from_secs(30);
/// A sign-in is given this long: a device code lasts 15 minutes.
const FLOW_TIME: Duration = Duration::from_secs(15 * 60);
/// `signin.start` waits this long for the address and the code before it answers.
const SAID_TIME: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Program {
    Claude,
    Opencode,
    Codex,
    Gh,
    Glab,
    Acli,
    Bkt,
}

/// The agents first, in the providers' order, then the forges and trackers.
const ALL: [Program; 7] = [
    Program::Claude,
    Program::Opencode,
    Program::Codex,
    Program::Gh,
    Program::Glab,
    Program::Acli,
    Program::Bkt,
];

/// How a program signs in or out.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Way {
    /// Its own command, run here: its words, and whether it asks for a code back.
    Run { args: Vec<String>, asks_code: bool },
    /// It needs a terminal: the line the user types in one, on this machine.
    Terminal(String),
}

impl Program {
    pub fn name(self) -> &'static str {
        match self {
            Program::Claude => "claude",
            Program::Opencode => "opencode",
            Program::Codex => "codex",
            Program::Gh => "gh",
            Program::Glab => "glab",
            Program::Acli => "acli",
            Program::Bkt => "bkt",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Program::Claude => "Claude",
            Program::Opencode => "OpenCode",
            Program::Codex => "Codex",
            Program::Gh => "GitHub CLI",
            Program::Glab => "GitLab CLI",
            Program::Acli => "Atlassian CLI",
            Program::Bkt => "Bitbucket CLI",
        }
    }

    fn of(name: &str) -> Option<Program> {
        ALL.into_iter().find(|program| program.name() == name)
    }

    fn is_agent(self) -> bool {
        matches!(self, Program::Claude | Program::Opencode | Program::Codex)
    }

    /// The host a forge program is checked and signed in to when no project names one.
    fn default_host(self) -> Option<&'static str> {
        match self {
            Program::Gh => Some(declared::GITHUB),
            Program::Glab => Some("gitlab.com"),
            Program::Bkt => Some(declared::BITBUCKET_CLOUD),
            _ => None,
        }
    }

    /// The program's own sign-in, from its documentation (`docs/agents.md`, "Signing in").
    fn sign_in(self, host: Option<&str>) -> Way {
        let run = |args: &[&str], asks_code| Way::Run {
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            asks_code,
        };
        let host = host.unwrap_or_default();
        match self {
            // Its browser shows a code to paste when it cannot call back here.
            Program::Claude => run(&["auth", "login"], true),
            // The device code flow, made for a machine without a browser.
            Program::Codex => run(&["login", "--device-auth"], false),
            Program::Opencode => Way::Terminal("opencode auth login".into()),
            Program::Gh => run(
                &[
                    "auth",
                    "login",
                    "--web",
                    "--hostname",
                    host,
                    "--git-protocol",
                    "https",
                ],
                false,
            ),
            Program::Glab => run(&["auth", "login", "--hostname", host, "--device"], false),
            Program::Acli => run(&["jira", "auth", "login", "--web"], false),
            Program::Bkt if host == declared::BITBUCKET_CLOUD => run(
                &[
                    "auth",
                    "login",
                    "https://bitbucket.org",
                    "--kind",
                    "cloud",
                    "--web",
                ],
                false,
            ),
            // Data Center signs in with a token typed in a terminal.
            Program::Bkt => Way::Terminal(format!("bkt auth login https://{host} --web-token")),
        }
    }

    /// The program's own sign-out.
    fn sign_out(self, host: Option<&str>) -> Way {
        let run = |args: Vec<&str>| Way::Run {
            args: args.into_iter().map(str::to_owned).collect(),
            asks_code: false,
        };
        let host = host.unwrap_or_default();
        match self {
            Program::Claude => run(vec!["auth", "logout"]),
            Program::Codex => run(vec!["logout"]),
            Program::Opencode => Way::Terminal("opencode auth logout".into()),
            Program::Gh => run(vec!["auth", "logout", "--hostname", host]),
            Program::Glab => run(vec!["auth", "logout", "--hostname", host]),
            Program::Acli => run(vec!["jira", "auth", "logout"]),
            // Bitbucket Cloud's key is its API's host; `bkt` removes its contexts with it.
            Program::Bkt if host == declared::BITBUCKET_CLOUD => {
                run(vec!["auth", "logout", "api.bitbucket.org"])
            }
            Program::Bkt => run(vec!["auth", "logout", host]),
        }
    }
}

/// Where the sign-in finds `program`: on the daemon's PATH, or in the test's folder of programs
/// alone when `FLOTTILLE_TEST_PROGRAMS` names one in a test build, so that no test ever runs a
/// program the machine has installed (a release build never reads it).
fn locate(program: Program) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let tests = std::env::var_os("FLOTTILLE_TEST_PROGRAMS").filter(|_| cfg!(debug_assertions));
    let path = match tests {
        Some(folder) => folder,
        None => std::env::var_os("PATH")?,
    };
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(program.name()))
        .find(|file| {
            std::fs::metadata(file)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// A command of `program` at `path`, without a shell, with the daemon's environment (the user's
/// own sign-in) and nothing that waits for an answer the window cannot give.
fn command(path: &PathBuf, args: &[String]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(path);
    command
        .args(args)
        .env("NO_COLOR", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GLAB_NO_PROMPT", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    command
}

/// `program <args>`, cut after `time`: whether it ended with 0 and what it printed on its output
/// and errors, which the caller reads for a word and drops. `None` when it could not run or took
/// too long.
async fn ran(path: &PathBuf, args: &[String], time: Duration) -> Option<(bool, String)> {
    let mut command = command(path, args);
    command.stdin(Stdio::null());
    let child = crate::children::spawn(&mut command).ok()?;
    let pid = child.id();
    let Ok(output) = tokio::time::timeout(time, child.wait_with_output()).await else {
        // What it started itself is in its group, and outlives the child the timeout dropped.
        if let Some(pid) = pid {
            crate::agents::signal_group(pid, libc::SIGKILL);
        }
        return None;
    };
    let output = output.ok()?;
    let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
    said.push('\n');
    said.push_str(&String::from_utf8_lossy(&output.stderr));
    Some((output.status.success(), said))
}

/// What a program says of its sign-in: signed in or not (`None` when it could not say), and the
/// account's name when it says one.
struct Status {
    signed_in: Option<bool>,
    account: Option<String>,
}

/// The account `gh auth status` names for `host`: the word after `account` on its line.
fn gh_account(said: &str, host: &str) -> Option<String> {
    let line = said
        .lines()
        .find(|line| line.contains(&format!("Logged in to {host} account ")))?;
    let after = line.split(" account ").nth(1)?;
    after.split_whitespace().next().map(str::to_owned)
}

/// `program`'s own status for `host` (`docs/agents.md`, "Signing in"): its exit code, and of what
/// it prints the account's name alone, masked; the rest is dropped here.
async fn status(program: Program, path: &PathBuf, host: Option<&str>) -> Status {
    let args = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    let host_or = host.unwrap_or_default();
    let unknown = Status {
        signed_in: None,
        account: None,
    };
    let said = match program {
        // OpenCode's status is a list asked in a terminal: its login file is checked for its
        // presence only, as the providers' check does.
        Program::Opencode => {
            return Status {
                signed_in: Some(crate::opencode::login_file().is_some_and(|file| file.exists())),
                account: None,
            };
        }
        Program::Claude => ran(path, &args(&["auth", "status"]), STATUS_TIME).await,
        Program::Codex => ran(path, &args(&["login", "status"]), STATUS_TIME).await,
        Program::Gh => {
            ran(
                path,
                &args(&["auth", "status", "--hostname", host_or]),
                STATUS_TIME,
            )
            .await
        }
        Program::Glab => {
            ran(
                path,
                &args(&["auth", "status", "--hostname", host_or]),
                STATUS_TIME,
            )
            .await
        }
        Program::Acli => ran(path, &args(&["jira", "auth", "status"]), STATUS_TIME).await,
        // Signed in is a context for the host (`docs/forges.md`, "Bitbucket"), never `bkt auth`.
        Program::Bkt => {
            let Some((true, said)) =
                ran(path, &args(&["context", "list", "--json"]), STATUS_TIME).await
            else {
                return unknown;
            };
            let listed = said
                .lines()
                .find_map(|line| serde_json::from_str::<Value>(line).ok())
                .unwrap_or_default();
            return Status {
                signed_in: Some(crate::forge::bitbucket_lists_host(&listed, host_or)),
                account: None,
            };
        }
    };
    let Some((ok, said)) = said else {
        return unknown;
    };
    let account = match (program, ok) {
        (Program::Gh, true) => gh_account(&said, host_or),
        (Program::Claude, true) => said
            .lines()
            .find_map(|line| serde_json::from_str::<Value>(line).ok())
            .and_then(|status| status["email"].as_str().map(str::to_owned)),
        _ => None,
    };
    Status {
        signed_in: Some(ok),
        account: account.as_deref().and_then(account_masked),
    }
}

/// What is shown of an account's name: its first letter, and its domain's first letter for an
/// address (`octocat` is `o***`, `someone@example.com` is `s***@e***`).
fn account_masked(account: &str) -> Option<String> {
    let first = |part: &str| part.chars().next().map(|letter| format!("{letter}***"));
    match account.trim().split_once('@') {
        Some((name, domain)) => Some(format!("{}@{}", first(name)?, first(domain)?)),
        None => first(account.trim()),
    }
}

/// The hosts the projects use, by program, in the order they first use them: the programs the
/// projects use come first among the forges, each with its first host.
fn used_hosts(daemon: &Daemon) -> Vec<(Program, String)> {
    let mut used: Vec<(Program, String)> = Vec::new();
    let Ok(loaded) = Registry::new(&daemon.paths).load_all() else {
        return used;
    };
    let mut push = |program: Program, host: &str| {
        if !used.iter().any(|(known, _)| *known == program) {
            used.push((program, host.to_owned()));
        }
    };
    for project in &loaded.projects {
        for repository in &project.repositories {
            match &repository.forge {
                Some(forge @ RepositoryForge::Github { .. }) => push(Program::Gh, forge.host()),
                Some(forge) => push(Program::Bkt, forge.host()),
                None => {}
            }
        }
        match &project.tracker {
            Some(ProjectTracker::Github { .. }) => push(Program::Gh, declared::GITHUB),
            Some(ProjectTracker::Jira { site, .. }) => push(Program::Acli, site),
            None => {}
        }
    }
    used
}

/// The programs, in the order the window shows them, each with the host it is checked for.
fn programs(daemon: &Daemon) -> Vec<(Program, Option<String>)> {
    let used = used_hosts(daemon);
    let mut listed: Vec<(Program, Option<String>)> = ALL
        .into_iter()
        .filter(|program| program.is_agent())
        .map(|program| (program, None))
        .collect();
    listed.extend(
        used.iter()
            .map(|(program, host)| (*program, Some(host.clone()))),
    );
    listed.extend(
        ALL.into_iter()
            .filter(|program| !program.is_agent())
            .filter(|program| !used.iter().any(|(known, _)| known == program))
            .map(|program| (program, program.default_host().map(str::to_owned))),
    );
    listed
}

/// A sign-in that runs: what the user needs of it, held here only while it runs.
struct Running {
    /// This flow's own: what its readers and its follower touch, never a later flow of the same
    /// program.
    id: u64,
    /// The key of the device that started it, `None` for a client of this machine: only that
    /// caller is given its address and its code, or gives it a code or cancels it.
    starter: Option<flottille_link::PublicKey>,
    /// Where it was started from, said to another caller.
    from: String,
    host: Option<String>,
    address: Option<String>,
    code: Option<String>,
    asks_code: bool,
    /// The program prints a one-time code for the user to enter in the browser.
    shows_code: bool,
    /// The program's input, while a pasted code may still be written to it: taken by the first.
    input: Option<ChildStdin>,
    pid: u32,
    cancelled: bool,
    /// Woken when the address or the code is read, and when the flow ends.
    said: Arc<Notify>,
    ended: bool,
}

/// The sign-ins running, by program: one at a time for each.
fn running() -> &'static Mutex<HashMap<Program, Running>> {
    static RUNNING: OnceLock<Mutex<HashMap<Program, Running>>> = OnceLock::new();
    RUNNING.get_or_init(Mutex::default)
}

/// The id of the next sign-in.
static NEXT_FLOW: AtomicU64 = AtomicU64::new(1);

/// Whom the request being served is from: the key of the device that signed it, or `None` for a
/// client of this machine, and the name of that machine (`connection.login`'s rule).
fn caller(daemon: &Daemon) -> (Option<flottille_link::PublicKey>, String) {
    match crate::daemon::asked() {
        Some(asked) => (Some(asked.key), asked.device.clone()),
        None => (
            None,
            crate::link::remote_of(daemon)
                .ok()
                .and_then(|remote| remote.listed().ok().map(|(_, name)| name))
                .unwrap_or_else(|| "this machine".to_owned()),
        ),
    }
}

/// The refusal another caller than the one that started `flow` is given: that it runs, and from
/// where, never what it shows.
fn not_yours(program: Program, flow: &Running) -> Failure {
    Failure::new(
        "refused",
        format!(
            "a sign-in of {} runs, started from {}",
            program.name(),
            flow.from
        ),
    )
}

/// What a line of a sign-in says the user needs: the first address it holds, and the one-time
/// code after `one-time code`: after a colon (gh and glab), in parentheses (a recent gh), or
/// alone on the line under (Codex). Only a word that looks like a code is taken (below): with
/// none found, the dialog says so rather than show a wrong one (#1248).
#[derive(Default)]
struct Reading {
    address: Option<String>,
    code: Option<String>,
    /// The last line announced the code without holding it.
    code_next: bool,
}

/// Whether `word` looks like a one-time code: groups of capital letters and digits joined by
/// dashes, as gh, glab and Codex print theirs (`WDJB-MJHT`).
fn looks_like_a_code(word: &str) -> bool {
    let mut groups = word.split('-');
    let group = |group: &str| {
        !group.is_empty()
            && group
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    };
    groups.next().is_some_and(group) && word.contains('-') && groups.all(group)
}

/// The first word of `text` that looks like a code, without the punctuation around it.
fn code_in(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
        .find(|word| looks_like_a_code(word))
        .map(str::to_owned)
}

impl Reading {
    fn line(&mut self, line: &str) {
        let line = line.trim();
        if self.code_next && self.code.is_none() && !line.is_empty() {
            // The line under the announcement is the code, or it is not: never a guess.
            self.code = line.split_whitespace().next().and_then(code_in);
            self.code_next = false;
        }
        if self.address.is_none()
            && let Some(at) = line.find("https://").or_else(|| line.find("http://"))
        {
            self.address = line[at..]
                .split_whitespace()
                .next()
                .map(|address| masked(&declared::without_credentials(address)));
        }
        const SAYING: &str = "one-time code";
        if self.code.is_none()
            && let Some(at) = line.to_ascii_lowercase().find(SAYING)
        {
            self.code = code_in(&line[at + SAYING.len()..]);
            self.code_next = self.code.is_none();
        }
    }
}

/// Reads a stream of the sign-in line by line, keeping only the address and the code.
async fn read_stream(
    program: Program,
    id: u64,
    stream: impl AsyncRead + Unpin,
    reading: Arc<Mutex<Reading>>,
) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let (address, code) = {
            let mut reading = reading.lock().unwrap();
            reading.line(&line);
            (reading.address.clone(), reading.code.clone())
        };
        let mut running = running().lock().unwrap();
        if let Some(flow) = running.get_mut(&program).filter(|flow| flow.id == id) {
            let changed = flow.address != address || flow.code != code;
            flow.address = address;
            flow.code = code;
            if changed {
                flow.said.notify_waiters();
            }
        }
    }
}

/// The answer of a sign-in that runs: what the user needs, to this caller alone.
fn shown(program: Program, flow: &Running) -> Value {
    json!({
        "program": program.name(),
        "host": flow.host,
        "running": !flow.ended,
        "address": flow.address,
        "code": flow.code,
        "asks_code": flow.asks_code && flow.input.is_some(),
        "shows_code": flow.shows_code,
    })
}

/// Says that `program`'s sign-in changed: its name, its host and the state only, since every
/// event is stored (`docs/events.md`).
fn announce(daemon: &Daemon, program: Program, host: Option<&str>, state: &str, why: Option<&str>) {
    LIST_CHANGES.fetch_add(1, Ordering::SeqCst);
    let mut data = json!({"program": program.name(), "host": host, "state": state});
    if let Some(why) = why {
        data["why"] = json!(why);
    }
    let _ = daemon.emit("signin.changed", data);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Named {
    program: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Code {
    program: String,
    code: String,
}

fn program_of(name: &str) -> Result<Program, Failure> {
    Program::of(name).ok_or_else(|| {
        let names: Vec<&str> = ALL.iter().map(|program| program.name()).collect();
        Failure::new(
            "not-found",
            format!(
                "there is no program {name} to sign in: the programs are {}",
                names.join(", ")
            ),
        )
    })
}

/// The host the program is signed in to: the one its row is checked for.
fn host_of(daemon: &Daemon, program: Program) -> Option<String> {
    programs(daemon)
        .into_iter()
        .find(|(known, _)| *known == program)
        .and_then(|(_, host)| host)
}

/// How long a list's answer is given again to the callers that ask it, unless a sign-in changed.
const LIST_KEPT: Duration = Duration::from_secs(2);

/// Bumped by each change of a sign-in: a list read before it is not given again.
static LIST_CHANGES: AtomicU64 = AtomicU64::new(0);

/// The last list, when it was read and the change it was read at: one list runs at a time, and
/// the callers that ask meanwhile wait for it.
fn last_list() -> &'static tokio::sync::Mutex<Option<(tokio::time::Instant, u64, Value)>> {
    static LAST: OnceLock<tokio::sync::Mutex<Option<(tokio::time::Instant, u64, Value)>>> =
        OnceLock::new();
    LAST.get_or_init(Default::default)
}

/// `signin.list`: every program, its state and what can be done about it. Each row runs two
/// programs: callers that ask together or right after one another share one list.
async fn list(daemon: &Daemon) -> Value {
    let mut last = last_list().lock().await;
    let changes = LIST_CHANGES.load(Ordering::SeqCst);
    if let Some((at, read_at, listed)) = &*last
        && *read_at == changes
        && at.elapsed() < LIST_KEPT
    {
        return listed.clone();
    }
    let rows = futures::future::join_all(
        programs(daemon)
            .into_iter()
            .map(|(program, host)| row(program, host)),
    )
    .await;
    let listed = json!({"programs": rows});
    *last = Some((tokio::time::Instant::now(), changes, listed.clone()));
    listed
}

async fn row(program: Program, host: Option<String>) -> Value {
    let found = locate(program);
    let (version, status) = match &found {
        Some(path) => {
            let (version, status) = tokio::join!(
                crate::programs::version_of(path),
                status(program, path, host.as_deref())
            );
            (version, status)
        }
        None => (
            None,
            Status {
                signed_in: None,
                account: None,
            },
        ),
    };
    let line = |way: Way| match way {
        Way::Terminal(line) => Some(line),
        Way::Run { .. } => None,
    };
    let sign_in = line(program.sign_in(host.as_deref()));
    let sign_out = line(program.sign_out(host.as_deref()));
    let running = running()
        .lock()
        .unwrap()
        .get(&program)
        .is_some_and(|flow| !flow.ended);
    let installed = found.is_some();
    json!({
        "program": program.name(),
        "title": program.title(),
        "kind": if program.is_agent() { "agent" } else { "forge" },
        "host": host,
        "found": found.is_some(),
        "path": found.map(|path| path.display().to_string()),
        "version": version,
        "signed_in": status.signed_in,
        "account": status.account,
        "can_sign_in": installed && sign_in.is_none(),
        "line": sign_in,
        "sign_out_line": sign_out,
        "running": running,
    })
}

/// `signin.start`: starts the program's own sign-in, and answers once it said the address (and
/// the code it shows), or ended.
async fn start(daemon: Arc<Daemon>, program: Program) -> Answer {
    let host = host_of(&daemon, program);
    let Some(path) = locate(program) else {
        return Err(Failure::new(
            "refused",
            format!("{} is not installed on this machine", program.name()),
        ));
    };
    let (args, asks_code) = match program.sign_in(host.as_deref()) {
        Way::Run { args, asks_code } => (args, asks_code),
        Way::Terminal(line) => {
            return Err(Failure::new(
                "refused",
                format!(
                    "{} cannot sign in without a terminal: run `{line}` in a terminal on this \
                     machine",
                    program.name()
                ),
            ));
        }
    };
    let wants_code = !asks_code && !matches!(program, Program::Acli | Program::Bkt);
    let (starter, from) = caller(&daemon);
    let id = NEXT_FLOW.fetch_add(1, Ordering::Relaxed);
    let said = {
        let mut flows = running().lock().unwrap();
        match flows.get(&program) {
            Some(flow) if !flow.ended && flow.starter == starter => {
                return Ok(shown(program, flow));
            }
            Some(flow) if !flow.ended => return Err(not_yours(program, flow)),
            _ => {}
        }
        let mut command = command(&path, &args);
        command.stdin(match asks_code {
            true => Stdio::piped(),
            false => Stdio::null(),
        });
        let mut child = crate::children::spawn(&mut command).map_err(|error| {
            Failure::new(
                "unavailable",
                format!("{} could not start: {error}", program.name()),
            )
        })?;
        let said = Arc::new(Notify::new());
        let reading = Arc::new(Mutex::new(Reading::default()));
        if let Some(out) = child.stdout.take() {
            tokio::spawn(read_stream(program, id, out, reading.clone()));
        }
        if let Some(err) = child.stderr.take() {
            tokio::spawn(read_stream(program, id, err, reading));
        }
        flows.insert(
            program,
            Running {
                id,
                starter,
                from,
                host: host.clone(),
                address: None,
                code: None,
                asks_code,
                shows_code: wants_code,
                input: child.stdin.take(),
                pid: child.id().unwrap_or_default(),
                cancelled: false,
                said: said.clone(),
                ended: false,
            },
        );
        tokio::spawn(follow(daemon.clone(), program, id, path, child));
        said
    };
    announce(&daemon, program, host.as_deref(), "started", None);
    // The answer waits for what the user needs: the address, and the code a device flow shows.
    let deadline = tokio::time::Instant::now() + SAID_TIME;
    loop {
        let notified = said.notified();
        {
            let flows = running().lock().unwrap();
            let Some(flow) = flows.get(&program).filter(|flow| flow.id == id) else {
                break;
            };
            let ready = flow.address.is_some() && (flow.code.is_some() || !wants_code);
            if ready || flow.ended {
                return Ok(shown(program, flow));
            }
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            break;
        }
    }
    let flows = running().lock().unwrap();
    match flows.get(&program).filter(|flow| flow.id == id) {
        Some(flow) => Ok(shown(program, flow)),
        None => Ok(json!({"program": program.name(), "host": host, "running": false})),
    }
}

/// Waits for the sign-in's end (cut after `FLOW_TIME`), then says how it ended: signed in by the
/// program's own status, or why not, in a sentence of Flottille's (never what it printed).
async fn follow(
    daemon: Arc<Daemon>,
    program: Program,
    id: u64,
    path: PathBuf,
    mut child: tokio::process::Child,
) {
    let ended = tokio::time::timeout(FLOW_TIME, child.wait()).await;
    let pid = child.id();
    if ended.is_err() {
        if let Some(pid) = pid {
            crate::agents::signal_group(pid, libc::SIGKILL);
        }
        let _ = child.wait().await;
    }
    let (host, cancelled) = {
        let mut flows = running().lock().unwrap();
        match flows.get_mut(&program).filter(|flow| flow.id == id) {
            Some(flow) => {
                flow.ended = true;
                flow.input = None;
                flow.said.notify_waiters();
                (flow.host.clone(), flow.cancelled)
            }
            None => (None, false),
        }
    };
    let (state, why) = if cancelled {
        ("cancelled", None)
    } else if status(program, &path, host.as_deref()).await.signed_in == Some(true) {
        ("signed_in", None)
    } else {
        let why = match &ended {
            Err(_) => format!(
                "{}'s sign-in took more than {} minutes",
                program.name(),
                FLOW_TIME.as_secs() / 60
            ),
            Ok(Ok(exit)) if exit.success() => format!(
                "{}'s sign-in ended, but {} does not say it is signed in{}",
                program.name(),
                program.name(),
                host.as_deref()
                    .map(|host| format!(" to {host}"))
                    .unwrap_or_default()
            ),
            Ok(Ok(exit)) => format!(
                "{}'s sign-in ended without signing in (exit code {})",
                program.name(),
                exit.code()
                    .map_or_else(|| "none".to_owned(), |code| code.to_string())
            ),
            Ok(Err(error)) => format!(
                "{}'s sign-in could not be followed: {error}",
                program.name()
            ),
        };
        ("failed", Some(why))
    };
    {
        // A sign-in started since, while this one's status was read, is not this one's to end.
        let mut flows = running().lock().unwrap();
        if flows.get(&program).is_some_and(|flow| flow.id == id) {
            flows.remove(&program);
        }
    }
    announce(&daemon, program, host.as_deref(), state, why.as_deref());
}

/// `signin.code`: the code the user pasted, written once to the program's input, which closes.
async fn code(daemon: &Daemon, program: Program, code: String) -> Answer {
    let code = code.trim().to_owned();
    if code.is_empty() || code.contains(['\n', '\r']) || code.len() > 4096 {
        return Err(Failure::new(
            "invalid-params",
            "the code is one line of text, as the browser shows it",
        ));
    }
    let (starter, _) = caller(daemon);
    let input = {
        let mut flows = running().lock().unwrap();
        let Some(flow) = flows.get_mut(&program).filter(|flow| !flow.ended) else {
            return Err(Failure::new(
                "refused",
                format!("{} is not signing in", program.name()),
            ));
        };
        if flow.starter != starter {
            return Err(not_yours(program, flow));
        }
        if !flow.asks_code {
            return Err(Failure::new(
                "refused",
                format!("{}'s sign-in asks for no code", program.name()),
            ));
        }
        flow.input.take().ok_or_else(|| {
            Failure::new(
                "refused",
                format!("{}'s sign-in was already given its code", program.name()),
            )
        })?
    };
    let mut input = input;
    let mut line = code.into_bytes();
    line.push(b'\n');
    let written = input.write_all(&line).await;
    line.fill(0);
    // Its input closes with it: nothing more is ever written there.
    drop(input);
    written.map_err(|_| {
        Failure::new(
            "unavailable",
            format!("{}'s sign-in no longer reads its input", program.name()),
        )
    })?;
    Ok(json!({"program": program.name()}))
}

/// `signin.cancel`: stops the sign-in, its whole process group.
fn cancel(daemon: &Daemon, program: Program) -> Answer {
    let (starter, _) = caller(daemon);
    let mut flows = running().lock().unwrap();
    let Some(flow) = flows.get_mut(&program).filter(|flow| !flow.ended) else {
        return Err(Failure::new(
            "refused",
            format!("{} is not signing in", program.name()),
        ));
    };
    if flow.starter != starter {
        return Err(not_yours(program, flow));
    }
    flow.cancelled = true;
    flow.input = None;
    if flow.pid != 0 {
        crate::agents::signal_group(flow.pid, libc::SIGKILL);
    }
    Ok(json!({"program": program.name()}))
}

/// `signin.sign_out`: runs the program's own sign-out, then says its state.
async fn sign_out(daemon: &Daemon, program: Program) -> Answer {
    let host = host_of(daemon, program);
    let Some(path) = locate(program) else {
        return Err(Failure::new(
            "refused",
            format!("{} is not installed on this machine", program.name()),
        ));
    };
    let args = match program.sign_out(host.as_deref()) {
        Way::Run { args, .. } => args,
        Way::Terminal(line) => {
            return Err(Failure::new(
                "refused",
                format!(
                    "{} cannot sign out without a terminal: run `{line}` in a terminal on this \
                     machine",
                    program.name()
                ),
            ));
        }
    };
    let ended = ran(&path, &args, STATUS_TIME).await;
    let signed_in = status(program, &path, host.as_deref()).await.signed_in;
    if signed_in == Some(true) {
        let why = match ended {
            Some((false, _)) => format!("{}'s sign-out failed", program.name()),
            None => format!("{}'s sign-out did not end", program.name()),
            Some((true, _)) => format!("{} still says it is signed in", program.name()),
        };
        announce(daemon, program, host.as_deref(), "failed", Some(&why));
        return Err(Failure::new("unavailable", why));
    }
    announce(daemon, program, host.as_deref(), "signed_out", None);
    Ok(json!({"program": program.name(), "host": host, "signed_in": signed_in}))
}

/// Answers `signin.list`, `signin.start`, `signin.code`, `signin.cancel` and `signin.sign_out`
/// (`docs/protocol.md`, "The sign-in methods").
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Answer {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "signin.list" => Ok(list(&daemon).await),
        "signin.start" => {
            let Named { program } = serde_json::from_value(params)?;
            start(daemon, program_of(&program)?).await
        }
        "signin.code" => {
            let Code {
                program,
                code: given,
            } = serde_json::from_value(params)?;
            code(&daemon, program_of(&program)?, given).await
        }
        "signin.cancel" => {
            let Named { program } = serde_json::from_value(params)?;
            cancel(&daemon, program_of(&program)?)
        }
        "signin.sign_out" => {
            let Named { program } = serde_json::from_value(params)?;
            sign_out(&daemon, program_of(&program)?).await
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(lines: &[&str]) -> (Option<String>, Option<String>) {
        let mut reading = Reading::default();
        for line in lines {
            reading.line(line);
        }
        (reading.address, reading.code)
    }

    #[test]
    fn gh_says_the_code_after_a_colon_and_the_address_on_the_next_line() {
        assert_eq!(
            read(&[
                "! First copy your one-time code: ABCD-1234",
                "Open this URL to continue in your web browser: https://github.com/login/device",
            ]),
            (
                Some("https://github.com/login/device".into()),
                Some("ABCD-1234".into())
            )
        );
    }

    #[test]
    fn the_code_of_a_recent_gh_is_read_from_its_parentheses() {
        assert_eq!(
            read(&[
                "",
                "! One-time code (ABCD-1234) copied to clipboard",
                "Open this URL to continue in your web browser: https://github.com/login/device",
            ]),
            (
                Some("https://github.com/login/device".into()),
                Some("ABCD-1234".into())
            )
        );
    }

    #[test]
    fn the_code_of_an_older_gh_is_read_after_its_colon() {
        assert_eq!(
            read(&["! First copy your one-time code: ABCD-1234"]).1,
            Some("ABCD-1234".into())
        );
        // glab says it on a line of its own, the same way.
        assert_eq!(
            read(&["", "First copy your one-time code: WDJB-MJHT", ""]).1,
            Some("WDJB-MJHT".into())
        );
    }

    #[test]
    fn a_word_that_is_not_a_code_is_never_shown_as_one() {
        // What gh 2.104.0 prints where no clipboard utility is installed: the line that names the
        // code holds none and the line under it is a sentence ("No" was shown as the code); the
        // code comes on the line after.
        assert_eq!(
            read(&[
                "! Failed to copy one-time code to clipboard",
                "  No clipboard utilities available. Please install xsel, xclip, wl-clipboard",
                "! First copy your one-time code: ABCD-1234",
                "Open this URL to continue in your web browser: https://github.com/login/device",
            ])
            .1,
            Some("ABCD-1234".into())
        );
        // With no code printed at all, none is found: never the first word of the next line.
        assert_eq!(
            read(&[
                "! One-time code copied to clipboard",
                "Open this URL to continue in your web browser: https://github.com/login/device",
            ])
            .1,
            None
        );
        assert_eq!(read(&["one-time code: soon", "and then"]).1, None);
    }

    #[test]
    fn claude_says_the_address_on_the_line_under_and_no_code() {
        assert_eq!(
            read(&[
                "Opening browser to sign in…",
                "If the browser didn't open, visit:",
                "https://claude.example.test/oauth/authorize?state=x",
                "Paste code here if prompted >",
            ]),
            (
                Some("https://claude.example.test/oauth/authorize?state=x".into()),
                None
            )
        );
    }

    #[test]
    fn codex_says_the_code_on_the_line_under() {
        assert_eq!(
            read(&[
                "1. Open this link in your browser and sign in to your account",
                "   https://auth.openai.com/codex/device",
                "",
                "2. Enter this one-time code (expires in 15 minutes)",
                "   WXYZ-12345",
            ]),
            (
                Some("https://auth.openai.com/codex/device".into()),
                Some("WXYZ-12345".into())
            )
        );
    }

    #[test]
    fn gh_names_its_account_on_the_host_s_line() {
        let said = "github.com\n  ✓ Logged in to github.com account octocat (keyring)\n  - \
                    Token: gho_****";
        assert_eq!(gh_account(said, "github.com"), Some("octocat".into()));
        assert_eq!(gh_account(said, "ghe.example.com"), None);
    }

    #[test]
    fn bitbucket_data_center_and_opencode_say_the_line_to_type() {
        assert_eq!(
            Program::Bkt.sign_in(Some("git.example.com")),
            Way::Terminal("bkt auth login https://git.example.com --web-token".into())
        );
        assert_eq!(
            Program::Opencode.sign_in(None),
            Way::Terminal("opencode auth login".into())
        );
    }

    /// Whether the process `pid` still runs: a zombie waiting for its parent does not.
    fn runs(pid: &str) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let state = stat.rsplit_once(") ")?.1.chars().next()?;
                Some(state != 'Z' && state != 'X')
            })
            .unwrap_or(false)
    }

    /// #1185 round 2: a status cut after its time leaves nothing of it running, not even what it
    /// started itself.
    #[tokio::test]
    async fn a_timed_out_status_leaves_no_process_behind() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir().unwrap();
        let started = folder.path().join("started");
        let program = folder.path().join("status");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nsleep 60 &\necho $! > {}\nwait\n",
                started.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

        let ran = ran(&program, &[], Duration::from_millis(500)).await;

        assert!(ran.is_none(), "it was cut");
        let pid = std::fs::read_to_string(&started).unwrap();
        let pid = pid.trim();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while runs(pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "what the status started still runs"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[test]
    fn an_address_with_credentials_is_read_without_them() {
        assert_eq!(
            read(&[
                "Open https://oauth2:glpat-inventedTokenOfATest0042@git.example.com/device now"
            ]),
            (Some("https://git.example.com/device".into()), None)
        );
    }
}
