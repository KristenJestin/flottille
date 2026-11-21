//! The presence of the window (#1002): whether it is focused and which session its page shows,
//! told by the window on a connection it keeps open (`presence.set`), so that the daemon sends
//! the desktop a notification only when nobody is looking, and none at all for the session in
//! sight. A window that closes, or dies, closes its connection: its presence goes with it, and
//! the daemon counts the user as away. The same presence the remote design uses
//! (`docs/remote.md`, "Notifications"), seen from this machine.
//!
//! The window says it again every 30 seconds: a focus it has not said again for [`STALE_MS`] is a
//! window that hangs, and counts as away, so that what rings is not drawn by a window nobody
//! sees.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use flottille_core::store::now_ms;

use crate::daemon::Daemon;

/// How long a presence holds without being told again: two of the window's 30 seconds, and room.
pub const STALE_MS: u64 = 75_000;

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
    /// Each window's presence, and when it was last told.
    by_connection: HashMap<u64, (Presence, u64)>,
}

impl Presences {
    /// The windows that are focused, and said so lately.
    pub fn focused(&self) -> Vec<Presence> {
        let now = now_ms();
        let windows = self.0.lock().unwrap();
        windows
            .by_connection
            .values()
            .filter(|(presence, told)| presence.focused && now.saturating_sub(*told) < STALE_MS)
            .map(|(presence, _)| presence.clone())
            .collect()
    }

    /// What the daemon knows of the window: a focused one's presence, else any one's, else
    /// nothing.
    pub fn current(&self) -> Option<Presence> {
        let windows = self.0.lock().unwrap();
        windows
            .by_connection
            .values()
            .map(|(presence, _)| presence)
            .find(|presence| presence.focused)
            .or_else(|| {
                windows
                    .by_connection
                    .values()
                    .map(|(presence, _)| presence)
                    .next()
            })
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
        windows.by_connection.insert(number, (presence, now_ms()));
        Hold {
            daemon: daemon.clone(),
            number,
        }
    }

    pub fn set(&self, presence: Presence) {
        let mut windows = self.daemon.presence.0.lock().unwrap();
        windows
            .by_connection
            .insert(self.number, (presence, now_ms()));
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut windows = self.daemon.presence.0.lock().unwrap();
        windows.by_connection.remove(&self.number);
    }
}
