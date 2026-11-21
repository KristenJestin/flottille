//! A routine (`docs/design.md`, "Routines"): work the daemon starts by itself, as a new root
//! session, when its trigger comes; and its runs, each a session but a skipped one.

use serde_json::{Value, json};

/// The modes of a routine: what its run's session may do.
pub const READ_ONLY: &str = "read_only";
pub const ASK: &str = "ask";
pub const AUTO: &str = "auto";
pub const MODES: [&str; 3] = [READ_ONLY, ASK, AUTO];

/// Where a run's result rings: in the notification centre when it has something to say, or
/// never (its history only).
pub const RINGS_SOMETHING: &str = "something";
pub const RINGS_NEVER: &str = "never";
pub const RINGS: [&str; 2] = [RINGS_SOMETHING, RINGS_NEVER];

/// A run missed while the daemon was off is still run this many hours after it was due.
pub const MISSED_HOURS: u64 = 12;

/// The outcomes of a run.
pub const RUNNING: &str = "running";
/// It ended with something to say.
pub const SOMETHING: &str = "something";
/// It ended with nothing to say.
pub const NOTHING: &str = "nothing";
/// It said something, and its session goes on as any session.
pub const GOES_ON: &str = "goes_on";
/// It did not start: the last run still ran, or it was missed too long ago.
pub const SKIPPED: &str = "skipped";
/// Its session ended or failed without its result, or could not start.
pub const FAILED: &str = "failed";

#[derive(Clone, Debug, PartialEq)]
pub struct Routine {
    /// Opaque, given by the daemon.
    pub id: String,
    /// The user's, unique.
    pub name: String,
    pub project: String,
    /// The user's own words: the first message of each run's session.
    pub prompt: String,
    /// The agent each run starts on; `None` for the defaults of a new session.
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `weekdays 08:30`, `every day 07:00`, `mondays 09:00`.
    pub schedule: String,
    /// The time zone the schedule is read in, an IANA name; `None` for the machine's.
    pub zone: Option<String>,
    /// One of `MODES`.
    pub mode: String,
    /// One of `RINGS`.
    pub rings: String,
    pub paused: bool,
    /// How long after its time a run missed while the daemon was off still runs.
    pub missed_hours: u64,
    /// When its next run is due; `None` while it is paused.
    pub next_at: Option<u64>,
    /// Why its next run comes later than its schedule says (a spent quota), while it does.
    pub delayed: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Routine {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "project": self.project,
            "prompt": self.prompt,
            "agent": {"provider": self.provider, "model": self.model, "effort": self.effort},
            "schedule": self.schedule,
            "zone": self.zone,
            "mode": self.mode,
            "rings": self.rings,
            "paused": self.paused,
            "missed_hours": self.missed_hours,
            "next_at": self.next_at,
            "delayed": self.delayed,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        })
    }
}

/// One run of a routine.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub id: u64,
    /// The routine's id.
    pub routine: String,
    /// When it was due (when it was asked, for a run started by hand).
    pub due_at: u64,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    /// Its root session; `None` for a skipped run.
    pub session: Option<String>,
    /// The routine's mode when it started: a later edit does not change a session under way.
    pub mode: String,
    /// One of the outcomes.
    pub outcome: String,
    /// The one line its result said.
    pub summary: Option<String>,
    /// Why it was skipped or failed.
    pub why: Option<String>,
    /// When the user opened its result, if they did: a result with something to say that was not
    /// opened is unread.
    pub opened_at: Option<u64>,
}

impl Run {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "routine": self.routine,
            "due_at": self.due_at,
            "started_at": self.started_at,
            "ended_at": self.ended_at,
            "session": self.session,
            "mode": self.mode,
            "outcome": self.outcome,
            "summary": self.summary,
            "why": self.why,
            "opened": self.opened_at.is_some(),
        })
    }
}
