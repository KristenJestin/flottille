//! The local ghost of each session of another machine (`docs/remote.md`, "Machines are one",
//! point 4): its conversation, its tree and its waits, kept in this machine's store from that
//! machine's events, so that opening it waits for no link and works while that machine is off.
//!
//! Once a link to a runner is made, this machine asks it for its sessions' ghosts (`ghost`), each
//! from after the last event it holds of it; the runner answers on the same stream what each one
//! missed (only that: its last events when it missed more than a ghost keeps), then each event of
//! its listed root sessions as it is stored, their tree and waits again when they move, and the end
//! of a session that leaves its list. Every part is what that machine's own handlers show
//! (`agent.show`'s events, `agent.tree`, `approval.list`, `vault.asks`), its secrets masked there
//! as they are for its own window: a ghost never holds more than that machine would show.
//!
//! The ghost only reflects: no method writes it but what that machine sends, and what the user
//! does to the session goes to that machine, through the same code as for a session there.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use flottille_link::{
    GHOST_EVENTS, GhostAsk, GhostNews, Link, LinkError, MOST_SESSIONS, MachineMessage, Message,
    PublicKey, Received, Role, Streamed,
};
use flottille_protocol::Event;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::daemon::Daemon;
use crate::link::Remote;
use crate::projects::Failure;

/// How long a session's tree and waits wait for its events to pause before they are sent again,
/// and how long at most from the first event that moved them.
const VIEW_QUIET: Duration = Duration::from_millis(150);
const VIEW_LATEST: Duration = Duration::from_secs(1);
/// The most bytes of events a frame holds: under the link's frame limit, with room to spare.
const FRAME_BYTES: usize = flottille_link::FRAME_LIMIT / 2;
/// The events of a session's stream of words: they move nothing of its tree or its waits.
const STREAMED: [&str; 2] = ["agent.message", "agent.thought"];

// The session's machine.

/// Serves the ghosts `ask` asks for, of the machine `name`, for as long as the stream lasts.
pub(crate) async fn serve(remote: Arc<Remote>, name: String, received: Received, ask: GhostAsk) {
    if let Err(why) = ask.check() {
        received.answer(Err(why));
        return;
    }
    let Some(daemon) = remote.daemon() else {
        received.answer(Err("the daemon is stopping".to_owned()));
        return;
    };
    // Subscribed before anything is read: what is stored from now comes live, what was before
    // comes in the catch-up, and an event in both is sent once.
    let Some(live) = daemon.live() else {
        received.answer(Err("the daemon is stopping".to_owned()));
        return;
    };
    let mut stream = received.stream();
    if !stream.send(b"{}").await {
        return;
    }
    let lists = remote.listed_sessions();
    let mut serving = Serving {
        daemon,
        name,
        known: ask.after,
        sent: HashMap::new(),
        roots: HashMap::new(),
        stale: HashSet::new(),
    };
    serving.run(&mut stream, live, lists).await;
    stream.end().await;
}

/// One machine's stream of ghosts.
struct Serving {
    daemon: Arc<Daemon>,
    /// The machine it is for, as the log names it.
    name: String,
    /// Where that machine's ghosts stopped when it asked: a session that joins the list later
    /// catches up from there.
    known: BTreeMap<String, u64>,
    /// Each listed session, and the last of its events sent.
    sent: HashMap<String, u64>,
    /// The root of each agent met, by its id.
    roots: HashMap<String, String>,
    /// The sessions whose tree or waits moved since they were last sent.
    stale: HashSet<String>,
}

