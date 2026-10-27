//! The command tools: `commands_list`, `commands_run`, `commands_output`, `commands_stop`
//! (`docs/tools.md`, `docs/reference/hemera.md` §3 "Commands").
//!
//! A run is a line split without a shell and spawned in a process group of its own, with the
//! place's variables and an environment that keeps forges out of reach: no forge token, the forge
//! command lines' configuration in an empty folder, and `git push` made to fail in the common case
//! (the gate refuses every form of it it can read). The call waits
//! for its end a while, then answers that it keeps going; its output is kept (its end), masked
//! when shown or stored.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_core::agent::new_id;
use flottille_core::command::split_line;
use flottille_core::procedure::Grant;
use flottille_core::run::{Run, RunState};
use flottille_core::secrets;
use flottille_core::store::{Store, now_ms};
use flottille_core::variables::{self, Names};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

use crate::agents::{signal_group, start_time};
use crate::daemon::Daemon;
use crate::gate::{self, Verdict};
use crate::mcp::{Outcome, arguments, masked};
use crate::place::{Place, Resolved};
use crate::procedures;
use crate::projects;

/// What the call waits for the end of a run, by default and at most, in seconds.
const WAIT_SECONDS: u64 = 30;
const WAIT_SECONDS_MAX: u64 = 600;
/// The lines of output an answer shows, by default and at most.
const TAIL_LINES: usize = 40;
const TAIL_LINES_MAX: usize = 1000;
/// A line longer than this is shown as several.
const LINE_CHARS: usize = 2000;
/// What the lines shown may hold, in bytes: the answer stays under 40,000 characters.
const SHOWN_BUDGET: usize = 30_000;
/// The end of the output kept, with a margin before it is cut back.
const OUTPUT_KEPT: usize = 64 * 1024;
const OUTPUT_MARGIN: usize = 16 * 1024;
/// A run asked to stop has this long before its group is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// The forge tokens a run never gets: each is set empty.
const FORGE_TOKENS: [&str; 9] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GITLAB_TOKEN",
    "GITLAB_ACCESS_TOKEN",
    "GL_TOKEN",
    "OAUTH_TOKEN",
    "BKT_TOKEN",
];
/// The forge command lines' configuration folders, pointed at an empty folder.
const FORGE_CONFIGS: [&str; 3] = ["GH_CONFIG_DIR", "GLAB_CONFIG_DIR", "BKT_CONFIG_DIR"];

/// The runs going on, by id.
#[derive(Default)]
pub struct Live {
    runs: Mutex<HashMap<String, Arc<Going>>>,
}

/// A run going on.
struct Going {
    run: Run,
    output: Mutex<Output>,
    end: watch::Sender<Option<End>>,
    stop: Notify,
    /// Why it was stopped, when it was.
    stopped_as: Mutex<Option<RunState>>,
    /// What its output is masked with: its environment and the place's secret files.
    env: BTreeMap<OsString, OsString>,
    secrets: Vec<String>,
}

#[derive(Default)]
struct Output {
    kept: Vec<u8>,
    /// Bytes cut from its start.
    cut: u64,
}

#[derive(Clone, Copy)]
struct End {
    state: RunState,
    exit_code: Option<i32>,
    signal: Option<i32>,
}

impl Output {
    fn push(&mut self, read: &[u8]) {
        self.kept.extend_from_slice(read);
        if self.kept.len() > OUTPUT_KEPT + OUTPUT_MARGIN {
            let mut drop = self.kept.len() - OUTPUT_KEPT;
            // From a line's start, when there is one near.
            if let Some(newline) = self.kept[drop..].iter().position(|b| *b == b'\n')
                && newline < OUTPUT_MARGIN
            {
                drop += newline + 1;
            }
            self.kept.drain(..drop);
            self.cut += drop as u64;
        }
    }
}

// The catalogue.

