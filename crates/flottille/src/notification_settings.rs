//! What rings, and when (#1003, `docs/protocol.md`, "The notification settings methods"): the
//! user's choices by kind and by project, the sound, the quiet hours and Do not disturb, in
//! `notifications.toml` of the configuration folder, read at every ring so that a change applies
//! at once and a file edited by hand is what the daemon follows.
//!
//! No tool of an agent reaches them, and no proposal path: an agent that could lower what rings
//! could silence the user's own gate.

use std::collections::BTreeMap;
use std::sync::Mutex;

use flottille_core::notification_settings::{
    DAYS, KINDS, Level, LocalTime, Settings, local_time, may_be_off, minute_of,
};
use flottille_core::paths::Paths;
use flottille_core::store::now_ms;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::providers;

/// What `notifications.toml` says; the defaults when there is no file.
pub fn read(paths: &Paths) -> Result<Settings, String> {
    match std::fs::read_to_string(paths.notifications()) {
        Ok(text) => Settings::parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(format!("notifications.toml: {error}")),
    }
}

/// What the daemon follows: a file that cannot be read is the defaults', which ring what needs the
/// user. It is never silent: the log says why once for each reason, and `notifications.settings`
/// answers it to Settings.
pub fn load(paths: &Paths) -> Settings {
    static SAID: Mutex<Option<String>> = Mutex::new(None);
    let read = read(paths);
    let mut said = SAID.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match read {
        Ok(settings) => {
            *said = None;
            settings
        }
        Err(why) => {
            if said.as_deref() != Some(why.as_str()) {
                eprintln!("flottille daemon: {why}: the defaults ring until it is fixed");
                *said = Some(why);
            }
            Settings::default()
        }
    }
}

/// The user's clock. In a test build `FLOTTILLE_TEST_LOCAL_TIME` (`Fri 23:30`) says what it
/// reads; a release build reads nothing of the environment.
pub fn local_now(now: u64) -> LocalTime {
    cfg!(debug_assertions)
        .then(|| std::env::var("FLOTTILLE_TEST_LOCAL_TIME").ok())
        .flatten()
        .and_then(|text| {
            let (day, time) = text.split_once(' ')?;
            Some(LocalTime {
                day: DAYS.iter().position(|d| day.eq_ignore_ascii_case(d))?,
                minute: minute_of(time)?,
            })
        })
        .unwrap_or_else(|| local_time(now))
}

/// Whether nothing may ring now: Do not disturb, or inside the quiet hours.
pub fn muted(settings: &Settings) -> bool {
    let now = now_ms();
    settings.muted(now, local_now(now))
}

