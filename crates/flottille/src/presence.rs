//! The presence of the window (#1002): whether it is focused and which session its page shows,
//! told by the window on a connection it keeps open (`presence.set`), so that the daemon sends
//! the desktop a notification only when nobody is looking, and none at all for the session in
//! sight. A window that closes, or dies, closes its connection: its presence goes with it, and
//! the daemon counts the user as away. The same presence the remote design uses
//! (`docs/remote.md`, "Notifications"), seen from this machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::daemon::Daemon;

/// What one window says of itself.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presence {
    pub focused: bool,
    /// The session whose page is open (its root agent's id), if a session's page is.
    #[serde(default)]
    pub session: Option<String>,
}

/// The windows connected, by the connection that tells each.
#[derive(Default)]
pub struct Presences(Mutex<Windows>);

#[derive(Default)]
struct Windows {
    next: u64,
    by_connection: HashMap<u64, Presence>,
}

impl Presences {
    /// The windows that are focused.
    pub fn focused(&self) -> Vec<Presence> {
        let windows = self.0.lock().unwrap();
        windows
            .by_connection
            .values()
            .filter(|presence| presence.focused)
            .cloned()
            .collect()
    }

    /// What the daemon knows of the window: a focused one's presence, else any one's, else
    /// nothing.
    pub fn current(&self) -> Option<Presence> {
        let windows = self.0.lock().unwrap();
        windows
            .by_connection
            .values()
            .find(|presence| presence.focused)
            .or_else(|| windows.by_connection.values().next())
            .cloned()
    }
}

/// A connection's presence, held while it is open: dropped with the connection.
pub struct Hold {
    daemon: Arc<Daemon>,
    number: u64,
}

impl Hold {
    pub fn new(daemon: &Arc<Daemon>, presence: Presence) -> Hold {
        let mut windows = daemon.presence.0.lock().unwrap();
        windows.next += 1;
        let number = windows.next;
        windows.by_connection.insert(number, presence);
        Hold {
            daemon: daemon.clone(),
            number,
        }
    }

    pub fn set(&self, presence: Presence) {
        let mut windows = self.daemon.presence.0.lock().unwrap();
        windows.by_connection.insert(self.number, presence);
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut windows = self.daemon.presence.0.lock().unwrap();
        windows.by_connection.remove(&self.number);
    }
}