pub fn commands_list(daemon: &Daemon, place: &Place, args: Value) -> Outcome {
    if let Err(refusal) = arguments::<Empty>("commands_list", args) {
        return refusal;
    }
    let project = match projects::load_project(daemon, &place.project) {
        Ok(project) => project,
        Err(failure) => return Outcome::Failed(failure.message),
    };
    if project.commands.is_empty() {
        return Outcome::Done(
            "the project's catalogue is empty: run a line with commands_run {\"line\": …}".into(),
        );
    }
    let lines: Vec<String> = project
        .commands
        .iter()
        .map(|(name, command)| {
            let mut notes = Vec::new();
            if let Some(repository) = &command.repository {
                notes.push(format!("in {repository}"));
            }
            for (set, note) in [
                (command.serve, "keeps running"),
                (command.check, "a check"),
                (command.read_only, "changes nothing"),
                (command.ask_before_running, "held for approval"),
            ] {
                if set {
                    notes.push(note.to_owned());
                }
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", notes.join(", "))
            };
            format!("{name}: {}{notes}", command.line)
        })
        .collect();
    Outcome::Done(lines.join("\n"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

// Running.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArguments {
    command: Option<String>,
    line: Option<String>,
    repository: Option<String>,
    timeout: Option<u64>,
    #[allow(dead_code)]
    why: Option<String>,
}

/// What a call to `commands_run` asks, once its command is found and its line split.
struct Request {
    name: Option<String>,
    line: String,
    words: Vec<String>,
    repository: String,
    serve: bool,
    /// Where it runs.
    folder: Resolved,
}

fn request(daemon: &Daemon, place: &Place, args: RunArguments) -> Result<Request, Outcome> {
    let (name, line, repository, serve) = match (args.command, args.line) {
        (Some(name), None) => {
            let project = match projects::load_project(daemon, &place.project) {
                Ok(project) => project,
                Err(failure) => return Err(Outcome::Failed(failure.message)),
            };
            let Some(command) = project.commands.get(&name) else {
                return Err(Outcome::Refused(format!(
                    "refused: the catalogue has no command {name}: list them with commands_list"
                )));
            };
            let repository = args.repository.or(command.repository.clone());
            (Some(name), command.line.clone(), repository, command.serve)
        }
        (None, Some(line)) => (None, line, args.repository, false),
        _ => {
            return Err(Outcome::Refused(
                "refused: give exactly one of command (a catalogue command) and line".into(),
            ));
        }
    };
    let words = split_line(&line).map_err(Outcome::Refused)?;
    let (repository, base) = match place.base(repository.as_deref()) {
        Ok((repository, base)) => (repository.to_owned(), base.to_owned()),
        Err(refusal) => return Err(Outcome::Refused(refusal)),
    };
    let folder = place.resolve(&base, ".");
    Ok(Request {
        name,
        line,
        words,
        repository,
        serve,
        folder,
    })
}

/// The folder a call to `commands_run` would run in, the words it would start and the catalogue
/// command it names, for the gate; nothing for a call that does not fit (the tool refuses it) and
/// for any other tool.
pub fn reaches(
    daemon: &Daemon,
    place: &Place,
    tool: &str,
    args: &Value,
) -> (Vec<Resolved>, Vec<String>, Option<String>) {
    if tool != "commands_run" {
        return (Vec::new(), Vec::new(), None);
    }
    let args = if args.is_null() {
        json!({})
    } else {
        args.clone()
    };
    serde_json::from_value::<RunArguments>(args)
        .ok()
        .and_then(|args| request(daemon, place, args).ok())
        .map_or_else(Default::default, |request| {
            (vec![request.folder], request.words, request.name)
        })
}

/// `lifted` is the grant of a procedure the gate let this line through by: its environment is
/// lifted for it, and only it (`defend`).
pub async fn commands_run(
    daemon: &Arc<Daemon>,
    agent: &str,
    place: &Place,
    args: Value,
    lifted: Option<Grant>,
    cancelled: CancellationToken,
) -> Outcome {
    let args: RunArguments = match arguments("commands_run", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    let wait = args.timeout.unwrap_or(WAIT_SECONDS);
    if !(1..=WAIT_SECONDS_MAX).contains(&wait) {
        return Outcome::Refused(format!(
            "refused: timeout must be 1 to {WAIT_SECONDS_MAX} seconds: a run that takes longer \
             keeps going, read it with commands_output"
        ));
    }
    let Request {
        name,
        line,
        words,
        repository,
        serve,
        folder,
    } = match request(daemon, place, args) {
        Ok(request) => request,
        Err(refusal) => return refusal,
    };
    if let Verdict::Deny(why) = gate::contained(std::slice::from_ref(&folder)) {
        return Outcome::Refused(why);
    }
    if matches!(lifted, Some(Grant::Push | Grant::PushForceWithLease))
        && let Err(refusal) =
            procedures::remote_kept(daemon, place, &repository, &folder.path).await
    {
        return Outcome::Refused(refusal);
    }
    // A server already running for the agent is joined, not started twice.
    if serve
        && let Some(going) = daemon
            .mcp
            .runs
            .runs
            .lock()
            .unwrap()
            .values()
            .find(|g| g.run.agent == agent && g.run.name == name)
            .cloned()
    {
        return Outcome::Done(describe(daemon, &going.run.id, None, None, true));
    }
    let env = match environment(daemon, place, &repository, lifted) {
        Ok(env) => env,
        Err(why) => return Outcome::Failed(why),
    };
    let id = match new_id() {
        Ok(id) => id,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let run = Run {
        id: id.clone(),
        agent: agent.to_owned(),
        name,
        // What is stored, sent in `run.started` and shown is masked like a call's arguments.
        line: masked(&line, place),
        folder: folder.path,
        state: RunState::Running,
        exit_code: None,
        signal: None,
        started_at: now_ms(),
        ended_at: None,
        output: String::new(),
    };
    if let Err(why) = start(daemon, run, words, env, place.secret_values()) {
        return Outcome::Failed(why);
    }
    if serve {
        return Outcome::Done(describe(daemon, &id, None, None, true));
    }
    let ended = wait_for_end(daemon, &id);
    tokio::select! {
        () = ended => {}
        () = tokio::time::sleep(Duration::from_secs(wait)) => {}
        () = cancelled.cancelled() => {
            stop(daemon, &id, RunState::Stopped).await;
            return Outcome::Failed("the call was stopped before it ended".into());
        }
    }
    Outcome::Done(describe(daemon, &id, None, None, true))
}

/// Ends when the run has ended.
fn wait_for_end(daemon: &Daemon, id: &str) -> impl Future<Output = ()> + use<> {
    let end = daemon
        .mcp
        .runs
        .runs
        .lock()
        .unwrap()
        .get(id)
        .map(|going| going.end.subscribe());
    async move {
        if let Some(mut end) = end {
            let _ = end.wait_for(Option::is_some).await;
        }
    }
}

/// The environment of a run in the place: the daemon's, the place's variables filled, the forges
/// and the daemon kept out of reach; lifted for the grant of a procedure the line goes through by.
fn environment(
    daemon: &Daemon,
    place: &Place,
    repository: &str,
    lifted: Option<Grant>,
) -> Result<BTreeMap<OsString, OsString>, String> {
    let names = Names {
        workspace: place.workspace.clone().unwrap_or_else(|| "main".into()),
        workspace_path: place.root.to_string_lossy().into_owned(),
        project: place.project.clone(),
        branch: place.branch.clone(),
        repository: repository.to_owned(),
    };
    let mut filled = BTreeMap::new();
    for (name, template) in &place.variables {
        filled.insert(name.clone(), variables::fill(template, &names)?);
    }
    let mut env = variables::environment(std::env::vars_os(), &[&filled]);
    let empty = daemon.paths.state.join("empty");
    flottille_core::paths::ensure_private_dir(&empty).map_err(|e| e.to_string())?;
    defend(&mut env, &empty, lifted);
    // Where the daemon's socket is: a client a run starts is refused anyway (`daemon.rs`).
    env.remove(&OsString::from("XDG_RUNTIME_DIR"));
    Ok(env)
}

/// No forge token, the forge command lines' configuration in `empty`, and, unless the run is a
/// push a procedure grants, every push URL rewritten to a path that is no remote
/// (`url.<nowhere>.pushInsteadOf` of every URL). A line a procedure's grant lets through runs no
/// hook: the hooks are files the agent writes, run where the grant lifts the gate.
fn defend(env: &mut BTreeMap<OsString, OsString>, empty: &Path, lifted: Option<Grant>) {
    for name in FORGE_TOKENS {
        env.insert(name.into(), OsString::new());
    }
    for name in FORGE_CONFIGS {
        env.insert(name.into(), empty.into());
    }
    if lifted.is_some() {
        configure(env, "core.hooksPath", "/dev/null");
    }
    if !matches!(lifted, Some(Grant::Push | Grant::PushForceWithLease)) {
        configure(
            env,
            "url./dev/null/flottille-refused-push/.pushInsteadOf",
            "",
        );
    }
}

/// One more git setting through the environment, after those it already has: it wins over the
/// repository's own.
fn configure(env: &mut BTreeMap<OsString, OsString>, key: &str, value: &str) {
    let count: usize = env
        .get(&OsString::from("GIT_CONFIG_COUNT"))
        .and_then(|count| count.to_str()?.parse().ok())
        .unwrap_or(0);
    env.insert(format!("GIT_CONFIG_KEY_{count}").into(), key.into());
    env.insert(format!("GIT_CONFIG_VALUE_{count}").into(), value.into());
    env.insert("GIT_CONFIG_COUNT".into(), (count + 1).to_string().into());
}

/// Spawns the run, records it, and leaves a task of its own to follow it to its end.
fn start(
    daemon: &Arc<Daemon>,
    run: Run,
    words: Vec<String>,
    env: BTreeMap<OsString, OsString>,
    secrets: Vec<String>,
) -> Result<(), String> {
    let (reader, writer) = std::io::pipe().map_err(|e| format!("cannot run it: {e}"))?;
    let mut command = tokio::process::Command::new(&words[0]);
    command
        .args(&words[1..])
        .env_clear()
        .envs(&env)
        .current_dir(&run.folder)
        .stdin(Stdio::null())
        .stdout(
            writer
                .try_clone()
                .map_err(|e| format!("cannot run it: {e}"))?,
        )
        .stderr(writer)
        .process_group(0)
        .kill_on_drop(true);
    let spawned = command.spawn();
    // The command holds the pipe's writing end: dropped, only the processes hold it.
    drop(command);
    let child = spawned.map_err(|e| format!("cannot run {}: {e}", words[0]))?;
    let pid = child.id().ok_or("the run ended at once")?;
    let reader = tokio::net::unix::pipe::Receiver::from_owned_fd(reader.into())
        .map_err(|e| format!("cannot read the output of {}: {e}", words[0]))?;
    let event = json!({"agent": run.agent, "run": run.id, "name": run.name, "line": run.line,
        "folder": run.folder});
    daemon
        .write(|store| {
            let event = store.create_run(&run, ("run.started", event))?;
            if let Some(started) = start_time(pid) {
                store.add_run_process(pid, started, &run.id)?;
            }
            Ok(((), event))
        })
        .map_err(|e| e.to_string())?;
    let going = Arc::new(Going {
        run,
        output: Mutex::default(),
        end: watch::Sender::new(None),
        stop: Notify::new(),
        stopped_as: Mutex::new(None),
        env,
        secrets,
    });
    daemon
        .mcp
        .runs
        .runs
        .lock()
        .unwrap()
        .insert(going.run.id.clone(), going.clone());
    tokio::spawn(follow(daemon.clone(), going, child, pid, reader));
    Ok(())
}

/// Reads the run's output until it ends by itself or is stopped, then records its end.
async fn follow(
    daemon: Arc<Daemon>,
    going: Arc<Going>,
    mut child: tokio::process::Child,
    pid: u32,
    mut reader: tokio::net::unix::pipe::Receiver,
) {
    use std::os::unix::process::ExitStatusExt;
    use tokio::io::AsyncReadExt;
    let mut buffer = vec![0; 8192];
    let mut open = true;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status.ok(),
            read = reader.read(&mut buffer), if open => match read {
                Ok(0) | Err(_) => open = false,
                Ok(read) => going.output.lock().unwrap().push(&buffer[..read]),
            },
            () = going.stop.notified() => {
                signal_group(pid, libc::SIGTERM);
                if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_err() {
                    signal_group(pid, libc::SIGKILL);
                    let _ = child.wait().await;
                }
                break None;
            }
        }
    };
    // What it left in the background holds the pipe and would keep running unseen: it ends with
    // the run, and what is already in the pipe is read without waiting for it.
    signal_group(pid, libc::SIGKILL);
    while let Ok(read) = reader.try_read(&mut buffer) {
        if read == 0 {
            break;
        }
        going.output.lock().unwrap().push(&buffer[..read]);
    }
    let end = match status {
        Some(status) => End {
            state: RunState::Exited,
            exit_code: status.code(),
            signal: status.signal(),
        },
        None => End {
            state: going
                .stopped_as
                .lock()
                .unwrap()
                .unwrap_or(RunState::Stopped),
            exit_code: None,
            signal: None,
        },
    };
    let output = masked_output(&going);
    let run = &going.run;
    let event = json!({"agent": run.agent, "run": run.id, "state": end.state.as_str(),
        "exit_code": end.exit_code, "signal": end.signal});
    let result = daemon.write(|store| {
        store.remove_run_process(pid)?;
        let event = store.end_run(
            &run.id,
            end.state,
            end.exit_code,
            end.signal,
            &output,
            ("run.ended", event),
        )?;
        Ok(((), event))
    });
    if let Err(error) = result {
        eprintln!("flottille daemon: run {}: {error}", run.id);
    }
    daemon.mcp.runs.runs.lock().unwrap().remove(&run.id);
    going.end.send_replace(Some(end));
}

fn masked_output(going: &Going) -> String {
    let output = going.output.lock().unwrap();
    secrets::mask_also(
        &String::from_utf8_lossy(&output.kept),
        &going.env,
        &going.secrets,
    )
}

/// Asks the run to stop and waits for its end.
async fn stop(daemon: &Daemon, id: &str, state: RunState) {
    let going = daemon.mcp.runs.runs.lock().unwrap().get(id).cloned();
    let Some(going) = going else { return };
    *going.stopped_as.lock().unwrap() = Some(state);
    let mut end = going.end.subscribe();
    going.stop.notify_one();
    let _ = end.wait_for(Option::is_some).await;
}

// Reading and stopping.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputArguments {
    run: String,
    tail: Option<usize>,
    start: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopArguments {
    run: String,
}

pub fn commands_output(daemon: &Daemon, agent: &str, args: Value) -> Outcome {
    let args: OutputArguments = match arguments("commands_output", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    let tail = args.tail.unwrap_or(TAIL_LINES);
    if !(1..=TAIL_LINES_MAX).contains(&tail) {
        return Outcome::Refused(format!(
            "refused: tail must be 1 to {TAIL_LINES_MAX} lines: read on with start"
        ));
    }
    if let Err(refusal) = own(daemon, agent, &args.run) {
        return refusal;
    }
    Outcome::Done(describe(daemon, &args.run, Some(tail), args.start, false))
}

pub async fn commands_stop(daemon: &Daemon, agent: &str, args: Value) -> Outcome {
    let args: StopArguments = match arguments("commands_stop", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    if let Err(refusal) = own(daemon, agent, &args.run) {
        return refusal;
    }
    stop(daemon, &args.run, RunState::Stopped).await;
    Outcome::Done(describe(daemon, &args.run, None, None, false))
}

/// The run, refused unless the agent started it.
fn own(daemon: &Daemon, agent: &str, id: &str) -> Result<Run, Outcome> {
    match daemon.read(|store| store.run(id)) {
        Ok(Some(run)) if run.agent == agent => Ok(run),
        Ok(_) => Err(Outcome::Refused(format!(
            "refused: {id} is not a run this agent started"
        ))),
        Err(error) => Err(Outcome::Failed(error.to_string())),
    }
}

/// What an answer says of a run: its state, then lines of its output, the last `tail` or those
/// from `start`, with how to read the others.
fn describe(
    daemon: &Daemon,
    id: &str,
    tail: Option<usize>,
    start: Option<usize>,
    answer_of_run: bool,
) -> String {
    let going = daemon.mcp.runs.runs.lock().unwrap().get(id).cloned();
    let (run, end, output, cut) = match &going {
        Some(going) => {
            let cut = going.output.lock().unwrap().cut;
            let end = *going.end.borrow();
            (going.run.clone(), end, masked_output(going), cut)
        }
        None => match daemon.read(|store| store.run(id)) {
            Ok(Some(run)) => {
                let end = End {
                    state: run.state,
                    exit_code: run.exit_code,
                    signal: run.signal,
                };
                let output = run.output.clone();
                (run, Some(end), output, 0)
            }
            _ => return format!("run {id}: there is no such run"),
        },
    };
    let label = match &run.name {
        Some(name) => format!("{name} (`{}`)", run.line),
        None => format!("`{}`", run.line),
    };
    let state = match end {
        None => "is running".to_owned(),
        Some(End {
            state: RunState::Exited,
            exit_code: Some(code),
            ..
        }) => format!("exited with code {code}"),
        Some(End {
            state: RunState::Exited,
            signal,
            ..
        }) => format!("was killed by signal {}", signal.unwrap_or_default()),
        Some(End {
            state: RunState::Stopped,
            ..
        }) => "was stopped".to_owned(),
        Some(End {
            state: RunState::Interrupted,
            ..
        }) => "was interrupted: the daemon that ran it stopped".to_owned(),
        Some(End {
            state: RunState::Running,
            ..
        }) => "is running".to_owned(),
    };
    let seconds = (run.ended_at.unwrap_or_else(now_ms) - run.started_at) as f64 / 1000.0;
    let mut text = format!("run {}: {label} {state} after {seconds:.1} s", run.id);
    if end.is_none() && answer_of_run {
        text.push_str("\nit keeps going: read it with commands_output, stop it with commands_stop");
    }
    let lines = display_lines(&output);
    if lines.is_empty() {
        text.push_str("\n(no output)");
        return text;
    }
    let tail = tail.unwrap_or(TAIL_LINES);
    let (from, to) = shown_range(&lines, tail, start);
    let mut about = format!("\n(lines {}-{} of {}", from + 1, to, lines.len());
    if cut > 0 || (end.is_some() && run.output.len() >= OUTPUT_KEPT && going.is_none()) {
        about.push_str(", the output being cut to its last 64 KiB");
    }
    if from > 0 {
        let earlier = from.saturating_sub(tail) + 1;
        about.push_str(&format!(
            "; earlier lines: commands_output {{\"run\": \"{}\", \"start\": {earlier}}}",
            run.id
        ));
    }
    if to < lines.len() {
        about.push_str(&format!(
            "; later lines: commands_output {{\"run\": \"{}\", \"start\": {}}}",
            run.id,
            to + 1
        ));
    }
    about.push(')');
    text.push_str(&about);
    text.push('\n');
    text.push_str(&lines[from..to].join("\n"));
    text
}

/// The lines of an output, a line longer than `LINE_CHARS` shown as several.
fn display_lines(output: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in output.trim_end_matches('\n').split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
        }
        for piece in chars.chunks(LINE_CHARS) {
            lines.push(piece.iter().collect());
        }
    }
    if output.is_empty() {
        lines.clear();
    }
    lines
}

/// Which lines to show, `[from, to)`: up to `count` from `start` (1-based), or the last ones,
/// within the budget.
fn shown_range(lines: &[String], count: usize, start: Option<usize>) -> (usize, usize) {
    match start {
        Some(start) => {
            let from = start.saturating_sub(1).min(lines.len());
            let mut to = from;
            let mut size = 0;
            while to < lines.len() && to - from < count && size + lines[to].len() < SHOWN_BUDGET {
                size += lines[to].len() + 1;
                to += 1;
            }
            (from, to.max((from + 1).min(lines.len())))
        }
        None => {
            let to = lines.len();
            let mut from = to;
            let mut size = 0;
            while from > 0 && to - from < count && size + lines[from - 1].len() < SHOWN_BUDGET {
                size += lines[from - 1].len() + 1;
                from -= 1;
            }
            (from.min(to.saturating_sub(1)), to)
        }
    }
}

// The daemon's start and stop.

/// At daemon start: kills the run process groups a previous daemon left running, each only when
/// its leader is still the process it started, and marks their runs interrupted.
pub fn settle_at_start(daemon: &Daemon) {
    let result = daemon.read(|store| {
        for (pid, started) in store.run_processes()? {
            if start_time(pid) == Some(started) {
                signal_group(pid, libc::SIGKILL);
                eprintln!("flottille daemon: killed the run process {pid} that was left running");
            }
            store.remove_run_process(pid)?;
        }
        Ok(())
    });
    if let Err(error) = result {
        eprintln!("flottille daemon: reading the runs' processes: {error}");
    }
    let runs = daemon.read(Store::running_runs).unwrap_or_default();
    for run in runs {
        let _ = daemon.write(|store| {
            let event = store.end_run(
                &run.id,
                RunState::Interrupted,
                None,
                None,
                &run.output,
                (
                    "run.ended",
                    json!({"agent": run.agent, "run": run.id, "state": "interrupted",
                        "exit_code": null, "signal": null}),
                ),
            )?;
            Ok(((), event))
        });
    }
}

/// The agent stopped: every run it started that goes on is stopped with it.
pub async fn stop_runs_of(daemon: &Daemon, agent: &str) {
    let ids: Vec<String> = daemon
        .mcp
        .runs
        .runs
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, going)| going.run.agent == agent)
        .map(|(id, _)| id.clone())
        .collect();
    futures::future::join_all(ids.iter().map(|id| stop(daemon, id, RunState::Stopped))).await;
}

