//! The routines and their runs in the store (`docs/design.md`, "Routines").

use rusqlite::{OptionalExtension, params};

use super::Store;
use crate::routine::{Routine, Run};
use crate::{Error, Result};

const ROUTINE: &str = "id, name, project, prompt, provider, model, effort, schedule, zone, mode, \
    rings, paused, missed_hours, next_at, delayed, created_at, updated_at";
const RUN: &str = "id, routine, due_at, started_at, ended_at, session, mode, outcome, summary, why";

fn routine_row(row: &rusqlite::Row) -> rusqlite::Result<Routine> {
    Ok(Routine {
        id: row.get(0)?,
        name: row.get(1)?,
        project: row.get(2)?,
        prompt: row.get(3)?,
        provider: row.get(4)?,
        model: row.get(5)?,
        effort: row.get(6)?,
        schedule: row.get(7)?,
        zone: row.get(8)?,
        mode: row.get(9)?,
        rings: row.get(10)?,
        paused: row.get(11)?,
        missed_hours: row.get(12)?,
        next_at: row.get(13)?,
        delayed: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}

fn run_row(row: &rusqlite::Row) -> rusqlite::Result<Run> {
    Ok(Run {
        id: row.get(0)?,
        routine: row.get(1)?,
        due_at: row.get(2)?,
        started_at: row.get(3)?,
        ended_at: row.get(4)?,
        session: row.get(5)?,
        mode: row.get(6)?,
        outcome: row.get(7)?,
        summary: row.get(8)?,
        why: row.get(9)?,
    })
}

/// What a new run is.
pub struct NewRun<'a> {
    pub routine: &'a str,
    pub due_at: u64,
    pub started_at: u64,
    pub mode: &'a str,
    pub outcome: &'a str,
    pub why: Option<&'a str>,
}

impl Store {
    /// Keeps a new routine; refused when its name is taken.
    pub fn add_routine(&self, routine: &Routine) -> Result<()> {
        if self.routine(&routine.name)?.is_some() {
            return Err(Error::Refused(format!(
                "there is a routine named {} already",
                routine.name
            )));
        }
        self.write_routine(routine, true)
    }

    /// Writes the routine as it is now; refused when another one has its name.
    pub fn update_routine(&self, routine: &Routine) -> Result<()> {
        if let Some(other) = self.routine(&routine.name)?
            && other.id != routine.id
        {
            return Err(Error::Refused(format!(
                "there is a routine named {} already",
                routine.name
            )));
        }
        self.write_routine(routine, false)
    }

    fn write_routine(&self, r: &Routine, new: bool) -> Result<()> {
        let sql = match new {
            true => format!(
                "INSERT INTO routines ({ROUTINE}) VALUES \
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"
            ),
            false => "UPDATE routines SET name = ?2, project = ?3, prompt = ?4, provider = ?5, \
                 model = ?6, effort = ?7, schedule = ?8, zone = ?9, mode = ?10, rings = ?11, \
                 paused = ?12, missed_hours = ?13, next_at = ?14, delayed = ?15, \
                 created_at = ?16, updated_at = ?17 WHERE id = ?1"
                .to_owned(),
        };
        self.connection.execute(
            &sql,
            params![
                r.id,
                r.name,
                r.project,
                r.prompt,
                r.provider,
                r.model,
                r.effort,
                r.schedule,
                r.zone,
                r.mode,
                r.rings,
                r.paused,
                r.missed_hours,
                r.next_at,
                r.delayed,
                r.created_at,
                r.updated_at
            ],
        )?;
        Ok(())
    }

    /// The routine of this id or name.
    pub fn routine(&self, key: &str) -> Result<Option<Routine>> {
        Ok(self
            .connection
            .query_row(
                &format!("SELECT {ROUTINE} FROM routines WHERE id = ?1 OR name = ?1"),
                params![key],
                routine_row,
            )
            .optional()?)
    }

    /// The routines, of one project or all, by name.
    pub fn routines(&self, project: Option<&str>) -> Result<Vec<Routine>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {ROUTINE} FROM routines WHERE ?1 IS NULL OR project = ?1 ORDER BY name"
        ))?;
        let rows = statement.query_map(params![project], routine_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The earliest run due of every routine not paused: what the daemon waits for.
    pub fn next_routine_due(&self) -> Result<Option<u64>> {
        Ok(self.connection.query_row(
            "SELECT min(next_at) FROM routines WHERE paused = 0",
            [],
            |row| row.get(0),
        )?)
    }

    /// Takes the run due at `due` and sets the next: false when another took it first.
    pub fn take_routine_due(&self, id: &str, due: u64, next: Option<u64>) -> Result<bool> {
        Ok(self.connection.execute(
            "UPDATE routines SET next_at = ?3, delayed = NULL WHERE id = ?1 AND next_at = ?2",
            params![id, due, next],
        )? == 1)
    }

    /// Moves the run due at `due` to `until`, saying why: false when it was taken meanwhile.
    pub fn delay_routine(&self, id: &str, due: u64, until: u64, why: &str) -> Result<bool> {
        Ok(self.connection.execute(
            "UPDATE routines SET next_at = ?3, delayed = ?4 WHERE id = ?1 AND next_at = ?2",
            params![id, due, until, why],
        )? == 1)
    }

    /// Forgets a routine and its runs; the sessions of its runs stay.
    pub fn remove_routine(&self, id: &str) -> Result<bool> {
        Ok(self
            .connection
            .execute("DELETE FROM routines WHERE id = ?1", params![id])?
            == 1)
    }

    pub fn add_routine_run(&self, new: &NewRun) -> Result<Run> {
        Ok(self.connection.query_row(
            &format!(
                "INSERT INTO routine_runs (routine, due_at, started_at, mode, outcome, why) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING {RUN}"
            ),
            params![
                new.routine,
                new.due_at,
                new.started_at,
                new.mode,
                new.outcome,
                new.why
            ],
            run_row,
        )?)
    }

    /// The run's session, once it started.
    pub fn set_routine_run_session(&self, run: u64, session: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE routine_runs SET session = ?2 WHERE id = ?1",
            params![run, session],
        )?;
        Ok(())
    }

    /// Ends a run under way with its outcome: `None` when it was not under way any more.
    pub fn end_routine_run(
        &self,
        run: u64,
        outcome: &str,
        summary: Option<&str>,
        why: Option<&str>,
        at: u64,
    ) -> Result<Option<Run>> {
        Ok(self
            .connection
            .query_row(
                &format!(
                    "UPDATE routine_runs SET outcome = ?2, summary = ?3, why = ?4, ended_at = ?5 \
                     WHERE id = ?1 AND outcome = 'running' RETURNING {RUN}"
                ),
                params![run, outcome, summary, why, at],
                run_row,
            )
            .optional()?)
    }

    /// The routine's runs, the last first, at most `limit`.
    pub fn routine_runs(&self, routine: &str, limit: usize) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {RUN} FROM routine_runs WHERE routine = ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = statement.query_map(params![routine, limit as i64], run_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The run whose session this is, if it is one.
    pub fn routine_run_of(&self, session: &str) -> Result<Option<Run>> {
        Ok(self
            .connection
            .query_row(
                &format!(
                    "SELECT {RUN} FROM routine_runs WHERE session = ?1 ORDER BY id DESC LIMIT 1"
                ),
                params![session],
                run_row,
            )
            .optional()?)
    }

    /// The routine's runs that may still be under way, the last first: those running, and those
    /// whose work goes on (as long as their session is open, which the caller reads).
    pub fn routine_runs_under_way(&self, routine: &str) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {RUN} FROM routine_runs WHERE routine = ?1 \
             AND outcome IN ('running', 'goes_on') ORDER BY id DESC"
        ))?;
        let rows = statement.query_map(params![routine], run_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every run under way.
    pub fn running_routine_runs(&self) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT {RUN} FROM routine_runs WHERE outcome = 'running' ORDER BY id"
        ))?;
        let rows = statement.query_map([], run_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The session may write nothing from now on: a read-only run's. Never cleared.
    pub fn set_read_only(&self, agent: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE agents SET read_only = 1 WHERE id = ?1",
            params![agent],
        )?;
        Ok(())
    }

    /// Whether the session may write nothing; false for one Flottille has no row of.
    pub fn read_only(&self, agent: &str) -> Result<bool> {
        Ok(self
            .connection
            .query_row(
                "SELECT read_only FROM agents WHERE id = ?1",
                params![agent],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }
}
