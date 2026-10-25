//! The open session page's own way to the daemon, on its own thread and connection: it subscribes
//! to the events first, reads the session's tree, the shown agent's stored conversation a page at
//! a time, the requests of the tree that wait for the user and the place's facts, then follows
//! the events: each event of the shown agent goes to the page as it comes, and what changes the
//! tree, the requests or the place is read again once the events stop coming. Nothing here waits
//! on the UI, and the UI never waits on it.
//!
//! Each link has a token: the page applies only what its current link sends, so that what a
//! closed page's link sent late is dropped.

use std::collections::HashSet;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_core::protocol::Event;
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};

use crate::actions::Changes;
use crate::conversation::one_line;
use crate::link::{Conn, Connect, Update};

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
    /// A page of the shown agent's stored conversation, oldest first; `first` starts it again.
    History {
        events: Vec<Event>,
        first: bool,
    },
    /// An event of the shown agent, as it comes.
    Live(Event),
    /// The requests the user holds that an agent of the session asked, oldest first.
    Requests(Vec<Request>),
    /// A request of the session was decided (`deny`, `once`, `tree`) or cancelled (`cancelled`).
    Decided {
        number: u64,
        decision: String,
    },
    Place(PlaceFacts),
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

/// Events that come together are taken in one go before what they change is read again.
const QUIET: Duration = Duration::from_millis(40);
const FIRST_RETRY: Duration = Duration::from_millis(100);
const LAST_RETRY: Duration = Duration::from_secs(5);
/// The stored conversation is read this many events at a time (`agent.show`'s page).
const PAGE: usize = 1000;

/// Starts the link of the session of `root`, showing `shown`'s conversation.
pub fn spawn(
    connect: Connect,
    updates: UnboundedSender<Update>,
    root: String,
    shown: String,
    place: Where,
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
    stop: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<UnixStream>>>,
}

#[derive(Default)]
struct Stale {
    tree: bool,
    requests: bool,
    place: bool,
}

impl Stale {
    fn any(&self) -> bool {
        self.tree || self.requests || self.place
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
        while !self.stopped() {
            if let Ok(stream) = (self.connect)()
                && let Ok(clone) = stream.try_clone()
            {
                *self.stream.lock().unwrap() = Some(clone);
                if self.stopped() {
                    return;
                }
                retry = FIRST_RETRY;
                if self.follow(stream).is_ok() {
                    return;
                }
            }
            std::thread::sleep(retry);
            retry = (retry * 2).min(LAST_RETRY);
        }
    }

    /// Follows one connection until it breaks (an error) or the link is stopped (`Ok`).
    fn follow(&self, stream: UnixStream) -> Result<(), String> {
        let mut conn = Conn::new(stream).map_err(|e| e.to_string())?;
        conn.request("events.watch", json!({}))?;
        let mut tree = self.read_tree(&mut conn)?;
        let mut last = 0;
        let mut first = true;
        loop {
            let shown = conn.request("agent.show", json!({"agent": self.shown, "after": last}))?;
            let events: Vec<Event> = shown["events"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| serde_json::from_value(e.clone()).ok())
                .collect();
            let full = events.len() >= PAGE;
            last = events.last().map_or(last, |e| e.seq);
            if !self.send(SessionUpdate::History { events, first }) {
                return Ok(());
            }
            first = false;
            if !full {
                break;
            }
        }
        self.read_requests(&mut conn, &tree)?;
        self.read_place(&mut conn)?;
        let mut stale = Stale::default();
        loop {
            if self.stopped() {
                return Ok(());
            }
            let wait = stale.any().then_some(QUIET);
            let Some(event) = conn.next_event(wait)? else {
                if stale.tree {
                    tree = self.read_tree(&mut conn)?;
                }
                if stale.requests {
                    self.read_requests(&mut conn, &tree)?;
                }
                if stale.place {
                    self.read_place(&mut conn)?;
                }
                stale = Stale::default();
                continue;
            };
            let data = &event.data;
            let agent = data["agent"].as_str().unwrap_or_default();
            let name = event.name.as_str();
            let ours = tree.contains(agent)
                || (name == "agent.created"
                    && data["parent"].as_str().is_some_and(|p| tree.contains(p)));
            if !ours {
                continue;
            }
            if name == "agent.created" {
                tree.insert(agent.to_owned());
            }
            stale.tree |= TREE_EVENTS.contains(&name) || name.starts_with("approval.");
            stale.requests |= name.starts_with("approval.");
            stale.place |= name == "agent.turn_ended";
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
            if agent == self.shown
                && event.seq > last
                && !self.send(SessionUpdate::Live(event.clone()))
            {
                return Ok(());
            }
        }
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

    fn read_requests(&self, conn: &mut Conn, tree: &HashSet<String>) -> Result<(), String> {
        let answer = conn.request("approval.list", json!({}))?;
        let requests = answer["requests"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["agent"].as_str().is_some_and(|a| tree.contains(a)))
            .map(Request::of)
            .collect();
        self.send(SessionUpdate::Requests(requests));
        Ok(())
    }

    fn read_place(&self, conn: &mut Conn) -> Result<(), String> {
        let facts = match &self.place.workspace {
            Some(name) => {
                let shown = conn.request(
                    "workspace.show",
                    json!({"project": self.place.project, "name": name}),
                )?;
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
                let shown = conn.request("project.show", json!({"name": self.place.project}))?;
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
        self.send(SessionUpdate::Place(facts));
        Ok(())
    }
}
