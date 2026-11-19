//! Tunnels (`docs/remote.md`, "Tunnels"): a run's port on another machine, reached on a local port
//! of this one.
//!
//! The machine that carries a tunnel (the client's own daemon) listens on `127.0.0.1` and opens
//! one stream of the link for each TCP connection (`flottille_link::Link::tunnel`); the machine
//! of the run connects to `127.0.0.1:<port>` there and copies the bytes both ways. The tunnel is
//! held open by its `tunnel.open` request, signed by the user's device and checked by the machine
//! of the run like any request of another machine's client (`Remote::accept`): its answer is the
//! first frame of the request's stream, and the stream ends when the tunnel does.
//!
//! What a tunnel may reach is decided by the machine of the run, never by the one that asks: a run
//! Flottille started, and a port that run reports as its address or its processes listen on, read
//! from the system. The check is made again for each connection. No agent's tool opens one, and
//! no machine message carries one: it is a user's request only.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flottille_core::protocol::{Event, Message as Answer};
use flottille_link::{Accepted, Answers, Link, LinkError, PublicKey, Streamed, TunnelStream};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use crate::daemon::Daemon;
use crate::link::Remote;
use crate::projects::Failure;

/// The tunnels one device may have open on a machine at once.
pub(crate) const MOST_TUNNELS: usize = 8;
/// The TCP connections one tunnel carries at once; a connection past it is closed at once.
pub(crate) const MOST_CONNECTIONS: usize = 64;
/// How long the machine of the run waits for its port to take a connection.
const CONNECT_WITHIN: Duration = Duration::from_secs(5);

/// The tunnels of a machine: the ones it carries (its clients' listeners) and the ones it serves
/// (for the clients of other machines).
#[derive(Default)]
pub(crate) struct Tunnels {
    carried: Mutex<HashMap<String, Arc<Carried>>>,
    served: Mutex<HashMap<String, Arc<Served>>>,
}

/// A tunnel this machine carries: its listener is here.
struct Carried {
    id: String,
    machine: String,
    run: String,
    port: u16,
    local_port: u16,
    since: u64,
    connections: AtomicUsize,
    /// Set to the reason when something asks it to close (`tunnel.close`).
    close: watch::Sender<Option<String>>,
}

/// A tunnel this machine serves for a device of another machine.
struct Served {
    id: String,
    /// The member whose client asked, its key: tunnels are counted by it.
    device: PublicKey,
    /// The machine the tunnel came from: only its link may open a connection of it.
    carrier: PublicKey,
    run: String,
    port: u16,
    pid: u32,
    url: Option<String>,
    connections: AtomicUsize,
    closed: watch::Sender<bool>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

/// Counts a connection while it lives.
struct Counted<'a>(&'a AtomicUsize);

impl<'a> Counted<'a> {
    /// `None` when `most` are counted already.
    fn take(count: &'a AtomicUsize, most: usize) -> Option<Counted<'a>> {
        count
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |held| {
                (held < most).then_some(held + 1)
            })
            .ok()
            .map(|_| Counted(count))
    }
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The methods of a tunnel answered by this machine's own daemon, to its own clients: the list
/// and the closing of what it carries. `tunnel.open` is a request for another machine, carried
/// (`link::forward`) and served by that machine (`serve`): here it is refused.
pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    let remote = crate::link::remote_of(&daemon)?;
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "tunnel.list" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct List {}
            serde_json::from_value::<List>(params)?;
            let mut listed: Vec<Arc<Carried>> = remote
                .tunnels
                .carried
                .lock()
                .unwrap()
                .values()
                .cloned()
                .collect();
            listed.sort_by_key(|tunnel| (tunnel.since, tunnel.id.clone()));
            Ok(json!({"tunnels": listed.iter().map(|tunnel| json!({
                "tunnel": tunnel.id,
                "machine": tunnel.machine,
                "run": tunnel.run,
                "port": tunnel.port,
                "local_port": tunnel.local_port,
                "address": format!("127.0.0.1:{}", tunnel.local_port),
                "connections": tunnel.connections.load(Ordering::SeqCst),
                "since": tunnel.since,
            })).collect::<Vec<_>>()}))
        }
        "tunnel.close" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Close {
                tunnel: String,
            }
            let Close { tunnel } = serde_json::from_value(params)?;
            let carried = remote.tunnels.carried.lock().unwrap().get(&tunnel).cloned();
            let Some(carried) = carried else {
                return Err(Failure::new(
                    flottille_protocol::code::NOT_FOUND,
                    format!("no tunnel {tunnel} is carried by this machine"),
                ));
            };
            carried
                .close
                .send_replace(Some("closed by the user".to_owned()));
            Ok(json!({"closed": tunnel}))
        }
        "tunnel.open" => Err(Failure::refused(
            "refused: a tunnel is to another machine's run: name the machine (--machine)",
        )),
        method => Err(Failure::new(
            flottille_protocol::code::UNKNOWN_METHOD,
            format!("unknown method `{method}`"),
        )),
    }
}

