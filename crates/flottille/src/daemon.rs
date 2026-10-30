//! The daemon: one per user, behind a Unix socket speaking JSON lines (`docs/protocol.md`).

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flottille_core::paths::{Paths, ensure_private_dir};
use flottille_core::protocol::{Event, Message, Request, WatchParams, WorkspaceForge};
use flottille_core::store::Store;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinSet;

use crate::VERSION;
use crate::agents::{self, Live};
use crate::approvals;
use crate::commands;
use crate::deliveries;
use crate::forge;
use crate::inbox;
use crate::mcp;
use crate::projects::{self, Projects};
use crate::providers;
use crate::rules;
use crate::run_methods;
use crate::runs;
use crate::setup;
use crate::skills;
use crate::sweep;
use crate::tasks;
use crate::tickets;
use crate::tree;
use crate::workspaces;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Events read from the store at once when a subscriber catches up.
const PAGE: usize = 500;
/// How long the connections have, once the daemon stops, to send what they still hold.
const GRACE: Duration = Duration::from_secs(2);
/// The user only, and the sticky bit: the XDG spec lets a cleanup of `$XDG_RUNTIME_DIR` remove files
/// untouched for 6 hours, except the ones with it, and a daemon's socket and lock are just that.
const RUNTIME_FILE: u32 = 0o1600;
/// How often the daemon checks that its socket is still the one at its path.
const SOCKET_CHECK: Duration = Duration::from_secs(2);
/// What a client an agent started is answered, whatever it asks.
const REFUSED_CLIENT: &str =
    "refused: this client was started by an agent; agents act only through their tools";
/// How long a refused client is answered before it is let go.
const REFUSED_FOR: Duration = Duration::from_secs(10);

pub fn run(paths: &Paths) -> Result<()> {
    // What the daemon's descendants leave behind when their parent ends is taken in by the
    // daemon, not by init: a process an agent started stays its descendant, however it detached,
    // and so is recognised when it connects (`started_by_an_agent`).
    // SAFETY: prctl with these arguments only sets a flag of this process.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    ensure_private_dir(&paths.runtime)?;
    ensure_private_dir(&paths.state)?;
    let _lock = take_lock(paths)?;
    let _store_lock = hold_store(paths)?;
    let store = Store::open(&paths.database())?;
    // Before anything starts an agent: what a daemon that died left running is its own to kill.
    agents::kill_orphans(&store);
    crate::claude::forget_bridges(paths);
    let projects = Projects::open(paths)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(serve(paths, store, projects))
}

/// The lock is held for the daemon's whole life: the kernel lets go of it when the process ends,
/// however it ends, so a daemon that died never blocks the next one.
fn take_lock(paths: &Paths) -> Result<File> {
    let mut lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(paths.lock())?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let pid = std::fs::read_to_string(paths.lock()).unwrap_or_default();
            return Err(
                format!("a Flottille daemon is already running (pid {})", pid.trim()).into(),
            );
        }
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }
    lock.set_permissions(std::fs::Permissions::from_mode(RUNTIME_FILE))?;
    lock.set_len(0)?;
    writeln!(lock, "{}", std::process::id())?;
    Ok(lock)
}

/// The runtime lock does not see a daemon whose runtime folder was removed: it may still be
/// stopping its agents, and settling or killing them under it would undo what it writes. The store
/// stays its until it exits; a daemon that died lets go of it as it does of the runtime lock.
fn hold_store(paths: &Paths) -> Result<File> {
    let mut lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(paths.store_lock())?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let pid = std::fs::read_to_string(paths.store_lock()).unwrap_or_default();
            eprintln!(
                "flottille daemon: waiting for the daemon {} to stop, which still holds the store",
                pid.trim()
            );
            lock.lock()?;
        }
        Err(TryLockError::Error(error)) => return Err(error.into()),
    }
    lock.set_len(0)?;
    writeln!(lock, "{}", std::process::id())?;
    Ok(lock)
}

