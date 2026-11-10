//! Review (`docs/extensions.md`, "Review"): a change read with fresh eyes, and what is wrong
//! delivered to the session that made it. The review is a pilot, a child of the reviewed session,
//! that runs its passes as its own children through `review`, then a verifier, and reports the
//! findings once; Flottille checks them, keeps them and delivers those that matter.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use flottille_core::Result;
use flottille_core::agent::Agent;
use flottille_core::protocol::Event;
use flottille_core::secrets;
use flottille_core::store::{Space, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Extension, Guidance, Kind, Methods, Place, Scope, Setting, To, Tool};
use crate::daemon::Daemon;
use crate::git::Class;
use crate::mcp::{Definition, Grant, Outcome};
use crate::projects::Failure;
use crate::providers;

/// The passes a review may run, in their order; the verifier comes after them.
const PASSES: &[&str] = &["spec", "bugs", "standards", "security"];
const VERIFIER: &str = "verifier";
/// Rounds of a session's review, at most: the first, and one re-review after the fixes.
const ROUNDS: u64 = 2;

/// The model a slot left unset runs on, by provider: the strongest it offers, and the one a size
/// smaller for the verifier. A provider not listed, or that has not said what it offers yet, runs
/// on its default.
const STRONGEST: &[(&str, &str)] = &[("claude", "opus")];
const SMALLER: &[(&str, &str)] = &[("claude", "sonnet")];

pub const EXTENSION: Extension = Extension {
    name: "review",
    title: "Review",
    sentence: "Reads a session's change with fresh eyes and delivers what is wrong to it: a pilot, \
               its passes and a verifier, each a model run on your own quota.",
    scope: Scope::Project,
    on_by_default: false,
    place: Place::Tab {
        title: "Review",
        order: 4,
    },
    settings: &[
        Setting {
            key: "passes",
            label: "Passes",
            hint: "What a review reads; the spec pass only for a session that develops a spec",
            kind: Kind::Subset {
                of: PASSES,
                default: &["spec", "bugs", "standards"],
            },
            per_project: true,
        },
        Setting {
            key: "spec_model",
            label: "The spec",
            hint: "Each expected behaviour met, nothing out of scope. Only for a session that builds a spec.",
            kind: Kind::Model,
            per_project: true,
        },
        Setting {
            key: "bugs_model",
            label: "Bugs",
            hint: "Wrong results, lost data, crashes the change brings.",
            kind: Kind::Model,
            per_project: true,
        },
        Setting {
            key: "standards_model",
            label: "Project rules",
            hint: "The project's AGENTS.md, read from the base branch.",
            kind: Kind::Model,
            per_project: true,
        },
        Setting {
            key: "security_model",
            label: "Security",
            hint: "Secrets, injections, access. Off by default.",
            kind: Kind::Model,
            per_project: true,
        },
        Setting {
            key: "verifier_model",
            label: "Checking the findings",
            hint: "Each finding confirmed before it reaches the session.",
            kind: Kind::Model,
            per_project: true,
        },
    ],
    migrations: &["
    -- A review of a session's change: one round. Its pilot, the passes it may run (JSON), the
    -- files its diff changes (JSON), where the diff starts in each repository (JSON
    -- [{path, folder, base, from, rules_from}]), and each repository's HEAD and `git status` before it (JSON
    -- [{path, head, status}]), compared when it reports.
    CREATE TABLE ext_review_reviews (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        project    TEXT NOT NULL,
        session    TEXT NOT NULL,
        workspace  TEXT NOT NULL,
        pilot      TEXT,
        round      INTEGER NOT NULL,
        spec       INTEGER,
        state      TEXT NOT NULL,
        passes     TEXT NOT NULL,
        files      TEXT NOT NULL,
        diffs      TEXT NOT NULL,
        before     TEXT NOT NULL,
        changed    TEXT,
        summary    TEXT,
        why        TEXT,
        started_at INTEGER NOT NULL,
        ended_at   INTEGER
    ) STRICT;
    -- The agents a review launched: its pilot, and the pilot's passes.
    CREATE TABLE ext_review_agents (
        agent  TEXT PRIMARY KEY,
        review INTEGER NOT NULL REFERENCES ext_review_reviews (id),
        kind   TEXT NOT NULL,
        pass   TEXT
    ) STRICT;
    -- A finding, by its identifier in the session's reviews (F1, F2…), matched across rounds.
    CREATE TABLE ext_review_findings (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        session    TEXT NOT NULL,
        key        TEXT NOT NULL,
        review     INTEGER NOT NULL REFERENCES ext_review_reviews (id),
        severity   TEXT NOT NULL,
        confidence TEXT NOT NULL,
        file       TEXT,
        lines      TEXT,
        concerns   TEXT NOT NULL,
        quote      TEXT,
        text       TEXT NOT NULL,
        failing    TEXT,
        status     TEXT NOT NULL,
        settled_in INTEGER,
        UNIQUE (session, key)
    ) STRICT;
    "],
    methods: Methods {
        prefix: "review",
        call,
    },
    actions: &["start"],
    tools: &[TOOL],
    started: starts_no_session,
    heard: &[
        ("item.opened", pull_opened),
        ("agent.ended", agent_ended),
        ("agent.failed", agent_ended),
    ],
    fact,
    launched: kind_of,
    commands: &[("review", typed)],
    guidance: Some(Guidance {
        default: include_str!("guidance.md"),
        limit: 4_000,
        named: "your review's guidance",
    }),
    declared: None,
    dynamic: None,
    needs_you: &[],
};

fn starts_no_session(_: &Daemon, _: &str) -> bool {
    false
}

const ROOT_ACTIONS: &[&str] = &["start"];
const PILOT_ACTIONS: &[&str] = &["run_pass", "report"];

const TOOL: Tool = Tool {
    definition: Definition {
        name: "review",
        title: "Review a change",
        description: "A review of a session's change with fresh eyes (`docs/extensions.md`, \
            \"Review\"). start, for a session: review your own change from its base, the answer \
            and the findings coming later as a delivery [flottille:review]; two rounds at most, \
            the second reading what changed since the first. run_pass, for the pilot of a review: \
            run one pass as your child (pass spec, bugs, standards, security or verifier; brief, \
            what it should look at), each once. report, for the pilot: the findings, once \
            (summary; findings [{severity high|medium|low|info, confidence confirmed|plausible|\
            rejected, file, lines, concerns code or a C or D of the spec, quote, text, case}]; \
            previous [{id, status fixed|open}] in a second round).",
        read_only: false,
        idempotent: false,
        destructive: false,
        schema: tool_schema,
    },
    given: &[
        (To::Roots, ROOT_ACTIONS),
        (To::Launched("pilot"), PILOT_ACTIONS),
    ],
    // Each writes the review in Flottille's store, and launches what the user asked for when they
    // asked for a review: the pilot, then the passes it may run, each once, within the cap.
    harmless: &["start", "run_pass", "report"],
    gate: tool_gate,
    call: tool_call,
};