// ---- The machine that carries the tunnel ------------------------------------------------------

/// A tunnel `request` (a signed `tunnel.open` for `machine`) was sent on `link` and `answers` is
/// what comes back: the machine's answer, then nothing until the tunnel ends. On its answer this
/// machine listens on `127.0.0.1`, tells the client where, and carries each connection until the
/// tunnel is closed: by the machine of the run (the run ended, the device was removed), by
/// `tunnel.close`, with the link, or when the client's connection goes.
pub(crate) async fn carry(
    remote: &Arc<Remote>,
    request: &Value,
    machine: &str,
    link: Link,
    mut answers: Answers,
    out: &mpsc::Sender<Answer>,
) {
    let id = request["id"].as_str().map(str::to_owned);
    let error = |code: &str, why: String| Answer::error(id.clone(), code, why);
    let first = match answers.next().await {
        Some(Ok(value)) => value,
        Some(Err(LinkError::Declined { reason, .. })) => {
            let _ = out
                .send(error(flottille_protocol::code::REFUSED, reason))
                .await;
            return;
        }
        Some(Err(why)) => {
            let _ = out.send(error("unavailable", why.to_string())).await;
            return;
        }
        None => {
            let _ = out
                .send(error(
                    "unavailable",
                    format!("the link to {machine} ended before it answered"),
                ))
                .await;
            return;
        }
    };
    let Ok(answer) = serde_json::from_value::<Answer>(first) else {
        let _ = out
            .send(error(
                "unavailable",
                format!("{machine} answered what is not an answer"),
            ))
            .await;
        return;
    };
    if answer.id != id || answer.error.is_some() {
        let _ = out.send(answer).await;
        return;
    }
    let result = answer.result.unwrap_or_default();
    let (Some(tunnel), Some(run), Some(port)) = (
        result["tunnel"].as_str(),
        result["run"].as_str(),
        result["port"]
            .as_u64()
            .and_then(|port| u16::try_from(port).ok()),
    ) else {
        let _ = out
            .send(error(
                "unavailable",
                format!("{machine} answered what is not a tunnel"),
            ))
            .await;
        return;
    };
    let (listener, local_port) = match bind_local(port) {
        Ok(bound) => bound,
        Err(why) => {
            let _ = out
                .send(error(
                    "unavailable",
                    format!("cannot listen on 127.0.0.1: {why}"),
                ))
                .await;
            return;
        }
    };
    let (close, mut closing) = watch::channel(None);
    let carried = Arc::new(Carried {
        id: tunnel.to_owned(),
        machine: machine.to_owned(),
        run: run.to_owned(),
        port,
        local_port,
        since: now(),
        connections: AtomicUsize::new(0),
        close,
    });
    remote
        .tunnels
        .carried
        .lock()
        .unwrap()
        .insert(carried.id.clone(), carried.clone());
    let mut holding = Holding {
        remote: remote.clone(),
        carried: carried.clone(),
        reason: "its client left".to_owned(),
        accepting: tokio::spawn(accept(
            listener,
            link.clone(),
            carried.clone(),
            closing.clone(),
        )),
    };
    remote.emit(
        "tunnel.opened",
        json!({"tunnel": carried.id, "machine": machine, "run": run, "port": port,
            "local_port": local_port}),
    );
    let shown = json!({"tunnel": carried.id, "machine": machine, "run": run, "port": port,
        "local_port": local_port, "address": format!("127.0.0.1:{local_port}")});
    if out
        .send(Answer::result(id.clone().unwrap_or_default(), shown))
        .await
        .is_err()
    {
        return;
    }
    // Until the tunnel ends, whichever side ends it.
    holding.reason = tokio::select! {
        // The machine of the run ended it, saying why, or its stream ended with the link.
        frame = answers.next() => match frame {
            Some(Ok(frame)) => frame["closed"].as_str().unwrap_or("the tunnel ended").to_owned(),
            Some(Err(why)) => why.to_string(),
            None => format!("{machine} closed it"),
        },
        why = link.closed() => why,
        _ = closing.changed() => closing.borrow().clone().unwrap_or_default(),
        _ = out.closed() => "its client left".to_owned(),
    };
    // The command that opened it says that it ended.
    let closed = Event {
        seq: 0,
        at: now(),
        name: "tunnel.closed".to_owned(),
        data: json!({"tunnel": carried.id, "machine": machine, "run": run,
            "reason": holding.reason}),
    };
    let _ = out.send(Answer::event(closed)).await;
}

