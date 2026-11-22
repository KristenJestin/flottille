//! The tool `routine` (`docs/tools.md`, "Routines"): what the user does with a routine, the
//! agent they talk to does too, always in its own project. `list`, `show` and `result` read at
//! once; `create`, `edit`, `pause`, `resume`, `run` and `remove` are writes the gate judges as the
//! other writes of the project, each said in words, and done by the daemon's own `routine.*`
//! methods. A routine never gets a mode above its author's session: the session that could
//! schedule an Auto session for itself would give itself more than its user did.

use std::sync::Arc;

use flottille_core::agent::GateMode;
use flottille_core::routine::{self, Routine, Run};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::mcp::{Grant, Outcome, arguments};
use crate::projects::Failure;
use crate::routines;

pub const TOOL: &str = "routine";

const ACTIONS: &str = "list, show, result, create, edit, pause, resume, run and remove";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    dead_code,
    reason = "a field the daemon reads is declared to check its type; its value passes on as given"
)]
struct Call {
    action: String,
    /// The routine acted on, by its name: every action's but list and create.
    routine: Option<String>,
    /// create: its name; edit: its new name.
    name: Option<String>,
    prompt: Option<String>,
    schedule: Option<String>,
    event: Option<Value>,
    zone: Option<String>,
    mode: Option<String>,
    rings: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    missed_hours: Option<u64>,
    /// result: the day of the run, `YYYY-MM-DD` in the routine's zone; the last when left out.
    date: Option<String>,
}

/// The fields a create or an edit passes on to the daemon as they were given.
const FIELDS: [&str; 11] = [
    "name",
    "prompt",
    "schedule",
    "event",
    "zone",
    "mode",
    "rings",
    "provider",
    "model",
    "effort",
    "missed_hours",
];

/// Whether a call of the tool writes: what the gate judges.
pub fn writes(args: &Value) -> bool {
    matches!(
        args["action"].as_str(),
        Some("create" | "edit" | "pause" | "resume" | "run" | "remove")
    )
}

/// Whether a call of the tool only reads: what a read-only routine's run may make.
pub fn reads(args: &Value) -> bool {
    matches!(args["action"].as_str(), Some("list" | "show" | "result"))
}

/// How far a mode lets a session go: a routine's, or its author's.
fn rank(mode: &str) -> u8 {
    match mode {
        routine::READ_ONLY => 0,
        routine::ASK => 1,
        _ => 2,
    }
}

/// The refusal of a routine whose mode would be above the caller's session's, `None` when it is
/// not. Read when the call is asked and again when it is done: the session's mode may have
/// changed in between.
fn above_its_author(daemon: &Daemon, grant: &Grant, mode: &str) -> Option<String> {
    let session = crate::judge::mode(daemon, &grant.agent);
    let most = match session {
        GateMode::Ask => routine::ASK,
        GateMode::Auto | GateMode::Full => routine::AUTO,
    };
    (routine::MODES.contains(&mode) && rank(mode) > rank(most)).then(|| {
        format!(
            "refused: this session is in {} mode: a routine it creates or edits is read_only or \
             ask, never {mode}: a routine never gets more than the session that made it",
            session.as_str()
        )
    })
}

/// The routine of the caller's project named `key`; another project's is none.
fn own(daemon: &Daemon, grant: &Grant, key: Option<&str>) -> Result<Routine, Outcome> {
    let Some(key) = key else {
        return Err(Outcome::Invalid(
            "refused: this action needs routine, the routine's name: {\"action\": \"show\", \
             \"routine\": \"Tickets to review\"}"
                .into(),
        ));
    };
    match daemon.read(|store| store.routine(key)) {
        Ok(Some(routine)) if routine.project == grant.place.project => Ok(routine),
        Ok(_) => Err(Outcome::Refused(format!(
            "refused: project {} has no routine {key}: list names them",
            grant.place.project
        ))),
        Err(error) => Err(Outcome::Failed(error.to_string())),
    }
}

fn refusal(failure: Failure) -> Outcome {
    match failure.code {
        "unavailable" | "invalid-config" => Outcome::Failed(failure.message),
        _ => Outcome::Refused(format!("refused: {}", failure.message)),
    }
}

/// The mode a create or an edit leaves the routine in.
fn mode_after(call: &Call, routine: Option<&Routine>) -> String {
    call.mode
        .clone()
        .or_else(|| routine.map(|r| r.mode.clone()))
        .unwrap_or_else(|| routine::READ_ONLY.to_owned())
}

fn mode_said(mode: &str) -> &'static str {
    match mode {
        routine::ASK => "Ask",
        routine::AUTO => "Auto",
        _ => "Read only",
    }
}