/// The daemon stopping: every run is stopped, and marked interrupted. All at once: each may take
/// its whole grace, and the daemon would wait for their sum.
pub async fn stop_all(daemon: &Daemon) {
    let ids: Vec<String> = daemon
        .mcp
        .runs
        .runs
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    futures::future::join_all(ids.iter().map(|id| stop(daemon, id, RunState::Interrupted))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_gets_no_forge_token_and_no_push() {
        let mut env: BTreeMap<OsString, OsString> = [
            ("GH_TOKEN", "ghp_x"),
            ("GIT_CONFIG_COUNT", "2"),
            ("PATH", "/bin"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();

        defend(&mut env, Path::new("/state/empty"), None);

        let get = |name: &str| env[&OsString::from(name)].to_str().unwrap().to_owned();
        assert_eq!(get("GH_TOKEN"), "");
        assert_eq!(get("GITHUB_TOKEN"), "");
        assert_eq!(get("GH_CONFIG_DIR"), "/state/empty");
        assert_eq!(get("GIT_CONFIG_COUNT"), "3");
        assert_eq!(
            get("GIT_CONFIG_KEY_2"),
            "url./dev/null/flottille-refused-push/.pushInsteadOf"
        );
        assert_eq!(get("GIT_CONFIG_VALUE_2"), "");
        assert_eq!(get("PATH"), "/bin");
    }

    #[test]
    fn a_push_a_procedure_grants_keeps_its_push_url_and_still_gets_no_forge_token() {
        let mut env: BTreeMap<OsString, OsString> =
            [("GH_TOKEN", "ghp_x"), ("GIT_CONFIG_COUNT", "2")]
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect();

        defend(&mut env, Path::new("/state/empty"), Some(Grant::Push));

        let get = |name: &str| env[&OsString::from(name)].to_str().unwrap().to_owned();
        assert_eq!(get("GH_TOKEN"), "");
        assert_eq!(get("GH_CONFIG_DIR"), "/state/empty");
        assert_eq!(get("GIT_CONFIG_COUNT"), "3");
        assert_eq!(get("GIT_CONFIG_KEY_2"), "core.hooksPath");
        assert_eq!(get("GIT_CONFIG_VALUE_2"), "/dev/null");
        assert!(!env.contains_key(&OsString::from("GIT_CONFIG_KEY_3")));
    }

    #[test]
    fn a_granted_commit_push_or_rebase_runs_with_its_hooks_off_and_only_a_push_without_the_wall() {
        for grant in [
            Grant::Commit,
            Grant::Rebase,
            Grant::Push,
            Grant::PushForceWithLease,
        ] {
            let mut env = BTreeMap::new();

            defend(&mut env, Path::new("/state/empty"), Some(grant));

            let get = |name: &str| env[&OsString::from(name)].to_str().unwrap().to_owned();
            assert_eq!(get("GIT_CONFIG_KEY_0"), "core.hooksPath", "{grant}");
            assert_eq!(get("GIT_CONFIG_VALUE_0"), "/dev/null", "{grant}");
            let walled = matches!(grant, Grant::Commit | Grant::Rebase);
            assert_eq!(get("GIT_CONFIG_COUNT"), if walled { "2" } else { "1" });
            assert_eq!(
                env.get(&OsString::from("GIT_CONFIG_KEY_1"))
                    .map(|key| key.to_str().unwrap()),
                walled.then_some("url./dev/null/flottille-refused-push/.pushInsteadOf"),
                "{grant}"
            );
        }
    }

    #[test]
    fn the_output_kept_is_its_end_from_a_line_start() {
        let mut output = Output::default();
        for n in 0..20_000 {
            output.push(format!("line {n}\n").as_bytes());
        }
        assert!(output.kept.len() <= OUTPUT_KEPT + OUTPUT_MARGIN);
        assert!(output.kept.starts_with(b"line "));
        assert!(output.kept.ends_with(b"line 19999\n"));
        assert_eq!(
            output.cut + output.kept.len() as u64,
            (0..20_000)
                .map(|n| format!("line {n}\n").len() as u64)
                .sum::<u64>()
        );
    }

    #[test]
    fn long_lines_are_shown_as_several_and_the_tail_fits_the_budget() {
        let lines = display_lines(&"x".repeat(LINE_CHARS * 2 + 5));
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2], "xxxxx");
        let many: Vec<String> = (0..100).map(|_| "y".repeat(LINE_CHARS)).collect();
        let (from, to) = shown_range(&many, 1000, None);
        assert_eq!(to, 100);
        assert!((to - from) * LINE_CHARS < SHOWN_BUDGET);
        assert_eq!(shown_range(&many, 5, Some(3)), (2, 7));
    }
}
