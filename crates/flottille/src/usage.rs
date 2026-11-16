//! The statistics of what agents used (`docs/usage.md`): weighted tokens, and the cost of a task
//! that ends merged. Read from the turns' usage (`turn_usage`), the workspaces' events (a task is
//! a workspace) and the pull requests seen merged (a task succeeded when one of its own was).
//!
//! The tokens are those of the turns that started in the period. The cost per merged task is
//! that of the tasks that finished in it, each with all of its turns, whatever their day. The days
//! are the daemon's local days.

use std::collections::{BTreeMap, HashMap, HashSet};

use flottille_core::agent::TurnUsage;
use flottille_core::protocol::Event;
use flottille_core::store::MergedPull;
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

/// The group of a task whose turns did not all run with the same optimisations on.
const MIXED: &str = "mixed";

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
#[serde(untagged)]
enum By {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    project: Option<String>,
    since: Option<String>,
    until: Option<String>,
    days: Option<u64>,
    by: By,
}

fn invalid(message: impl ToString) -> Failure {
    Failure::new("invalid-params", message.to_string())
}

fn stats(daemon: &Daemon, params: Value) -> Result<Value, Failure> {
    let params: Params = serde_json::from_value(params).map_err(invalid)?;
    let date = |text: &str| {
        text.parse::<Date>()
            .map_err(|_| invalid(format!("`{text}` is no date: give one as 2026-11-15")))
    };
    let zone = TimeZone::system();
    // The daemon's today: a client in another time zone asks for days, not dates.
    let until = match &params.until {
        Some(until) => date(until)?,
        None => Timestamp::now().to_zoned(zone.clone()).date(),
    };
    let since = match (&params.since, params.days) {
        (Some(since), None) => date(since)?,
        (None, Some(days)) if days >= 1 => until
            .checked_sub(jiff::Span::new().days(days as i64 - 1))
            .map_err(invalid)?,
        _ => return Err(invalid("give since (a date) or days (1 or more), not both")),
    };
    if until < since {
        return Err(invalid("until is before since"));
    }
    let by = match params.by {
        By::One(by) => vec![by],
        By::Many(by) => by,
    };
    if by.is_empty() || by.iter().any(|b| !BY.contains(&b.as_str())) {
        return Err(invalid(format!("by is one or more of {}", BY.join(", "))));
    }
    let start = |day: Date| -> Result<u64, Failure> {
        let at = day.to_zoned(zone.clone()).map_err(invalid)?;
        Ok(at.timestamp().as_millisecond().max(0) as u64)
    };
    let (from, to) = (start(since)?, start(until.tomorrow().map_err(invalid)?)?);

    let project = params.project.as_deref();
    let (turns, history, merges) = daemon.read(|store| {
        Ok((
            store.turn_usage_between(project, from, to)?,
            store.workspace_history(project)?,
            store.merged_pulls(project)?,
        ))
    })?;
    let forges: HashMap<String, HashSet<String>> = projects::all(daemon)
        .into_iter()
        .map(|p| {
            let with: HashSet<String> = p
                .repositories
                .iter()
                .filter(|r| r.forge.is_some())
                .map(|r| r.path.clone())
                .collect();
            (p.name, with)
        })
        .collect();
    let tasks = Tasks::of(&history, &merges, to, forges);

    // The tasks that finished in the period, each with every turn of it.
    let finished: Vec<usize> = (0..tasks.tasks.len())
        .filter(|&i| tasks.end(i).is_some_and(|(_, at)| (from..to).contains(&at)))
        .collect();
    let whole: Vec<Vec<TurnUsage>> = daemon.read(|store| {
        finished
            .iter()
            .map(|&i| {
                let task = &tasks.tasks[i];
                store.turn_usage_of_workspace(
                    &task.project,
                    &task.name,
                    task.created_at,
                    task.until,
                )
            })
            .collect()
    })?;
    let mut ratio = Ratio::default();
    let mut ratio_by_tuning: BTreeMap<String, Ratio> = BTreeMap::new();
    for (&i, turns) in finished.iter().zip(&whole) {
        // A workspace no agent worked in is no agent's task.
        let Some(first) = turns.first() else {
            continue;
        };
        let cost: u64 = turns.iter().map(weighed).sum();
        let tuning = match turns.iter().all(|t| t.tuning == first.tuning) {
            true => first.tuning.clone(),
            false => MIXED.to_owned(),
        };
        let merged = matches!(tasks.end(i), Some((Outcome::Merged, _)));
        ratio.add(merged, cost);
        ratio_by_tuning.entry(tuning).or_default().add(merged, cost);
    }

    // The period's turns: its tokens, and the tasks still without an outcome.
    let mut total = Sum::default();
    let mut open: HashMap<TaskKey, u64> = HashMap::new();
    let mut main = 0;
    let mut splits: Vec<BTreeMap<String, Sum>> = vec![BTreeMap::new(); by.len()];
    for (split, name) in splits.iter_mut().zip(&by) {
        match name.as_str() {
            "day" => {
                let mut day = since;
                while day <= until {
                    split.insert(day.to_string(), Sum::default());
                    day = day.tomorrow().map_err(invalid)?;
                }
            }
            "tuning" => {
                for tuning in ratio_by_tuning.keys() {
                    split.insert(tuning.clone(), Sum::default());
                }
            }
            _ => {}
        }
    }
    for turn in &turns {
        total.add(turn);
        match &turn.workspace {
            Some(workspace) => {
                *open
                    .entry(tasks.find(&turn.project, workspace, turn.started_at))
                    .or_default() += weighed(turn)
            }
            None => main += weighed(turn),
        }
        for (split, name) in splits.iter_mut().zip(&by) {
            let key = match name.as_str() {
                "day" => Timestamp::from_millisecond(turn.started_at as i64)
                    .map(|at| at.to_zoned(zone.clone()).date().to_string())
                    .unwrap_or_default(),
                "model" => turn.used.model.clone().unwrap_or_default(),
                "provider" => turn.provider.clone(),
                "role" => turn.role.clone(),
                _ => turn.tuning.clone(),
            };
            split.entry(key).or_default().add(turn);
        }
    }
    let mut unknown: HashMap<Outcome, (u64, u64)> = HashMap::new();
    for (task, weighted) in &open {
        let outcome = tasks.outcome(task);
        if matches!(outcome, Outcome::InProgress | Outcome::NoForge) {
            let counted = unknown.entry(outcome).or_default();
            counted.0 += 1;
            counted.1 += weighted;
        }
    }
    let of = |outcome| {
        let (count, weighted) = unknown.get(&outcome).copied().unwrap_or_default();
        json!({"count": count, "weighted": weighted as f64 / UNIT})
    };
    let mut said_total = total.said();
    said_total["tasks"] = ratio.said();
    said_total["tasks"]["in_progress"] = of(Outcome::InProgress);
    said_total["tasks"]["no_forge"] = of(Outcome::NoForge);
    said_total["tasks"]["main"] = json!(main as f64 / UNIT);

    let mut groups = serde_json::Map::new();
    for (split, name) in splits.into_iter().zip(&by) {
        let mut split: Vec<(String, Sum)> = split.into_iter().collect();
        if name != "day" {
            split.sort_by(|a, b| b.1.weighted.cmp(&a.1.weighted).then(a.0.cmp(&b.0)));
        }
        let said: Vec<Value> = split
            .into_iter()
            .map(|(key, sum)| {
                let mut said = sum.said();
                said["key"] = json!(key);
                // Each task is in one group of what was on; in the other splits a task would be
                // counted whole in every group it touches: they say the tokens alone.
                if name == "tuning" {
                    said["tasks"] = ratio_by_tuning
                        .get(&key)
                        .cloned()
                        .unwrap_or_default()
                        .said();
                }
                said
            })
            .collect();
        groups.insert(name.clone(), Value::Array(said));
    }
    Ok(json!({
        "project": params.project,
        "since": since.to_string(),
        "until": until.to_string(),
        "by": by,
        "total": said_total,
        "groups": groups,
    }))
}

