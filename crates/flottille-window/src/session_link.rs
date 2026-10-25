//! The open session page's own way to the daemon, on its own thread and connection: it subscribes
//! to the events first, reads the session's tree, the requests of the tree that reached the user,
//! the last pages of the shown agent's stored conversation and the place's facts, then follows
//! the events: each event of the shown agent goes to the page as it comes, and what changes the
//! tree, the requests or the place is read again once the events pause (or a short while after
//! the first, when they do not). A broken connection resumes the events after the last one seen:
//! the conversation is read once per link. Nothing here waits on the UI, and the UI never waits
//! on it.
//!
//! Each link has a token: the page applies only what its current link sends, so that what a
//! closed page's link sent late is dropped.

use std::collections::HashSet;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flottille_core::protocol::Event;
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};

use crate::actions::Changes;
use crate::conversation::one_line;
use crate::link::{Conn, Connect, Update, quiet_wait};

/// One agent of the session's tree, as `agent.tree` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeAgent {
    pub id: String,
    pub parent: Option<String>,
    pub depth: u64,
    pub role: String,
    pub title: Option<String>,
    pub state: String,
    pub state_reason: Option<String>,
    pub model: Option<String>,
    pub doing: String,
    /// `parent`, `user` or nothing.
    pub waiting_on: Option<String>,
    pub needs_user: bool,
}

impl TreeAgent {
    fn of(entry: &Value) -> TreeAgent {
        let agent = &entry["agent"];
        let text = |value: &Value| value.as_str().map(str::to_owned);
        TreeAgent {
            id: text(&agent["id"]).unwrap_or_default(),
            parent: text(&agent["parent"]),
            depth: agent["depth"].as_u64().unwrap_or(0),
            role: text(&agent["role"]).unwrap_or_default(),
            title: text(&agent["title"]),
            state: text(&agent["state"]).unwrap_or_default(),
            state_reason: text(&agent["state_reason"]),
            model: text(&agent["model"]),
            doing: text(&entry["doing"]).unwrap_or_default(),
            waiting_on: text(&entry["waiting_on"]),
            needs_user: entry["needs_user"] == true,
        }
    }
}

/// A request the user holds, asked by an agent of the session.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub number: u64,
    /// The agent that asked.
    pub agent: String,
    pub tool: String,
    /// Its arguments, as the daemon cut and masked them.
    pub call: Value,
    pub reasons: Vec<String>,
    /// The agents that passed it up, each with its sentence.
    pub forwards: Vec<(String, String)>,
    /// It reaches a sensitive place: the daemon never allows it for the tree.
    pub sensitive: bool,
    /// When it was asked.
    pub created_at: u64,
    /// `deny`, `once`, `tree` once decided, `cancelled` when nobody answered it.
    pub decision: Option<String>,
}

impl Request {
    fn of(request: &Value) -> Request {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        Request {
            number: request["number"].as_u64().unwrap_or(0),
            agent: text(&request["agent"]),
            tool: text(&request["tool"]),
            call: request["call"].clone(),
            reasons: request["reasons"]
                .as_array()
                .into_iter()
                .flatten()
                .map(text)
                .collect(),
            forwards: request["forwards"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| (text(&f["by"]), text(&f["why"])))
                .collect(),
            sensitive: request["sensitive"] == true,
            created_at: request["created_at"].as_u64().unwrap_or(0),
            decision: match request["state"].as_str() {
                Some("cancelled") => Some("cancelled".into()),
                _ => request["decision"].as_str().map(str::to_owned),
            },
        }
    }

    /// What is asked, in a few words: `Run npm test`, `Read .env`.
    pub fn asked(&self) -> String {
        let field = |name: &str| self.call[name].as_str().map(str::to_owned);
        match (self.tool.as_str(), field("line"), field("path")) {
            ("commands_run", Some(line), _) => format!("Run {line}"),
            ("commands_run", None, _) => match field("name") {
                Some(name) => format!("Run {name}"),
                None => format!("commands_run {}", one_line(&self.call)),
            },
            ("fs_read" | "fs_list" | "search", _, Some(path)) => format!("Read {path}"),
            ("fs_write" | "fs_edit", _, Some(path)) => format!("Write {path}"),
            (tool, _, _) => format!("{tool} {}", one_line(&self.call)),
        }
    }
}