/// A tunnel carried, until it is dropped: the listener and what it carries go, the tunnel leaves
/// the list, and the event says why.
struct Holding {
    remote: Arc<Remote>,
    carried: Arc<Carried>,
    reason: String,
    accepting: tokio::task::JoinHandle<()>,
}

impl Drop for Holding {
    fn drop(&mut self) {
        self.accepting.abort();
        self.remote
            .tunnels
            .carried
            .lock()
            .unwrap()
            .remove(&self.carried.id);
        eprintln!(
            "flottille daemon: the tunnel to port {} of run {} on {} ended: {}",
            self.carried.port, self.carried.run, self.carried.machine, self.reason
        );
        self.remote.emit(
            "tunnel.closed",
            json!({"tunnel": self.carried.id, "machine": self.carried.machine,
                "run": self.carried.run, "reason": self.reason}),
        );
    }
}

/// Listens on `127.0.0.1` only: `port` when it is free, another one otherwise. The port it got.
fn bind_local(port: u16) -> std::io::Result<(TcpListener, u16)> {
    let bind = |port| {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        Ok::<_, std::io::Error>((TcpListener::from_std(listener)?, port))
    };
    bind(port).or_else(|_| bind(0))
}

/// Takes the connections of the local port: each one is a stream of the link, until the tunnel
/// closes (the tasks of the connections go with this one).
async fn accept(
    listener: TcpListener,
    link: Link,
    carried: Arc<Carried>,
    mut closing: watch::Receiver<Option<String>>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((tcp, _)) = accepted else { continue };
                let (link, carried) = (link.clone(), carried.clone());
                connections.spawn(async move {
                    // Past the bound it is closed, as a full server would.
                    let Some(_held) = Counted::take(&carried.connections, MOST_CONNECTIONS) else {
                        return;
                    };
                    let _ = tcp.set_nodelay(true);
                    carry_connection(link, &carried.id, tcp).await;
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            _ = closing.changed() => break,
        }
    }
}

/// One TCP connection, as one stream of the link; no word of why when it fails: the other side's
/// log says.
async fn carry_connection(link: Link, id: &str, mut tcp: TcpStream) {
    let Ok(mut stream) = link.tunnel(id).await else {
        return;
    };
    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
}