/// The settings as the protocol says them: every kind with what it does, so that no client keeps
/// its own list of defaults.
fn said(settings: &Settings) -> Value {
    let now = now_ms();
    let kinds: BTreeMap<&str, &str> = KINDS
        .iter()
        .map(|kind| (*kind, settings.level(kind, None).word()))
        .collect();
    let sounds: BTreeMap<&str, bool> = KINDS
        .iter()
        .map(|kind| {
            (
                *kind,
                settings.sound.kinds.get(*kind).copied().unwrap_or(true),
            )
        })
        .collect();
    json!({
        "kinds": kinds,
        "sound": {"on": settings.sound.on, "volume": settings.sound.volume, "kinds": sounds},
        "quiet": {
            "on": settings.quiet.on,
            "from": settings.quiet.from,
            "to": settings.quiet.to,
            "days": settings.quiet.days,
        },
        "do_not_disturb": {
            "on": settings.do_not_disturb.holds(now),
            "until": settings.do_not_disturb.until,
        },
        "projects": settings
            .projects
            .iter()
            .map(|(project, own)| (project.as_str(), &own.kinds))
            .collect::<BTreeMap<_, _>>(),
        "muted": settings.muted(now, local_now(now)),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    project: Option<String>,
    /// Kind by level: `ring`, `centre`, `off`, or `default` for what the defaults say.
    kinds: Option<BTreeMap<String, String>>,
    sound: Option<SoundSet>,
    quiet: Option<QuietSet>,
    do_not_disturb: Option<DndSet>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SoundSet {
    on: Option<bool>,
    volume: Option<u64>,
    kinds: Option<BTreeMap<String, bool>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuietSet {
    on: Option<bool>,
    from: Option<String>,
    to: Option<String>,
    days: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DndSet {
    on: Option<bool>,
    until: Option<u64>,
}

fn invalid(message: impl Into<String>) -> Failure {
    Failure::new("invalid-params", message)
}

/// `settings` with what `set` changes; nothing is kept here, the caller writes the file once the
/// whole of it is checked.
fn apply(daemon: &Daemon, settings: &mut Settings, set: Set) -> Result<(), Failure> {
    let Set {
        project,
        kinds,
        sound,
        quiet,
        do_not_disturb,
    } = set;
    if kinds.is_none() && sound.is_none() && quiet.is_none() && do_not_disturb.is_none() {
        return Err(invalid(
            "give kinds, sound, quiet or do_not_disturb to change",
        ));
    }
    if let Some(project) = &project {
        if kinds.is_none() || sound.is_some() || quiet.is_some() || do_not_disturb.is_some() {
            return Err(invalid(
                "a project has its own `kinds` only: the sound, the quiet hours and Do not \
                 disturb are for every project",
            ));
        }
        crate::projects::load_project(daemon, project)
            .map_err(|_| invalid(format!("there is no project {project}")))?;
    }
    for (kind, word) in kinds.into_iter().flatten() {
        if !KINDS.contains(&kind.as_str()) {
            return Err(invalid(format!(
                "{kind} is not a kind of notification: {}",
                KINDS.join(", ")
            )));
        }
        let level = match word.as_str() {
            "default" => None,
            word => Some(Level::from_word(word).ok_or_else(|| {
                invalid(format!(
                    "{word} is not what a kind does: ring, centre, off or default"
                ))
            })?),
        };
        if level == Some(Level::Off) && !may_be_off(&kind) {
            return Err(invalid(format!(
                "a {} needs you, so it cannot be off; only its ringing can be lowered to centre",
                kind.replace('_', " ")
            )));
        }
        match (&project, level) {
            (None, Some(level)) => {
                settings.kinds.insert(kind, level);
            }
            (None, None) => {
                settings.kinds.remove(&kind);
            }
            (Some(project), Some(level)) => {
                settings
                    .projects
                    .entry(project.clone())
                    .or_default()
                    .kinds
                    .insert(kind, level);
            }
            (Some(project), None) => {
                if let Some(own) = settings.projects.get_mut(project) {
                    own.kinds.remove(&kind);
                    if own.kinds.is_empty() {
                        settings.projects.remove(project);
                    }
                }
            }
        }
    }
    if let Some(sound) = sound {
        if let Some(on) = sound.on {
            settings.sound.on = on;
        }
        if let Some(volume) = sound.volume {
            settings.sound.volume = u8::try_from(volume)
                .ok()
                .filter(|volume| *volume <= 100)
                .ok_or_else(|| invalid("the volume is from 0 to 100"))?;
        }
        for (kind, on) in sound.kinds.into_iter().flatten() {
            if !KINDS.contains(&kind.as_str()) {
                return Err(invalid(format!("{kind} is not a kind of notification")));
            }
            match on {
                true => settings.sound.kinds.remove(&kind),
                false => settings.sound.kinds.insert(kind, false),
            };
        }
    }
    if let Some(quiet) = quiet {
        if let Some(on) = quiet.on {
            settings.quiet.on = on;
        }
        if let Some(from) = quiet.from {
            settings.quiet.from = from;
        }
        if let Some(to) = quiet.to {
            settings.quiet.to = to;
        }
        if let Some(days) = quiet.days {
            // In the week's order, each once.
            settings.quiet.days = DAYS
                .iter()
                .filter(|day| days.iter().any(|chosen| chosen == *day))
                .map(|day| (*day).to_owned())
                .collect();
            if let Some(unknown) = days.iter().find(|day| !DAYS.contains(&day.as_str())) {
                return Err(invalid(format!(
                    "{unknown} is not a day: {}",
                    DAYS.join(", ")
                )));
            }
        }
    }
    if let Some(dnd) = do_not_disturb {
        let on = dnd.on.unwrap_or(dnd.until.is_some());
        settings.do_not_disturb.on = on;
        settings.do_not_disturb.until = dnd.until.filter(|_| on);
    }
    settings.check().map_err(invalid)
}

/// Answers a `notifications.settings` or `notifications.set_settings` request of the user.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    let paths = &daemon.paths;
    match method {
        "notifications.settings" => {
            let settings = read(paths).map_err(|e| Failure::new("invalid-config", e))?;
            Ok(said(&settings))
        }
        "notifications.set_settings" => {
            let set: Set = serde_json::from_value(params)?;
            // One change at a time: each reads the file the last wrote.
            let _one = daemon
                .rings
                .settings_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut settings = read(paths).map_err(|e| Failure::new("invalid-config", e))?;
            apply(daemon, &mut settings, set)?;
            let text = settings.to_toml().map_err(invalid)?;
            std::fs::create_dir_all(&paths.config).map_err(|e| {
                Failure::new("unavailable", format!("{}: {e}", paths.config.display()))
            })?;
            providers::write_in_place(&paths.notifications(), &text)
                .map_err(|e| Failure::new("unavailable", format!("notifications.toml: {e}")))?;
            let answer = said(&settings);
            daemon
                .emit("notifications.settings_changed", answer.clone())
                .map_err(|error| Failure::new("unavailable", error.to_string()))?;
            Ok(answer)
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}
