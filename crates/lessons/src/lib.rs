//! Alt+I — урок на ~15 минут (S7, F-25…F-31; A6, B20, B25). Alt+I organises
//! learning; it does not teach. A subject (Alt+S) holds the long-term goal; a
//! lesson holds a ~15-minute goal and a plan of 2–5 items that the reader can
//! only skip, never edit (B20), or replace whole ([Другой план]). Steps run
//! under a clock; «проработано» never means «освоено» and never touches FSRS
//! (INV-02). An unfinished lesson is resumed, never silently closed (INV-08).

mod context;
mod import;
mod plan;
mod rules;
mod store;

pub use context::gather;
pub use import::{
    ImportError, Imported, SubjectDraft, apply as import_subjects, export as export_subjects,
    parse as parse_subjects,
};
pub use plan::{
    Context, Criterion, PLAN_PROMPT, Plan, PlanError, PlanItemDraft, ProgramView, generate,
    plan_check, study_text_prompt, validate,
};
pub use rules::{
    Bridge, CYCLE_GAP, DEFAULT_MINUTES, MAX_MINUTES, READING, SESSION_GAP, parse_step,
};
pub use store::{
    FeynmanRecord, FeynmanStatus, ItemStatus, Journey, Lesson, LessonStore, Outcome, PlanItem,
    Step, StoreError, Subject, Topic, end_of_day,
};