// ---- The machine of the run -------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    run: String,
    #[serde(default)]
    port: Option<u16>,
}

/// A `tunnel.*` request another machine's client signed, checked by `Remote::accept`, as this
/// machine of the run serves it. Only `tunnel.open` is (the others are of the machine that
/// carries): the answer is the first frame of `answer`, which then stays open until the tunnel
/// ends, and its last frame says why.
pub(crate) async fn serve(
    remote: &Arc<Remote>,
    link: &Link,
    accepted: Accepted,
    answer: &mut Streamed,
    from: &str,
) {
    let id = accepted.id.clone();
    let device = accepted.device.name.clone();
    let signer = accepted.device.key;
    let refuse = |code: &str, why: String| Answer::error(Some(id.clone()), code, why);
    let Some(daemon) = remote.daemon() else {
        let _ = answer
            .send(&bytes(&refuse(
                "unavailable",
                "the daemon is stopping".to_owned(),
            )))
            .await;
        return;
    };
    if accepted.method != "tunnel.open" {
        let why = format!("{} is answered only on its own machine", accepted.method);
        let _ = answer
            .send(&bytes(&refuse(flottille_protocol::code::REFUSED, why)))
            .await;
        return;
    }
    eprintln!("flottille daemon: tunnel.open asked by {device} from {from}");
    let opened = match open(remote, &daemon, link, signer, accepted.params) {
        Ok(opened) => opened,
        Err(why) => {
            let _ = answer
                .send(&bytes(&refuse(flottille_protocol::code::REFUSED, why)))
                .await;
            return;
        }
    };
    let (served, mut reach) = opened;
    let mut lists = remote.lists.subscribe();
    let shown = json!({"tunnel": served.id, "run": served.run, "port": served.port});
    remote.emit(
        "tunnel.opened",
        json!({"tunnel": served.id, "run": served.run, "port": served.port, "device": device}),
    );
    let ended = Ending {
        remote: remote.clone(),
        served: served.clone(),
        device: device.clone(),
    };
    if !answer
        .send(&bytes(&Answer::result(id.clone(), shown)))
        .await
    {
        return;
    }
    let gone = answer.gone();
    tokio::pin!(gone);
    // Until the run ends, the link goes, the device is no member's, or its client closes it.
    let reason = loop {
        tokio::select! {
            _ = &mut gone => break "its client closed it".to_owned(),
            _ = reach.ended() => break format!("run {} ended", served.run),
            _ = link.closed() => break "the link was lost".to_owned(),
            changed = lists.changed() => {
                if changed.is_err() || !remote.still_member(&signer).await {
                    break format!("{device} is no longer one of the user's devices");
                }
            }
        }
    };
    let _ = answer.send(&bytes(&json!({"closed": reason}))).await;
    drop(ended);
}

/// What `open` makes of a request's parameters: the tunnel to serve, and the run it reaches.
fn open(
    remote: &Arc<Remote>,
    daemon: &Arc<Daemon>,
    link: &Link,
    signer: PublicKey,
    params: Value,
) -> Result<(Arc<Served>, crate::runs::Reachable), String> {
    let Open { run, port } = serde_json::from_value(params)
        .map_err(|error| format!("refused: tunnel.open takes {{run, port?}}: {error}"))?;
    let Some(reach) = crate::runs::reachable(daemon, &run) else {
        return Err(format!("refused: there is no run {run} going on here"));
    };
    let reached = reached(reach.pid, reach.url.as_deref());
    let port = match port {
        Some(port) if reached.allows(port) => port,
        Some(port) => {
            return Err(format!(
                "refused: port {port} is none of run {run}'s: {}",
                reached.said(&run)
            ));
        }
        None => reached
            .only()
            .map_err(|why| format!("refused: {why} (run {run})"))?,
    };
    let mut served = remote.tunnels.served.lock().unwrap();
    if served
        .values()
        .filter(|tunnel| tunnel.device == signer)
        .count()
        >= MOST_TUNNELS
    {
        return Err(format!(
            "refused: {MOST_TUNNELS} tunnels are open for this device already: close one"
        ));
    }
    let tunnel = Arc::new(Served {
        id: new_id(),
        device: signer,
        carrier: link.peer(),
        run,
        port,
        pid: reach.pid,
        url: reach.url.clone(),
        connections: AtomicUsize::new(0),
        closed: watch::Sender::new(false),
    });
    served.insert(tunnel.id.clone(), tunnel.clone());
    Ok((tunnel, reach))
}

