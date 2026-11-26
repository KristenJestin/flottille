//! The daemon's link to the user's other machines (`docs/remote.md`, "Architecture", "The
//! transport", "The version"), with remote on: one endpoint, whose links go through the user's hub
//! over the session this machine keeps to it (`hub_session.rs`); a link kept to every machine this
//! one trusts that the hub says is online. Nothing listens and no address is dialled: the direct
//! link was removed on 2026-11-21.
//!
//! No NAT traversal, no discovery: with the hub unreachable, machines do not link, and say so with
//! the hub's own sentence.
//!
//! The machines it trusts are in `remote/peers.json`, which the control session to the hub writes
//! from the member list once the machine is set up (`hub_session.rs`). It is in the key folder,
//! which no agent reaches (`gate::keys`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flottille_core::paths::Paths;
use flottille_link::{
    Capabilities, ConnectionShare, Endpoint, Hello, Link, LinkError, MachineKey, MachineMessage,
    Message, Offered, PeerState, PublicKey, Role, Share, Traffic,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::VERSION;
use crate::daemon::Daemon;
use crate::projects::Failure;

/// The machines this one trusts.
const PEERS_FILE: &str = "peers.json";
/// The addresses of this machine declared by hand for the direct link, removed on 2026-11-21: a
/// key folder of before holds it, which the link's start takes away.
const ADDRESSES_FILE: &str = "addresses.json";
/// How often the daemon looks for a machine to dial.
const ROUND: Duration = Duration::from_secs(1);
/// The longest a machine that cannot be reached waits before it is dialled again.
const LONGEST_WAIT: Duration = Duration::from_secs(5);
/// What a machine this one does not trust is told.
const NOT_TRUSTED: &str = "refused: this machine's key is not one of the user's";
/// What closes the link to a machine the hub no longer says online.
const GONE_FROM_HUB: &str = "it is no longer connected to the hub";
/// What closes the second link two machines that dialled each other at once made.
const LINKED_ALREADY: &str = "linked already";
/// How long a request whose link was let go for another waits for the link kept.
const WAIT_KEPT: Duration = Duration::from_secs(10);
/// How long a key refused is not said again in the log.
const SAID_AGAIN: Duration = Duration::from_secs(60);
/// The most keys refused said in the log within `SAID_AGAIN`: a key costs a stranger nothing, so
/// each of its dials may bring a new one. The others are counted, and the count said.
const MOST_SAID: usize = 16;
/// How long the links wait at start for the hub's list, before they go on with the one kept.
const FIRST_LIST: Duration = Duration::from_secs(10);

/// What `remote/peers.json` holds.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Peers {
    #[serde(default)]
    machines: Vec<Peer>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Peer {
    name: String,
    key: PublicKey,
}

/// The peers' file as last read whole: one caught half written, or broken by hand, leaves it as
/// it was, and is said once.
struct Kept {
    last: Peers,
    /// Whether `last` is a file that was read whole: false while the file never was, so that an
    /// empty list is not taken for a member list with nobody in it.
    whole: bool,
    said: bool,
    /// What the file was when it was last read, whole or not.
    read: Option<Stamp>,
}

/// The keys refused since `since`, as the log said them.
struct Refused {
    since: Instant,
    /// The keys said, each once: `MOST_SAID` at most.
    said: HashSet<[u8; 32]>,
    /// The links refused whose key was not said.
    others: usize,
}

impl Refused {
    fn new(since: Instant) -> Refused {
        Refused {
            since,
            said: HashSet::new(),
            others: 0,
        }
    }
}

/// What this daemon sees of another machine.
struct Seen {
    state: PeerState,
    /// Since when, in milliseconds since the Unix epoch.
    since: u64,
    reason: Option<String>,
    /// The link, its generation, and whether this machine dialled it.
    link: Option<(u64, Link, bool)>,
    /// The client link this machine dialled to carry its clients' requests to it.
    client: Option<Link>,
    /// What it offers, as it last said over its link: none once the link ended.
    offers: Option<Capabilities>,
    dialling: bool,
    next_try: Instant,
    wait: Duration,
}

impl Seen {
    fn new() -> Seen {
        Seen {
            state: PeerState::Offline,
            since: now(),
            reason: None,
            link: None,
            client: None,
            offers: None,
            dialling: false,
            next_try: Instant::now(),
            wait: ROUND,
        }
    }

    fn set(&mut self, state: PeerState, reason: Option<String>) {
        if self.state != state {
            self.since = now();
        }
        self.state = state;
        self.reason = reason;
    }
}

/// Emits an event of the daemon's (`docs/events.md`), from the link and the hub's session.
pub(crate) type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Why a link to a machine removed from the member list is closed.
const REMOVED: &str = "it was removed from the member list";

/// The daemon's side of the link.
pub struct Remote {
    endpoint: Endpoint,
    folder: PathBuf,
    hello: Hello,
    peers: Mutex<Kept>,
    seen: Mutex<HashMap<[u8; 32], Seen>>,
    links: std::sync::atomic::AtomicU64,
    /// The keys refused lately, as the log said them.
    refused: Mutex<Refused>,
    /// The closes of sessions of this machine served, so that one is served once.
    closes: Mutex<flottille_link::ClosesServed>,
    /// Held while the peers' file is read and written again: apart from `peers`, so that a link
    /// coming in is not judged after a write to the disk.
    writing: Mutex<()>,
    /// The control session to the hub.
    pub(crate) control: std::sync::OnceLock<Arc<crate::hub_session::Control>>,
    /// `None` once the hub's list was taken, or the hub could not give one at start; since when
    /// the daemon waits for one, at start and after the session to the hub was lost (a machine
    /// that wakes from a suspend). Meanwhile, `first_list` at most, no link is accepted or
    /// dialled, so that a machine off during a removal never links to the removed machine by the
    /// list it kept.
    heard: tokio::sync::watch::Sender<Option<tokio::time::Instant>>,
    /// `FIRST_LIST`; shorter in a test.
    first_list: Duration,
    /// The machines a link of which waits for the hub's list.
    waiting: Mutex<HashSet<[u8; 32]>>,
    /// The clocks as the session to the hub last saw them: a machine that woke from a suspend is
    /// told at once, before the session's next round sees it and ends.
    awake: Mutex<crate::hub_session::Awake>,
    emit: Emit,
    /// The requests of the user's clients this machine took, for every link: kept in memory, so
    /// that a restart refuses what was signed before it (`flottille_link::Requests`).
    requests: Mutex<flottille_link::Requests>,
    /// The daemon, which serves those requests as its own clients'; held weakly, the daemon
    /// holding the link.
    daemon: std::sync::OnceLock<std::sync::Weak<Daemon>>,
    /// What this machine offers the others while its member is a runner; none otherwise. Each
    /// link sends it when made and whenever it changes (`offer`).
    offered: tokio::sync::watch::Sender<Option<Capabilities>>,
    /// Asks `keep_offering` to look at what this machine offers now: a link was made.
    offer_now: tokio::sync::Notify,
    /// Wakes the requests that wait for the link kept to a machine (`Remote::send`).
    relinked: tokio::sync::Notify,
    /// This machine's live root sessions, light, while its member is a runner; none otherwise.
    /// Each link sends it when made and whenever it changes, as `offered`.
    sessions: tokio::sync::watch::Sender<Option<flottille_link::SessionList>>,
    /// This machine's projects, each by its repositories' remotes, while its member is a runner:
    /// sent as a message of its own, apart from the sessions (`docs/remote.md`, "The version").
    projects: tokio::sync::watch::Sender<Option<flottille_link::ProjectList>>,
    /// The pilots of another machine of this machine's led sessions, while its member is a runner:
    /// sent as a message of its own, as the projects are.
    pilots: tokio::sync::watch::Sender<Option<flottille_link::PilotList>>,
    /// Counts the member lists taken: a watch carried for a device looks again whether the
    /// device is still a member.
    pub(crate) lists: tokio::sync::watch::Sender<u64>,
    /// The bundles offered to another machine, while it reads them (`travel.rs`).
    pub(crate) transfers: crate::travel::Transfers,
    /// The machines this one is reading a bundle from or placing code of, one thing at a time
    /// each (`travel.rs`).
    pub(crate) receiving: crate::travel::Receiving,
    /// The calls of this machine's agents sent to another machine and not answered yet
    /// (`across::forward`).
    pub(crate) calling: crate::across::Calling,
    /// The launches sent to another machine whose session is not recorded here yet
    /// (`launches.rs`).
    pub(crate) placing: crate::launches::Placings,
    /// The tunnels this machine carries and serves (`tunnel.rs`).
    pub(crate) tunnels: crate::tunnel::Tunnels,
    /// Becomes true when this link stops, for its own tasks: the daemon's stop, or remote turned
    /// off, or the link made again somewhere else.
    halt: Arc<tokio::sync::watch::Sender<bool>>,
}

/// Whether `error` says a message was not served because its link was let go for another to the
/// same machine (`LINKED_ALREADY`): by this side, or by the other, which turned it away.
fn unserved_for_another(error: &LinkError) -> bool {
    match error {
        LinkError::NotSent {
            reason: Some(reason),
            ..
        } => reason == LINKED_ALREADY,
        LinkError::TurnedAway(_) => true,
        _ => false,
    }
}

/// Starts the link: the endpoint, the loop that accepts and the one that dials, every link
/// through the hub. A link that cannot start is said, and the daemon goes on without it.
pub fn start(
    paths: &Paths,
    key: MachineKey,
    stopping: tokio::sync::watch::Receiver<bool>,
    emit: Emit,
    daemon: std::sync::Weak<Daemon>,
) -> Result<Arc<Remote>, String> {
    let endpoint = Endpoint::new(&key).map_err(|error| error.to_string())?;
    if crate::remote::says_listen(paths) {
        eprintln!(
            "flottille daemon: [remote] listen in daemon.toml is no longer read: every link goes through \
             the hub, and nothing listens (the key may be removed)"
        );
    }
    let folder = crate::remote::folder(paths);
    migrate(&folder);
    let remote = Arc::new(Remote::new(endpoint, folder, emit.clone()));
    let _ = remote.daemon.set(daemon);
    // The daemon's stop is this link's, and so is the link's own.
    tokio::spawn({
        let halt = remote.halt.clone();
        async move {
            tokio::select! {
                _ = stopped(stopping) => { halt.send_replace(true); }
                _ = stopped(halt.subscribe()) => {}
            }
        }
    });
    let stopping = remote.halt.subscribe();
    // Read before the first link comes in, which is judged by the file as last read.
    remote.peers();
    let control = Arc::new(crate::hub_session::Control::new(
        key,
        crate::remote::folder(paths),
        emit,
    ));
    let _ = remote.control.set(control.clone());
    tokio::spawn(accept(remote.clone()));
    tokio::spawn(crate::hub_session::run(
        control,
        remote.clone(),
        stopping.clone(),
    ));
    tokio::spawn(keep_offering(remote.clone(), stopping.clone()));
    tokio::spawn(dial(remote.clone(), stopping));
    Ok(remote)
}

/// Takes the direct link's addresses, removed on 2026-11-21, out of a key folder of before: the
/// addresses declared by hand (`addresses.json`), and those each machine trusted was given or told
/// (`addresses` and `told` in `peers.json`). The machines trusted stay. A file that cannot be read
/// or written is said, and left as it is: nothing reads those addresses any more.
fn migrate(folder: &Path) {
    let declared = folder.join(ADDRESSES_FILE);
    match std::fs::remove_file(&declared) {
        Ok(()) => eprintln!(
            "flottille daemon: {} of the direct link is removed: every link goes through the hub",
            declared.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "flottille daemon: {} is not removed: {error}",
            declared.display()
        ),
    }
    let file = folder.join(PEERS_FILE);
    let Ok(Some(before)) = read_json::<Option<Value>>(&file) else {
        return;
    };
    let addressed = before["machines"].as_array().is_some_and(|machines| {
        machines
            .iter()
            .any(|machine| machine.get("addresses").is_some() || machine.get("told").is_some())
    });
    if !addressed {
        return;
    }
    let peers = serde_json::from_value::<Peers>(before)
        .map_err(|error| error.to_string())
        .and_then(|peers| write_json(&file, &peers).map_err(|error| error.to_string()));
    if let Err(error) = peers {
        eprintln!(
            "flottille daemon: the addresses in {} are not removed: {error}",
            file.display()
        );
    }
}

/// Until `stopping` says so; nothing held of it, so that a `select!` holding it stays `Send`.
async fn stopped(mut stopping: tokio::sync::watch::Receiver<bool>) {
    let _ = stopping.wait_for(|stop| *stop).await;
}

/// This build's hello; a test build may make it another's.
/// The wall clock; in a test build, as far ahead as the file a test names says, in seconds, to
/// play a suspend.
pub(crate) fn wall_now() -> SystemTime {
    let now = SystemTime::now();
    let ahead = cfg!(debug_assertions)
        .then(|| std::env::var_os("FLOTTILLE_TEST_WALL_AHEAD_FILE"))
        .flatten()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| text.trim().parse().ok())
        .map(Duration::from_secs);
    ahead.map_or(now, |ahead| now + ahead)
}

pub(crate) fn hello() -> Hello {
    cfg!(debug_assertions)
        .then(|| std::env::var("FLOTTILLE_TEST_LINK_HELLO").ok())
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| Hello::ours(VERSION))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

impl Remote {
    fn new(endpoint: Endpoint, folder: PathBuf, emit: Emit) -> Remote {
        Remote {
            endpoint,
            folder,
            hello: hello(),
            peers: Mutex::new(Kept {
                last: Peers::default(),
                whole: false,
                said: false,
                read: None,
            }),
            seen: Mutex::new(HashMap::new()),
            links: Default::default(),
            refused: Mutex::new(Refused::new(Instant::now())),
            closes: Mutex::new(flottille_link::ClosesServed::default()),
            writing: Mutex::new(()),
            control: std::sync::OnceLock::new(),
            heard: tokio::sync::watch::Sender::new(Some(tokio::time::Instant::now())),
            first_list: FIRST_LIST,
            waiting: Mutex::new(HashSet::new()),
            awake: Mutex::new(crate::hub_session::Awake::now()),
            emit,
            requests: Mutex::new(flottille_link::Requests::new(SystemTime::now())),
            daemon: std::sync::OnceLock::new(),
            offered: tokio::sync::watch::Sender::new(None),
            offer_now: tokio::sync::Notify::new(),
            relinked: tokio::sync::Notify::new(),
            sessions: tokio::sync::watch::Sender::new(None),
            projects: tokio::sync::watch::Sender::new(None),
            pilots: tokio::sync::watch::Sender::new(None),
            lists: tokio::sync::watch::Sender::new(0),
            tunnels: crate::tunnel::Tunnels::default(),
            halt: Arc::new(tokio::sync::watch::Sender::new(false)),
            transfers: Default::default(),
            receiving: Default::default(),
            calling: Default::default(),
            placing: Default::default(),
        }
    }

    /// Carries the links through the hub over a session to it just opened, until it ends.
    pub(crate) fn through(&self, port: flottille_link::RelayPort) {
        self.endpoint.through(port);
    }

    /// Why the hub cannot be reached now, when this machine has one and is not connected to it:
    /// what the links that go through it say.
    fn hub_down(&self) -> Option<String> {
        let seen = self.control.get()?.seen();
        if seen["state"] == "connected" {
            return None;
        }
        seen["reason"].as_str().map(str::to_owned)
    }

    /// `error` in a sentence: a machine with no session to the hub says why the hub is out of
    /// reach.
    fn said(&self, error: &LinkError) -> String {
        match (error, self.hub_down()) {
            (LinkError::NoHub(name), Some(why)) => format!("{name} cannot be reached: {why}"),
            _ => error.to_string(),
        }
    }

    /// This machine's name among the user's, as the member list says it.
    pub(crate) fn name_as(&self, name: &str) {
        self.endpoint.name_as(name);
    }

