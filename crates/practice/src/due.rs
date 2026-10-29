//! Units whose FSRS review is due, with their evidence from the journal, for
//! «Достань из памяти» (B31). Shared by the CLI and the Alt+T window.

use std::path::Path;

use cognitive_exoskeleton_journal::{Journal, citable_events, marked_text};
use cognitive_exoskeleton_memory::Memory;
use cognitive_exoskeleton_units::UnitStore;

use crate::families::Family;
use crate::recall::{DueUnit, EvidenceLine};

#[derive(Debug, thiserror::Error)]
pub enum DueError {
    #[error("units: {0}")]
    Units(#[from] cognitive_exoskeleton_units::StoreError),
    #[error("memory: {0}")]
    Memory(#[from] cognitive_exoskeleton_memory::MemoryError),
    #[error("journal: {0}")]
    Journal(#[from] cognitive_exoskeleton_journal::JournalError),
}

pub fn due_units(db: impl AsRef<Path>, at: i64) -> Result<Vec<DueUnit>, DueError> {
    let db = db.as_ref();
    let store = UnitStore::open(db)?;
    let mut memory = Memory::open(db)?;
    let journal = Journal::open(db)?;
    sync_memory(&store, &mut memory, at)?;
    let mut units = Vec::new();
    for due in memory.due(at)? {
        let registry = store.registry(&due.subject)?;
        let Some(unit) = registry.into_iter().find(|u| u.id == due.unit) else {
            continue;
        };
        let sequences = store.events_of(&unit.id)?;
        let events = journal.read_sequences(&sequences)?;
        let evidence = citable_events(&events)
            .into_iter()
            .map(|stored| {
                let (marker, text) = marked_text(stored);
                EvidenceLine {
                    sequence: stored.sequence,
                    marker,
                    text,
                }
            })
            .collect();
        let family = store.family(&unit.id)?.and_then(|f| Family::parse(&f));
        units.push(DueUnit {
            family,
            id: unit.id,
            title: unit.title,
            action: unit.action,
            kind: unit.kind,
            evidence,
        });
    }
    Ok(units)
}

/// Every current unit is on the schedule (B35: units always rest on the reader's
/// own events and belong to an Alt+S subject).
pub fn sync_memory(store: &UnitStore, memory: &mut Memory, at: i64) -> Result<(), DueError> {
    for (unit, subject) in store.all_units()? {
        memory.enroll(&unit, &subject, at)?;
    }
    Ok(())
}