/// A tunnel's id: 8 random bytes in hexadecimal. It names a tunnel on the link of the machine it
/// was opened for, which is what lets a connection in; it is no secret.
fn new_id() -> String {
    let mut bytes = [0u8; 8];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| std::io::Read::read_exact(&mut random, &mut bytes))
        .expect("the system has random bytes");
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("t{hex}")
}

/// A served tunnel, until it is dropped: it leaves the list, its connections are cut, the event
/// and the log say it.
struct Ending {
    remote: Arc<Remote>,
    served: Arc<Served>,
    device: String,
}

impl Drop for Ending {
    fn drop(&mut self) {
        self.served.closed.send_replace(true);
        self.remote
            .tunnels
            .served
            .lock()
            .unwrap()
            .remove(&self.served.id);
        eprintln!(
            "flottille daemon: the tunnel asked by {} ended",
            self.device
        );
        self.remote.emit(
            "tunnel.closed",
            json!({"tunnel": self.served.id, "run": self.served.run, "port": self.served.port,
                "device": self.device}),
        );
    }
}

fn bytes(message: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(message).expect("a message is JSON")
}

/// A connection of a tunnel, opened by the machine `carrier` on its client link: connected to the
/// port of the run, on `127.0.0.1` and nowhere else, and copied both ways. A connection for a
/// tunnel this machine did not open for that machine is closed with no word, and said in the log.
pub(crate) async fn connection(
    remote: Arc<Remote>,
    carrier: PublicKey,
    tunnel: String,
    mut stream: TunnelStream,
) {
    let served = remote.tunnels.served.lock().unwrap().get(&tunnel).cloned();
    let Some(served) = served.filter(|served| served.carrier == carrier) else {
        let from = remote
            .name_of(&carrier)
            .unwrap_or_else(|| "a machine".to_owned());
        eprintln!(
            "flottille daemon: {from} opened a connection of a tunnel {tunnel} that is none of its: closed"
        );
        return;
    };
    let Some(_held) = Counted::take(&served.connections, MOST_CONNECTIONS) else {
        return;
    };
    // The port is checked again: the run's own listener may have closed, and another process
    // taken its port since.
    let (pid, url, port) = (served.pid, served.url.clone(), served.port);
    let allowed = tokio::task::spawn_blocking(move || reached(pid, url.as_deref()).allows(port))
        .await
        .unwrap_or(false);
    if !allowed {
        eprintln!(
            "flottille daemon: port {port} is no longer one of run {}'s: a connection of tunnel {tunnel} is closed",
            served.run
        );
        return;
    }
    let connected = tokio::time::timeout(
        CONNECT_WITHIN,
        TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port))),
    )
    .await;
    let Ok(Ok(mut tcp)) = connected else {
        return;
    };
    let _ = tcp.set_nodelay(true);
    let mut closed = served.closed.subscribe();
    tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream) => {}
        _ = closed.wait_for(|closed| *closed) => {}
    }
}

// ---- Which ports a run has --------------------------------------------------------------------

/// The ports of a run a tunnel may reach.
struct Reached {
    /// The port of the address the run reports, when it has one on this machine.
    address: Option<u16>,
    /// The ports its processes listen on, on `127.0.0.1` or on every address.
    listening: Vec<u16>,
}

