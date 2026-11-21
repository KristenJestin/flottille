//! The desktop's own notifications (#1002, `docs/design.md`, §11): `org.freedesktop.Notifications`
//! on the session bus, which every Linux desktop's notification server answers. The daemon sends
//! them itself, never through `notify-send`, and nothing here is specific to one desktop.
//!
//! This is the one module a platform's sender lives in: another platform's (macOS, Windows) is
//! another implementation of [`Desktop`]'s three calls, `notify`, `close` and `status`, with the
//! [`Heard`] it reports.
//!
//! One notification of Flottille's is one of the desktop's, sent once and closed when it is
//! answered or read elsewhere. The actions are asked for only when the server says it has them
//! (`actions`): a request has Deny, and Allow when the notification says the whole call
//! (`rings.rs`), and every notification's default action opens its session. No sound hint is
//! sent: the sound is Flottille's own, played by the daemon.
//!
//! What the desktop tells back is said with the server that told it, the bus's unique name of
//! the connection that owned `org.freedesktop.Notifications` then: another server that takes the
//! name later numbers its notifications from 1 again, and its numbers name none of Flottille's.
//! Every call is given [`ANSWER_WITHIN`]: a bus or a server that hangs holds nothing up.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt as _;
use zbus::zvariant::Value;

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The name and the launcher entry Flottille's notifications are shown under.
const APP_NAME: &str = "Flottille";
const ENTRY: &str = "flottille";

/// How long the bus and the server have to answer a call.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(5);

/// What a notification says to the desktop.
pub struct Outgoing {
    pub summary: String,
    pub body: String,
    /// Allow: a request whose whole call the body says.
    pub allows: bool,
    /// Deny: a request.
    pub denies: bool,
    /// Whether it stays until it is answered, rather than leaving after the desktop's own time.
    pub stays: bool,
}

/// What the desktop tells back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// The user chose an action of the notification `number` of `server` (a bus's unique name):
    /// `allow`, `deny` or `default`.
    Action {
        server: String,
        number: u32,
        key: String,
    },
    /// The notification `number` of `server` is gone: closed by the user, expired, or by the
    /// daemon.
    Closed { server: String, number: u32 },
}

/// A notification the desktop shows: the server that numbered it, and its number.
pub type Shown = (String, u32);

/// What is done with what the desktop tells back.
pub type Hears = Arc<dyn Fn(Heard) + Send + Sync>;

struct Link {
    connection: zbus::Connection,
    /// Whether the server supports actions.
    actions: bool,
    /// The server that answered last: a notification of another is not closed, its number may
    /// be another program's now.
    server: String,
}

/// The desktop's notifications, as far as this machine's session has them.
pub struct Desktop {
    link: tokio::sync::Mutex<Option<Link>>,
    /// Why nothing is shown, as the last attempt left it; `None` while a server answers or before
    /// the first attempt.
    why: std::sync::Mutex<Option<String>>,
    /// Whether `why` was said in the log already: said once, until a server answers again.
    said: AtomicBool,
}

impl Desktop {
    pub fn new() -> Desktop {
        Desktop {
            link: tokio::sync::Mutex::new(None),
            why: std::sync::Mutex::new(None),
            said: AtomicBool::new(false),
        }
    }

    /// Why the desktop shows nothing of Flottille's, `None` while it does (or before it is
    /// known).
    pub fn why_not(&self) -> Option<String> {
        self.why.lock().unwrap().clone()
    }

    /// Connects to the bus and the server when that is not done, and listens to what the desktop
    /// says of the notifications. The reason it cannot is said once in the log, and kept for
    /// Settings.
    pub async fn connect(&self, hears: &Hears) -> bool {
        let mut link = self.link.lock().await;
        if link.is_some() {
            return true;
        }
        match open(hears).await {
            Ok(opened) => {
                *link = Some(opened);
                *self.why.lock().unwrap() = None;
                self.said.store(false, Ordering::Relaxed);
                true
            }
            Err(why) => {
                *self.why.lock().unwrap() = Some(why.clone());
                if !self.said.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "flottille daemon: {why}: the desktop shows no notification of \
                         Flottille's, its window and its sound still tell the user"
                    );
                }
                false
            }
        }
    }

    /// Sends a notification: the server that shows it and its number there. `None` when the
    /// desktop shows nothing.
    pub async fn notify(&self, hears: &Hears, outgoing: &Outgoing) -> Option<Shown> {
        if !self.connect(hears).await {
            return None;
        }
        let (connection, actions) = {
            let link = self.link.lock().await;
            let link = link.as_ref()?;
            (link.connection.clone(), link.actions)
        };
        let mut offered: Vec<&str> = Vec::new();
        if actions {
            if outgoing.allows {
                offered.extend(["allow", "Allow"]);
            }
            if outgoing.denies {
                offered.extend(["deny", "Deny"]);
            }
            offered.extend(["default", "Open"]);
        }
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        hints.insert("desktop-entry", Value::from(ENTRY));
        let timeout: i32 = if outgoing.stays { 0 } else { -1 };
        let sent = within(connection.call_method(
            Some(DESTINATION),
            PATH,
            Some(DESTINATION),
            "Notify",
            &(
                APP_NAME,
                0u32,
                ENTRY,
                outgoing.summary.as_str(),
                outgoing.body.as_str(),
                offered,
                hints,
                timeout,
            ),
        ))
        .await
        .and_then(|reply| {
            let number = reply
                .body()
                .deserialize::<u32>()
                .map_err(|e| e.to_string())?;
            Ok((
                sender_of(&reply).ok_or("the answer names no sender")?,
                number,
            ))
        });
        match sent {
            Ok((server, number)) => {
                if let Some(link) = self.link.lock().await.as_mut() {
                    link.server = server.clone();
                }
                Some((server, number))
            }
            Err(error) => {
                // The server left, or the bus: the next notification connects again.
                *self.link.lock().await = None;
                *self.why.lock().unwrap() =
                    Some(format!("no notification server answers ({error})"));
                None
            }
        }
    }

    /// Closes a notification the desktop shows: what was answered or read elsewhere is not left
    /// on the screen. One of a server that left is gone with it.
    pub async fn close(&self, (server, number): &Shown) {
        let connection = match self.link.lock().await.as_ref() {
            Some(link) if link.server == *server => link.connection.clone(),
            _ => return,
        };
        let _ = within(connection.call_method(
            Some(DESTINATION),
            PATH,
            Some(DESTINATION),
            "CloseNotification",
            &(*number,),
        ))
        .await;
    }
}