    /// The machines the member list says this one trusts: `(name, key)`. Written over the peers'
    /// file; the link to a machine no longer there is cut at once, and it is let go at the next
    /// round.
    pub(crate) fn trust_machines(&self, machines: Vec<(String, PublicKey)>) {
        let trusted: HashSet<[u8; 32]> = machines.iter().map(|(_, key)| *key.as_bytes()).collect();
        let file = self.folder.join(PEERS_FILE);
        {
            let _writing = self.writing.lock().unwrap();
            let peers = Peers {
                machines: machines
                    .into_iter()
                    .map(|(name, key)| Peer { name, key })
                    .collect(),
            };
            // Kept before the file is written: a link judged meanwhile is judged by this list,
            // never by the one before it, which may still hold a machine removed now.
            {
                let mut kept = self.peers.lock().unwrap();
                kept.last = peers.clone();
                kept.whole = true;
            }
            // A file broken by hand is written over: the list is what says whom to trust.
            let before = read_json::<Peers>(&file).unwrap_or_default();
            let same = serde_json::to_value(&peers).ok() == serde_json::to_value(&before).ok();
            if !same && let Err(error) = write_json(&file, &peers) {
                eprintln!(
                    "flottille daemon: the machines the hub's list trusts were not kept in {}: \
                     {error}",
                    file.display()
                );
            }
            // Kept again with the file's stamp: a `peers()` that read the file before it was
            // written set the list before back, and the stamp would keep it from being read again.
            let mut kept = self.peers.lock().unwrap();
            kept.last = peers;
            kept.read = stamp(&file);
            kept.said = false;
        }
        self.lists
            .send_modify(|taken| *taken = taken.wrapping_add(1));
        // A machine removed from the list: its link is cut now, not when it next says something.
        for (key, seen) in self.seen.lock().unwrap().iter() {
            if !trusted.contains(key) {
                for link in seen
                    .link
                    .iter()
                    .map(|(_, link, _)| link)
                    .chain(seen.client.as_ref())
                {
                    link.close(REMOVED);
                }
            }
        }
    }

    /// The hub's list was taken, or the hub cannot give one now: the links go on.
    pub(crate) fn heard_hub(&self) {
        self.heard.send_replace(None);
    }

    /// The session to the hub was lost: the links wait for the next list again.
    pub(crate) fn lost_hub(&self) {
        self.heard.send_replace(Some(tokio::time::Instant::now()));
    }

    /// The clocks the session to the hub saw at its round.
    /// A sleep since the pair stored is judged before it is overwritten: the session's round or
    /// start would otherwise hide it from `holding`.
    pub(crate) fn awake_at(&self, awake: crate::hub_session::Awake) {
        let slept = {
            let mut stored = self.awake.lock().unwrap();
            let slept = stored.slept_until(&awake);
            *stored = awake;
            slept
        };
        if slept {
            self.lost_hub();
        }
    }

    /// Whether new links wait for the hub's list now: also when the machine slept since the
    /// session's last round, the list it kept having missed what changed meanwhile.
    fn holding(&self) -> bool {
        let slept = {
            let mut awake = self.awake.lock().unwrap();
            let now = crate::hub_session::Awake::now();
            let slept = awake.slept_until(&now);
            if slept {
                *awake = now;
            }
            slept
        };
        if slept {
            self.lost_hub();
        }
        self.heard
            .borrow()
            .is_some_and(|since| since.elapsed() < self.first_list)
    }

    /// Until the hub was heard, `first_list` at most from when the daemon started waiting.
    async fn hub_heard(&self) {
        let mut heard = self.heard.subscribe();
        loop {
            let Some(since) = *heard.borrow_and_update() else {
                return;
            };
            let until = since + self.first_list;
            match tokio::time::timeout_at(until, heard.changed()).await {
                Ok(Ok(())) => continue,
                _ => return,
            }
        }
    }

    /// Closes every link, telling the other machines.
    pub async fn close(&self) {
        self.endpoint.close().await;
    }

    /// Stops this link for good: its tasks end and every link closes (remote turned off).
    pub async fn stop(&self) {
        self.halt.send_replace(true);
        self.close().await;
    }

    /// The machines this one trusts: the file as last read whole, read again only when it changed
    /// since (a hand edit changes its stamp). A list kept that the file could not be written
    /// with (disk full, read-only folder) is not given back the machines it lost.
    fn peers(&self) -> Peers {
        let file = self.folder.join(PEERS_FILE);
        let mut kept = self.peers.lock().unwrap();
        let now = stamp(&file);
        if now == kept.read {
            return kept.last.clone();
        }
        kept.read = now;
        match read_json::<Peers>(&file) {
            Ok(peers) => {
                kept.said = false;
                kept.whole = true;
                kept.last = peers.clone();
                peers
            }
            Err(error) => {
                if !kept.said {
                    eprintln!(
                        "flottille daemon: {} is not read, the last one read is kept: {error}",
                        file.display()
                    );
                    kept.said = true;
                }
                kept.last.clone()
            }
        }
    }

    /// The machine of `key`, when this one trusts it: by the peers' file as last read, read
    /// again only when it changed since, so that a key refused costs no read of it.
    fn trusted(&self, key: &PublicKey) -> Option<Peer> {
        let find = |peers: &Peers| peers.machines.iter().find(|peer| peer.key == *key).cloned();
        {
            let kept = self.peers.lock().unwrap();
            if let Some(peer) = find(&kept.last) {
                return Some(peer);
            }
            if stamp(&self.folder.join(PEERS_FILE)) == kept.read {
                return None;
            }
        }
        find(&self.peers())
    }

    /// Whether the peers as last read hold `key`: no file is read, so that it is asked under the
    /// `seen` lock. `trust_machines` changes them before it cuts the links under that lock.
    fn trusted_kept(&self, key: &PublicKey) -> bool {
        let kept = self.peers.lock().unwrap();
        kept.last.machines.iter().any(|peer| peer.key == *key)
    }

    /// A key refused: said in the log, unless it was lately.
    fn say_refused(&self, key: &PublicKey) {
        for line in self.refused_lines(Some(key), Instant::now()) {
            eprintln!("flottille daemon: {line}");
        }
    }

    /// What the log says of `key` refused at `now`: the key, the first time within `SAID_AGAIN`
    /// and while fewer than `MOST_SAID` were; then, once, that the others are counted. When
    /// `SAID_AGAIN` has passed, with a key or none, how many were counted, and all starts again.
    fn refused_lines(&self, key: Option<&PublicKey>, now: Instant) -> Vec<String> {
        let mut refused = self.refused.lock().unwrap();
        let mut lines = Vec::new();
        if now.duration_since(refused.since) >= SAID_AGAIN {
            if refused.others > 0 {
                lines.push(format!(
                    "{} other links were refused in the last minute, from keys not said",
                    refused.others
                ));
            }
            *refused = Refused::new(now);
        }
        let Some(key) = key else {
            return lines;
        };
        if refused.said.contains(key.as_bytes()) {
            return lines;
        }
        if refused.said.len() < MOST_SAID {
            refused.said.insert(*key.as_bytes());
            lines.push(format!(
                "refused a link from {}: not a machine this one trusts",
                key.fingerprint()
            ));
        } else {
            if refused.others == 0 {
                lines.push(format!(
                    "more than {MOST_SAID} keys were refused within a minute; the others are \
                     counted, not said"
                ));
            }
            refused.others += 1;
        }
        lines
    }

    /// A link made, `dialled` by this machine or by the other: kept, and served. When the two dialled each other at once, each holds two links: both
    /// keep the one the machine of the lower key dialled, and close the other.
    fn linked(self: &Arc<Self>, link: Link, name: String, dialled: bool) {
        let generation = self
            .links
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let key = *link.peer().as_bytes();
        // The peers' file read again if it changed, before the lock: none is read under it.
        let trusted = self.trusted(&link.peer()).is_some();
        {
            let mut seen = self.seen.lock().unwrap();
            // Judged again under the lock `trust_machines` cuts the links under: a removal that
            // came while this link was made has cut the others already, and would miss this one.
            if !trusted || !self.trusted_kept(&link.peer()) {
                link.close(NOT_TRUSTED);
                return;
            }
            let seen = seen.entry(key).or_insert_with(Seen::new);
            if let Some((_, old, old_dialled)) = &seen.link
                && *old_dialled != dialled
            {
                let ours_lower = self.endpoint.key().as_bytes() < link.peer().as_bytes();
                // The lower key's link is the one this side dialled when its key is the lower.
                let keep_new = dialled == ours_lower;
                let dialler = if ours_lower {
                    self.endpoint.key()
                } else {
                    link.peer()
                };
                eprintln!(
                    "flottille daemon: linked to {name} twice; kept the link dialled by {}",
                    dialler.fingerprint()
                );
                // The other is let go, not cut: what is under way on it is finished, what comes
                // is turned away unserved, and goes on the link kept (`Remote::send`).
                if !keep_new {
                    link.retire(LINKED_ALREADY);
                    // Read still, so that what comes on it is turned away, never left unread.
                    tokio::spawn(async move { while link.next().await.is_some() {} });
                    return;
                }
                old.retire(LINKED_ALREADY);
            } else if let Some((_, old, _)) = &seen.link {
                // A link of the same way again: the old one is gone and not noticed yet.
                old.retire(LINKED_ALREADY);
            }
            seen.set(PeerState::Online, None);
            seen.link = Some((generation, link.clone(), dialled));
            seen.wait = ROUND;
        }
        self.relinked.notify_waiters();
        eprintln!(
            "flottille daemon: linked to {name} (protocol {})",
            link.protocol()
        );
        (self.emit)(
            "remote.connected",
            json!({"name": name, "fingerprint": link.peer().fingerprint()}),
        );
        tokio::spawn(offer(
            link.clone(),
            self.offered.subscribe(),
            MachineMessage::Capabilities,
            None,
        ));
        tokio::spawn(offer(
            link.clone(),
            self.sessions.subscribe(),
            MachineMessage::Sessions,
            None,
        ));
        tokio::spawn(offer(
            link.clone(),
            self.projects.subscribe(),
            MachineMessage::Projects,
            Some((
                name.to_owned(),
                "this machine's projects: its sessions go on, its project rows stay by name",
            )),
        ));
        tokio::spawn(offer(
            link.clone(),
            self.pilots.subscribe(),
            MachineMessage::Pilots,
            Some((
                name.to_owned(),
                "the pilots of this machine's led sessions: its sessions go on, those led from \
                 another machine as roots",
            )),
        ));
        self.offer_now.notify_one();
        // What was kept for that machine while it was off goes now, in order.
        if let Some(daemon) = self.daemon() {
            let machine: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
            crate::across::send_kept(&daemon, &machine);
            // What its led sessions asked the user while it was off, told once.
            crate::across::relinked(&daemon, &machine);
        }
        // The ghosts of its sessions, kept current while this link is the machine's.
        tokio::spawn(crate::ghost::follow(
            self.clone(),
            link.clone(),
            name.to_owned(),
            generation,
        ));
        tokio::spawn(serve(self.clone(), link, name, generation));
    }

    /// A link that could not be made, or that ended: the machine's state, said once in the log.
    fn failed(&self, key: [u8; 32], name: &str, error: &LinkError) {
        let reason = self.said(error);
        let mut seen = self.seen.lock().unwrap();
        let seen = seen.entry(key).or_insert_with(Seen::new);
        if seen.reason.as_deref() != Some(reason.as_str()) {
            eprintln!("flottille daemon: no link to {name}: {reason}");
        }
        seen.set(error.state(), Some(reason));
        seen.dialling = false;
        seen.next_try = Instant::now() + seen.wait;
        seen.wait = (seen.wait * 2).min(LONGEST_WAIT);
    }

    /// `remote.machines`: this machine, and the others as it sees them, with their roles and what
    /// the runners offer, this machine's looked at now; with `project`, the runners that would
    /// take a session of it on `agent` too (`offered`). It reads files and checks the list: called
    /// on a blocking thread.
    fn machines(&self, project: Option<&str>, agent: Option<&str>) -> Value {
        let key = self.endpoint.key();
        let listed = self.listed().ok();
        let role_of = |key: &PublicKey| {
            listed
                .as_ref()
                .and_then(|(members, _)| members.by_machine(key))
                .map(|member| member.role)
        };
        let offers = self.offers_here(listed.as_ref());
        let this = json!({
            // Its member's name, once it is one: what a client signs a request of this machine's
            // for (`vault.share`).
            "name": listed.as_ref().map(|(_, name)| name),
            "role": role_of(&key),
            "offers": offers,
            "key": key,
            "fingerprint": key.fingerprint(),
        });
        // Read before the lock is taken: no file is read under it.
        let peers = self.peers().machines;
        let mut seen = self.seen.lock().unwrap();
        let mut offered = Vec::new();
        if let (Some(project), Some((_, name)), Some(offers)) = (project, &listed, &offers)
            && offers.refusal(name, project, agent).is_none()
        {
            offered.push(name.clone());
        }
        let machines: Vec<Value> = peers
            .into_iter()
            .map(|peer| {
                let seen = seen.entry(*peer.key.as_bytes()).or_insert_with(Seen::new);
                if let Some(project) = project
                    && seen.link.is_some()
                    && let Some(offers) = &seen.offers
                    && offers.refusal(&peer.name, project, agent).is_none()
                {
                    offered.push(peer.name.clone());
                }
                json!({
                    "name": peer.name,
                    "key": peer.key,
                    "fingerprint": peer.key.fingerprint(),
                    "role": role_of(&peer.key),
                    "offers": seen.offers,
                    "state": seen.state.to_string(),
                    "since": seen.since,
                    "reason": seen.reason,
                    // The live link, through the hub, and whether this machine dialled it.
                    "reached": seen.link.as_ref().map(|(_, _, dialled)| {
                        json!({"address": "the hub", "dialled": dialled})
                    }),
                })
            })
            .collect();
        drop(seen);
        let hub = self
            .control
            .get()
            .map_or(Value::Null, |control| control.seen());
        let proposed = self
            .daemon
            .get()
            .and_then(std::sync::Weak::upgrade)
            .map_or(Value::Null, |daemon| crate::hub_session::proposed(&daemon));
        let mut machines = json!({
            "this": this,
            "machines": machines,
            "hub": hub,
            "proposed": proposed,
        });
        // Starting a session from a client: the runners that would take it, this machine first.
        if project.is_some() {
            machines["offered"] = json!(offered);
        }
        machines
    }
}

impl Remote {
    /// What this machine offers now, as `listed` says its member: none unless it is a runner.
    /// It reads the projects' files and the store: called on a blocking thread.
    fn offers_here(
        &self,
        listed: Option<&(flottille_link::Members, String)>,
    ) -> Option<Capabilities> {
        let (members, name) = listed?;
        let runner = members
            .members()
            .iter()
            .any(|member| member.name == *name && member.role == Role::Runner);
        let daemon = self.daemon.get().and_then(std::sync::Weak::upgrade)?;
        runner.then(|| offered(&daemon))
    }

    /// This machine's live root sessions, light, as `listed` says its member: none unless it is
    /// a runner, and an error when the store cannot be read (what is listed then stays as it was).
    /// It reads the store: called on a blocking thread.
    fn sessions_here(
        &self,
        listed: Option<&(flottille_link::Members, String)>,
    ) -> flottille_core::Result<Option<flottille_link::SessionList>> {
        let Some((members, name)) = listed else {
            return Ok(None);
        };
        let runner = members
            .members()
            .iter()
            .any(|member| member.name == *name && member.role == Role::Runner);
        let Some(daemon) = self.daemon.get().and_then(std::sync::Weak::upgrade) else {
            return Ok(None);
        };
        if !runner {
            return Ok(None);
        }
        try_sessions_of(&daemon).map(Some)
    }

    /// This machine's projects, as `sessions_here` says its sessions. It runs git for each
    /// repository: called on a blocking thread.
    fn projects_here(
        &self,
        listed: Option<&(flottille_link::Members, String)>,
    ) -> Option<flottille_link::ProjectList> {
        let (members, name) = listed?;
        let runner = members
            .members()
            .iter()
            .any(|member| member.name == *name && member.role == Role::Runner);
        let daemon = self.daemon.get().and_then(std::sync::Weak::upgrade)?;
        runner.then(|| projects_of(&daemon))
    }

    /// Whether a machine is linked to this one now.
    fn linked_to_any(&self) -> bool {
        self.seen
            .lock()
            .unwrap()
            .values()
            .any(|seen| seen.link.is_some())
    }

    /// What this machine offers now, kept for the links to send when it changed. Looked at away
    /// from the tasks that take links: the list is checked, the projects' files read.
    async fn offer_if_changed(self: &Arc<Self>) {
        let remote = self.clone();
        let (offers, sessions, projects, pilots) = tokio::task::spawn_blocking(move || {
            let listed = remote.listed().ok();
            // A store that cannot be read says nothing this round: the sessions and their pilots
            // stay as last said, never an empty list that makes every machine forget them.
            let sessions = match remote.sessions_here(listed.as_ref()) {
                Ok(sessions) => Some(sessions),
                Err(error) => {
                    eprintln!("flottille daemon: the sessions could not be listed: {error}");
                    None
                }
            };
            let pilots = sessions.as_ref().map(|sessions| {
                sessions
                    .as_ref()
                    .zip(remote.daemon())
                    .map(|(sessions, daemon)| pilots_of(&daemon, sessions))
            });
            (
                remote.offers_here(listed.as_ref()),
                sessions,
                remote.projects_here(listed.as_ref()),
                pilots,
            )
        })
        .await
        .unwrap_or_default();
        self.offered.send_if_modified(|current| {
            let changed = *current != offers;
            *current = offers;
            changed
        });
        if let Some(sessions) = sessions {
            self.sessions.send_if_modified(|current| {
                let changed = *current != sessions;
                *current = sessions;
                changed
            });
        }
        self.projects.send_if_modified(|current| {
            let changed = *current != projects;
            *current = projects;
            changed
        });
        if let Some(pilots) = pilots {
            self.pilots.send_if_modified(|current| {
                let changed = *current != pilots;
                *current = pilots;
                changed
            });
        }
    }

