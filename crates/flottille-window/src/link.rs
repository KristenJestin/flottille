//! The window's side of the socket, on its own thread: it connects, subscribes to the events,
//! reads what the shell shows, and reads it again when an event says it changed. On a broken
//! connection it connects again (with a backoff), resumes the events after the last one it saw and
//! reads everything again, which the events alone cannot rebuild. The UI only ever receives parsed
//! [`Update`]s; it never waits on the socket.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flottille_core::protocol::{Event, Message, Request};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde_json::{Value, json};

use crate::actions::Outcome;
use crate::model::{self, Answers, Snapshot};

type Result<T> = std::result::Result<T, String>;

/// Opens a connection to the daemon, starting it when nobody listens: the command line's
/// `Client::connect_or_start` in the binary, something else in a test.
pub type Connect = Arc<dyn Fn() -> std::io::Result<UnixStream> + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// A fresh read of everything the shell shows.
    Snapshot(Snapshot),
    /// The connection broke, with why; the link is connecting again.
    Disconnected(String),
    /// What came of something the window asked (`actions`).
    Done(Outcome),
    /// A fresh read of the providers (`provider.list`), with the read's number
    /// (`providers::next_read`).
    Providers(u64, crate::providers::Providers),
    /// What the open session page's link read or saw, with the link's token.
    Session {
        token: u64,
        update: crate::session_link::SessionUpdate,
    },
}

/// What the link has seen, readable from any thread: the number of the last event, and how many
/// events came that did not follow the one before (a gap in the stream).
#[derive(Debug, Default)]
pub struct Seen {
    pub last_seq: AtomicU64,
    pub events: AtomicU64,
    pub gaps: AtomicU64,
    pub connections: AtomicU64,
}

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
pub fn spawn_into(connect: Connect, sender: UnboundedSender<Update>) -> Arc<Seen> {
    let seen = Arc::new(Seen::default());
    let shared = seen.clone();
    std::thread::Builder::new()
        .name("flottille-link".into())
        .spawn(move || run(connect, sender, shared))
        .expect("a thread for the daemon's socket");
    seen
}

