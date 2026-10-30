//! What the probe window highlights next (S5, rules 3–5 and 7). Rules, no model.

/// One mark on a card. Before [Ответ] it is an independent attempt; after it,
/// the reader compares with the revealed answer (rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt {
    pub correct: bool,
    pub attempt_before_reveal: bool,
}

/// One card on screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CardSession {
    revealed: bool,
    marked: bool,
}

impl CardSession {
    pub fn reveal(&mut self) {
        self.revealed = true;
    }

    pub fn is_revealed(&self) -> bool {
        self.revealed
    }

    /// ✓ / ✗ or «Совпало / Промахнулся». The first mark counts; a second click
    /// on the same card changes nothing (INV-04).
    pub fn mark(&mut self, correct: bool) -> Option<Attempt> {
        if self.marked {
            return None;
        }
        self.marked = true;
        Some(Attempt {
            correct,
            attempt_before_reveal: !self.revealed,
        })
    }
}

/// The highlighted button (F-19, `ProbeGuidance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// First miss: another case of the same operation.
    OtherCase,
    /// Second miss in a row: a smaller step.
    SmallerStep,
    /// Success on a new case: three more.
    MoreThree,
    /// Three clean in a row below the top level: one level up (B83).
    Harder,
    /// Three clean in a row at the top level: nothing highlighted.
    Nothing,
}

/// Per category of the probe tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guidance {
    misses_in_row: u32,
    clean_in_row: u32,
    level: u8,
}

impl Default for Guidance {
    fn default() -> Self {
        Self {
            misses_in_row: 0,
            clean_in_row: 0,
            level: 1,
        }
    }
}

impl Guidance {
    /// The level of the cards on screen (B83): 1 is the text's own.
    pub fn level(&self) -> u8 {
        self.level
    }

    /// [Сложнее] was answered with new cards: one level up, a fresh streak.
    pub fn level_up(&mut self) {
        self.level = (self.level + 1).min(crate::MAX_LEVEL);
        self.misses_in_row = 0;
        self.clean_in_row = 0;
    }

    /// [Шаг поменьше]: one level down, not below the text's own.
    pub fn level_down(&mut self) {
        self.level = self.level.saturating_sub(1).max(1);
    }

    pub fn after(&mut self, attempt: Attempt) -> Hint {
        let clean = attempt.correct && attempt.attempt_before_reveal;
        if !attempt.correct {
            self.clean_in_row = 0;
            self.misses_in_row += 1;
            return if self.misses_in_row == 1 {
                Hint::OtherCase
            } else {
                Hint::SmallerStep
            };
        }
        self.misses_in_row = 0;
        self.clean_in_row = if clean { self.clean_in_row + 1 } else { 0 };
        if self.clean_in_row >= 3 && self.level < crate::MAX_LEVEL {
            Hint::Harder
        } else if self.clean_in_row >= 3 {
            Hint::Nothing
        } else {
            Hint::MoreThree
        }
    }

    /// [Собери вместе] after three clean variations in the category (B24).
    pub fn whole_task_ready(&self) -> bool {
        self.clean_in_row >= 3
    }
}

/// Whole-task types allowed for a unit (rule 7): a reader with no independent
/// correct answer in the unit yet gets worked-example types only.
pub fn whole_task_types(has_independent_correct: bool) -> Vec<u8> {
    if has_independent_correct {
        (1..=10).collect()
    } else {
        vec![1, 7, 8, 9, 10]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_before_the_answer_is_an_independent_attempt() {
        let mut card = CardSession::default();
        let attempt = card.mark(true).unwrap();
        assert!(attempt.attempt_before_reveal);
        assert_eq!(card.mark(false), None, "a second click changes nothing");

        let mut card = CardSession::default();
        card.reveal();
        assert!(!card.mark(true).unwrap().attempt_before_reveal);
    }

    #[test]
    fn hints_follow_the_rules() {
        let ok = Attempt {
            correct: true,
            attempt_before_reveal: true,
        };
        let miss = Attempt {
            correct: false,
            attempt_before_reveal: true,
        };
        let mut g = Guidance::default();
        assert_eq!(g.after(miss), Hint::OtherCase);
        assert_eq!(g.after(miss), Hint::SmallerStep);
        assert_eq!(g.after(ok), Hint::MoreThree);
        assert_eq!(g.after(ok), Hint::MoreThree);
        assert!(!g.whole_task_ready());
        assert_eq!(g.after(ok), Hint::Harder, "three clean: one level up");
        assert!(g.whole_task_ready());
        assert_eq!(g.after(miss), Hint::OtherCase);
        assert!(!g.whole_task_ready());
    }

    #[test]
    fn levels_go_from_one_to_five_and_reset_the_streak() {
        let ok = Attempt {
            correct: true,
            attempt_before_reveal: true,
        };
        let mut g = Guidance::default();
        assert_eq!(g.level(), 1);
        g.level_down();
        assert_eq!(g.level(), 1, "not below the text's own level");
        for _ in 0..3 {
            g.after(ok);
        }
        g.level_up();
        assert_eq!(g.level(), 2);
        assert!(!g.whole_task_ready(), "a new level starts a new streak");
        for _ in 0..10 {
            g.level_up();
        }
        assert_eq!(g.level(), crate::MAX_LEVEL);
        for _ in 0..2 {
            assert_eq!(g.after(ok), Hint::MoreThree);
        }
        assert_eq!(g.after(ok), Hint::Nothing, "nothing harder than the top");
        g.level_down();
        assert_eq!(g.level(), crate::MAX_LEVEL - 1);
    }

    #[test]
    fn a_match_after_seeing_the_answer_is_not_clean() {
        let seen = Attempt {
            correct: true,
            attempt_before_reveal: false,
        };
        let mut g = Guidance::default();
        for _ in 0..5 {
            g.after(seen);
        }
        assert!(!g.whole_task_ready());
    }

    #[test]
    fn a_newcomer_gets_worked_example_types_only() {
        assert_eq!(whole_task_types(false), vec![1, 7, 8, 9, 10]);
        assert_eq!(whole_task_types(true).len(), 10);
    }
}
