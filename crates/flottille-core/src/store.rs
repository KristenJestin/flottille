//! The store: one SQLite file, its schema brought up to date by numbered migrations.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::protocol::Event;
use crate::{Error, Result};

/// Applied in order; the database's `user_version` is the number of those already applied. A
/// migration is never edited once released: a change is a new one.
const MIGRATIONS: &[&str] = &["
    -- AUTOINCREMENT: a number is never given twice, even after the last events are deleted, so a
    -- client resuming with `--after` never misses one.
    CREATE TABLE events (
        seq  INTEGER PRIMARY KEY AUTOINCREMENT,
        at   INTEGER NOT NULL,
        name TEXT NOT NULL,
        data TEXT NOT NULL
    ) STRICT;
"];

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
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let text = serde_json::to_string(&data)?;
        let seq: u64 = self.connection.query_row(
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
