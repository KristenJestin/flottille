//! What rings (#1002, `docs/design.md`, §11): each new notification of the centre whose settings
//! say it rings plays its sound, and is told either to the window, which draws it as a toast, or
//! to the desktop, as a system notification of its own. Never both, and nothing at all for the
//! session whose page is open in the focused window: it is in sight.
//!
//! The daemon does it, so that it works with the window closed. The window tells its presence
//! (`presence.set`, `presence.rs`): focused or not, and the session it shows. A notification that
//! is answered or read elsewhere (`notifications.changed`) is withdrawn from the desktop.
//!
//! The keeper of the centre (`notifications.rs`) makes the rows; this follows the rows, from the
//! last one at the daemon's start (what waited before is not rung again), so that a notification
//! is rung once whatever the number of events that tell of it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use flottille_core::notification::Notification;
use flottille_core::secrets;
use flottille_protocol::Event;
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use crate::daemon::Daemon;
use crate::sounds::{Sound, Sounds};
use crate::system_notifications::{Desktop, Heard, Hears, Outgoing};

/// The volume every sound plays at, until the settings of #1003 say another.
const VOLUME: f32 = 1.0;

/// Whether the settings say a notification rings. Until #1003 (on and off, volume, quiet hours,
/// Do not disturb) lands, the interim rule: the requests, the questions, the failures and the
/// reports of a root session (the centre holds no other session's: a child's go to its parent).
/// The one place #1003 plugs in: it answers from the user's settings and the time of day.
fn rings(notification: &Notification) -> bool {
    matches!(
        notification.kind.as_str(),
        "request" | "question" | "failure" | "report"
    )
}

/// The sound of a notification that rings: one for what needs the user, another for the rest.
fn sound_of(notification: &Notification) -> Sound {
    if notification.needs_you {
        Sound::NeedsYou
    } else {
        Sound::Tell
    }
}

/// A notification shown by the desktop.
struct Shown {
    notification: u64,
    /// The request's number, for a request: what Allow and Deny answer.
    request: Option<u64>,
    session: Option<String>,
}

/// What rings, and what it rang.
pub struct Rings {
    desktop: Desktop,
    sounds: Sounds,
    /// By the desktop's own number of each notification.
    shown: Mutex<HashMap<u32, Shown>>,
}

impl Rings {
    pub fn new(paths: &flottille_core::paths::Paths) -> Rings {
        Rings {
            desktop: Desktop::new(),
            sounds: Sounds::open(paths),
            shown: Mutex::new(HashMap::new()),
        }
    }

    /// What `notifications.status` says: whether the desktop shows Flottille's notifications.
    pub fn desktop(&self) -> Value {
        let why = self.desktop.why_not();
        json!({"shows": why.is_none(), "why": why, "sound": self.sounds.why_not()})
    }

    fn desktop_number_of(&self, notification: u64) -> Option<u32> {
        let shown = self.shown.lock().unwrap();
        shown
            .iter()
            .find(|(_, one)| one.notification == notification)
            .map(|(number, _)| *number)
    }
}