/// What a row's tokens weigh, in twentieths of an input token; a count not given weighs nothing.
fn weighed(turn: &TurnUsage) -> u64 {
    let used = &turn.used;
    INPUT * used.input.unwrap_or(0)
        + CACHE_WRITE * used.cache_write.unwrap_or(0)
        + CACHE_READ * used.cache_read.unwrap_or(0)
        + OUTPUT * used.output.unwrap_or(0)
}

/// The tokens of some rows (one per turn and model).
#[derive(Clone, Default)]
struct Sum {
    rows: u64,
    missing: u64,
    input: u64,
    cache_write: u64,
    cache_read: u64,
    output: u64,
    reasoning: u64,
    /// In twentieths of an input token.
    weighted: u64,
}

impl Sum {
    fn add(&mut self, turn: &TurnUsage) {
        let used = &turn.used;
        self.rows += 1;
        let counts = [used.input, used.cache_write, used.cache_read, used.output];
        if counts.iter().any(Option::is_none) {
            self.missing += 1;
        }
        self.input += used.input.unwrap_or(0);
        self.cache_write += used.cache_write.unwrap_or(0);
        self.cache_read += used.cache_read.unwrap_or(0);
        self.output += used.output.unwrap_or(0);
        self.reasoning += used.reasoning.unwrap_or(0);
        self.weighted += weighed(turn);
    }