fn tool_schema() -> Value {
    json!({"type": "object", "properties": {
        "action": {"type": "string", "enum": ["start", "run_pass", "report"]},
        "pass": {"type": "string", "enum": ["spec", "bugs", "standards", "security", "verifier"]},
        "brief": {"type": "string"},
        "summary": {"type": "string"},
        "findings": {"type": "array", "items": {"type": "object", "properties": {
            "severity": {"type": "string", "enum": ["high", "medium", "low", "info"]},
            "confidence": {"type": "string", "enum": ["confirmed", "plausible", "rejected"]},
            "file": {"type": "string"},
            "lines": {"type": "string"},
            "concerns": {"type": "string"},
            "quote": {"type": "string"},
            "text": {"type": "string"},
            "case": {"type": "string"}
        }, "required": ["severity", "confidence", "concerns", "text"]}},
        "previous": {"type": "array", "items": {"type": "object", "properties": {
            "id": {"type": "string"},
            "status": {"type": "string", "enum": ["fixed", "open"]}
        }, "required": ["id", "status"]}}
    }, "required": ["action"]})
}

// The store.

/// A review as its row holds it.
#[derive(Clone, Debug)]
struct Review {
    id: u64,
    project: String,
    session: String,
    workspace: String,
    pilot: Option<String>,
    round: u64,
    spec: Option<u64>,
    state: String,
    passes: Vec<String>,
    files: Vec<String>,
    diffs: Vec<Diff>,
    before: Vec<Snapshot>,
    changed: Option<String>,
    summary: Option<String>,
    why: Option<String>,
    started_at: u64,
    ended_at: Option<u64>,
}

/// Where a repository's diff starts.
#[derive(Clone, Debug, serde::Serialize, Deserialize)]
struct Diff {
    path: String,
    folder: String,
    base: String,
    from: String,
    /// Where the first round's diff started, the base the rules are read from in every round:
    /// a second round's `from` is a commit of the change. Empty in a review written before it
    /// was kept, whose first round's `from` is that base.
    #[serde(default)]
    rules_from: String,
}

impl Diff {
    fn rules_from(&self) -> &str {
        match self.rules_from.as_str() {
            "" => &self.from,
            kept => kept,
        }
    }
}

/// A repository's state, compared after the review.
#[derive(Clone, Debug, PartialEq, serde::Serialize, Deserialize)]
struct Snapshot {
    path: String,
    head: String,
    status: String,
}

const COLUMNS: &str = "id, project, session, workspace, pilot, round, spec, state, passes, files, \
    diffs, before, changed, summary, why, started_at, ended_at";

fn row(row: &rusqlite::Row) -> rusqlite::Result<Review> {
    let json = |at: usize| -> rusqlite::Result<String> { row.get(at) };
    Ok(Review {
        id: row.get(0)?,
        project: row.get(1)?,
        session: row.get(2)?,
        workspace: row.get(3)?,
        pilot: row.get(4)?,
        round: row.get(5)?,
        spec: row.get(6)?,
        state: row.get(7)?,
        passes: serde_json::from_str(&json(8)?).unwrap_or_default(),
        files: serde_json::from_str(&json(9)?).unwrap_or_default(),
        diffs: serde_json::from_str(&json(10)?).unwrap_or_default(),
        before: serde_json::from_str(&json(11)?).unwrap_or_default(),
        changed: row.get(12)?,
        summary: row.get(13)?,
        why: row.get(14)?,
        started_at: row.get(15)?,
        ended_at: row.get(16)?,
    })
}

