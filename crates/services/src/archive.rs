//! Read-only inspection of a self-contained historical SQLite archive.
//! immutable=1 prevents creation/modification of WAL/SHM sidecars. Live WAL
//! databases are refused instead of silently omitting uncheckpointed history.
use crate::Error;
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde::Serialize;
use std::path::Path;

pub struct Archive {
    connection: Connection,
}

#[derive(Debug, Serialize)]
pub struct TablePreview {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_limit: usize,
    pub text_limit_bytes: usize,
    pub truncated: bool,
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().canonicalize()?;
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        if std::fs::metadata(Path::new(&wal)).is_ok_and(|metadata| metadata.len() > 0) {
            return Err(Error::Invalid(
                "archive has a live WAL; supply a self-contained SQLite snapshot, not a running database",
            ));
        }
        let mut url = reqwest::Url::from_file_path(path)
            .map_err(|_| Error::Invalid("invalid archive path"))?;
        url.set_query(Some("mode=ro&immutable=1"));
        let connection = Connection::open_with_flags(
            url.as_str(),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.pragma_update(None, "query_only", true)?;
        Ok(Self { connection })
    }

    pub fn tables(&self) -> Result<Vec<String>, Error> {
        let mut statement=self.connection.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
        Ok(statement
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn preview(&self, table: &str, limit: usize) -> Result<TablePreview, Error> {
        if !self.tables()?.iter().any(|name| name == table) {
            return Err(Error::Invalid("archive table not found"));
        }
        let limit = limit.clamp(1, 200);
        let quoted = table.replace('"', "\"\"");
        let mut statement = self
            .connection
            .prepare(&format!("SELECT * FROM \"{quoted}\" LIMIT ?1"))?;
        let columns = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut rows = statement.query([limit as i64 + 1])?;
        let mut result = TablePreview {
            columns,
            rows: vec![],
            row_limit: limit,
            text_limit_bytes: 65536,
            truncated: false,
        };
        while let Some(row) = rows.next()? {
            if result.rows.len() == limit {
                result.truncated = true;
                break;
            }
            let mut values = Vec::new();
            for index in 0..result.columns.len() {
                values.push(match row.get_ref(index)? {
                    ValueRef::Null => serde_json::Value::Null,
                    ValueRef::Integer(value) => value.into(),
                    ValueRef::Real(value) => serde_json::json!(value),
                    ValueRef::Text(value) => {
                        let text = String::from_utf8_lossy(value);
                        let mut end = text.len().min(65536);
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        if end < text.len() {
                            result.truncated = true;
                        }
                        serde_json::Value::String(text[..end].to_owned())
                    }
                    ValueRef::Blob(value) => {
                        result.truncated = true;
                        serde_json::json!({"blob_bytes":value.len(),"omitted":true})
                    }
                });
            }
            result.rows.push(values);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_read_preserves_all_bytes_and_creates_no_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old archive.sqlite3");
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(
                "CREATE TABLE notes(text TEXT); INSERT INTO notes VALUES('original');",
            )
            .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let archive = Archive::open(&path).unwrap();
        assert_eq!(archive.tables().unwrap(), vec!["notes"]);
        let preview = archive.preview("notes", 10).unwrap();
        assert_eq!(preview.rows[0][0], "original");
        assert!(archive.preview("notes\"; DROP TABLE notes; --", 1).is_err());
        assert_eq!(before, std::fs::read(path).unwrap());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
