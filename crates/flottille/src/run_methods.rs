//! The commands of the agents, as the user's clients read them (`docs/protocol.md`, "The run
//! methods"): the runs of a session, a project or the whole daemon with the last line of their
//! output, a run's output, and the user's two answers, stop and dismiss. What the agents do with
//! the same runs is `runs.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use flottille_core::agent::Agent;
use flottille_core::run::{Run, RunState};
use flottille_core::store::Store;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::projects::Failure;
use crate::{runs, tree};

type Answer = Result<Value, Failure>;

/// The ended runs of a list, the latest ones: a session that ran hundreds of commands lists the
/// last few, not all.
const ENDED_LISTED: usize = 30;
/// A run's last line is cut to this many characters.
const LAST_LINE_CHARS: usize = 300;
/// What a list reads of an ended run's output: its last characters, which hold the lines below.
const TAIL_CHARS: usize = 8 * 1024;
/// The lines of an ended run's output a list gives, for the card of its result.
const CARD_LINES: usize = 20;
/// The lines of output `run.output` gives by default and at most.
const TAIL_LINES: usize = 200;
const TAIL_LINES_MAX: usize = 1000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    project: Option<String>,
    /// A root, or any agent of its tree.
    tree: Option<String>,
    /// Also the runs that ended and were not dismissed.
    #[serde(default)]
    ended: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    run: String,
    tail: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Name {
    run: String,
}

pub async fn call(daemon: Arc<Daemon>, method: &str, params: Value) -> Answer {
    let params = if params.is_null() { json!({}) } else { params };
    match method {
        "run.list" => list(&daemon, serde_json::from_value(params)?),
        "run.output" => output(&daemon, serde_json::from_value(params)?),
        "run.stop" => {
            let Name { run } = serde_json::from_value(params)?;
            stop(&daemon, &run).await
        }
        "run.dismiss" => {
            let Name { run } = serde_json::from_value(params)?;
            dismiss(&daemon, &run)
        }
        method => Err(Failure::new(
            "unknown-method",
            format!("unknown method `{method}`"),
        )),
    }
}

fn list(daemon: &Daemon, asked: List) -> Answer {
    let agents = daemon.read(Store::agents)?;
    let by_id: HashMap<&str, &Agent> = agents.iter().map(|a| (a.id.as_str(), a)).collect();
    let (ids, ended_listed) = match (&asked.tree, &asked.project) {
        (Some(agent), _) => {
            if !by_id.contains_key(agent.as_str()) {
                return Err(Failure::new(
                    "not-found",
                    format!("there is no agent {agent}"),
                ));
            }
            let root = tree::root_of(daemon, agent);
            // The commands of a session that ended go with it.
            let over = by_id.get(root.as_str()).is_some_and(|a| a.state.is_final());
            (tree::ids(daemon, &root)?, asked.ended && !over)
        }
        (None, Some(project)) => (
            agents
                .iter()
                .filter(|a| &a.project == project)
                .map(|a| a.id.clone())
                .collect(),
            asked.ended,
        ),
        (None, None) => (agents.iter().map(|a| a.id.clone()).collect(), asked.ended),
    };
    let mut commands: Vec<Value> = Vec::new();
    for stored in daemon.read(|store| store.running_runs_of(&ids))? {
        if let Some(view) = runs::view(daemon, &stored.id) {
            let last = last_line(&view.output);
            commands.push(listed(daemon, &by_id, &view.run, last, None));
        }
    }
    if ended_listed {
        for stored in daemon.read(|store| store.ended_runs_of(&ids, ENDED_LISTED, TAIL_CHARS))? {
            let last = last_line(&stored.output);
            let tail = tail_lines(&stored.output);
            commands.push(listed(daemon, &by_id, &stored, last, Some(tail)));
        }
    }
    Ok(json!({"commands": commands}))
}

/// A run of a list: the run, where it is, its last line, and for one that ended the lines of its
/// output a card shows.
fn listed(
    daemon: &Daemon,
    agents: &HashMap<&str, &Agent>,
    run: &Run,
    last_line: String,
    tail: Option<Vec<String>>,
) -> Value {
    let agent = agents.get(run.agent.as_str());
    let mut shown = run_json(run);
    shown["root"] = tree::root_of(daemon, &run.agent).into();
    shown["project"] = agent.map(|a| a.project.clone()).into();
    shown["workspace"] = agent.and_then(|a| a.workspace.clone()).into();
    shown["last_line"] = last_line.into();
    if let Some(tail) = tail {
        shown["tail"] = tail.into();
    }
    shown
}

