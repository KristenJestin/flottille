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

use std::collections::HashMap;
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
    let remote = Arc::new(Remote {
        endpoint,
        folder: crate::remote::folder(paths),
        hello: hello(),
        peers: Mutex::new(Kept {
            last: Peers::default(),
            said: false,
        }),
        seen: Mutex::new(HashMap::new()),
        told: Mutex::new(Vec::new()),
        links: Default::default(),
    });
    tokio::spawn(accept(remote.clone()));
    tokio::spawn(dial(remote.clone(), stopping));
    Ok(remote)
}

/// This build's hello; a test may make it another's.
fn hello() -> Hello {
    std::env::var("FLOTTILLE_TEST_LINK_HELLO")
        .ok()
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
    /// Closes every link, telling the other machines.
    pub async fn close(&self) {
        self.endpoint.close().await;
    }

    /// The machines this one trusts: the file as last read whole.
    fn peers(&self) -> Peers {
        let file = self.folder.join(PEERS_FILE);
        let mut kept = self.peers.lock().unwrap();
        match read_json::<Peers>(&file) {
            Ok(peers) => {
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
    /// written over.
    fn learn(&self, key: &PublicKey, addresses: &[String]) {
        let addresses: Vec<String> = addresses
            .iter()
            .filter_map(|text| Address::parse(text).ok())
            .map(|address| address.to_string())
            .take(MOST_ADDRESSES)
            .collect();
        let Ok(mut peers) = read_json::<Peers>(&self.folder.join(PEERS_FILE)) else {
            return;
        };
        let Some(peer) = peers.machines.iter_mut().find(|peer| peer.key == *key) else {
            return;
        };
        if peer.told == addresses {
            return;
        }
        peer.told = addresses;
        let name = peer.name.clone();
        if let Err(error) = write_json(&self.folder.join(PEERS_FILE), &peers) {
            eprintln!("flottille daemon: the addresses {name} told were not kept: {error}");
        }
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
    // other at once.
    let late = std::env::var("FLOTTILLE_TEST_LINK_ACCEPT_DELAY_MS")
        .ok()
        .and_then(|text| text.parse().ok())
        .map(Duration::from_millis);
    while let Some(accepting) = remote.endpoint.accept().await {
        let key = accepting.peer();
        let Some(peer) = remote
            .peers()
            .machines
            .into_iter()
            .find(|peer| peer.key == key)
        else {
            eprintln!(
                "flottille daemon: refused a link from {}: not a machine this one trusts",
                key.fingerprint()
            );
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
                remote.learn(&link.peer(), addresses);
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
}
