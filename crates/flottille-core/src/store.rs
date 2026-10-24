//! The store: one SQLite file, its schema brought up to date by numbered migrations.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::agent::{Agent, AgentState, Choices};
use crate::protocol::Event;
use crate::run::{Run, RunState};
use crate::workspace::{Action, Failure, Step, StepState, Workspace};
use crate::{Error, Result};

/// Applied in order; the database's `user_version` is the number of those already applied. A
/// migration is never edited once released: a change is a new one.
const MIGRATIONS: &[&str] = &[
    "
    -- AUTOINCREMENT: a number is never given twice, even after the last events are deleted, so a
    -- client resuming with `--after` never misses one.
    CREATE TABLE events (
        seq  INTEGER PRIMARY KEY AUTOINCREMENT,
        at   INTEGER NOT NULL,
        name TEXT NOT NULL,
        data TEXT NOT NULL
    ) STRICT;
",
    "
    -- A workspace and what it was made from: its repositories with their bases and the project's
    -- variables, as JSON, written once. Its steps change as it is prepared.
    CREATE TABLE workspaces (
        id             INTEGER PRIMARY KEY,
        project        TEXT NOT NULL,
        name           TEXT NOT NULL,
        branch         TEXT NOT NULL,
        folder         TEXT NOT NULL,
        project_folder TEXT NOT NULL,
        created_at     INTEGER NOT NULL,
        variables      TEXT NOT NULL,
        repositories   TEXT NOT NULL,
        UNIQUE (project, name)
    ) STRICT;
    CREATE TABLE workspace_steps (
        workspace     INTEGER NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
        position      INTEGER NOT NULL,
        repository    TEXT,
        action        TEXT NOT NULL,
        state         TEXT NOT NULL,
        failed_doing  TEXT,
        failed_output TEXT,
        PRIMARY KEY (workspace, position)
    ) STRICT;
",
    "
    -- An agent session (`docs/agents.md`): where it works, what it was started with (frozen:
    -- its instructions and its tool names), what the agent took, and the ACP session id a
    -- restarted process opens again. The tree (parent, lineage, depth) comes with the deliveries.
    CREATE TABLE agents (
        id            TEXT PRIMARY KEY,
        provider      TEXT NOT NULL,
        role          TEXT NOT NULL,
        project       TEXT NOT NULL,
        workspace     TEXT,
        folder        TEXT NOT NULL,
        state         TEXT NOT NULL,
        state_reason  TEXT,
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL,
        active_at     INTEGER,
        ended_at      INTEGER,
        chosen_model  TEXT,
        chosen_effort TEXT,
        chosen_mode   TEXT,
        taken_model   TEXT,
        taken_effort  TEXT,
        taken_mode    TEXT,
        native_id     TEXT,
        agent_version TEXT,
        instructions  TEXT NOT NULL,
        tools         TEXT NOT NULL
    ) STRICT;
    -- Which events are an agent's conversation: an agent's events are read without scanning
    -- every event.
    CREATE TABLE agent_events (
        seq   INTEGER PRIMARY KEY REFERENCES events (seq),
        agent TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE
    ) STRICT;
    CREATE INDEX agent_events_by_agent ON agent_events (agent, seq);
    -- The adapter processes running, each known by its pid and its start time, so that a daemon
    -- started after a crash kills its own leftovers and never a process that took the pid since.
    CREATE TABLE agent_processes (
        pid     INTEGER PRIMARY KEY,
        started INTEGER NOT NULL,
        agent   TEXT NOT NULL
    ) STRICT;
",
    "
    -- What each agent's session read (`docs/tools.md`): the fingerprint of every file it read or
    -- wrote, so that it never writes over a change it has not seen, across restarts too.
    CREATE TABLE session_files (
        agent  TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        path   TEXT NOT NULL,
        sha256 TEXT NOT NULL,
        PRIMARY KEY (agent, path)
    ) STRICT;
    -- The commands agents run, and the end of their output once they ended.
    CREATE TABLE runs (
        id         TEXT PRIMARY KEY,
        agent      TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
        name       TEXT,
        line       TEXT NOT NULL,
        folder     TEXT NOT NULL,
        state      TEXT NOT NULL,
        exit_code  INTEGER,
        signal     INTEGER,
        started_at INTEGER NOT NULL,
        ended_at   INTEGER,
        output     TEXT NOT NULL
    ) STRICT;
    -- The process group of each run going on, as for the agents' processes: a daemon started
    -- after a crash kills its own leftovers and never a process that took the pid since.
    CREATE TABLE run_processes (
        pid     INTEGER PRIMARY KEY,
        started INTEGER NOT NULL,
        run     TEXT NOT NULL
    ) STRICT;
",
];

