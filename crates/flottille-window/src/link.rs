//! The window's side of the socket, on its own thread: it connects, subscribes to the events,
//! reads what the shell shows, and reads it again when an event says it changed. On a broken
//! connection it connects again (with a backoff), resumes the events after the last one it saw and
//! reads everything again, which the events alone cannot rebuild. The UI only ever receives parsed
//! [`Update`]s; it never waits on the socket.

use std::collections::{HashMap, HashSet};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flottille_client::Connection;
use flottille_protocol::Event;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde_json::{Value, json};

use crate::actions::Outcome;
use crate::conversation::sections;
use crate::model::{self, Answers, OpenQuestion, Snapshot};

/// The open questions of each root asking the user, by its id.
type Questions = HashMap<String, Vec<OpenQuestion>>;

type Result<T> = std::result::Result<T, String>;

/// Opens a connection to the daemon, starting it when nobody listens: the command line's
/// `Client::connect_or_start` in the binary, something else in a test.
pub type Connect = Arc<dyn Fn() -> std::io::Result<UnixStream> + Send + Sync>;

/// Signs a request for the machine named first with this device's key, which the window reads
/// and the daemon never holds (`docs/remote.md`, "Requests from another machine's clients"): what
/// the line written is then. The binary gives the command line's own; a test, a key of its own.
pub type Sign = Arc<
    dyn Fn(&str, &flottille_protocol::Request) -> std::io::Result<serde_json::Value> + Send + Sync,
>;

static SIGN: std::sync::OnceLock<Sign> = std::sync::OnceLock::new();

/// How this window signs what only the user's device may ask (`vault.share`): given once, before
/// the window opens. A window given none says so when it is asked.
pub fn sign_with(sign: Sign) {
    let _ = SIGN.set(sign);
}

pub(crate) fn signer() -> Option<Sign> {
    SIGN.get().cloned()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// The close of the session `agent` of the machine `machine` was kept by this machine's
    /// daemon, or why not.
    Closed {
        machine: String,
        agent: String,
        refused: Option<String>,
    },
    /// A fresh read of everything the shell shows, with the generation of the machine shown its
    /// connection was made for ([`crate::machines::Aim::generation`]).
    Snapshot(Snapshot, u64),
    /// What the daemon keeps of the window (the sidebar's folds, and the sessions that have a
    /// draft), read once, before the first snapshot: the tree is made as it was left, not
    /// animated from its default.
    Kept(
        crate::kept::Folds,
        crate::arrange::Arrangement,
        std::collections::HashSet<String>,
        // The projects this machine hides (`window.get`, `hidden`), in the order they were hidden.
        Vec<String>,
    ),
    /// The connection broke, with why, and the generation of the machine shown it was made for;
    /// the link is connecting again.
    Disconnected(String, u64),
    /// Flottille has not answered the link for `NOT_ANSWERING`: since when it was asked, in ms
    /// after the epoch; `None` once it answers again.
    NotAnswering(Option<u64>),
    /// What came of something the window asked (`actions`).
    Done(Outcome),
    /// A fresh read of the providers (`provider.list`), with the read's number
    /// (`providers::next_read`).
    Providers(u64, crate::providers::Providers),
    /// The user's rules (of a project, or theirs for every project when `None`) or the Auto
    /// judge's guidance (`judge` true) changed, from any client: their editor reads them again.
    TextChanged {
        judge: bool,
        project: Option<String>,
    },
    /// What the daemon says of an update of Flottille (`update.show`, `update.changed`): read at
    /// each connection, so that a daemon started again from another build is seen.
    UpdateShown(Value),
    /// What the daemon says is new since the version last run (`whatsnew.show`,
    /// `whatsnew.changed`).
    WhatsNewShown(Value),
    /// What the user says of themselves changed, from any client: the settings page reads it again.
    AboutChanged,
    /// What all machines hold, or what the machine holds itself, may have changed.
    SharedChanged,
    /// The developer mode was switched, from any client: the settings page reads it again.
    DeveloperChanged,
    /// An agent filed a finding, or one was marked filed: the settings page lists them again.
    FindingsChanged,
    /// The window's appearance was chosen, from any client: the window reads it again.
    AppearanceChanged,
    /// The limit of tool results was chosen, from any client: the settings page reads it again.
    ToolResultsChanged,
    /// The compaction's settings or guidance changed, or a compactor was picked.
    CompactionChanged,
    /// After how many days an inactive session is ended was chosen, from any client: the settings
    /// page reads it again.
    SessionsChanged,
    /// A service of the machine started, became ready, ended or was changed, or a project's tasks
    /// changed what needs one: the services page reads them again.
    ServicesChanged,
    /// An entry of the vault was set or removed, from any client: the settings page reads the
    /// list again.
    VaultChanged,
    /// A program's own sign-in or sign-out moved (`signin.changed`): its name, its state and,
    /// when it failed, Flottille's sentence of why; never what the program printed.
    ProgramSignIn {
        program: String,
        state: String,
        why: Option<String>,
    },
    /// Where workspaces go was chosen (a project's own when it names one, the default of every
    /// project when `None`), from any client: the sections read it again.
    WorkspacesFolderChanged { project: Option<String> },
    /// An agent created or changed a ticket or a pull request.
    Forged(model::Forged),
    /// An agent said something, reported, or was woken: news for its session.
    News(model::News),
    /// An agent handed the user a link: a toast offers it.
    Link(model::LinkShown),
    /// An agent was told something (`agent.prompted`): it ends the row of links it was sharing.
    Prompted(String),
    /// An item of the project changed (`item.*`, which names it), or what it declares
    /// (`project.changed`): its pull requests and tickets are read again.
    Items {
        project: String,
        item: Option<crate::items::ItemKey>,
    },
    /// A part of Flottille the window draws in its places was switched or changed what it keeps
    /// (`extension.changed`, `spec.*`), or a project changed: what it shows is read again.
    Extensions,
    /// A connection's sign-in ended, or its tokens were lost or not kept (`connection.signed_in`,
    /// `.sign_in_failed`, `.signed_out`, `.tokens_unsaved`): the event's last word, the
    /// connection's name and why.
    SignIn {
        said: String,
        name: String,
        why: Option<String>,
    },
    /// What this machine's daemon says of the user's machines (`machines`).
    Machines(Box<crate::machines::News>),
    /// What the open session page's link read or saw, with the link's token.
    Session {
        token: u64,
        update: crate::session_link::SessionUpdate,
    },
    /// Another machine's notification centre, as its bell link read it (#1280): a snapshot that
    /// holds its bell and its unread counts only.
    Bell(Snapshot),
    /// The sessions that ended on another machine, as it answered when its "Ended" group was
    /// opened (#1280): a snapshot of those roots only, or why it could not be read.
    EndedThere(Result<Snapshot>),
    /// Notifications of another machine asked to be marked read there, which were not: asked
    /// again the next time.
    NotReadThere(Vec<u64>),
    /// An update of another machine's connections ([`Updates::of`]): what its pages asked, or
    /// read of it.
    Machine {
        machine: String,
        update: Box<Update>,
    },
}