fn output(daemon: &Daemon, asked: Output) -> Answer {
    let tail = asked.tail.unwrap_or(TAIL_LINES);
    if !(1..=TAIL_LINES_MAX).contains(&tail) {
        return Err(Failure::refused(format!(
            "tail must be 1 to {TAIL_LINES_MAX} lines"
        )));
    }
    let view = runs::view(daemon, &asked.run).ok_or_else(|| not_found(&asked.run))?;
    let lines: Vec<&str> = view.output.lines().collect();
    let from = lines.len().saturating_sub(tail);
    let mut shown = run_json(&view.run);
    shown["output"] = lines[from..].join("\n").into();
    shown["earlier"] = (from > 0 || view.cut).into();
    Ok(shown)
}

/// Stops the run, all its process group, and answers once it ended. The agent that started it is
/// told it was the user.
async fn stop(daemon: &Arc<Daemon>, id: &str) -> Answer {
    let view = runs::view(daemon, id).ok_or_else(|| not_found(id))?;
    if view.run.state == RunState::Running {
        runs::stop_by_user(daemon, id).await;
    }
    let view = runs::view(daemon, id).ok_or_else(|| not_found(id))?;
    Ok(json!({"run": run_json(&view.run)}))
}

fn dismiss(daemon: &Daemon, id: &str) -> Answer {
    let run = daemon
        .read(|store| store.run(id))?
        .ok_or_else(|| not_found(id))?;
    if run.state == RunState::Running {
        return Err(Failure::refused(format!(
            "run {id} is still running: stop it first"
        )));
    }
    daemon.write(|store| {
        let event = store.dismiss_run(
            id,
            ("run.dismissed", json!({"agent": run.agent, "run": id})),
        )?;
        Ok(((), event))
    })?;
    Ok(json!({"run": id}))
}

fn not_found(id: &str) -> Failure {
    Failure::new("not-found", format!("there is no run {id}"))
}

/// A run as the clients read it, without its output.
fn run_json(run: &Run) -> Value {
    json!({
        "run": run.id,
        "agent": run.agent,
        "name": run.name,
        "line": run.line,
        "folder": run.folder,
        "state": run.state.as_str(),
        "exit_code": run.exit_code,
        "signal": run.signal,
        "started_at": run.started_at,
        "ended_at": run.ended_at,
    })
}

/// The last lines of the end of an output, to the card's size. An output cut by the store's
/// tail starts mid-line: that piece is left out.
fn tail_lines(tail: &str) -> Vec<String> {
    let mut lines: Vec<&str> = tail.trim_end().lines().collect();
    if tail.chars().count() >= TAIL_CHARS && !lines.is_empty() {
        lines.remove(0);
    }
    let from = lines.len().saturating_sub(CARD_LINES);
    lines[from..]
        .iter()
        .map(|line| line.chars().take(LAST_LINE_CHARS).collect())
        .collect()
}

/// The last line of the output with something on it, as the program wrote it.
fn last_line(output: &str) -> String {
    // A line redrawn in place ends with its last state: the window keeps what follows `\r`.
    let line = output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim_end();
    line.chars().take(LAST_LINE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_is_the_last_lines_without_the_piece_the_cut_left_at_its_start() {
        assert_eq!(tail_lines("one\ntwo\n"), ["one", "two"]);
        assert!(tail_lines("").is_empty());
        let cut = format!("rest of a line\nwhole\n{}", "x".repeat(TAIL_CHARS));
        let lines = tail_lines(&cut);
        assert_eq!(lines[0], "whole", "{lines:?}");
        let many: String = (0..30).map(|n| format!("l{n}\n")).collect();
        let lines = tail_lines(&many);
        assert_eq!((lines.len(), lines[0].as_str()), (CARD_LINES, "l10"));
    }

    #[test]
    fn the_last_line_is_the_last_with_something_on_it_cut_to_a_length() {
        assert_eq!(last_line("one\ntwo \n\n  \n"), "two");
        assert_eq!(last_line(""), "");
        assert_eq!(
            last_line(&"x".repeat(LAST_LINE_CHARS + 50)).chars().count(),
            LAST_LINE_CHARS
        );
    }
}
