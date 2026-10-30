//! «Лента» in Alt+T (B90): one card at a time, the next one already waiting.
//! Rules, no model: the code picks the operation, its level and when to try a
//! step up; the model only writes cards at the level it is asked for.
//!
//! - A clean card is a match marked before the answer was opened.
//! - Three clean in a row on the current operation → one card a level higher
//!   (two when the last eight were almost all clean). Clean on it → the level
//!   goes up; a miss → the level stays, nothing is lost.
//! - Two misses in a row → one level down, not below the text's own. A match
//!   after opening the answer is a miss here. Speed never changes the level:
//!   slow can be thinking; it is only kept for later study.
//! - About one card in seven keeps something else alive: a due FSRS card, else
//!   another operation of the text. When the last eight go badly, more often.
//! - Clean three times at the top level → the operation is settled for this
//!   sitting and the next one comes up.
//! - A level saved from an earlier sitting starts one lower (a short warm-up).
//!
//! Formats open up as operations hold (second layer, when the window offers
//! them): two operations at level 3 or higher → «Какая операция тут нужна?»;
//! a settled operation → «В двух словах: почему так?» on it; two settled →
//! «Собери вместе». About one card in seven is one of them. Two misses in a
//! row in a format put it aside for the next eight cards.

use std::collections::VecDeque;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::MAX_LEVEL;
use crate::guidance::Attempt;

/// Cards remembered for the success rate.
pub const WINDOW: usize = 8;
/// Ready cards the window keeps for the current operation.
pub const BUFFER: usize = 5;
/// Clean in a row before a step up is tried.
pub const STEP_UP_AFTER: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// The current operation at its level.
    Current,
    /// One card of the current operation a level higher.
    StepUp,
    /// Another operation of the text, at its own level.
    Other,
    /// A due card from «Достань из памяти».
    Recall,
    /// «Какая операция тут нужна?»: a task without its operation named.
    Which,
    /// «В двух словах: почему так?» on a settled operation.
    Explain,
    /// «Собери вместе»: one task on several settled operations.
    Whole,
}

