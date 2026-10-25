//! The store: one SQLite file, its schema brought up to date by numbered migrations.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::protocol::Event;
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
];

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

    /// At most `limit` events numbered after `after`, oldest first.
    pub fn events_after(&self, after: u64, limit: usize) -> Result<Vec<Event>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT seq, at, name, data FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = statement.query_map(params![after, limit as u64], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (seq, at, name, data) = row?;
            Ok(Event {
                seq,
                at,
                name,
                data: serde_json::from_str(&data)?,
            })
        })
        .collect()
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
