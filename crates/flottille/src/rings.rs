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
use crate::system_notifications::{Desktop, Heard, Hears, Outgoing, Shown};

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

/// A notification of the centre the desktop shows.
struct Showing {
    notification: u64,
    /// The request's number, for a request: what Deny answers, and Allow when it was offered.
    request: Option<u64>,
    /// Whether Allow was offered: the notification said the whole call.
    allows: bool,
    session: Option<String>,
}

/// What rings, and what it rang.
pub struct Rings {
    desktop: Desktop,
    sounds: Sounds,
    /// By the server that shows each and its number there.
    shown: Mutex<HashMap<Shown, Showing>>,
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

    fn shown_as(&self, notification: u64) -> Option<Shown> {
        let shown = self.shown.lock().unwrap();
        shown
            .iter()
            .find(|(_, one)| one.notification == notification)
            .map(|(shown, _)| shown.clone())
    }
}

/// What is done with what the desktop tells back.
fn hears_of(daemon: &Arc<Daemon>) -> Hears {
    let daemon = daemon.clone();
    Arc::new(move |heard| on_heard(&daemon, heard))
}

/// Follows the centre until the daemon stops, from the notification numbered `from`.
pub async fn run(
    daemon: Arc<Daemon>,
    mut live: broadcast::Receiver<Event>,
    mut from: u64,
    mut stopping: watch::Receiver<bool>,
) {
    let hears = hears_of(&daemon);
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
    let whole = request.and_then(|number| whole_of(daemon, number));
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
                "whole": whole,
            }),
        );
        return;
    }
    to_the_desktop(daemon, hears, notification, request, whole).await;
}

/// A notification told to the desktop. Allow is offered on what the notification says whole
/// alone, and never while a confined command may be what answers on the bus: where the kernel
/// gives it no private mount namespace, or the bus found at the start was replaced, a run could
/// own the server's name and click Allow on its own request (`docs/vault.md`, "The gaps").
async fn to_the_desktop(
    daemon: &Arc<Daemon>,
    hears: &Hears,
    notification: &Notification,
    request: Option<u64>,
    whole: Option<String>,
) {
    let allows = whole.is_some() && !bus_reachable_by_runs();
    let outgoing = Outgoing {
        summary: masked(&notification.title),
        body: body_of(daemon, notification, whole.as_deref()),
        allows,
        denies: request.is_some(),
        stays: notification.needs_you,
    };
    if let Some(shown) = daemon.rings.desktop.notify(hears, &outgoing).await {
        daemon.rings.shown.lock().unwrap().insert(
            shown,
            Showing {
                notification: notification.id,
                request,
                allows,
                session: notification.session.clone(),
            },
        );
    }
}

/// Whether a confined command may reach the session bus: the kernel gives no private mount
/// namespace, or what is at the bus's path is not what the daemon found at its start.
fn bus_reachable_by_runs() -> bool {
    crate::mounts::namespaces() == crate::mounts::Namespaces::Refused
        || crate::mounts::bus_replaced()
}