pub struct Daemon {
    pub paths: Paths,
    started: Instant,
    events: Mutex<Events>,
    /// Taken before `events` when both are: the projects' methods emit under it.
    pub projects: Mutex<Projects>,
    /// The workspaces being prepared or removed: one at a time for each.
    pub preparing: Mutex<HashSet<i64>>,
    /// The agents that have a task, and so maybe a process.
    pub agents: Live,
    /// The agents' MCP server: its tokens, and what its tools keep (reads, runs).
    pub mcp: mcp::Server,
    /// The deliveries pinned as notes on agents' next tool results.
    pub notes: deliveries::Notes,
    /// The agents waiting on their provider (overloaded, rate-limited): not stuck.
    pub waiting_provider: Mutex<HashSet<String>>,
    /// The slots launches took in each project's children cap, before their child exists.
    pub launching: Mutex<HashMap<String, usize>>,
    /// What the forges said of each workspace, and who watches them.
    pub forges: forge::Forges,
    /// Becomes true when the daemon stops.
    stopping: watch::Sender<bool>,
}

struct Events {
    store: Store,
    /// Gone once the daemon stops: subscribers then end after the last event.
    bus: Option<broadcast::Sender<Event>>,
}

/// A subscriber's place in the stream: every event after `last`, from the store then live.
struct Subscription {
    live: broadcast::Receiver<Event>,
    last: u64,
}

impl Daemon {
    /// Stores the event and hands it to the subscribers under one lock, so that they see the
    /// events in the order of their numbers.
    pub fn emit(&self, name: &str, data: Value) -> Result<Event> {
        let events = self.events.lock().unwrap();
        let event = events.store.append_event(name, data)?;
        if let Some(bus) = &events.bus {
            let _ = bus.send(event.clone());
        }
        Ok(event)
    }

    /// Reads the store.
    pub fn read<T>(
        &self,
        read: impl FnOnce(&Store) -> flottille_core::Result<T>,
    ) -> flottille_core::Result<T> {
        read(&self.events.lock().unwrap().store)
    }

    /// Writes the store, the change and its event (when it has one) in one transaction of
    /// `write`'s own, then hands the event to the subscribers, under the same lock as `emit`.
    pub fn write<T, E: Into<Option<Event>>>(
        &self,
        write: impl FnOnce(&mut Store) -> flottille_core::Result<(T, E)>,
    ) -> flottille_core::Result<T> {
        let mut events = self.events.lock().unwrap();
        let (value, event) = write(&mut events.store)?;
        if let (Some(bus), Some(event)) = (&events.bus, event.into()) {
            let _ = bus.send(event);
        }
        Ok(value)
    }

    /// Writes the store as `write` does, for a change with several events.
    pub fn write_all<T>(
        &self,
        write: impl FnOnce(&mut Store) -> flottille_core::Result<(T, Vec<Event>)>,
    ) -> flottille_core::Result<T> {
        let mut events = self.events.lock().unwrap();
        let (value, written) = write(&mut events.store)?;
        if let Some(bus) = &events.bus {
            for event in written {
                let _ = bus.send(event);
            }
        }
        Ok(value)
    }

    fn subscribe(&self, after: Option<u64>) -> Result<Subscription> {
        let events = self.events.lock().unwrap();
        let bus = events.bus.as_ref().ok_or("the daemon is stopping")?;
        // Subscribed before the store is read: an event stored meanwhile comes both ways and the
        // number drops the second copy, none falls between the two.
        let live = bus.subscribe();
        let last = match after {
            Some(after) => after,
            None => events.store.last_seq()?,
        };
        Ok(Subscription { live, last })
    }

    fn events_after(&self, after: u64) -> Result<Vec<Event>> {
        Ok(self
            .events
            .lock()
            .unwrap()
            .store
            .events_after(after, PAGE)?)
    }
}

/// The file (device, inode) the socket path leads to now.
fn socket_identity(socket: &std::path::Path) -> std::io::Result<(u64, u64)> {
    let metadata = std::fs::symlink_metadata(socket)?;
    Ok((metadata.dev(), metadata.ino()))
}

/// What the socket path leads to, against the socket the daemon bound.
#[derive(Debug, PartialEq)]
enum Socket {
    Ours,
    /// Absent, or another file now: nobody can reach this daemon.
    Gone,
    /// Not known: one failed `lstat` is no reason to stop the user's agents.
    Unknown,
}

fn judge(seen: std::io::Result<(u64, u64)>, ours: (u64, u64)) -> Socket {
    match seen {
        Ok(identity) if identity == ours => Socket::Ours,
        Ok(_) => Socket::Gone,
        Err(error) if error.kind() == ErrorKind::NotFound => Socket::Gone,
        Err(_) => Socket::Unknown,
    }
}

