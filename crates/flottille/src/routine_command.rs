//! `flottille routine`: the routines, work the daemon starts by itself on a schedule
//! (`docs/cli.md`, "Routines"): added, listed, shown, edited, run now, paused, resumed, removed.

use clap::{Args, Subcommand};
use flottille_core::paths::Paths;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::{Map, Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Subcommand)]
pub enum RoutineAction {
    /// Add a routine: each run is a new session of the project, started at its time
    Add {
        name: String,
        #[arg(long)]
        project: String,
        /// What its session is asked, in your own words
        #[arg(long)]
        prompt: String,
        #[command(flatten)]
        fields: Fields,
        #[arg(long)]
        json: bool,
    },
    /// The routines, each with its schedule, its mode and its last run
    List {
        /// Only this project's
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// One routine: its settings, its next run and its history
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Change a routine: only what is given changes
    Edit {
        name: String,
        /// Its new name
        #[arg(long)]
        rename: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        #[command(flatten)]
        fields: Fields,
        #[arg(long)]
        json: bool,
    },
    /// Start a run now
    Run {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Start no run until it is resumed
    Pause {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Run it on its schedule again
    Resume {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Forget a routine and its history; the sessions of its runs stay
    Remove {
        name: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
pub struct Fields {
    /// When it runs: `every day 07:00`, `weekdays 08:30`, `mondays 09:00`
    #[arg(long)]
    schedule: Option<String>,
    /// The time zone of the schedule (Europe/Paris); the machine's by default
    #[arg(long)]
    zone: Option<String>,
    /// read-only (the default: every write is refused), ask or auto
    #[arg(long, value_parser = ["read-only", "ask", "auto"])]
    mode: Option<String>,
    /// Where its result rings: something (when it has something to say) or never
    #[arg(long, value_parser = ["something", "never"])]
    rings: Option<String>,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    /// How many hours after its time a run missed while Flottille was off still runs (12)
    #[arg(long, value_name = "HOURS")]
    missed_hours: Option<u64>,
}

impl Fields {
    fn into_params(self, params: &mut Map<String, Value>) {
        let mode = self.mode.map(|mode| mode.replace('-', "_"));
        for (key, value) in [
            ("schedule", self.schedule),
            ("zone", self.zone),
            ("mode", mode),
            ("rings", self.rings),
            ("provider", self.provider),
            ("model", self.model),
            ("effort", self.effort),
        ] {
            if let Some(value) = value {
                params.insert(key.into(), json!(value));
            }
        }
        if let Some(hours) = self.missed_hours {
            params.insert("missed_hours".into(), json!(hours));
        }
    }
}

pub fn run(paths: &Paths, action: RoutineAction) -> Result<()> {
    let client = || Client::connect_or_start(paths);
    match action {
        RoutineAction::Add {
            name,
            project,
            prompt,
            fields,
            json,
        } => {
            let mut params = Map::new();
            params.insert("name".into(), json!(name));
            params.insert("project".into(), json!(project));
            params.insert("prompt".into(), json!(prompt));
            fields.into_params(&mut params);
            if !params.contains_key("schedule") {
                return Err("a routine needs --schedule: `weekdays 08:30`".into());
            }
            let added = client()?.request("routine.add", Value::Object(params))?;
            say(json, &added, || {
                format!(
                    "Added {}.\n{}",
                    added["routine"]["name"],
                    line(&added["routine"])
                )
            });
        }
        RoutineAction::List { project, json } => {
            let listed = client()?.request("routine.list", json!({"project": project}))?;
            say(json, &listed, || {
                let routines = listed["routines"].as_array().map_or(&[][..], Vec::as_slice);
                if routines.is_empty() {
                    return "No routine. Add one with `flottille routine add`.".into();
                }
                routines.iter().map(line).collect::<Vec<_>>().join("\n")
            });
        }
        RoutineAction::Show { name, json } => {
            let shown = client()?.request("routine.show", json!({"routine": name}))?;
            say(json, &shown, || shown_text(&shown));
        }
        RoutineAction::Edit {
            name,
            rename,
            project,
            prompt,
            fields,
            json,
        } => {
            let mut params = Map::new();
            params.insert("routine".into(), json!(name));
            for (key, value) in [("name", rename), ("project", project), ("prompt", prompt)] {
                if let Some(value) = value {
                    params.insert(key.into(), json!(value));
                }
            }
            fields.into_params(&mut params);
            let edited = client()?.request("routine.edit", Value::Object(params))?;
            say(json, &edited, || line(&edited["routine"]));
        }
        RoutineAction::Run { name, json } => {
            let ran = client()?.request("routine.run", json!({"routine": name}))?;
            say(json, &ran, || {
                format!(
                    "Started a run of {name}: session {}.",
                    ran["run"]["session"].as_str().unwrap_or_default()
                )
            });
        }
        RoutineAction::Pause { name, json } => {
            let paused = client()?.request("routine.pause", json!({"routine": name}))?;
            say(json, &paused, || line(&paused["routine"]));
        }
        RoutineAction::Resume { name, json } => {
            let resumed = client()?.request("routine.resume", json!({"routine": name}))?;
            say(json, &resumed, || line(&resumed["routine"]));
        }
        RoutineAction::Remove { name, json } => {
            let removed = client()?.request("routine.remove", json!({"routine": name}))?;
            say(json, &removed, || format!("Removed {name}."));
        }
    }
    Ok(())
}

fn say(json: bool, answer: &Value, text: impl FnOnce() -> String) {
    if json {
        println!("{answer}");
    } else {
        println!("{}", text());
    }
}

/// An instant in the user's local time.
fn when(at: &Value) -> String {
    at.as_i64()
        .and_then(|ms| Timestamp::from_millisecond(ms).ok())
        .map(|at| {
            at.to_zoned(TimeZone::system())
                .strftime("%a %Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "never".into())
}

/// One routine on a line: its name, project, schedule, mode, next and last run.
fn line(routine: &Value) -> String {
    let next = match (routine["paused"] == true, &routine["next_at"]) {
        (true, _) => "paused".to_owned(),
        (false, at) => format!("next {}", when(at)),
    };
    let last = match routine["last"].as_object() {
        Some(_) => format!(", last: {}", outcome(&routine["last"])),
        None => String::new(),
    };
    format!(
        "{} ({}): {}, {}, {next}{last}",
        routine["name"].as_str().unwrap_or_default(),
        routine["project"].as_str().unwrap_or_default(),
        routine["schedule"].as_str().unwrap_or_default(),
        routine["mode"]
            .as_str()
            .unwrap_or_default()
            .replace('_', " "),
    )
}

/// A run's outcome in words.
fn outcome(run: &Value) -> String {
    let summary = run["summary"].as_str().unwrap_or_default();
    let why = run["why"].as_str().unwrap_or_default();
    match run["outcome"].as_str().unwrap_or_default() {
        "running" => "under way".to_owned(),
        "something" => summary.to_owned(),
        "nothing" => format!("nothing ({summary})"),
        "goes_on" => format!("{summary}, its session goes on"),
        "skipped" => format!("skipped: {why}"),
        "failed" => format!("failed: {why}"),
        other => other.to_owned(),
    }
}

fn shown_text(shown: &Value) -> String {
    let routine = &shown["routine"];
    let mut out = line(routine);
    out.push_str(&format!(
        "\nPrompt: {}",
        routine["prompt"].as_str().unwrap_or_default()
    ));
    if let Some(delayed) = routine["delayed"].as_str() {
        out.push_str(&format!("\nDelayed: {delayed}"));
    }
    let runs = shown["runs"].as_array().map_or(&[][..], Vec::as_slice);
    if runs.is_empty() {
        out.push_str("\nNever run yet.");
    }
    for run in runs {
        let session = match run["session"].as_str() {
            Some(session) => format!(" (session {session})"),
            None => String::new(),
        };
        out.push_str(&format!(
            "\n{}: {}{session}",
            when(&run["due_at"]),
            outcome(run)
        ));
    }
    out
}
