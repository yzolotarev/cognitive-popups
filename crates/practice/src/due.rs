//! Units whose FSRS review is due, with their evidence from the journal, for
//! «Достань из памяти» (B31). Shared by the CLI and the Alt+T window.

use std::path::Path;

use cognitive_exoskeleton_journal::{Journal, citable_events, marked_text};
use cognitive_exoskeleton_memory::Memory;
use cognitive_exoskeleton_units::{RegistryUnit, UnitStore};

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
        units.push(load_unit(&store, &journal, unit)?);
    }
    Ok(units)
}

/// A unit with its evidence from the journal, ready for a recall card.
pub fn load_unit(
    store: &UnitStore,
    journal: &Journal,
    unit: RegistryUnit,
) -> Result<DueUnit, DueError> {
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
    Ok(DueUnit {
        family,
        id: unit.id,
        title: unit.title,
        action: unit.action,
        kind: unit.kind,
        evidence,
    })
}

/// Every current unit is on the schedule (B35: units always rest on the reader's
/// own events and belong to an Alt+S subject).
pub fn sync_memory(store: &UnitStore, memory: &mut Memory, at: i64) -> Result<(), DueError> {
    for (unit, subject) in store.all_units()? {
        memory.enroll(&unit, &subject, at)?;
    }
    Ok(())
}

/// Already generated experimental cards retain exactly their registered question and quote.
pub fn block_cards(
    db: impl AsRef<Path>,
    at: i64,
) -> Result<Vec<crate::recall::RecallCard>, String> {
    let db = db.as_ref();
    let materials =
        cognitive_exoskeleton_materials::Materials::open(db).map_err(|e| e.to_string())?;
    let due = Memory::open(db)
        .and_then(|m| m.due(at))
        .map_err(|e| e.to_string())?;
    Ok(materials
        .assignments()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|a| a.card_status == "created" && due.iter().any(|d| d.unit == a.id))
        .map(|a| crate::recall::RecallCard {
            unit: a.id,
            question: a.question.unwrap_or_default(),
            reference: a.reference.unwrap_or_default(),
            events: Vec::new(),
            family: None,
            seed: None,
        })
        .collect())
}

/// Stored experimental cards remain usable even if generation for ordinary units fails.
pub fn recall_with_blocks<T: cognitive_exoskeleton_ai::Transport>(
    db: impl AsRef<Path>,
    client: &cognitive_exoskeleton_ai::Client<T>,
    prompts: &cognitive_exoskeleton_ai::PromptStore,
    at: i64,
) -> Result<crate::recall::RecallSet, crate::PracticeError> {
    let db = db.as_ref();
    let stored = block_cards(db, at).unwrap_or_default();
    let due = due_units(db, at).unwrap_or_default();
    let generated = if due.is_empty() {
        Ok(crate::recall::RecallSet::default())
    } else {
        crate::recall::recall_cards(
            client,
            prompts,
            &due,
            at as u64,
            cognitive_exoskeleton_ai::Priority::Foreground,
        )
    };
    let mut set = match generated {
        Ok(set) => set,
        Err(e) if !stored.is_empty() => crate::recall::RecallSet {
            cards: Vec::new(),
            notes: vec![e.to_string()],
        },
        Err(e) => return Err(e),
    };
    set.cards.extend(stored);
    Ok(set)
}