const RUN_COLUMNS: &str =
    "id, agent, name, line, folder, state, exit_code, signal, started_at, ended_at, output";

const AGENT_COLUMNS: &str = "id, provider, role, project, workspace, folder, state, state_reason, \
    created_at, updated_at, active_at, ended_at, chosen_model, chosen_effort, chosen_mode, \
    taken_model, taken_effort, taken_mode, native_id, agent_version, instructions, tools";

/// What changes on an agent; what is left `None` stays as it is.
#[derive(Clone, Debug, Default)]
pub struct AgentChange {
    pub state: Option<AgentState>,
    /// Set with the state: `None` there forgets the reason.
    pub state_reason: Option<String>,
    pub native_id: Option<String>,
    pub agent_version: Option<String>,
    pub taken: Option<Choices>,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        Store::prepare(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Store> {
        Store::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut connection: Connection) -> Result<Store> {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut connection)?;
        Ok(Store { connection })
    }

    pub fn schema_version(&self) -> Result<usize> {
        Ok(self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    pub fn append_event(&self, name: &str, data: Value) -> Result<Event> {
        append_event(&self.connection, name, data)
    }

    /// Writes a new workspace with its steps, all pending, and its event, in one transaction.
    /// The id is the store's. Refused when the project has a workspace of that name.
    pub fn create_workspace(
        &mut self,
        workspace: &Workspace,
        event: (&str, Value),
    ) -> Result<(i64, Event)> {
        let transaction = self.connection.transaction()?;
        let inserted = transaction.execute(
            "INSERT INTO workspaces (project, name, branch, folder, project_folder, created_at, \
             variables, repositories) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                workspace.project,
                workspace.name,
                workspace.branch,
                path_text(&workspace.folder)?,
                path_text(&workspace.project_folder)?,
                workspace.created_at,
                serde_json::to_string(&workspace.variables)?,
                serde_json::to_string(&workspace.repositories)?,
            ],
        );
        match inserted {
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(Error::Refused(format!(
                    "project {} already has a workspace named {}",
                    workspace.project, workspace.name
                )));
            }
            inserted => inserted?,
        };
        let id = transaction.last_insert_rowid();
        for step in &workspace.steps {
            transaction.execute(
                "INSERT INTO workspace_steps (workspace, position, repository, action, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    step.position,
                    step.repository,
                    serde_json::to_string(&step.action)?,
                    StepState::Pending.as_str(),
                ],
            )?;
        }
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok((id, event))
    }

    /// The project's workspace of that name.
    pub fn workspace(&self, project: &str, name: &str) -> Result<Option<Workspace>> {
        let id: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM workspaces WHERE project = ?1 AND name = ?2",
                params![project, name],
                |row| row.get(0),
            )
            .optional()?;
        id.map(|id| self.workspace_by_id(id)).transpose()
    }

    /// Every workspace, or the project's, by project then name.
    pub fn workspaces(&self, project: Option<&str>) -> Result<Vec<Workspace>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT id FROM workspaces WHERE ?1 IS NULL OR project = ?1 ORDER BY project, name",
        )?;
        let ids = statement
            .query_map(params![project], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        ids.into_iter().map(|id| self.workspace_by_id(id)).collect()
    }

    pub fn workspace_by_id(&self, id: i64) -> Result<Workspace> {
        let (project, name, branch, folder, project_folder, created_at, variables, repositories) =
            self.connection.query_row(
                "SELECT project, name, branch, folder, project_folder, created_at, variables, \
             repositories FROM workspaces WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, u64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )?;
        let mut statement = self.connection.prepare_cached(
            "SELECT position, repository, action, state, failed_doing, failed_output \
             FROM workspace_steps WHERE workspace = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map(params![id], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut steps = Vec::new();
        for row in rows {
            let (position, repository, action, state, doing, output) = row?;
            steps.push(Step {
                position,
                repository,
                action: serde_json::from_str::<Action>(&action)?,
                state: StepState::parse(&state).ok_or_else(|| {
                    Error::Refused(format!(
                        "the store holds a step in an unknown state {state}"
                    ))
                })?,
                failure: doing.map(|doing| Failure {
                    doing,
                    output: output.unwrap_or_default(),
                }),
            });
        }
        Ok(Workspace {
            id,
            project,
            name,
            branch,
            folder: folder.into(),
            project_folder: project_folder.into(),
            created_at,
            variables: serde_json::from_str(&variables)?,
            repositories: serde_json::from_str(&repositories)?,
            steps,
        })
    }

    /// Sets one step's state, and its failure (forgotten unless given), with its event, in one
    /// transaction.
    pub fn set_step(
        &mut self,
        workspace: i64,
        position: u32,
        state: StepState,
        failure: Option<&Failure>,
        event: (&str, Value),
    ) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE workspace_steps SET state = ?3, failed_doing = ?4, failed_output = ?5 \
             WHERE workspace = ?1 AND position = ?2",
            params![
                workspace,
                position,
                state.as_str(),
                failure.map(|f| &f.doing),
                failure.map(|f| &f.output)
            ],
        )?;
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Puts these steps back to pending, forgetting their failure, with one event, in one
    /// transaction.
    pub fn reset_steps(
        &mut self,
        workspace: i64,
        positions: &[u32],
        event: (&str, Value),
    ) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        for position in positions {
            transaction.execute(
                "UPDATE workspace_steps SET state = 'pending', failed_doing = NULL, \
                 failed_output = NULL WHERE workspace = ?1 AND position = ?2",
                params![workspace, position],
            )?;
        }
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Forgets a workspace and its steps, with its event, in one transaction.
    pub fn remove_workspace(&mut self, workspace: i64, event: (&str, Value)) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM workspaces WHERE id = ?1", params![workspace])?;
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Writes a new agent with its event, in one transaction.
    pub fn create_agent(&mut self, agent: &Agent, event: (&str, Value)) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            &format!(
                "INSERT INTO agents ({AGENT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                 ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)"
            ),
            params![
                agent.id,
                agent.provider,
                agent.role,
                agent.project,
                agent.workspace,
                path_text(&agent.folder)?,
                agent.state.as_str(),
                agent.state_reason,
                agent.created_at,
                agent.updated_at,
                agent.active_at,
                agent.ended_at,
                agent.chosen.model,
                agent.chosen.effort,
                agent.chosen.mode,
                agent.taken.model,
                agent.taken.effort,
                agent.taken.mode,
                agent.native_id,
                agent.agent_version,
                agent.instructions,
                serde_json::to_string(&agent.tools)?,
            ],
        )?;
        let event = append_agent_event(&transaction, &agent.id, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    pub fn agent(&self, id: &str) -> Result<Option<Agent>> {
        let mut statement = self
            .connection
            .prepare_cached(&format!("SELECT {AGENT_COLUMNS} FROM agents WHERE id = ?1"))?;
        let agent = statement
            .query_row(params![id], agent_row)
            .optional()?
            .transpose()?;
        Ok(agent)
    }

    /// Every agent, the oldest first.
    pub fn agents(&self) -> Result<Vec<Agent>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {AGENT_COLUMNS} FROM agents ORDER BY created_at, id"
        ))?;
        let rows = statement.query_map([], agent_row)?;
        rows.map(|row| row?).collect()
    }

    /// Changes an agent, with its event when one is given, in one transaction; the agent as it
    /// is now. An agent in a final state keeps it.
    pub fn change_agent(
        &mut self,
        id: &str,
        change: AgentChange,
        event: Option<(&str, Value)>,
    ) -> Result<(Agent, Option<Event>)> {
        let transaction = self.connection.transaction()?;
        let now = now_ms();
        if let Some(state) = change.state {
            transaction.execute(
                "UPDATE agents SET state = ?2, state_reason = ?3, \
                 ended_at = CASE WHEN ?4 THEN ?5 ELSE ended_at END \
                 WHERE id = ?1 AND state NOT IN ('ended', 'failed')",
                params![
                    id,
                    state.as_str(),
                    change.state_reason,
                    state.is_final(),
                    now
                ],
            )?;
        }
        if let Some(native_id) = change.native_id {
            transaction.execute(
                "UPDATE agents SET native_id = ?2 WHERE id = ?1",
                params![id, native_id],
            )?;
        }
        if let Some(version) = change.agent_version {
            transaction.execute(
                "UPDATE agents SET agent_version = ?2 WHERE id = ?1",
                params![id, version],
            )?;
        }
        if let Some(taken) = change.taken {
            transaction.execute(
                "UPDATE agents SET taken_model = ?2, taken_effort = ?3, taken_mode = ?4 \
                 WHERE id = ?1",
                params![id, taken.model, taken.effort, taken.mode],
            )?;
        }
        transaction.execute(
            "UPDATE agents SET updated_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        let event = event
            .map(|(name, data)| append_agent_event(&transaction, id, name, data))
            .transpose()?;
        transaction.commit()?;
        let agent = self
            .agent(id)?
            .ok_or_else(|| Error::Refused(format!("there is no agent {id}")))?;
        Ok((agent, event))
    }

    /// An event of the agent's conversation; `activity` says it is a sign of activity, which
    /// moves the agent's `active_at`.
    pub fn append_agent_event(
        &mut self,
        agent: &str,
        name: &str,
        data: Value,
        activity: bool,
    ) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        let event = append_agent_event(&transaction, agent, name, data)?;
        if activity {
            transaction.execute(
                "UPDATE agents SET active_at = ?2 WHERE id = ?1",
                params![agent, event.at],
            )?;
        }
        transaction.commit()?;
        Ok(event)
    }

    /// At most `limit` events of the agent numbered after `after`, oldest first.
    pub fn agent_events(&self, agent: &str, after: u64, limit: usize) -> Result<Vec<Event>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT e.seq, e.at, e.name, e.data FROM agent_events a JOIN events e ON e.seq = a.seq \
             WHERE a.agent = ?1 AND a.seq > ?2 ORDER BY a.seq LIMIT ?3",
        )?;
        let rows = statement.query_map(params![agent, after, limit as u64], event_row)?;
        rows.map(|row| row?).collect()
    }

    /// An adapter process started for the agent: its pid and its start time.
    pub fn add_process(&self, pid: u32, started: u64, agent: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO agent_processes (pid, started, agent) VALUES (?1, ?2, ?3)",
            params![pid, started, agent],
        )?;
        Ok(())
    }

    pub fn remove_process(&self, pid: u32) -> Result<()> {
        self.connection
            .execute("DELETE FROM agent_processes WHERE pid = ?1", params![pid])?;
        Ok(())
    }

    /// Every process recorded, each `(pid, start time)`.
    pub fn processes(&self) -> Result<Vec<(u32, u64)>> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT pid, started FROM agent_processes ORDER BY pid")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The fingerprint of the file as the agent last read or wrote it.
    pub fn file_read(&self, agent: &str, path: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT sha256 FROM session_files WHERE agent = ?1 AND path = ?2",
                params![agent, path],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_file_read(&self, agent: &str, path: &str, sha256: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO session_files (agent, path, sha256) VALUES (?1, ?2, ?3)",
            params![agent, path, sha256],
        )?;
        Ok(())
    }

    /// Writes a new run with its event, in one transaction.
    pub fn create_run(&mut self, run: &Run, event: (&str, Value)) -> Result<Event> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            &format!(
                "INSERT INTO runs ({RUN_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, \
                 ?10, ?11)"
            ),
            params![
                run.id,
                run.agent,
                run.name,
                run.line,
                path_text(&run.folder)?,
                run.state.as_str(),
                run.exit_code,
                run.signal,
                run.started_at,
                run.ended_at,
                run.output,
            ],
        )?;
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Ends a running run, with its output and its event, in one transaction; `None` when it had
    /// already ended.
    pub fn end_run(
        &mut self,
        id: &str,
        state: RunState,
        exit_code: Option<i32>,
        signal: Option<i32>,
        output: &str,
        event: (&str, Value),
    ) -> Result<Option<Event>> {
        let transaction = self.connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE runs SET state = ?2, exit_code = ?3, signal = ?4, output = ?5, ended_at = ?6 \
             WHERE id = ?1 AND state = 'running'",
            params![id, state.as_str(), exit_code, signal, output, now_ms()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let event = append_event(&transaction, event.0, event.1)?;
        transaction.commit()?;
        Ok(Some(event))
    }

    pub fn run(&self, id: &str) -> Result<Option<Run>> {
        let mut statement = self
            .connection
            .prepare_cached(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1"))?;
        let run = statement
            .query_row(params![id], run_row)
            .optional()?
            .transpose()?;
        Ok(run)
    }

    /// The runs still marked running, the oldest first.
    pub fn running_runs(&self) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {RUN_COLUMNS} FROM runs WHERE state = 'running' ORDER BY started_at, id"
        ))?;
        let rows = statement.query_map([], run_row)?;
        rows.map(|row| row?).collect()
    }

    /// A run's process group started: its leader's pid and start time.
    pub fn add_run_process(&self, pid: u32, started: u64, run: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO run_processes (pid, started, run) VALUES (?1, ?2, ?3)",
            params![pid, started, run],
        )?;
        Ok(())
    }

    pub fn remove_run_process(&self, pid: u32) -> Result<()> {
        self.connection
            .execute("DELETE FROM run_processes WHERE pid = ?1", params![pid])?;
        Ok(())
    }

    /// Every run process recorded, each `(pid, start time)`.
    pub fn run_processes(&self) -> Result<Vec<(u32, u64)>> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT pid, started FROM run_processes ORDER BY pid")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// At most `limit` events numbered after `after`, oldest first.
    pub fn events_after(&self, after: u64, limit: usize) -> Result<Vec<Event>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT seq, at, name, data FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = statement.query_map(params![after, limit as u64], event_row)?;
        rows.map(|row| row?).collect()
    }

    /// The number of the last event, 0 when there is none.
    pub fn last_seq(&self) -> Result<u64> {
        let seq: Option<u64> = self
            .connection
            .query_row("SELECT max(seq) FROM events", [], |row| row.get(0))
            .optional()?
            .flatten();
        Ok(seq.unwrap_or(0))
    }
}

