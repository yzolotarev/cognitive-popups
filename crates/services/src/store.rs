//! Versioned local objects with optimistic concurrency and an atomic audit trail.
//! Credential material is never accepted by this API.
use crate::Error;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};

pub struct Store {
    pub(crate) connection: Connection,
}

#[derive(Debug, Clone)]
pub struct Record<T> {
    pub id: String,
    pub revision: i64,
    pub value: T,
}

impl Store {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 1 {
            return Err(Error::Invalid(
                "application database is newer than this build",
            ));
        }
        connection.execute_batch("BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS objects (kind TEXT NOT NULL, id TEXT NOT NULL, revision INTEGER NOT NULL, json TEXT NOT NULL, PRIMARY KEY(kind,id));
            CREATE TABLE IF NOT EXISTS changes (sequence INTEGER PRIMARY KEY, kind TEXT NOT NULL, id TEXT NOT NULL, revision INTEGER NOT NULL, at INTEGER NOT NULL, json TEXT NOT NULL);
            PRAGMA user_version=1; COMMIT;")?;
        Ok(Self { connection })
    }

    pub fn get<T: DeserializeOwned>(
        &self,
        kind: &str,
        id: &str,
    ) -> Result<Option<Record<T>>, Error> {
        let row: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT revision,json FROM objects WHERE kind=?1 AND id=?2",
                params![kind, id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(revision, json)| {
            Ok(Record {
                id: id.into(),
                revision,
                value: serde_json::from_str(&json)?,
            })
        })
        .transpose()
    }

    pub fn list<T: DeserializeOwned>(&self, kind: &str) -> Result<Vec<Record<T>>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT id,revision,json FROM objects WHERE kind=?1 ORDER BY rowid DESC")?;
        let rows = statement
            .query_map([kind], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, revision, json)| {
                Ok(Record {
                    id,
                    revision,
                    value: serde_json::from_str(&json)?,
                })
            })
            .collect()
    }

    /// Expected revision zero creates an object; all later writes must match.
    pub fn save<T: Serialize>(
        &mut self,
        kind: &str,
        id: &str,
        expected: i64,
        value: &T,
        now: i64,
    ) -> Result<i64, Error> {
        let json = serde_json::to_string(value)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT revision FROM objects WHERE kind=?1 AND id=?2",
                params![kind, id],
                |r| r.get(0),
            )
            .optional()?;
        if current.unwrap_or(0) != expected {
            return Err(Error::Conflict);
        }
        let next = expected
            .checked_add(1)
            .ok_or(Error::Invalid("revision overflow"))?;
        tx.execute("INSERT INTO objects(kind,id,revision,json) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO UPDATE SET revision=excluded.revision,json=excluded.json", params![kind,id,next,json])?;
        tx.execute(
            "INSERT INTO changes(kind,id,revision,at,json) VALUES(?1,?2,?3,?4,?5)",
            params![kind, id, next, now, json],
        )?;
        tx.commit()?;
        Ok(next)
    }

    /// Diagnostics report counts only. Raw text is not a diagnostic payload.
    pub fn counts(&self) -> Result<Vec<(String, i64)>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT kind,count(*) FROM objects GROUP BY kind ORDER BY kind")?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmed_data_survives_reopen_and_stale_window_cannot_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        {
            let mut store = Store::open(&path).unwrap();
            assert_eq!(store.save("note", "n", 0, &"  original\n", 1).unwrap(), 1);
            assert!(matches!(
                store.save("note", "n", 0, &"replacement", 2),
                Err(Error::Conflict)
            ));
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.get::<String>("note", "n").unwrap().unwrap().value,
            "  original\n"
        );
        assert_eq!(store.counts().unwrap(), vec![("note".into(), 1)]);
    }

    #[test]
    fn refuses_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(Store::open(&path).is_err());
    }
}