/// A call of the bus, given [`ANSWER_WITHIN`].
async fn within<T>(call: impl Future<Output = zbus::Result<T>>) -> Result<T, String> {
    match tokio::time::timeout(ANSWER_WITHIN, call).await {
        Ok(answer) => answer.map_err(|error| error.to_string()),
        Err(_) => Err(format!("no answer within {} s", ANSWER_WITHIN.as_secs())),
    }
}

/// The unique name of the connection that sent a message.
fn sender_of(message: &zbus::Message) -> Option<String> {
    message.header().sender().map(|sender| sender.to_string())
}

/// The session bus's socket when it is a path nobody listens on: the connection is not tried
/// then. A failed attempt leaves a thread of the D-Bus library's pool behind, which wakes twice a
/// second for good, and the daemon wakes at rest for nothing (`daemon_at_rest.rs`).
fn absent_bus_socket() -> Option<String> {
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .ok()
        .filter(|address| !address.is_empty())
        .or_else(|| {
            let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
            Some(format!("unix:path={runtime}/bus"))
        })?;
    let paths = address.split(';').map(|one| {
        one.strip_prefix("unix:path=")
            .map(|rest| rest.split(',').next().unwrap_or_default())
    });
    // Only addresses of a socket file are known to be absent: a TCP or abstract one is tried.
    let paths: Vec<&str> = paths.collect::<Option<_>>()?;
    paths
        .iter()
        .all(|path| !std::path::Path::new(path).exists())
        .then(|| paths.join(", "))
}

async fn open(hears: &Hears) -> Result<Link, String> {
    if let Some(path) = absent_bus_socket() {
        return Err(format!(
            "no session bus (nothing at {path}: No such file or directory)"
        ));
    }
    let connection = within(zbus::Connection::session())
        .await
        .map_err(|error| format!("no session bus ({error})"))?;
    let no_server =
        |error: String| format!("no notification server answers on the session bus ({error})");
    let reply = within(connection.call_method(
        Some(DESTINATION),
        PATH,
        Some(DESTINATION),
        "GetCapabilities",
        &(),
    ))
    .await
    .map_err(no_server)?;
    let capabilities = reply
        .body()
        .deserialize::<Vec<String>>()
        .map_err(|error| no_server(error.to_string()))?;
    let server = sender_of(&reply).ok_or_else(|| no_server("the answer names no sender".into()))?;
    let proxy = within(zbus::Proxy::new(
        &connection,
        DESTINATION,
        PATH,
        DESTINATION,
    ))
    .await
    .map_err(no_server)?;
    // zbus hears these signals only from the connection that owns the name at the time, and
    // follows a change of owner: a program that only says them is not heard.
    let mut invoked = within(proxy.receive_signal("ActionInvoked"))
        .await
        .map_err(no_server)?;
    let mut closed = within(proxy.receive_signal("NotificationClosed"))
        .await
        .map_err(no_server)?;
    let on_action = hears.clone();
    tokio::spawn(async move {
        while let Some(message) = invoked.next().await {
            if let (Ok((number, key)), Some(server)) = (
                message.body().deserialize::<(u32, String)>(),
                sender_of(&message),
            ) {
                on_action(Heard::Action {
                    server,
                    number,
                    key,
                });
            }
        }
    });
    let on_close = hears.clone();
    tokio::spawn(async move {
        while let Some(message) = closed.next().await {
            if let (Ok((number, _reason)), Some(server)) = (
                message.body().deserialize::<(u32, u32)>(),
                sender_of(&message),
            ) {
                on_close(Heard::Closed { server, number });
            }
        }
    });
    Ok(Link {
        connection,
        actions: capabilities.iter().any(|name| name == "actions"),
        server,
    })
}