/// What the run whose session `pid` leads, and that reports `url`, reaches. Reads `/proc`.
fn reached(pid: u32, url: Option<&str>) -> Reached {
    let inodes = sockets_of(&crate::children::session_members(pid));
    let mut listening = Vec::new();
    for (table, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        if let Ok(text) = std::fs::read_to_string(table) {
            listening.extend(listeners(&text, &inodes, v6));
        }
    }
    listening.sort_unstable();
    listening.dedup();
    Reached {
        address: url.and_then(address_port),
        listening,
    }
}

impl Reached {
    fn allows(&self, port: u16) -> bool {
        self.address == Some(port) || self.listening.contains(&port)
    }

    /// The port to take when none was asked for: the address's, or the only one listened on.
    fn only(&self) -> Result<u16, String> {
        if let Some(port) = self.address {
            return Ok(port);
        }
        match self.listening.as_slice() {
            [port] => Ok(*port),
            [] => Err("it reports no address and listens on no port yet".to_owned()),
            several => Err(format!(
                "it listens on several ports ({}): say which with --port",
                several
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// What is reachable, for a refusal.
    fn said(&self, run: &str) -> String {
        let mut ports: Vec<u16> = self.listening.clone();
        ports.extend(self.address);
        ports.sort_unstable();
        ports.dedup();
        match ports.as_slice() {
            [] => "it reports no address and listens on no port".to_owned(),
            ports => format!(
                "run {run} reaches {}",
                ports
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// The port of an address a run reports (`http://localhost:3000/`) when it is one of this machine:
/// `localhost`, a name under it, `127.0.0.1`, `[::1]` or `0.0.0.0`, with a port written.
fn address_port(url: &str) -> Option<u16> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let (host, port) = authority.rsplit_once(':')?;
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let here = host == "localhost"
        || host.ends_with(".localhost")
        || matches!(host.as_str(), "127.0.0.1" | "::1" | "0.0.0.0");
    if here { port.parse().ok() } else { None }
}

/// The inodes of the sockets the processes `pids` hold open.
fn sockets_of(pids: &[u32]) -> HashSet<u64> {
    let mut inodes = HashSet::new();
    for pid in pids {
        let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(target) = std::fs::read_link(entry.path()) else {
                continue;
            };
            let target = target.to_string_lossy();
            if let Some(inode) = target
                .strip_prefix("socket:[")
                .and_then(|rest| rest.strip_suffix(']'))
                .and_then(|inode| inode.parse().ok())
            {
                inodes.insert(inode);
            }
        }
    }
    inodes
}

/// The ports a table of `/proc/net/tcp` (`v6`: `tcp6`) lists as listening, on `127.0.0.1` or on
/// every address, whose socket is one of `inodes`.
fn listeners(table: &str, inodes: &HashSet<u64>, v6: bool) -> Vec<u16> {
    // Little-endian words, as the kernel prints them: 127.0.0.1, ::1, ::, and 127.0.0.1 mapped.
    const HERE: [&str; 2] = ["0100007F", "00000000"];
    const HERE6: [&str; 4] = [
        "00000000000000000000000001000000",
        "00000000000000000000000000000000",
        "0000000000000000FFFF00000100007F",
        "0000000000000000FFFF000000000000",
    ];
    let mut ports = Vec::new();
    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // `local_address`, `st` (0A listens) and `inode`.
        let (Some(local), Some(state), Some(inode)) = (fields.get(1), fields.get(3), fields.get(9))
        else {
            continue;
        };
        let Some((host, port)) = local.split_once(':') else {
            continue;
        };
        let here = if v6 {
            HERE6.contains(&host)
        } else {
            HERE.contains(&host)
        };
        let (Ok(port), Ok(inode)) = (u16::from_str_radix(port, 16), inode.parse::<u64>()) else {
            continue;
        };
        if *state == "0A" && here && inodes.contains(&inode) {
            ports.push(port);
        }
    }
    ports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_port_is_the_run_s_when_it_is_free_and_another_when_it_is_not() {
        let held = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken = held.local_addr().unwrap().port();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (_other, got) = bind_local(taken).unwrap();
        assert_ne!(got, taken);
        drop(held);
        let (_same, got) = bind_local(taken).unwrap();
        assert_eq!(got, taken);
        // On the loopback alone.
        assert!(_same.local_addr().unwrap().ip().is_loopback());
    }

    #[test]
    fn a_run_s_address_is_a_port_of_this_machine_or_none() {
        assert_eq!(address_port("http://localhost:3000/"), Some(3000));
        assert_eq!(address_port("http://127.0.0.1:8080"), Some(8080));
        assert_eq!(address_port("https://app.localhost:5173/x?y=1"), Some(5173));
        assert_eq!(address_port("http://[::1]:9000/"), Some(9000));
        assert_eq!(address_port("http://0.0.0.0:4000"), Some(4000));
        // No port written, a host of another machine, a name that only ends like it.
        assert_eq!(address_port("http://localhost/"), None);
        assert_eq!(address_port("https://example.org:8443/"), None);
        assert_eq!(address_port("http://evil-localhost:80/"), None);
        assert_eq!(address_port("http://user@example.org:8443/"), None);
        assert_eq!(address_port("not a url"), None);
    }

    const TABLE: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0000000000000000 100 0 0 10 0\n\
   1: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 222 1 0000000000000000 100 0 0 10 0\n\
   2: 0100007F:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 333 1 0000000000000000 100 0 0 10 0\n\
   3: 0100007F:1770 0100007F:9999 01 00000000:00000000 00:00000000 00000000  1000        0 111 1 0000000000000000 100 0 0 10 0\n\
   4: 0A00020F:2328 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0000000000000000 100 0 0 10 0\n";

    #[test]
    fn only_the_listening_sockets_of_the_run_s_processes_on_the_loopback_count() {
        let ours: HashSet<u64> = [111, 222].into();
        // 0BB8 = 3000 and 1F90 = 8080; 22 is another process's, 6000 is connected, not
        // listening, and 9000 listens on a LAN address.
        assert_eq!(listeners(TABLE, &ours, false), vec![3000, 8080]);
        assert!(listeners(TABLE, &HashSet::new(), false).is_empty());
        let v6 = "  sl  local_address rem_address st tx rx tr tm retr uid timeout inode\n\
            0: 00000000000000000000000001000000:2328 00000000000000000000000000000000:0000 0A 0:0 0:0 0 1000 0 444 1\n\
            1: 0000000000000000FFFF0000C0A80001:2329 00000000000000000000000000000000:0000 0A 0:0 0:0 0 1000 0 444 1\n";
        assert_eq!(listeners(v6, &[444].into(), true), vec![9000]);
    }

    #[test]
    fn a_port_is_reached_by_the_address_or_by_a_listener_and_none_is_chosen_for_many() {
        let reach = |address, listening: &[u16]| Reached {
            address,
            listening: listening.to_vec(),
        };
        assert!(reach(Some(3000), &[]).allows(3000));
        assert!(reach(None, &[8080]).allows(8080));
        assert!(!reach(Some(3000), &[8080]).allows(22));
        assert_eq!(reach(Some(3000), &[8080]).only(), Ok(3000));
        assert_eq!(reach(None, &[8080]).only(), Ok(8080));
        assert!(reach(None, &[1, 2]).only().unwrap_err().contains("--port"));
        assert!(reach(None, &[]).only().is_err());
    }

    #[test]
    fn a_tunnel_takes_at_most_its_connections() {
        let count = AtomicUsize::new(0);
        let held: Vec<_> = (0..MOST_CONNECTIONS)
            .map(|_| Counted::take(&count, MOST_CONNECTIONS).unwrap())
            .collect();
        assert!(Counted::take(&count, MOST_CONNECTIONS).is_none());
        drop(held);
        assert!(Counted::take(&count, MOST_CONNECTIONS).is_some());
    }
}