fn run(connect: Connect, updates: UnboundedSender<Update>, seen: Arc<Seen>) {
    let mut retry = FIRST_RETRY;
    let mut briefs = HashMap::new();
    let reads = ProviderReads::default();
    loop {
        let why = match connect() {
            Ok(stream) => {
                seen.connections.fetch_add(1, Ordering::Relaxed);
                retry = FIRST_RETRY;
                match follow(stream, &connect, &updates, &seen, &mut briefs, &reads) {
                    Ok(()) => return,
                    Err(why) => why,
                }
            }
            Err(error) => format!("cannot reach the daemon: {error}"),
        };
        if updates.unbounded_send(Update::Disconnected(why)).is_err() {
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
    updates: &UnboundedSender<Update>,
    seen: &Seen,
    briefs: &mut HashMap<String, String>,
    reads: &ProviderReads,
) -> Result<()> {
    let mut conn = Conn::new(stream).map_err(|e| e.to_string())?;
    let last = seen.last_seq.load(Ordering::Relaxed);
    let after = (seen.connections.load(Ordering::Relaxed) > 1 || last > 0).then_some(last);
    let started = conn.request("events.watch", json!({"after": after}))?;
    if after.is_none() {
        seen.last_seq
            .store(started["after"].as_u64().unwrap_or(0), Ordering::Relaxed);
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
    };
    loop {
        // Everything that came already, then a short quiet (or a short while at most), before
        // reading the shell again.
        let since = [stale, providers].into_iter().flatten().min();
        match conn.next_event(quiet_wait(since))? {
            Some(event) => mark(&event, &mut stale, &mut providers),
            None => {
                if stale.take().is_some() {
                    let snapshot = read(&mut conn, briefs, seen)?;
                    if updates.unbounded_send(Update::Snapshot(snapshot)).is_err() {
                        return Ok(());
                    }
                }
                if providers.take().is_some() {
                    reads.start(connect, updates);
                }
                // Events that came while it read are queued: they make it stale again.
                while let Some(event) = conn.queued.pop() {
                    mark(&event, &mut stale, &mut providers);
                }
            }
        }
    }
}

/// Whether an event may change what `provider.list` says: a provider enabled or disabled, or a
/// session opened, which says the models its provider offers.
fn about_providers(event: &Event) -> bool {
    event.name.starts_with("provider.") || event.name == "agent.started"
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
    fn start(&self, connect: &Connect, updates: &UnboundedSender<Update>) {
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

    fn run(&self, connect: &Connect, updates: &UnboundedSender<Update>) {
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
}

/// Whether an event changes what the shell shows. A conversation's pieces and tool calls do not:
/// they are many, and the session page follows them itself.
fn matters(event: &Event) -> bool {
    let name = event.name.as_str();
    match name.split_once('.').map(|(subject, _)| subject) {
        // A workspace's preparation shows on its card, step by step.
        Some("project" | "approval" | "daemon" | "workspace") => true,
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
        ),
        _ => false,
    }
}

/// A fresh read of everything the shell shows, on any connection.
pub fn read_once(stream: UnixStream) -> Result<Snapshot> {
    let mut conn = Conn::new(stream).map_err(|e| e.to_string())?;
    read(&mut conn, &mut HashMap::new(), &Seen::default())
}

fn read(conn: &mut Conn, briefs: &mut HashMap<String, String>, seen: &Seen) -> Result<Snapshot> {
    let projects = conn.request("project.list", json!({}))?;
    let workspaces = conn.request("workspace.list", json!({}))?;
    let agents = conn.request("agent.list", json!({}))?;
    let approvals = conn.request("approval.list", json!({}))?;
    let commands = conn.request("command.list", json!({}))?;
    let needs_user = needing_the_user(conn)?;
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
        if !briefs.contains_key(id) {
            let shown = conn.request("agent.show", json!({"agent": id, "after": now}))?;
            let brief = shown["agent"]["brief"].as_str().unwrap_or_default();
            briefs.insert(id.to_owned(), brief.to_owned());
        }
    }
    let questions = questions(conn, &agents)?;
    Ok(Snapshot::build(Answers {
        projects: &projects,
        workspaces: &workspaces,
        agents: &agents,
        approvals: &approvals,
        needs_user: &needs_user,
        briefs,
        questions: &questions,
        commands: &commands,
    }))
}

/// The last question of each root asking the user, from the inbox: read only while one asks.
fn questions(conn: &mut Conn, agents: &Value) -> Result<HashMap<String, (String, u64)>> {
    let asking: HashSet<&str> = agents["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|agent| model::asks_the_user(agent))
        .filter_map(|agent| agent["id"].as_str())
        .collect();
    let mut found = HashMap::new();
    if asking.is_empty() {
        return Ok(found);
    }
    let inbox = conn.request("inbox.list", json!({}))?;
    for entry in inbox["entries"].as_array().into_iter().flatten() {
        let agent = entry["agent"].as_str().unwrap_or_default();
        if entry["kind"] == "question" && asking.contains(agent) {
            let text = entry["text"].as_str().unwrap_or_default().to_owned();
            found.insert(agent.to_owned(), (text, entry["at"].as_u64().unwrap_or(0)));
        }
    }
    Ok(found)
}

/// The roots the daemon says need the user, as `agent.trees` gives them (the live roots, a page
/// of them at a time): the window does not decide this itself.
fn needing_the_user(conn: &mut Conn) -> Result<HashSet<String>> {
    let mut found = HashSet::new();
    let mut before: Option<String> = None;
    loop {
        let page = conn.request("agent.trees", json!({"before": before}))?;
        let trees = page["trees"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        for tree in trees {
            if tree["status"]["needs_user"] == true {
                found.insert(
                    tree["status"]["agent"]["id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                );
            }
        }
        before = trees
            .last()
            .and_then(|tree| tree["status"]["agent"]["id"].as_str())
            .map(str::to_owned);
        if page["more"] != true || before.is_none() {
            return Ok(found);
        }
    }
}

/// One connection: requests answered in turn, events kept aside meanwhile.
pub(crate) struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
    /// The start of a line a timeout cut short, kept as bytes: a cut can fall inside a character.
    partial: Vec<u8>,
    /// Events that came while a request waited for its answer, the newest first.
    queued: Vec<Event>,
}

impl Conn {
    pub(crate) fn new(stream: UnixStream) -> std::io::Result<Conn> {
        stream.set_read_timeout(None)?;
        Ok(Conn {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            next_id: 1,
            partial: Vec::new(),
            queued: Vec::new(),
        })
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
        self.ask_until(method, params, None)
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
        self.ask_until(method, params, Some((Instant::now() + bound, bound)))
    }

    fn ask_until(
        &mut self,
        method: &str,
        params: Value,
        deadline: Option<(Instant, Duration)>,
    ) -> Result<std::result::Result<Value, String>> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let params = match params {
            Value::Object(mut map) => {
                map.retain(|_, value| !value.is_null());
                Value::Object(map)
            }
            other => other,
        };
        let request = Request {
            id: id.clone(),
            method: method.to_owned(),
            params,
        };
        let mut line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .map_err(|e| format!("the daemon's socket: {e}"))?;
        loop {
            let message = match deadline {
                None => self.read(None)?.ok_or("the daemon closed the connection")?,
                Some((at, bound)) => {
                    let left = at.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(format!(
                            "the daemon did not answer {method} within {}",
                            spoken(bound)
                        ));
                    }
                    match self.read(Some(left))? {
                        Some(message) => message,
                        None => continue,
                    }
                }
            };
            if let Some(event) = message.event {
                self.queued.insert(0, event);
                continue;
            }
            if message.id.as_deref() != Some(&id) {
                continue;
            }
            if let Some(error) = message.error {
                return Ok(Err(error.message));
            }
            return Ok(Ok(message.result.unwrap_or(Value::Null)));
        }
    }

    /// The next event, waiting at most `wait` (`None`: as long as it takes); `None` when none
    /// came in time.
    pub(crate) fn next_event(&mut self, wait: Option<Duration>) -> Result<Option<Event>> {
        if let Some(event) = self.queued.pop() {
            return Ok(Some(event));
        }
        let deadline = wait.map(|wait| Instant::now() + wait);
        loop {
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            if left == Some(Duration::ZERO) {
                return Ok(None);
            }
            match self.read(left)? {
                Some(Message {
                    event: Some(event), ..
                }) => return Ok(Some(event)),
                Some(_) => {}
                None if left.is_some() => return Ok(None),
                None => return Err("the daemon closed the connection".into()),
            }
        }
    }

    /// The next message; `None` at the end of the stream, or when `wait` ran out.
    fn read(&mut self, wait: Option<Duration>) -> Result<Option<Message>> {
        self.writer
            .set_read_timeout(wait)
            .map_err(|e| e.to_string())?;
        let read = self.reader.read_until(b'\n', &mut self.partial);
        let _ = self.writer.set_read_timeout(None);
        match read {
            Ok(0) if wait.is_some() => Err("the daemon closed the connection".into()),
            Ok(0) => Ok(None),
            Ok(_) if !self.partial.ends_with(b"\n") => Ok(None),
            Ok(_) => {
                let line = std::mem::take(&mut self.partial);
                serde_json::from_slice(&line)
                    .map(Some)
                    .map_err(|e| format!("the daemon sent a line that is not a message: {e}"))
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(format!("the daemon's socket: {e}")),
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
        assert!(matters(&event("run.started", json!({}))), "the tree's icon");
        assert!(matters(&event("run.ended", json!({}))));
        assert!(matters(&event("workspace.created", json!({}))));
        assert!(
            matters(&event("workspace.step_done", json!({}))),
            "a card shows the preparation"
        );
    }

    #[test]
    fn a_provider_changed_or_a_session_opened_reads_the_providers_again() {
        assert!(about_providers(&event("provider.changed", json!({}))));
        assert!(about_providers(&event("agent.started", json!({}))));
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
