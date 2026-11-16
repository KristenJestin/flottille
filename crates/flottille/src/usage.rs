//! The statistics of what agents used (`docs/usage.md`): weighted tokens, and the cost of a task
//! that ends merged. Read from the turns' usage (`turn_usage`), the workspaces' events (a task is
//! a workspace) and the pull requests the items keep (a task succeeded when one was merged).
//!
//! The turns counted are those that started in the period; a task across its edge counts the part
//! in it. The days are the daemon's local days.

use std::collections::{BTreeMap, HashMap, HashSet};

use flottille_core::agent::TurnUsage;
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::{self, Failure};

/// What a turn's tokens weigh, in twentieths of an input token, so that every sum is exact:
/// `input + 1.25 × cache_write + 0.1 × cache_read + 5 × output` (`docs/usage.md`).
const INPUT: u64 = 20;
const CACHE_WRITE: u64 = 25;
const CACHE_READ: u64 = 2;
const OUTPUT: u64 = 100;
const UNIT: f64 = 20.0;

/// The ways the statistics are split.
const BY: [&str; 5] = ["day", "model", "provider", "role", "tuning"];

/// Answers `usage.stats`. No tool of an agent reaches it.
pub fn call(daemon: &Daemon, method: &str, params: Value) -> Result<Value, Failure> {
    match method {
        "usage.stats" => stats(daemon, params),
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    project: Option<String>,
    since: String,
    until: String,
    by: String,
}

fn stats(daemon: &Daemon, params: Value) -> Result<Value, Failure> {
    let params: Params = serde_json::from_value(params)
        .map_err(|e| Failure::new("invalid-params", e.to_string()))?;
    let date = |text: &str| {
        text.parse::<Date>().map_err(|_| {
            Failure::new(
                "invalid-params",
                format!("`{text}` is no date: give one as 2026-11-15"),
            )
        })
    };
    let (since, until) = (date(&params.since)?, date(&params.until)?);
    if until < since {
        return Err(Failure::new("invalid-params", "until is before since"));
    }
    if !BY.contains(&params.by.as_str()) {
        return Err(Failure::new(
            "invalid-params",
            format!("by is one of {}", BY.join(", ")),
        ));
    }
    let zone = TimeZone::system();
    let start = |day: Date| -> Result<u64, Failure> {
        let at = day
            .to_zoned(zone.clone())
            .map_err(|e| Failure::new("invalid-params", e.to_string()))?;
        Ok(at.timestamp().as_millisecond().max(0) as u64)
    };
    let end_day = until
        .tomorrow()
        .map_err(|e| Failure::new("invalid-params", e.to_string()))?;
    let (from, to) = (start(since)?, start(end_day)?);

    let project = params.project.as_deref();
    let (turns, history, merged) = daemon.read(|store| {
        Ok((
            store.turn_usage_between(project, from, to)?,
            store.events_named(
                &[
                    "workspace.created",
                    "workspace.changed",
                    "workspace.removed",
                ],
                0,
            )?,
            store.merged_pulls(project)?,
        ))
    })?;
    let with_forge: HashSet<String> = projects::all(daemon)
        .into_iter()
        .filter(|p| p.repositories.iter().any(|r| r.forge.is_some()))
        .map(|p| p.name)
        .collect();
    let tasks = Tasks::of(&history, merged, with_forge);

    let mut total = Group::default();
    let mut groups: BTreeMap<Option<String>, Group> = BTreeMap::new();
    if params.by == "day" {
        let mut day = since;
        while day <= until {
            groups.insert(Some(day.to_string()), Group::default());
            day = day
                .tomorrow()
                .map_err(|e| Failure::new("invalid-params", e.to_string()))?;
        }
    }
    for turn in &turns {
        let key = match params.by.as_str() {
            "day" => Timestamp::from_millisecond(turn.started_at as i64)
                .ok()
                .map(|at| at.to_zoned(zone.clone()).date().to_string()),
            "model" => turn.used.model.clone(),
            "provider" => Some(turn.provider.clone()),
            "role" => Some(turn.role.clone()),
            _ => Some(turn.tuning.clone()),
        };
        let task = turn
            .workspace
            .as_deref()
            .map(|workspace| tasks.find(&turn.project, workspace, turn.started_at));
        total.add(turn, task.clone());
        groups.entry(key).or_default().add(turn, task);
    }

    let mut groups: Vec<(Option<String>, Group)> = groups.into_iter().collect();
    if params.by != "day" {
        groups.sort_by(|a, b| b.1.weighted.cmp(&a.1.weighted).then(a.0.cmp(&b.0)));
    }
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|(key, group)| {
            let mut said = group.said(&tasks);
            said["key"] = json!(key);
            said
        })
        .collect();
    Ok(json!({
        "project": params.project,
        "since": since.to_string(),
        "until": until.to_string(),
        "by": params.by,
        "total": total.said(&tasks),
        "groups": groups,
    }))
}