async fn serve(paths: &Paths, store: Store, projects: Projects) -> Result<()> {
    let socket = paths.socket();
    // Holding the lock, a socket already there is one a dead daemon left.
    match std::fs::remove_file(&socket) {
        Err(error) if error.kind() != ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(RUNTIME_FILE))?;
    let ours = socket_identity(&socket)?;
    // This machine only, on a port of the system's choosing.
    let tools = TcpListener::bind(("127.0.0.1", 0)).await?;
    let (bus, _) = broadcast::channel(1024);
    let daemon = Arc::new(Daemon {
        paths: paths.clone(),
        started: Instant::now(),
        events: Mutex::new(Events {
            store,
            bus: Some(bus),
        }),
        projects: Mutex::new(projects),
        preparing: Mutex::new(HashSet::new()),
        agents: Live::default(),
        mcp: mcp::Server::new(tools.local_addr()?.port()),
        notes: deliveries::Notes::default(),
        waiting_provider: Mutex::new(HashSet::new()),
        launching: Mutex::new(HashMap::new()),
        forges: forge::Forges::default(),
        stopping: watch::Sender::new(false),
    });
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut child_ended = signal(SignalKind::child())?;
    let mut stop_requested = daemon.stopping.subscribe();
    // A runtime folder removed under the daemon leaves nobody able to reach it, or to ask it to
    // stop: it ends itself.
    let mut socket_check =
        tokio::time::interval_at(tokio::time::Instant::now() + SOCKET_CHECK, SOCKET_CHECK);
    daemon.emit(
        "daemon.started",
        json!({"version": VERSION, "pid": std::process::id()}),
    )?;
    eprintln!(
        "flottille daemon {VERSION} listening on {}",
        socket.display()
    );
    workspaces::resume_at_start(&daemon);
    agents::settle_at_start(&daemon);
    tree::brief_at_start(&daemon);
    runs::settle_at_start(&daemon);
    approvals::settle_at_start(&daemon);
    projects::migrate_files(&daemon);
    tokio::spawn(mcp::serve(
        daemon.clone(),
        tools,
        daemon.stopping.subscribe(),
    ));
    tokio::spawn(sweep::run(daemon.clone(), daemon.stopping.subscribe()));

    let mut connections = JoinSet::new();
    let reason = loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => match started_by_an_agent(&stream) {
                    Some(pid) => {
                        eprintln!(
                            "flottille daemon: refused a client started by an agent (pid {pid})"
                        );
                        connections.spawn(refuse(stream));
                    }
                    None => {
                        connections.spawn(connection(daemon.clone(), stream));
                    }
                },
                Err(error) => eprintln!("flottille daemon: accept: {error}"),
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            _ = stop_requested.wait_for(|stopping| *stopping) => break "request",
            _ = child_ended.recv() => reap_strays(),
            _ = socket_check.tick() => {
                if judge(socket_identity(&socket), ours) == Socket::Gone {
                    break "socket";
                }
                // `/proc`'s list of children may miss one that changed while it was read.
                reap_strays();
            }
            _ = terminate.recv() => break "signal",
            _ = interrupt.recv() => break "signal",
        }
    };

    drop(listener);
    // Not the socket of another daemon that took the path since.
    if judge(socket_identity(&socket), ours) == Socket::Ours {
        std::fs::remove_file(&socket)?;
    }
    agents::stop_all(&daemon).await;
    runs::stop_all(&daemon).await;
    daemon.emit("daemon.stopping", json!({"reason": reason}))?;
    daemon.events.lock().unwrap().bus = None;
    daemon.stopping.send_replace(true);
    let _ = tokio::time::timeout(GRACE, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    eprintln!("flottille daemon stopped ({reason})");
    Ok(())
}

/// The fields of `/proc/<pid>/stat` the daemon reads.
struct Stat {
    state: char,
    parent: u32,
    group: u32,
    session: u32,
    /// In clock ticks after boot.
    started: u64,
}

fn stat(pid: u32) -> Option<Stat> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name, field 2, is in parentheses and may hold spaces.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(Stat {
        state: fields.first()?.chars().next()?,
        parent: fields.get(1)?.parse().ok()?,
        group: fields.get(2)?.parse().ok()?,
        session: fields.get(3)?.parse().ok()?,
        started: fields.get(19)?.parse().ok()?,
    })
}