    fn said(&self) -> Value {
        let raw_input = self.input + self.cache_read + self.cache_write;
        json!({
            "rows": self.rows,
            "missing": self.missing,
            "input": self.input,
            "cache_write": self.cache_write,
            "cache_read": self.cache_read,
            "output": self.output,
            "reasoning": self.reasoning,
            "weighted": self.weighted as f64 / UNIT,
            // Of the input tokens, raw, the share read from the cache.
            "cache_read_share": if raw_input == 0 {
                0.0
            } else {
                self.cache_read as f64 / raw_input as f64
            },
        })
    }
}

/// The finished tasks of a period, merged or failed, with their whole cost.
#[derive(Clone, Default)]
struct Ratio {
    merged: (u64, u64),
    failed: (u64, u64),
}

impl Ratio {
    fn add(&mut self, merged: bool, cost: u64) {
        let counted = match merged {
            true => &mut self.merged,
            false => &mut self.failed,
        };
        counted.0 += 1;
        counted.1 += cost;
    }

    fn said(&self) -> Value {
        let ((merged, merged_cost), (failed, failed_cost)) = (self.merged, self.failed);
        // A failed task's cost counts against the merged ones: what it took to get them.
        let per_merged =
            (merged > 0).then(|| (merged_cost + failed_cost) as f64 / UNIT / merged as f64);
        json!({
            "merged": {"count": merged, "weighted": merged_cost as f64 / UNIT},
            "failed": {"count": failed, "weighted": failed_cost as f64 / UNIT},
            "cost_per_merged_task": per_merged,
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
    /// When the next workspace of its name was made: its turns are those before.
    until: u64,
    removed_at: Option<u64>,
    /// Whether one of its repositories had a forge declared when it was removed, as its removal
    /// said; none when it said nothing (removed before it was said).
    forge_when_removed: Option<bool>,
    /// Every branch it was on: a pull request of any of them may be its.
    branches: HashSet<String>,
    repositories: Vec<String>,
    /// When the first pull request it opened was seen merged, before the end of the period.
    merged_at: Option<u64>,
}

/// A task: one the workspaces' events tell, by its place among them, or a workspace they do not
/// tell, by its project and name.
#[derive(Clone, PartialEq, Eq, Hash)]
enum TaskKey {
    Told(usize),
    Untold(String, String),
}

/// Every task the workspaces' events tell, as of the end of the period (`to`).
struct Tasks {
    tasks: Vec<Task>,
    /// The tasks of each project and name, by their creation.
    by_name: HashMap<(String, String), Vec<usize>>,
    /// Of each project, the paths of its repositories with a forge declared now.
    forges: HashMap<String, HashSet<String>>,
    to: u64,
}

impl Tasks {
    fn of(
        history: &[Event],
        merges: &[MergedPull],
        to: u64,
        forges: HashMap<String, HashSet<String>>,
    ) -> Tasks {
        let mut tasks: Vec<Task> = Vec::new();
        let mut by_name: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for event in history {
            let (Some(project), Some(name)) = (
                event.data["project"].as_str(),
                event.data["workspace"].as_str(),
            ) else {
                continue;
            };
            let named = by_name
                .entry((project.to_owned(), name.to_owned()))
                .or_default();
            let last = named.last().copied();
            match (event.name.as_str(), last) {
                ("workspace.created", _) => {
                    if let Some(last) = last {
                        tasks[last].until = event.at;
                    }
                    named.push(tasks.len());
                    tasks.push(Task {
                        project: project.to_owned(),
                        name: name.to_owned(),
                        created_at: event.at,
                        until: i64::MAX as u64,
                        removed_at: None,
                        forge_when_removed: None,
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
                        merged_at: None,
                    });
                }
                ("workspace.changed", Some(last)) => {
                    if let Some(branch) = event.data["branch"].as_str() {
                        tasks[last].branches.insert(branch.to_owned());
                    }
                }
                ("workspace.removed", Some(last)) => {
                    tasks[last].removed_at = Some(event.at);
                    tasks[last].forge_when_removed = event.data["forge"].as_bool();
                }
                _ => {}
            }
        }
        for named in by_name.values_mut() {
            named.sort_by_key(|&i| tasks[i].created_at);
        }
        // Each merge is one task's: the last of its repository and branch made before the pull
        // request was opened (before it was seen merged, when the forge did not say).
        let mut by_branch: HashMap<(&str, &str, &str), Vec<usize>> = HashMap::new();
        for (i, task) in tasks.iter().enumerate() {
            for repository in &task.repositories {
                for branch in &task.branches {
                    by_branch
                        .entry((&task.project, repository, branch))
                        .or_default()
                        .push(i);
                }
            }
        }
        for list in by_branch.values_mut() {
            list.sort_by_key(|&i| tasks[i].created_at);
        }
        let mut merged: Vec<(usize, u64)> = Vec::new();
        for merge in merges.iter().filter(|m| m.merged_at < to) {
            let key = (
                merge.project.as_str(),
                merge.repository.as_str(),
                merge.branch.as_str(),
            );
            let Some(list) = by_branch.get(&key) else {
                continue;
            };
            let opened = merge.opened_at.unwrap_or(merge.merged_at);
            let before = list.partition_point(|&i| tasks[i].created_at <= opened);
            if let Some(at) = before.checked_sub(1) {
                merged.push((list[at], merge.merged_at));
            }
        }
        for (i, at) in merged {
            let task = &mut tasks[i];
            task.merged_at = Some(task.merged_at.map_or(at, |was| was.min(at)));
        }
        Tasks {
            tasks,
            by_name,
            forges,
            to,
        }
    }

    /// Whether one of the task's repositories has a forge declared now.
    fn has_forge(&self, task: &Task) -> bool {
        self.forges
            .get(&task.project)
            .is_some_and(|with| task.repositories.iter().any(|r| with.contains(r)))
    }

    /// Whether the task was removed by the end of the period.
    fn removed(&self, task: &Task) -> bool {
        task.removed_at.is_some_and(|at| at < self.to)
    }

    /// Whether a forge says the task's outcome: the one it had when it was removed, else one of
    /// its repositories has one now.
    fn forge(&self, task: &Task) -> bool {
        match self.removed(task) {
            true => task
                .forge_when_removed
                .unwrap_or_else(|| self.has_forge(task)),
            false => self.has_forge(task),
        }
    }

    /// How the task `i` ended, and when, as of the end of the period; none while it has not.
    fn end(&self, i: usize) -> Option<(Outcome, u64)> {
        let task = &self.tasks[i];
        if let Some(at) = task.merged_at {
            return Some((Outcome::Merged, at));
        }
        let removed_at = task.removed_at.filter(|_| self.removed(task))?;
        self.forge(task).then_some((Outcome::Failed, removed_at))
    }

    /// The task a turn in `workspace` belongs to: the last of that name made before it started.
    fn find(&self, project: &str, workspace: &str, started_at: u64) -> TaskKey {
        let named = self
            .by_name
            .get(&(project.to_owned(), workspace.to_owned()))
            .map_or(&[][..], Vec::as_slice);
        let before = named.partition_point(|&i| self.tasks[i].created_at <= started_at);
        match before.checked_sub(1) {
            Some(at) => TaskKey::Told(named[at]),
            None => TaskKey::Untold(project.to_owned(), workspace.to_owned()),
        }
    }

    /// A task's outcome as of the end of the period. A workspace the events do not tell has no
    /// branch to look for: in progress, unless its project has no forge.
    fn outcome(&self, key: &TaskKey) -> Outcome {
        match key {
            TaskKey::Told(i) => match self.end(*i) {
                Some((outcome, _)) => outcome,
                None if self.forge(&self.tasks[*i]) => Outcome::InProgress,
                None => Outcome::NoForge,
            },
            TaskKey::Untold(project, _) => match self.forges.get(project) {
                Some(with) if !with.is_empty() => Outcome::InProgress,
                _ => Outcome::NoForge,
            },
        }
    }
}
