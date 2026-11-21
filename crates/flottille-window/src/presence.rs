//! The window tells the local daemon its presence (#1002, `presence.set`): whether it is focused
//! and which session its page shows, so that the daemon sends the desktop a notification only
//! when nobody is looking, and tells the focused window to draw a toast of its own otherwise.
//!
//! A thread holds one connection to the daemon for as long as the window lives: the daemon keeps
//! the presence as long as that connection is open, so a window that closes or dies is not
//! counted as there. The state is said when it changes and again each [`AGAIN`], so that a daemon
//! that started again is told, and a connection that broke is made again. The same line says the
//! toasts the window could not draw (`notifications.unshown`), for the desktop to show.

use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use serde_json::json;

use crate::link::{Conn, Connect};

/// How long before the state is said again, whether it changed or not.
const AGAIN: Duration = Duration::from_secs(30);

/// What the window says of itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub focused: bool,
    /// The session whose page is open (its root agent's id).
    pub session: Option<String>,
}

/// What the thread says.
enum Said {
    State(State),
    /// The notification whose toast was not drawn.
    Unshown(u64),
}

/// The line to the daemon, made once for the window.
pub struct Presence {
    tell: Sender<Said>,
}

impl Presence {
    /// Starts the thread; it connects when it has something to say, to the daemon of this
    /// machine (`connect`: never the machine shown, which may be another).
    pub fn spawn(connect: Connect) -> Presence {
        let (tell, told) = channel::<Said>();
        std::thread::spawn(move || {
            let mut conn: Option<Conn> = None;
            let mut state: Option<State> = None;
            loop {
                let mut unshown = Vec::new();
                match told.recv_timeout(AGAIN) {
                    Ok(first) => {
                        // Only the latest state matters: what came in between is not said.
                        for said in std::iter::once(first).chain(told.try_iter()) {
                            match said {
                                Said::State(latest) => state = Some(latest),
                                Said::Unshown(id) => unshown.push(id),
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    // The window is gone: its connection closes with this thread.
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
                let Some(state) = &state else { continue };
                if conn.is_none() {
                    conn = connect().ok().and_then(|stream| Conn::new(stream).ok());
                }
                let asked = std::iter::once((
                    "presence.set",
                    json!({"focused": state.focused, "session": state.session}),
                ))
                .chain(
                    unshown
                        .into_iter()
                        .map(|id| ("notifications.unshown", json!({"id": id}))),
                );
                for (method, params) in asked {
                    if let Some(open) = conn.as_mut()
                        && open
                            .ask_within(method, params, Duration::from_secs(10))
                            .is_err()
                    {
                        conn = None;
                    }
                }
            }
        });
        Presence { tell }
    }

    pub fn tell(&self, state: State) {
        let _ = self.tell.send(Said::State(state));
    }

    /// The toast of notification `id` was not drawn: the window lost the focus before it came.
    pub fn unshown(&self, id: u64) {
        let _ = self.tell.send(Said::Unshown(id));
    }
}