/// The formats of the second layer, in the order they open up.
pub const FORMATS: [Pick; 3] = [Pick::Which, Pick::Explain, Pick::Whole];
/// Cards a format stays aside after two misses in a row in it.
pub const FORMAT_PAUSE: u32 = 8;
/// Share of the feed for the second layer once something has opened up.
const FORMAT_SHARE: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Next {
    pub op: usize,
    pub level: u8,
    pub pick: Pick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Op {
    key: String,
    level: u8,
    clean_in_row: u32,
    misses_in_row: u32,
    settled: bool,
    seen: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    ops: Vec<Op>,
    current: usize,
    recent: VecDeque<bool>,
    done: u32,
    /// Whether the window can show the second layer.
    formats: bool,
    /// Per format of [`FORMATS`]: misses in a row, and the card count until
    /// which it is put aside.
    format_misses: [u32; 3],
    format_paused_until: [u32; 3],
}

impl Feed {
    /// Operations of the text with the level saved for each (1 when new).
    pub fn new(ops: impl IntoIterator<Item = (String, u8)>) -> Self {
        let ops = ops
            .into_iter()
            .map(|(key, saved)| Op {
                key,
                level: saved.clamp(1, MAX_LEVEL).saturating_sub(1).max(1),
                clean_in_row: 0,
                misses_in_row: 0,
                settled: false,
                seen: 0,
            })
            .collect();
        Self {
            ops,
            current: 0,
            recent: VecDeque::new(),
            done: 0,
            formats: false,
            format_misses: [0; 3],
            format_paused_until: [0; 3],
        }
    }

    /// The window shows the second layer too.
    pub fn with_formats(mut self) -> Self {
        self.formats = true;
        self
    }

    fn format_index(pick: Pick) -> Option<usize> {
        FORMATS.iter().position(|f| *f == pick)
    }

    /// Formats open now: unlocked by the operations and not put aside.
    pub fn open_formats(&self) -> Vec<Pick> {
        if !self.formats {
            return Vec::new();
        }
        let strong = self.ops.iter().filter(|op| op.level >= 3).count();
        let settled = self.ops.iter().filter(|op| op.settled).count();
        let unlocked = [strong >= 2, settled >= 1, settled >= 2];
        FORMATS
            .iter()
            .enumerate()
            .filter(|(i, _)| unlocked[*i] && self.done >= self.format_paused_until[*i])
            .map(|(_, f)| *f)
            .collect()
    }

    /// A settled operation for «В двух словах», the least seen first.
    fn settled_op(&self) -> Option<usize> {
        (0..self.ops.len())
            .filter(|i| self.ops[*i].settled)
            .min_by_key(|i| self.ops[*i].seen)
    }

    /// Settled operations, for «Собери вместе».
    pub fn settled(&self) -> Vec<usize> {
        (0..self.ops.len())
            .filter(|i| self.ops[*i].settled)
            .collect()
    }

    /// Share of clean cards among the last eight; None before four.
    pub fn success_rate(&self) -> Option<f64> {
        (self.recent.len() >= 4)
            .then(|| self.recent.iter().filter(|c| **c).count() as f64 / self.recent.len() as f64)
    }

    fn step_up_after(&self) -> u32 {
        match self.success_rate() {
            Some(rate) if self.recent.len() == WINDOW && rate > 0.9 => STEP_UP_AFTER - 1,
            _ => STEP_UP_AFTER,
        }
    }

    fn maintenance_share(&self) -> f64 {
        match self.success_rate() {
            Some(rate) if rate < 0.7 => 0.3,
            _ => 0.15,
        }
    }

    pub fn current(&self) -> usize {
        self.current
    }

    pub fn level(&self, op: usize) -> u8 {
        self.ops[op].level
    }

    pub fn key(&self, op: usize) -> &str {
        &self.ops[op].key
    }

    /// Every operation settled: the feed has nothing new to raise.
    pub fn all_settled(&self) -> bool {
        self.ops.iter().all(|op| op.settled)
    }

    fn step_up_due(&self) -> bool {
        let op = &self.ops[self.current];
        op.level < MAX_LEVEL && op.clean_in_row >= self.step_up_after()
    }

    /// The least seen other operation, if there is one.
    fn other(&self) -> Option<usize> {
        (0..self.ops.len())
            .filter(|i| *i != self.current)
            .min_by_key(|i| self.ops[*i].seen)
    }

    /// What comes next. `roll` is uniform in [0, 1); `recall_due` says whether
    /// «Достань из памяти» has a card due now.
    pub fn next(&self, roll: f64, recall_due: bool) -> Next {
        let current = Next {
            op: self.current,
            level: self.ops[self.current].level,
            pick: Pick::Current,
        };
        // A due review is the warm-up: easy to start, and it is due anyway.
        if self.done == 0 && recall_due {
            return Next {
                pick: Pick::Recall,
                ..current
            };
        }
        if self.step_up_due() {
            return Next {
                level: current.level + 1,
                pick: Pick::StepUp,
                ..current
            };
        }
        let upkeep = self.maintenance_share();
        let formats = self.open_formats();
        if !formats.is_empty() && roll >= upkeep && roll < upkeep + FORMAT_SHARE {
            let slot = ((roll - upkeep) / FORMAT_SHARE * formats.len() as f64) as usize;
            let pick = formats[slot.min(formats.len() - 1)];
            let op = match pick {
                Pick::Explain => self.settled_op().unwrap_or(self.current),
                _ => self.current,
            };
            return Next {
                op,
                level: self.ops[op].level,
                pick,
            };
        }
        if roll < upkeep {
            if recall_due {
                return Next {
                    pick: Pick::Recall,
                    ..current
                };
            }
            if let Some(op) = self.other() {
                return Next {
                    op,
                    level: self.ops[op].level,
                    pick: Pick::Other,
                };
            }
        }
        current
    }

    /// What the window should have ready, most urgent first: the current
    /// operation at its level, the level above once a step up is near, and
    /// the next other operation.
    pub fn wanted(&self) -> Vec<(usize, u8)> {
        let op = &self.ops[self.current];
        let mut wanted = vec![(self.current, op.level)];
        if op.level < MAX_LEVEL && op.clean_in_row + 1 >= self.step_up_after() {
            wanted.push((self.current, op.level + 1));
        }
        if let Some(other) = self.other() {
            wanted.push((other, self.ops[other].level));
        }
        wanted
    }

    /// The reader marked the card `next` brought. `clean` is a match marked
    /// before the answer was opened.
    pub fn record(&mut self, next: Next, clean: bool) {
        self.done += 1;
        // A review is graded in its own card (FSRS); it does not steer the feed.
        if next.pick != Pick::Recall {
            self.recent.push_back(clean);
            if self.recent.len() > WINDOW {
                self.recent.pop_front();
            }
        }
        match next.pick {
            Pick::Recall => {}
            Pick::Which | Pick::Explain | Pick::Whole => {
                let Some(i) = Self::format_index(next.pick) else {
                    return;
                };
                if clean {
                    self.format_misses[i] = 0;
                } else {
                    self.format_misses[i] += 1;
                    if self.format_misses[i] >= 2 {
                        self.format_misses[i] = 0;
                        self.format_paused_until[i] = self.done + FORMAT_PAUSE;
                    }
                }
            }
            Pick::Other => self.ops[next.op].seen += 1,
            Pick::StepUp => {
                let op = &mut self.ops[next.op];
                op.seen += 1;
                op.clean_in_row = 0;
                op.misses_in_row = 0;
                if clean {
                    op.level = (op.level + 1).min(MAX_LEVEL);
                }
            }
            Pick::Current => {
                let op = &mut self.ops[next.op];
                op.seen += 1;
                if clean {
                    op.misses_in_row = 0;
                    op.clean_in_row += 1;
                    if op.level == MAX_LEVEL && op.clean_in_row >= STEP_UP_AFTER {
                        op.settled = true;
                        self.advance();
                    }
                } else {
                    op.clean_in_row = 0;
                    op.misses_in_row += 1;
                    if op.misses_in_row >= 2 && op.level > 1 {
                        op.level -= 1;
                        op.misses_in_row = 0;
                    }
                }
            }
        }
    }

    /// «Шаг поменьше» pressed by the reader: one level down now.
    pub fn smaller_step(&mut self) {
        let op = &mut self.ops[self.current];
        op.level = op.level.saturating_sub(1).max(1);
        op.clean_in_row = 0;
        op.misses_in_row = 0;
    }

    fn advance(&mut self) {
        let n = self.ops.len();
        if let Some(next) = (1..=n)
            .map(|step| (self.current + step) % n)
            .find(|i| !self.ops[*i].settled)
        {
            self.current = next;
        }
    }

    /// Levels to save for the next sitting.
    pub fn levels(&self) -> Vec<(String, u8)> {
        self.ops
            .iter()
            .map(|op| (op.key.clone(), op.level))
            .collect()
    }
}

impl Pick {
    pub fn as_str(self) -> &'static str {
        match self {
            Pick::Current => "current",
            Pick::StepUp => "step_up",
            Pick::Other => "other",
            Pick::Recall => "recall",
            Pick::Which => "which",
            Pick::Explain => "explain",
            Pick::Whole => "whole",
        }
    }
}