/// What a write does, in words, for the card the user reads; or why it cannot be done, refused
/// before anything is asked.
pub fn says(daemon: &Daemon, grant: &Grant, args: &Value) -> Result<Vec<String>, Outcome> {
    let call: Call = arguments(TOOL, args.clone())?;
    if !writes(args) {
        return match call.action.as_str() {
            "list" | "show" | "result" => Ok(Vec::new()),
            other => Err(unknown(other)),
        };
    }
    let routine = match call.action.as_str() {
        "create" => None,
        _ => Some(own(daemon, grant, call.routine.as_deref())?),
    };
    let named = |r: &Option<Routine>| r.as_ref().map_or("", |r| r.name.as_str()).to_owned();
    let said = match call.action.as_str() {
        "create" => {
            let mode = mode_after(&call, None);
            if let Some(why) = above_its_author(daemon, grant, &mode) {
                return Err(Outcome::Refused(why));
            }
            let Some(name) = &call.name else {
                return Err(Outcome::Invalid(
                    "refused: create needs name, prompt and a schedule or an event".into(),
                ));
            };
            if let Ok(Some(_)) = daemon.read(|store| store.routine(name)) {
                return Err(Outcome::Refused(format!(
                    "refused: there is a routine named {name} already"
                )));
            }
            let trigger = match (&call.schedule, &call.event) {
                (Some(schedule), _) => schedule.clone(),
                (None, Some(event)) => routines::events::said(event),
                (None, None) => {
                    return Err(Outcome::Invalid(
                        "refused: a routine runs on a schedule or on an event: give one of them"
                            .into(),
                    ));
                }
            };
            vec![
                format!(
                    "Create the routine {name} in project {}: {trigger}, {}, rings {}",
                    grant.place.project,
                    mode_said(&mode),
                    call.rings.as_deref().unwrap_or(routine::RINGS_SOMETHING)
                ),
                format!("Its prompt: {}", call.prompt.as_deref().unwrap_or_default()),
            ]
        }
        "edit" => {
            let mode = mode_after(&call, routine.as_ref());
            if let Some(why) = above_its_author(daemon, grant, &mode) {
                return Err(Outcome::Refused(why));
            }
            let mut said = vec![format!("Edit the routine {}", named(&routine))];
            for field in FIELDS {
                if let Some(value) = args.get(field) {
                    let value = value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned);
                    said.push(format!("Its {}: {value}", field.replace('_', " ")));
                }
            }
            said
        }
        "pause" => vec![format!(
            "Pause the routine {}: no run until it is resumed",
            named(&routine)
        )],
        "resume" => vec![format!("Resume the routine {}", named(&routine))],
        "run" => {
            let r = routine.as_ref().expect("an action on a routine");
            vec![format!(
                "Run the routine {} now: a new session in {}, asked: {}",
                r.name,
                mode_said(&r.mode),
                r.prompt
            )]
        }
        _ => vec![format!(
            "Remove the routine {} and its history (the sessions of its runs stay)",
            named(&routine)
        )],
    };
    Ok(said)
}

fn unknown(action: &str) -> Outcome {
    Outcome::Invalid(format!(
        "refused: routine has no action {action}: its actions are {ACTIONS}"
    ))
}

fn answered(answer: Result<Value, Failure>) -> Outcome {
    match answer {
        Ok(value) => Outcome::Done(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(failure) => refusal(failure),
    }
}

/// Performs a call of the tool, allowed by the gate.
pub fn tool(daemon: &Arc<Daemon>, grant: &Grant, args: Value) -> Outcome {
    let call: Call = match arguments(TOOL, args.clone()) {
        Ok(call) => call,
        Err(refused) => return refused,
    };
    let project = grant.place.project.clone();
    // Only the fields given, as the daemon's methods take them.
    let given = || {
        let mut params = json!({});
        for field in FIELDS {
            if let Some(value) = args.get(field) {
                params[field] = value.clone();
            }
        }
        params
    };
    match call.action.as_str() {
        "list" => answered(routines::call(
            daemon,
            "routine.list",
            json!({"project": project}),
        )),
        "create" => {
            if let Some(why) = above_its_author(daemon, grant, &mode_after(&call, None)) {
                return Outcome::Refused(why);
            }
            let mut params = given();
            params["project"] = json!(project);
            answered(routines::add(daemon, params, Some(&grant.agent)))
        }
        action => {
            let routine = match own(daemon, grant, call.routine.as_deref()) {
                Ok(routine) => routine,
                Err(refused) => return refused,
            };
            let named = json!({"routine": routine.id});
            match action {
                "show" => answered(routines::call(daemon, "routine.show", named)),
                "result" => result(daemon, &routine, call.date.as_deref()),
                "edit" => {
                    let mode = mode_after(&call, Some(&routine));
                    if let Some(why) = above_its_author(daemon, grant, &mode) {
                        return Outcome::Refused(why);
                    }
                    let mut params = given();
                    params["routine"] = json!(routine.id);
                    answered(routines::call(daemon, "routine.edit", params))
                }
                "pause" | "resume" | "run" | "remove" => {
                    answered(routines::call(daemon, &format!("routine.{action}"), named))
                }
                other => unknown(other),
            }
        }
    }
}

/// The day a run started, in the routine's zone.
fn day_of(routine: &Routine, run: &Run) -> Option<String> {
    let at = Timestamp::from_millisecond(i64::try_from(run.started_at).ok()?).ok()?;
    Some(at.to_zoned(routines::zone_of(routine)).date().to_string())
}

/// `result`: the last run that gave one, or the last of that day.
fn result(daemon: &Daemon, routine: &Routine, date: Option<&str>) -> Outcome {
    let runs = match daemon.read(|store| store.routine_runs(&routine.id, routines::HISTORY)) {
        Ok(runs) => runs,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let found = runs.iter().find(|run| {
        matches!(
            run.outcome.as_str(),
            routine::SOMETHING | routine::NOTHING | routine::GOES_ON
        ) && date.is_none_or(|date| day_of(routine, run).as_deref() == Some(date))
    });
    match found {
        Some(run) => Outcome::Done(
            serde_json::to_string_pretty(&json!({"routine": routine.name, "run": run.to_json()}))
                .unwrap_or_default(),
        ),
        None => Outcome::Refused(match date {
            Some(date) => format!(
                "refused: the routine {} gave no result on {date} (a day YYYY-MM-DD in its \
                 zone): show lists its runs",
                routine.name
            ),
            None => format!(
                "refused: the routine {} has given no result yet: show lists its runs",
                routine.name
            ),
        }),
    }
}