/// The pid of a client that descends from the daemon. Everything the daemon starts is an agent,
/// a command of one, or a program of the daemon's own (git, a forge's command line, a provider's
/// `--version`), and none of them is a client of the daemon: such a client was started by an
/// agent. A client whose ancestry cannot be read is refused too.
fn started_by_an_agent(stream: &UnixStream) -> Option<u32> {
    let pid = stream
        .peer_cred()
        .ok()
        .and_then(|credentials| credentials.pid())
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or(0);
    descends_from(pid, std::process::id()).then_some(pid)
}

/// Whether `pid` is `ancestor` or one of its descendants, walking up the parents; a process that
/// cannot be read (gone already, or out of sight) counts as one.
fn descends_from(pid: u32, ancestor: u32) -> bool {
    let mut pid = pid;
    let Some(mut process) = stat(pid) else {
        return true;
    };
    loop {
        if pid == ancestor {
            return true;
        }
        // Up to init, or to a process with no parent in sight.
        if process.parent == 0 {
            return false;
        }
        let Some(parent) = stat(process.parent) else {
            return true;
        };
        // A parent that started after its child is another process under the pid of one that
        // ended: the child was taken in since, and its ancestry is not known.
        if parent.started > process.started {
            return true;
        }
        pid = process.parent;
        process = parent;
    }
}

/// Answers each request of a refused client with the refusal, until it leaves or `REFUSED_FOR`
/// ends: nothing it sends is done, and it reads why rather than a closed connection.
async fn refuse(stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let _ = tokio::time::timeout(REFUSED_FOR, async {
        while let Ok(Some(line)) = lines.next_line().await {
            let id = serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|request| request.get("id")?.as_str().map(str::to_owned));
            let answer = Message::error(id, "refused", REFUSED_CLIENT);
            let mut line = serde_json::to_vec(&answer).expect("a message is always JSON");
            line.push(b'\n');
            if write.write_all(&line).await.is_err() {
                break;
            }
        }
    })
    .await;
}

/// Waits for the processes the daemon took in as their subreaper and that ended, which nobody
/// else waits for. The children the daemon started are waited for by whoever started them
/// (tokio, `std::process`), by their pid: waiting for one of them here would take its status
/// away. Each of those leads its own process group (agents, runs, git, the forges' command lines)
/// or stays in the daemon's (a provider's `--version`), in the daemon's session; a process taken
/// in is in another session (it detached), or in a group led by another (the group of the command
/// it came from). One that made a group of its own in the daemon's session is not told apart, and
/// stays a zombie until the daemon exits.
fn reap_strays() {
    let daemon = std::process::id();
    let Some(own) = stat(daemon) else { return };
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return;
    };
    for task in tasks.flatten() {
        let Ok(children) = std::fs::read_to_string(task.path().join("children")) else {
            continue;
        };
        for pid in children
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok())
        {
            if let Some(child) = stat(pid)
                && child.state == 'Z'
                && (child.session != own.session
                    || (child.group != pid && child.group != own.group))
            {
                // SAFETY: waitpid has no memory precondition with a null status.
                unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG) };
            }
        }
    }
}