/// Where the window's updates go: each one of another machine's connections is said of that
/// machine ([`Update::Machine`]), so that what is read of one machine never fills a page of
/// another.
#[derive(Clone, Debug)]
pub struct Updates {
    sender: UnboundedSender<Update>,
    machine: Option<String>,
    /// Set once the one that made them no longer takes them ([`Updates::ending`]).
    ended: Option<Arc<Ending>>,
}

/// Whether the updates are taken no more, and the connection of the link that sends them.
#[derive(Debug, Default)]
struct Ending {
    ended: AtomicBool,
    stream: Mutex<Option<UnixStream>>,
}

/// Dropped, the updates it was made with are taken no more: their link ends at once, its
/// connection shut, idle or not.
#[derive(Debug)]
pub struct End(Arc<Ending>);

impl Drop for End {
    fn drop(&mut self) {
        self.0.ended.store(true, Ordering::SeqCst);
        if let Some(stream) = self.0.stream.lock().unwrap().take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Updates {
    /// The same updates, said of `machine`.
    pub fn of(&self, machine: &str) -> Updates {
        Updates {
            sender: self.sender.clone(),
            machine: Some(machine.to_owned()),
            ended: self.ended.clone(),
        }
    }

    /// The same updates, taken until the [`End`] given with them is dropped.
    pub fn ending(&self) -> (Updates, End) {
        let ended = Arc::new(Ending::default());
        let updates = Updates {
            ended: Some(ended.clone()),
            ..self.clone()
        };
        (updates, End(ended))
    }

    fn ended(&self) -> bool {
        self.ended
            .as_ref()
            .is_some_and(|ending| ending.ended.load(Ordering::SeqCst))
    }

    /// The connection of the link that sends them: shut when they are ended, now if they are.
    fn hold(&self, stream: &UnixStream) {
        let Some(ending) = &self.ended else {
            return;
        };
        let Ok(stream) = stream.try_clone() else {
            return;
        };
        let mut held = ending.stream.lock().unwrap();
        match ending.ended.load(Ordering::SeqCst) {
            true => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            false => *held = Some(stream),
        }
    }

    /// Fails once the window is gone: nothing takes the updates any more.
    pub fn unbounded_send(&self, update: Update) -> std::result::Result<(), Gone> {
        if self.ended() {
            return Err(Gone);
        }
        let update = match &self.machine {
            Some(machine) => Update::Machine {
                machine: machine.clone(),
                update: Box::new(update),
            },
            None => update,
        };
        self.sender.unbounded_send(update).map_err(|_| Gone)
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed() || self.ended()
    }
}

/// The window that took the updates is gone.
#[derive(Debug)]
pub struct Gone;

impl From<UnboundedSender<Update>> for Updates {
    fn from(sender: UnboundedSender<Update>) -> Updates {
        Updates {
            sender,
            machine: None,
            ended: None,
        }
    }
}

/// What the link has seen, readable from any thread: the number of the last event, and how many
/// events came that did not follow the one before (a gap in the stream).
#[derive(Debug, Default)]
pub struct Seen {
    pub last_seq: AtomicU64,
    pub events: AtomicU64,
    pub gaps: AtomicU64,
    pub connections: AtomicU64,
    /// The reads of the shell sent to the window: the first one says the link follows the events,
    /// which it subscribes to before it reads.
    pub snapshots: AtomicU64,
    /// The `project.changed` events seen.
    pub project_changes: AtomicU64,
    /// When the link asked what it waits for, in ms after the epoch; 0 while it waits for nothing.
    pub asking: Arc<AtomicU64>,
    /// The generation of the machine shown ([`crate::machines::Aim::generations`]): 0 for good
    /// when the window shows only this machine.
    pub aimed: Arc<AtomicU64>,
}

/// How long a request of the link may wait before the window says Flottille is not answering.
pub const NOT_ANSWERING: Duration = Duration::from_secs(30);

/// [`NOT_ANSWERING`], or what a scenario of the window set for the window it opens on its thread.
pub(crate) fn not_answering() -> Duration {
    #[cfg(feature = "test-support")]
    if let Some(limit) = NOT_ANSWERING_HERE.with(std::cell::Cell::get) {
        return limit;
    }
    NOT_ANSWERING
}

#[cfg(feature = "test-support")]
thread_local! {
    static NOT_ANSWERING_HERE: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
}

/// For a scenario of the window: the windows opened on this thread say Flottille is not
/// answering after `limit`, not [`NOT_ANSWERING`]. A thread of its own, so that the scenarios
/// that run beside it keep the production's.
#[cfg(feature = "test-support")]
pub fn not_answering_after_on_this_thread(limit: Duration) {
    NOT_ANSWERING_HERE.with(|here| here.set(Some(limit)));
}

/// How long the link waits for nothing before it asks Flottille whether it still answers.
const PING: Duration = Duration::from_secs(15);

const FIRST_RETRY: Duration = Duration::from_millis(100);
const LAST_RETRY: Duration = Duration::from_secs(5);
/// Events that come together are taken in one go before what they change is read again: once
/// they pause this long, or this long after the first of them at most, so that an agent
/// streaming without a pause does not hold the rest back.
const QUIET: Duration = Duration::from_millis(40);
const LONGEST: Duration = Duration::from_millis(250);

/// How long to wait for more events, when what they change was first known to be stale at
/// `since`; `None` (as long as it takes) when nothing is.
pub(crate) fn quiet_wait(since: Option<Instant>) -> Option<Duration> {
    since.map(|since| QUIET.min(LONGEST.saturating_sub(since.elapsed())))
}

/// Starts the link's thread. It ends when the receiver is dropped.
pub fn spawn(connect: Connect) -> (UnboundedReceiver<Update>, Arc<Seen>) {
    let (sender, receiver) = unbounded();
    (receiver, spawn_into(connect, sender))
}

/// Starts the link's thread, sending its updates to `sender`.
pub fn spawn_into(connect: Connect, sender: impl Into<Updates>) -> Arc<Seen> {
    spawn_aimed_into(connect, Arc::default(), sender)
}

/// [`spawn_into`], on connections made to the machine shown, whose generation is `aimed`: what
/// the link reads and says is stamped with the generation its connection was made for.
pub fn spawn_aimed_into(
    connect: Connect,
    aimed: Arc<AtomicU64>,
    sender: impl Into<Updates>,
) -> Arc<Seen> {
    let sender: Updates = sender.into();
    let seen = Arc::new(Seen {
        aimed,
        ..Seen::default()
    });
    let shared = seen.clone();
    let watched = (seen.asking.clone(), sender.clone());
    let limit = not_answering();
    std::thread::Builder::new()
        .name("flottille-link".into())
        .spawn(move || run(connect, sender, shared))
        .expect("a thread for the link to Flottille");
    std::thread::Builder::new()
        .name("flottille-link-watch".into())
        .spawn(move || watch(&watched.0, &watched.1, limit))
        .expect("a thread watching the link to Flottille");
    seen
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Says when what the link asked waits past `limit`, and when it is answered after that; ends
/// with the window.
fn watch(asking: &AtomicU64, updates: &Updates, limit: Duration) {
    let limit_ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX);
    let mut told = None;
    while !updates.is_closed() {
        std::thread::sleep((limit / 10).min(Duration::from_millis(500)));
        let since = Some(asking.load(Ordering::SeqCst)).filter(|since| *since != 0);
        let late = since.filter(|since| now_ms().saturating_sub(*since) > limit_ms);
        match (late, told) {
            (Some(since), None) => {
                told = Some(since);
                let _ = updates.unbounded_send(Update::NotAnswering(Some(since)));
            }
            // The request it was told of was answered, or the connection broke.
            (_, Some(said)) if since != Some(said) => {
                told = None;
                let _ = updates.unbounded_send(Update::NotAnswering(None));
            }
            _ => {}
        }
    }
}

fn run(connect: Connect, updates: Updates, seen: Arc<Seen>) {
    let mut retry = FIRST_RETRY;
    let mut briefs = Briefs::default();
    let reads = ProviderReads::default();
    loop {
        let aimed = seen.aimed.load(Ordering::SeqCst);
        let why = match connect() {
            // Made while another machine was chosen: made again, for that one.
            Ok(_) if seen.aimed.load(Ordering::SeqCst) != aimed => continue,
            Ok(stream) => {
                seen.connections.fetch_add(1, Ordering::Relaxed);
                retry = FIRST_RETRY;
                match follow(
                    stream,
                    &connect,
                    &updates,
                    &seen,
                    &mut briefs,
                    &reads,
                    aimed,
                ) {
                    Ok(()) => return,
                    Err(why) => why,
                }
            }
            Err(error) => format!("cannot reach Flottille: {error}"),
        };
        if updates
            .unbounded_send(Update::Disconnected(why, aimed))
            .is_err()
        {
            return;
        }
        std::thread::sleep(retry);
        retry = (retry * 2).min(LAST_RETRY);
    }
}

/// Follows one connection until it breaks (an error) or the window is gone (`Ok`).
fn follow(
    stream: UnixStream,
    connect: &Connect,
    updates: &Updates,
    seen: &Seen,
    briefs: &mut Briefs,
    reads: &ProviderReads,
    aimed: u64,
) -> Result<()> {
    updates.hold(&stream);
    let mut conn = Conn::new(stream).map_err(|e| e.to_string())?;
    conn.asking = Some(seen.asking.clone());
    let last = seen.last_seq.load(Ordering::Relaxed);
    let after = (seen.connections.load(Ordering::Relaxed) > 1 || last > 0).then_some(last);
    // A window: the daemon reads the items at the window's rhythm while it is open.
    let started = conn.request("events.watch", json!({"after": after, "client": "window"}))?;
    if after.is_none() {
        seen.last_seq
            .store(started["after"].as_u64().unwrap_or(0), Ordering::Relaxed);
    }
    // Before the first snapshot, so that the nodes of the tree are made unfolded or folded as the
    // user left them. A daemon that cannot say leaves every fold at its default.
    if seen.connections.load(Ordering::Relaxed) == 1
        && let Ok(state) = conn.request("window.get", json!({}))
    {
        // The sessions that hold a draft: their rows say it before their pages are opened.
        let drafts = conn
            .request("draft.list", json!({}))
            .ok()
            .and_then(|listed| {
                listed["drafts"].as_array().map(|keys| {
                    keys.iter()
                        .filter_map(|key| key.as_str()?.strip_prefix("session:"))
                        .map(str::to_owned)
                        .collect()
                })
            })
            .unwrap_or_default();
        let kept = Update::Kept(
            crate::kept::Folds::of(&state["state"]),
            crate::arrange::Arrangement::of(&state["state"]["arrangement"]),
            drafts,
            state["state"]["hidden"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect(),
        );
        if updates.unbounded_send(kept).is_err() {
            return Ok(());
        }
    }
    // A daemon too old to say it leaves the version section waiting.
    if let Ok(shown) = conn.request("update.show", json!({}))
        && updates.unbounded_send(Update::UpdateShown(shown)).is_err()
    {
        return Ok(());
    }
    if let Ok(shown) = conn.request("whatsnew.show", json!({}))
        && updates
            .unbounded_send(Update::WhatsNewShown(shown))
            .is_err()
    {
        return Ok(());
    }
    // When what the shell shows, and the providers, were first known to be stale.
    let mut stale = Some(Instant::now());
    let mut providers = Some(Instant::now());
    let mark = |event: &Event, stale: &mut Option<Instant>, providers: &mut Option<Instant>| {
        note(seen, event);
        if matters(event) {
            stale.get_or_insert_with(Instant::now);
        }
        if about_providers(event) {
            providers.get_or_insert_with(Instant::now);
        }
        if let Some(changed) = settings_changed(event) {
            let _ = updates.unbounded_send(changed);
        }
        if event.name == "update.changed" {
            let _ = updates.unbounded_send(Update::UpdateShown(event.data.clone()));
        }
        if event.name == "whatsnew.changed" {
            let _ = updates.unbounded_send(Update::WhatsNewShown(event.data.clone()));
        }
        if event.name == "appearance.changed" {
            let _ = updates.unbounded_send(Update::AppearanceChanged);
        }
        if event.name.starts_with("service.") || event.name == "project.changed" {
            let _ = updates.unbounded_send(Update::ServicesChanged);
        }
        if let Some(forged) = model::Forged::of(event) {
            let _ = updates.unbounded_send(Update::Forged(forged));
        }
        if let Some(news) = model::News::of(event) {
            let _ = updates.unbounded_send(Update::News(news));
        }
        if extensions_of(event) {
            let _ = updates.unbounded_send(Update::Extensions);
        }
        if event.name == "agent.prompted"
            && let Some(agent) = event.data["agent"].as_str()
        {
            let _ = updates.unbounded_send(Update::Prompted(agent.to_owned()));
        }
        if let Some(link) = model::LinkShown::of(event) {
            let _ = updates.unbounded_send(Update::Link(link));
        }
        if let Some((project, item)) = items_of(event) {
            let _ = updates.unbounded_send(Update::Items { project, item });
        }
    };
    // When the daemon last said anything: a link that heard nothing for `PING` asks it.
    let mut heard = Instant::now();
    loop {
        // Everything that came already, then a short quiet (or a short while at most), before
        // reading the shell again.
        let since = [stale, providers].into_iter().flatten().min();
        let wait = quiet_wait(since).unwrap_or_else(|| PING.saturating_sub(heard.elapsed()));
        match conn.next_event(Some(wait))? {
            Some(event) => {
                heard = Instant::now();
                mark(&event, &mut stale, &mut providers);
            }
            None if since.is_none() => {
                conn.request("daemon.status", json!({}))?;
                heard = Instant::now();
            }
            None => {
                if stale.take().is_some() {
                    let snapshot = read(&mut conn, briefs, seen)?;
                    if updates
                        .unbounded_send(Update::Snapshot(snapshot, aimed))
                        .is_err()
                    {
                        return Ok(());
                    }
                    seen.snapshots.fetch_add(1, Ordering::Relaxed);
                }
                if providers.take().is_some() {
                    reads.start(connect, updates);
                }
                // Events that came while it read are queued: they make it stale again.
                while let Some(event) = conn.queued() {
                    mark(&event, &mut stale, &mut providers);
                }
            }
        }
    }
}

/// The project whose pull requests and tickets an event says changed, and the item when it names
/// one: an `item.*`, a change of what the project declares, or a source's items forgotten.
fn items_of(event: &Event) -> Option<(String, Option<crate::items::ItemKey>)> {
    let data = &event.data;
    let text = |field: &str| data[field].as_str().map(str::to_owned);
    match event.name.as_str() {
        "project.changed" => Some((text("name")?, None)),
        "items.forgotten" => Some((text("project")?, None)),
        name if name.starts_with("item.") => {
            let item = crate::items::ItemKey {
                kind: text("kind")?,
                repository: text("repository").unwrap_or_default(),
                key: text("key")?,
            };
            Some((text("project")?, Some(item)))
        }
        _ => None,
    }
}

/// Whether an event may change what the window draws in the places of a part of Flottille: one
/// switched, a spec made or changed, a review's guidance or findings changed, a project's file
/// changed (its own switch is there) or a project added or removed.
fn extensions_of(event: &Event) -> bool {
    let name = event.name.as_str();
    name == "extension.changed"
        || name.starts_with("spec.")
        || name.starts_with("review.")
        || name.starts_with("connection.")
        || matches!(
            name,
            "project.changed" | "project.added" | "project.removed"
        )
}

/// Whether an event may change what `provider.list` says: a provider enabled or disabled, or a
/// session opened or given another model, or the models of a provider read, which say the models
/// it offers and the efforts of the one a session has.
fn about_providers(event: &Event) -> bool {
    event.name.starts_with("provider.")
        // All machines' judge and new session are what `provider.list` says too.
        || event.name == "settings.shared_changed"
        || event.name == "agent.started"
        || event.name == "agent.options_taken"
}

/// What an event says changed of the machine's settings, which its Settings read again: from
/// the shell's link, and from the bell link of another machine, which follows it while its
/// Settings are shown (#1476).
fn settings_changed(event: &Event) -> Option<Update> {
    let text = |key: &str| event.data[key].as_str().map(str::to_owned);
    let name = event.name.as_str();
    if let Some(said) = name.strip_prefix("connection.")
        && matches!(
            said,
            "signed_in" | "sign_in_failed" | "signed_out" | "tokens_unsaved"
        )
    {
        return Some(Update::SignIn {
            said: said.to_owned(),
            name: text("name").unwrap_or_default(),
            why: text("why"),
        });
    }
    Some(match name {
        "judge.changed" => Update::TextChanged {
            judge: true,
            project: None,
        },
        "rules.changed" => Update::TextChanged {
            judge: false,
            project: text("project"),
        },
        "about.changed" => Update::AboutChanged,
        "settings.shared_changed"
        | "provider.new_session_changed"
        | "provider.judge_changed"
        | "extension.changed"
        | "connection.changed" => Update::SharedChanged,
        "developer.changed" => Update::DeveloperChanged,
        "developer.reported" => Update::FindingsChanged,
        "workspaces_folder.changed" => Update::WorkspacesFolderChanged {
            project: text("project"),
        },
        "vault.changed" => Update::VaultChanged,
        "signin.changed" => Update::ProgramSignIn {
            program: text("program").unwrap_or_default(),
            state: text("state").unwrap_or_default(),
            why: text("why"),
        },
        "sessions.changed" => Update::SessionsChanged,
        "compaction.changed" => Update::CompactionChanged,
        "tool_results.changed" => Update::ToolResultsChanged,
        _ => return None,
    })
}

/// The providers' reads, one at a time: they run on a thread and a connection of their own, with
/// a bounded wait, so that the events are never held behind them (`--version` may take seconds).
/// A read asked while one is in flight only marks that another is due, which the thread makes
/// when its read ends: a lead starting many sessions against a slow daemon gives a read and a
/// last one, not one read each.
#[derive(Clone, Default)]
struct ProviderReads(Arc<Mutex<ReadState>>);

#[derive(Default)]
struct ReadState {
    running: bool,
    due: bool,
}

impl ProviderReads {
    fn start(&self, connect: &Connect, updates: &Updates) {
        let (reads, connect, updates) = (self.clone(), connect.clone(), updates.clone());
        self.begin(move || {
            std::thread::Builder::new()
                .name("flottille-providers".into())
                .spawn(move || reads.run(&connect, &updates))
                .map(drop)
        });
    }

    /// Runs `spawn` when no read is in flight, else marks that another is due. A thread that
    /// cannot start is told and skipped: the next event asks again.
    fn begin(&self, spawn: impl FnOnce() -> std::io::Result<()>) {
        if !self.ask() {
            return;
        }
        if let Err(error) = spawn() {
            eprintln!("flottille: the providers cannot be read: {error}");
            *self.0.lock().unwrap() = ReadState::default();
        }
    }

    /// Whether to read now; when a read is in flight, it will read again when it ends.
    fn ask(&self) -> bool {
        let mut state = self.0.lock().unwrap();
        if state.running {
            state.due = true;
            return false;
        }
        state.running = true;
        true
    }

    /// A read ended: whether another is due. When none is, no read is in flight any more.
    fn ended(&self) -> bool {
        let mut state = self.0.lock().unwrap();
        state.running = std::mem::take(&mut state.due);
        state.running
    }

    fn run(&self, connect: &Connect, updates: &Updates) {
        loop {
            // Numbered as it is asked, whoever asks (`providers::next_read`).
            let read = crate::providers::next_read();
            if let Some(listed) =
                crate::actions::read_providers(connect, crate::actions::BOUNDS.quick)
            {
                let _ = updates.unbounded_send(Update::Providers(read, listed));
            }
            if !self.ended() {
                return;
            }
        }
    }
}

fn note(seen: &Seen, event: &Event) {
    let last = seen.last_seq.swap(event.seq, Ordering::Relaxed);
    if last != 0 && event.seq != last + 1 {
        seen.gaps.fetch_add(1, Ordering::Relaxed);
    }
    seen.events.fetch_add(1, Ordering::Relaxed);
    if event.name == "project.changed" {
        seen.project_changes.fetch_add(1, Ordering::Relaxed);
    }
}

/// Whether an event changes what the shell shows. A conversation's pieces and tool calls do not:
/// they are many, and the session page follows them itself.
fn matters(event: &Event) -> bool {
    let name = event.name.as_str();
    match name.split_once('.').map(|(subject, _)| subject) {
        // A workspace's preparation shows in its place, step by step.
        Some("project" | "approval" | "daemon" | "workspace") => true,
        // The notification centre: a notification came, was read or was answered, in any client.
        Some("notifications") => true,
        // A request for a vault entry marks its session and rings the bell until it is answered.
        Some("vault") => matches!(name, "vault.asked" | "vault.ask_answered"),
        // A subscription opening or ending moves the session from waiting to rest.
        Some("subscription") => true,
        // A following that pauses or resumes shows on its row in the rail.
        Some("session") => name.starts_with("session.following_"),
        // A command starting or ending shows in the tree; its output is no event.
        Some("run") => true,
        Some("agent") => matches!(
            name,
            "agent.created"
                | "agent.started"
                | "agent.prompted"
                | "agent.turn_ended"
                | "agent.released"
                | "agent.died"
                | "agent.replaced"
                | "agent.failed"
                | "agent.ended"
                | "agent.resumed"
                | "agent.reported"
                | "agent.asked"
                | "agent.answered"
                // A session that removed its own workspace goes on under main.
                | "agent.moved"
                // A session's title, from its agent or the user.
                | "agent.titled"
                // What a session is about, which its row and its page say.
                | "agent.updated"
                // The session's state says "Compacting" from the first to the last of them.
                | "agent.compacting"
                | "agent.compacted"
                | "agent.compaction_failed"
                // A session that waits for its quota's reset says it on its row.
                | "agent.quota"
                // A session given another model or effort says it on its row.
                | "agent.options_taken"
                // Blocked by someone else, or no longer: its mark, its row's place, its banner.
                | "agent.blocked"
        ),
        _ => false,
    }
}

/// A fresh read of everything the shell shows, on any connection.
pub fn read_once(stream: UnixStream) -> Result<Snapshot> {
    let mut conn = Conn::new(stream).map_err(|e| e.to_string())?;
    read(&mut conn, &mut Briefs::default(), &Seen::default())
}

/// The message each root is titled by, and how far the search for it has gone.
#[derive(Default)]
struct Briefs {
    texts: HashMap<String, String>,
    /// For a root whose first message is a greeting: the last event read of its conversation, so
    /// that each read goes on from there.
    scanned: HashMap<String, u64>,
}

impl Briefs {
    /// The title follows the first message of the user that is more than a greeting: the first
    /// message is no title when it says no work, and the later ones are looked through, once.
    fn follow(&mut self, conn: &mut Conn, id: &str) -> Result<()> {
        let mut after = self.scanned.get(id).copied().unwrap_or(0);
        loop {
            let shown = conn.request("agent.show", json!({"agent": id, "after": after}))?;
            for event in shown["events"].as_array().into_iter().flatten() {
                after = event["seq"].as_u64().unwrap_or(after);
                if event["name"] != "agent.prompted" {
                    continue;
                }
                let text = event["data"]["text"].as_str().unwrap_or_default();
                let said = sections(text)
                    .into_iter()
                    .filter(|(kind, _)| matches!(kind.as_str(), "" | "user"))
                    .find(|(_, body)| !model::title_of(body).is_empty());
                if let Some((_, body)) = said {
                    self.texts.insert(id.to_owned(), body);
                    return Ok(());
                }
            }
            if shown["more"] != true {
                self.scanned.insert(id.to_owned(), after);
                return Ok(());
            }
        }
    }
}

fn read(conn: &mut Conn, briefs: &mut Briefs, seen: &Seen) -> Result<Snapshot> {
    let projects = conn.request("project.list", json!({}))?;
    let workspaces = conn.request("workspace.list", json!({}))?;
    let agents = conn.request("agent.list", json!({}))?;
    let approvals = conn.request("approval.list", json!({}))?;
    let vault_asks = conn.request("vault.asks", json!({}))?;
    let commands = conn.request("run.list", json!({}))?;
    let subscriptions = conn.request("subscription.list", json!({}))?;
    let notifications = conn.request("notifications.list", json!({"limit": 1000}))?;
    let (needs_user, questions) = needing_the_user(conn)?;
    let roots: Vec<&str> = agents["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|agent| agent["parent"].is_null())
        .filter_map(|agent| agent["id"].as_str())
        .collect();
    // A root's first message is its brief and never changes: read once.
    let now = seen.last_seq.load(Ordering::Relaxed);
    for id in roots {
        if !briefs.texts.contains_key(id) {
            let shown = conn.request("agent.show", json!({"agent": id, "after": now}))?;
            let brief = shown["agent"]["brief"].as_str().unwrap_or_default();
            briefs.texts.insert(id.to_owned(), brief.to_owned());
        }
        if model::title_of(&briefs.texts[id]).is_empty() {
            briefs.follow(conn, id)?;
        }
    }
    let mut snapshot = Snapshot::build(Answers {
        projects: &projects,
        workspaces: &workspaces,
        agents: &agents,
        approvals: &approvals,
        vault_asks: &vault_asks,
        needs_user: &needs_user,
        briefs: &briefs.texts,
        questions: &questions,
        commands: &commands,
        subscriptions: &subscriptions,
    });
    snapshot.set_notifications(&notifications);
    snapshot.project_changes = seen.project_changes.load(Ordering::Relaxed);
    Ok(snapshot)
}

/// The roots the daemon says need the user, and the open questions of each, as `agent.trees`
/// gives them (the live roots, a page of them at a time): the window does not decide this
/// itself.
fn needing_the_user(conn: &mut Conn) -> Result<(HashSet<String>, Questions)> {
    let mut found = HashSet::new();
    let mut questions = Questions::new();
    let mut before: Option<String> = None;
    loop {
        let page = conn.request("agent.trees", json!({"before": before}))?;
        let trees = page["trees"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        for tree in trees {
            let id = tree["status"]["agent"]["id"].as_str().unwrap_or_default();
            if tree["status"]["needs_user"] == true {
                found.insert(id.to_owned());
            }
            let open = OpenQuestion::of_entry(&tree["status"]);
            if !open.is_empty() {
                questions.insert(id.to_owned(), open);
            }
        }
        before = trees
            .last()
            .and_then(|tree| tree["status"]["agent"]["id"].as_str())
            .map(str::to_owned);
        if page["more"] != true || before.is_none() {
            return Ok((found, questions));
        }
    }
}

/// Follows the notification centre of the machine `connect` reaches, for the bell (#1280): read
/// at each connection and whenever a notification comes, is read or is answered there. Its
/// settings and its providers follow too while its pages are shown (#1476, #1483): its events say
/// what changed, and the providers are read again, off this link ([`ProviderReads`]), so that a
/// daemon slow to list them never holds its notices. Connects again with a backoff, which starts
/// over only once the machine was read: a connection that this machine's daemon makes and the
/// other refuses at once is no reason to ask again at once. The events resume after the last one
/// seen, so that nothing made while it was cut is lost. Ends with its updates.
pub fn spawn_bell_into(connect: Connect, updates: Updates) {
    std::thread::Builder::new()
        .name("flottille-bell".into())
        .spawn(move || {
            let mut retry = FIRST_RETRY;
            let mut last = None;
            let reads = ProviderReads::default();
            while !updates.is_closed() {
                if let Ok(stream) = connect()
                    && let Ok(()) =
                        follow_bell(stream, &connect, &updates, &reads, &mut retry, &mut last)
                {
                    return;
                }
                std::thread::sleep(retry);
                retry = (retry * 2).min(LAST_RETRY);
            }
        })
        .expect("a thread for the bell of another machine");
}

fn follow_bell(
    stream: UnixStream,
    connect: &Connect,
    updates: &Updates,
    reads: &ProviderReads,
    retry: &mut Duration,
    last: &mut Option<u64>,
) -> Result<()> {
    updates.hold(&stream);
    let mut conn = Conn::new(stream).map_err(|e| e.to_string())?;
    // The first connection starts after the events there are; a later one, after the last it saw.
    let started = conn.request("events.watch", json!({"after": *last}))?;
    if last.is_none() {
        *last = Some(started["after"].as_u64().unwrap_or(0));
    }
    // Only a provider's own events: a session's start says no change of the providers here.
    let providers_of = |event: &Event| event.name.starts_with("provider.");
    let about = |event: &Event| event.name.starts_with("notifications.");
    let mut stale = Some(Instant::now());
    let mut providers: Option<Instant> = None;
    let mut heard = Instant::now();
    loop {
        let since = [stale, providers].into_iter().flatten().min();
        let wait = quiet_wait(since).unwrap_or_else(|| PING.saturating_sub(heard.elapsed()));
        match conn.next_event(Some(wait))? {
            Some(event) => {
                heard = Instant::now();
                *last = Some(event.seq);
                if about(&event) {
                    stale.get_or_insert_with(Instant::now);
                }
                if providers_of(&event) {
                    providers.get_or_insert_with(Instant::now);
                }
                if let Some(changed) = settings_changed(&event)
                    && updates.unbounded_send(changed).is_err()
                {
                    return Ok(());
                }
            }
            None if since.is_none() => {
                conn.request("daemon.status", json!({}))?;
                heard = Instant::now();
            }
            None => {
                if stale.take().is_some() {
                    let mut read = Snapshot::default();
                    read.set_notifications(
                        &conn.request("notifications.list", json!({"limit": 1000}))?,
                    );
                    if updates.unbounded_send(Update::Bell(read)).is_err() {
                        return Ok(());
                    }
                    *retry = FIRST_RETRY;
                }
                if providers.take().is_some() {
                    // On a thread of its own, one at a time: a daemon slow to list them never
                    // holds the notices of this link.
                    reads.start(connect, updates);
                }
                // Events that came while it read are queued: they make it stale again.
                while let Some(event) = conn.queued() {
                    *last = Some(event.seq);
                    if about(&event) {
                        stale.get_or_insert_with(Instant::now);
                    }
                    if providers_of(&event) {
                        providers.get_or_insert_with(Instant::now);
                    }
                    if let Some(changed) = settings_changed(&event) {
                        let _ = updates.unbounded_send(changed);
                    }
                }
            }
        }
    }
}

/// Marks notifications of the machine `connect` reaches read (`notifications.read`), by a
/// connection of its own, off the UI thread: its bell link reads them again on the event that
/// follows. Not marked, `ids` are said back ([`Update::NotReadThere`]).
pub fn read_there(connect: Connect, params: Value, ids: Vec<u64>, updates: Updates) {
    let spawned = std::thread::Builder::new()
        .name("flottille-bell-read".into())
        .spawn(move || {
            let marked = connect()
                .map_err(|error| error.to_string())
                .and_then(|stream| Conn::new(stream).map_err(|error| error.to_string()))
                .and_then(|mut conn| {
                    conn.ask_within("notifications.read", params, NOT_ANSWERING)?
                })
                .map(drop);
            if let Err(why) = marked {
                eprintln!("flottille: the notifications were not marked read: {why}");
                let _ = updates.unbounded_send(Update::NotReadThere(ids));
            }
        });
    if let Err(error) = spawned {
        eprintln!("flottille: the notifications cannot be marked read: {error}");
    }
}

/// How many of a machine's ended sessions its "Ended" group lists here: each is a round trip
/// over the link, for its brief.
pub const ENDED_THERE: usize = 50;

/// The sessions that ended on the machine `connect` reaches, the latest [`ENDED_THERE`], each
/// titled as its own window titles it (#1280), by a connection of their own.
pub fn read_ended(connect: &Connect, bound: Duration) -> Result<Snapshot> {
    let stream = connect().map_err(|error| format!("cannot reach it: {error}"))?;
    let mut conn = Conn::new(stream).map_err(|e| e.to_string())?;
    let mut ask = |method: &str, params: Value| -> Result<Value> {
        conn.ask_within(method, params, bound)?
            .map_err(|message| format!("{method}: {message}"))
    };
    let listed = ask("agent.list", json!({}))?;
    let mut ended: Vec<Value> = listed["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|agent| agent["parent"].is_null())
        .filter(|agent| matches!(agent["state"].as_str(), Some("ended" | "failed")))
        .cloned()
        .collect();
    ended.sort_by_key(|agent| std::cmp::Reverse(agent["ended_at"].as_u64().unwrap_or(0)));
    ended.truncate(ENDED_THERE);
    let mut briefs = HashMap::new();
    for root in &ended {
        let Some(id) = root["id"].as_str() else {
            continue;
        };
        // Its agent alone: no event is numbered before the first. One that is gone since the
        // list is left out, not the whole read.
        let Ok(shown) = conn.ask_within("agent.show", json!({"agent": id, "before": 1}), bound)?
        else {
            continue;
        };
        let brief = shown["agent"]["brief"].as_str().unwrap_or_default();
        briefs.insert(id.to_owned(), brief.to_owned());
    }
    ended.retain(|root| {
        root["id"]
            .as_str()
            .is_some_and(|id| briefs.contains_key(id))
    });
    let nothing = json!({});
    Ok(Snapshot::build(Answers {
        projects: &nothing,
        workspaces: &nothing,
        agents: &json!({"agents": ended}),
        approvals: &nothing,
        vault_asks: &nothing,
        needs_user: &HashSet::new(),
        briefs: &briefs,
        questions: &HashMap::new(),
        commands: &nothing,
        subscriptions: &nothing,
    }))
}

/// One connection: requests answered in turn, events kept aside meanwhile.
pub(crate) struct Conn {
    link: Connection<UnixStream>,
    /// Where the link's own connection says when it asked what it waits for (`Seen::asking`).
    asking: Option<Arc<AtomicU64>>,
}

impl Conn {
    pub(crate) fn new(stream: UnixStream) -> std::io::Result<Conn> {
        stream.set_read_timeout(None)?;
        Ok(Conn {
            link: Connection::new(stream),
            asking: None,
        })
    }

    /// Every request of this connection signed for the machine `machine` by `sign`.
    pub(crate) fn signing(mut self, sign: Sign, machine: String) -> Conn {
        self.link = self
            .link
            .signing(Box::new(move |request| sign(&machine, request)));
        self
    }

    pub(crate) fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.ask(method, params)?
            .map_err(|message| format!("{method}: {message}"))
    }

    /// The daemon's answer: its result, or the sentence of its error. The outer error is the
    /// connection's.
    pub(crate) fn ask(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<std::result::Result<Value, String>> {
        Ok(self
            .ask_until(method, params, None)?
            .map_err(|error| error.message))
    }

    /// [`Conn::ask`], waiting at most `bound`: past it the daemon is taken as silent, an error of
    /// the connection, which is not to be used again (its late answer would come to the next
    /// request).
    pub(crate) fn ask_within(
        &mut self,
        method: &str,
        params: Value,
        bound: Duration,
    ) -> Result<std::result::Result<Value, String>> {
        Ok(self
            .ask_until(method, params, Some(bound))?
            .map_err(|error| error.message))
    }

    /// [`Conn::ask_within`], with the code of the daemon's error beside its sentence.
    pub(crate) fn ask_within_coded(
        &mut self,
        method: &str,
        params: Value,
        bound: Duration,
    ) -> Result<flottille_client::Answer> {
        self.ask_until(method, params, Some(bound))
    }

    fn ask_until(
        &mut self,
        method: &str,
        params: Value,
        bound: Option<Duration>,
    ) -> Result<flottille_client::Answer> {
        if let Some(asking) = &self.asking {
            asking.store(now_ms(), Ordering::SeqCst);
        }
        let answer = self.answer_of(method, params, bound);
        if let Some(asking) = &self.asking {
            asking.store(0, Ordering::SeqCst);
        }
        answer
    }

    fn answer_of(
        &mut self,
        method: &str,
        params: Value,
        bound: Option<Duration>,
    ) -> Result<flottille_client::Answer> {
        let params = match params {
            Value::Object(mut map) => {
                map.retain(|_, value| !value.is_null());
                Value::Object(map)
            }
            other => other,
        };
        let answer = match bound {
            None => self.link.ask(method, params),
            Some(bound) => self.link.ask_within(method, params, bound),
        };
        match answer {
            Ok(answer) => Ok(answer),
            Err(flottille_client::Error::TimedOut) => Err(format!(
                "Flottille did not answer {method} within {}",
                spoken(bound.unwrap_or_default())
            )),
            Err(error) => Err(said(error)),
        }
    }

    /// The next event, waiting at most `wait` (`None`: as long as it takes); `None` when none
    /// came in time.
    pub(crate) fn next_event(&mut self, wait: Option<Duration>) -> Result<Option<Event>> {
        self.link.next_event(wait).map_err(said)
    }

    /// The next event that came while a request waited for its answer, without reading.
    fn queued(&mut self) -> Option<Event> {
        self.link.queued()
    }
}

/// What went wrong with the connection, as the window says it.
fn said(error: flottille_client::Error) -> String {
    match error {
        flottille_client::Error::Io(error) => format!("the link to Flottille: {error}"),
        flottille_client::Error::Closed => "Flottille closed the connection".into(),
        flottille_client::Error::TimedOut => "Flottille did not answer in time".into(),
        flottille_client::Error::NotAMessage(error) => {
            format!("Flottille sent a line that is not a message: {error}")
        }
    }
}

/// A bound as a sentence says it.
fn spoken(bound: Duration) -> String {
    match bound.as_secs() {
        0 => format!("{} ms", bound.as_millis()),
        secs if secs < 120 => format!("{secs} s"),
        secs => format!("{} min", secs / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, data: Value) -> Event {
        Event {
            seq: 1,
            at: 0,
            name: name.into(),
            data,
        }
    }

    #[test]
    fn a_request_that_waits_past_the_limit_is_said_once_then_its_answer() {
        let asking = Arc::new(AtomicU64::new(0));
        let (updates, mut said) = unbounded();
        let watched = asking.clone();
        let updates = Updates::from(updates);
        let watcher =
            std::thread::spawn(move || watch(&watched, &updates, Duration::from_millis(100)));
        let next = |said: &mut UnboundedReceiver<Update>| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(update) = said.try_recv() {
                    return update;
                }
                assert!(Instant::now() < deadline, "nothing said");
                std::thread::sleep(Duration::from_millis(5));
            }
        };

        let since = now_ms() - 1000;
        asking.store(since, Ordering::SeqCst);
        assert_eq!(next(&mut said), Update::NotAnswering(Some(since)));
        std::thread::sleep(Duration::from_millis(100));
        assert!(said.try_recv().is_err(), "said once");
        asking.store(0, Ordering::SeqCst);
        assert_eq!(next(&mut said), Update::NotAnswering(None));

        drop(said);
        watcher.join().unwrap();
    }

    #[test]
    fn a_conversation_piece_does_not_read_the_shell_again() {
        assert!(!matters(&event("agent.message", json!({"text": "hi"}))));
        assert!(!matters(&event("agent.tool_call", json!({}))));
        assert!(!matters(&event(
            "delivery.queued",
            json!({"kind": "brief"})
        )));
        assert!(!matters(&event("delivery.queued", json!({"kind": "user"}))));
        assert!(matters(&event("approval.requested", json!({}))));
        assert!(matters(&event("agent.turn_ended", json!({}))));
        for name in [
            "agent.compacting",
            "agent.compacted",
            "agent.compaction_failed",
        ] {
            assert!(
                matters(&event(name, json!({}))),
                "{name}: the session's state"
            );
        }
        assert!(matters(&event("run.started", json!({}))), "the tree's icon");
        assert!(matters(&event("run.ended", json!({}))));
        assert!(
            matters(&event("subscription.ended", json!({}))),
            "a session at rest or waiting"
        );
        assert!(matters(&event("workspace.created", json!({}))));
        assert!(matters(&event("agent.moved", json!({}))), "the rail");
        assert!(
            matters(&event("agent.blocked", json!({}))),
            "the row's place and mark"
        );
        assert!(
            matters(&event("workspace.step_done", json!({}))),
            "a card shows the preparation"
        );
    }

    #[test]
    fn a_provider_changed_or_a_session_opened_reads_the_providers_again() {
        assert!(about_providers(&event("provider.changed", json!({}))));
        assert!(about_providers(&event("provider.models_read", json!({}))));
        assert!(about_providers(&event("agent.started", json!({}))));
        assert!(
            about_providers(&event("agent.options_taken", json!({}))),
            "a model set says its efforts"
        );
        assert!(!about_providers(&event("agent.turn_ended", json!({}))));
        assert!(!about_providers(&event("agent.message", json!({}))));
    }

    #[test]
    fn a_read_asked_while_one_is_in_flight_waits_for_it_and_many_make_one() {
        let reads = ProviderReads::default();
        assert!(reads.ask());
        assert!(!reads.ask());
        assert!(!reads.ask());
        assert!(reads.ended(), "one more read is due");
        assert!(!reads.ended(), "and only one");
        assert!(reads.ask(), "none is in flight any more");
    }

    #[test]
    fn a_read_thread_that_cannot_start_is_skipped_and_the_next_event_asks_again() {
        let reads = ProviderReads::default();
        reads.begin(|| Err(std::io::Error::other("no thread")));

        let mut started = 0;
        reads.begin(|| {
            started += 1;
            Ok(())
        });

        assert_eq!(started, 1);
    }

    #[test]
    fn an_event_that_does_not_follow_the_last_is_a_gap() {
        let seen = Seen::default();
        for seq in [4, 5, 7] {
            note(
                &seen,
                &Event {
                    seq,
                    ..event("x.y", json!({}))
                },
            );
        }
        assert_eq!(seen.gaps.load(Ordering::Relaxed), 1);
        assert_eq!(seen.last_seq.load(Ordering::Relaxed), 7);
    }
}
