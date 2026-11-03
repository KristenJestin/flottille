//! The command tools: `run` and `run_status` (`docs/tools.md`, "Running", and
//! `docs/reference/hemera.md` §3 "Commands"), and the tasks the user runs from the window.
//!
//! A run is a line split without a shell and spawned in a process group of its own, with the
//! place's variables and an environment that keeps forges out of reach: no forge token, the forge
//! command lines' configuration in an empty folder, and no git hook. The call waits
//! for its end a while, then answers that it keeps going; its output is kept (its end), masked
//! when shown or stored.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_core::agent::new_id;
use flottille_core::approval;
use flottille_core::command::split_line;
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
use crate::deliveries;
use crate::gate::{self, Verdict};
use crate::mcp::{Outcome, arguments, masked};
use crate::place::{Place, Resolved};
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
pub(crate) const OUTPUT_KEPT: usize = 64 * 1024;
const OUTPUT_MARGIN: usize = 16 * 1024;
/// A run asked to stop has this long before its group is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// The lines of output a `command-ended` delivery holds.
const ENDED_TAIL_LINES: usize = 20;

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
    /// Its process, which leads its process group.
    pid: u32,
    /// Whether the call that started it still waits for its end, answered with it running, or
    /// is over: only a run that outlived its call wakes the agent when it ends.
    phase: Mutex<Phase>,
    /// The user stopped it (`run.stop`), which the agent is told.
    stopped_by_user: AtomicBool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The call that started it waits for it.
    Waited,
    /// The call answered while it ran: its end is delivered.
    Detached,
    /// It ended.
    Over,
}

/// What a program wrote, bounded to its end from a line's start: a run's, and a setup step's.
#[derive(Default)]
pub(crate) struct Output {
    pub(crate) kept: Vec<u8>,
    /// Bytes cut from its start.
    pub(crate) cut: u64,
}

#[derive(Clone, Copy)]
struct End {
    state: RunState,
    exit_code: Option<i32>,
    signal: Option<i32>,
}

