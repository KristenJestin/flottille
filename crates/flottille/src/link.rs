//! The daemon's link to the user's other machines (`docs/remote.md`, "Architecture", "The
//! transport", "The version"), with remote on: one endpoint, listening where `daemon.toml` says
//! (`[remote] listen`), nowhere when it says nothing; a link kept to every machine this one trusts, dialled at the addresses it
//! knows for it; each side telling the other its addresses whenever they change.
//!
//! Flottille handles no network channel: no relay, no NAT traversal, no discovery. How two machines
//! reach each other (a LAN, a VPN, a public address, a tunnel) is the user's; a machine dials what
//! it is told.
//!
//! The machines it trusts, and their addresses, are in `remote/peers.json`, which the hub will keep
//! (#820); until then a test writes it. The addresses declared by hand are in
//! `remote/addresses.json`. Both are in the key folder, which no agent reaches (`gate::keys`).

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flottille_core::paths::Paths;
use flottille_link::{
    Address, Endpoint, Hello, Link, LinkError, MachineKey, MachineMessage, Message, PeerState,
    PublicKey, Traffic,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::VERSION;
use crate::daemon::Daemon;
use crate::projects::Failure;

/// The machines this one trusts, and the addresses it knows for them.
const PEERS_FILE: &str = "peers.json";
/// The addresses of this machine declared by hand.
const ADDRESSES_FILE: &str = "addresses.json";
/// How often the daemon looks for a machine to dial, and for addresses of its own that changed.
const ROUND: Duration = Duration::from_secs(1);
/// The longest a machine that cannot be reached waits before it is dialled again.
const LONGEST_WAIT: Duration = Duration::from_secs(5);
/// The most addresses taken from another machine at once.
const MOST_ADDRESSES: usize = 64;
/// What a machine this one does not trust is told.
const NOT_TRUSTED: &str = "refused: this machine's key is not one of the user's";
/// What closes the second link two machines that dialled each other at once made.
const LINKED_ALREADY: &str = "linked already";
/// How long a key refused is not said again in the log.
const SAID_AGAIN: Duration = Duration::from_secs(60);
/// The most keys refused said in the log within `SAID_AGAIN`: a key costs a stranger nothing, so
/// each of its dials may bring a new one. The others are counted, and the count said.
const MOST_SAID: usize = 16;
/// How often the addresses one machine told are written, at most: the latest told is kept.
const LEARN_EVERY: Duration = Duration::from_secs(2);
/// What a client's request over a link is answered, until they are served (#813).
const NOT_SERVED: &str = "requests from another machine's clients are not served yet";

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
    /// The addresses it was given, by the hub or by hand: a machine never changes them.
    #[serde(default)]
    addresses: Vec<String>,
    /// The addresses the machine itself told, beside them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    told: Vec<String>,
}

impl Peer {
    /// Every address known for it: those it was given, then those it told.
    fn known(&self) -> Vec<String> {
        let mut known = self.addresses.clone();
        for address in &self.told {
            if !known.contains(address) {
                known.push(address.clone());
            }
        }
        known
    }
}

