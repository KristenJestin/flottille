//! The user's phones, told through the hub (`docs/remote.md`, "Notifications"): no push service.
//! What the notification centre keeps for the user (a request held for them, a question to them, a
//! root's report or failure) waits `DELAY_MS`; then, unless it was seen or answered meanwhile, it
//! is told to the hub, which tells the user's phones, when no client of the user had an input in
//! the last `AWAY_MS` (or always, as the user chose: `[remote] notify`). Once told, what is seen or
//! answered at a client tells `dismissed`. What travels is a kind and ids only, never a title, an
//! excerpt, a path or a command: the hub stays a relay that reads nothing more.
//!
//! The presence is each client's: the window says when it is focused and when the user gave it an
//! input (`client.presence`), stamped with the daemon's clock and told to the hub, which tells the
//! runners every client's. With the hub unreachable nothing is told, and nothing waits for it: a
//! notice due then is dropped.
//!
//! What is decided is kept in memory: a row is decided once, at its delay, and what came before
//! the daemon started is never told.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use flottille_core::store::now_ms;
use flottille_link::{Ask, Asker, Notice, NoticeKind};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::daemon::Daemon;
use crate::projects::Failure;

/// How long a thing to notify waits before the phone is told of it.
pub const DELAY_MS: u64 = 30_000;
/// An input at a client this recent means the user is there: the phone is not told.
pub const AWAY_MS: u64 = 2 * 60_000;
/// An input said to come later than this is taken from a clock that is wrong, and not counted:
/// it would keep the phone silent until that time.
const AHEAD_MS: u64 = 60_000;
/// The rows of the centre looked at, the last ones.
const LOOKED: usize = 200;
/// How long the hub may take to take a notice.
const ASK_WAIT: Duration = Duration::from_secs(10);

/// What the daemon decided for the phones.
#[derive(Default)]
pub struct Phone {
    state: Mutex<State>,
    /// The notices on their way to the hub, told in order by one task.
    out: Mutex<Option<mpsc::UnboundedSender<(Asker, Notice)>>>,
}

#[derive(Default)]
struct State {
    /// The store's clock at the daemon's start: a row from before is never told.
    since: Option<u64>,
    /// Each row decided: the notice told for it until it is dismissed, `None` when nothing waits.
    decided: HashMap<u64, Option<Notice>>,
}

/// The kind a row of the centre is told as, when it is one the phone is told.
fn kind_of(row: &str) -> Option<NoticeKind> {
    match row {
        "request" => Some(NoticeKind::Request),
        "question" => Some(NoticeKind::Question),
        "report" => Some(NoticeKind::Report),
        "failure" => Some(NoticeKind::Failure),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Presence {
    /// Whether the client is in front of the user (the window focused).
    focused: bool,
    /// The user gave it an input just now.
    #[serde(default)]
    input: bool,
}

/// `client.presence`: what a client says of the user, told to the hub.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    if method != "client.presence" {
        return Err(Failure::new(
            flottille_protocol::code::UNKNOWN_METHOD,
            format!("unknown method `{method}`"),
        ));
    }
    let said: Presence = serde_json::from_value(params)?;
    if let Some(control) = control(daemon) {
        control.tell_presence(said.focused, said.input.then(now_ms));
    }
    Ok(json!({}))
}

fn control(daemon: &Daemon) -> Option<std::sync::Arc<crate::hub_session::Control>> {
    let remote = daemon.link()?.ok()?;
    remote.control.get().cloned()
}

/// Whether no client of the user had an input in the last `AWAY_MS`, as this machine's clients
/// said it and the hub told every other's but the phones'. An input said to come more than
/// `AHEAD_MS` from now is not counted. With no session to the hub, the user is away.
fn away(control: &crate::hub_session::Control, now: u64) -> bool {
    let recent =
        |at_ms: u64| at_ms <= now.saturating_add(AHEAD_MS) && now.saturating_sub(at_ms) < AWAY_MS;
    if control.local_input().is_some_and(recent) {
        return false;
    }
    !control
        .presence_at_clients()
        .iter()
        .any(|told| recent(told.last_input.saturating_mul(1000)))
}

/// Tells the phones what is due and dismisses what was seen or answered. Called whenever the
/// centre changes and by the sweep.
pub fn check(daemon: &Daemon) {
    let Ok(rows) = daemon.read(|store| store.notifications(None, LOOKED, false)) else {
        return;
    };
    let now = now_ms();
    let control = control(daemon);
    let mut told = Vec::new();
    {
        let mut state = daemon.phone.state.lock().unwrap();
        let since = *state.since.get_or_insert(now);
        let seen: HashMap<u64, bool> = rows
            .iter()
            .map(|row| (row.id, row.read_at.is_some()))
            .collect();
        // A row read, answered, or gone from the centre is dismissed on the phones it was told.
        for (id, notice) in state.decided.iter_mut() {
            if seen.get(id).copied().unwrap_or(true)
                && let Some(was) = notice.take()
            {
                told.push(Notice {
                    kind: NoticeKind::Dismissed,
                    ..was
                });
            }
        }
        let listed: HashSet<u64> = seen.keys().copied().collect();
        state
            .decided
            .retain(|id, notice| notice.is_some() || listed.contains(id));
        // Read once, when a row is due.
        let mut tells = None;
        for row in &rows {
            let Some(kind) = kind_of(&row.kind) else {
                continue;
            };
            if state.decided.contains_key(&row.id) {
                continue;
            }
            if row.at < since || row.read_at.is_some() {
                state.decided.insert(row.id, None);
                continue;
            }
            if now < row.at.saturating_add(DELAY_MS) {
                continue;
            }
            let tell = *tells.get_or_insert_with(|| {
                crate::remote::notify_always(&daemon.paths)
                    || control.as_ref().is_none_or(|control| away(control, now))
            });
            let notice = tell.then(|| Notice {
                kind,
                notification: row.id,
                session: row.session.as_deref().and_then(|id| id.parse().ok()),
            });
            told.extend(notice.clone());
            state.decided.insert(row.id, notice);
        }
    }
    // With no session to a hub that takes notices, nothing is told and nothing waits for it.
    let Some(asker) = control.and_then(|control| control.notifier()) else {
        return;
    };
    for notice in told {
        send(&daemon.phone, asker.clone(), notice);
    }
}

/// `notice` to the hub, after those before it: one task tells them in order.
fn send(phone: &Phone, asker: Asker, notice: Notice) {
    let mut out = phone.out.lock().unwrap();
    if out.as_ref().is_none_or(|out| out.is_closed()) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (sender, mut notices) = mpsc::unbounded_channel::<(Asker, Notice)>();
        runtime.spawn(async move {
            while let Some((asker, notice)) = notices.recv().await {
                let asked = asker.ask(Ask::Notify { notice });
                if let Ok(Err(error)) = tokio::time::timeout(ASK_WAIT, asked).await {
                    eprintln!("flottille daemon: the hub did not take a notice: {error}");
                }
            }
        });
        *out = Some(sender);
    }
    if let Some(out) = out.as_ref() {
        let _ = out.send((asker, notice));
    }
}
