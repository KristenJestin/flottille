//! The desktop's own notifications (#1002, `docs/design.md`, §11): `org.freedesktop.Notifications`
//! on the session bus, which every Linux desktop's notification server answers. The daemon sends
//! them itself, never through `notify-send`, and nothing here is specific to one desktop.
//!
//! This is the one module a platform's sender lives in: another platform's (macOS, Windows) is
//! another implementation of [`Desktop`]'s three calls, `notify`, `close` and `status`, with the
//! [`Heard`] it reports.
//!
//! One notification of Flottille's is one of the desktop's, replaced (`replaces_id`) when it
//! changes and closed when it is answered or read elsewhere, never stacked. The actions are asked
//! for only when the server says it has them (`actions`): a request has Allow and Deny, and every
//! notification's default action opens its session. No sound hint is sent: the sound is
//! Flottille's own, played by the daemon.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;
use zbus::zvariant::Value;

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The name and the launcher entry Flottille's notifications are shown under.
const APP_NAME: &str = "Flottille";
const ENTRY: &str = "flottille";

/// What a notification says to the desktop.
pub struct Outgoing {
    /// The desktop's number of the notification this one replaces, 0 for a new one.
    pub replaces: u32,
    pub summary: String,
    pub body: String,
    /// Allow and Deny: a request.
    pub decides: bool,
    /// Whether it stays until it is answered, rather than leaving after the desktop's own time.
    pub stays: bool,
}

/// What the desktop tells back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// The user chose an action of the notification numbered so: `allow`, `deny` or `default`.
    Action(u32, String),
    /// The notification numbered so is gone: closed by the user, expired, or by the daemon.
    Closed(u32),
}

/// What is done with what the desktop tells back.
pub type Hears = Arc<dyn Fn(Heard) + Send + Sync>;

struct Link {
    connection: zbus::Connection,
    /// Whether the server supports actions.
    actions: bool,
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

    /// Sends a notification, or replaces one: the desktop's number of it. `None` when the desktop
    /// shows nothing.
    pub async fn notify(&self, hears: &Hears, outgoing: &Outgoing) -> Option<u32> {
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
            if outgoing.decides {
                offered.extend(["allow", "Allow", "deny", "Deny"]);
            }
            offered.extend(["default", "Open"]);
        }
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        hints.insert("desktop-entry", Value::from(ENTRY));
        let timeout: i32 = if outgoing.stays { 0 } else { -1 };
        let sent = connection
            .call_method(
                Some(DESTINATION),
                PATH,
                Some(DESTINATION),
                "Notify",
                &(
                    APP_NAME,
                    outgoing.replaces,
                    ENTRY,
                    outgoing.summary.as_str(),
                    outgoing.body.as_str(),
                    offered,
                    hints,
                    timeout,
                ),
            )
            .await;
        match sent.and_then(|reply| reply.body().deserialize::<u32>()) {
            Ok(number) => Some(number),
            Err(error) => {
                // The server left, or the bus: the next notification connects again.
                *self.link.lock().await = None;
                *self.why.lock().unwrap() =
                    Some(format!("no notification server answers ({error})"));
                None
            }
        }
    }

    /// Closes the desktop's notification numbered so: what was answered or read elsewhere is not
    /// left on the screen.
    pub async fn close(&self, number: u32) {
        let connection = match self.link.lock().await.as_ref() {
            Some(link) => link.connection.clone(),
            None => return,
        };
        let _ = connection
            .call_method(
                Some(DESTINATION),
                PATH,
                Some(DESTINATION),
                "CloseNotification",
                &(number,),
            )
            .await;
    }
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
    let connection = zbus::Connection::session()
        .await
        .map_err(|error| format!("no session bus ({error})"))?;
    let capabilities = connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(DESTINATION),
            "GetCapabilities",
            &(),
        )
        .await
        .and_then(|reply| reply.body().deserialize::<Vec<String>>())
        .map_err(|error| format!("no notification server answers on the session bus ({error})"))?;
    let proxy = zbus::Proxy::new(&connection, DESTINATION, PATH, DESTINATION)
        .await
        .map_err(|error| format!("no notification server answers on the session bus ({error})"))?;
    let mut invoked = proxy
        .receive_signal("ActionInvoked")
        .await
        .map_err(|error| format!("no notification server answers on the session bus ({error})"))?;
    let mut closed = proxy
        .receive_signal("NotificationClosed")
        .await
        .map_err(|error| format!("no notification server answers on the session bus ({error})"))?;
    let on_action = hears.clone();
    tokio::spawn(async move {
        while let Some(message) = invoked.next().await {
            if let Ok((number, key)) = message.body().deserialize::<(u32, String)>() {
                on_action(Heard::Action(number, key));
            }
        }
    });
    let on_close = hears.clone();
    tokio::spawn(async move {
        while let Some(message) = closed.next().await {
            if let Ok((number, _reason)) = message.body().deserialize::<(u32, u32)>() {
                on_close(Heard::Closed(number));
            }
        }
    });
    Ok(Link {
        connection,
        actions: capabilities.iter().any(|name| name == "actions"),
    })
}
