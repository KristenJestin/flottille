//! The daemon: one per user, behind a Unix socket speaking JSON lines (`docs/protocol.md`).

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flottille_core::paths::{Paths, ensure_private_dir};
use flottille_core::protocol::{Event, Message, Request, WatchParams};
use flottille_core::store::Store;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinSet;

use crate::VERSION;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Events read from the store at once when a subscriber catches up.
const PAGE: usize = 500;
/// How long the connections have, once the daemon stops, to send what they still hold.
const GRACE: Duration = Duration::from_secs(2);

pub fn run(paths: &Paths) -> Result<()> {
    ensure_private_dir(&paths.runtime)?;
    ensure_private_dir(&paths.state)?;
    let _lock = take_lock(paths)?;
    let store = Store::open(&paths.database())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(serve(paths, store))
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
    lock.set_len(0)?;
    writeln!(lock, "{}", std::process::id())?;
    Ok(lock)
}

struct Daemon {
    started: Instant,
    events: Mutex<Events>,
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
    fn emit(&self, name: &str, data: Value) -> Result<Event> {
        let events = self.events.lock().unwrap();
        let event = events.store.append_event(name, data)?;
        if let Some(bus) = &events.bus {
            let _ = bus.send(event.clone());
        }
        Ok(event)
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

async fn serve(paths: &Paths, store: Store) -> Result<()> {
    let socket = paths.socket();
    // Holding the lock, a socket already there is one a dead daemon left.
    match std::fs::remove_file(&socket) {
        Err(error) if error.kind() != ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let listener = UnixListener::bind(&socket)?;
    let (bus, _) = broadcast::channel(1024);
    let daemon = Arc::new(Daemon {
        started: Instant::now(),
        events: Mutex::new(Events {
            store,
            bus: Some(bus),
        }),
        stopping: watch::Sender::new(false),
    });
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut stop_requested = daemon.stopping.subscribe();
    daemon.emit(
        "daemon.started",
        json!({"version": VERSION, "pid": std::process::id()}),
    )?;
    eprintln!(
        "flottille daemon {VERSION} listening on {}",
        socket.display()
    );

    let mut connections = JoinSet::new();
    let reason = loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    connections.spawn(connection(daemon.clone(), stream));
                }
                Err(error) => eprintln!("flottille daemon: accept: {error}"),
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            _ = stop_requested.wait_for(|stopping| *stopping) => break "request",
            _ = terminate.recv() => break "signal",
            _ = interrupt.recv() => break "signal",
        }
    };

    drop(listener);
    std::fs::remove_file(&socket)?;
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
        "daemon.status" => (Message::result(id, status(daemon)), None),
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

fn status(daemon: &Daemon) -> Value {
    json!({
        "version": VERSION,
        "pid": std::process::id(),
        "uptime_s": daemon.started.elapsed().as_secs(),
        // Filled as agents arrive.
        "agents": {"running": 0, "waiting_for_user": 0, "failed": 0},
    })
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