/// The tokens of some turns, and what each task of them cost.
#[derive(Default)]
struct Group {
    turns: u64,
    missing: u64,
    input: u64,
    cache_write: u64,
    cache_read: u64,
    output: u64,
    reasoning: u64,
    /// In twentieths of an input token.
    weighted: u64,
    /// Of the turns in a workspace, by task.
    tasks: HashMap<TaskKey, u64>,
    /// Of the turns in `main`.
    main: u64,
}

impl Group {
    fn add(&mut self, turn: &TurnUsage, task: Option<TaskKey>) {
        let used = &turn.used;
        let (input, cache_write, cache_read, output) = (
            used.input.unwrap_or(0),
            used.cache_write.unwrap_or(0),
            used.cache_read.unwrap_or(0),
            used.output.unwrap_or(0),
        );
        let weighted =
            INPUT * input + CACHE_WRITE * cache_write + CACHE_READ * cache_read + OUTPUT * output;
        self.turns += 1;
        let counts = [used.input, used.cache_write, used.cache_read, used.output];
        if counts.iter().any(Option::is_none) {
            self.missing += 1;
        }
        self.input += input;
        self.cache_write += cache_write;
        self.cache_read += cache_read;
        self.output += output;
        self.reasoning += used.reasoning.unwrap_or(0);
        self.weighted += weighted;
        match task {
            Some(task) => *self.tasks.entry(task).or_default() += weighted,
            None => self.main += weighted,
        }
    }