/// The call of request `number` and its reasons, when they can be said whole
/// (`notification::whole`), masked with the daemon's secrets too.
fn whole_of(daemon: &Arc<Daemon>, number: u64) -> Option<String> {
    let request = daemon.read(|store| store.request(number)).ok().flatten()?;
    let whole = flottille_core::notification::whole(&request)?;
    (masked(&whole) == whole).then_some(whole)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Unshown {
    id: u64,
}

/// `notifications.unshown {"id"}`: answered at once, the desktop told after.
pub fn unshown_asked(
    daemon: &Arc<Daemon>,
    params: Value,
) -> Result<Value, crate::projects::Failure> {
    let asked: Unshown = serde_json::from_value(params)
        .map_err(|error| crate::projects::Failure::new("invalid-params", error.to_string()))?;
    let daemon = daemon.clone();
    tokio::spawn(async move {
        if let Err(error) = unshown(&daemon, asked.id).await {
            eprintln!(
                "flottille daemon: notification {} not shown: {error}",
                asked.id
            );
        }
    });
    Ok(json!({}))
}

/// The window could not draw the toast of `notification` (it lost the focus before it came):
/// the desktop shows it instead, with no second sound (`notifications.unshown`).
pub async fn unshown(daemon: &Arc<Daemon>, notification: u64) -> flottille_core::Result<()> {
    let found = daemon.read(|store| store.notifications_of(&[notification]))?;
    let Some((notification, source)) = found.into_iter().next() else {
        return Ok(());
    };
    if notification.resolved
        || notification.read_at.is_some()
        || !rings(&notification)
        || daemon.rings.shown_as(notification.id).is_some()
    {
        return Ok(());
    }
    let hears = hears_of(daemon);
    let request = request_of(&source);
    let whole = request.and_then(|number| whole_of(daemon, number));
    to_the_desktop(daemon, &hears, &notification, request, whole).await;
    Ok(())
}

/// The request a notification's `source` names (`request:3`).
fn request_of(source: &str) -> Option<u64> {
    source.strip_prefix("request:")?.parse().ok()
}

/// A text, masked with the daemon's secrets as every text an agent or the user gave is: what the
/// desktop is told, which its server may keep in a history, is never a secret.
fn masked(text: &str) -> String {
    let env: BTreeMap<_, _> = std::env::vars_os().collect();
    secrets::mask(text, &env)
}

/// The session and project, then the call whole when it is said so, else the excerpt.
fn body_of(daemon: &Arc<Daemon>, notification: &Notification, whole: Option<&str>) -> String {
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
    let said = whole.unwrap_or(&notification.excerpt);
    let body = match (place.is_empty(), said.is_empty()) {
        (true, _) => said.to_owned(),
        (false, true) => place,
        (false, false) => format!("{place}\n{said}"),
    };
    masked(&body)
}

/// Takes a notification off the desktop: it was answered or read elsewhere.
async fn withdraw(daemon: &Arc<Daemon>, notification: u64) {
    let Some(shown) = daemon.rings.shown_as(notification) else {
        return;
    };
    daemon.rings.shown.lock().unwrap().remove(&shown);
    daemon.rings.desktop.close(&shown).await;
}

/// What the user does with a notification of the desktop. Only what the notification offered,
/// of the server that shows it, is done: Allow where it was offered, Deny on a request, Open.
fn on_heard(daemon: &Arc<Daemon>, heard: Heard) {
    match heard {
        Heard::Closed { server, number } => {
            daemon.rings.shown.lock().unwrap().remove(&(server, number));
        }
        Heard::Action {
            server,
            number,
            key,
        } => {
            let (request, allows, session) = {
                let shown = daemon.rings.shown.lock().unwrap();
                match shown.get(&(server, number)) {
                    Some(one) => (one.request, one.allows, one.session.clone()),
                    None => return,
                }
            };
            let daemon = daemon.clone();
            tokio::spawn(async move {
                let (method, number) = match (key.as_str(), request, session) {
                    ("allow", Some(number), _) if allows => ("approval.approve", number),
                    ("deny", Some(number), _) => ("approval.deny", number),
                    ("default", _, Some(session)) => {
                        open_session(&daemon.paths, &session);
                        return;
                    }
                    _ => return,
                };
                let answered =
                    crate::approvals::call(daemon.clone(), method, json!({"number": number})).await;
                if let Err(failure) = answered {
                    refused(&daemon, number, &failure.message).await;
                }
            });
        }
    }
}

/// An answer from a notification that the daemon refused (decided already, elsewhere): said in
/// the log and on the desktop, so that the click is not lost without a word.
async fn refused(daemon: &Arc<Daemon>, number: u64, why: &str) {
    let why = masked(why);
    eprintln!("flottille daemon: the answer to request #{number} from a notification: {why}");
    let outgoing = Outgoing {
        summary: format!("Request #{number} was not answered"),
        body: why,
        allows: false,
        denies: false,
        stays: false,
    };
    daemon
        .rings
        .desktop
        .notify(&hears_of(daemon), &outgoing)
        .await;
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
