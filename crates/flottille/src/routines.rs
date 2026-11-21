//! Routines (`docs/design.md`, "Routines"): work the daemon starts by itself. A routine is a
//! prompt, a project, an agent, a schedule, a mode and where its result rings; each run is a new
//! root session, started as a click would start it, one-shot: it ends with its result
//! (`routine_result`), or goes on as any session when its work is not done.
//!
//! No timer wakes an agent, and routines add none to the daemon: the next run due of every
//! routine is one deadline (`Daemon::routine_due`), which the sweep already running reads, so
//! the daemon stays at rest between runs. A run missed while the daemon was off is the run found
//! due late: it runs once, within its routine's limit, and the next is the schedule's next after
//! now. A run due while the last still runs is skipped and said; one due while the quota of its
//! provider is spent waits for the reset.
//!
//! A read-only routine's session reads and writes nothing: every call not in `reads` is refused,
//! in every mode, whatever its prompt says (`refuses`).
//!
//! Every time is read from the store's clock, which a test moves ahead (`test.advance`).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use flottille_core::agent::{AgentState, GateMode, new_id};
use flottille_core::approval;
use flottille_core::routine::{self, Routine, Run};
use flottille_core::secrets;
use flottille_core::store::{NewRun, Store, now_ms};
use jiff::civil::{Time, Weekday};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::deliveries;
use crate::mcp::{Grant, Outcome, arguments};
use crate::projects::Failure;
use crate::{agents, providers, roles, tree};

/// The runs `routine.show` gives, the last first.
const HISTORY: usize = 50;
/// The most characters of a result's summary: one line.
const SUMMARY_CHARS: usize = 300;
/// The delivery that reminds a run's session to give its result.
const REMINDER: &str = "routine-reminder";

// The schedule.

/// When a routine runs: days of the week and a time of day, never a cron line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Schedule {
    /// Monday first.
    days: [bool; 7],
    time: Time,
}

const DAYS: [(&str, Weekday); 7] = [
    ("monday", Weekday::Monday),
    ("tuesday", Weekday::Tuesday),
    ("wednesday", Weekday::Wednesday),
    ("thursday", Weekday::Thursday),
    ("friday", Weekday::Friday),
    ("saturday", Weekday::Saturday),
    ("sunday", Weekday::Sunday),
];

const SCHEDULE_SAID: &str = "a schedule is the days and an hour: `every day 07:00`, \
    `weekdays 08:30`, `mondays 09:00`";

impl Schedule {
    /// `every day 07:00`, `weekdays 08:30`, `weekends 10:00`, `mondays 09:00` (`monday` too).
    pub fn parse(text: &str) -> Result<Schedule, String> {
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        let (days, time) = match words.as_slice() {
            [every, day, time] if every == "every" && day == "day" => ([true; 7], time),
            [days, time] => {
                let days = match days.as_str() {
                    "daily" => [true; 7],
                    "weekdays" => [true, true, true, true, true, false, false],
                    "weekends" => [false, false, false, false, false, true, true],
                    day => {
                        let day = day.strip_suffix('s').unwrap_or(day);
                        let at = DAYS
                            .iter()
                            .position(|(name, _)| *name == day)
                            .ok_or_else(|| format!("{text}: {SCHEDULE_SAID}"))?;
                        let mut days = [false; 7];
                        days[at] = true;
                        days
                    }
                };
                (days, time)
            }
            _ => return Err(format!("{text}: {SCHEDULE_SAID}")),
        };
        let time = time
            .split_once(':')
            .filter(|(_, minute)| minute.len() == 2)
            .and_then(|(hour, minute)| {
                Time::new(hour.parse().ok()?, minute.parse().ok()?, 0, 0).ok()
            })
            .ok_or_else(|| format!("{text}: the hour is HH:MM, as 08:30 ({SCHEDULE_SAID})"))?;
        Ok(Schedule { days, time })
    }

    /// The first time after `after` it names, in `zone`.
    pub fn next_after(&self, zone: &TimeZone, after: u64) -> Option<u64> {
        let here = Timestamp::from_millisecond(i64::try_from(after).ok()?)
            .ok()?
            .to_zoned(zone.clone());
        let mut date = here.date();
        for _ in 0..8 {
            let day = DAYS
                .iter()
                .position(|(_, weekday)| *weekday == date.weekday())?;
            if self.days[day] {
                let at: Zoned = date.to_datetime(self.time).to_zoned(zone.clone()).ok()?;
                if at.timestamp() > here.timestamp() {
                    return u64::try_from(at.timestamp().as_millisecond()).ok();
                }
            }
            date = date.tomorrow().ok()?;
        }
        None
    }
}