/// The key an operation is saved under: its title, case and spaces aside.
pub fn op_key(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Levels between sittings and every marked card of the feed (B90).
pub struct FeedStore {
    connection: Connection,
}

impl FeedStore {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> rusqlite::Result<Self> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS probe_levels (
                 op         TEXT PRIMARY KEY,
                 level      INTEGER NOT NULL CHECK (level BETWEEN 1 AND 5),
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS probe_attempts (
                 key        TEXT PRIMARY KEY,
                 op         TEXT NOT NULL,
                 level      INTEGER NOT NULL,
                 pick       TEXT NOT NULL,
                 clean      INTEGER NOT NULL,
                 correct    INTEGER NOT NULL,
                 before_reveal INTEGER NOT NULL,
                 -- Seconds from the card on screen to the mark: kept to study
                 -- later, never used to change the level (B90).
                 seconds    INTEGER NOT NULL,
                 at         INTEGER NOT NULL
             );",
        )?;
        Ok(Self { connection })
    }

    /// The saved level of an operation; 1 when it is new.
    pub fn level(&self, op: &str) -> rusqlite::Result<u8> {
        Ok(self
            .connection
            .query_row(
                "SELECT level FROM probe_levels WHERE op = ?1",
                [op_key(op)],
                |r| r.get::<_, u8>(0),
            )
            .optional()?
            .unwrap_or(1))
    }

    pub fn save_levels(&self, levels: &[(String, u8)], now: i64) -> rusqlite::Result<()> {
        for (op, level) in levels {
            self.connection.execute(
                "INSERT INTO probe_levels (op, level, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(op) DO UPDATE SET level = excluded.level,
                                               updated_at = excluded.updated_at",
                params![op_key(op), (*level).clamp(1, MAX_LEVEL), now],
            )?;
        }
        Ok(())
    }

    /// One marked card. The same key twice is one record (INV-04).
    pub fn attempt(
        &self,
        key: &str,
        op: &str,
        next: Next,
        attempt: Attempt,
        shown_at: i64,
        now: i64,
    ) -> rusqlite::Result<bool> {
        let added = self.connection.execute(
            "INSERT OR IGNORE INTO probe_attempts
                 (key, op, level, pick, clean, correct, before_reveal, seconds, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                key,
                op_key(op),
                next.level,
                next.pick.as_str(),
                attempt.correct && attempt.attempt_before_reveal,
                attempt.correct,
                attempt.attempt_before_reveal,
                (now - shown_at).max(0),
                now
            ],
        )?;
        Ok(added == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(levels: &[u8]) -> Feed {
        Feed::new(
            levels
                .iter()
                .enumerate()
                .map(|(i, level)| (format!("op{i}"), *level)),
        )
    }

    /// Marks whatever the feed brings with a roll that never asks for upkeep.
    fn run(feed: &mut Feed, clean: bool) -> Next {
        let next = feed.next(0.99, false);
        feed.record(next, clean);
        next
    }

    #[test]
    fn a_saved_level_starts_one_lower() {
        let f = feed(&[1, 3, 5]);
        assert_eq!((f.level(0), f.level(1), f.level(2)), (1, 2, 4));
    }

    #[test]
    fn three_clean_bring_one_card_a_level_higher() {
        let mut f = feed(&[1]);
        for _ in 0..3 {
            assert_eq!(run(&mut f, true).pick, Pick::Current);
        }
        let up = f.next(0.99, false);
        assert_eq!((up.pick, up.level), (Pick::StepUp, 2));
        f.record(up, true);
        assert_eq!(f.level(0), 2, "clean on the higher card: the level goes up");
        assert_eq!(run(&mut f, true).level, 2);
    }

    #[test]
    fn a_miss_on_the_step_up_keeps_the_level_and_tries_again_later() {
        let mut f = feed(&[1]);
        for _ in 0..3 {
            run(&mut f, true);
        }
        assert_eq!(run(&mut f, false).pick, Pick::StepUp);
        assert_eq!(f.level(0), 1, "nothing is lost");
        for _ in 0..2 {
            assert_eq!(run(&mut f, true).pick, Pick::Current);
        }
        run(&mut f, true);
        assert_eq!(f.next(0.99, false).pick, Pick::StepUp);
    }

    #[test]
    fn two_misses_in_a_row_go_one_level_down_not_below_one() {
        let mut f = feed(&[4]);
        assert_eq!(f.level(0), 3);
        run(&mut f, false);
        assert_eq!(f.level(0), 3);
        run(&mut f, false);
        assert_eq!(f.level(0), 2);
        let mut f = feed(&[1]);
        run(&mut f, false);
        run(&mut f, false);
        assert_eq!(f.level(0), 1);
    }

    #[test]
    fn a_match_after_the_answer_is_not_clean() {
        let mut f = feed(&[1]);
        for _ in 0..5 {
            let next = f.next(0.99, false);
            f.record(next, false);
        }
        assert_eq!(f.next(0.99, false).pick, Pick::Current);
        assert_eq!(f.success_rate(), Some(0.0));
    }

    #[test]
    fn a_due_review_is_the_warm_up_and_upkeep_comes_back() {
        let mut f = feed(&[1, 1]);
        let first = f.next(0.99, true);
        assert_eq!(first.pick, Pick::Recall);
        f.record(first, true);
        assert_eq!(f.next(0.99, true).pick, Pick::Current);
        assert_eq!(f.next(0.1, true).pick, Pick::Recall);
        let other = f.next(0.1, false);
        assert_eq!((other.pick, other.op), (Pick::Other, 1));
        assert_eq!(
            feed(&[1]).next(0.1, false).pick,
            Pick::Current,
            "one operation"
        );
    }

    #[test]
    fn upkeep_comes_more_often_when_it_goes_badly() {
        let mut f = feed(&[1, 1]);
        assert_eq!(f.next(0.2, false).pick, Pick::Current);
        for _ in 0..4 {
            let next = Next {
                op: 0,
                level: 1,
                pick: Pick::Current,
            };
            f.record(next, false);
        }
        assert_eq!(f.next(0.2, false).pick, Pick::Other);
    }

    #[test]
    fn steps_up_come_sooner_when_almost_everything_is_clean() {
        let mut f = feed(&[1]);
        for _ in 0..3 {
            run(&mut f, true);
        }
        f.record(f.next(0.99, false), true);
        for _ in 0..4 {
            run(&mut f, true);
            if f.next(0.99, false).pick == Pick::StepUp {
                break;
            }
        }
        assert_eq!(f.level(0), 2);
        assert_eq!(
            f.next(0.99, false).pick,
            Pick::StepUp,
            "two clean were enough"
        );
    }

    #[test]
    fn the_level_above_is_ready_before_the_step_up() {
        let mut f = feed(&[1, 1]);
        assert_eq!(f.wanted(), vec![(0, 1), (1, 1)]);
        run(&mut f, true);
        run(&mut f, true);
        assert_eq!(f.wanted(), vec![(0, 1), (0, 2), (1, 1)]);
    }

    #[test]
    fn three_clean_at_the_top_settle_the_operation_and_the_next_comes_up() {
        let mut f = feed(&[5, 1]);
        for _ in 0..3 {
            run(&mut f, true);
        }
        assert_eq!(run(&mut f, true).pick, Pick::StepUp);
        assert_eq!(f.level(0), 5);
        for _ in 0..3 {
            run(&mut f, true);
        }
        assert_eq!(f.current(), 1);
        assert!(!f.all_settled());
        assert_eq!(f.levels(), vec![("op0".into(), 5), ("op1".into(), 1)]);
    }

    #[test]
    fn formats_open_up_as_operations_hold() {
        let f = feed(&[4, 4, 1]).with_formats();
        assert_eq!(f.open_formats(), vec![Pick::Which], "two at level 3");
        assert!(
            feed(&[4, 4]).open_formats().is_empty(),
            "window without them"
        );
        let mut f = feed(&[5, 5, 1]).with_formats();
        for _ in 0..2 {
            for _ in 0..3 {
                run(&mut f, true);
            }
            run(&mut f, true);
            for _ in 0..3 {
                run(&mut f, true);
            }
        }
        assert_eq!(f.settled(), vec![0, 1]);
        assert_eq!(
            f.open_formats(),
            vec![Pick::Which, Pick::Explain, Pick::Whole]
        );
        let explain = f.next(0.15 + 0.15 * 0.5, false);
        assert_eq!(explain.pick, Pick::Explain);
        assert!(explain.op < 2, "on a settled operation");
    }

    #[test]
    fn two_misses_in_a_format_put_it_aside_for_a_while() {
        let mut f = feed(&[4, 4]).with_formats();
        let which = f.next(0.2, false);
        assert_eq!(which.pick, Pick::Which);
        f.record(which, false);
        f.record(which, false);
        assert!(f.open_formats().is_empty());
        for _ in 0..FORMAT_PAUSE {
            run(&mut f, true);
        }
        assert!(f.open_formats().contains(&Pick::Which));
    }

    #[test]
    fn levels_are_kept_between_sittings_and_a_card_is_one_record() {
        let store = FeedStore::open_in_memory().unwrap();
        assert_eq!(store.level("Формула Байеса").unwrap(), 1);
        store
            .save_levels(&[("Формула  Байеса".into(), 4)], 1)
            .unwrap();
        assert_eq!(store.level("формула байеса").unwrap(), 4);
        let next = Next {
            op: 0,
            level: 4,
            pick: Pick::Current,
        };
        let clean = Attempt {
            correct: true,
            attempt_before_reveal: true,
        };
        assert!(
            store
                .attempt("k1", "Формула Байеса", next, clean, 1, 2)
                .unwrap()
        );
        assert!(
            !store
                .attempt("k1", "Формула Байеса", next, clean, 1, 2)
                .unwrap()
        );
    }

    #[test]
    fn the_reader_can_always_step_down() {
        let mut f = feed(&[3]);
        f.smaller_step();
        f.smaller_step();
        f.smaller_step();
        assert_eq!(f.level(0), 1);
    }
}
