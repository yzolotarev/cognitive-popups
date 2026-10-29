//! The reader's own say over one skill (F-37, Q-39, B67): pause it, take it
//! back, or retire it. Only «Умение» (Alt+T) looks at this; the FSRS schedule,
//! «Достань из памяти» and the lessons are not touched.
//!
//! - `suspended` - does not come up in «Умение» until [`Action::Resume`];
//! - `retired` (снято) - never offered, yet kept with its journal and brought
//!   back by [`Action::Resume`] («Вернуть»);
//! - nothing here is automatic and nothing is deleted (INV-12): the row stays
//!   after a resume, only its state changes.
//!
//! Every change is also a user event in the journal (INV-04: the key holds the
//! unit, the new state and the moment, so one click is one record).

use std::path::Path;

use cognitive_exoskeleton_domain::{NewEvent, OperationId};
use cognitive_exoskeleton_journal::Journal;
use cognitive_exoskeleton_units::UnitStore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum StandingError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("units: {0}")]
    Units(#[from] cognitive_exoskeleton_units::StoreError),
    #[error("journal: {0}")]
    Journal(#[from] cognitive_exoskeleton_journal::JournalError),
    #[error("event: {0}")]
    Event(#[from] cognitive_exoskeleton_domain::DomainError),
    #[error("bad time")]
    Time,
    #[error("единицы {0} нет")]
    UnknownUnit(String),
}

/// Where a unit stands for the «Умение» route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    #[default]
    Active,
    Suspended,
    Retired,
}

impl Standing {
    pub fn as_str(self) -> &'static str {
        match self {
            Standing::Active => "active",
            Standing::Suspended => "suspended",
            Standing::Retired => "retired",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "suspended" => Standing::Suspended,
            "retired" => Standing::Retired,
            _ => Standing::Active,
        }
    }
}

/// What the reader presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Suspend,
    Resume,
    Retire,
}

impl Action {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "suspend" => Some(Action::Suspend),
            "resume" => Some(Action::Resume),
            "retire" => Some(Action::Retire),
            _ => None,
        }
    }

    fn target(self) -> Standing {
        match self {
            Action::Suspend => Standing::Suspended,
            Action::Resume => Standing::Active,
            Action::Retire => Standing::Retired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub unit: String,
    pub standing: Standing,
    pub at: i64,
}

pub struct SkillStandings {
    connection: Connection,
}

impl SkillStandings {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StandingError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, StandingError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, StandingError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS practice_skill_standing (
                 unit_id  TEXT PRIMARY KEY,
                 standing TEXT NOT NULL,
                 at       INTEGER NOT NULL
             );",
        )?;
        Ok(Self { connection })
    }

    /// No row means the unit was never touched: active.
    pub fn standing(&self, unit: &str) -> Result<Standing, StandingError> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT standing FROM practice_skill_standing WHERE unit_id = ?1",
                [unit],
                |r| r.get(0),
            )
            .optional()?;
        Ok(value.map_or(Standing::Active, |v| Standing::parse(&v)))
    }

    /// Sets the state; true if it changed (same state again is a no-op).
    pub fn set(&mut self, unit: &str, standing: Standing, now: i64) -> Result<bool, StandingError> {
        if self.standing(unit)? == standing {
            return Ok(false);
        }
        self.connection.execute(
            "INSERT INTO practice_skill_standing (unit_id, standing, at) VALUES (?1, ?2, ?3)
             ON CONFLICT (unit_id) DO UPDATE SET standing = ?2, at = ?3",
            params![unit, standing.as_str(), now],
        )?;
        Ok(true)
    }

    /// Units the reader paused or retired, oldest decision first.
    pub fn held(&self) -> Result<Vec<Entry>, StandingError> {
        let mut statement = self.connection.prepare(
            "SELECT unit_id, standing, at FROM practice_skill_standing
             WHERE standing <> 'active' ORDER BY at, unit_id",
        )?;
        let rows = statement
            .query_map([], |r| {
                Ok(Entry {
                    unit: r.get(0)?,
                    standing: Standing::parse(&r.get::<_, String>(1)?),
                    at: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// What came of a press.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub unit: String,
    pub before: Standing,
    pub after: Standing,
    /// False when the unit already stood there: nothing is written.
    pub changed: bool,
}

/// The reader's action on one unit: the state and the user event in the
/// journal. Idempotent: pressing the same thing twice writes once.
pub fn apply(
    db: impl AsRef<Path>,
    unit: &str,
    action: Action,
    now: i64,
) -> Result<Change, StandingError> {
    let db = db.as_ref();
    let units = UnitStore::open(db)?;
    let subject = units
        .all_units()?
        .into_iter()
        .find(|(id, _)| id == unit)
        .map(|(_, subject)| subject)
        .ok_or_else(|| StandingError::UnknownUnit(unit.to_string()))?;
    let title = units
        .registry(&subject)?
        .into_iter()
        .find(|u| u.id == unit)
        .map_or_else(|| unit.to_string(), |u| u.title);
    let mut standings = SkillStandings::open(db)?;
    let before = standings.standing(unit)?;
    let after = action.target();
    if !standings.set(unit, after, now)? {
        return Ok(Change {
            unit: unit.to_string(),
            before,
            after,
            changed: false,
        });
    }
    let text = match action {
        Action::Suspend => format!("Умение «{title}»: поставил на паузу"),
        Action::Resume if before == Standing::Retired => {
            format!("Умение «{title}»: вернул из снятых")
        }
        Action::Resume => format!("Умение «{title}»: вернул с паузы"),
        Action::Retire => format!("Умение «{title}»: снял, не предлагать"),
    };
    let key = format!("skill-standing:{unit}:{}:{now}", after.as_str());
    let event = NewEvent::user_text(
        OperationId::new(key.clone())?,
        OffsetDateTime::from_unix_timestamp(now).map_err(|_| StandingError::Time)?,
        key,
        text,
    )?;
    Journal::open(db)?.append(&[event])?;
    Ok(Change {
        unit: unit.to_string(),
        before,
        after,
        changed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_is_active_and_every_move_is_reversible() {
        let mut s = SkillStandings::open_in_memory().unwrap();
        assert_eq!(s.standing("u").unwrap(), Standing::Active);
        assert!(s.set("u", Standing::Suspended, 1).unwrap());
        assert!(!s.set("u", Standing::Suspended, 2).unwrap(), "no-op");
        assert!(s.set("u", Standing::Retired, 3).unwrap());
        assert_eq!(s.held().unwrap().len(), 1);
        assert!(s.set("u", Standing::Active, 4).unwrap());
        assert!(s.held().unwrap().is_empty());
        assert_eq!(s.standing("u").unwrap(), Standing::Active);
    }

    #[test]
    fn actions_parse() {
        assert_eq!(Action::parse("retire"), Some(Action::Retire));
        assert_eq!(Action::parse("x"), None);
    }
}