impl Serving {
    async fn run(
        &mut self,
        stream: &mut Streamed,
        mut live: tokio::sync::broadcast::Receiver<Event>,
        mut lists: tokio::sync::watch::Receiver<Option<flottille_link::SessionList>>,
    ) {
        let gone = stream.gone();
        tokio::pin!(gone);
        // A list not made yet says nothing of what left it: waited for. None comes while this
        // machine is no runner, which lists no session.
        while lists.borrow_and_update().is_none() {
            tokio::select! {
                _ = &mut gone => return,
                changed = lists.changed() => if changed.is_err() { return },
            }
        }
        let listed = ids_of(&lists.borrow_and_update());
        // A ghost of a session that left the list while that machine was away goes now.
        let left: Vec<String> = self
            .known
            .keys()
            .filter(|id| !listed.contains(*id))
            .cloned()
            .collect();
        for id in left {
            if !self.gone(stream, &id).await {
                return;
            }
        }
        if !self.list(stream, listed).await {
            return;
        }
        let mut stale_since: Option<tokio::time::Instant> = None;
        let mut quiet_from = tokio::time::Instant::now();
        loop {
            let due = stale_since.map(|since| (since + VIEW_LATEST).min(quiet_from + VIEW_QUIET));
            let wake = async {
                match due {
                    Some(due) => tokio::time::sleep_until(due).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = &mut gone => return,
                changed = lists.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    // No list any more (no runner now) is no session gone.
                    if lists.borrow_and_update().is_none() {
                        continue;
                    }
                    let listed = ids_of(&lists.borrow_and_update());
                    if !self.list(stream, listed).await {
                        return;
                    }
                }
                event = live.recv() => {
                    // The sessions whose conversation moved: what each has after its last sent
                    // is read as `agent.show` reads it.
                    let mut touched: HashSet<String> = HashSet::new();
                    let mut next = Some(event);
                    let mut taken = 0;
                    while let Some(event) = next.take() {
                        match event {
                            Ok(event) => self.take(event, &mut touched),
                            // Too many at once: each listed session reads what it missed.
                            Err(RecvError::Lagged(_)) => {
                                touched.extend(self.sent.keys().cloned());
                                self.stale.extend(self.sent.keys().cloned());
                            }
                            Err(RecvError::Closed) => return,
                        }
                        taken += 1;
                        // What came meanwhile goes in the same frames.
                        if taken < 256 {
                            next = match live.try_recv() {
                                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                                    Some(Err(RecvError::Lagged(n)))
                                }
                                other => other.ok().map(Ok),
                            };
                        }
                    }
                    for id in touched {
                        let Some(&from) = self.sent.get(&id) else {
                            continue;
                        };
                        let (events, reset, earlier) = self.missed(&id, from);
                        if let Some(last) = events.last() {
                            self.sent.insert(id.clone(), last.seq);
                        }
                        if !self.events(stream, &id, events, reset, earlier).await {
                            return;
                        }
                    }
                    if !self.stale.is_empty() {
                        quiet_from = tokio::time::Instant::now();
                        stale_since.get_or_insert(quiet_from);
                    }
                }
                () = wake => {
                    stale_since = None;
                    let stale: Vec<String> = self.stale.drain().collect();
                    for id in stale {
                        if self.sent.contains_key(&id) && !self.view(stream, &id).await {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// An event stored: its session's conversation is read again when it is one of a listed
    /// session's; its tree marked to be sent again when it moves it.
    fn take(&mut self, event: Event, touched: &mut HashSet<String>) {
        let Some(agent) = event.data["agent"].as_str().map(str::to_owned) else {
            return;
        };
        if self.sent.get(&agent).is_some_and(|last| event.seq > *last) {
            touched.insert(agent.clone());
        }
        if STREAMED.contains(&event.name.as_str()) {
            return;
        }
        let root = match self.roots.get(&agent) {
            Some(root) => root.clone(),
            None => {
                let root = crate::tree::root_of(&self.daemon, &agent);
                self.roots.insert(agent, root.clone());
                root
            }
        };
        if self.sent.contains_key(&root) {
            self.stale.insert(root);
        }
    }

    /// The list is `listed` now: each session that left it goes, each that joined it catches up.
    async fn list(&mut self, stream: &mut Streamed, listed: Vec<String>) -> bool {
        let left: Vec<String> = self
            .sent
            .keys()
            .filter(|id| !listed.contains(*id))
            .cloned()
            .collect();
        for id in left {
            self.sent.remove(&id);
            if !self.gone(stream, &id).await {
                return false;
            }
        }
        for id in listed {
            if self.sent.contains_key(&id) {
                continue;
            }
            let from = self.known.get(&id).copied().unwrap_or(0);
            if !self.catch_up(stream, &id, from).await {
                return false;
            }
        }
        true
    }

    async fn gone(&mut self, stream: &mut Streamed, id: &str) -> bool {
        self.known.remove(id);
        send(
            stream,
            &GhostNews {
                session: id.to_owned(),
                gone: true,
                ..GhostNews::default()
            },
        )
        .await
    }

    /// What the ghost of `id` holding its events up to `from` missed: what came after, or its
    /// last events in place of what it holds (`reset`, and whether it has `earlier` ones) when it
    /// holds none or missed more than a ghost keeps.
    fn missed(&self, id: &str, from: u64) -> (Vec<Event>, bool, Option<bool>) {
        let missed = match from {
            0 => Vec::new(),
            from => self
                .daemon
                .read(|store| store.agent_events(id, from, GHOST_EVENTS + 1))
                .unwrap_or_default(),
        };
        if from != 0 && missed.len() <= GHOST_EVENTS {
            return (missed, false, None);
        }
        let mut last = self
            .daemon
            .read(|store| store.agent_events_before(id, u64::MAX, GHOST_EVENTS + 1))
            .unwrap_or_default();
        let earlier = last.len() > GHOST_EVENTS;
        if earlier {
            last.remove(0);
        }
        (last, true, Some(earlier))
    }

    /// What the ghost of `id` missed after its event `from`, then its tree and waits.
    async fn catch_up(&mut self, stream: &mut Streamed, id: &str, from: u64) -> bool {
        let (events, reset, earlier) = self.missed(id, from);
        eprintln!(
            "flottille daemon: the ghost of session {id} for {} caught up from event {from}: {} \
             events",
            self.name,
            events.len()
        );
        self.sent
            .insert(id.to_owned(), events.last().map_or(from, |event| event.seq));
        self.events(stream, id, events, reset, earlier).await && self.view(stream, id).await
    }

    /// `events` of `id`, in frames that fit the link: the first one says `reset` and `earlier`,
    /// sent even with no event.
    async fn events(
        &self,
        stream: &mut Streamed,
        id: &str,
        events: Vec<Event>,
        reset: bool,
        earlier: Option<bool>,
    ) -> bool {
        let mut frame = GhostNews {
            session: id.to_owned(),
            reset,
            earlier,
            ..GhostNews::default()
        };
        let mut head = reset || earlier.is_some();
        let mut bytes = 0;
        for event in events {
            let event = serde_json::to_value(&event).unwrap_or_default();
            let size = event.to_string().len();
            if size > FRAME_BYTES {
                eprintln!(
                    "flottille daemon: an event of session {id} is too large for its ghost on {}: \
                     it is left out of it",
                    self.name
                );
                continue;
            }
            if bytes + size > FRAME_BYTES && !frame.events.is_empty() {
                if !send(stream, &frame).await {
                    return false;
                }
                head = false;
                frame = GhostNews {
                    session: id.to_owned(),
                    ..GhostNews::default()
                };
                bytes = 0;
            }
            bytes += size;
            frame.events.push(event);
        }
        if frame.events.is_empty() && !head {
            return true;
        }
        send(stream, &frame).await
    }

    /// The tree and the waits of `id`, as this machine's own window reads them.
    async fn view(&self, stream: &mut Streamed, id: &str) -> bool {
        let Some(view) = view_of(&self.daemon, id).await else {
            return true;
        };
        send(
            stream,
            &GhostNews {
                session: id.to_owned(),
                view: Some(view),
                ..GhostNews::default()
            },
        )
        .await
    }
}

fn ids_of(list: &Option<flottille_link::SessionList>) -> Vec<String> {
    list.iter()
        .flat_map(|list| &list.sessions)
        .map(|session| session.id.clone())
        .collect()
}

async fn send(stream: &mut Streamed, news: &GhostNews) -> bool {
    let frame = serde_json::to_vec(news).expect("a ghost's news is JSON");
    stream.send(&frame).await
}

/// The tree of the session `id` and what waits in it, through the very handlers its own window
/// reads them by: `agent.tree`, `approval.list` and the vault asks of its agents.
async fn view_of(daemon: &Arc<Daemon>, id: &str) -> Option<Value> {
    let tree = crate::agents::call(daemon.clone(), "agent.tree", json!({"agent": id}))
        .await
        .ok()?;
    let requests = crate::approvals::call(daemon.clone(), "approval.list", json!({"tree": id}))
        .await
        .ok()?;
    let asks = crate::vault::call(daemon.clone(), "vault.asks", json!({}))
        .await
        .ok()?;
    let agents: HashSet<&str> = tree["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["agent"]["id"].as_str())
        .collect();
    let asks: Vec<&Value> = asks["asks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|ask| ask["agent"].as_str().is_some_and(|a| agents.contains(a)))
        .collect();
    Some(json!({"tree": tree["agents"], "requests": requests["requests"], "asks": asks}))
}

// The machine that shows it.

/// Asks the machine `name`, linked by `link` of `generation`, for the ghosts of its sessions, and
/// keeps them while that link is the machine's.
pub(crate) async fn follow(remote: Arc<Remote>, link: Link, name: String, generation: u64) {
    let key = link.peer();
    let Some(daemon) = remote.daemon() else {
        return;
    };
    let machine = crate::link::hex_of(&key);
    // Only a runner lists sessions: only a runner's have a ghost.
    let (asking, cursors_of) = (remote.clone(), daemon.clone());
    let at = machine.clone();
    let after = tokio::task::spawn_blocking(move || {
        let (members, _) = asking.listed().ok()?;
        let member = members.by_machine(&key)?;
        (member.role == Role::Runner).then_some(())?;
        cursors_of.read(|store| store.ghost_cursors(&at)).ok()
    })
    .await
    .ok()
    .flatten();
    let Some(after) = after else {
        return;
    };
    let ask = GhostAsk {
        after: after.into_iter().collect(),
    };
    let Ok(mut answers) = link
        .ask(&Message::Machine(MachineMessage::Ghost(ask)))
        .await
    else {
        return;
    };
    match answers.next().await {
        Some(Ok(_)) => {}
        Some(Err(LinkError::Declined { reason, .. }))
            if reason == flottille_link::UNKNOWN_MESSAGE =>
        {
            eprintln!(
                "flottille daemon: {name} runs an older Flottille, which keeps no ghost of its \
                 sessions here until it is updated (flottille update): they are read there"
            );
            return;
        }
        Some(Err(error)) => {
            eprintln!("flottille daemon: the ghosts of {name}'s sessions were not asked: {error}");
            return;
        }
        None => return,
    }
    while let Some(Ok(frame)) = answers.next().await {
        let news = match serde_json::from_value::<GhostNews>(frame) {
            Ok(news) => news,
            Err(error) => {
                eprintln!("flottille daemon: {name} sent what is no ghost's: {error}");
                return;
            }
        };
        if let Err(why) = news.check() {
            eprintln!("flottille daemon: a ghost {name} sent was not kept: {why}");
            continue;
        }
        let (keeping, daemon, machine) = (remote.clone(), daemon.clone(), machine.clone());
        let kept = tokio::task::spawn_blocking(move || {
            keep(&keeping, &daemon, &key, generation, &machine, &news)
        })
        .await
        .unwrap_or(false);
        if !kept {
            return;
        }
    }
}

/// Keeps `news` of the machine of `key`, while its link of `generation` is the one kept: whether
/// to go on.
fn keep(
    remote: &Remote,
    daemon: &Daemon,
    key: &PublicKey,
    generation: u64,
    machine: &str,
    news: &GhostNews,
) -> bool {
    if !remote.on_link(key, generation) {
        return false;
    }
    let part = flottille_core::store::GhostPart {
        session: &news.session,
        events: &news.events,
        reset: news.reset,
        earlier: news.earlier,
        view: news.view.as_ref(),
        gone: news.gone,
    };
    let kept = daemon.write(|store| {
        store
            .keep_ghost(machine, &part, now(), MOST_SESSIONS, GHOST_EVENTS)
            .map(|kept| (kept, None))
    });
    match kept {
        Ok(true) => {}
        Ok(false) => eprintln!(
            "flottille daemon: the ghost of session {} was not kept: this machine holds \
             {MOST_SESSIONS} of that machine's already",
            news.session
        ),
        Err(error) => eprintln!(
            "flottille daemon: the ghost of session {} could not be kept: {error}",
            news.session
        ),
    }
    true
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// `remote.ghost {machine, session}`: what this machine keeps of the session `session` of the
/// machine `machine` (by its name), read from its own store alone: `{events, more, view, last}`.
pub(crate) fn read(daemon: &Daemon, params: Value) -> Result<Value, Failure> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Asked {
        machine: String,
        session: String,
    }
    let asked: Asked = serde_json::from_value(params)?;
    let lists = daemon.read(|store| store.machine_sessions())?;
    let not_found = || {
        Failure::new(
            flottille_protocol::code::NOT_FOUND,
            format!(
                "this machine holds no ghost of the session {} of {}",
                asked.session, asked.machine
            ),
        )
    };
    let key = lists
        .into_iter()
        .find(|(_, name, ..)| *name == asked.machine)
        .map(|(key, ..)| key)
        .ok_or_else(not_found)?;
    let ghost = daemon
        .read(|store| store.ghost(&key, &asked.session))?
        .ok_or_else(not_found)?;
    let last = ghost.events.last().map(|event| event["seq"].clone());
    Ok(json!({
        "events": ghost.events,
        "more": ghost.earlier,
        "view": ghost.view,
        "last": last,
    }))
}