impl Output {
    pub(crate) fn push(&mut self, read: &[u8]) {
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

// Running.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArguments {
    /// `command`, its name before tasks, is still read.
    #[serde(alias = "command")]
    task: Option<String>,
    line: Option<String>,
    repository: Option<String>,
    timeout: Option<u64>,
    #[allow(dead_code)]
    why: Option<String>,
}

/// What a call to `run` asks, once its task is found and its line split.
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
    let (name, line, repository, serve) = match (args.task, args.line) {
        (Some(name), None) => {
            let project = match projects::load_project(daemon, &place.project) {
                Ok(project) => project,
                Err(failure) => return Err(Outcome::Failed(failure.message)),
            };
            let Some(task) = project.tasks.get(&name) else {
                return Err(Outcome::Refused(format!(
                    "refused: the project has no task {name}: list them with tasks {{\"action\": \
                     \"list\"}}"
                )));
            };
            let repository = args.repository.or(task.repository.clone());
            (Some(name), task.line.clone(), repository, task.serve)
        }
        (None, Some(line)) => (None, line, args.repository, false),
        _ => {
            return Err(Outcome::Refused(
                "refused: give exactly one of task (one of the project's) and line".into(),
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

/// The folder a call to `run` would run in, the words it would start and the task it names, for
/// the gate; nothing for a call that does not fit (the tool refuses it) and for any other tool.
pub fn reaches(
    daemon: &Daemon,
    place: &Place,
    tool: &str,
    args: &Value,
) -> (Vec<Resolved>, Vec<String>, Option<String>) {
    if tool != "run" {
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

/// `approved` when the user or the judge let the call through: it may then run outside the place.
pub async fn run(
    daemon: &Arc<Daemon>,
    agent: &str,
    place: &Place,
    args: Value,
    approved: bool,
    cancelled: CancellationToken,
) -> Outcome {
    let args: RunArguments = match arguments("run", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    let wait = args.timeout.unwrap_or(WAIT_SECONDS);
    if !(1..=WAIT_SECONDS_MAX).contains(&wait) {
        return Outcome::Refused(format!(
            "refused: timeout must be 1 to {WAIT_SECONDS_MAX} seconds: a run that takes longer \
             keeps going, read it with run_status"
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
    if !approved && let Verdict::Deny(why) = gate::contained(std::slice::from_ref(&folder)) {
        return Outcome::Refused(why);
    }
    if gate::pushes(&words)
        && let Err(refusal) = remote_kept(daemon, place, &repository, &folder.path).await
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
            .find(|g| g.run.agent.as_deref() == Some(agent) && g.run.name == name)
            .cloned()
    {
        return Outcome::Done(describe(daemon, &going.run.id, None, None, true, false));
    }
    // Only a push the gate read, and that the user or the judge let through, goes out.
    let lift = approved && gate::is_push(&words);
    let env = match environment(daemon, place, &repository, lift) {
        Ok(env) => env,
        Err(why) => return Outcome::Failed(why),
    };
    let id = match new_id() {
        Ok(id) => id,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let run = Run {
        id: id.clone(),
        agent: Some(agent.to_owned()),
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
        dismissed: false,
        project: place.project.clone(),
    };
    if let Err(why) = start(daemon, run, words, env, place.secret_values()) {
        return Outcome::Failed(why);
    }
    if serve {
        detach(daemon, &id);
        return Outcome::Done(describe(daemon, &id, None, None, true, false));
    }
    // Held now: once ended, the run is no longer among those going on.
    let going = daemon.mcp.runs.runs.lock().unwrap().get(&id).cloned();
    let ended = wait_for_end(daemon, &id);
    tokio::select! {
        () = ended => {}
        () = tokio::time::sleep(Duration::from_secs(wait)) => detach(daemon, &id),
        () = cancelled.cancelled() => {
            stop(daemon, &id, RunState::Stopped).await;
            return Outcome::Failed("the call was stopped before it ended".into());
        }
    }
    let by_user = going.is_some_and(|going| going.stopped_by_user.load(Ordering::SeqCst));
    Outcome::Done(describe(daemon, &id, None, None, true, by_user))
}

/// A task the user runs from the window, in the project folder (`main`): a run of no agent's,
/// which the gate does not see, since the user asked for it, in the environment an agent's run
/// gets. It is not waited for: the window follows it in the project's list of commands. A task
/// that keeps running and already runs for the user is joined, not started twice.
pub fn run_for_user(
    daemon: &Arc<Daemon>,
    project: &flottille_core::project::Project,
    name: &str,
) -> Result<String, String> {
    let task = project
        .tasks
        .get(name)
        .ok_or_else(|| format!("project {} has no task named {name}", project.name))?;
    if task.serve
        && let Some(going) = daemon.mcp.runs.runs.lock().unwrap().values().find(|g| {
            g.run.agent.is_none()
                && g.run.project == project.name
                && g.run.name.as_deref() == Some(name)
        })
    {
        return Ok(going.run.id.clone());
    }
    let place = Place::main(project, project.folder.clone());
    let words = split_line(&task.line)?;
    let (repository, base) = place.base(task.repository.as_deref())?;
    let folder = place.resolve(base, ".");
    let env = environment(daemon, &place, repository, false)?;
    let id = new_id().map_err(|error| error.to_string())?;
    let run = Run {
        id: id.clone(),
        agent: None,
        name: Some(name.to_owned()),
        line: masked(&task.line, &place),
        folder: folder.path,
        state: RunState::Running,
        exit_code: None,
        signal: None,
        started_at: now_ms(),
        ended_at: None,
        output: String::new(),
        dismissed: false,
        project: project.name.clone(),
    };
    start(daemon, run, words, env, place.secret_values())?;
    Ok(id)
}

/// The call that started the run answers while it runs: its end wakes the agent, unless it
/// ended meanwhile, which the answer then says.
fn detach(daemon: &Daemon, id: &str) {
    let going = daemon.mcp.runs.runs.lock().unwrap().get(id).cloned();
    if let Some(going) = going {
        let mut phase = going.phase.lock().unwrap();
        if *phase == Phase::Waited {
            *phase = Phase::Detached;
        }
    }
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
/// and the daemon kept out of reach, and pushes too unless `lift`.
fn environment(
    daemon: &Daemon,
    place: &Place,
    repository: &str,
    lift: bool,
) -> Result<BTreeMap<OsString, OsString>, String> {
    let names = names(place, repository);
    let mut filled = BTreeMap::new();
    for (name, template) in &place.variables {
        filled.insert(name.clone(), variables::fill(template, &names)?);
    }
    let mut env = variables::environment(std::env::vars_os(), &[&filled]);
    let empty = daemon.paths.state.join("empty");
    flottille_core::paths::ensure_private_dir(&empty).map_err(|e| e.to_string())?;
    defend(&mut env, &empty, lift);
    // Where the daemon's socket is: a client a run starts is refused anyway (`daemon.rs`).
    env.remove(&OsString::from("XDG_RUNTIME_DIR"));
    Ok(env)
}

fn names(place: &Place, repository: &str) -> Names {
    Names {
        workspace: place.workspace.clone().unwrap_or_else(|| "main".into()),
        workspace_path: place.root.to_string_lossy().into_owned(),
        project: place.project.clone(),
        branch: place.branch.clone(),
        repository: repository.to_owned(),
    }
}

/// Where a run in the place looks for a program named bare: the `PATH` of its environment.
pub fn search_path(place: &Place, repository: &str) -> Option<OsString> {
    match place.variables.get("PATH") {
        Some(template) => variables::fill(template, &names(place, repository))
            .ok()
            .map(OsString::from),
        None => std::env::var_os("PATH"),
    }
}

/// The process groups of the agent's runs that go on: each run leads its own.
pub fn groups_of(daemon: &Daemon, agent: &str) -> Vec<u32> {
    daemon
        .mcp
        .runs
        .runs
        .lock()
        .unwrap()
        .values()
        .filter(|going| going.run.agent.as_deref() == Some(agent))
        .map(|going| going.pid)
        .collect()
}

/// No forge token, the forge command lines' configuration in `empty` (`forge` is the way to
/// the forge), and no hook: the hooks are files the agent writes, and a commit or a push the gate
/// let through would run them where it does not look. Every push URL is rewritten to a path that
/// is no remote (`url.<nowhere>.pushInsteadOf` of every URL), unless `lift`: the run is itself a
/// `git push` the user or the judge let through. A harmless command cannot push from a script
/// the gate never read (`docs/design.md` §7).
fn defend(env: &mut BTreeMap<OsString, OsString>, empty: &Path, lift: bool) {
    for name in FORGE_TOKENS {
        env.insert(name.into(), OsString::new());
    }
    for name in FORGE_CONFIGS {
        env.insert(name.into(), empty.into());
    }
    configure(env, "core.hooksPath", "/dev/null");
    if !lift {
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

/// Refuses a push from a workspace unless the repository's remote leads where it led when the
/// workspace was created (`forge::remote_kept`): neither the judge nor the user reads where a
/// remote leads now. A push from the project folder has nothing to compare with.
async fn remote_kept(
    daemon: &Daemon,
    place: &Place,
    repository: &str,
    folder: &Path,
) -> Result<(), String> {
    let Some(name) = &place.workspace else {
        return Ok(());
    };
    let workspace = daemon
        .read(|store| store.workspace(&place.project, name))
        .ok()
        .flatten();
    let base = workspace
        .iter()
        .flat_map(|workspace| &workspace.repositories)
        .find(|r| r.path == repository)
        .map(|r| &r.base)
        .ok_or_else(|| {
            format!("refused: {repository} is not a repository of this agent's workspace")
        })?;
    crate::forge::remote_kept(folder, base).await
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
    let event = json!({"agent": run.agent, "project": run.project, "run": run.id,
        "name": run.name, "line": run.line, "folder": run.folder});
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
        pid,
        phase: Mutex::new(Phase::Waited),
        stopped_by_user: AtomicBool::new(false),
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
    let event = json!({"agent": run.agent, "project": run.project, "run": run.id,
        "state": end.state.as_str(), "exit_code": end.exit_code, "signal": end.signal});
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
    let detached =
        std::mem::replace(&mut *going.phase.lock().unwrap(), Phase::Over) == Phase::Detached;
    let by_user = going.stopped_by_user.load(Ordering::SeqCst);
    // Told to the agent only when no call was left to answer, and only for an end that is news
    // to it: not its own stop, nor the daemon's or its own end.
    let told = match end.state {
        RunState::Exited => true,
        RunState::Stopped => by_user,
        RunState::Running | RunState::Interrupted => false,
    };
    if detached
        && told
        && let Some(agent) = &run.agent
    {
        let body = ended_text(&going, end, &output, by_user);
        deliveries::deliver(
            &daemon,
            agent,
            "command-ended",
            Some(format!("command-ended-{}", run.id)),
            &body,
            approval::URGENT,
        );
    }
    going.end.send_replace(Some(end));
}

/// What the agent is told of a run that ended after its call: how it ended, how long it ran and
/// the end of its output.
fn ended_text(going: &Going, end: End, output: &str, by_user: bool) -> String {
    let state = state_text(Some(end), by_user);
    let seconds = (now_ms() - going.run.started_at) as f64 / 1000.0;
    let mut text = format!(
        "Your command `{}` (run {}) {state} after {seconds:.1} s.",
        going.run.line, going.run.id
    );
    let lines = display_lines(output);
    if lines.is_empty() {
        text.push_str("\nIt wrote nothing.");
        return text;
    }
    let (from, to) = shown_range(&lines, ENDED_TAIL_LINES, None);
    text.push_str(&format!(
        "\nIts last lines (lines {}-{} of {}; read more with run_status {{\"run\": \"{}\"}}):\n{}",
        from + 1,
        to,
        lines.len(),
        going.run.id,
        lines[from..to].join("\n")
    ));
    text
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

/// How a run is: running, or how it ended; `by_user` when the user stopped it.
fn state_text(end: Option<End>, by_user: bool) -> String {
    match end {
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
        }) if by_user => "was stopped by the user".to_owned(),
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
    }
}

// What the user's clients read and do (`commands.rs`).

/// A run as the user reads it: its record, and what it wrote so far, masked.
pub struct View {
    pub run: Run,
    pub output: String,
    /// The output was cut to its last 64 KiB.
    pub cut: bool,
}

/// The run going on, or the stored one once it ended; `None` when there is no such run.
pub fn view(daemon: &Daemon, id: &str) -> Option<View> {
    let going = daemon.mcp.runs.runs.lock().unwrap().get(id).cloned();
    if let Some(going) = going
        && going.end.borrow().is_none()
    {
        let cut = going.output.lock().unwrap().cut > 0;
        return Some(View {
            output: masked_output(&going),
            run: going.run.clone(),
            cut,
        });
    }
    let run = daemon.read(|store| store.run(id)).ok().flatten()?;
    Some(View {
        output: run.output.clone(),
        cut: run.output.len() >= OUTPUT_KEPT,
        run,
    })
}

/// The user stops the run: its whole group, as the agent's own stop does, and the agent is told
/// it was the user once it ended.
pub async fn stop_by_user(daemon: &Daemon, id: &str) {
    let going = daemon.mcp.runs.runs.lock().unwrap().get(id).cloned();
    if let Some(going) = going {
        going.stopped_by_user.store(true, Ordering::SeqCst);
    }
    stop(daemon, id, RunState::Stopped).await;
}

// Reading and stopping.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArguments {
    run: String,
    action: Option<String>,
    tail: Option<usize>,
    start: Option<usize>,
}

/// `run_status`: the state and the output of a run this agent started, or its stop.
pub async fn run_status(daemon: &Daemon, agent: &str, args: Value) -> Outcome {
    let args: StatusArguments = match arguments("run_status", args) {
        Ok(args) => args,
        Err(refusal) => return refusal,
    };
    if let Err(refusal) = own(daemon, agent, &args.run) {
        return refusal;
    }
    match args.action.as_deref().unwrap_or("output") {
        "output" => {
            let tail = args.tail.unwrap_or(TAIL_LINES);
            if !(1..=TAIL_LINES_MAX).contains(&tail) {
                return Outcome::Refused(format!(
                    "refused: tail must be 1 to {TAIL_LINES_MAX} lines: read on with start"
                ));
            }
            Outcome::Done(describe(
                daemon,
                &args.run,
                Some(tail),
                args.start,
                false,
                false,
            ))
        }
        "stop" => {
            if args.tail.is_some() || args.start.is_some() {
                return Outcome::Refused(
                    "refused: tail and start are for action output; action stop only stops the \
                     run and answers with the end of its output"
                        .into(),
                );
            }
            stop(daemon, &args.run, RunState::Stopped).await;
            Outcome::Done(describe(daemon, &args.run, None, None, false, false))
        }
        other => Outcome::Refused(format!(
            "refused: run_status has no action {other:?}: its actions are output (the default) \
             and stop"
        )),
    }
}

/// The run, refused unless the agent started it.
fn own(daemon: &Daemon, agent: &str, id: &str) -> Result<Run, Outcome> {
    match daemon.read(|store| store.run(id)) {
        Ok(Some(run)) if run.agent.as_deref() == Some(agent) => Ok(run),
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
    by_user: bool,
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
    let state = state_text(end, by_user);
    let seconds = (run.ended_at.unwrap_or_else(now_ms) - run.started_at) as f64 / 1000.0;
    let mut text = format!("run {}: {label} {state} after {seconds:.1} s", run.id);
    if end.is_none() && answer_of_run {
        text.push_str(&format!(
            "\nit keeps going: read it with run_status {{\"run\": \"{id}\"}}, stop it with \
             run_status {{\"run\": \"{id}\", \"action\": \"stop\"}}; Flottille tells you when \
             it ends",
            id = run.id
        ));
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
            "; earlier lines: run_status {{\"run\": \"{}\", \"start\": {earlier}}}",
            run.id
        ));
    }
    if to < lines.len() {
        about.push_str(&format!(
            "; later lines: run_status {{\"run\": \"{}\", \"start\": {}}}",
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
                    json!({"agent": run.agent, "project": run.project, "run": run.id,
                        "state": "interrupted", "exit_code": null, "signal": null}),
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
        .filter(|(_, going)| going.run.agent.as_deref() == Some(agent))
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
    fn a_run_s_pushes_are_walled_unless_it_is_a_push_that_passed_the_gate() {
        for lift in [false, true] {
            let mut env = BTreeMap::new();
            defend(&mut env, Path::new("/state/empty"), lift);
            let keys: Vec<&str> = (0..3)
                .filter_map(|n| env.get(&OsString::from(format!("GIT_CONFIG_KEY_{n}"))))
                .map(|key| key.to_str().unwrap())
                .collect();
            assert_eq!(
                keys.contains(&"url./dev/null/flottille-refused-push/.pushInsteadOf"),
                !lift,
                "{keys:?}"
            );
            assert!(keys.contains(&"core.hooksPath"));
        }
    }

    #[test]
    fn a_run_gets_no_forge_token_and_no_hook() {
        let mut env: BTreeMap<OsString, OsString> = [
            ("GH_TOKEN", "ghp_x"),
            ("GIT_CONFIG_COUNT", "2"),
            ("PATH", "/bin"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();

        defend(&mut env, Path::new("/state/empty"), true);

        let get = |name: &str| env[&OsString::from(name)].to_str().unwrap().to_owned();
        assert_eq!(get("GH_TOKEN"), "");
        assert_eq!(get("GITHUB_TOKEN"), "");
        assert_eq!(get("GH_CONFIG_DIR"), "/state/empty");
        assert_eq!(get("GIT_CONFIG_COUNT"), "3");
        assert_eq!(get("GIT_CONFIG_KEY_2"), "core.hooksPath");
        assert_eq!(get("GIT_CONFIG_VALUE_2"), "/dev/null");
        assert!(!env.contains_key(&OsString::from("GIT_CONFIG_KEY_3")));
        assert_eq!(get("PATH"), "/bin");
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