impl Schedule {
    /// The last time at or before `now` it names, in `zone`.
    pub fn last_at_or_before(&self, zone: &TimeZone, now: u64) -> Option<u64> {
        let here = Timestamp::from_millisecond(i64::try_from(now).ok()?)
            .ok()?
            .to_zoned(zone.clone());
        let mut date = here.date();
        for _ in 0..8 {
            let day = DAYS
                .iter()
                .position(|(_, weekday)| *weekday == date.weekday())?;
            if self.days[day] {
                let at: Zoned = date.to_datetime(self.time).to_zoned(zone.clone()).ok()?;
                if at.timestamp() <= here.timestamp() {
                    return u64::try_from(at.timestamp().as_millisecond()).ok();
                }
            }
            date = date.yesterday().ok()?;
        }
        None
    }
}

/// The routine's time zone: its own, else the machine's.
fn zone_of(routine: &Routine) -> TimeZone {
    routine
        .zone
        .as_deref()
        .and_then(|name| TimeZone::get(name).ok())
        .unwrap_or_else(TimeZone::system)
}

/// The next run of `routine` after `after`, by its schedule; `None` while it is paused.
fn next_of(routine: &Routine, after: u64) -> Option<u64> {
    if routine.paused {
        return None;
    }
    Schedule::parse(&routine.schedule)
        .ok()?
        .next_after(&zone_of(routine), after)
}

// The methods.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Add {
    name: String,
    project: String,
    prompt: String,
    schedule: String,
    zone: Option<String>,
    mode: Option<String>,
    rings: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    missed_hours: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    routine: String,
    name: Option<String>,
    project: Option<String>,
    prompt: Option<String>,
    schedule: Option<String>,
    zone: Option<String>,
    mode: Option<String>,
    rings: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    missed_hours: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Named {
    routine: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    project: Option<String>,
}

/// Answers a `routine.*` request of the user's.
pub fn call(daemon: &Arc<Daemon>, method: &str, params: Value) -> Result<Value, Failure> {
    let answer = match method {
        "routine.list" => {
            let asked: List = match params {
                Value::Null => List::default(),
                params => serde_json::from_value(params)?,
            };
            let routines = daemon.read(|store| store.routines(asked.project.as_deref()))?;
            let mut listed = Vec::new();
            for routine in routines {
                let last = daemon.read(|store| store.routine_runs(&routine.id, 1))?;
                let mut shown = routine.to_json();
                shown["last"] = last.first().map_or(Value::Null, Run::to_json);
                listed.push(shown);
            }
            json!({"routines": listed})
        }
        "routine.show" => {
            let asked: Named = serde_json::from_value(params)?;
            let routine = find(daemon, &asked.routine)?;
            let runs = daemon.read(|store| store.routine_runs(&routine.id, HISTORY))?;
            json!({"routine": routine.to_json(),
                "runs": runs.iter().map(Run::to_json).collect::<Vec<_>>()})
        }
        "routine.add" => {
            let asked: Add = serde_json::from_value(params)?;
            let now = now_ms();
            let mut routine = Routine {
                id: new_id().map_err(|e| Failure::new("unavailable", e.to_string()))?,
                name: asked.name,
                project: asked.project,
                prompt: asked.prompt,
                provider: asked.provider,
                model: asked.model,
                effort: asked.effort,
                schedule: asked.schedule,
                zone: asked.zone,
                mode: asked.mode.unwrap_or_else(|| routine::READ_ONLY.to_owned()),
                rings: asked
                    .rings
                    .unwrap_or_else(|| routine::RINGS_SOMETHING.to_owned()),
                paused: false,
                missed_hours: asked.missed_hours.unwrap_or(routine::MISSED_HOURS),
                next_at: None,
                delayed: None,
                created_at: now,
                updated_at: now,
            };
            checked(daemon, &mut routine)?;
            routine.next_at = next_of(&routine, now);
            daemon.read(|store| store.add_routine(&routine))?;
            changed(daemon, &routine)?
        }
        "routine.edit" => {
            let asked: Edit = serde_json::from_value(params)?;
            let mut routine = find(daemon, &asked.routine)?;
            let schedule_moves = asked.schedule.is_some() || asked.zone.is_some();
            let fields = [
                (&mut routine.name, asked.name),
                (&mut routine.project, asked.project),
                (&mut routine.prompt, asked.prompt),
                (&mut routine.schedule, asked.schedule),
                (&mut routine.mode, asked.mode),
                (&mut routine.rings, asked.rings),
            ];
            for (field, value) in fields {
                if let Some(value) = value {
                    *field = value;
                }
            }
            // An empty text clears what may be left to the defaults.
            for (field, value) in [
                (&mut routine.zone, asked.zone),
                (&mut routine.provider, asked.provider),
                (&mut routine.model, asked.model),
                (&mut routine.effort, asked.effort),
            ] {
                if let Some(value) = value {
                    *field = (!value.is_empty()).then_some(value);
                }
            }
            if let Some(hours) = asked.missed_hours {
                routine.missed_hours = hours;
            }
            checked(daemon, &mut routine)?;
            let now = now_ms();
            if schedule_moves || routine.next_at.is_none() {
                routine.next_at = next_of(&routine, now);
                routine.delayed = None;
            }
            routine.updated_at = now;
            daemon.read(|store| store.update_routine(&routine))?;
            changed(daemon, &routine)?
        }
        "routine.pause" | "routine.resume" => {
            let asked: Named = serde_json::from_value(params)?;
            let mut routine = find(daemon, &asked.routine)?;
            routine.paused = method == "routine.pause";
            let now = now_ms();
            routine.next_at = next_of(&routine, now);
            routine.delayed = None;
            routine.updated_at = now;
            daemon.read(|store| store.update_routine(&routine))?;
            changed(daemon, &routine)?
        }
        "routine.remove" => {
            let asked: Named = serde_json::from_value(params)?;
            let routine = find(daemon, &asked.routine)?;
            daemon.read(|store| store.remove_routine(&routine.id))?;
            daemon.routine_due.store(0, Ordering::Relaxed);
            daemon
                .emit(
                    "routine.changed",
                    json!({"routine": {"id": routine.id, "name": routine.name}, "removed": true,
                        "project": routine.project}),
                )
                .map_err(|error| Failure::new("unavailable", error.to_string()))?;
            json!({"removed": routine.name})
        }
        "routine.run" => {
            let asked: Named = serde_json::from_value(params)?;
            let routine = find(daemon, &asked.routine)?;
            if let Some(last) = running(daemon, &routine) {
                return Err(Failure::refused(format!(
                    "the routine {} runs already, in session {}: a routine never runs twice at \
                     once",
                    routine.name,
                    last.session.as_deref().unwrap_or_default()
                )));
            }
            let run = start(daemon, &routine, now_ms()).map_err(Failure::refused)?;
            json!({"run": run.to_json()})
        }
        method => {
            return Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            ));
        }
    };
    Ok(answer)
}