fn reviews_where(
    tables: &Connection,
    filter: &str,
    value: &dyn rusqlite::ToSql,
) -> Result<Vec<Review>> {
    let mut statement = tables.prepare(&format!(
        "SELECT {COLUMNS} FROM ext_review_reviews WHERE {filter} ORDER BY id"
    ))?;
    let found = statement
        .query_map([value], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(found)
}

/// A finding as it is kept.
#[derive(Clone, Debug, serde::Serialize)]
struct Finding {
    id: String,
    review: u64,
    severity: String,
    confidence: String,
    file: Option<String>,
    lines: Option<String>,
    concerns: String,
    quote: Option<String>,
    text: String,
    case: Option<String>,
    status: String,
    settled_in: Option<u64>,
}

fn findings_of(tables: &Connection, session: &str) -> Result<Vec<Finding>> {
    let mut statement = tables.prepare(
        "SELECT key, review, severity, confidence, file, lines, concerns, quote, text, failing, \
         status, settled_in FROM ext_review_findings WHERE session = ?1 ORDER BY id",
    )?;
    let found = statement
        .query_map([session], |row| {
            Ok(Finding {
                id: row.get(0)?,
                review: row.get(1)?,
                severity: row.get(2)?,
                confidence: row.get(3)?,
                file: row.get(4)?,
                lines: row.get(5)?,
                concerns: row.get(6)?,
                quote: row.get(7)?,
                text: row.get(8)?,
                case: row.get(9)?,
                status: row.get(10)?,
                settled_in: row.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(found)
}

/// What a closure reads or writes in Review's space, with the event it appended, if any.
fn in_space<T>(
    daemon: &Daemon,
    work: impl FnOnce(&Space) -> Result<(T, Option<Event>)>,
) -> Result<T> {
    daemon.write(|store| {
        let space = store.space(EXTENSION.name)?;
        let (value, event) = work(&space)?;
        space.commit()?;
        Ok((value, event))
    })
}

/// What a closure reads of Review's tables, writing nothing.
fn read<T>(daemon: &Daemon, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    daemon.read(|store| store.read_space(EXTENSION.name, work))
}

fn review_of_agent(daemon: &Daemon, agent: &str) -> Option<(Review, String, Option<String>)> {
    read(daemon, |tables| {
        let found: Option<(u64, String, Option<String>)> = tables
            .query_row(
                "SELECT review, kind, pass FROM ext_review_agents WHERE agent = ?1",
                [agent],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((id, kind, pass)) = found else {
            return Ok(None);
        };
        let review = reviews_where(tables, "id = ?1", &id)?.pop();
        Ok(review.map(|review| (review, kind, pass)))
    })
    .ok()
    .flatten()
}

/// Which of Review's agents the agent is: `pilot` or `pass`.
fn kind_of(daemon: &Daemon, agent: &str) -> Option<&'static str> {
    let kind: Option<String> = read(daemon, |tables| {
        Ok(tables
            .query_row(
                "SELECT kind FROM ext_review_agents WHERE agent = ?1",
                [agent],
                |row| row.get(0),
            )
            .optional()?)
    })
    .ok()
    .flatten();
    match kind.as_deref() {
        Some("pilot") => Some("pilot"),
        Some(_) => Some("pass"),
        None => None,
    }
}

// What the tree says of a review's agents.

/// The fact `agent.tree` gives of a review's agent: its review, its state, its passes run of
/// those it may run, and the findings it delivered; for a pass, which one.
fn fact(daemon: &Daemon, agent: &str) -> Option<Value> {
    let (review, kind, pass) = review_of_agent(daemon, agent)?;
    let (done, findings) = progress(daemon, &review);
    // The pilot's carries the review's findings, `info` included, for its line to show them.
    let shown: Vec<Finding> = match kind.as_str() {
        "pilot" => read(daemon, |tables| findings_of(tables, &review.session))
            .unwrap_or_default()
            .into_iter()
            .filter(|finding| finding.review == review.id)
            .collect(),
        _ => Vec::new(),
    };
    Some(json!({
        "review": review.id,
        "kind": kind,
        "pass": pass,
        "round": review.round,
        "state": review.state,
        "passes": {"done": done, "total": review.passes.len()},
        "findings": findings,
        "shown": shown,
        "changed": review.changed,
    }))
}

/// The passes of the review that ended or reported, and the findings it delivered.
fn progress(daemon: &Daemon, review: &Review) -> (usize, usize) {
    let passes: Vec<String> = read(daemon, |tables| {
        let mut statement = tables.prepare(
            "SELECT agent FROM ext_review_agents WHERE review = ?1 AND kind = 'pass' \
             AND pass != 'verifier'",
        )?;
        let agents = statement
            .query_map([review.id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(agents)
    })
    .unwrap_or_default();
    let done = passes
        .iter()
        .filter(|id| {
            daemon
                .read(|store| store.agent(id))
                .ok()
                .flatten()
                .is_some_and(|agent| agent.state.is_final() || agent.reported_at.is_some())
        })
        .count();
    let findings = read(daemon, |tables| {
        Ok(findings_of(tables, &review.session)?
            .into_iter()
            .filter(|finding| finding.review == review.id && delivered(finding))
            .count())
    })
    .unwrap_or_default();
    (done, findings)
}

/// Whether a finding is delivered to the session: confirmed or plausible, and not `info`, which is
/// shown only.
fn delivered(finding: &Finding) -> bool {
    finding.severity != "info" && ["confirmed", "plausible"].contains(&finding.confidence.as_str())
}

/// What a pilot's report and a pass's brief hold at most, in characters, each: they ride whole in a
/// delivery, as a child's report does.
const TEXT_CHARS: usize = 20_000;

// Starting a review.

/// The sessions whose review is starting: two asked at once for one session start one.
static STARTING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// A session's place in `STARTING`, given back when its start ends, whichever way.
struct Starting(String);

impl Starting {
    fn take(session: &str) -> Option<Starting> {
        let mut starting = STARTING.lock().unwrap_or_else(PoisonError::into_inner);
        starting
            .insert(session.to_owned())
            .then(|| Starting(session.to_owned()))
    }
}

impl Drop for Starting {
    fn drop(&mut self) {
        STARTING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// A running review whose pilot is gone or ended, which no `agent.ended` failed (a launch that
/// failed once the review was written, a daemon stopped meanwhile), is failed: it would hold the
/// session's next review back for ever.
fn fail_orphans(daemon: &Daemon, reviews: &[Review]) -> Result<()> {
    for review in reviews.iter().filter(|review| review.state == "running") {
        let pilot = match &review.pilot {
            Some(pilot) => daemon.read(|store| store.agent(pilot))?,
            None => None,
        };
        if pilot.is_some_and(|pilot| !pilot.state.is_final()) {
            continue;
        }
        in_space(daemon, |space| {
            let changed = space.tables().execute(
                "UPDATE ext_review_reviews SET state = 'failed', why = ?2, ended_at = ?3 \
                 WHERE id = ?1 AND state = 'running'",
                params![review.id, "its pilot is gone", now_ms()],
            )?;
            if changed == 0 {
                return Ok(((), None));
            }
            let event = space.append_event(
                "review.failed",
                json!({"review": review.id, "project": review.project,
                    "session": review.session, "agent": review.pilot,
                    "why": "its pilot is gone"}),
            )?;
            Ok(((), Some(event)))
        })?;
    }
    Ok(())
}

/// The review a session asked for, or why not: said before any agent starts.
async fn start(daemon: &Arc<Daemon>, session: &Agent) -> std::result::Result<Value, Failure> {
    let project = super::active(daemon, &EXTENSION, &session.project)?;
    if session.parent.is_some() {
        return Err(Failure::refused(
            "a review reads a session's change: this agent is a child, review its session",
        ));
    }
    if session.state.is_final() {
        return Err(Failure::refused(format!(
            "session {} ended: a review is delivered to a session that runs",
            session.id
        )));
    }
    let Some(workspace_name) = session.workspace.clone() else {
        return Err(Failure::refused(format!(
            "session {} works in the project folder (main): a review reads a workspace's change \
             from its base",
            session.id
        )));
    };
    let workspace = daemon
        .read(|store| store.workspace(&project.name, &workspace_name))?
        .ok_or_else(|| {
            Failure::new(
                "not-found",
                format!("the workspace {workspace_name} of the session is gone"),
            )
        })?;
    let Some(_starting) = Starting::take(&session.id) else {
        return Err(Failure::refused(format!(
            "a review of session {} runs already: its findings come as a delivery",
            session.id
        )));
    };
    let of_session = |daemon: &Daemon| {
        read(daemon, |tables| {
            reviews_where(tables, "session = ?1", &session.id)
        })
    };
    fail_orphans(daemon, &of_session(daemon)?)?;
    let earlier = of_session(daemon)?;
    if earlier.iter().any(|review| review.state == "running") {
        return Err(Failure::refused(format!(
            "a review of session {} runs already: its findings come as a delivery",
            session.id
        )));
    }
    let done: Vec<&Review> = earlier.iter().filter(|r| r.state == "done").collect();
    let round = done.len() as u64 + 1;
    if round > ROUNDS {
        return Err(Failure::refused(format!(
            "session {} had its {ROUNDS} rounds of review: what is left stays open",
            session.id
        )));
    }
    let open = read(daemon, |tables| {
        Ok(findings_of(tables, &session.id)?
            .into_iter()
            .filter(|finding| finding.status == "open" && delivered(finding))
            .collect::<Vec<_>>())
    })?;
    // Where the diff starts: the base, or what the first round read.
    let mut diffs = Vec::new();
    let mut files = Vec::new();
    let mut before = Vec::new();
    for tree in &workspace.repositories {
        let folder = workspace.folder_of(&tree.path);
        let base = git_line(&folder, &["merge-base", &tree.base.reference, "HEAD"])
            .await
            .unwrap_or_else(|| tree.base.commit.clone());
        let first = done.first();
        let from = match first {
            Some(first) => first
                .before
                .iter()
                .find(|snapshot| snapshot.path == tree.path)
                .map(|snapshot| snapshot.head.clone())
                .unwrap_or_else(|| tree.base.commit.clone()),
            None => base.clone(),
        };
        // The rules are the base's, never a commit the session made before the first round.
        let rules_from = first
            .and_then(|first| first.diffs.iter().find(|diff| diff.path == tree.path))
            .map_or(base, |diff| diff.rules_from().to_owned());
        let mut changed = git_lines(&folder, &["diff", "--name-only", &from]).await;
        changed.extend(git_lines(&folder, &["ls-files", "--others", "--exclude-standard"]).await);
        for file in changed {
            let file = match tree.path.as_str() {
                "." => file,
                path => format!("{path}/{file}"),
            };
            if !files.contains(&file) {
                files.push(file);
            }
        }
        diffs.push(Diff {
            path: tree.path.clone(),
            folder: folder.display().to_string(),
            base: tree.base.branch.clone(),
            from,
            rules_from,
        });
        before.push(snapshot(&tree.path, &folder).await);
    }
    if files.is_empty() && (round == 1 || open.is_empty()) {
        let bases: Vec<&str> = diffs.iter().map(|d| d.base.as_str()).collect();
        return Err(Failure::refused(match round {
            1 => format!(
                "nothing to review: workspace {workspace_name} has no change from its base ({})",
                bases.join(", ")
            ),
            _ => format!(
                "nothing to review again: workspace {workspace_name} did not change since the \
                 first round, and no finding is open"
            ),
        }));
    }
    let spec = super::specs::developed_by(daemon, &session.id);
    let enabled = super::value(daemon, &EXTENSION, &project, "passes")?;
    let passes: Vec<String> = enabled
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|pass| *pass != "spec" || spec.is_some())
        .map(str::to_owned)
        .collect();
    let instructions = instructions_of(daemon, "pilot", None)?;
    let brief = pilot_brief(
        &workspace_name,
        round,
        &diffs,
        &files,
        &passes,
        spec.as_ref(),
        &open,
    );
    let slot = slot(daemon, &project, session, "pilot")?;
    let mut id = 0;
    let pilot = super::launch(
        daemon,
        &EXTENSION,
        session,
        "pilot",
        &format!("Review, round {round}"),
        &brief,
        instructions,
        slot,
        |pilot| {
            id = in_space(daemon, |space| {
                space.tables().execute(
                    "INSERT INTO ext_review_reviews (project, session, workspace, pilot, round, \
                     spec, state, passes, files, diffs, before, started_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, ?9, ?10, ?11)",
                    params![
                        project.name,
                        session.id,
                        workspace_name,
                        pilot.id,
                        round,
                        spec.as_ref().map(|spec| spec.id),
                        json!(passes).to_string(),
                        json!(files).to_string(),
                        json!(diffs).to_string(),
                        json!(before).to_string(),
                        now_ms(),
                    ],
                )?;
                let id = space.tables().last_insert_rowid() as u64;
                space.tables().execute(
                    "INSERT INTO ext_review_agents (agent, review, kind) VALUES (?1, ?2, 'pilot')",
                    params![pilot.id, id],
                )?;
                let event = space.append_event(
                    "review.started",
                    json!({"review": id, "project": project.name, "session": session.id,
                        "agent": pilot.id, "round": round, "passes": passes}),
                )?;
                Ok((id, Some(event)))
            })
            .map_err(|error| error.to_string())?;
            Ok(())
        },
    )
    .map_err(|failure| {
        // Written in `before`, the review whose pilot was never written goes with it.
        if id != 0 {
            let _ = in_space(daemon, |space| {
                space
                    .tables()
                    .execute("DELETE FROM ext_review_agents WHERE review = ?1", [id])?;
                space
                    .tables()
                    .execute("DELETE FROM ext_review_reviews WHERE id = ?1", [id])?;
                Ok(((), None))
            });
        }
        Failure::new(
            failure.code,
            format!("the review cannot start: {}", failure.message),
        )
    })?;
    Ok(
        json!({"review": id, "session": session.id, "pilot": pilot.id, "round": round,
        "passes": passes, "files": files}),
    )
}

async fn git_line(folder: &std::path::Path, args: &[&str]) -> Option<String> {
    crate::git::output(folder, args, Class::Read)
        .await
        .ok()
        .map(|out| out.trim().to_owned())
        .filter(|out| !out.is_empty())
}

async fn git_lines(folder: &std::path::Path, args: &[&str]) -> Vec<String> {
    crate::git::output(folder, args, Class::Read)
        .await
        .map(|out| out.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

async fn snapshot(path: &str, folder: &std::path::Path) -> Snapshot {
    Snapshot {
        path: path.to_owned(),
        head: git_line(folder, &["rev-parse", "HEAD"])
            .await
            .unwrap_or_default(),
        status: crate::git::output(
            folder,
            &["status", "--porcelain=v1", "--untracked-files=all"],
            Class::Read,
        )
        .await
        .unwrap_or_default(),
    }
}

/// The model an agent of the review runs on: its slot in the project or in Review's tab, else
/// the session's provider, on its strongest model (a size smaller for the verifier) once it said
/// what it offers. The pilot runs as the strongest.
fn slot(
    daemon: &Daemon,
    project: &flottille_core::project::Project,
    session: &Agent,
    pass: &str,
) -> std::result::Result<providers::Asked, Failure> {
    if pass != "pilot" {
        let set = super::value(daemon, &EXTENSION, project, &format!("{pass}_model"))?;
        if let Some(slot) = set.as_object() {
            let text = |key: &str| slot.get(key).and_then(Value::as_str).map(str::to_owned);
            return Ok(providers::Asked {
                provider: text("provider"),
                model: text("model"),
                effort: text("effort"),
            });
        }
    }
    let provider = providers::get(&session.provider);
    let named = |list: &[(&str, &str)]| {
        let provider = provider?;
        let (_, model) = list.iter().find(|(name, _)| *name == provider.name)?;
        providers::offered_model(daemon, provider, model)
    };
    let model = match pass {
        VERIFIER => named(SMALLER).or_else(|| named(STRONGEST)),
        _ => named(STRONGEST),
    };
    Ok(providers::Asked {
        provider: Some(session.provider.clone()),
        model,
        effort: None,
    })
}

/// An agent's instructions: Review's fixed text, the guidance, and for the standards pass the
/// project's rules read from the base branch.
fn instructions_of(
    daemon: &Daemon,
    what: &str,
    rules: Option<&str>,
) -> std::result::Result<String, Failure> {
    let guidance = super::guidance::text(daemon, &EXTENSION).map_err(Failure::refused)?;
    let fixed = match what {
        "pilot" => include_str!("pilot.md"),
        "spec" => include_str!("spec.md"),
        "bugs" => include_str!("bugs.md"),
        "standards" => include_str!("standards.md"),
        "security" => include_str!("security.md"),
        _ => include_str!("verifier.md"),
    };
    let mut text = fixed.trim_end().to_owned();
    if what != "pilot" {
        text.push_str(&format!(
            "\n\n{}",
            include_str!("pass-common.md").trim_end()
        ));
    }
    text.push_str(&format!(
        "\n\n## The user's guidance for reviews\n\n{}",
        guidance.trim_end()
    ));
    if let Some(rules) = rules {
        text.push_str(&format!(
            "\n\n## The project's rules, read from the base branch\n\n{}",
            rules.trim_end()
        ));
    }
    let env = std::env::vars_os().collect();
    Ok(secrets::mask(&text, &env))
}

/// What the diff is: where it starts in each repository, and the files it changes.
fn diff_said(diffs: &[Diff], files: &[String]) -> String {
    let mut text = String::from("The change, in each repository (run the command in its folder):");
    for diff in diffs {
        text.push_str(&format!(
            "\n- {} ({}), base {}: `git diff {}`",
            diff.path, diff.folder, diff.base, diff.from
        ));
    }
    text.push_str("\n\nThe files it changes:");
    for file in files {
        text.push_str(&format!("\n- {file}"));
    }
    if files.is_empty() {
        text.push_str(" none since the first round.");
    }
    text
}

fn spec_said(spec: &super::specs::Developed) -> String {
    let mut text = format!("The spec it builds, \"{}\" (spec {}):", spec.title, spec.id);
    for (id, said) in spec.behaviour.iter().chain(&spec.decisions) {
        text.push_str(&format!("\n- {id}: {said}"));
    }
    for outside in &spec.out_of_scope {
        text.push_str(&format!("\n- out of scope: {outside}"));
    }
    text
}

fn pilot_brief(
    workspace: &str,
    round: u64,
    diffs: &[Diff],
    files: &[String],
    passes: &[String],
    spec: Option<&super::specs::Developed>,
    open: &[Finding],
) -> String {
    let mut text = format!(
        "Review, round {round} of {ROUNDS}, of the change of workspace {workspace}.\n\n{}",
        diff_said(diffs, files)
    );
    match spec {
        Some(spec) => text.push_str(&format!("\n\n{}", spec_said(spec))),
        None => text.push_str("\n\nThe session develops no spec: the spec pass is skipped."),
    }
    text.push_str(&format!(
        "\n\nThe passes you may run, each once: {}; then {VERIFIER} on the candidates.",
        passes.join(", ")
    ));
    if round > 1 {
        text.push_str(
            "\n\nThis round reads what changed since the first, and the findings still open: \
             give each in previous, fixed or open.",
        );
        for finding in open {
            text.push_str(&format!("\n- {}", finding_line(finding)));
        }
    }
    let env = std::env::vars_os().collect();
    secrets::mask(&text, &env)
}

fn finding_line(finding: &Finding) -> String {
    let place = match (&finding.file, &finding.lines) {
        (Some(file), Some(lines)) => format!(" at {file}:{lines}"),
        (Some(file), None) => format!(" in {file}"),
        _ => String::new(),
    };
    let concerns = match &finding.quote {
        Some(quote) => format!("{} (\"{quote}\")", finding.concerns),
        None => finding.concerns.clone(),
    };
    let mut line = format!(
        "{} [{}, {}] {concerns}{place}: {}",
        finding.id, finding.severity, finding.confidence, finding.text
    );
    if let Some(case) = &finding.case {
        line.push_str(&format!(" Failing case: {case}"));
    }
    line
}

// The doors.

/// `/review` typed in a session's composer.
fn typed(daemon: Arc<Daemon>, session: Agent) -> super::Answered {
    Box::pin(async move { start(&daemon, &session).await })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Of {
    agent: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listed {
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    spec: Option<u64>,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settle {
    session: String,
    finding: String,
    status: String,
}

/// `review.start {agent}`, `review.list {session | spec | project}`, `review.finding {session,
/// finding, status}`: the user's door.
fn call(daemon: Arc<Daemon>, method: String, params: Value) -> super::Answered {
    Box::pin(async move {
        match method.as_str() {
            "review.start" => {
                let Of { agent } = serde_json::from_value(params)?;
                let session = daemon
                    .read(|store| store.agent(&agent))?
                    .ok_or_else(|| Failure::new("not-found", format!("no agent {agent}")))?;
                start(&daemon, &session).await
            }
            "review.list" => list(&daemon, serde_json::from_value(params)?),
            "review.finding" => settle(&daemon, serde_json::from_value(params)?),
            method => Err(Failure::new(
                "unknown-method",
                format!("unknown method `{method}`"),
            )),
        }
    })
}

fn review_json(daemon: &Daemon, review: &Review, findings: &[Finding]) -> Value {
    let agents: Vec<Value> = read(daemon, |tables| {
        let mut statement = tables.prepare(
            "SELECT agent, kind, pass FROM ext_review_agents WHERE review = ?1 ORDER BY rowid",
        )?;
        let agents = statement
            .query_map([review.id], |row| {
                Ok(
                    json!({"agent": row.get::<_, String>(0)?, "kind": row.get::<_, String>(1)?,
                    "pass": row.get::<_, Option<String>>(2)?}),
                )
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(agents)
    })
    .unwrap_or_default();
    let (done, delivered) = progress(daemon, review);
    json!({
        "review": review.id,
        "project": review.project,
        "session": review.session,
        "workspace": review.workspace,
        "pilot": review.pilot,
        "round": review.round,
        "spec": review.spec,
        "state": review.state,
        "passes": review.passes,
        "passes_done": done,
        "files": review.files,
        "changed": review.changed,
        "summary": review.summary,
        "why": review.why,
        "started_at": review.started_at,
        "ended_at": review.ended_at,
        "agents": agents,
        "delivered": delivered,
        "findings": findings.iter().filter(|f| f.review == review.id).collect::<Vec<_>>(),
    })
}

fn list(daemon: &Daemon, asked: Listed) -> std::result::Result<Value, Failure> {
    let reviews = read(daemon, |tables| {
        let reviews = match (&asked.session, asked.spec, &asked.project) {
            (Some(session), _, _) => reviews_where(tables, "session = ?1", session)?,
            (None, Some(spec), _) => reviews_where(tables, "spec = ?1", &spec)?,
            (None, None, Some(project)) => reviews_where(tables, "project = ?1", project)?,
            (None, None, None) => reviews_where(tables, "1 = ?1", &1)?,
        };
        let mut all = Vec::new();
        for review in reviews {
            let findings = findings_of(tables, &review.session)?;
            all.push((review, findings));
        }
        Ok(all)
    })?;
    // Off in a project, Review shows nothing there: its data stays for when it is on again.
    let reviews: Vec<(Review, Vec<Finding>)> = reviews
        .into_iter()
        .filter(|(review, _)| super::active(daemon, &EXTENSION, &review.project).is_ok())
        .collect();
    Ok(json!({"reviews": reviews
        .iter()
        .map(|(review, findings)| review_json(daemon, review, findings))
        .collect::<Vec<_>>()}))
}

/// `review.finding`: the user marks a finding open, fixed, dismissed or to settle with them.
fn settle(daemon: &Daemon, asked: Settle) -> std::result::Result<Value, Failure> {
    if !["open", "fixed", "dismissed", "settle"].contains(&asked.status.as_str()) {
        return Err(Failure::refused(format!(
            "a finding is open, fixed, dismissed or settle (to settle with you), not {}",
            asked.status
        )));
    }
    let changed = in_space(daemon, |space| {
        let changed = space.tables().execute(
            "UPDATE ext_review_findings SET status = ?3 WHERE session = ?1 AND key = ?2",
            params![asked.session, asked.finding, asked.status],
        )?;
        if changed == 0 {
            return Ok((false, None));
        }
        let pilot: Option<String> = space.tables().query_row(
            "SELECT r.pilot FROM ext_review_findings f JOIN ext_review_reviews r \
             ON r.id = f.review WHERE f.session = ?1 AND f.key = ?2",
            params![asked.session, asked.finding],
            |row| row.get(0),
        )?;
        let event = space.append_event(
            "review.finding_changed",
            json!({"session": asked.session, "finding": asked.finding, "status": asked.status,
                "agent": pilot}),
        )?;
        Ok((true, Some(event)))
    })?;
    if !changed {
        return Err(Failure::new(
            "not-found",
            format!("session {} has no finding {}", asked.session, asked.finding),
        ));
    }
    Ok(json!({"session": asked.session, "finding": asked.finding, "status": asked.status}))
}

/// The Core event trigger on a pull request opened: the session whose workspace's branch it is
/// gets a review, as `/review` would start one. Refused at the cap, the session is told, and
/// `/review` can be run later; refused for nothing to read or one running, nothing is said.
fn pull_opened(daemon: &Arc<Daemon>, data: &Value) {
    if data["kind"] != "pull" {
        return;
    }
    let (Some(project), Some(key)) = (data["project"].as_str(), data["key"].as_str()) else {
        return;
    };
    if super::active(daemon, &EXTENSION, project).is_err() {
        return;
    }
    let filter = flottille_core::item::Filter {
        kind: Some("pull".to_owned()),
        ..Default::default()
    };
    let Some(head) = daemon
        .read(|store| store.items(project, &filter))
        .ok()
        .into_iter()
        .flatten()
        .find(|pull| pull.key == key && data["repository"] == pull.repository.as_str())
        .and_then(|pull| pull.details["head"].as_str().map(str::to_owned))
    else {
        return;
    };
    let Ok(workspaces) = daemon.read(|store| store.workspaces(Some(project))) else {
        return;
    };
    let Some(workspace) = workspaces.into_iter().find(|w| w.branch == head) else {
        return;
    };
    let sessions: Vec<Agent> = daemon
        .read(|store| store.agents())
        .unwrap_or_default()
        .into_iter()
        .filter(|agent| {
            agent.project == project
                && agent.parent.is_none()
                && !agent.state.is_final()
                && agent.workspace.as_deref() == Some(workspace.name.as_str())
        })
        .collect();
    for session in sessions {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            // The children cap: a refusal of now, the one the session can ask again later.
            if let Err(failure) = start(&daemon, &session).await
                && failure.code == "unavailable"
            {
                super::tell(
                    &daemon,
                    &EXTENSION,
                    &session.id,
                    &format!(
                        "Your pull request was opened, and its review could not start: {} Ask \
                         for it later with /review, or the review tool.",
                        failure.message
                    ),
                );
            }
        });
    }
}

/// The Core event trigger on an agent that ended or failed: a pilot that ends without its report
/// fails its review, and the session is told; a pass that ends moves the review's progress.
fn agent_ended(daemon: &Arc<Daemon>, data: &Value) {
    let Some(agent) = data["agent"].as_str() else {
        return;
    };
    let Some((review, kind, _)) = review_of_agent(daemon, agent) else {
        return;
    };
    if review.state != "running" {
        return;
    }
    if kind != "pilot" {
        let _ = in_space(daemon, |space| {
            let event = space.append_event(
                "review.progress",
                json!({"review": review.id, "project": review.project,
                    "session": review.session, "agent": review.pilot}),
            )?;
            Ok(((), Some(event)))
        });
        return;
    }
    let reason = data["reason"].as_str().unwrap_or("it ended").to_owned();
    let failed = in_space(daemon, |space| {
        let changed = space.tables().execute(
            "UPDATE ext_review_reviews SET state = 'failed', why = ?2, ended_at = ?3 \
             WHERE id = ?1 AND state = 'running'",
            params![review.id, reason, now_ms()],
        )?;
        if changed == 0 {
            return Ok((false, None));
        }
        let event = space.append_event(
            "review.failed",
            json!({"review": review.id, "project": review.project, "session": review.session,
                "agent": review.pilot, "why": reason}),
        )?;
        Ok((true, Some(event)))
    })
    .unwrap_or(false);
    if failed {
        super::tell(
            daemon,
            &EXTENSION,
            &review.session,
            &format!(
                "The review of your change (round {}) ended without its findings: {reason}. It \
                 counts as no round: ask for it again with /review, or the review tool.",
                review.round
            ),
        );
    }
}

// The tool.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCall {
    action: String,
    #[serde(default)]
    pass: Option<String>,
    #[serde(default)]
    brief: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    findings: Vec<Reported>,
    #[serde(default)]
    previous: Vec<Previous>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reported {
    severity: String,
    confidence: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    lines: Option<String>,
    concerns: String,
    #[serde(default)]
    quote: Option<String>,
    text: String,
    #[serde(default)]
    case: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Previous {
    id: String,
    status: String,
}

fn tool_gate(_: &Daemon, _: &Grant, _: &Value) -> std::result::Result<(), String> {
    Ok(())
}

fn tool_call(daemon: &Arc<Daemon>, grant: &Grant, args: Value) -> super::Called {
    let (daemon, agent) = (daemon.clone(), grant.agent.clone());
    Box::pin(async move {
        let asked: ToolCall = match serde_json::from_value(args) {
            Ok(asked) => asked,
            Err(error) => return Outcome::Refused(format!("refused: review: {error}")),
        };
        let caller = match daemon.read(|store| store.agent(&agent)) {
            Ok(Some(caller)) => caller,
            _ => return Outcome::Failed(format!("there is no agent {agent}")),
        };
        let done = match asked.action.as_str() {
            "start" => start(&daemon, &caller)
                .await
                .map(|started| {
                    format!(
                        "Started the review of your change, round {} of {ROUNDS}: review {}, its \
                         pilot agent {}. It is not yours to steer: the agent_* tools do not reach \
                         it, and its findings come to you as [flottille:review] once it reports. \
                         Go on, or end your turn.",
                        started["round"],
                        started["review"],
                        started["pilot"].as_str().unwrap_or_default()
                    )
                })
                .map_err(|failure| failure.message),
            "run_pass" => run_pass(&daemon, &caller, asked.pass, asked.brief).await,
            "report" => report(&daemon, &caller, asked).await,
            other => Err(format!("review takes no action {other}")),
        };
        match done {
            Ok(text) => Outcome::Done(text),
            Err(why) => Outcome::Refused(format!("refused: {why}")),
        }
    })
}

/// The pilot runs one of its review's passes as its child, once.
async fn run_pass(
    daemon: &Arc<Daemon>,
    pilot: &Agent,
    pass: Option<String>,
    brief: Option<String>,
) -> std::result::Result<String, String> {
    let (review, kind, _) =
        review_of_agent(daemon, &pilot.id).ok_or("you are not the pilot of a review")?;
    if kind != "pilot" {
        return Err("only the pilot of a review runs its passes".into());
    }
    if review.state != "running" {
        return Err(format!(
            "the review is {}: it runs nothing more",
            review.state
        ));
    }
    let pass = pass.ok_or("run_pass needs pass, the one to run")?;
    let length = brief.as_deref().map_or(0, |brief| brief.chars().count());
    if length > TEXT_CHARS {
        return Err(format!(
            "the brief is {length} characters, over {TEXT_CHARS}: say what the pass should look \
             at in fewer words"
        ));
    }
    if pass == "spec" && review.spec.is_none() {
        return Err("the session develops no spec: the spec pass is skipped".into());
    }
    if pass != VERIFIER && !review.passes.contains(&pass) {
        return Err(format!(
            "this review may run the passes {} and the {VERIFIER}, not {pass}",
            review.passes.join(", ")
        ));
    }
    let ran: bool = read(daemon, |tables| {
        Ok(tables
            .query_row(
                "SELECT 1 FROM ext_review_agents WHERE review = ?1 AND pass = ?2",
                params![review.id, pass],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    })
    .map_err(|error| error.to_string())?;
    if ran {
        return Err(format!(
            "the {pass} pass ran already in this review: each runs once"
        ));
    }
    let project = crate::projects::load_project(daemon, &review.project).map_err(|f| f.message)?;
    let session = daemon
        .read(|store| store.agent(&review.session))
        .ok()
        .flatten()
        .ok_or("the reviewed session is gone")?;
    let rules = match pass.as_str() {
        "standards" => Some(base_rules(&review).await),
        _ => None,
    };
    let instructions =
        instructions_of(daemon, &pass, rules.as_deref()).map_err(|failure| failure.message)?;
    let mut text = diff_said(&review.diffs, &review.files);
    if let Some(spec) = review
        .spec
        .and_then(|_| super::specs::developed_by(daemon, &review.session))
    {
        text.push_str(&format!("\n\n{}", spec_said(&spec)));
    }
    if let Some(brief) = brief.filter(|brief| !brief.trim().is_empty()) {
        text.push_str(&format!("\n\nFrom the pilot of the review:\n\n{brief}"));
    }
    let slot = slot(daemon, &project, &session, &pass).map_err(|failure| failure.message)?;
    let child = super::launch(
        daemon,
        &EXTENSION,
        pilot,
        "pass",
        &format!("Review: {pass}"),
        &text,
        instructions,
        slot,
        |child| {
            in_space(daemon, |space| {
                space.tables().execute(
                    "INSERT INTO ext_review_agents (agent, review, kind, pass) \
                     VALUES (?1, ?2, 'pass', ?3)",
                    params![child.id, review.id, pass],
                )?;
                let event = space.append_event(
                    "review.progress",
                    json!({"review": review.id, "project": review.project,
                        "session": review.session, "agent": review.pilot}),
                )?;
                Ok(((), Some(event)))
            })
            .map_err(|error| error.to_string())
        },
    )
    .map_err(|failure| failure.message)?;
    Ok(format!(
        "Started the {pass} pass, agent {}: its report comes to you as [flottille:child-report].",
        child.id
    ))
}

/// The project's rules as the base branch has them, in each repository: `CLAUDE.md` else
/// `AGENTS.md`, never the workspace's, which the change may have rewritten. A symbolic link, as a
/// `CLAUDE.md` pointing at `AGENTS.md` is, would give its target's name, not its rules: skipped.
async fn base_rules(review: &Review) -> String {
    let mut text = String::new();
    for diff in &review.diffs {
        let folder = std::path::Path::new(&diff.folder);
        for name in ["CLAUDE.md", "AGENTS.md"] {
            let entry = git_line(folder, &["ls-tree", diff.rules_from(), "--", name]).await;
            if entry.is_none_or(|entry| entry.starts_with("120000 ")) {
                continue;
            }
            let shown = crate::git::output(
                folder,
                &["show", &format!("{}:{name}", diff.rules_from())],
                Class::Read,
            )
            .await;
            if let Ok(rules) = shown {
                text.push_str(&format!(
                    "### {name} of {} (base {})\n\n{}\n\n",
                    diff.path,
                    diff.base,
                    rules.trim_end()
                ));
                break;
            }
        }
    }
    if text.is_empty() {
        text.push_str("The base branch holds no AGENTS.md nor CLAUDE.md.");
    }
    text
}

const SEVERITIES: &[&str] = &["high", "medium", "low", "info"];

/// The pilot's report: checked, kept, and what matters delivered to the session, once.
async fn report(
    daemon: &Arc<Daemon>,
    pilot: &Agent,
    asked: ToolCall,
) -> std::result::Result<String, String> {
    let (review, kind, _) =
        review_of_agent(daemon, &pilot.id).ok_or("you are not the pilot of a review")?;
    if kind != "pilot" {
        return Err("only the pilot of a review reports its findings".into());
    }
    if review.state != "running" {
        return Err(format!(
            "the review is {}, no longer running: it takes no report",
            review.state
        ));
    }
    let length = asked.summary.as_deref().map_or(0, |s| s.chars().count())
        + asked
            .findings
            .iter()
            .map(|finding| {
                [
                    Some(&finding.text),
                    Some(&finding.concerns),
                    finding.quote.as_ref(),
                    finding.case.as_ref(),
                    finding.file.as_ref(),
                    finding.lines.as_ref(),
                ]
                .into_iter()
                .flatten()
                .map(|text| text.chars().count())
                .sum::<usize>()
            })
            .sum::<usize>();
    if length > TEXT_CHARS {
        return Err(format!(
            "the report is {length} characters, over {TEXT_CHARS}: say each finding in fewer \
             words"
        ));
    }
    let spec = review
        .spec
        .and_then(|_| super::specs::developed_by(daemon, &review.session));
    for (at, finding) in asked.findings.iter().enumerate() {
        checked(finding, &review, spec.as_ref())
            .map_err(|why| format!("finding {}: {why}", at + 1))?;
    }
    let earlier = read(daemon, |tables| findings_of(tables, &review.session))
        .map_err(|error| error.to_string())?;
    for previous in &asked.previous {
        if !["fixed", "open"].contains(&previous.status.as_str()) {
            return Err(format!(
                "previous {}: a finding of the first round is fixed or open, not {}",
                previous.id, previous.status
            ));
        }
        if !earlier
            .iter()
            .any(|finding| finding.id == previous.id && finding.review != review.id)
        {
            return Err(format!(
                "previous {}: not a finding of an earlier round of this session",
                previous.id
            ));
        }
    }
    let changed = workspace_changed(&review).await;
    // Turned off while it worked: what it reports is not applied.
    let off = super::active(daemon, &EXTENSION, &review.project).is_err();
    let env = std::env::vars_os().collect();
    let masked = |text: &str| secrets::mask(text, &env);
    // Ended first, while still running: a review failed meanwhile, or a report sent at the same
    // time, leaves it as it is.
    let kept: Option<Vec<Finding>> = in_space(daemon, |space| {
        if off {
            let changed = space.tables().execute(
                "UPDATE ext_review_reviews SET state = 'failed', why = ?2, ended_at = ?3 \
                 WHERE id = ?1 AND state = 'running'",
                params![review.id, "Review was turned off", now_ms()],
            )?;
            return Ok(((changed > 0).then(Vec::new), None));
        }
        let summary = asked.summary.as_deref().map(masked);
        let changed_rows = space.tables().execute(
            "UPDATE ext_review_reviews SET state = 'done', summary = ?2, changed = ?3, \
             ended_at = ?4 WHERE id = ?1 AND state = 'running'",
            params![review.id, summary, changed, now_ms()],
        )?;
        if changed_rows == 0 {
            return Ok((None, None));
        }
        let mut next: u64 = space.tables().query_row(
            "SELECT count(*) FROM ext_review_findings WHERE session = ?1",
            [&review.session],
            |row| row.get(0),
        )?;
        let mut kept = Vec::new();
        for finding in &asked.findings {
            if finding.confidence == "rejected" {
                continue;
            }
            next += 1;
            let kept_one = Finding {
                id: format!("F{next}"),
                review: review.id,
                severity: finding.severity.clone(),
                confidence: finding.confidence.clone(),
                file: finding.file.clone().filter(|f| !f.trim().is_empty()),
                lines: finding.lines.clone().filter(|l| !l.trim().is_empty()),
                concerns: finding.concerns.trim().to_owned(),
                quote: finding
                    .quote
                    .as_deref()
                    .map(masked)
                    .filter(|q| !q.trim().is_empty()),
                text: masked(&finding.text),
                case: finding
                    .case
                    .as_deref()
                    .map(masked)
                    .filter(|c| !c.trim().is_empty()),
                status: "open".into(),
                settled_in: None,
            };
            space.tables().execute(
                "INSERT INTO ext_review_findings (session, key, review, severity, confidence, \
                 file, lines, concerns, quote, text, failing, status) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'open')",
                params![
                    review.session,
                    kept_one.id,
                    review.id,
                    kept_one.severity,
                    kept_one.confidence,
                    kept_one.file,
                    kept_one.lines,
                    kept_one.concerns,
                    kept_one.quote,
                    kept_one.text,
                    kept_one.case,
                ],
            )?;
            kept.push(kept_one);
        }
        for previous in &asked.previous {
            space.tables().execute(
                "UPDATE ext_review_findings SET status = ?3, settled_in = ?4 \
                 WHERE session = ?1 AND key = ?2",
                params![review.session, previous.id, previous.status, review.id],
            )?;
        }
        let event = space.append_event(
            "review.reported",
            json!({"review": review.id, "project": review.project, "session": review.session,
                "agent": review.pilot, "round": review.round,
                "findings": kept.iter().filter(|f| delivered(f)).count(),
                "shown": kept.len(), "changed": changed}),
        )?;
        Ok((Some(kept), Some(event)))
    })
    .map_err(|error| error.to_string())?;
    let Some(kept) = kept else {
        return Err("the review is no longer running: it takes no report".into());
    };
    // Its session ends with its turn, as a child's after `report`; its passes still running stop.
    let _ = super::reported(daemon, &pilot.id);
    for pass in crate::tree::descendants(daemon, &pilot.id) {
        crate::tree::stop_tree(daemon, &pass.id, "its review reported");
    }
    if off {
        return Ok(
            "Review was turned off meanwhile: your report is not applied. End your turn.".into(),
        );
    }
    super::tell(
        daemon,
        &EXTENSION,
        &review.session,
        &delivery(
            &review,
            &kept,
            &asked.previous,
            asked.summary.as_deref(),
            changed.as_deref(),
        ),
    );
    Ok(format!(
        "Your report went to the session: {} findings kept, {} delivered. Your session ends with \
         this turn: end it now.",
        kept.len(),
        kept.iter().filter(|f| delivered(f)).count()
    ))
}

/// What is wrong with a finding as the pilot gave it.
fn checked(
    finding: &Reported,
    review: &Review,
    spec: Option<&super::specs::Developed>,
) -> std::result::Result<(), String> {
    if !SEVERITIES.contains(&finding.severity.as_str()) {
        return Err(format!(
            "severity is high, medium, low or info, not {}",
            finding.severity
        ));
    }
    if !["confirmed", "plausible", "rejected"].contains(&finding.confidence.as_str()) {
        return Err(format!(
            "confidence is confirmed, plausible or rejected, the verifier's word, not {}",
            finding.confidence
        ));
    }
    if finding.text.trim().is_empty() {
        return Err("text, what is wrong, is empty".into());
    }
    let given = |text: &Option<String>| text.as_deref().is_some_and(|t| !t.trim().is_empty());
    if ["high", "medium"].contains(&finding.severity.as_str())
        && finding.confidence != "rejected"
        && !(given(&finding.file) && given(&finding.lines) && given(&finding.case))
    {
        return Err(format!(
            "a {} finding needs its place (file and lines in the diff) and its failing case",
            finding.severity
        ));
    }
    if let Some(file) = finding.file.as_deref().filter(|f| !f.trim().is_empty())
        && !review.files.iter().any(|changed| changed == file)
    {
        return Err(format!(
            "{file} is not a file of the diff: its files are {}",
            review.files.join(", ")
        ));
    }
    let concerns = finding.concerns.trim();
    if concerns != "code" {
        let items: Vec<&str> = spec
            .map(|spec| {
                spec.behaviour
                    .iter()
                    .chain(&spec.decisions)
                    .map(|(id, _)| id.as_str())
                    .collect()
            })
            .unwrap_or_default();
        if !items.contains(&concerns) {
            return Err(match spec {
                Some(_) => format!(
                    "concerns is code, or a C or D of the spec ({}), not {concerns}",
                    items.join(", ")
                ),
                None => {
                    format!("concerns is code: the session develops no spec, so not {concerns}")
                }
            });
        }
    }
    Ok(())
}

/// What changed in the workspace during the review, each repository's HEAD and status compared
/// with what they were before it; none when nothing did.
async fn workspace_changed(review: &Review) -> Option<String> {
    let mut said = Vec::new();
    for (diff, was) in review.diffs.iter().zip(&review.before) {
        let now = snapshot(&was.path, std::path::Path::new(&diff.folder)).await;
        if now.head != was.head {
            said.push(format!("{}: HEAD moved", was.path));
        } else if now.status != was.status {
            said.push(format!("{}: its files changed (git status)", was.path));
        }
    }
    (!said.is_empty()).then(|| said.join("; "))
}

fn delivery(
    review: &Review,
    kept: &[Finding],
    previous: &[Previous],
    summary: Option<&str>,
    changed: Option<&str>,
) -> String {
    let shown: Vec<&Finding> = SEVERITIES
        .iter()
        .flat_map(|severity| {
            kept.iter()
                .filter(move |f| f.severity == *severity && delivered(f))
        })
        .collect();
    let mut text = format!(
        "The review of your change, round {} of {ROUNDS}, is done: {} finding{}.",
        review.round,
        shown.len(),
        if shown.len() == 1 { "" } else { "s" }
    );
    if let Some(summary) = summary.filter(|s| !s.trim().is_empty()) {
        text.push_str(&format!("\n\n{summary}"));
    }
    for finding in &shown {
        text.push_str(&format!("\n\n- {}", finding_line(finding)));
    }
    if !previous.is_empty() {
        text.push_str("\n\nThe findings of the first round:");
        for previous in previous {
            text.push_str(&format!("\n- {}: {}", previous.id, previous.status));
        }
    }
    if let Some(changed) = changed {
        text.push_str(&format!(
            "\n\nThe workspace changed during the review, which modifies nothing: {changed}. \
             Look at it before you go on."
        ));
    }
    text.push_str("\n\nFindings block nothing: fix what you agree with, say why for the others. ");
    if review.round < ROUNDS {
        text.push_str("After your fixes, one more round reads what changed: the review tool, or the user's /review.");
    } else {
        text.push_str("This was the last round: what is left stays open.");
    }
    text
}