    /// What the machine of `key` offers, said over its link of `generation`: kept when its member
    /// is a runner and the offer within its bounds; refused otherwise, with why. It checks the
    /// list: called on a blocking thread.
    fn took_offers(
        &self,
        key: &PublicKey,
        generation: u64,
        offers: Capabilities,
    ) -> Result<Value, String> {
        let (members, _) = self.listed()?;
        let member = members
            .by_machine(key)
            .ok_or_else(|| "this machine is no member's".to_owned())?;
        if member.role != Role::Runner {
            return Err(format!(
                "{} is a {}: only a runner offers",
                member.name,
                member.role.word()
            ));
        }
        offers.check()?;
        self.keep_offers(key, generation, offers);
        Ok(json!({}))
    }

    /// This machine's live root sessions as it lists them to the others, from now on: none
    /// while its member is no runner.
    pub(crate) fn listed_sessions(
        &self,
    ) -> tokio::sync::watch::Receiver<Option<flottille_link::SessionList>> {
        self.sessions.subscribe()
    }

    /// Whether the link of `generation` is the one kept for the machine of `key`.
    pub(crate) fn on_link(&self, key: &PublicKey, generation: u64) -> bool {
        self.seen
            .lock()
            .unwrap()
            .get(key.as_bytes())
            .is_some_and(|seen| {
                seen.link
                    .as_ref()
                    .is_some_and(|(at, _, _)| *at == generation)
            })
    }

    /// Keeps what the machine of `key` offers, when its link of `generation` is the one kept: a
    /// link replaced since says nothing of the machine any more.
    fn keep_offers(&self, key: &PublicKey, generation: u64, offers: Capabilities) {
        if let Some(seen) = self.seen.lock().unwrap().get_mut(key.as_bytes())
            && seen
                .link
                .as_ref()
                .is_some_and(|(at, _, _)| *at == generation)
        {
            seen.offers = Some(offers);
        }
    }
}

/// How often, while a link is up, what this machine offers is looked at again for what no event
/// says: a login file that appeared or went.
const OFFER_POLL: Duration = Duration::from_secs(30);

/// [`OFFER_POLL`], or, in a test build, `FLOTTILLE_TEST_OFFER_MS`: a test sees a login file
/// within a moment.
fn offer_poll() -> Duration {
    if cfg!(debug_assertions)
        && let Some(ms) = std::env::var("FLOTTILLE_TEST_OFFER_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
    {
        return Duration::from_millis(ms);
    }
    OFFER_POLL
}

/// Whether what this machine offers, or its list of sessions, may have changed with the event
/// `name`: a project added, edited or removed, a provider changed, a session that started,
/// ended, was written to, answered or asked; never what an agent streams during a turn.
fn moves_offer(name: &str) -> bool {
    (name.starts_with("agent.")
        && !matches!(
            name,
            "agent.message"
                | "agent.thought"
                | "agent.tool_call"
                | "agent.tool_served"
                | "agent.usage"
                | "agent.plan"
        ))
        || name.starts_with("approval.")
        || name.starts_with("project.")
        || name.starts_with("provider.")
        || matches!(
            name,
            "agent.created"
                | "agent.ended"
                | "agent.failed"
                | "agent.died"
                | "agent.resumed"
                | "agent.replaced"
        )
}

/// Keeps what this machine offers (`docs/remote.md`, "Roles"), looked at when a link is made,
/// after an event that may change it (`moves_offer`) and every `OFFER_POLL`; never while no
/// machine is linked to this one.
async fn keep_offering(remote: Arc<Remote>, mut stopping: tokio::sync::watch::Receiver<bool>) {
    let mut live = remote
        .daemon
        .get()
        .and_then(std::sync::Weak::upgrade)
        .and_then(|daemon| daemon.live());
    let mut poll = tokio::time::interval(offer_poll());
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = remote.offer_now.notified() => {}
            _ = poll.tick() => {}
            moved = moved(&mut live) => if !moved { continue },
            _ = stopping.wait_for(|stopping| *stopping) => return,
        }
        if remote.linked_to_any() {
            remote.offer_if_changed().await;
        }
    }
}

/// The next event of the daemon's: whether it may change what this machine offers. Events
/// missed say yes; once the daemon stops, no event comes.
async fn moved(
    live: &mut Option<tokio::sync::broadcast::Receiver<flottille_protocol::Event>>,
) -> bool {
    let Some(events) = live else {
        return std::future::pending().await;
    };
    match events.recv().await {
        Ok(event) => moves_offer(&event.name),
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
            *live = None;
            false
        }
    }
}

/// What this machine offers (`docs/remote.md`, "Roles"): each project with its children cap
/// minus the children that run or are being launched, and each provider enabled, installed (its
/// program on `PATH`, or a test's override) and signed in where Flottille can see it: its login
/// file present, checked by its metadata only and never opened, or its sign-in variable set,
/// never read.
pub(crate) fn offered(daemon: &Daemon) -> Capabilities {
    let launching = daemon.launching.lock().unwrap().clone();
    let projects = crate::projects::all(daemon)
        .into_iter()
        .map(|project| {
            let running = daemon
                .read(|store| store.live_children(&project.name))
                // A store that cannot say takes nothing.
                .unwrap_or(usize::MAX)
                .saturating_add(launching.get(&project.name).copied().unwrap_or(0));
            let slots = usize::from(project.max_children).saturating_sub(running);
            Offered {
                name: project.name,
                slots: u32::try_from(slots).unwrap_or(u32::MAX),
            }
        })
        .collect();
    let agents = flottille_core::provider::Settings::load(&daemon.paths)
        .map(|settings| {
            crate::providers::checks(&settings, crate::providers::Version::Skip)
                .into_iter()
                .filter(|check| {
                    (check.program.is_some() || check.overridden.is_some())
                        && check.ready()
                        && (check.login_present || check.api_key.is_some())
                })
                .map(|check| check.provider.name.to_owned())
                .collect()
        })
        .unwrap_or_default();
    Capabilities { projects, agents }
}

/// This machine's live root sessions, light (`docs/remote.md`, "One list of every machine"):
/// each one's id, title, project, state and last exchange, and the session of the agent that
/// leads it here; never its conversation. Those of the latest exchange when there are more than
/// a list holds.
pub(crate) fn sessions_of(daemon: &Daemon) -> flottille_link::SessionList {
    try_sessions_of(daemon).unwrap_or_default()
}

/// `sessions_of`, or why the agents could not be read: an empty list is no answer of a store that
/// failed, since the machines that follow this list would drop every session they keep of it.
pub(crate) fn try_sessions_of(
    daemon: &Daemon,
) -> flottille_core::Result<flottille_link::SessionList> {
    use flottille_core::agent::AgentState;
    let agents = daemon.read(|store| store.agents())?;
    let parents: HashMap<&str, Option<&str>> = agents
        .iter()
        .map(|agent| (agent.id.as_str(), agent.parent.as_deref()))
        .collect();
    fn root<'a>(parents: &HashMap<&'a str, Option<&'a str>>, mut id: &'a str) -> &'a str {
        while let Some(Some(parent)) = parents.get(id) {
            id = parent;
        }
        id
    }
    let root_of = |id: &str| root(&parents, id).to_owned();
    // The trees where a turn runs, by their root.
    let busy: HashSet<String> = agents
        .iter()
        .filter(|agent| matches!(agent.state, AgentState::Working | AgentState::Starting))
        .map(|agent| root_of(&agent.id))
        .collect();
    // The trees a request held for the user waits in, by their root: a launch held for the user
    // among them, as the session's own window says it (`needs_user`), and those whose agent asked
    // for a vault entry, as the window counts them.
    let mut held: HashSet<String> = daemon
        .read(|store| store.pending_requests(flottille_core::approval::USER))
        .unwrap_or_default()
        .iter()
        .map(|request| root_of(&request.agent))
        .collect();
    held.extend(
        daemon
            .read(crate::vault::asking)
            .unwrap_or_default()
            .iter()
            .map(|agent| root_of(agent)),
    );
    // Only the projects this machine's own window shows, those whose file can be used: a session
    // of a project whose file is gone or has a typo is shown by no window here, and the machine
    // it is listed on could not act on it. It is listed again once the file is back. A folder
    // that cannot be read says nothing of any project: then none is left out.
    let projects = crate::projects::usable_names(daemon);
    let mut sessions: Vec<flottille_link::LightSession> = agents
        .iter()
        // A child an agent of another machine launched here is that agent's, no session to list.
        .filter(|agent| {
            agent.parent.is_none()
                && !agent.state.is_final()
                && projects
                    .as_ref()
                    .is_none_or(|known| known.contains(&agent.project))
                && !crate::across::remote_child(daemon, agent)
        })
        .map(|agent| {
            let title = agent
                .title
                .clone()
                .or_else(|| {
                    agent
                        .brief
                        .as_deref()
                        .and_then(|brief| brief.lines().find(|line| !line.trim().is_empty()))
                        .map(|line| line.trim().to_owned())
                })
                .unwrap_or_default();
            let busy = busy.contains(&agent.id);
            // A question to its pilot is the pilot's to answer, not the user's.
            let state = if crate::tree::asks_the_user(daemon, agent) || held.contains(&agent.id) {
                "needs you"
            } else if busy {
                "running"
            } else {
                "idle"
            };
            flottille_link::LightSession {
                id: agent.id.clone(),
                title: shown_text(&title, flottille_link::TITLE_LIMIT),
                project: agent.project.clone(),
                state: state.to_owned(),
                exchanged_at: agent.exchanged_at.unwrap_or(agent.created_at),
                created_at: agent.created_at,
                pilot: agent
                    .launched_by
                    .as_deref()
                    .filter(|_| agent.led)
                    // A pilot of another machine is no session of this one's list.
                    .filter(|pilot| !crate::across::is_stand_in(pilot))
                    .filter(|pilot| parents.contains_key(pilot))
                    .map(root_of),
            }
        })
        .collect();
    sessions.sort_by(|a, b| b.exchanged_at.cmp(&a.exchanged_at).then(a.id.cmp(&b.id)));
    sessions.truncate(flottille_link::MOST_SESSIONS);
    Ok(flottille_link::SessionList { sessions })
}

/// The pilots of another machine of the led sessions of `list`, this machine's: each session led by
/// an agent of another machine stood in for here, as `agent.list` says it (`launched_from`), by
/// that machine's name and the agent's id there. Its window draws such a session under its pilot,
/// and every other machine's does from this (`docs/remote.md`, "One list of every machine").
pub(crate) fn pilots_of(
    daemon: &Daemon,
    list: &flottille_link::SessionList,
) -> flottille_link::PilotList {
    let pilots = list
        .sessions
        .iter()
        .filter_map(|session| {
            let agent = daemon.read(|store| store.agent(&session.id)).ok()??;
            let stand_in = agent.launched_by.as_deref().filter(|_| agent.led)?;
            let (caller, machine, kind) = crate::launches::launched_from(daemon, &agent.id)?;
            // Still led, by that agent: a session detached is the user's, under no row.
            (kind == "led" && stand_in.split_once('@').map(|(id, _)| id) == Some(caller.as_str()))
                .then_some(flottille_link::LightPilot {
                    session: session.id.clone(),
                    machine,
                    pilot: caller,
                })
        })
        // What would make the other machine refuse the list is not sent: that session stays a
        // root there.
        .filter(|pilot| {
            flottille_link::PilotList {
                pilots: vec![pilot.clone()],
            }
            .check()
            .is_ok()
        })
        .collect();
    flottille_link::PilotList { pilots }
}

/// Says beside each session of the list of the machine `name` its pilot of another machine, as
/// that machine said it (`pilot_elsewhere`, `{machine, agent}`, or `null`): a pilot is said only
/// of a session of that same list, and only on a member machine other than `name`. Where it
/// draws the row is the window's to judge.
fn with_pilots(
    sessions: &mut [Value],
    pilots: &[flottille_link::LightPilot],
    name: &str,
    members: &[String],
) {
    for session in sessions {
        let pilot = pilots
            .iter()
            .find(|pilot| session["id"].as_str() == Some(pilot.session.as_str()))
            .filter(|pilot| pilot.machine != name && members.contains(&pilot.machine))
            .map(|pilot| json!({"machine": pilot.machine, "agent": pilot.pilot}));
        session["pilot_elsewhere"] = pilot.unwrap_or(Value::Null);
    }
}

/// This machine's projects as a list shows them: each one's name and the remotes of its
/// repositories, normalised, so that another machine can tell it holds the same project
/// (`docs/remote.md`, "One list of every machine"). A repository with no remote says nothing, and
/// its project then matches none. It runs `git remote get-url` for each repository: called on a
/// blocking thread.
fn projects_of(daemon: &Daemon) -> flottille_link::ProjectList {
    let mut projects: Vec<flottille_link::LightProject> = crate::projects::all(daemon)
        .into_iter()
        .map(|project| {
            let remotes: Vec<Option<String>> = project
                .repositories
                .iter()
                .map(|repository| {
                    let folder = match repository.path.as_str() {
                        "." => project.folder.clone(),
                        path => project.folder.join(path),
                    };
                    repository_url(&folder, &repository.remote)
                })
                .collect();
            flottille_link::LightProject {
                name: shown_text(&project.name, 64),
                remotes: flottille_core::remote_key::project_key(
                    remotes.iter().map(Option::as_deref),
                )
                .unwrap_or_default(),
            }
        })
        .collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    projects.truncate(flottille_link::MOST_PROJECTS);
    flottille_link::ProjectList { projects }
}