async fn connection(daemon: Arc<Daemon>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let (out, mut outgoing) = mpsc::channel::<Message>(256);
    let writer = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            let mut line = serde_json::to_vec(&message).expect("a message is always JSON");
            line.push(b'\n');
            if write.write_all(&line).await.is_err() {
                break;
            }
        }
    });

    let mut lines = BufReader::new(read).lines();
    let mut stopping = daemon.stopping.subscribe();
    let mut subscriptions = JoinSet::new();
    // The workspaces this connection watches: their forges are read again while it stays open.
    let mut watched: HashMap<(String, String), forge::Watch> = HashMap::new();
    let client_left = loop {
        let line = tokio::select! {
            line = lines.next_line() => line,
            _ = stopping.wait_for(|stopping| *stopping) => break false,
        };
        let Ok(Some(line)) = line else { break true };
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                let _ = out
                    .send(Message::error(None, "invalid-request", error.to_string()))
                    .await;
                continue;
            }
        };
        if request.method == "workspace.forge"
            && let Ok(asked) = serde_json::from_value::<WorkspaceForge>(request.params.clone())
            && asked.watch
            && !watched.contains_key(&(asked.project.clone(), asked.name.clone()))
            && let Ok(Some(workspace)) =
                daemon.read(|store| store.workspace(&asked.project, &asked.name))
        {
            let watch = forge::watch(&daemon, &workspace);
            watched.insert((asked.project, asked.name), watch);
        }
        // They run git or a provider's `--version`, wait on an agent's task or start what an
        // approval allows: answered by a task of their own, the connection goes on.
        if [
            "workspace.",
            "agent.",
            "approval.",
            "test.",
            "provider.",
            "run.",
        ]
        .iter()
        .any(|prefix| request.method.starts_with(prefix))
            || [
                "project.show",
                "project.repositories",
                "project.add_repository",
                "project.remove_repository",
                "project.settings",
                "project.set_settings",
                "project.ticket",
                "task.run",
            ]
            .contains(&request.method.as_str())
        {
            let (daemon, out) = (daemon.clone(), out.clone());
            tokio::spawn(async move {
                let answer = if request.method.starts_with("agent.") {
                    agents::call(daemon, &request.method, request.params).await
                } else if request.method.starts_with("approval.") {
                    approvals::call(daemon, &request.method, request.params).await
                } else if request.method.starts_with("run.") {
                    run_methods::call(daemon, &request.method, request.params).await
                } else if request.method.starts_with("provider.") {
                    providers::call(daemon, &request.method, request.params).await
                } else if request.method.starts_with("test.") {
                    sweep::call(daemon, &request.method, request.params).await
                } else if request.method == "project.show" {
                    projects::show(daemon, request.params).await
                } else if request.method == "project.repositories" {
                    projects::repositories(daemon, request.params).await
                } else if request.method == "project.add_repository" {
                    projects::add_repository(daemon, request.params).await
                } else if request.method == "project.remove_repository" {
                    projects::remove_repository(daemon, request.params).await
                } else if request.method == "project.settings" {
                    projects::settings(daemon, request.params).await
                } else if request.method == "project.set_settings" {
                    projects::set_settings(daemon, request.params).await
                } else if request.method == "project.ticket" {
                    tickets::preview(daemon, request.params).await
                } else if request.method == "task.run" {
                    tasks::run(&daemon, request.params)
                } else {
                    workspaces::call(daemon, &request.method, request.params).await
                };
                let answer = match answer {
                    Ok(result) => Message::result(request.id, result),
                    Err(failure) => Message::error(Some(request.id), failure.code, failure.message),
                };
                let _ = out.send(answer).await;
            });
            continue;
        }
        let (answer, subscription) = handle(&daemon, request);
        if out.send(answer).await.is_err() {
            break true;
        }
        // Started after the answer is queued, so the client reads the answer first.
        if let Some(subscription) = subscription {
            subscriptions.spawn(forward(daemon.clone(), subscription, out.clone()));
        }
    };

    // A client that left reads nothing more; a stopping daemon lets subscribers send the rest.
    if client_left {
        subscriptions.abort_all();
    }
    while subscriptions.join_next().await.is_some() {}
    drop(out);
    let _ = writer.await;
}