fn find(daemon: &Daemon, key: &str) -> Result<Routine, Failure> {
    daemon
        .read(|store| store.routine(key))?
        .ok_or_else(|| Failure::new("not-found", format!("there is no routine {key}")))
}

/// What a routine must be before it is kept: a name, a project Flottille knows, a prompt, a
/// schedule it reads, a zone it knows, a mode and a ring it has, an agent that can be chosen.
fn checked(daemon: &Daemon, routine: &mut Routine) -> Result<(), Failure> {
    let invalid = |why: String| Failure::new("invalid-params", why);
    routine.name = routine.name.trim().to_owned();
    if routine.name.is_empty() {
        return Err(invalid("a routine needs a name".into()));
    }
    if routine.prompt.trim().is_empty() {
        return Err(invalid(
            "a routine needs a prompt: what its session is asked".into(),
        ));
    }
    let schedule = Schedule::parse(&routine.schedule).map_err(invalid)?;
    routine.schedule = said(&routine.schedule);
    let _ = schedule;
    if let Some(zone) = &routine.zone
        && TimeZone::get(zone).is_err()
    {
        return Err(invalid(format!(
            "there is no time zone {zone}: name one as Europe/Paris or UTC"
        )));
    }
    if !routine::MODES.contains(&routine.mode.as_str()) {
        return Err(invalid(format!(
            "there is no mode {}: a routine is read_only, ask or auto",
            routine.mode
        )));
    }
    if !routine::RINGS.contains(&routine.rings.as_str()) {
        return Err(invalid(format!(
            "a routine rings `something` (when it has something to say) or `never`, not {}",
            routine.rings
        )));
    }
    if routine.missed_hours == 0 || routine.missed_hours > 7 * 24 {
        return Err(invalid(
            "a missed run is run within 1 to 168 hours of its time".into(),
        ));
    }
    let project = crate::projects::load_project(daemon, &routine.project)?;
    providers::choose(
        daemon,
        &project,
        providers::Asked {
            provider: routine.provider.clone(),
            model: routine.model.clone(),
            effort: routine.effort.clone(),
        },
    )?;
    Ok(())
}