/// What a place shows in the rail: each repository and its lines changed, and its facts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaceFacts {
    /// Each repository's path, and its lines changed (a workspace), or the branch its checkout
    /// is on (the project folder).
    pub repositories: Vec<(String, RepositoryFacts)>,
    pub branch: Option<String>,
    pub bases: Vec<String>,
    /// How long its preparation took, once it is ready.
    pub prepared_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RepositoryFacts {
    Changes(Option<Changes>),
    Branch(Option<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionUpdate {
    Tree(Vec<TreeAgent>),
    /// The last pages of the shown agent's stored conversation, oldest first, once per link;
    /// `earlier` when older events were left out.
    History {
        events: Vec<Event>,
        earlier: bool,
    },
    /// An event of the shown agent, as it comes.
    Live(Event),
    /// The requests an agent of the session asked that reached the user, pending or decided,
    /// oldest first.
    Requests(Vec<Request>),
    /// A request of the session was decided (`deny`, `once`, `tree`) or cancelled (`cancelled`).
    Decided {
        number: u64,
        decision: String,
    },
    /// The place's facts; `None` when the daemon has none (the workspace was removed).
    Place(Option<PlaceFacts>),
}

/// Where the session works.
#[derive(Clone, Debug, PartialEq)]
pub struct Where {
    pub project: String,
    pub workspace: Option<String>,
}

/// The link's handle: dropping it stops the link.
pub struct SessionLink {
    token: u64,
    stop: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<UnixStream>>>,
}

impl SessionLink {
    pub fn token(&self) -> u64 {
        self.token
    }
}

impl Drop for SessionLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the thread from its read, at once.
        if let Some(stream) = self.stream.lock().unwrap().take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

static TOKENS: AtomicU64 = AtomicU64::new(1);

/// The events after which the tree is read again: those that move a state, a launch, a request.
const TREE_EVENTS: &[&str] = &[
    "agent.created",
    "agent.started",
    "agent.prompted",
    "agent.turn_ended",
    "agent.released",
    "agent.died",
    "agent.replaced",
    "agent.failed",
    "agent.ended",
    "agent.reported",
    "agent.asked",
    "delivery.queued",
];

const FIRST_RETRY: Duration = Duration::from_millis(100);
const LAST_RETRY: Duration = Duration::from_secs(5);
/// A connection that lasted this long was healthy: the next one is tried at once again. One that
/// breaks sooner (a request that keeps failing) waits longer each time.
const HEALTHY: Duration = Duration::from_secs(10);

/// Starts the link of the session of `root`, showing the last `pages` pages of `shown`'s
/// conversation.
pub fn spawn(
    connect: Connect,
    updates: UnboundedSender<Update>,
    root: String,
    shown: String,
    place: Where,
    pages: usize,
) -> SessionLink {
    let token = TOKENS.fetch_add(1, Ordering::Relaxed);
    let stop = Arc::new(AtomicBool::new(false));
    let stream = Arc::new(Mutex::new(None));
    let link = Link {
        connect,
        updates,
        token,
        root,
        shown,
        place,
        pages: pages.max(1),
        stop: stop.clone(),
        stream: stream.clone(),
    };
    std::thread::Builder::new()
        .name("flottille-session".into())
        .spawn(move || link.run())
        .expect("a thread for the session page");
    SessionLink {
        token,
        stop,
        stream,
    }
}

struct Link {
    connect: Connect,
    updates: UnboundedSender<Update>,
    token: u64,
    root: String,
    shown: String,
    place: Where,
    pages: usize,
    stop: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<UnixStream>>>,
}

/// What outlives a connection: the last event seen, to resume after it, and whether the
/// conversation was read.
#[derive(Default)]
struct Resume {
    last: Option<u64>,
    read: bool,
}

#[derive(Default)]
struct Stale {
    tree: bool,
    requests: bool,
    place: bool,
    /// When the first of them was marked.
    since: Option<Instant>,
}

impl Stale {
    fn mark(&mut self, tree: bool, requests: bool, place: bool) {
        self.tree |= tree;
        self.requests |= requests;
        self.place |= place;
        if (tree || requests || place) && self.since.is_none() {
            self.since = Some(Instant::now());
        }
    }
}

impl Link {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn send(&self, update: SessionUpdate) -> bool {
        !self.stopped()
            && self
                .updates
                .unbounded_send(Update::Session {
                    token: self.token,
                    update,
                })
                .is_ok()
    }

    fn run(self) {
        let mut retry = FIRST_RETRY;
        let mut resume = Resume::default();
        while !self.stopped() {
            let began = Instant::now();
            if let Ok(stream) = (self.connect)()
                && let Ok(clone) = stream.try_clone()
            {
                *self.stream.lock().unwrap() = Some(clone);
                if self.stopped() {
                    return;
                }
                if self.follow(stream, &mut resume).is_ok() {
                    return;
                }
            }
            if began.elapsed() >= HEALTHY {
                retry = FIRST_RETRY;
            }
            std::thread::sleep(retry);
            retry = (retry * 2).min(LAST_RETRY);
        }
    }

    /// Follows one connection until it breaks (an error) or the link is stopped (`Ok`).
    fn follow(&self, stream: UnixStream, resume: &mut Resume) -> Result<(), String> {
        let mut conn = Conn::new(stream).map_err(|e| e.to_string())?;
        let started = conn.request("events.watch", json!({"after": resume.last}))?;
        let after = *resume
            .last
            .get_or_insert(started["after"].as_u64().unwrap_or(0));
        let mut tree = self.read_tree(&mut conn)?;
        self.read_requests(&mut conn)?;
        if !resume.read {
            let (events, earlier) = self.read_history(&mut conn, after)?;
            if !self.send(SessionUpdate::History { events, earlier }) {
                return Ok(());
            }
            resume.read = true;
        }
        self.read_place(&mut conn)?;
        let mut stale = Stale::default();
        loop {
            if self.stopped() {
                return Ok(());
            }
            let Some(event) = conn.next_event(quiet_wait(stale.since))? else {
                if stale.tree {
                    tree = self.read_tree(&mut conn)?;
                }
                if stale.requests {
                    self.read_requests(&mut conn)?;
                }
                if stale.place {
                    self.read_place(&mut conn)?;
                }
                stale = Stale::default();
                continue;
            };
            resume.last = Some(event.seq);
            let data = &event.data;
            let agent = data["agent"].as_str().unwrap_or_default();
            let name = event.name.as_str();
            if name.starts_with("workspace.")
                && self.place.workspace.as_deref() == data["workspace"].as_str()
                && data["project"] == self.place.project.as_str()
            {
                stale.mark(false, false, true);
            }
            let ours = tree.contains(agent)
                || (name == "agent.created"
                    && data["parent"].as_str().is_some_and(|p| tree.contains(p)));
            if !ours {
                continue;
            }
            if name == "agent.created" {
                tree.insert(agent.to_owned());
            }
            let approval = name.starts_with("approval.");
            stale.mark(
                TREE_EVENTS.contains(&name) || approval,
                approval,
                name == "agent.turn_ended",
            );
            let decided = match name {
                "approval.decided" => data["decision"].as_str(),
                "approval.cancelled" => Some("cancelled"),
                _ => None,
            };
            if let Some(decision) = decided {
                let number = data["number"].as_u64().unwrap_or(0);
                let decision = decision.to_owned();
                if !self.send(SessionUpdate::Decided { number, decision }) {
                    return Ok(());
                }
            }
            if agent == self.shown && !self.send(SessionUpdate::Live(event.clone())) {
                return Ok(());
            }
        }
    }

    /// The last pages of the shown agent's events up to `after`, oldest first, as the daemon
    /// pages them; whether older ones remain.
    fn read_history(&self, conn: &mut Conn, after: u64) -> Result<(Vec<Event>, bool), String> {
        let mut events: Vec<Event> = Vec::new();
        let mut before = after.saturating_add(1);
        let mut earlier = false;
        for _ in 0..self.pages {
            let shown =
                conn.request("agent.show", json!({"agent": self.shown, "before": before}))?;
            let page: Vec<Event> = shown["events"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| serde_json::from_value(e.clone()).ok())
                .collect();
            earlier = shown["more"] == true;
            before = page.first().map_or(before, |e| e.seq);
            events.splice(0..0, page);
            if !earlier {
                break;
            }
        }
        Ok((events, earlier))
    }

    /// The tree, sent; the ids of its agents.
    fn read_tree(&self, conn: &mut Conn) -> Result<HashSet<String>, String> {
        let answer = conn.request("agent.tree", json!({"agent": self.root}))?;
        let agents: Vec<TreeAgent> = answer["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .map(TreeAgent::of)
            .collect();
        let ids = agents.iter().map(|a| a.id.clone()).collect();
        self.send(SessionUpdate::Tree(agents));
        Ok(ids)
    }

    fn read_requests(&self, conn: &mut Conn) -> Result<(), String> {
        let answer = conn.request("approval.list", json!({"tree": self.root}))?;
        let requests = answer["requests"]
            .as_array()
            .into_iter()
            .flatten()
            .map(Request::of)
            .collect();
        self.send(SessionUpdate::Requests(requests));
        Ok(())
    }

    fn read_place(&self, conn: &mut Conn) -> Result<(), String> {
        let facts = match &self.place.workspace {
            Some(name) => {
                let shown = conn.ask(
                    "workspace.show",
                    json!({"project": self.place.project, "name": name}),
                )?;
                // A workspace removed under the session: it has no facts, the link goes on.
                let Ok(shown) = shown else {
                    self.send(SessionUpdate::Place(None));
                    return Ok(());
                };
                let mut bases: Vec<String> = Vec::new();
                let mut repositories = Vec::new();
                for repository in shown["repositories"].as_array().into_iter().flatten() {
                    let base = repository["base"]["branch"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    if !bases.contains(&base) {
                        bases.push(base);
                    }
                    let changes = &repository["changes"];
                    let changes = (!changes.is_null()).then(|| Changes {
                        added: changes["added"].as_u64().unwrap_or(0),
                        removed: changes["removed"].as_u64().unwrap_or(0),
                        capped: changes["capped"] == true,
                    });
                    repositories.push((
                        repository["path"].as_str().unwrap_or_default().to_owned(),
                        RepositoryFacts::Changes(changes),
                    ));
                }
                PlaceFacts {
                    repositories,
                    branch: shown["branch"].as_str().map(str::to_owned),
                    bases,
                    prepared_ms: shown["ready_at"].as_u64().map(|ready| {
                        ready.saturating_sub(shown["created_at"].as_u64().unwrap_or(0))
                    }),
                }
            }
            None => {
                let Ok(shown) = conn.ask("project.show", json!({"name": self.place.project}))?
                else {
                    self.send(SessionUpdate::Place(None));
                    return Ok(());
                };
                PlaceFacts {
                    repositories: shown["repositories"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|r| {
                            (
                                r["path"].as_str().unwrap_or_default().to_owned(),
                                RepositoryFacts::Branch(r["branch"].as_str().map(str::to_owned)),
                            )
                        })
                        .collect(),
                    ..PlaceFacts::default()
                }
            }
        };
        self.send(SessionUpdate::Place(Some(facts)));
        Ok(())
    }
}