    fn said(&self, tasks: &Tasks) -> Value {
        let mut by_outcome: HashMap<Outcome, (u64, u64)> = HashMap::new();
        for (task, weighted) in &self.tasks {
            let counted = by_outcome.entry(tasks.outcome(task)).or_default();
            counted.0 += 1;
            counted.1 += weighted;
        }
        let of = |outcome| {
            let (count, weighted) = by_outcome.get(&outcome).copied().unwrap_or_default();
            json!({"count": count, "weighted": weighted as f64 / UNIT})
        };
        let (merged, merged_cost) = by_outcome
            .get(&Outcome::Merged)
            .copied()
            .unwrap_or_default();
        let failed_cost = by_outcome
            .get(&Outcome::Failed)
            .map_or(0, |(_, weighted)| *weighted);
        // A failed task's cost counts against the merged ones: what it took to get them.
        let per_merged =
            (merged > 0).then(|| (merged_cost + failed_cost) as f64 / UNIT / merged as f64);
        json!({
            "turns": self.turns,
            "missing": self.missing,
            "input": self.input,
            "cache_write": self.cache_write,
            "cache_read": self.cache_read,
            "output": self.output,
            "reasoning": self.reasoning,
            "weighted": self.weighted as f64 / UNIT,
            "cache_read_share": if self.weighted == 0 {
                0.0
            } else {
                (CACHE_READ * self.cache_read) as f64 / self.weighted as f64
            },
            "tasks": {
                "merged": of(Outcome::Merged),
                "failed": of(Outcome::Failed),
                "in_progress": of(Outcome::InProgress),
                "no_forge": of(Outcome::NoForge),
                "main": self.main as f64 / UNIT,
                "cost_per_merged_task": per_merged,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Outcome {
    Merged,
    Failed,
    InProgress,
    NoForge,
}

/// A workspace, from its creation to its removal.
struct Task {
    project: String,
    name: String,
    created_at: u64,
    removed: bool,
    /// Every branch it was on: a pull request of any of them is its.
    branches: HashSet<String>,
    repositories: HashSet<String>,
}

/// A task: one the workspaces' events tell, by its place among them, or a workspace they do not
/// tell, by its project and name.
#[derive(Clone, PartialEq, Eq, Hash)]
enum TaskKey {
    Told(usize),
    Untold(String, String),
}

/// Every task the workspaces' events tell, and their outcomes.
struct Tasks {
    tasks: Vec<Task>,
    outcomes: Vec<Outcome>,
    with_forge: HashSet<String>,
}

impl Tasks {
    fn of(
        history: &[flottille_core::protocol::Event],
        merged: Vec<(String, String, String)>,
        with_forge: HashSet<String>,
    ) -> Tasks {
        let mut tasks: Vec<Task> = Vec::new();
        for event in history {
            let (Some(project), Some(name)) = (
                event.data["project"].as_str(),
                event.data["workspace"].as_str(),
            ) else {
                continue;
            };
            let last = tasks
                .iter_mut()
                .rev()
                .find(|t| t.project == project && t.name == name);
            match (event.name.as_str(), last) {
                ("workspace.created", _) => tasks.push(Task {
                    project: project.to_owned(),
                    name: name.to_owned(),
                    created_at: event.at,
                    removed: false,
                    branches: event.data["branch"]
                        .as_str()
                        .map(str::to_owned)
                        .into_iter()
                        .collect(),
                    repositories: event.data["repositories"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|r| r["path"].as_str().map(str::to_owned))
                        .collect(),
                }),
                ("workspace.changed", Some(task)) => {
                    if let Some(branch) = event.data["branch"].as_str() {
                        task.branches.insert(branch.to_owned());
                    }
                }
                ("workspace.removed", Some(task)) => task.removed = true,
                _ => {}
            }
        }
        let merged: HashSet<(String, String, String)> = merged.into_iter().collect();
        let outcomes = tasks
            .iter()
            .map(|task| {
                if !with_forge.contains(&task.project) {
                    return Outcome::NoForge;
                }
                let merged = task.repositories.iter().any(|repository| {
                    task.branches.iter().any(|branch| {
                        merged.contains(&(task.project.clone(), repository.clone(), branch.clone()))
                    })
                });
                match (merged, task.removed) {
                    (true, _) => Outcome::Merged,
                    (false, true) => Outcome::Failed,
                    (false, false) => Outcome::InProgress,
                }
            })
            .collect();
        Tasks {
            tasks,
            outcomes,
            with_forge,
        }
    }

    /// The task a turn in `workspace` belongs to: the last of that name made before it started.
    fn find(&self, project: &str, workspace: &str, started_at: u64) -> TaskKey {
        self.tasks
            .iter()
            .rposition(|t| {
                t.project == project && t.name == workspace && t.created_at <= started_at
            })
            .map_or_else(
                || TaskKey::Untold(project.to_owned(), workspace.to_owned()),
                TaskKey::Told,
            )
    }

    /// A workspace the events do not tell has no branch to look for: in progress, unless its
    /// project has no forge.
    fn outcome(&self, task: &TaskKey) -> Outcome {
        match task {
            TaskKey::Told(index) => self.outcomes[*index],
            TaskKey::Untold(project, _) if !self.with_forge.contains(project) => Outcome::NoForge,
            TaskKey::Untold(..) => Outcome::InProgress,
        }
    }
}