fn append_event(connection: &Connection, name: &str, data: Value) -> Result<Event> {
    let at = now_ms();
    let text = serde_json::to_string(&data)?;
    let seq: u64 = connection.query_row(
        "INSERT INTO events (at, name, data) VALUES (?1, ?2, ?3) RETURNING seq",
        params![at, name, text],
        |row| row.get(0),
    )?;
    Ok(Event {
        seq,
        at,
        name: name.to_owned(),
        data,
    })
}

fn append_agent_event(
    connection: &Connection,
    agent: &str,
    name: &str,
    data: Value,
) -> Result<Event> {
    let event = append_event(connection, name, data)?;
    connection.execute(
        "INSERT INTO agent_events (seq, agent) VALUES (?1, ?2)",
        params![event.seq, agent],
    )?;
    Ok(event)
}

fn event_row(row: &rusqlite::Row) -> rusqlite::Result<Result<Event>> {
    let (seq, at, name, data): (u64, u64, String, String) =
        (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
    Ok(serde_json::from_str(&data)
        .map(|data| Event {
            seq,
            at,
            name,
            data,
        })
        .map_err(Error::from))
}

fn run_row(row: &rusqlite::Row) -> rusqlite::Result<Result<Run>> {
    let state: String = row.get(5)?;
    let folder: String = row.get(4)?;
    let Some(state) = RunState::parse(&state) else {
        return Ok(Err(Error::Refused(format!(
            "the store holds a run in an unknown state {state}"
        ))));
    };
    Ok(Ok(Run {
        id: row.get(0)?,
        agent: row.get(1)?,
        name: row.get(2)?,
        line: row.get(3)?,
        folder: folder.into(),
        state,
        exit_code: row.get(6)?,
        signal: row.get(7)?,
        started_at: row.get(8)?,
        ended_at: row.get(9)?,
        output: row.get(10)?,
    }))
}

fn agent_row(row: &rusqlite::Row) -> rusqlite::Result<Result<Agent>> {
    let text = |i| row.get::<_, Option<String>>(i);
    let state: String = row.get(6)?;
    let folder: String = row.get(5)?;
    let tools: String = row.get(21)?;
    let Some(state) = AgentState::parse(&state) else {
        return Ok(Err(Error::Refused(format!(
            "the store holds an agent in an unknown state {state}"
        ))));
    };
    let tools = match serde_json::from_str(&tools) {
        Ok(tools) => tools,
        Err(error) => return Ok(Err(error.into())),
    };
    Ok(Ok(Agent {
        id: row.get(0)?,
        provider: row.get(1)?,
        role: row.get(2)?,
        project: row.get(3)?,
        workspace: text(4)?,
        folder: folder.into(),
        state,
        state_reason: text(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        active_at: row.get(10)?,
        ended_at: row.get(11)?,
        chosen: Choices {
            model: text(12)?,
            effort: text(13)?,
            mode: text(14)?,
        },
        taken: Choices {
            model: text(15)?,
            effort: text(16)?,
            mode: text(17)?,
        },
        native_id: text(18)?,
        agent_version: text(19)?,
        instructions: row.get(20)?,
        tools,
    }))
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        Error::Refused(format!(
            "{} is not valid UTF-8: Flottille cannot keep it",
            path.display()
        ))
    })
}