fn handle(daemon: &Daemon, request: Request) -> (Message, Option<Subscription>) {
    let id = request.id;
    match request.method.as_str() {
        "daemon.status" => (
            match status(daemon) {
                Ok(status) => Message::result(id, status),
                Err(error) => Message::error(Some(id), "unavailable", error.to_string()),
            },
            None,
        ),
        "daemon.stop" => {
            daemon.stopping.send_replace(true);
            (Message::result(id, json!({"stopping": true})), None)
        }
        "events.watch" => {
            let params = match request.params {
                Value::Null => Ok(WatchParams::default()),
                params => serde_json::from_value::<WatchParams>(params),
            };
            match params {
                Err(error) => (
                    Message::error(Some(id), "invalid-params", error.to_string()),
                    None,
                ),
                Ok(params) => match daemon.subscribe(params.after) {
                    Ok(subscription) => (
                        Message::result(id, json!({"after": subscription.last})),
                        Some(subscription),
                    ),
                    Err(error) => (
                        Message::error(Some(id), "unavailable", error.to_string()),
                        None,
                    ),
                },
            }
        }
        method if method.starts_with("inbox.") => (
            match inbox::call(daemon, method, request.params) {
                Ok(result) => Message::result(id, result),
                Err(failure) => Message::error(Some(id), failure.code, failure.message),
            },
            None,
        ),
        method if method.starts_with("rules.") => (
            match rules::call(daemon, method, request.params) {
                Ok(result) => Message::result(id, result),
                Err(failure) => Message::error(Some(id), failure.code, failure.message),
            },
            None,
        ),
        method if method.starts_with("project.") => (
            match projects::call(daemon, method, request.params) {
                Ok(result) => Message::result(id, result),
                Err(failure) => Message::error(Some(id), failure.code, failure.message),
            },
            None,
        ),
        method if method.starts_with("skill.") => (
            match skills::call(daemon, method, request.params) {
                Ok(result) => Message::result(id, result),
                Err(failure) => Message::error(Some(id), failure.code, failure.message),
            },
            None,
        ),
        method if method.starts_with("command.") => (
            match commands::call(daemon, method, request.params) {
                Ok(result) => Message::result(id, result),
                Err(failure) => Message::error(Some(id), failure.code, failure.message),
            },
            None,
        ),
        method if method.starts_with("task.") => (
            match tasks::call(daemon, method, request.params) {
                Ok(result) => Message::result(id, result),
                Err(failure) => Message::error(Some(id), failure.code, failure.message),
            },
            None,
        ),
        method if method.starts_with("setup.") => (
            match setup::call(daemon, method, request.params) {
                Ok(result) => Message::result(id, result),
                Err(failure) => Message::error(Some(id), failure.code, failure.message),
            },
            None,
        ),
        method => (
            Message::error(
                Some(id),
                "unknown-method",
                format!("unknown method `{method}`"),
            ),
            None,
        ),
    }
}

fn status(daemon: &Daemon) -> flottille_core::Result<Value> {
    use flottille_core::agent::AgentState;
    let agents = daemon.read(Store::agents)?;
    let count = |states: &[AgentState]| agents.iter().filter(|a| states.contains(&a.state)).count();
    Ok(json!({
        "version": VERSION,
        "pid": std::process::id(),
        "uptime_s": daemon.started.elapsed().as_secs(),
        "agents": {
            "running": count(&[AgentState::Starting, AgentState::Working]),
            "waiting_for_user": approvals::waiting_for_user(daemon)?,
            "failed": count(&[AgentState::Failed]),
        },
    }))
}

async fn forward(daemon: Arc<Daemon>, mut subscription: Subscription, out: mpsc::Sender<Message>) {
    loop {
        loop {
            let page = match daemon.events_after(subscription.last) {
                Ok(page) => page,
                Err(error) => {
                    eprintln!("flottille daemon: reading events: {error}");
                    return;
                }
            };
            if page.is_empty() {
                break;
            }
            for event in page {
                subscription.last = event.seq;
                if out.send(Message::event(event)).await.is_err() {
                    return;
                }
            }
        }
        loop {
            match subscription.live.recv().await {
                Ok(event) if event.seq <= subscription.last => {}
                Ok(event) => {
                    subscription.last = event.seq;
                    if out.send(Message::event(event)).await.is_err() {
                        return;
                    }
                }
                // A subscriber too slow for the bus catches up from the store.
                Err(RecvError::Lagged(_)) => break,
                Err(RecvError::Closed) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: (u64, u64) = (1, 2);

    #[test]
    fn a_process_descends_from_its_parents_not_from_its_children_and_one_gone_counts_as_one() {
        let own = std::process::id();
        let parent = stat(own).unwrap().parent;

        assert!(descends_from(own, own));
        assert!(descends_from(own, parent));
        assert!(!descends_from(parent, own));
        assert!(!descends_from(1, own));
        // No such process: refused rather than guessed.
        assert!(descends_from(u32::MAX, own));
        assert!(descends_from(0, own));
    }

    #[test]
    fn only_a_socket_path_that_is_not_found_or_leads_elsewhere_is_gone() {
        let error = |kind| Err(std::io::Error::from(kind));

        assert_eq!(judge(Ok(OURS), OURS), Socket::Ours);
        assert_eq!(judge(Ok((1, 3)), OURS), Socket::Gone);
        assert_eq!(judge(error(ErrorKind::NotFound), OURS), Socket::Gone);
        // A transient error skips the tick.
        assert_eq!(
            judge(error(ErrorKind::PermissionDenied), OURS),
            Socket::Unknown
        );
        assert_eq!(judge(error(ErrorKind::Interrupted), OURS), Socket::Unknown);
    }
}