/// The peers' file as last read whole: one caught half written, or broken by hand, leaves it as
/// it was, and is said once.
struct Kept {
    last: Peers,
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

/// What `remote/addresses.json` holds.
#[derive(Default, Serialize, Deserialize)]
struct Declared {
    #[serde(default)]
    addresses: Vec<String>,
}

/// What this daemon sees of another machine.
struct Seen {
    state: PeerState,
    /// Since when, in milliseconds since the Unix epoch.
    since: u64,
    reason: Option<String>,
    /// The link, its generation, and whether this machine dialled it.
    link: Option<(u64, Link, bool)>,
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

/// The daemon's side of the link.
pub struct Remote {
    endpoint: Endpoint,
    folder: PathBuf,
    hello: Hello,
    peers: Mutex<Kept>,
    seen: Mutex<HashMap<[u8; 32], Seen>>,
    /// The addresses last told to the other machines.
    told: Mutex<Vec<String>>,
    links: std::sync::atomic::AtomicU64,
    /// The keys refused lately, as the log said them.
    refused: Mutex<Refused>,
    /// The machines whose addresses told are being written: the latest told, not written yet.
    learning: Mutex<HashMap<[u8; 32], Option<Vec<String>>>>,
    /// Held while the peers' file is read and written again: apart from `peers`, so that a link
    /// coming in is not judged after a write to the disk.
    writing: Mutex<()>,
}

/// Starts the link: the endpoint, the loop that accepts and the one that dials. A link that
/// cannot listen is said, and the daemon goes on without it.
pub fn start(
    paths: &Paths,
    key: MachineKey,
    stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<Arc<Remote>, String> {
    let endpoint = match crate::remote::listen(paths)? {
        Some(listen) => Endpoint::bind(&key, listen),
        None => Endpoint::dialler(&key),
    }
    .map_err(|error| error.to_string())?;
    match endpoint.listening() {
        Some(local) => eprintln!("flottille daemon: the link listens on {local}"),
        None => eprintln!(
            "flottille daemon: the link listens nowhere ([remote] listen in daemon.toml); it dials"
        ),
    }
    let remote = Arc::new(Remote::new(endpoint, crate::remote::folder(paths)));
    // Read before the first link comes in, which is judged by the file as last read.
    remote.peers();
    tokio::spawn(accept(remote.clone()));
    tokio::spawn(dial(remote.clone(), stopping));
    Ok(remote)
}

/// This build's hello; a test build may make it another's.
fn hello() -> Hello {
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
    fn new(endpoint: Endpoint, folder: PathBuf) -> Remote {
        Remote {
            endpoint,
            folder,
            hello: hello(),
            peers: Mutex::new(Kept {
                last: Peers::default(),
                said: false,
                read: None,
            }),
            seen: Mutex::new(HashMap::new()),
            told: Mutex::new(Vec::new()),
            links: Default::default(),
            refused: Mutex::new(Refused::new(Instant::now())),
            learning: Mutex::new(HashMap::new()),
            writing: Mutex::new(()),
        }
    }

    /// Closes every link, telling the other machines.
    pub async fn close(&self) {
        self.endpoint.close().await;
    }

    /// The machines this one trusts: the file as last read whole.
    fn peers(&self) -> Peers {
        let file = self.folder.join(PEERS_FILE);
        let mut kept = self.peers.lock().unwrap();
        kept.read = stamp(&file);
        match read_json::<Peers>(&file) {
            Ok(peers) => {
                // A machine trusted dials from one of its addresses, as often as not.
                self.endpoint.know(
                    peers
                        .machines
                        .iter()
                        .flat_map(Peer::known)
                        .filter_map(|text| Address::parse(&text).ok()?.ip()),
                );
                kept.said = false;
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

    fn declared(&self) -> Declared {
        read_json(&self.folder.join(ADDRESSES_FILE)).unwrap_or_default()
    }

    /// This machine's addresses: those it listens on, then those declared by hand, which take
    /// the link's port when they say none.
    fn addresses(&self) -> Vec<(String, &'static str)> {
        let port = self
            .endpoint
            .listening()
            .map_or(crate::remote::PORT, |local| local.port());
        let found = self
            .endpoint
            .addresses()
            .into_iter()
            .map(|address| (Address::from(address).to_string(), "found"));
        let declared = self
            .declared()
            .addresses
            .into_iter()
            .filter_map(|text| Address::parse(&text).ok())
            .map(|address| (address.with_port(port).to_string(), "declared"));
        let mut addresses: Vec<(String, &'static str)> = Vec::new();
        for (address, how) in found.chain(declared) {
            if !addresses.iter().any(|(known, _)| *known == address) {
                addresses.push((address, how));
            }
        }
        addresses
    }

    fn told_addresses(&self) -> Vec<String> {
        self.addresses()
            .into_iter()
            .map(|(address, _)| address)
            .collect()
    }

    /// Tells every machine linked to this one its addresses, when they changed since last told.
    fn tell_if_changed(&self) {
        let now = self.told_addresses();
        {
            let mut told = self.told.lock().unwrap();
            if *told == now {
                return;
            }
            told.clone_from(&now);
        }
        let links: Vec<Link> = self
            .seen
            .lock()
            .unwrap()
            .values()
            .filter_map(|seen| seen.link.as_ref().map(|(_, link, _)| link.clone()))
            .collect();
        for link in links {
            tokio::spawn(tell(link, now.clone()));
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

    /// A link made, `dialled` by this machine or by the other: kept, told this machine's
    /// addresses, and served. When the two dialled each other at once, each holds two links: both
    /// keep the one the machine of the lower key dialled, and close the other.
    fn linked(self: &Arc<Self>, link: Link, name: String, dialled: bool) {
        let generation = self
            .links
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let key = *link.peer().as_bytes();
        {
            let mut seen = self.seen.lock().unwrap();
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
                if !keep_new {
                    link.close(LINKED_ALREADY);
                    return;
                }
                old.close(LINKED_ALREADY);
            } else if let Some((_, old, _)) = &seen.link {
                // A link of the same way again: the old one is gone and not noticed yet.
                old.close(LINKED_ALREADY);
            }
            seen.set(PeerState::Online, None);
            seen.link = Some((generation, link.clone(), dialled));
            seen.wait = ROUND;
        }
        eprintln!(
            "flottille daemon: linked to {name} (protocol {})",
            link.protocol()
        );
        tokio::spawn(tell(link.clone(), self.told_addresses()));
        tokio::spawn(serve(self.clone(), link, name, generation));
    }

    /// A link that could not be made, or that ended: the machine's state, said once in the log.
    fn failed(&self, key: [u8; 32], name: &str, error: &LinkError) {
        let reason = error.to_string();
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

    /// The addresses another machine told: kept for it in the peers' file, beside those it was
    /// given, so that the two can link again without the hub. A file not read whole is not
    /// written over; one changed by another hand while it is updated is read again, once, rather
    /// than written over.
    fn learn(&self, key: &PublicKey, addresses: &[String]) {
        let addresses: Vec<String> = addresses
            .iter()
            .filter_map(|text| Address::parse(text).ok())
            .map(|address| address.to_string())
            .take(MOST_ADDRESSES)
            .collect();
        let file = self.folder.join(PEERS_FILE);
        // Held across the read and the write: two machines telling at once lose nothing.
        let _writing = self.writing.lock().unwrap();
        for _ in 0..2 {
            let read = stamp(&file);
            let Ok(mut peers) = read_json::<Peers>(&file) else {
                return;
            };
            let Some(peer) = peers.machines.iter_mut().find(|peer| peer.key == *key) else {
                return;
            };
            if peer.told == addresses {
                return;
            }
            peer.told.clone_from(&addresses);
            let name = peer.name.clone();
            let written = write_json_unless(&file, &peers, || stamp(&file) != read);
            match written {
                Ok(true) => return,
                Ok(false) => continue,
                Err(error) => {
                    eprintln!("flottille daemon: the addresses {name} told were not kept: {error}");
                    return;
                }
            }
        }
        eprintln!(
            "flottille daemon: {} kept changing, the addresses told were not kept",
            file.display()
        );
    }

    /// The addresses the machine of `key` told: written by `learn`, away from the tasks that
    /// take links, at most once every `LEARN_EVERY` for it; the latest told is the one written.
    fn told_by(self: &Arc<Self>, key: PublicKey, addresses: Vec<String>) {
        {
            let mut learning = self.learning.lock().unwrap();
            if let Some(latest) = learning.get_mut(key.as_bytes()) {
                *latest = Some(addresses);
                return;
            }
            learning.insert(*key.as_bytes(), Some(addresses));
        }
        let remote = self.clone();
        tokio::spawn(async move {
            loop {
                let addresses = {
                    let mut learning = remote.learning.lock().unwrap();
                    match learning.get_mut(key.as_bytes()).and_then(Option::take) {
                        Some(addresses) => addresses,
                        None => {
                            learning.remove(key.as_bytes());
                            return;
                        }
                    }
                };
                let writing = remote.clone();
                let _ = tokio::task::spawn_blocking(move || writing.learn(&key, &addresses)).await;
                tokio::time::sleep(LEARN_EVERY).await;
            }
        });
    }

    /// `remote.machines`: this machine, and the others as it sees them.
    fn machines(&self) -> Value {
        let key = self.endpoint.key();
        let this = json!({
            "key": key,
            "fingerprint": key.fingerprint(),
            "listening": self.endpoint.listening().map(|local| local.to_string()),
            "addresses": self.addresses().into_iter()
                .map(|(address, how)| json!({"address": address, "how": how}))
                .collect::<Vec<_>>(),
        });
        let mut seen = self.seen.lock().unwrap();
        let machines: Vec<Value> = self
            .peers()
            .machines
            .into_iter()
            .map(|peer| {
                let seen = seen.entry(*peer.key.as_bytes()).or_insert_with(Seen::new);
                json!({
                    "name": peer.name,
                    "key": peer.key,
                    "fingerprint": peer.key.fingerprint(),
                    "state": seen.state.to_string(),
                    "since": seen.since,
                    "reason": seen.reason,
                    "addresses": peer.known(),
                })
            })
            .collect();
        json!({"this": this, "machines": machines})
    }
}

/// Tells a machine this one's addresses.
async fn tell(link: Link, addresses: Vec<String>) {
    let _ = link
        .send(&Message::Machine(MachineMessage::Addresses(addresses)))
        .await;
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
        let Some(peer) = remote.trusted(&key) else {
            remote.say_refused(&key);
            accepting.refuse(NOT_TRUSTED);
            continue;
        };
        let remote = remote.clone();
        tokio::spawn(async move {
            let traffic = accepting.traffic();
            match accepting.handshake(&remote.hello, &peer.name).await {
                Ok(link) if traffic == Traffic::Machine => {
                    if let Some(late) = late {
                        tokio::time::sleep(late).await;
                    }
                    remote.linked(link, peer.name, false)
                }
                Ok(link) => serve_clients(link).await,
                Err(error) => remote.failed(*key.as_bytes(), &peer.name, &error),
            }
        });
    }
}

/// What another machine sends on a machine link.
async fn serve(remote: Arc<Remote>, link: Link, name: String, generation: u64) {
    while let Some(received) = link.next().await {
        match received.message() {
            Message::Machine(MachineMessage::Addresses(addresses)) => {
                remote.told_by(link.peer(), addresses.clone());
                received.answer(Ok(json!({})));
            }
            Message::Client(_) => received.answer(Err(NOT_SERVED.to_owned())),
        }
    }
    let why = link.closed().await;
    let mut seen = remote.seen.lock().unwrap();
    if let Some(seen) = seen.get_mut(link.peer().as_bytes())
        && seen
            .link
            .as_ref()
            .is_some_and(|(at, _, _)| *at == generation)
    {
        eprintln!("flottille daemon: the link to {name} ended: {why}");
        seen.link = None;
        seen.set(PeerState::Offline, Some(why));
        seen.next_try = Instant::now();
    }
}

/// A client link: its requests are refused until they are served (#813).
async fn serve_clients(link: Link) {
    while let Some(received) = link.next().await {
        received.answer(Err(NOT_SERVED.to_owned()));
    }
}

/// Every round: dials each machine trusted and not linked whose turn has come, and tells the
/// linked ones the addresses of this machine that changed.
async fn dial(remote: Arc<Remote>, mut stopping: tokio::sync::watch::Receiver<bool>) {
    let mut round = tokio::time::interval(ROUND);
    loop {
        tokio::select! {
            _ = round.tick() => {}
            _ = stopping.wait_for(|stopping| *stopping) => return,
        }
        remote.tell_if_changed();
        for line in remote.refused_lines(None, Instant::now()) {
            eprintln!("flottille daemon: {line}");
        }
        let peers = remote.peers().machines;
        let mut due = Vec::new();
        {
            let mut seen = remote.seen.lock().unwrap();
            // A machine no longer trusted is let go.
            seen.retain(|key, seen| {
                let trusted = peers.iter().any(|peer| peer.key.as_bytes() == key);
                if !trusted && let Some((_, link, _)) = &seen.link {
                    link.close(NOT_TRUSTED);
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
    let mut sockets: Vec<SocketAddr> = Vec::new();
    for text in &peer.known() {
        if let Ok(address) = Address::parse(text) {
            // An address told always has its port; one the user gave without uses the default.
            for socket in address.resolve(crate::remote::PORT).await {
                if !sockets.contains(&socket) {
                    sockets.push(socket);
                }
            }
        }
    }
    let key = *peer.key.as_bytes();
    let made = remote
        .endpoint
        .connect(
            peer.key,
            &sockets,
            Traffic::Machine,
            &remote.hello,
            &peer.name,
        )
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
fn write_json(file: &Path, value: &impl Serialize) -> std::io::Result<()> {
    write_json_unless(file, value, || false).map(|_| ())
}

/// Writes a JSON file of the key folder whole, as `write_json`, unless `changed` says, just
/// before it is renamed into place, that the file changed since it was read: then it is left
/// as it is, and `false` said.
fn write_json_unless(
    file: &Path,
    value: &impl Serialize,
    changed: impl Fn() -> bool,
) -> std::io::Result<bool> {
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
    if changed() {
        std::fs::remove_file(&temporary)?;
        return Ok(false);
    }
    std::fs::rename(&temporary, file)?;
    Ok(true)
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
    let remote = remote_of(&daemon)?;
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "remote.machines" => Ok(remote.machines()),
        "remote.address.add" | "remote.address.remove" => {
            let text = params["address"].as_str().unwrap_or_default();
            let address = Address::parse(text)
                .map_err(|why| Failure::new(flottille_protocol::code::INVALID_PARAMS, why))?;
            let file = remote.folder.join(ADDRESSES_FILE);
            let mut declared: Declared = read_json(&file).map_err(|error| {
                Failure::new(
                    flottille_protocol::code::UNAVAILABLE,
                    format!("{} is not read: {error}", file.display()),
                )
            })?;
            let at = declared
                .addresses
                .iter()
                .position(|known| Address::parse(known).ok().as_ref() == Some(&address));
            match (method, at) {
                ("remote.address.add", None) => declared.addresses.push(address.to_string()),
                ("remote.address.add", Some(_)) => {}
                (_, Some(at)) => {
                    declared.addresses.remove(at);
                }
                (_, None) => {
                    return Err(Failure::new(
                        flottille_protocol::code::NOT_FOUND,
                        format!("{address} is not an address declared on this machine"),
                    ));
                }
            }
            write_json(&file, &declared).map_err(|error| {
                Failure::new(
                    flottille_protocol::code::UNAVAILABLE,
                    format!("{} cannot be written: {error}", file.display()),
                )
            })?;
            remote.tell_if_changed();
            Ok(remote.machines()["this"].clone())
        }
        _ => Err(Failure::new(
            flottille_protocol::code::UNKNOWN_METHOD,
            format!("unknown method `{method}`"),
        )),
    }
}

fn remote_of(daemon: &Daemon) -> Result<Arc<Remote>, Failure> {
    match daemon.remote.get() {
        None => Err(Failure::refused(
            "remote is off: turn it on in daemon.toml ([remote] on = true)",
        )),
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
        assert!(peers.machines[0].addresses.is_empty());

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

    /// A daemon's side of the link over a key folder of a test's, linked to nothing; and the keys
    /// of `peers` machines it trusts, each with no address.
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
                    addresses: vec!["192.0.2.1:7447".to_owned()],
                    told: Vec::new(),
                })
                .collect(),
        };
        write_json(&folder.join(PEERS_FILE), &peers).unwrap();
        let endpoint = Endpoint::dialler(&key).unwrap();
        (Remote::new(endpoint, folder.to_owned()), keys)
    }

    fn told(folder: &Path, key: &PublicKey) -> Vec<String> {
        let peers: Peers = read_json(&folder.join(PEERS_FILE)).unwrap();
        let peer = peers.machines.iter().find(|peer| peer.key == *key).unwrap();
        assert_eq!(
            peer.addresses,
            ["192.0.2.1:7447"],
            "the given addresses are kept"
        );
        peer.told.clone()
    }

    #[tokio::test]
    async fn a_machine_telling_many_addresses_has_64_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, keys) = remote_in(dir.path(), 1);
        let many: Vec<String> = (0..100).map(|at| format!("192.0.2.{at}:4000")).collect();
        remote.learn(&keys[0], &many);
        assert_eq!(told(dir.path(), &keys[0]), many[..MOST_ADDRESSES]);
    }

    #[tokio::test]
    async fn learning_leaves_a_broken_peers_file_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, keys) = remote_in(dir.path(), 1);
        let file = dir.path().join(PEERS_FILE);
        let broken = br#"{"machines": [{"name": "machine-0", "ke"#;
        std::fs::write(&file, broken).unwrap();
        remote.learn(&keys[0], &["192.0.2.7:4000".to_owned()]);
        assert_eq!(std::fs::read(&file).unwrap(), broken);
    }

    #[tokio::test]
    async fn machines_telling_at_once_lose_no_address() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, keys) = remote_in(dir.path(), 16);
        for round in 0..5 {
            let start = std::sync::Barrier::new(keys.len());
            std::thread::scope(|scope| {
                for (at, key) in keys.iter().enumerate() {
                    let (remote, start) = (&remote, &start);
                    scope.spawn(move || {
                        start.wait();
                        remote.learn(key, &[format!("192.0.2.{at}:{}", 4000 + round)]);
                    });
                }
            });
            for (at, key) in keys.iter().enumerate() {
                assert_eq!(
                    told(dir.path(), key),
                    [format!("192.0.2.{at}:{}", 4000 + round)],
                    "round {round}, machine {at}"
                );
            }
        }
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
            ["24 other links were refused in the last minute, from keys not said"]
        );
        assert!(remote.refused_lines(None, later).is_empty());
        assert_eq!(remote.refused_lines(Some(&keys[0]), later).len(), 1);
    }

    #[tokio::test]
    async fn a_machine_telling_again_and_again_has_its_latest_written_every_2_s() {
        let dir = tempfile::tempdir().unwrap();
        let (remote, keys) = remote_in(dir.path(), 1);
        let remote = Arc::new(remote);
        for at in 0..50 {
            remote.told_by(keys[0], vec![format!("192.0.2.{at}:4000")]);
        }
        // The first is written at once; those told after it wait, the latest kept.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(told(dir.path(), &keys[0]), ["192.0.2.0:4000"]);
        tokio::time::sleep(LEARN_EVERY).await;
        assert_eq!(told(dir.path(), &keys[0]), ["192.0.2.49:4000"]);
        // Once written, nothing waits.
        tokio::time::sleep(LEARN_EVERY).await;
        assert!(remote.learning.lock().unwrap().is_empty());
    }
}