fn migrate(connection: &mut Connection) -> Result<()> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let applied: usize = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if applied > MIGRATIONS.len() {
        return Err(Error::Refused(format!(
            "the store is at schema {applied}, made by a newer Flottille than this one (schema {})",
            MIGRATIONS.len()
        )));
    }
    for migration in &MIGRATIONS[applied..] {
        transaction.execute_batch(migration)?;
    }
    transaction.pragma_update(None, "user_version", MIGRATIONS.len())?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Base, Worktree};
    use serde_json::json;

    #[test]
    fn a_new_store_is_brought_to_the_last_schema() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn reopening_a_store_applies_nothing_twice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        Store::open(&path)
            .unwrap()
            .append_event("test.one", json!({}))
            .unwrap();

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.last_seq().unwrap(), 1);
    }

    #[test]
    fn a_store_of_the_first_schema_keeps_its_events_and_gains_the_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        old.execute_batch(MIGRATIONS[0]).unwrap();
        old.pragma_update(None, "user_version", 1).unwrap();
        append_event(&old, "test.one", json!({})).unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.last_seq().unwrap(), 1);
        assert!(store.workspaces(None).unwrap().is_empty());
    }

    #[test]
    fn a_store_from_a_newer_flottille_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", MIGRATIONS.len() + 1)
            .unwrap();

        let error = Store::open(&path).err().unwrap();

        assert!(error.to_string().contains("newer Flottille"), "{error}");
    }

    fn a_workspace() -> Workspace {
        Workspace {
            id: 0,
            project: "app".into(),
            name: "login".into(),
            branch: "app/login".into(),
            folder: "/data/workspaces/app/login".into(),
            project_folder: "/work/app".into(),
            created_at: 1_792_843_730_808,
            variables: [("STORE".to_owned(), "{workspace.path}/.store".to_owned())].into(),
            repositories: vec![Worktree {
                path: ".".into(),
                base: Base {
                    branch: "main".into(),
                    remote: "origin".into(),
                    reference: "refs/remotes/origin/main".into(),
                    commit: "0123abcd".into(),
                    fetched_now: true,
                    fetched_at: Some(1_792_843_730_000),
                },
            }],
            steps: vec![
                Step {
                    position: 0,
                    repository: Some(".".into()),
                    action: Action::Worktree,
                    state: StepState::Pending,
                    failure: None,
                },
                Step {
                    position: 1,
                    repository: None,
                    action: Action::Run {
                        line: "make".into(),
                        command: None,
                        time_limit: 1800,
                    },
                    state: StepState::Pending,
                    failure: None,
                },
            ],
        }
    }

    #[test]
    fn a_workspace_is_written_with_its_event_and_read_back() {
        let mut store = Store::open_in_memory().unwrap();
        let workspace = a_workspace();

        let (id, event) = store
            .create_workspace(&workspace, ("workspace.created", json!({"name": "login"})))
            .unwrap();

        assert_eq!(event.name, "workspace.created");
        let read = store.workspace("app", "login").unwrap().unwrap();
        assert_eq!(read, Workspace { id, ..workspace });
        assert_eq!(
            store.workspaces(Some("app")).unwrap(),
            std::slice::from_ref(&read)
        );
        assert_eq!(store.workspaces(None).unwrap(), [read]);
        assert!(store.workspaces(Some("other")).unwrap().is_empty());
        assert_eq!(store.workspace("app", "other").unwrap(), None);
    }

    #[test]
    fn a_second_workspace_of_the_same_name_is_refused() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap();

        let error = store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "project app already has a workspace named login"
        );
        assert_eq!(store.last_seq().unwrap(), 1);
    }

    #[test]
    fn a_step_changes_state_with_its_event_and_goes_back_to_pending() {
        let mut store = Store::open_in_memory().unwrap();
        let (id, _) = store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap();
        let failure = Failure {
            doing: "make (in .)".into(),
            output: "make: *** no rule".into(),
        };

        store
            .set_step(
                id,
                0,
                StepState::Done,
                None,
                ("workspace.step_done", json!({})),
            )
            .unwrap();
        store
            .set_step(
                id,
                1,
                StepState::Failed,
                Some(&failure),
                ("workspace.step_failed", json!({})),
            )
            .unwrap();
        let failed = store.workspace_by_id(id).unwrap();
        store
            .reset_steps(id, &[1], ("workspace.resumed", json!({})))
            .unwrap();
        let resumed = store.workspace_by_id(id).unwrap();

        assert_eq!(failed.state(), crate::workspace::State::Failed);
        assert_eq!(failed.steps[0].state, StepState::Done);
        assert_eq!(failed.steps[1].failure, Some(failure));
        assert_eq!(resumed.steps[1].state, StepState::Pending);
        assert_eq!(resumed.steps[1].failure, None);
        assert_eq!(resumed.state(), crate::workspace::State::Preparing);
        let names: Vec<_> = store
            .events_after(1, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            [
                "workspace.step_done",
                "workspace.step_failed",
                "workspace.resumed"
            ]
        );
    }

    #[test]
    fn a_removed_workspace_takes_its_steps_with_it() {
        let mut store = Store::open_in_memory().unwrap();
        let (id, _) = store
            .create_workspace(&a_workspace(), ("workspace.created", json!({})))
            .unwrap();

        store
            .remove_workspace(id, ("workspace.removed", json!({})))
            .unwrap();

        assert_eq!(store.workspace("app", "login").unwrap(), None);
        let steps: i64 = store
            .connection
            .query_row("SELECT count(*) FROM workspace_steps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(steps, 0);
    }

    fn an_agent() -> Agent {
        Agent {
            id: "0123456789ab".into(),
            provider: "claude".into(),
            role: "lead".into(),
            project: "app".into(),
            workspace: Some("login".into()),
            folder: "/data/workspaces/app/login".into(),
            state: AgentState::Starting,
            state_reason: None,
            created_at: 1_792_843_730_808,
            updated_at: 1_792_843_730_808,
            active_at: None,
            ended_at: None,
            chosen: Choices {
                model: Some("haiku".into()),
                ..Choices::default()
            },
            taken: Choices::default(),
            native_id: None,
            agent_version: None,
            instructions: "shared\n\n__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__\n\nsession".into(),
            tools: vec!["mcp__flottille__fs_read".into()],
        }
    }

    #[test]
    fn an_agent_is_written_with_its_event_and_read_back() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();

        let event = store
            .create_agent(&agent, ("agent.created", json!({"agent": agent.id})))
            .unwrap();

        assert_eq!(store.agent(&agent.id).unwrap(), Some(agent.clone()));
        assert_eq!(store.agents().unwrap(), std::slice::from_ref(&agent));
        assert_eq!(store.agent("other").unwrap(), None);
        assert_eq!(store.agent_events(&agent.id, 0, 10).unwrap(), [event]);
    }

    #[test]
    fn an_agent_changes_and_a_final_state_is_never_left() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();
        let taken = Choices {
            model: Some("haiku-4".into()),
            ..Choices::default()
        };

        let (opened, _) = store
            .change_agent(
                &agent.id,
                AgentChange {
                    state: Some(AgentState::Idle),
                    native_id: Some("session-1".into()),
                    agent_version: Some("fake 1.0".into()),
                    taken: Some(taken.clone()),
                    ..AgentChange::default()
                },
                None,
            )
            .unwrap();
        let (failed, event) = store
            .change_agent(
                &agent.id,
                AgentChange {
                    state: Some(AgentState::Failed),
                    state_reason: Some("its process died".into()),
                    ..AgentChange::default()
                },
                Some(("agent.died", json!({}))),
            )
            .unwrap();
        let (still, _) = store
            .change_agent(
                &agent.id,
                AgentChange {
                    state: Some(AgentState::Idle),
                    ..AgentChange::default()
                },
                None,
            )
            .unwrap();

        assert_eq!(opened.state, AgentState::Idle);
        assert_eq!(opened.native_id.as_deref(), Some("session-1"));
        assert_eq!(opened.taken, taken);
        assert_eq!(failed.state, AgentState::Failed);
        assert!(failed.ended_at.is_some());
        assert_eq!(event.unwrap().name, "agent.died");
        assert_eq!(still.state, AgentState::Failed);
        assert_eq!(still.state_reason.as_deref(), Some("its process died"));
    }

    #[test]
    fn only_an_event_of_activity_moves_the_agent_active_time() {
        let mut store = Store::open_in_memory().unwrap();
        let agent = an_agent();
        store
            .create_agent(&agent, ("agent.created", json!({})))
            .unwrap();

        let replayed = store
            .append_agent_event(&agent.id, "agent.message", json!({"replay": true}), false)
            .unwrap();
        let before = store.agent(&agent.id).unwrap().unwrap().active_at;
        let live = store
            .append_agent_event(&agent.id, "agent.message", json!({"replay": false}), true)
            .unwrap();
        store.append_event("daemon.started", json!({})).unwrap();

        assert_eq!(before, None);
        assert_eq!(
            store.agent(&agent.id).unwrap().unwrap().active_at,
            Some(live.at)
        );
        let names: Vec<u64> = store
            .agent_events(&agent.id, replayed.seq - 1, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(names, [replayed.seq, live.seq]);
    }

    #[test]
    fn processes_are_recorded_with_their_start_time_and_forgotten() {
        let store = Store::open_in_memory().unwrap();
        store.add_process(4242, 77, "a").unwrap();
        store.add_process(4343, 78, "b").unwrap();
        store.remove_process(4242).unwrap();
        assert_eq!(store.processes().unwrap(), [(4343, 78)]);
    }

    #[test]
    fn a_store_of_the_workspaces_schema_keeps_them_and_gains_the_agents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        old.execute_batch(MIGRATIONS[0]).unwrap();
        old.execute_batch(MIGRATIONS[1]).unwrap();
        old.pragma_update(None, "user_version", 2).unwrap();
        old.execute_batch(
            "INSERT INTO workspaces (project, name, branch, folder, project_folder, created_at, \
             variables, repositories) VALUES ('app', 'login', 'app/login', '/w', '/p', 1, '{}', '[]')",
        )
        .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.workspaces(None).unwrap().len(), 1);
        assert!(store.agents().unwrap().is_empty());
    }

    #[test]
    fn a_store_of_the_agents_schema_keeps_them_and_gains_the_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flottille.db");
        let old = Connection::open(&path).unwrap();
        for migration in &MIGRATIONS[..3] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", 3).unwrap();
        drop(old);
        let mut old = Store {
            connection: Connection::open(&path).unwrap(),
        };
        old.create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        drop(old);

        let store = Store::open(&path).unwrap();

        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(store.agents().unwrap().len(), 1);
        assert!(store.running_runs().unwrap().is_empty());
    }

    #[test]
    fn a_file_read_keeps_its_fingerprint_per_agent() {
        let mut store = Store::open_in_memory().unwrap();
        for id in ["a", "b"] {
            let mut agent = an_agent();
            agent.id = id.into();
            store
                .create_agent(&agent, ("agent.created", json!({})))
                .unwrap();
        }
        store.set_file_read("a", "/w/f", "one").unwrap();
        store.set_file_read("a", "/w/f", "two").unwrap();

        assert_eq!(
            store.file_read("a", "/w/f").unwrap().as_deref(),
            Some("two")
        );
        assert_eq!(store.file_read("b", "/w/f").unwrap(), None);
    }

    #[test]
    fn a_run_is_written_ended_once_and_read_back() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .create_agent(&an_agent(), ("agent.created", json!({})))
            .unwrap();
        let run = Run {
            id: "r1".into(),
            agent: an_agent().id,
            name: Some("test".into()),
            line: "cargo test".into(),
            folder: "/w".into(),
            state: RunState::Running,
            exit_code: None,
            signal: None,
            started_at: 5,
            ended_at: None,
            output: String::new(),
        };
        store.create_run(&run, ("run.started", json!({}))).unwrap();
        assert_eq!(store.run("r1").unwrap().unwrap(), run);
        assert_eq!(store.running_runs().unwrap(), std::slice::from_ref(&run));

        let ended = store
            .end_run(
                "r1",
                RunState::Exited,
                Some(0),
                None,
                "ok\n",
                ("run.ended", json!({})),
            )
            .unwrap();
        let again = store
            .end_run(
                "r1",
                RunState::Stopped,
                None,
                None,
                "",
                ("run.ended", json!({})),
            )
            .unwrap();

        assert!(ended.is_some() && again.is_none());
        let read = store.run("r1").unwrap().unwrap();
        assert_eq!((read.state, read.exit_code), (RunState::Exited, Some(0)));
        assert_eq!(read.output, "ok\n");
        assert!(read.ended_at.is_some());
        assert!(store.running_runs().unwrap().is_empty());
        store.add_run_process(4242, 7, "r1").unwrap();
        assert_eq!(store.run_processes().unwrap(), [(4242, 7)]);
        store.remove_run_process(4242).unwrap();
        assert!(store.run_processes().unwrap().is_empty());
    }

    #[test]
    fn events_are_numbered_in_order_and_read_back_after_a_number() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.last_seq().unwrap(), 0);
        let first = store.append_event("test.one", json!({"n": 1})).unwrap();
        let second = store.append_event("test.two", json!({"n": 2})).unwrap();
        let third = store.append_event("test.three", json!({"n": 3})).unwrap();

        assert!(first.seq < second.seq && second.seq < third.seq);
        assert_eq!(
            store.events_after(first.seq, 10).unwrap(),
            vec![second.clone(), third]
        );
        assert_eq!(store.events_after(0, 1).unwrap(), vec![first]);
        assert_eq!(store.last_seq().unwrap(), 3);
    }
}