/// The address the remote `remote` of the repository in `folder` leads to, when it has one; never
/// read for a folder that is not its own repository (git would answer for the one around it).
fn repository_url(folder: &Path, remote: &str) -> Option<String> {
    if !folder.join(".git").exists() {
        return None;
    }
    let output = crate::children::spawn_std(&mut crate::git::blocking(
        crate::git::own(),
        folder,
        &["remote", "get-url", "--end-of-options", remote],
    ))
    .and_then(std::process::Child::wait_with_output)
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `text` as another machine may show it: its first `most` characters, without a control
/// character.
fn shown_text(text: &str, most: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(most)
        .collect()
}

/// The first wait before an offer the other machine did not take is sent again; it doubles up to
/// `OFFER_RETRY_MOST`.
const OFFER_RETRY: Duration = Duration::from_secs(1);
const OFFER_RETRY_MOST: Duration = Duration::from_secs(60);

/// Sends a machine what this one offers (or its list of sessions) when the link is made, then
/// each time it changes, in order, until the link ends, as the message `message` makes. One it
/// did not take is sent again, waiting longer each time.
///
/// `older` names the machine, and what it does not take, when the message is one a machine of
/// before does not know: its refusal of the kind (`UNKNOWN_MESSAGE`) is said once, as that machine
/// running an older Flottille, and ends the offer on this link, rather than being sent again for
/// ever. The link and the other messages go on.
async fn offer<T: Clone>(
    link: Link,
    mut offered: tokio::sync::watch::Receiver<Option<T>>,
    message: fn(T) -> MachineMessage,
    older: Option<(String, &'static str)>,
) {
    let mut wait = OFFER_RETRY;
    loop {
        let offers = offered.borrow_and_update().clone();
        let mut again = None;
        if let Some(offers) = offers {
            let sent = link.send(&Message::Machine(message(offers))).await;
            match sent {
                Ok(_) => wait = OFFER_RETRY,
                Err(flottille_link::LinkError::Declined { reason, .. })
                    if older.is_some() && reason == flottille_link::UNKNOWN_MESSAGE =>
                {
                    let (name, what) = older.unwrap_or_default();
                    eprintln!(
                        "flottille daemon: {name} runs an older Flottille, which does not take \
                         {what} until it is updated (flottille update)"
                    );
                    return;
                }
                Err(_) => {
                    again = Some(wait);
                    wait = (wait * 2).min(OFFER_RETRY_MOST);
                }
            }
        }
        let retry = async {
            match again {
                Some(wait) => tokio::time::sleep(wait).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            changed = offered.changed() => if changed.is_err() { return },
            _ = link.closed() => return,
            _ = retry => {}
        }
    }
}

/// The machines that dial this one: kept when trusted, refused otherwise.
async fn accept(remote: Arc<Remote>) {
    // A test makes both machines take a link they are dialled on late, so that they dial each
    // other at once: in a test build only.
    let late = cfg!(debug_assertions)
        .then(|| std::env::var("FLOTTILLE_TEST_LINK_ACCEPT_DELAY_MS").ok())
        .flatten()
        .and_then(|text| text.parse().ok())
        .map(Duration::from_millis);
    while let Some(accepting) = remote.endpoint.accept().await {
        let key = accepting.peer();
        // A key the list kept does not trust is refused now, whatever comes: a machine the next
        // list adds dials again. Only one it trusts waits, so that no one piles up waiting links.
        if remote.trusted(&key).is_none() {
            remote.say_refused(&key);
            accepting.refuse(NOT_TRUSTED);
            continue;
        }
        let remote = remote.clone();
        tokio::spawn(async move {
            // Judged by the hub's list, not by the one kept, while one is on its way: one link of
            // a key waits at most.
            if remote.holding() {
                let Some(_waiting) = Waiting::of(&remote, &key) else {
                    accepting.refuse(WAITS_ALREADY);
                    return;
                };
                remote.hub_heard().await;
            }
            let Some(peer) = remote.trusted(&key) else {
                remote.say_refused(&key);
                accepting.refuse(NOT_TRUSTED);
                return;
            };
            let traffic = accepting.traffic();
            match accepting.handshake(&remote.hello, &peer.name).await {
                Ok(link) if traffic == Traffic::Machine => {
                    if let Some(late) = late {
                        tokio::time::sleep(late).await;
                    }
                    remote.linked(link, peer.name, false)
                }
                Ok(link) => serve_clients(remote, link, peer.name).await,
                Err(error) => remote.failed(*key.as_bytes(), &peer.name, &error),
            }
        });
    }
}

/// Why a second link of a machine whose first one waits for the hub's list is refused.
const WAITS_ALREADY: &str = "a link from this machine waits already for the hub's list";

/// A key whose link waits for the hub's list, until the link is judged.
struct Waiting<'a> {
    remote: &'a Remote,
    key: [u8; 32],
}

impl<'a> Waiting<'a> {
    /// `None` when a link of `key` waits already.
    fn of(remote: &'a Remote, key: &PublicKey) -> Option<Waiting<'a>> {
        let key = *key.as_bytes();
        let first = remote.waiting.lock().unwrap().insert(key);
        // Made only for the first: the guard of another would let the first one's key go.
        first.then(|| Waiting { remote, key })
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.remote.waiting.lock().unwrap().remove(&self.key);
    }
}

/// What another machine sends on a machine link.
async fn serve(remote: Arc<Remote>, link: Link, name: String, generation: u64) {
    while let Some(received) = link.next().await {
        match received.message() {
            // A machine of the previous version tells its addresses on every link: ignored.
            Message::Machine(MachineMessage::Addresses(_)) => received.answer(Ok(json!({}))),
            Message::Machine(MachineMessage::Capabilities(offers)) => {
                // The list is checked, its signatures too: away from the tasks that take links.
                let (taking, key, offers) = (remote.clone(), link.peer(), offers.clone());
                let answer = tokio::task::spawn_blocking(move || {
                    taking.took_offers(&key, generation, offers)
                })
                .await
                .unwrap_or_else(|_| Err("the offer could not be taken".to_owned()));
                if let Err(why) = &answer {
                    eprintln!("flottille daemon: what {name} offers was not taken: {why}");
                }
                received.answer(answer);
            }
            Message::Machine(MachineMessage::Sessions(list)) => {
                // The list is checked, the store written: away from the tasks that take links.
                let (taking, key, list) = (remote.clone(), link.peer(), list.clone());
                let answer = tokio::task::spawn_blocking(move || {
                    taking.took_sessions(&key, generation, list)
                })
                .await
                .unwrap_or_else(|_| Err("the list could not be taken".to_owned()));
                if let Err(why) = &answer {
                    eprintln!("flottille daemon: the sessions {name} lists were not kept: {why}");
                }
                received.answer(answer);
            }
            Message::Machine(MachineMessage::Projects(list)) => {
                let (taking, key, list) = (remote.clone(), link.peer(), list.clone());
                let answer = tokio::task::spawn_blocking(move || {
                    taking.took_projects(&key, generation, list)
                })
                .await
                .unwrap_or_else(|_| Err("the projects could not be taken".to_owned()));
                if let Err(why) = &answer {
                    eprintln!("flottille daemon: the projects {name} lists were not kept: {why}");
                }
                received.answer(answer);
            }
            Message::Machine(MachineMessage::Pilots(list)) => {
                let (taking, key, list) = (remote.clone(), link.peer(), list.clone());
                let answer =
                    tokio::task::spawn_blocking(move || taking.took_pilots(&key, generation, list))
                        .await
                        .unwrap_or_else(|_| Err("the pilots could not be taken".to_owned()));
                if let Err(why) = &answer {
                    eprintln!("flottille daemon: the pilots {name} lists were not kept: {why}");
                }
                received.answer(answer);
            }
            // In a task of its own: the session's stop may take a moment.
            Message::Machine(MachineMessage::Close(request)) => {
                let (remote, request, peer) = (remote.clone(), (**request).clone(), link.peer());
                tokio::spawn(async move {
                    let answered = remote.closed(&peer, request).await;
                    received.answer(crate::across::answer_of(answered, "a close"));
                });
            }
            // Each in a task of its own: a bundle offered is read back on this same link.
            Message::Machine(MachineMessage::Git(git)) => {
                let git = git.clone();
                tokio::spawn(crate::travel::serve(
                    remote.clone(),
                    link.clone(),
                    received,
                    git,
                ));
            }
            Message::Machine(MachineMessage::Share(share)) => {
                let (remote, share, peer) = (remote.clone(), share.clone(), link.peer());
                tokio::spawn(async move {
                    let answered = remote.shared(&peer, share).await;
                    received.answer(answered);
                });
            }
            Message::Machine(MachineMessage::ShareConnection(share)) => {
                let (remote, share, peer) = (remote.clone(), share.clone(), link.peer());
                tokio::spawn(async move {
                    let answered = remote.shared_connection(&peer, share).await;
                    received.answer(answered);
                });
            }
            // In a task of its own: the judge may take seconds.
            Message::Machine(MachineMessage::Launch(asked)) => {
                let asked = asked.clone();
                tokio::spawn(crate::launches::serve(
                    remote.clone(),
                    link.peer(),
                    received,
                    asked,
                ));
            }
            Message::Machine(MachineMessage::Deliver(deliver)) => {
                let deliver = deliver.clone();
                tokio::spawn(crate::across::serve_deliver(
                    remote.clone(),
                    link.peer(),
                    received,
                    deliver,
                ));
            }
            // Served for as long as its stream lasts.
            Message::Machine(MachineMessage::Ghost(ask)) => {
                let ask = ask.clone();
                tokio::spawn(crate::ghost::serve(
                    remote.clone(),
                    name.clone(),
                    received,
                    ask,
                ));
            }
            Message::Machine(MachineMessage::Call(call)) => {
                let call = call.clone();
                tokio::spawn(crate::across::serve_call(
                    remote.clone(),
                    link.peer(),
                    received,
                    call,
                ));
            }
            // Kept apart by the link's protocol name: a client's request on a machine link is
            // refused before it gets here (`Link::next`).
            Message::Client(_) => received.answer(Err(
                "a client's request is not taken on a machine link".into(),
            )),
        }
    }
    let why = link.closed().await;
    if remote.lost(&link, &name, generation, why)
        && let Some(daemon) = remote.daemon()
    {
        // What the sessions it started here asked of its agents goes to the user at once.
        let machine: String = link
            .peer()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        crate::across::unlinked(&daemon, &machine, &name);
    }
}

impl Remote {
    /// The link `link` of `generation` to `name` ended: whether it was the machine's link then.
    fn lost(&self, link: &Link, name: &str, generation: u64, why: String) -> bool {
        let mut seen = self.seen.lock().unwrap();
        let Some(seen) = seen.get_mut(link.peer().as_bytes()) else {
            return false;
        };
        if !seen
            .link
            .as_ref()
            .is_some_and(|(at, _, _)| *at == generation)
        {
            return false;
        }
        eprintln!("flottille daemon: the link to {name} ended: {why}");
        (self.emit)(
            "remote.disconnected",
            json!({"name": name, "fingerprint": link.peer().fingerprint(), "reason": why}),
        );
        seen.link = None;
        seen.offers = None;
        if let Some(client) = seen.client.take() {
            client.close(&why);
        }
        seen.set(PeerState::Offline, Some(why));
        seen.next_try = Instant::now();
        true
    }
}

impl Remote {
    /// The live root sessions the machine of `key` lists, said over its link: kept in place of
    /// those it said before when its member is a runner and the list within its bounds; refused
    /// otherwise, with why. It checks the list and writes the store: called on a blocking thread.
    fn took_sessions(
        &self,
        key: &PublicKey,
        generation: u64,
        list: flottille_link::SessionList,
    ) -> Result<Value, String> {
        let (members, _) = self.listed()?;
        let member = members
            .by_machine(key)
            .ok_or_else(|| "this machine is no member's".to_owned())?;
        if member.role != Role::Runner {
            return Err(format!(
                "{} is a {}: only a runner lists sessions",
                member.name,
                member.role.word()
            ));
        }
        list.check()?;
        // A link replaced since says nothing of the machine any more.
        if !self.on_link(key, generation) {
            return Ok(json!({}));
        }
        let daemon = self
            .daemon()
            .ok_or_else(|| "the daemon is stopping".to_owned())?;
        let machine = hex_of(key);
        let list = serde_json::to_value(&list).map_err(|error| error.to_string())?;
        daemon
            .write(|store| {
                store
                    .keep_machine_sessions(&machine, &member.name, &list, now())
                    .map(|()| ((), None))
            })
            .map_err(|error| error.to_string())?;
        (self.emit)("remote.sessions", json!({"name": member.name}));
        Ok(json!({}))
    }

    /// The projects the machine of key `key` lists, said over its link: kept beside its sessions
    /// under the same conditions as them (`took_sessions`).
    fn took_projects(
        &self,
        key: &PublicKey,
        generation: u64,
        list: flottille_link::ProjectList,
    ) -> Result<Value, String> {
        let (members, _) = self.listed()?;
        let member = members
            .by_machine(key)
            .ok_or_else(|| "this machine is no member's".to_owned())?;
        if member.role != Role::Runner {
            return Err(format!(
                "{} is a {}: only a runner lists projects",
                member.name,
                member.role.word()
            ));
        }
        list.check()?;
        if !self.on_link(key, generation) {
            return Ok(json!({}));
        }
        let daemon = self
            .daemon()
            .ok_or_else(|| "the daemon is stopping".to_owned())?;
        let machine = hex_of(key);
        let projects = serde_json::to_value(&list.projects).map_err(|error| error.to_string())?;
        daemon
            .write(|store| {
                store
                    .keep_machine_projects(&machine, &member.name, &projects, now())
                    .map(|()| ((), None))
            })
            .map_err(|error| error.to_string())?;
        (self.emit)("remote.sessions", json!({"name": member.name}));
        Ok(json!({}))
    }

    /// The pilots of another machine of the led sessions the machine of key `key` lists, said over
    /// its link: kept beside its sessions under the same conditions as them (`took_sessions`).
    /// `remote.sessions` says each beside its session, never anywhere else.
    fn took_pilots(
        &self,
        key: &PublicKey,
        generation: u64,
        list: flottille_link::PilotList,
    ) -> Result<Value, String> {
        let (members, _) = self.listed()?;
        let member = members
            .by_machine(key)
            .ok_or_else(|| "this machine is no member's".to_owned())?;
        if member.role != Role::Runner {
            return Err(format!(
                "{} is a {}: only a runner lists pilots",
                member.name,
                member.role.word()
            ));
        }
        list.check()?;
        if !self.on_link(key, generation) {
            return Ok(json!({}));
        }
        let daemon = self
            .daemon()
            .ok_or_else(|| "the daemon is stopping".to_owned())?;
        let machine = hex_of(key);
        let pilots = serde_json::to_value(&list.pilots).map_err(|error| error.to_string())?;
        daemon
            .write(|store| {
                store
                    .keep_machine_part(&machine, &member.name, "pilots", &pilots, now())
                    .map(|()| ((), None))
            })
            .map_err(|error| error.to_string())?;
        (self.emit)("remote.sessions", json!({"name": member.name}));
        Ok(json!({}))
    }

    /// The user's close of a session of this machine, kept by the machine of `from` while this one
    /// was off (`docs/remote.md`, "One list of every machine"): checked (`kept_close`), then
    /// served as that device's `agent.stop`, once: a close sent again, or played by a member that
    /// saw it signed, is answered and does nothing, as is one of a session started or resumed
    /// after it was signed. A session ended already, or no longer here, is answered as closed.
    /// A device this machine's member list does not hold may be one it has not heard of yet: that
    /// refusal is for now.
    async fn closed(
        self: &Arc<Self>,
        from: &PublicKey,
        request: flottille_link::Request,
    ) -> Result<Value, crate::across::Refused> {
        use crate::across::Refused;
        let Some(daemon) = self.daemon() else {
            return Err(Refused::Now("the daemon is stopping".to_owned()));
        };
        let checking = (self.clone(), request.clone());
        let (member, agent) = tokio::task::spawn_blocking(move || {
            let (remote, request) = checking;
            let (members, name) = remote.listed().map_err(Refused::Now)?;
            if members.get(&request.signed.device).is_none() {
                return Err(Refused::Now(
                    "the close is not signed by a member this machine knows yet".to_owned(),
                ));
            }
            flottille_link::kept_close(&request, &name, &members, wall_now())
                .map_err(|error| Refused::Never(error.to_string()))
        })
        .await
        .map_err(|_| Refused::Now("the close could not be checked".to_owned()))??;
        let over = |daemon: &Daemon| {
            daemon
                .read(|store| store.agent(&agent))
                .ok()
                .flatten()
                .is_none_or(|found| found.state.is_final())
        };
        let signed_at = request.signed.at;
        let since = |daemon: &Daemon| {
            daemon
                .read(|store| {
                    let started = store
                        .agent(&agent)?
                        .is_none_or(|found| found.created_at > signed_at);
                    let resumed = store
                        .last_agent_events(&agent, "agent.resumed", 1)?
                        .first()
                        .is_some_and(|event| event.at > signed_at);
                    Ok(started || resumed)
                })
                .unwrap_or(false)
        };
        if over(&daemon)
            || since(&daemon)
            || !self.closes.lock().unwrap().serve(&request, wall_now())
        {
            return Ok(json!({}));
        }
        let from = self.name_of(from).unwrap_or_default();
        let accepted = flottille_link::Accepted {
            device: member,
            id: request.id.clone(),
            method: "agent.stop".to_owned(),
            params: json!({"agent": agent}),
        };
        let signed = serde_json::to_string(&request).unwrap_or_default();
        let mut serving = match self.start(accepted, signed, &from).await {
            Ok(serving) => serving,
            Err(refused) => {
                self.closes.lock().unwrap().release(&request);
                return Err(Refused::Never(
                    serde_json::to_value(&*refused)
                        .ok()
                        .and_then(|answer| answer["error"]["message"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| "the close could not be served".to_owned()),
                ));
            }
        };
        let line = serving.lines.next_line().await.ok().flatten();
        let answer: Value = line
            .as_deref()
            .and_then(|line| serde_json::from_str(line).ok())
            .unwrap_or_default();
        if answer["error"].is_null() && line.is_some() || over(&daemon) {
            return Ok(json!({}));
        }
        self.closes.lock().unwrap().release(&request);
        Err(Refused::Never(
            answer["error"]["message"]
                .as_str()
                .map_or_else(|| "the close was not answered".to_owned(), str::to_owned),
        ))
    }
}

impl Remote {
    /// Forgets the list of each machine that is no runner among the members any more (removed, or
    /// added again as another role): what it told before is no one's to show. Left as it is when
    /// the member list cannot be read now.
    fn unlist(&self, daemon: &Arc<Daemon>) {
        let Ok((members, _)) = self.listed() else {
            return;
        };
        let runners: Vec<String> = members
            .members()
            .iter()
            .filter(|member| member.role == Role::Runner)
            .filter_map(|member| member.machine.as_ref().map(hex_of))
            .collect();
        let _ = daemon.write(|store| {
            store
                .forget_machine_sessions_except(&runners)
                .map(|()| ((), None))
        });
    }

    /// The keys of the other machines the member list trusts; none while the list was never read
    /// whole (a broken file at the start), which is not a list of nobody.
    pub(crate) fn trusted_keys(&self) -> Option<Vec<PublicKey>> {
        let peers = self.peers();
        let whole = self.peers.lock().unwrap().whole;
        whole.then(|| peers.machines.into_iter().map(|peer| peer.key).collect())
    }

    /// The live root sessions each other machine of the user's last listed, without those whose
    /// close waits to be sent: each with its machine's key, as hex, and name. It reads the store.
    pub(crate) fn sessions_elsewhere(
        &self,
        daemon: &Daemon,
    ) -> Vec<(String, String, flottille_link::LightSession)> {
        let closing = crate::across::closing(daemon);
        let kept = daemon
            .read(|store| store.machine_sessions())
            .unwrap_or_default();
        let peers = self.peers().machines;
        kept.into_iter()
            .filter(|(machine, _, _, _)| peers.iter().any(|peer| hex_of(&peer.key) == *machine))
            .flat_map(|(machine, name, list, _)| {
                // The list kept may hold more than its sessions (its machine's projects).
                let sessions: Vec<flottille_link::LightSession> =
                    serde_json::from_value(list["sessions"].clone()).unwrap_or_default();
                sessions
                    .into_iter()
                    .filter(|session| !closing.contains(&(machine.clone(), session.id.clone())))
                    .map(|session| (machine.clone(), name.clone(), session))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// `remote.sessions`: this machine's live root sessions, and each other machine's as it last
    /// listed them, without those whose close waits to be sent. It reads the store: called on a
    /// blocking thread.
    fn lists(&self, daemon: &Arc<Daemon>) -> Result<Value, String> {
        self.unlist(daemon);
        let closing = crate::across::closing(daemon);
        let kept = daemon
            .read(|store| store.machine_sessions())
            .map_err(|error| error.to_string())?;
        let peers = self.peers().machines;
        let members: Vec<String> = self
            .listed()
            .map(|(members, _)| members.members().iter().map(|m| m.name.clone()).collect())
            .unwrap_or_default();
        let machines: Vec<Value> = kept
            .into_iter()
            // A machine no longer among the user's lists nothing.
            .filter(|(machine, _, _, _)| peers.iter().any(|peer| hex_of(&peer.key) == *machine))
            .map(|(machine, name, mut list, at)| {
                let pilots: Vec<flottille_link::LightPilot> =
                    serde_json::from_value(list["pilots"].clone()).unwrap_or_default();
                if let Some(sessions) = list["sessions"].as_array_mut() {
                    sessions.retain(|session| {
                        !closing.contains(&(
                            machine.clone(),
                            session["id"].as_str().unwrap_or_default().to_owned(),
                        ))
                    });
                    with_pilots(sessions, &pilots, &name, &members);
                }
                json!({
                    "name": name,
                    "sessions": list["sessions"],
                    "projects": list["projects"],
                    "at": at,
                })
            })
            .collect();
        let listed = sessions_of(daemon);
        let mut this = serde_json::to_value(&listed.sessions).map_err(|error| error.to_string())?;
        if let (Some(sessions), Ok((_, own))) = (this.as_array_mut(), self.listed()) {
            with_pilots(sessions, &pilots_of(daemon, &listed).pilots, &own, &members);
        }
        let projects = serde_json::to_value(projects_of(daemon).projects)
            .map_err(|error| error.to_string())?;
        Ok(json!({"this": this, "projects": projects, "machines": machines}))
    }

    /// `remote.close`: keeps the user's close of a session of another machine (`agent.stop`,
    /// signed by their device for that machine), stored before it is sent, then sent once that
    /// machine is linked, now or when it links again. It reads the peers and writes the store:
    /// called on a blocking thread.
    fn keep_close(
        &self,
        daemon: &Arc<Daemon>,
        request: flottille_link::Request,
    ) -> Result<Value, String> {
        let own = self.listed().ok().map(|(_, name)| name);
        if own.as_deref() == Some(request.machine.as_str()) {
            return Err(format!(
                "the close is for {}, this machine: only a close for another machine is kept",
                request.machine
            ));
        }
        if request.signed.method != "agent.stop" {
            return Err(format!(
                "only a close is kept for another machine: agent.stop, not {}",
                request.signed.method.chars().take(64).collect::<String>()
            ));
        }
        let params: Value = serde_json::from_str(&request.signed.params).unwrap_or_default();
        let agent = params["agent"]
            .as_str()
            .filter(|agent| {
                !agent.is_empty()
                    && agent.len() <= 64
                    && agent
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
            .ok_or_else(|| "the close names no session".to_owned())?
            .to_owned();
        let peer = self
            .peers()
            .machines
            .into_iter()
            .find(|peer| peer.name == request.machine)
            .ok_or_else(|| {
                format!(
                    "{} is no machine of the user's",
                    request.machine.chars().take(64).collect::<String>()
                )
            })?;
        crate::across::keep_close(daemon, &hex_of(&peer.key), &peer.name, &agent, request)?;
        (self.emit)("remote.sessions", json!({"name": peer.name}));
        Ok(json!({}))
    }
}

/// A key as the outbox and the copies of the lists name its machine.
pub(crate) fn hex_of(key: &PublicKey) -> String {
    key.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A client link: each request of the user's another machine carried, checked and served as one
/// of a local client's, its answer, then the events of a subscription, sent back on its stream.
async fn serve_clients(remote: Arc<Remote>, link: Link, from: String) {
    while let Some(received) = link.next().await {
        // A machine removed from the list since its link came in carries nothing more.
        if remote.trusted(&link.peer()).is_none() {
            received.answer(Err(REMOVED.to_owned()));
            link.close(REMOVED);
            break;
        }
        // A connection of a tunnel opened for this link's machine (`docs/remote.md`, "Tunnels").
        let received = match received.take_tunnel() {
            Ok((tunnel, stream)) => {
                tokio::spawn(crate::tunnel::connection(
                    remote.clone(),
                    link.peer(),
                    tunnel,
                    stream,
                ));
                continue;
            }
            Err(received) => *received,
        };
        let Message::Client(request) = received.message().clone() else {
            // Kept apart by the link's protocol name: refused before it gets here.
            received.answer(Err(
                "a machine's message is not taken on a client link".into()
            ));
            continue;
        };
        let (remote, from, link) = (remote.clone(), from.clone(), link.clone());
        tokio::spawn(async move {
            let mut answer = received.stream();
            let mut lists = remote.lists.subscribe();
            let gone = answer.gone();
            tokio::pin!(gone);
            let accepted = match remote.accept(&request) {
                Ok(accepted) => accepted,
                Err(refused) => {
                    let _ = answer.send(&message_bytes(&refused)).await;
                    answer.end().await;
                    return;
                }
            };
            // A tunnel is held open by its request, which its own module serves.
            if accepted.method.starts_with("tunnel.") {
                crate::tunnel::serve(&remote, &link, accepted, &mut answer, &from).await;
                answer.end().await;
                return;
            }
            match remote.start(accepted, request.to_string(), &from).await {
                Err(refused) => {
                    let _ = answer.send(&message_bytes(&refused)).await;
                }
                Ok(mut serving) => loop {
                    let line = tokio::select! {
                        line = serving.lines.next_line() => line,
                        _ = &mut gone => break,
                        // A device removed from the list is served no more of its watch.
                        changed = lists.changed(), if !serving.once => {
                            if changed.is_err() || !remote.still_member(&serving.device).await {
                                break;
                            }
                            continue;
                        }
                    };
                    let Ok(Some(line)) = line else { break };
                    if !answer.send(line.as_bytes()).await || serving.once {
                        break;
                    }
                },
            }
            answer.end().await;
        });
    }
}

/// A request served for another machine's client: what its connection writes, line by line.
struct Serving {
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>,
    /// Held while it is served: the connection ends when it is let go.
    _write: tokio::net::unix::OwnedWriteHalf,
    /// Its answer is the only line: anything but a watch that was answered.
    once: bool,
    /// The device a watch is served for: the log says when it ends, which is how a watch that
    /// outlived its client is seen.
    watch_of: Option<String>,
    /// The key of the device that signed it.
    device: PublicKey,
}

impl Drop for Serving {
    fn drop(&mut self) {
        if let Some(device) = &self.watch_of {
            eprintln!("flottille daemon: the watch asked by {device} ended");
        }
    }
}

fn message_bytes(message: &flottille_core::protocol::Message) -> Vec<u8> {
    serde_json::to_vec(message).expect("a message is JSON")
}

impl Remote {
    /// The machine link to `peer`; why there is none, otherwise.
    fn machine_link(&self, peer: &Peer) -> Result<Link, String> {
        let machine = &peer.name;
        let seen = self.seen.lock().unwrap();
        match seen.get(peer.key.as_bytes()) {
            Some(Seen {
                link: Some((_, link, _)),
                ..
            }) => Ok(link.clone()),
            Some(seen) if seen.state == PeerState::NeedsUpdate => Err(format!(
                "{machine} needs an update: {}",
                seen.reason.clone().unwrap_or_default()
            )),
            // With the hub down, why: links go through it.
            Some(seen) => Err(match self.hub_down() {
                Some(why) => format!(
                    "{machine} is offline since {}: {why}",
                    time_of_day(seen.since)
                ),
                None => format!("{machine} is offline since {}", time_of_day(seen.since)),
            }),
            None => Err(format!("{machine} is offline")),
        }
    }

    /// Sends `message` on `link`, a machine link this machine holds. When the two machines
    /// dialled each other at once and `link` is the one let go (`linked`), the message goes on
    /// the link kept, once, if it is known not to have been served: it was never written whole
    /// on `link`, or the other machine turned it away unserved. It waits `WAIT_KEPT` at most for
    /// that link, said then as a machine that cannot be reached now. One that may
    /// have been served (its link cut once it was written, by a machine of before) is not sent
    /// again, and its failure is said as any other.
    pub(crate) async fn send(&self, link: &Link, message: &Message) -> Result<Value, LinkError> {
        let failed = match link.send(message).await {
            Err(failed) if unserved_for_another(&failed) => failed,
            sent => return sent,
        };
        match self.kept_instead_of(link).await {
            Some(kept) => kept.send(message).await,
            None => Err(failed),
        }
    }

    /// The link to `link`'s machine kept instead of it: waited for `WAIT_KEPT` at most, since the
    /// other machine may have chosen before this one had both links.
    async fn kept_instead_of(&self, link: &Link) -> Option<Link> {
        let until = tokio::time::Instant::now() + WAIT_KEPT;
        loop {
            let relinked = self.relinked.notified();
            tokio::pin!(relinked);
            relinked.as_mut().enable();
            let kept = self
                .seen
                .lock()
                .unwrap()
                .get(link.peer().as_bytes())
                .and_then(|seen| seen.link.as_ref())
                .map(|(_, kept, _)| kept.clone())
                .filter(|kept| !kept.same(link));
            if kept.is_some() {
                return kept;
            }
            tokio::time::timeout_at(until, relinked).await.ok()?;
        }
    }

    /// The machine link to the machine named `machine`, and its key.
    pub(crate) fn link_to(&self, machine: &str) -> Result<(Link, PublicKey), String> {
        let peer = self
            .peers()
            .machines
            .into_iter()
            .find(|peer| peer.name == machine)
            .ok_or_else(|| {
                format!(
                    "no machine of the user's is named {}",
                    machine.chars().take(64).collect::<String>().escape_debug()
                )
            })?;
        Ok((self.machine_link(&peer)?, peer.key))
    }

    /// The role of the member whose machine is `key`, by the checked member list; none when it
    /// is no member's, or there is no list. Code travels from a runner's or a controller's
    /// machine: `flottille_link::runs_agents` says runner alone, so travel checks the role here.
    pub(crate) fn role_of(&self, key: &PublicKey) -> Option<flottille_link::Role> {
        let (members, _) = self.listed().ok()?;
        members.by_machine(key).map(|member| member.role)
    }

    /// The name of the machine of `key`, while this one trusts it.
    pub(crate) fn name_of(&self, key: &PublicKey) -> Option<String> {
        self.trusted(key).map(|peer| peer.name)
    }

    /// Emits an event of the daemon's (`docs/events.md`).
    pub(crate) fn emit(&self, name: &str, data: Value) {
        (self.emit)(name, data);
    }

    /// The daemon, unless it is stopping.
    pub(crate) fn daemon(&self) -> Option<Arc<Daemon>> {
        self.daemon.get().and_then(std::sync::Weak::upgrade)
    }

    /// What each machine linked to this one offers, by its name: only a runner's is kept.
    pub(crate) fn offers(&self) -> Vec<(String, Capabilities)> {
        let peers = self.peers().machines;
        let seen = self.seen.lock().unwrap();
        peers
            .into_iter()
            .filter_map(|peer| {
                let offers = seen.get(peer.key.as_bytes())?.offers.clone()?;
                Some((peer.name, offers))
            })
            .collect()
    }

    /// The checked member list this machine keeps, and its own member's name. It reads a file:
    /// not under a lock.
    pub(crate) fn members(&self) -> Result<(flottille_link::Members, String), String> {
        self.listed()
    }

    /// The checked member list this machine keeps, and its own member's name.
    pub(crate) fn listed(&self) -> Result<(flottille_link::Members, String), String> {
        let control = self
            .control
            .get()
            .ok_or_else(|| "the link is not up".to_owned())?;
        let (_, members) = control.checked()?;
        let name = members
            .by_machine(&self.endpoint.key())
            .map(|member| member.name.clone())
            .ok_or_else(|| "this machine is no member's".to_owned())?;
        Ok((members, name))
    }

    /// `request`, as a link or a client of this machine carried it from the machine `from`:
    /// checked (`accept`), then served on a connection of its own (`start`).
    async fn serve(
        &self,
        request: &Value,
        from: &str,
    ) -> Result<Serving, Box<flottille_core::protocol::Message>> {
        let accepted = self.accept(request)?;
        self.start(accepted, request.to_string(), from).await
    }

    /// `request` as a link or a client of this machine carried it, checked
    /// (`flottille_link::Requests`): the refusal, as an answer, otherwise.
    pub(crate) fn accept(
        &self,
        request: &Value,
    ) -> Result<flottille_link::Accepted, Box<flottille_core::protocol::Message>> {
        use flottille_core::protocol::Message as Answer;
        let id = request["id"].as_str().map(str::to_owned);
        let refused =
            |why: String| Answer::error(id.clone(), flottille_protocol::code::REFUSED, why);
        let (members, name) = self
            .listed()
            .map_err(|why| refused(format!("refused: {why}")))?;
        let text = request.to_string();
        let accepted = self
            .requests
            .lock()
            .unwrap()
            .check(&text, &name, &members, SystemTime::now())
            .map_err(|error| refused(error.to_string()))?;
        Ok(accepted)
    }

    /// A request `accept` took, served on a connection of its own as one of a local client's, the
    /// member that signed it named in the log and in the events its serving emits; `signed` is the
    /// request as it was signed, which a method that carries the user's consent further sends on.
    async fn start(
        &self,
        accepted: flottille_link::Accepted,
        signed: String,
        from: &str,
    ) -> Result<Serving, Box<flottille_core::protocol::Message>> {
        use flottille_core::protocol::Message as Answer;
        let id = Some(accepted.id.clone());
        let daemon = self
            .daemon
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| {
                Box::new(Answer::error(
                    id.clone(),
                    "unavailable",
                    "the daemon is stopping",
                ))
            })?;
        let unavailable = |error: std::io::Error| {
            Box::new(Answer::error(id.clone(), "unavailable", error.to_string()))
        };
        let (ours, theirs) = tokio::net::UnixStream::pair().map_err(unavailable)?;
        eprintln!(
            "flottille daemon: {} asked by {} from {from}",
            accepted.method, accepted.device.name
        );
        // The key of the member that signed it, its public half: never the device's own key.
        let flottille_link::Member { key: signer, .. } = accepted.device;
        tokio::spawn(crate::daemon::served_for(
            daemon,
            theirs,
            crate::daemon::Asked {
                device: accepted.device.name.clone(),
                key: signer,
                signed,
            },
        ));
        let (read, mut write) = ours.into_split();
        let once = accepted.method != "events.watch";
        let mut line = serde_json::to_vec(&flottille_core::protocol::Request {
            id: accepted.id,
            method: accepted.method,
            params: accepted.params,
        })
        .expect("a request is JSON");
        line.push(b'\n');
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        write.write_all(&line).await.map_err(unavailable)?;
        Ok(Serving {
            lines: tokio::io::BufReader::new(read).lines(),
            _write: write,
            once,
            watch_of: (!once).then(|| accepted.device.name.clone()),
            device: signer,
        })
    }

    /// Whether the device of `key` is a member of the list this machine checked: a list that
    /// cannot be checked says no.
    pub(crate) async fn still_member(self: &Arc<Self>, key: &PublicKey) -> bool {
        let (remote, key) = (self.clone(), *key);
        tokio::task::spawn_blocking(move || {
            remote
                .listed()
                .is_ok_and(|(members, _)| members.members().iter().any(|member| member.key == key))
        })
        .await
        .unwrap_or(false)
    }

    /// The client link to `peer`, dialled when there is none.
    async fn client_link(&self, peer: &Peer) -> Result<Link, LinkError> {
        let kept = self
            .seen
            .lock()
            .unwrap()
            .get(peer.key.as_bytes())
            .and_then(|seen| seen.client.clone());
        if let Some(link) = kept {
            return Ok(link);
        }
        let link = self
            .endpoint
            .connect(peer.key, Traffic::Client, &self.hello, &peer.name)
            .await?;
        let trusted = self.trusted(&peer.key).is_some();
        let mut seen = self.seen.lock().unwrap();
        // Judged again under the lock `trust_machines` cuts the links under, as a machine link is
        // (`linked`): a removal that came while this link was dialled would miss it.
        if !trusted || !self.trusted_kept(&peer.key) {
            link.close(REMOVED);
            return Err(LinkError::Closed(format!("{}: {REMOVED}", peer.name)));
        }
        // Kept even for a machine not seen yet, so that a removal that comes next cuts it.
        seen.entry(*peer.key.as_bytes())
            .or_insert_with(Seen::new)
            .client = Some(link.clone());
        Ok(link)
    }

    /// Forgets the client link to `peer`, one that failed: the next request dials again.
    fn drop_client(&self, peer: &Peer) {
        if let Some(seen) = self.seen.lock().unwrap().get_mut(peer.key.as_bytes())
            && let Some(link) = seen.client.take()
        {
            link.close("it failed");
        }
    }
}

impl Remote {
    /// A vault entry the user shared, as the machine `peer` sent it: taken only with the user's
    /// own request, which this machine checks itself as it checks any request of a device (a
    /// machine alone shares nothing): `vault.share` of that very entry with this machine, signed
    /// for the machine that sent it, within its time, and never twice.
    /// The user's request `request` a share from the machine `peer` carries, checked as any
    /// request of a device: signed by a member's device for the machine that sent it, within its
    /// time, never seen, its role allowing it; `method`, aimed at this machine; and from a
    /// runner's or a controller's machine. The sender's name, this machine's, and the request.
    fn share_checked(
        &self,
        peer: &PublicKey,
        request: &Value,
        method: &str,
        what: &str,
    ) -> Result<(String, String, flottille_link::Accepted), String> {
        let (members, name) = self.listed().map_err(|why| format!("refused: {why}"))?;
        let sender = members
            .by_machine(peer)
            .ok_or_else(|| "refused: this machine is no member's".to_owned())?;
        let from = sender.name.clone();
        // As code (`travel`): a viewer's machine sends nothing the user decided, whoever signed.
        if sender.role == Role::Viewer {
            return Err(format!(
                "refused: {from} is a viewer's machine: only a runner's or a controller's \
                 machine shares {what}"
            ));
        }
        let accepted = self
            .requests
            .lock()
            .unwrap()
            .check(&request.to_string(), &from, &members, SystemTime::now())
            .map_err(|error| error.to_string())?;
        if accepted.method != method {
            return Err(format!(
                "refused: {} is not the user's share of {what}",
                accepted.method
            ));
        }
        if accepted.params["machine"].as_str() != Some(name.as_str()) {
            return Err(format!(
                "refused: the user shared it with {}, not with {name}",
                accepted.params["machine"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(64)
                    .collect::<String>()
                    .escape_debug()
            ));
        }
        Ok((from, name, accepted))
    }

    /// A connection the user shared, as the machine `peer` sent it: taken only with the user's
    /// own request, checked here as `shared` checks a vault entry's: `connection.share` of that
    /// very connection with this machine, and with these entries, signed for the machine that
    /// sent it, within its time, and never twice.
    async fn shared_connection(
        &self,
        peer: &PublicKey,
        share: ConnectionShare,
    ) -> Result<Value, String> {
        let (from, name, accepted) =
            self.share_checked(peer, &share.request, "connection.share", "a connection")?;
        let shown = |value: &Value| {
            value
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(64)
                .collect::<String>()
                .escape_debug()
                .to_string()
        };
        if accepted.params["name"] != share.connection["name"]
            || !share.connection["name"].is_string()
        {
            return Err(format!(
                "refused: the user shared {}, not {}",
                shown(&accepted.params["name"]),
                shown(&share.connection["name"])
            ));
        }
        let consented: Vec<&str> = accepted.params["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let sent: Vec<&str> = share
            .entries
            .iter()
            .map(|copy| copy.entry["name"].as_str().unwrap_or_default())
            .collect();
        if consented != sent {
            return Err(
                "refused: the vault entries sent are not the ones the user shared".to_owned(),
            );
        }
        let daemon = self
            .daemon
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| "the daemon is stopping".to_owned())?;
        crate::extensions::receive_connection(
            &daemon,
            &share.connection,
            &share.entries,
            &from,
            &name,
        )
        .await?;
        Ok(json!({"name": share.connection["name"]}))
    }

    async fn shared(&self, peer: &PublicKey, share: Share) -> Result<Value, String> {
        let (from, _, accepted) =
            self.share_checked(peer, &share.request, "vault.share", "a vault entry")?;
        let shown = |value: &Value| {
            value
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(64)
                .collect::<String>()
                .escape_debug()
                .to_string()
        };
        if accepted.params["name"] != share.entry["name"] || !share.entry["name"].is_string() {
            return Err(format!(
                "refused: the user shared {}, not {}",
                shown(&accepted.params["name"]),
                shown(&share.entry["name"])
            ));
        }
        let daemon = self
            .daemon
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| "the daemon is stopping".to_owned())?;
        crate::vault::receive(&daemon, &share.entry, &share.secret.0, &from).await?;
        Ok(json!({"name": share.entry["name"]}))
    }
}

/// Sends the vault entry the user shared, `share`, to their machine `machine`, a runner, which
/// checks the user's signature it carries and keeps it: what it answered, or why it is refused.
pub(crate) async fn share(daemon: &Daemon, machine: &str, share: Share) -> Result<(), String> {
    let (remote, link) = runner_link(daemon, machine, "a vault entry")?;
    remote
        .send(&link, &Message::Machine(MachineMessage::Share(share)))
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The machine link to the user's runner `machine`, which `what` is shared with.
fn runner_link(daemon: &Daemon, machine: &str, what: &str) -> Result<(Arc<Remote>, Link), String> {
    let remote = remote_of(daemon).map_err(|failure| failure.message)?;
    let (members, _) = remote.listed()?;
    let Some(peer) = remote
        .peers()
        .machines
        .into_iter()
        .find(|peer| peer.name == machine)
    else {
        return Err(format!(
            "no machine of the user's is named {}",
            machine.chars().take(64).collect::<String>().escape_debug()
        ));
    };
    if members
        .by_machine(&peer.key)
        .is_none_or(|member| member.role != flottille_link::Role::Runner)
    {
        return Err(format!(
            "{machine} runs no agent: {what} is shared with a runner"
        ));
    }
    let link = remote.machine_link(&peer)?;
    Ok((remote, link))
}

/// Sends the connection the user shared, `share`, to their machine `machine`, a runner, which
/// checks the user's signature it carries and declares it: why it is refused, otherwise nothing.
pub(crate) async fn share_connection(
    daemon: &Daemon,
    machine: &str,
    share: ConnectionShare,
) -> Result<(), String> {
    let (remote, link) = runner_link(daemon, machine, "a connection")?;
    remote
        .send(
            &link,
            &Message::Machine(MachineMessage::ShareConnection(share)),
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// `since`, milliseconds since the Unix epoch, as the time of day it was here (`22:14`).
fn time_of_day(since: u64) -> String {
    jiff::Timestamp::from_millisecond(since as i64)
        .map(|at| {
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// A request a client of this machine signed for the machine it names (`docs/remote.md`,
/// "Requests from another machine's clients"), written on `out` as its answer then, for a watch,
/// the events that came back. Carried as it is: this daemon holds no device key, and the
/// machine it is for checks it. One for this machine is checked and served here the same way.
/// What the user sends to a machine that is off is refused, never kept. `true` when a watch the
/// other machine answered ended with its link: the connection is to end, so that its client
/// resumes it after the last event it saw.
pub(crate) async fn forward(
    daemon: Arc<Daemon>,
    request: Value,
    out: tokio::sync::mpsc::Sender<flottille_core::protocol::Message>,
) -> bool {
    use flottille_core::protocol::Message as Answer;
    let id = request["id"].as_str().map(str::to_owned);
    let error = |code: &str, why: String| Answer::error(id.clone(), code, why);
    let send = |answer: Answer| {
        let out = out.clone();
        async move { out.send(answer).await.is_ok() }
    };
    let Some(machine) = request["machine"].as_str().map(str::to_owned) else {
        send(error(
            flottille_protocol::code::INVALID_REQUEST,
            "`machine` is the name of one of the user's machines".to_owned(),
        ))
        .await;
        return false;
    };
    let watch = request["signed"]["method"] == "events.watch";
    let remote = match remote_of(&daemon) {
        Ok(remote) => remote,
        Err(failure) => {
            send(failure.answer(id.clone())).await;
            return false;
        }
    };
    if remote.listed().is_ok_and(|(_, name)| name == machine) {
        // This machine, by its name: checked as another machine would check it.
        let mut serving = match remote.serve(&request, &machine).await {
            Ok(serving) => serving,
            Err(refused) => {
                send(*refused).await;
                return false;
            }
        };
        while let Ok(Some(line)) = serving.lines.next_line().await {
            let Ok(message) = serde_json::from_str::<Answer>(&line) else {
                continue;
            };
            if !send(message).await || serving.once {
                break;
            }
        }
        return false;
    }
    // A launch aimed at a machine that runs no agent is refused here, as there.
    if let Ok((members, _)) = remote.listed()
        && let Err(why) = flottille_link::runs_agents(
            &members,
            &machine,
            request["signed"]["method"].as_str().unwrap_or_default(),
        )
    {
        send(error(flottille_protocol::code::REFUSED, why.to_string())).await;
        return false;
    }
    let Some(peer) = remote
        .peers()
        .machines
        .into_iter()
        .find(|peer| peer.name == machine)
    else {
        send(error(
            flottille_protocol::code::NOT_FOUND,
            format!(
                "no machine of the user's is named {}",
                machine.chars().take(64).collect::<String>().escape_debug()
            ),
        ))
        .await;
        return false;
    };
    if let Err(offline) = remote.machine_link(&peer) {
        send(error("unavailable", offline)).await;
        return false;
    }
    let message = Message::Client(request.clone());
    // A client link kept may be gone unnoticed: dialled again once.
    let mut tries = 0;
    let (mut answers, link) = loop {
        tries += 1;
        let asked = match remote.client_link(&peer).await {
            Ok(link) => link
                .ask(&message)
                .await
                .map(|answers| (answers, link))
                .inspect_err(|_| remote.drop_client(&peer)),
            Err(error) => Err(error),
        };
        match asked {
            Ok(asked) => break asked,
            Err(_) if tries < 2 => continue,
            Err(why) => {
                send(error("unavailable", remote.said(&why))).await;
                return false;
            }
        }
    };
    // A tunnel stays open after its answer, and listens here: its own module carries it.
    if request["signed"]["method"] == "tunnel.open" {
        crate::tunnel::carry(&remote, &request, &machine, link, answers, &out).await;
        return false;
    }
    let mut answered = false;
    loop {
        match answers.next().await {
            Some(Ok(value)) => {
                let Ok(message) = serde_json::from_value::<Answer>(value) else {
                    continue;
                };
                // Only this request's answer, or a watch's events: nothing under another id.
                let ours = match &message.id {
                    Some(given) => Some(given) == id.as_ref() && !answered,
                    None => answered && watch && message.event.is_some(),
                };
                if !ours {
                    continue;
                }
                let failed = message.error.is_some();
                answered = true;
                if !send(message).await || failed || !watch {
                    return false;
                }
            }
            Some(Err(LinkError::Declined { reason, .. })) => {
                send(error(flottille_protocol::code::REFUSED, reason)).await;
                return false;
            }
            Some(Err(why)) if !answered => {
                send(error("unavailable", why.to_string())).await;
                return false;
            }
            Some(Err(_)) | None if answered => return true,
            Some(Err(_)) | None => {
                send(error(
                    "unavailable",
                    format!("the link to {machine} ended before it answered"),
                ))
                .await;
                return false;
            }
        }
    }
}

/// Every round, and whenever the hub says another list of the members online: dials each machine
/// trusted and not linked whose turn has come, and lets go of the links of those gone from it.
async fn dial(remote: Arc<Remote>, mut stopping: tokio::sync::watch::Receiver<bool>) {
    let mut round = tokio::time::interval(ROUND);
    loop {
        tokio::select! {
            _ = round.tick() => {}
            // A machine that stops and starts again is off the hub for less than a round.
            _ = remote.endpoint.online_changed() => {}
            _ = stopping.wait_for(|stopping| *stopping) => return,
        }
        if remote.holding() {
            continue;
        }
        for line in remote.refused_lines(None, Instant::now()) {
            eprintln!("flottille daemon: {line}");
        }
        let peers = remote.peers().machines;
        // Every link goes through the hub: a machine it no longer says online has none, though the
        // close it sent may have been lost with its session. Its link is let go now, not when it
        // times out, so that the link it makes again is not refused for the dead one.
        let online = remote.endpoint.online();
        let mut due = Vec::new();
        {
            let mut seen = remote.seen.lock().unwrap();
            for (key, seen) in seen.iter_mut() {
                if online.iter().any(|member| member.as_bytes() == key) {
                    continue;
                }
                if let Some((_, link, _)) = &seen.link {
                    link.close(GONE_FROM_HUB);
                }
                if let Some(client) = seen.client.take() {
                    client.close(GONE_FROM_HUB);
                }
            }
            // A machine no longer trusted is let go, once its link ended and said so.
            seen.retain(|key, seen| {
                let trusted = peers.iter().any(|peer| peer.key.as_bytes() == key);
                if !trusted && let Some((_, link, _)) = &seen.link {
                    link.close(NOT_TRUSTED);
                    return true;
                }
                trusted
            });
            for peer in &peers {
                let seen = seen.entry(*peer.key.as_bytes()).or_insert_with(Seen::new);
                if seen.link.is_none() && !seen.dialling && seen.next_try <= Instant::now() {
                    seen.dialling = true;
                    due.push(peer.clone());
                }
            }
        }
        for peer in due {
            tokio::spawn(dial_one(remote.clone(), peer));
        }
    }
}

async fn dial_one(remote: Arc<Remote>, peer: Peer) {
    let key = *peer.key.as_bytes();
    let made = remote
        .endpoint
        .connect(peer.key, Traffic::Machine, &remote.hello, &peer.name)
        .await;
    match made {
        Ok(link) => {
            if let Some(seen) = remote.seen.lock().unwrap().get_mut(&key) {
                seen.dialling = false;
            }
            remote.linked(link, peer.name, true);
        }
        Err(error) => remote.failed(key, &peer.name, &error),
    }
}

/// What a JSON file of the key folder holds: nothing when it is missing; why, when it cannot be
/// read or is not what it should be.
fn read_json<T: Default + for<'de> Deserialize<'de>>(file: &Path) -> Result<T, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(error.to_string()),
    };
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// Writes a JSON file of the key folder whole: a temporary file, the user's only, renamed into
/// place.
pub(crate) fn write_json(file: &Path, value: &impl Serialize) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = file.with_extension("json.new");
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    out.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    out.sync_all()?;
    std::fs::rename(&temporary, file)
}

/// A file's inode, size and time it was modified.
type Stamp = (u64, u64, Option<SystemTime>);

/// What says a file changed: its inode, size and time it was modified; `None` when it is missing.
fn stamp(file: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(file).ok()?;
    Some((metadata.ino(), metadata.len(), metadata.modified().ok()))
}

/// The link's methods (`docs/protocol.md`, "Remote").
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    // Said of this machine, on or off: before a link is needed.
    if matches!(method, "remote.settings" | "remote.configure") {
        return crate::remote_config::call(daemon, method, params).await;
    }
    // Read from this machine's store alone: a ghost opens while its machine, or the link, is off.
    if method == "remote.ghost" {
        let params = if params.is_null() { json!({}) } else { params };
        return tokio::task::spawn_blocking(move || crate::ghost::read(&daemon, params))
            .await
            .map_err(|_| {
                Failure::new(
                    flottille_protocol::code::UNAVAILABLE,
                    "the ghost could not be read",
                )
            })?;
    }
    let remote = remote_of(&daemon)?;
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "remote.machines" => {
            let project = params["project"].as_str().map(str::to_owned);
            let agent = params["agent"].as_str().map(str::to_owned);
            tokio::task::spawn_blocking(move || {
                remote.machines(project.as_deref(), agent.as_deref())
            })
            .await
            .map_err(|_| {
                Failure::new(
                    flottille_protocol::code::UNAVAILABLE,
                    "the machines could not be read",
                )
            })
        }
        "remote.launches" => crate::across::launched(&daemon)
            .map_err(|why| Failure::new(flottille_protocol::code::UNAVAILABLE, why)),
        "remote.sessions" => {
            let (reading, daemon) = (remote.clone(), daemon.clone());
            tokio::task::spawn_blocking(move || reading.lists(&daemon))
                .await
                .map_err(|_| {
                    Failure::new(
                        flottille_protocol::code::UNAVAILABLE,
                        "the lists could not be read",
                    )
                })?
                .map_err(|why| Failure::new(flottille_protocol::code::UNAVAILABLE, why))
        }
        "remote.close" => {
            let request: flottille_link::Request = param(&params, "request")?;
            let keeping = (remote.clone(), daemon.clone());
            tokio::task::spawn_blocking(move || {
                let (remote, daemon) = keeping;
                remote.keep_close(&daemon, request)
            })
            .await
            .map_err(|_| {
                Failure::new(
                    flottille_protocol::code::UNAVAILABLE,
                    "the close could not be kept",
                )
            })?
            .map_err(Failure::refused)
        }
        "remote.members" => control_of(&remote)?.members().map_err(Failure::refused),
        "remote.change" => {
            let change: flottille_link::Change = param(&params, "change")?;
            control_of(&remote)?
                .change(change)
                .await
                .map_err(Failure::refused)
        }
        "remote.pair" => {
            let code: String = param(&params, "code")?;
            let proposed: flottille_link::Proposed = param(&params, "proposed")?;
            let replace = params["replace"] == json!(true);
            control_of(&remote)?
                .pair(&code, proposed, replace)
                .await
                .map_err(Failure::refused)
        }
        "remote.declare" => {
            let change: flottille_link::Change = param(&params, "change")?;
            control_of(&remote)?
                .declare(change)
                .await
                .map_err(Failure::refused)
        }
        _ => Err(Failure::new(
            flottille_protocol::code::UNKNOWN_METHOD,
            format!("unknown method `{method}`"),
        )),
    }
}

/// The parameter `name` of a method, as `T`.
fn param<T: for<'de> Deserialize<'de>>(params: &Value, name: &str) -> Result<T, Failure> {
    serde_json::from_value(params.get(name).cloned().unwrap_or(Value::Null)).map_err(|error| {
        Failure::new(
            flottille_protocol::code::INVALID_PARAMS,
            format!("{name}: {error}"),
        )
    })
}

fn control_of(remote: &Remote) -> Result<Arc<crate::hub_session::Control>, Failure> {
    remote
        .control
        .get()
        .cloned()
        .ok_or_else(|| Failure::new(flottille_protocol::code::UNAVAILABLE, "the link is not up"))
}

pub(crate) fn remote_of(daemon: &Daemon) -> Result<Arc<Remote>, Failure> {
    match daemon.link() {
        None => Err(Failure::refused(crate::remote_config::OFF)),
        Some(Err(why)) => Err(Failure::new(
            flottille_protocol::code::UNAVAILABLE,
            format!("the link is not up: {why}"),
        )),
        Some(Ok(remote)) => Ok(remote.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_reads_a_peers_file_the_hub_will_write() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(PEERS_FILE);
        let key = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
        std::fs::write(
            &file,
            format!(r#"{{"machines": [{{"name": "server", "key": "{key}"}}]}}"#),
        )
        .unwrap();
        let peers: Peers = read_json(&file).unwrap();
        assert_eq!(peers.machines.len(), 1);
        assert_eq!(peers.machines[0].name, "server");

        // Written back whole, the user's only.
        write_json(&file, &peers).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let again: Peers = read_json(&file).unwrap();
        assert_eq!(again.machines[0].key, peers.machines[0].key);
    }

    #[test]
    fn the_link_reads_nothing_from_a_file_that_is_not_its() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(PEERS_FILE);
        assert!(read_json::<Peers>(&file).unwrap().machines.is_empty());
        std::fs::write(&file, "not json").unwrap();
        assert!(read_json::<Peers>(&file).is_err());
        // A key that is not one is refused with the whole file.
        std::fs::write(&file, r#"{"machines": [{"name": "server", "key": "00"}]}"#).unwrap();
        assert!(read_json::<Peers>(&file).is_err());
    }

    #[test]
    fn the_direct_link_s_addresses_are_taken_out_of_the_key_folder() {
        let dir = tempfile::tempdir().unwrap();
        let key = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
        let (declared, peers) = (dir.path().join(ADDRESSES_FILE), dir.path().join(PEERS_FILE));
        std::fs::write(&declared, r#"{"addresses": ["192.0.2.7"]}"#).unwrap();
        std::fs::write(
            &peers,
            format!(
                r#"{{"machines": [{{"name": "server", "key": "{key}",
                    "addresses": ["192.0.2.8:7447"], "told": ["192.0.2.9:7447"]}}]}}"#
            ),
        )
        .unwrap();
        migrate(dir.path());
        assert!(!declared.exists());
        let kept: Value = read_json::<Option<Value>>(&peers).unwrap().unwrap();
        assert_eq!(kept, json!({"machines": [{"name": "server", "key": key}]}));

        // A folder with none is left as it is, byte for byte.
        let before = std::fs::read(&peers).unwrap();
        let stamped = stamp(&peers);
        migrate(dir.path());
        assert_eq!(std::fs::read(&peers).unwrap(), before);
        assert_eq!(stamp(&peers), stamped);
    }

    /// A daemon's side of the link over a key folder of a test's, linked to nothing; and the keys
    /// of `peers` machines it trusts.
    fn remote_in(folder: &Path, peers: usize) -> (Remote, Vec<PublicKey>) {
        let key = MachineKey::load_or_make(&folder.join("key")).unwrap();
        let keys: Vec<PublicKey> = (0..peers)
            .map(|at| {
                MachineKey::load_or_make(&folder.join(format!("peer-{at}")))
                    .unwrap()
                    .public()
            })
            .collect();
        let peers = Peers {
            machines: keys
                .iter()
                .enumerate()
                .map(|(at, key)| Peer {
                    name: format!("machine-{at}"),
                    key: *key,
                })
                .collect(),
        };
        write_json(&folder.join(PEERS_FILE), &peers).unwrap();
        let endpoint = Endpoint::new(&key).unwrap();
        (
            Remote::new(endpoint, folder.to_owned(), Arc::new(|_: &str, _| {})),
            keys,
        )
    }

    #[test]
    fn a_machine_removed_stays_removed_when_the_peers_file_cannot_be_written() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (remote, keys) = remote_in(dir.path(), 2);
        remote.peers();
        assert!(remote.trusted(&keys[1]).is_some());

        // A folder that cannot be written (full or read-only disk): the file keeps the old list.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        remote.trust_machines(vec![("machine-0".to_owned(), keys[0])]);
        let kept: Vec<PublicKey> = remote.peers().machines.iter().map(|p| p.key).collect();
        let trusted = remote.trusted(&keys[1]).is_some();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(
            kept,
            vec![keys[0]],
            "the machine removed is back by the file"
        );
        assert!(!trusted, "the machine removed is trusted again");
    }

    /// Whether `remote` would let a link go on within `within`.
    async fn released(remote: &Remote, within: Duration) -> bool {
        tokio::time::timeout(within, remote.hub_heard())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn after_the_hub_session_is_lost_links_wait_for_the_next_list() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, _) = remote_in(dir.path(), 1);
        let remote = Arc::new(remote);
        remote.heard_hub();
        assert!(released(&remote, Duration::from_millis(100)).await);

        // The session is lost, as on a machine that wakes from a suspend: no link is accepted or
        // dialled by the list kept until the hub sends one.
        remote.lost_hub();
        assert!(remote.holding());
        assert!(!released(&remote, Duration::from_millis(500)).await);
        let waiting = tokio::spawn({
            let remote = remote.clone();
            async move { remote.hub_heard().await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiting.is_finished());
        remote.heard_hub();
        tokio::time::timeout(Duration::from_millis(500), waiting)
            .await
            .expect("the list releases the links at once")
            .unwrap();
        assert!(!remote.holding());
    }

    #[tokio::test]
    async fn a_sleep_between_two_rounds_is_taken_where_the_clocks_are_told() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, _) = remote_in(dir.path(), 1);
        let (wall, steady) = (std::time::SystemTime::now(), std::time::Instant::now());
        let at = |seconds: u64, ran: u64| {
            crate::hub_session::Awake::new(
                wall + Duration::from_secs(seconds),
                steady + Duration::from_secs(ran),
            )
        };
        remote.heard_hub();
        remote.awake_at(at(0, 0));
        // Rounds a second apart: the hub's list is trusted.
        remote.awake_at(at(1, 1));
        assert!(!remote.holding());

        // The clocks told after a suspend, at a round or at a session's start: the stored pair is
        // not overwritten before the sleep is judged, so the links wait for the next list.
        remote.awake_at(at(3601, 2));
        assert!(remote.holding());
    }

    #[tokio::test]
    async fn links_are_released_when_the_hub_sends_no_list_within_the_bound() {
        let dir = tempfile::tempdir().unwrap();
        let (mut remote, _) = remote_in(dir.path(), 1);
        remote.first_list = Duration::from_secs(3);
        // As at start, then after a session lost: held until the bound, then released with no
        // list. The hold starts here, not when the remote was made, so that the wait for it is
        // not what a loaded machine took to make it.
        for _ in 0..2 {
            remote.lost_hub();
            assert!(remote.holding());
            assert!(!released(&remote, Duration::from_millis(100)).await);
            assert!(released(&remote, Duration::from_secs(20)).await);
            assert!(!remote.holding());
        }
    }

    #[tokio::test]
    async fn an_unknown_key_is_refused_at_once_during_a_hold() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, _) = remote_in(dir.path(), 1);
        let key = MachineKey::load_or_make(&dir.path().join("key")).unwrap();
        let stranger = MachineKey::load_or_make(&dir.path().join("stranger")).unwrap();
        let trusted = MachineKey::load_or_make(&dir.path().join("peer-0")).unwrap();
        // Joined as the hub joins its members' sessions, the stranger with them, as a hub that
        // carries a stranger's packets would.
        let ports =
            flottille_link::relays_in_memory(&[key.public(), stranger.public(), trusted.public()]);
        let endpoints: Vec<Endpoint> = [&key, &stranger, &trusted]
            .into_iter()
            .zip(ports)
            .map(|(key, port)| {
                let endpoint = Endpoint::new(key).unwrap();
                endpoint.through(port);
                endpoint
            })
            .collect();
        let mut remote = Remote {
            endpoint: endpoints[0].clone(),
            ..remote
        };
        // A hold far longer than any of the waits below: a link that waits for it is told apart
        // from one refused at once, however loaded the machine is.
        remote.first_list = Duration::from_secs(120);
        let remote = Arc::new(remote);
        remote.peers();
        remote.lost_hub();
        let public = key.public();
        tokio::spawn(accept(remote.clone()));
        let dial = |endpoint: Endpoint| async move {
            endpoint
                .connect(public, Traffic::Machine, &hello(), "this")
                .await
        };

        // A key the list kept does not trust: refused now, not once the hold ends.
        let refused = tokio::time::timeout(Duration::from_secs(5), dial(endpoints[1].clone()))
            .await
            .expect("an unknown key waits for the hold");
        assert!(
            matches!(refused, Err(LinkError::Refused { .. })),
            "{:?}",
            refused.err()
        );

        // A key it trusts waits for the list; a second link of the same key meanwhile does not.
        let waiting = tokio::spawn(dial(endpoints[2].clone()));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!waiting.is_finished());
        let again = tokio::time::timeout(Duration::from_secs(5), dial(endpoints[2].clone()))
            .await
            .expect("a second link of a key that waits waits too");
        assert!(
            matches!(again, Err(LinkError::Refused { .. })),
            "{:?}",
            again.err()
        );
        remote.heard_hub();
        let linked = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the list releases the link waiting")
            .unwrap();
        assert!(linked.is_ok(), "{:?}", linked.err());
    }

    #[tokio::test]
    async fn a_flood_of_fresh_keys_is_said_within_a_budget_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, _) = remote_in(dir.path(), 0);
        let keys: Vec<PublicKey> = (0..40)
            .map(|at| {
                MachineKey::load_or_make(&dir.path().join(format!("stranger-{at}")))
                    .unwrap()
                    .public()
            })
            .collect();
        let start = Instant::now();
        let mut said = Vec::new();
        for _ in 0..3 {
            for key in &keys {
                said.extend(remote.refused_lines(Some(key), start));
                assert!(remote.refused.lock().unwrap().said.len() <= MOST_SAID);
            }
        }
        let refusals = said
            .iter()
            .filter(|line| line.starts_with("refused a link"))
            .count();
        assert_eq!(refusals, MOST_SAID, "{said:#?}");
        assert_eq!(said.len(), MOST_SAID + 1, "{said:#?}");
        assert!(said[MOST_SAID].contains("counted, not said"), "{said:#?}");

        // A minute on, the count is said, and the keys may be said again.
        let later = start + SAID_AGAIN;
        assert_eq!(
            remote.refused_lines(None, later),
            ["72 other links were refused in the last minute, from keys not said"]
        );
        assert!(remote.refused_lines(None, later).is_empty());
        assert_eq!(remote.refused_lines(Some(&keys[0]), later).len(), 1);
    }

    /// A link between two machines of a test's, through relay ports joined as the hub joins
    /// them: one end and the other, and the endpoints that hold them.
    async fn linked(folder: &Path) -> ((Endpoint, Endpoint), Link, Link) {
        let ours = MachineKey::load_or_make(&folder.join("ours")).unwrap();
        let theirs = MachineKey::load_or_make(&folder.join("theirs")).unwrap();
        let mut ports =
            flottille_link::relays_in_memory(&[ours.public(), theirs.public()]).into_iter();
        let (dialler, listening) = (
            Endpoint::new(&ours).unwrap(),
            Endpoint::new(&theirs).unwrap(),
        );
        dialler.through(ports.next().unwrap());
        listening.through(ports.next().unwrap());
        let accepting = listening.clone();
        let accepted = tokio::spawn(async move {
            let accepting = accepting.accept().await.unwrap();
            accepting.handshake(&hello(), "ours").await.unwrap()
        });
        let link = dialler
            .connect(theirs.public(), Traffic::Machine, &hello(), "theirs")
            .await
            .unwrap();
        let other = accepted.await.unwrap();
        ((dialler, listening), link, other)
    }

    fn app() -> Capabilities {
        Capabilities {
            projects: vec![Offered {
                name: "app".to_owned(),
                slots: 1,
            }],
            agents: Vec::new(),
        }
    }

    #[tokio::test]
    async fn an_offer_that_was_not_taken_is_sent_again() {
        let dir = tempfile::tempdir().unwrap();
        let (_ends, ours, theirs) = linked(dir.path()).await;
        let (_offered, receiver) = tokio::sync::watch::channel(Some(app()));
        tokio::spawn(offer(ours, receiver, MachineMessage::Capabilities, None));
        let is_offer = |message: &Message| matches!(message, Message::Machine(MachineMessage::Capabilities(offers)) if *offers == app());

        // Refused, as by a machine whose list does not say yet that this one is a runner.
        let first = tokio::time::timeout(Duration::from_secs(5), theirs.next())
            .await
            .unwrap()
            .unwrap();
        assert!(is_offer(first.message()));
        first.answer(Err("not a runner yet".to_owned()));
        let again = tokio::time::timeout(Duration::from_secs(10), theirs.next())
            .await
            .expect("the offer is sent again")
            .unwrap();
        assert!(is_offer(again.message()));
        again.answer(Ok(json!({})));
        // Taken: not sent again.
        assert!(
            tokio::time::timeout(Duration::from_secs(3), theirs.next())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn the_projects_a_machine_of_before_does_not_know_are_not_sent_again() {
        let dir = tempfile::tempdir().unwrap();
        let (_ends, ours, theirs) = linked(dir.path()).await;
        let list = flottille_link::ProjectList {
            projects: vec![flottille_link::LightProject {
                name: "app".to_owned(),
                remotes: vec!["github.com/acme/app".to_owned()],
            }],
        };
        let (offered, receiver) = tokio::sync::watch::channel(Some(list.clone()));
        tokio::spawn(offer(
            ours.clone(),
            receiver,
            MachineMessage::Projects,
            Some(("server".to_owned(), "this machine's projects")),
        ));
        let first = tokio::time::timeout(Duration::from_secs(5), theirs.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            first.message(),
            Message::Machine(MachineMessage::Projects(sent)) if *sent == list
        ));
        // As a machine of before answers a kind it does not know.
        first.answer(Err(flottille_link::UNKNOWN_MESSAGE.to_owned()));
        // Neither again at once nor after a change: that machine is told no more of them.
        offered.send_modify(|current| *current = Some(flottille_link::ProjectList::default()));
        assert!(
            tokio::time::timeout(Duration::from_secs(3), theirs.next())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_late_offer_of_a_link_replaced_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, keys) = remote_in(dir.path(), 1);
        let (_ends, link, _other) = linked(dir.path()).await;
        let key = keys[0];
        let kept = |remote: &Remote| remote.seen.lock().unwrap()[key.as_bytes()].offers.clone();
        remote
            .seen
            .lock()
            .unwrap()
            .entry(*key.as_bytes())
            .or_insert_with(Seen::new)
            .link = Some((2, link, true));

        // The link of generation 1 was replaced by the 2nd: what it said last is not kept.
        remote.keep_offers(&key, 1, app());
        assert_eq!(kept(&remote), None);
        remote.keep_offers(&key, 2, app());
        assert_eq!(kept(&remote), Some(app()));
    }

    #[tokio::test]
    async fn a_late_list_of_a_link_replaced_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, keys) = remote_in(dir.path(), 1);
        let (_ends, link, _other) = linked(dir.path()).await;
        let key = keys[0];
        remote
            .seen
            .lock()
            .unwrap()
            .entry(*key.as_bytes())
            .or_insert_with(Seen::new)
            .link = Some((2, link, true));

        // The link of generation 1 was replaced by the 2nd: its sessions are not the machine's.
        assert!(!remote.on_link(&key, 1));
        assert!(remote.on_link(&key, 2));
    }

    /// Two machines that dialled each other at once, as the daemon of the one with the higher key
    /// sees them: its `Remote`, holding the link it dialled (`ours`), and the link the other
    /// dialled (`kept`, not linked yet), with the other machine's end of each.
    struct Twice {
        remote: Arc<Remote>,
        _theirs: Endpoint,
        ours: Link,
        theirs_of_ours: Link,
        kept: Link,
        theirs_of_kept: Link,
    }

    async fn dialled_twice(folder: &Path) -> Twice {
        let one = MachineKey::load_or_make(&folder.join("one")).unwrap();
        let two = MachineKey::load_or_make(&folder.join("two")).unwrap();
        // This side holds the higher key: the link the other machine dialled is the one kept.
        let (ours, theirs) = if one.public().as_bytes() > two.public().as_bytes() {
            (one, two)
        } else {
            (two, one)
        };
        write_json(
            &folder.join(PEERS_FILE),
            &Peers {
                machines: vec![Peer {
                    name: "laptop".to_owned(),
                    key: theirs.public(),
                }],
            },
        )
        .unwrap();
        let mut ports =
            flottille_link::relays_in_memory(&[ours.public(), theirs.public()]).into_iter();
        let (here, there) = (
            Endpoint::new(&ours).unwrap(),
            Endpoint::new(&theirs).unwrap(),
        );
        here.through(ports.next().unwrap());
        there.through(ports.next().unwrap());
        let accepting = there.clone();
        let accepted = tokio::spawn(async move {
            let accepting = accepting.accept().await.unwrap();
            accepting.handshake(&hello(), "server").await.unwrap()
        });
        let ours_link = here
            .connect(theirs.public(), Traffic::Machine, &hello(), "laptop")
            .await
            .unwrap();
        let theirs_of_ours = accepted.await.unwrap();
        let accepting = here.clone();
        let accepted = tokio::spawn(async move {
            let accepting = accepting.accept().await.unwrap();
            accepting.handshake(&hello(), "laptop").await.unwrap()
        });
        let theirs_of_kept = there
            .connect(ours.public(), Traffic::Machine, &hello(), "server")
            .await
            .unwrap();
        let kept = accepted.await.unwrap();
        let remote = Arc::new(Remote::new(
            here,
            folder.to_owned(),
            Arc::new(|_: &str, _| {}),
        ));
        Twice {
            remote,
            _theirs: there,
            ours: ours_link,
            theirs_of_ours,
            kept,
            theirs_of_kept,
        }
    }

    /// What the other machine takes on `link`, each answered `{}`: counted.
    fn answering(link: Link) -> Arc<std::sync::atomic::AtomicUsize> {
        let taken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counting = taken.clone();
        tokio::spawn(async move {
            while let Some(received) = link.next().await {
                counting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                received.answer(Ok(json!({})));
            }
        });
        taken
    }

    fn asked() -> Message {
        Message::Machine(MachineMessage::Capabilities(app()))
    }

    #[tokio::test]
    async fn a_request_on_a_link_closed_for_another_goes_on_the_kept_one() {
        let dir = tempfile::tempdir().unwrap();
        let twice = dialled_twice(dir.path()).await;
        let on_ours = answering(twice.theirs_of_ours.clone());
        let on_kept = answering(twice.theirs_of_kept.clone());
        twice
            .remote
            .linked(twice.ours.clone(), "laptop".to_owned(), true);
        // The request took the link this machine dialled; the other machine's comes, and is kept.
        let taken = twice.ours.clone();
        twice
            .remote
            .linked(twice.kept.clone(), "laptop".to_owned(), false);

        let sent =
            tokio::time::timeout(Duration::from_secs(10), twice.remote.send(&taken, &asked()))
                .await
                .expect("the request is answered");
        assert!(sent.is_ok(), "{}", sent.unwrap_err());
        let count = |taken: &std::sync::atomic::AtomicUsize| {
            taken.load(std::sync::atomic::Ordering::SeqCst)
        };
        // Once, on the link kept.
        assert_eq!((count(&on_ours), count(&on_kept)), (0, 1));
    }

    #[tokio::test]
    async fn a_request_in_flight_when_its_link_is_closed_for_another_is_answered_on_it() {
        let dir = tempfile::tempdir().unwrap();
        let twice = dialled_twice(dir.path()).await;
        let on_kept = answering(twice.theirs_of_kept.clone());
        twice
            .remote
            .linked(twice.ours.clone(), "laptop".to_owned(), true);
        let (remote, taken) = (twice.remote.clone(), twice.ours.clone());
        let sending = tokio::spawn(async move { remote.send(&taken, &asked()).await });
        // The other machine has it, and is serving it, when the link it came on loses.
        let received = tokio::time::timeout(Duration::from_secs(5), twice.theirs_of_ours.next())
            .await
            .unwrap()
            .unwrap();
        twice
            .remote
            .linked(twice.kept.clone(), "laptop".to_owned(), false);
        received.answer(Ok(json!({"served": true})));

        let sent = tokio::time::timeout(Duration::from_secs(10), sending)
            .await
            .expect("the request is answered")
            .unwrap();
        assert_eq!(sent.map_err(|e| e.to_string()), Ok(json!({"served": true})));
        // Served where it came: never sent again (a resend would have come before the answer).
        assert_eq!(on_kept.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_request_the_other_machine_turned_away_as_linked_already_goes_on_the_kept_one() {
        let dir = tempfile::tempdir().unwrap();
        let twice = dialled_twice(dir.path()).await;
        twice
            .remote
            .linked(twice.ours.clone(), "laptop".to_owned(), true);
        // The other machine saw both links first, and lets this one go.
        twice.theirs_of_ours.retire(LINKED_ALREADY);
        let on_ours = answering(twice.theirs_of_ours.clone());
        let on_kept = answering(twice.theirs_of_kept.clone());
        let (remote, taken) = (twice.remote.clone(), twice.ours.clone());
        let sending = tokio::spawn(async move { remote.send(&taken, &asked()).await });
        // The link kept reaches this machine a moment later.
        tokio::time::sleep(Duration::from_millis(300)).await;
        twice
            .remote
            .linked(twice.kept.clone(), "laptop".to_owned(), false);

        let sent = tokio::time::timeout(Duration::from_secs(10), sending)
            .await
            .expect("the request is answered")
            .unwrap();
        assert!(sent.is_ok(), "{}", sent.unwrap_err());
        let count = |taken: &std::sync::atomic::AtomicUsize| {
            taken.load(std::sync::atomic::Ordering::SeqCst)
        };
        assert_eq!((count(&on_ours), count(&on_kept)), (0, 1));
    }

    #[tokio::test]
    async fn a_link_lost_for_good_still_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let twice = dialled_twice(dir.path()).await;
        let on_kept = answering(twice.theirs_of_kept.clone());
        twice
            .remote
            .linked(twice.ours.clone(), "laptop".to_owned(), true);
        let taken = twice.ours.clone();
        // The hub no longer says the other machine online: its link is let go, none is kept.
        {
            let mut seen = twice.remote.seen.lock().unwrap();
            for seen in seen.values_mut() {
                if let Some((_, link, _)) = seen.link.take() {
                    link.close(GONE_FROM_HUB);
                }
            }
        }

        let sent =
            tokio::time::timeout(Duration::from_secs(2), twice.remote.send(&taken, &asked()))
                .await
                .expect("the loss is said at once");
        let said = sent.expect_err("the request fails").to_string();
        assert_eq!(said, "the link to laptop was lost: closed");
        assert_eq!(on_kept.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// One side of two machines that dialled each other at once, each its own `Remote`: its end
    /// of the link it dialled, and of the one it accepted, neither linked yet.
    struct Side {
        remote: Arc<Remote>,
        _endpoint: Endpoint,
        dialled: Link,
        accepted: Link,
    }

    impl Side {
        /// Its end of the link the other side keeps too (the one the lower key dialled), and of
        /// the one both let go, each with whether this side dialled it.
        fn kept_and_lost(&self, lower: bool) -> ((Link, bool), (Link, bool)) {
            let (dialled, accepted) =
                ((self.dialled.clone(), true), (self.accepted.clone(), false));
            if lower {
                (dialled, accepted)
            } else {
                (accepted, dialled)
            }
        }
    }

    /// Two machines, each trusting the other as `other`, linked both ways: the lower key's first.
    async fn both_ways(folder: &Path) -> (Side, Side) {
        let one = MachineKey::load_or_make(&folder.join("one.key")).unwrap();
        let two = MachineKey::load_or_make(&folder.join("two.key")).unwrap();
        let (lower, higher) = if one.public().as_bytes() < two.public().as_bytes() {
            (one, two)
        } else {
            (two, one)
        };
        let mut ports =
            flottille_link::relays_in_memory(&[lower.public(), higher.public()]).into_iter();
        let ends = [&lower, &higher].map(|key| {
            let endpoint = Endpoint::new(key).unwrap();
            endpoint.through(ports.next().unwrap());
            endpoint
        });
        let dial = async |from: &Endpoint, to: &Endpoint, key: &MachineKey| {
            let accepting = to.clone();
            let accepted = tokio::spawn(async move {
                let accepting = accepting.accept().await.unwrap();
                accepting.handshake(&hello(), "other").await.unwrap()
            });
            let dialled = from
                .connect(key.public(), Traffic::Machine, &hello(), "other")
                .await
                .unwrap();
            (dialled, accepted.await.unwrap())
        };
        let (lower_dialled, higher_accepted) = dial(&ends[0], &ends[1], &higher).await;
        let (higher_dialled, lower_accepted) = dial(&ends[1], &ends[0], &lower).await;
        let side = |name: &str, other: &MachineKey, endpoint: &Endpoint, dialled, accepted| {
            let place = folder.join(name);
            std::fs::create_dir_all(&place).unwrap();
            write_json(
                &place.join(PEERS_FILE),
                &Peers {
                    machines: vec![Peer {
                        name: "other".to_owned(),
                        key: other.public(),
                    }],
                },
            )
            .unwrap();
            Side {
                remote: Arc::new(Remote::new(
                    endpoint.clone(),
                    place,
                    Arc::new(|_: &str, _| {}),
                )),
                _endpoint: endpoint.clone(),
                dialled,
                accepted,
            }
        };
        (
            side("lower", &higher, &ends[0], lower_dialled, lower_accepted),
            side("higher", &lower, &ends[1], higher_dialled, higher_accepted),
        )
    }

    /// A request one machine sends on the link both let go, which it alone holds yet, while the
    /// other holds both: answered, by the other's own serving, whatever the order the other took
    /// its two links in (`kept_first`) and whichever key is the lower.
    async fn a_request_on_the_link_let_go_is_answered(kept_first: bool) {
        for receiver_lower in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let (lower, higher) = both_ways(dir.path()).await;
            let (receiver, sender) = if receiver_lower {
                (lower, higher)
            } else {
                (higher, lower)
            };
            let (kept, lost) = receiver.kept_and_lost(receiver_lower);
            let order = if kept_first {
                [kept, lost]
            } else {
                [lost, kept]
            };
            for (link, dialled) in order {
                receiver.remote.linked(link, "other".to_owned(), dialled);
            }
            let ((kept, kept_dialled), (lost, lost_dialled)) =
                sender.kept_and_lost(!receiver_lower);
            sender
                .remote
                .linked(lost.clone(), "other".to_owned(), lost_dialled);
            let (remote, taken) = (sender.remote.clone(), lost.clone());
            let sending = tokio::spawn(async move {
                remote
                    .send(
                        &taken,
                        &Message::Machine(MachineMessage::Addresses(Vec::new())),
                    )
                    .await
            });
            // The link kept reaches the sender a moment later.
            tokio::time::sleep(Duration::from_millis(300)).await;
            sender.remote.linked(kept, "other".to_owned(), kept_dialled);

            let sent = tokio::time::timeout(Duration::from_secs(15), sending)
                .await
                .expect("the request is answered")
                .unwrap();
            assert_eq!(
                sent.map_err(|error| error.to_string()),
                Ok(json!({})),
                "kept first: {kept_first}, the receiver's key the lower: {receiver_lower}"
            );
        }
    }

    #[tokio::test]
    async fn a_request_on_a_link_that_lost_as_the_new_one_is_answered_on_the_kept_one() {
        a_request_on_the_link_let_go_is_answered(true).await;
    }

    #[tokio::test]
    async fn a_request_on_a_link_that_lost_as_the_old_one_is_answered_on_the_kept_one() {
        a_request_on_the_link_let_go_is_answered(false).await;
    }

    /// A daemon on temporary folders, with no remote of its own.
    fn a_daemon(dir: &Path) -> Arc<Daemon> {
        let paths = Paths {
            runtime: dir.join("runtime"),
            state: dir.join("state"),
            config: dir.join("config"),
            data: dir.join("data"),
        };
        std::fs::create_dir_all(&paths.config).unwrap();
        let store = flottille_core::store::Store::open_in_memory().unwrap();
        let projects = crate::projects::Projects::open(&paths).unwrap();
        Arc::new(Daemon::new(&paths, store, projects, 0, None))
    }

    #[tokio::test]
    async fn a_delivery_kept_for_a_machine_sent_on_the_link_it_let_go_goes_on_the_kept_one() {
        let dir = tempfile::tempdir().unwrap();
        let twice = dialled_twice(dir.path()).await;
        let daemon = a_daemon(&dir.path().join("daemon"));
        daemon.set_link(Some(Ok(twice.remote.clone())));
        let _ = twice.remote.daemon.set(Arc::downgrade(&daemon));
        let machine: String = twice
            .ours
            .peer()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let message = MachineMessage::Addresses(vec!["kept for laptop".to_owned()]);
        daemon
            .write(|store| {
                store
                    .keep_outgoing(&flottille_core::store::Outgoing {
                        seq: 0,
                        id: "delivery-1".to_owned(),
                        machine: machine.clone(),
                        name: "laptop".to_owned(),
                        launch: String::new(),
                        message: serde_json::to_value(&message).unwrap(),
                        back: false,
                        at: 0,
                    })
                    .map(|()| ((), None))
            })
            .unwrap();
        // The other machine saw both links first, let this one go, and serves it still.
        twice.theirs_of_ours.retire(LINKED_ALREADY);
        let on_ours = answering(twice.theirs_of_ours.clone());
        let on_kept = answering(twice.theirs_of_kept.clone());
        // Linked, the link sends what was kept for that machine: turned away there.
        twice
            .remote
            .linked(twice.ours.clone(), "laptop".to_owned(), true);
        tokio::time::sleep(Duration::from_millis(300)).await;
        twice
            .remote
            .linked(twice.kept.clone(), "laptop".to_owned(), false);

        let count = |taken: &std::sync::atomic::AtomicUsize| {
            taken.load(std::sync::atomic::Ordering::SeqCst)
        };
        let until = tokio::time::Instant::now() + Duration::from_secs(10);
        while count(&on_kept) == 0 && tokio::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!((count(&on_ours), count(&on_kept)), (0, 1));
        let kept = daemon.read(|store| store.outgoing(Some(&machine))).unwrap();
        assert_eq!(kept.len(), 0);
    }
}