/// Follows the centre until the daemon stops, from the notification numbered `from`.
pub async fn run(
    daemon: Arc<Daemon>,
    mut live: broadcast::Receiver<Event>,
    mut from: u64,
    mut stopping: watch::Receiver<bool>,
) {
    let hears: Hears = {
        let daemon = daemon.clone();
        Arc::new(move |heard| on_heard(&daemon, heard))
    };
    // Said at the start when the desktop shows nothing, and kept for Settings.
    daemon.rings.desktop.connect(&hears).await;
    loop {
        let wakes = tokio::select! {
            received = live.recv() => match received {
                Ok(event) => event.name == "notifications.changed",
                // Events were missed: the store has the rows.
                Err(broadcast::error::RecvError::Lagged(_)) => true,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = async { let _ = stopping.wait_for(|stopping| *stopping).await; } => return,
        };
        if wakes {
            follow(&daemon, &hears, &mut from).await;
        }
    }
}

/// Rings the rows that came after `from`, and withdraws those shown that were answered or read.
async fn follow(daemon: &Arc<Daemon>, hears: &Hears, from: &mut u64) {
    let open: Vec<u64> = {
        let shown = daemon.rings.shown.lock().unwrap();
        shown.values().map(|one| one.notification).collect()
    };
    let read = daemon.read(|store| {
        Ok((
            store.notifications_since(*from, 100)?,
            store.notifications_of(&open)?,
        ))
    });
    let Ok((new, kept)) = read else {
        return;
    };
    for (notification, source) in new {
        *from = (*from).max(notification.id);
        // Answered before it rang: nothing to tell.
        if notification.resolved || notification.read_at.is_some() {
            continue;
        }
        ring(daemon, hears, &notification, &source).await;
    }
    for (notification, _) in kept {
        if notification.resolved || notification.read_at.is_some() {
            withdraw(daemon, notification.id).await;
        }
    }
}

/// A notification that is new: its sound, and then the window's or the desktop's.
async fn ring(daemon: &Arc<Daemon>, hears: &Hears, notification: &Notification, source: &str) {
    if !rings(notification) {
        return;
    }
    let focused = daemon.presence.focused();
    // The session whose page is open in a focused window is in sight: it rings nothing.
    if notification.session.is_some()
        && focused
            .iter()
            .any(|window| window.session == notification.session)
    {
        return;
    }
    let sound = sound_of(notification);
    let sounding = daemon.clone();
    tokio::spawn(async move { sounding.rings.sounds.play(sound, VOLUME).await });
    let request = request_of(source);
    if !focused.is_empty() {
        let _ = daemon.emit(
            "notification.rang",
            json!({
                "id": notification.id,
                "in": "window",
                "kind": notification.kind,
                "session": notification.session,
                "project": notification.project,
                "workspace": notification.workspace,
                "title": notification.title,
                "excerpt": notification.excerpt,
                "needs_you": notification.needs_you,
                "request": request,
            }),
        );
        return;
    }
    let outgoing = Outgoing {
        replaces: daemon
            .rings
            .desktop_number_of(notification.id)
            .unwrap_or_default(),
        summary: masked(&notification.title),
        body: body_of(daemon, notification),
        decides: request.is_some(),
        stays: notification.needs_you,
    };
    if let Some(number) = daemon.rings.desktop.notify(hears, &outgoing).await {
        daemon.rings.shown.lock().unwrap().insert(
            number,
            Shown {
                notification: notification.id,
                request,
                session: notification.session.clone(),
            },
        );
    }
}

/// The request a notification's `source` names (`request:3`).
fn request_of(source: &str) -> Option<u64> {
    source.strip_prefix("request:")?.parse().ok()
}

/// A text, masked: what the desktop is told is never a secret.
fn masked(text: &str) -> String {
    secrets::mask(text, &BTreeMap::new())
}

/// The session and project, then the excerpt.
fn body_of(daemon: &Arc<Daemon>, notification: &Notification) -> String {
    let session = notification
        .session
        .as_deref()
        .and_then(|id| daemon.read(|store| store.agent(id)).ok().flatten())
        .and_then(|agent| {
            agent
                .title
                .or(agent.brief)
                .map(|text| flottille_core::notification::first_line(&text))
        });
    let place = match (&notification.project, session) {
        (Some(project), Some(session)) => format!("{project} · {session}"),
        (Some(project), None) => project.clone(),
        (None, Some(session)) => session,
        (None, None) => String::new(),
    };
    let body = match (place.is_empty(), notification.excerpt.is_empty()) {
        (true, _) => notification.excerpt.clone(),
        (false, true) => place,
        (false, false) => format!("{place}\n{}", notification.excerpt),
    };
    masked(&body)
}

/// Takes a notification off the desktop: it was answered or read elsewhere.
async fn withdraw(daemon: &Arc<Daemon>, notification: u64) {
    let Some(number) = daemon.rings.desktop_number_of(notification) else {
        return;
    };
    daemon.rings.shown.lock().unwrap().remove(&number);
    daemon.rings.desktop.close(number).await;
}

/// What the user does with a notification of the desktop.
fn on_heard(daemon: &Arc<Daemon>, heard: Heard) {
    match heard {
        Heard::Closed(number) => {
            daemon.rings.shown.lock().unwrap().remove(&number);
        }
        Heard::Action(number, key) => {
            let (request, session) = {
                let shown = daemon.rings.shown.lock().unwrap();
                match shown.get(&number) {
                    Some(one) => (one.request, one.session.clone()),
                    None => return,
                }
            };
            let daemon = daemon.clone();
            tokio::spawn(async move {
                match (key.as_str(), request, session) {
                    ("allow", Some(number), _) => {
                        let _ = crate::approvals::call(
                            daemon,
                            "approval.approve",
                            json!({"number": number}),
                        )
                        .await;
                    }
                    ("deny", Some(number), _) => {
                        let _ = crate::approvals::call(
                            daemon,
                            "approval.deny",
                            json!({"number": number}),
                        )
                        .await;
                    }
                    ("default", _, Some(session)) => open_session(&daemon.paths, &session),
                    _ => {}
                }
            });
        }
    }
}

/// Brings the window forward on a session, or starts it there: what `flottille open` does, and
/// the default action of a desktop notification.
pub fn open_session(paths: &flottille_core::paths::Paths, session: &str) {
    if flottille_window::instance::open_session(paths, session).is_ok_and(|taken| taken) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let program = crate::client::program_to_start(exe);
    let mut command = std::process::Command::new(&program);
    command
        .args(["window", "--open", &format!("session:{session}")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match crate::children::spawn_std(&mut command) {
        // Waited for on a thread of its own, so that the window leaves no zombie behind it.
        Ok(mut window) => {
            std::thread::spawn(move || window.wait());
        }
        Err(error) => eprintln!("flottille daemon: the window cannot be started: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(kind: &str, needs_you: bool) -> Notification {
        Notification {
            id: 1,
            at: 0,
            kind: kind.to_owned(),
            session: Some("s".into()),
            project: Some("app".into()),
            workspace: None,
            title: "t".into(),
            excerpt: "e".into(),
            read_at: None,
            resolved: false,
            needs_you,
        }
    }

    #[test]
    fn the_requests_questions_failures_and_reports_ring_and_the_rest_does_not() {
        for kind in ["request", "question", "failure", "report"] {
            assert!(rings(&notification(kind, false)), "{kind}");
        }
        for kind in [
            "vault",
            "pull_request",
            "ticket",
            "paused",
            "unchecked",
            "dropped",
        ] {
            assert!(!rings(&notification(kind, false)), "{kind}");
        }
    }

    #[test]
    fn what_needs_the_user_has_one_sound_and_the_rest_another() {
        assert_eq!(sound_of(&notification("request", true)), Sound::NeedsYou);
        assert_eq!(sound_of(&notification("question", true)), Sound::NeedsYou);
        assert_eq!(sound_of(&notification("failure", false)), Sound::Tell);
        assert_eq!(sound_of(&notification("report", false)), Sound::Tell);
    }

    #[test]
    fn a_request_is_named_by_its_source() {
        assert_eq!(request_of("request:12"), Some(12));
        assert_eq!(request_of("question:abc:4"), None);
        assert_eq!(request_of("request:x"), None);
    }
}