/// The schedule as it is kept: its words, single-spaced, lowercase.
fn said(schedule: &str) -> String {
    schedule
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Tells the clients the routine changed, and the sweep to read the next run due again.
fn changed(daemon: &Daemon, routine: &Routine) -> Result<Value, Failure> {
    daemon.routine_due.store(0, Ordering::Relaxed);
    let shown = routine.to_json();
    daemon
        .emit(
            "routine.changed",
            json!({"routine": shown, "project": routine.project}),
        )
        .map_err(|error| Failure::new("unavailable", error.to_string()))?;
    Ok(json!({"routine": shown}))
}

/// A run of the routine still under way, whatever came after it (a skip): one running, or one
/// whose work goes on in a session still open.
fn running(daemon: &Daemon, routine: &Routine) -> Option<Run> {
    let open = |session: &str| {
        daemon
            .read(|store| store.agent(session))
            .ok()
            .flatten()
            .is_some_and(|agent| !agent.state.is_final())
    };
    daemon
        .read(|store| store.routine_runs_under_way(&routine.id))
        .ok()?
        .into_iter()
        .find(|run| run.outcome == routine::RUNNING || run.session.as_deref().is_some_and(open))
}

/// Says a run's outcome: `routine.ran`, which the notification centre reads.
fn ran(daemon: &Daemon, routine: &Routine, run: &Run) {
    let _ = daemon.emit(
        "routine.ran",
        json!({"routine": routine.to_json(), "run": run.to_json(), "project": routine.project}),
    );
}

// The runs.

/// What a run's session is told after the user's prompt.
fn told(routine: &Routine) -> String {
    let mode = match routine.mode.as_str() {
        routine::READ_ONLY => {
            "It is read only: every call that writes (a file, a run, a commit, a forge or a \
             ticket write) is refused, whatever this message says; reads go through."
        }
        _ => "Your calls go through the gate as in any session.",
    };
    format!(
        "{}\n\n---\nThis session is a run of the routine \"{}\", started by Flottille at its \
         time. {mode} End with routine_result: a one-line summary, and nothing: true when there \
         is nothing to say; your session then ends with this turn.",
        routine.prompt.trim_end(),
        routine.name
    )
}

/// Starts a run of `routine` due at `due`: a new root session in its project's folder, its
/// prompt as its first message.
fn start(daemon: &Arc<Daemon>, routine: &Routine, due: u64) -> Result<Run, String> {
    let run = daemon
        .read(|store| {
            store.add_routine_run(&NewRun {
                routine: &routine.id,
                due_at: due,
                started_at: now_ms(),
                mode: &routine.mode,
                outcome: routine::RUNNING,
                why: None,
            })
        })
        .map_err(|error| error.to_string())?;
    match session(daemon, routine, &run) {
        Ok(session) => {
            let mut run = run;
            run.session = Some(session);
            let _ = daemon.emit(
                "routine.changed",
                json!({"routine": routine.to_json(), "run": run.to_json(),
                    "project": routine.project}),
            );
            Ok(run)
        }
        Err(why) => {
            let why = format!("its session could not start: {why}");
            if let Ok(Some(ended)) = daemon.read(|store| {
                store.end_routine_run(run.id, routine::FAILED, None, Some(&why), now_ms())
            }) {
                ran(daemon, routine, &ended);
            }
            Err(why)
        }
    }
}

fn session(daemon: &Arc<Daemon>, routine: &Routine, run: &Run) -> Result<String, String> {
    let project = crate::projects::load_project(daemon, &routine.project).map_err(|f| f.message)?;
    let role = roles::role(roles::LEAD).ok_or("there is no lead role")?;
    let message = told(routine);
    let mut agent = agents::made(
        daemon,
        &project,
        agents::New {
            id: None,
            workspace: None,
            preparing: false,
            role,
            parent: None,
            title: Some(routine.name.clone()),
            brief: message.clone(),
            provider: routine.provider.clone(),
            model: routine.model.clone(),
            effort: routine.effort.clone(),
            gate_mode: Some(match routine.mode.as_str() {
                routine::ASK => GateMode::Ask,
                _ => GateMode::Auto,
            }),
            launched_by: None,
            led: false,
            launched: None,
        },
    )
    .map_err(|f| f.message)?;
    // Only a routine's session gives a result, at the end of its tools.
    let provider = providers::of(&agent)?;
    agent.tools.push(format!("{}{TOOL}", provider.tool_prefix));
    // The run knows its session before the session makes its first call: the gate reads it.
    daemon
        .read(|store| store.set_routine_run_session(run.id, &agent.id))
        .map_err(|error| error.to_string())?;
    agents::write_new(daemon, &agent).map_err(|f| f.message)?;
    if routine.mode == routine::READ_ONLY {
        daemon
            .read(|store| store.set_read_only(&agent.id))
            .map_err(|error| error.to_string())?;
    }
    deliveries::deliver(
        daemon,
        &agent.id,
        "user",
        Some(format!("brief-{}-0", agent.id)),
        &message,
        approval::BETWEEN_TURNS,
    );
    Ok(agent.id)
}

/// From the daemon's start and the sweep: each routine whose run is due runs, or is skipped or
/// delayed, saying so; a run whose session ended without its result failed.
pub fn check(daemon: &Arc<Daemon>) {
    reconcile(daemon);
    let now = now_ms();
    let due = daemon.routine_due.load(Ordering::Relaxed);
    if due != 0 && now < due {
        return;
    }
    let Ok(routines) = daemon.read(|store| store.routines(None)) else {
        return;
    };
    for routine in routines {
        let Some(at) = routine.next_at.filter(|at| *at <= now && !routine.paused) else {
            continue;
        };
        due_now(daemon, &routine, at, now);
    }
    let next = daemon
        .read(Store::next_routine_due)
        .ok()
        .flatten()
        .unwrap_or(u64::MAX);
    // A change made meanwhile asked for a read again: it is kept.
    let _ =
        daemon
            .routine_due
            .compare_exchange(due, next.max(1), Ordering::Relaxed, Ordering::Relaxed);
}

/// The run of `routine` due at `at` comes, `now`.
fn due_now(daemon: &Arc<Daemon>, routine: &Routine, at: u64, now: u64) {
    // A spent quota of its provider delays it to the reset, once its wait is known.
    if let Some((until, provider)) = quota_reset(daemon, routine, now) {
        let why = format!("the quota of {provider} is spent: the run waits for its reset");
        if daemon
            .read(|store| store.delay_routine(&routine.id, at, until, &why))
            .unwrap_or(false)
        {
            let mut delayed = routine.clone();
            delayed.next_at = Some(until);
            delayed.delayed = Some(why);
            let _ = daemon.emit(
                "routine.changed",
                json!({"routine": delayed.to_json(), "project": routine.project}),
            );
        }
        return;
    }
    // Taken in one write: a run due goes once, and the next is the schedule's next after now,
    // however many were missed.
    let next = next_of(routine, now);
    if !daemon
        .read(|store| store.take_routine_due(&routine.id, at, next))
        .unwrap_or(false)
    {
        return;
    }
    // Runs missed while the daemon was off: the last of them is the one that may run, however
    // late the first was (off over a weekend, Monday's run is due, not Saturday's). One its
    // schedule does not name (a run delayed to a quota's reset) is `at` itself.
    let at = Schedule::parse(&routine.schedule)
        .ok()
        .and_then(|schedule| schedule.last_at_or_before(&zone_of(routine), now))
        .filter(|last| *last > at)
        .unwrap_or(at);
    let skip = |why: String| {
        let skipped = daemon.read(|store| {
            store.add_routine_run(&NewRun {
                routine: &routine.id,
                due_at: at,
                started_at: now,
                mode: &routine.mode,
                outcome: routine::SKIPPED,
                why: Some(&why),
            })
        });
        if let Ok(skipped) = skipped {
            ran(daemon, routine, &skipped);
        }
    };
    let limit = routine.missed_hours * 3_600_000;
    if now.saturating_sub(at) > limit {
        skip(format!(
            "missed while Flottille was off, more than {} hours ago",
            routine.missed_hours
        ));
    } else if let Some(last) = running(daemon, routine) {
        let session = last.session.as_deref().unwrap_or_default();
        skip(match last.outcome.as_str() {
            routine::GOES_ON => format!("the last run's work goes on, in session {session}"),
            _ => format!("the last run still runs, in session {session}"),
        });
    } else {
        let _ = start(daemon, routine, at);
    }
}

/// When the quota of the routine's provider resets, plus a margin, while a session of that
/// provider waits for it: the provider's name with it.
fn quota_reset(daemon: &Daemon, routine: &Routine, now: u64) -> Option<(u64, &'static str)> {
    let project = crate::projects::load_project(daemon, &routine.project).ok()?;
    let (provider, _) = providers::choose(
        daemon,
        &project,
        providers::Asked {
            provider: routine.provider.clone(),
            model: routine.model.clone(),
            effort: routine.effort.clone(),
        },
    )
    .ok()?;
    let waits = daemon.read(Store::quota_waits).ok()?;
    waits
        .iter()
        .filter_map(|wait| {
            let until = wait.reset_at?.saturating_add(crate::quota::MARGIN_MS);
            let agent = daemon.read(|store| store.agent(&wait.agent)).ok()??;
            (until > now && agent.provider == provider.name && !agent.state.is_final())
                .then_some(until)
        })
        .max()
        .map(|until| (until, provider.name))
}

/// The runs under way whose session ended or failed without giving its result failed.
fn reconcile(daemon: &Arc<Daemon>) {
    let Ok(runs) = daemon.read(Store::running_routine_runs) else {
        return;
    };
    for run in runs {
        let Some(session) = &run.session else {
            // A daemon stopped between a run and its session's start: it never started.
            if now_ms().saturating_sub(run.started_at) > 60_000 {
                end_run(
                    daemon,
                    &run,
                    routine::FAILED,
                    None,
                    Some("its session never started"),
                );
            }
            continue;
        };
        let Ok(Some(agent)) = daemon.read(|store| store.agent(session)) else {
            continue;
        };
        if !agent.state.is_final() {
            continue;
        }
        let why = match agent.state {
            AgentState::Failed => format!(
                "its session failed: {}",
                agent.state_reason.as_deref().unwrap_or("no reason said")
            ),
            _ => "its session ended without its result".to_owned(),
        };
        end_run(daemon, &run, routine::FAILED, None, Some(&why));
    }
}

/// Ends a run under way, and says it.
fn end_run(daemon: &Daemon, run: &Run, outcome: &str, summary: Option<&str>, why: Option<&str>) {
    let ended = daemon
        .read(|store| store.end_routine_run(run.id, outcome, summary, why, now_ms()))
        .ok()
        .flatten();
    if let Some(ended) = ended
        && let Ok(Some(routine)) = daemon.read(|store| store.routine(&ended.routine))
    {
        ran(daemon, &routine, &ended);
    }
}

/// The end of a turn of a root: a run's session that gave its result ends; one that ended its
/// turn without it, with nothing to wait for, is reminded once, and then its run failed while
/// the session stays, as any session.
pub fn turn_ended(daemon: &Arc<Daemon>, agent: &flottille_core::agent::Agent, stop_reason: &str) {
    let Ok(Some(run)) = daemon.read(|store| store.routine_run_of(&agent.id)) else {
        return;
    };
    match run.outcome.as_str() {
        routine::SOMETHING | routine::NOTHING => {
            tree::stop_tree(daemon, &agent.id, "its routine's run is done");
        }
        routine::RUNNING if stop_reason == "end_turn" => {
            let waits = agent.waiting_on_answer
                || daemon
                    .read(|store| store.deliveries(&agent.id, "queued"))
                    .is_ok_and(|queued| !queued.is_empty())
                || daemon
                    .read(|store| store.pending_requests_of(&agent.id))
                    .is_ok_and(|requests| !requests.is_empty());
            if waits {
                return;
            }
            let reminded = daemon
                .read(|store| store.last_deliveries(&agent.id, 16))
                .is_ok_and(|last| last.iter().any(|d| d.kind == REMINDER));
            if reminded {
                end_run(
                    daemon,
                    &run,
                    routine::FAILED,
                    None,
                    Some("its session ended its turns without its result"),
                );
            } else {
                deliveries::deliver(
                    daemon,
                    &agent.id,
                    REMINDER,
                    Some(format!("{REMINDER}-{}", run.id)),
                    "End with routine_result: a one-line summary, and nothing: true when there \
                     is nothing to say. It is what this routine's run returns.",
                    approval::BETWEEN_TURNS,
                );
            }
        }
        _ => {}
    }
}

// The tool of a run's session.

/// The tool only a routine's session has.
pub const TOOL: &str = "routine_result";

/// Whether the session is a routine's run.
pub fn is_run(daemon: &Daemon, session: &str) -> bool {
    daemon
        .read(|store| store.routine_run_of(session))
        .is_ok_and(|run| run.is_some())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Result_ {
    summary: String,
    #[serde(default)]
    nothing: bool,
    #[serde(default)]
    goes_on: bool,
}

/// `routine_result`: the run's result, kept in its history and rung when it has something to say.
pub fn result(daemon: &Arc<Daemon>, grant: &Grant, args: Value) -> Outcome {
    let asked: Result_ = match arguments(TOOL, args) {
        Ok(asked) => asked,
        Err(refused) => return refused,
    };
    let run = match daemon.read(|store| store.routine_run_of(&grant.agent)) {
        Ok(Some(run)) => run,
        Ok(None) => return Outcome::Refused("refused: this session is no routine's run".into()),
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    if run.outcome != routine::RUNNING {
        return Outcome::Refused(
            "refused: this run's result was given already: end your turn now".into(),
        );
    }
    let summary = secrets::mask(asked.summary.trim(), &std::env::vars_os().collect());
    if summary.is_empty() || summary.contains('\n') || summary.chars().count() > SUMMARY_CHARS {
        return Outcome::Invalid(format!(
            "refused: the summary is one line of at most {SUMMARY_CHARS} characters"
        ));
    }
    if asked.nothing && asked.goes_on {
        return Outcome::Invalid(
            "refused: a run with nothing to say has no work going on: nothing or goes_on".into(),
        );
    }
    if asked.goes_on && run.mode == routine::READ_ONLY {
        return Outcome::Refused(
            "refused: a read-only routine's run does no work that goes on: give its result".into(),
        );
    }
    let outcome = match (asked.nothing, asked.goes_on) {
        (true, _) => routine::NOTHING,
        (_, true) => routine::GOES_ON,
        _ => routine::SOMETHING,
    };
    end_run(daemon, &run, outcome, Some(&summary), None);
    Outcome::Done(match asked.goes_on {
        true => "Your result is kept and said to the user; your session stays open as any \
                 session."
            .into(),
        false => "Your result is kept. Your session ends with this turn: end it now.".into(),
    })
}

// Read only.

/// Whether a call reads only: what a read-only routine's session may make. Everything else is
/// refused there, an unknown tool included.
pub fn reads(tool: &str, args: &Value) -> bool {
    let action = args["action"].as_str().unwrap_or_default();
    match tool {
        "fs_read" | "fs_list" | "search" | "skill_read" | "agent_status" | "session_read"
        | "session_title" | "wake" | "report" | "ask" | "ask_user" | "ask_parent" | TOOL => true,
        "run_status" => matches!(action, "" | "output"),
        // Not `checks`: it reruns the failed runs of the branch's last commit.
        "forge" => matches!(
            action,
            "list" | "show" | "people" | "pipelines" | "runs" | "run_view" | "run_logs"
        ),
        "tickets" => matches!(
            action,
            "list"
                | "search"
                | "show"
                | "people"
                | "types"
                | "fields"
                | "link_types"
                | "sprints"
                | "milestones"
                | "progress"
                | "time_spent"
                | "worklogs"
        ),
        "tasks" | "skills" | "commands" | "workspace" | "services" | "connections" => {
            matches!(action, "list" | "show")
        }
        "setup" | "vault" => action == "list",
        "project" => matches!(action, "settings" | "rules"),
        // A bodiless GET of no one's: headers can make it another method
        // (`X-HTTP-Method-Override`), and an entry signs it in as the user, where such a GET
        // writes on many APIs. The tracker and the forge are read through their own tools.
        "web" => {
            !crate::show::is(args)
                && args["method"]
                    .as_str()
                    .is_none_or(|method| method.eq_ignore_ascii_case("GET"))
                && args["body"].is_null()
                && args["headers"].is_null()
                && args["entry"].is_null()
                && args["auth"].is_null()
        }
        _ => false,
    }
}

/// The refusal of a call that writes, made by a read-only routine's session; `None` for any
/// other call or session.
pub fn refuses(daemon: &Daemon, agent: &str, tool: &str, args: &Value) -> Option<String> {
    if reads(tool, args) {
        return None;
    }
    // The flag on the session's own row first: it holds whatever becomes of the routine (a
    // removal forgets its runs). Its root's, and the run's mode, for the moment between the
    // run's start and the flag.
    let root = tree::root_of(daemon, agent);
    let flagged = |id: &str| daemon.read(|store| store.read_only(id)).unwrap_or(false);
    let read_only = flagged(agent)
        || flagged(&root)
        || daemon
            .read(|store| store.routine_run_of(&root))
            .ok()
            .flatten()
            .is_some_and(|run| run.mode == routine::READ_ONLY);
    read_only.then(|| {
        format!(
            "refused: this session is the run of a read-only routine: it reads, and writes \
             nothing, in every mode ({tool} {}writes or acts)",
            match args["action"].as_str() {
                Some(action) => format!("{action} "),
                None => String::new(),
            }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monday 9 November 2026, 10:00 UTC.
    const MONDAY: u64 = 1_794_218_400_000;

    fn said(at: u64) -> String {
        Timestamp::from_millisecond(at as i64)
            .unwrap()
            .to_zoned(TimeZone::UTC)
            .strftime("%a %Y-%m-%d %H:%M")
            .to_string()
    }

    #[test]
    fn a_schedule_is_days_and_an_hour() {
        let next = |text: &str| {
            said(
                Schedule::parse(text)
                    .unwrap()
                    .next_after(&TimeZone::UTC, MONDAY)
                    .unwrap(),
            )
        };
        assert_eq!(said(MONDAY), "Mon 2026-11-09 10:00");
        assert_eq!(next("every day 07:00"), "Tue 2026-11-10 07:00");
        assert_eq!(next("every day 10:30"), "Mon 2026-11-09 10:30");
        assert_eq!(next("weekdays 08:30"), "Tue 2026-11-10 08:30");
        assert_eq!(next("Mondays 09:00"), "Mon 2026-11-16 09:00");
        assert_eq!(next("friday 18:00"), "Fri 2026-11-13 18:00");
        assert_eq!(next("weekends 10:00"), "Sat 2026-11-14 10:00");
        // Its own time is not after it.
        assert_eq!(next("mondays 10:00"), "Mon 2026-11-16 10:00");
    }

    #[test]
    fn a_schedule_is_read_in_its_zone() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let at = Schedule::parse("every day 09:00")
            .unwrap()
            .next_after(&paris, MONDAY)
            .unwrap();
        assert_eq!(said(at), "Tue 2026-11-10 08:00");
    }

    #[test]
    fn an_hour_skipped_by_the_spring_clock_change_runs_after_it() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        // Sunday 28 March 2027, 00:00 in Paris: at 02:00 the clocks go to 03:00.
        let midnight: u64 = 1_806_188_400_000;
        assert_eq!(said(midnight), "Sat 2027-03-27 23:00");
        let at = Schedule::parse("every day 02:30")
            .unwrap()
            .next_after(&paris, midnight)
            .unwrap();
        // 03:30 in Paris, summer time.
        assert_eq!(said(at), "Sun 2027-03-28 01:30");
    }

    #[test]
    fn an_hour_repeated_by_the_autumn_clock_change_runs_once() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        // Sunday 31 October 2027, 00:00 in Paris: at 03:00 the clocks go back to 02:00.
        let midnight: u64 = 1_824_933_600_000;
        assert_eq!(said(midnight), "Sat 2027-10-30 22:00");
        let schedule = Schedule::parse("every day 02:30").unwrap();
        let first = schedule.next_after(&paris, midnight).unwrap();
        // The first 02:30, summer time.
        assert_eq!(said(first), "Sun 2027-10-31 00:30");
        // Not again an hour later: the next is Monday's.
        let next = schedule.next_after(&paris, first).unwrap();
        assert_eq!(said(next), "Mon 2027-11-01 01:30");
    }

    #[test]
    fn the_last_time_due_is_read_back_from_now() {
        let last = |text: &str, now: u64| {
            said(
                Schedule::parse(text)
                    .unwrap()
                    .last_at_or_before(&TimeZone::UTC, now)
                    .unwrap(),
            )
        };
        // Off from Saturday 06:00 to Monday 07:30: Monday's own run is the one due.
        assert_eq!(
            last("every day 07:00", MONDAY - 150 * 60_000),
            "Mon 2026-11-09 07:00"
        );
        assert_eq!(
            last("weekdays 07:00", MONDAY - 26 * 3_600_000),
            "Fri 2026-11-06 07:00"
        );
        // Its own time is due.
        assert_eq!(last("every day 10:00", MONDAY), "Mon 2026-11-09 10:00");
        assert_eq!(last("mondays 10:01", MONDAY), "Mon 2026-11-02 10:01");
    }

    #[test]
    fn a_schedule_it_cannot_read_is_refused() {
        for text in [
            "",
            "07:00",
            "every 07:00",
            "sometimes 07:00",
            "weekdays 7",
            "weekdays 25:00",
            "weekdays 8:3",
            "cron 0 7 * * *",
        ] {
            assert!(Schedule::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_read_only_session_reads_and_writes_nothing() {
        for (tool, args) in [
            ("fs_read", json!({"path": "a"})),
            ("search", json!({"pattern": "a"})),
            ("tickets", json!({"action": "show", "id": "7"})),
            ("forge", json!({"action": "runs"})),
            ("web", json!({"url": "https://example.com"})),
            (TOOL, json!({"summary": "x"})),
        ] {
            assert!(reads(tool, &args), "{tool} {args}");
        }
        for (tool, args) in [
            ("fs_write", json!({"path": "a", "content": ""})),
            ("fs_edit", json!({"path": "a"})),
            ("run", json!({"line": "ls"})),
            (
                "tickets",
                json!({"action": "comment", "id": "7", "body": "x"}),
            ),
            ("tickets", json!({"action": "edit", "id": "7"})),
            ("forge", json!({"action": "create"})),
            ("forge", json!({"action": "merge"})),
            ("forge", json!({"action": "subscribe"})),
            // It reruns the failed runs of the branch's last commit.
            ("forge", json!({"action": "checks"})),
            ("workspace", json!({"action": "create"})),
            ("agent_launch", json!({})),
            ("session_send", json!({})),
            (
                "web",
                json!({"url": "https://example.com", "method": "POST"}),
            ),
            (
                "web",
                json!({"url": "https://example.com/items",
                    "headers": {"X-HTTP-Method-Override": "DELETE"}}),
            ),
            (
                "web",
                json!({"url": "https://example.com/items", "entry": "api-token"}),
            ),
            ("vault", json!({"action": "ask"})),
            ("review", json!({})),
        ] {
            assert!(!reads(tool, &args), "{tool} {args}");
        }
    }
}
