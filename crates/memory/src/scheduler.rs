//! One FSRS schedule (S4, rules 1 and 5): learning steps of 1 and 10 minutes,
//! a relearning step of 10 minutes, then intervals from FSRS-6 for a desired
//! retention of 0.9. The phases follow py-fsrs; memory state is updated on every
//! review, including same-day ones (FSRS-6 short-term formula).

use fsrs::{FSRS, MemoryState};

pub const DESIRED_RETENTION: f32 = 0.9;
pub const LEARNING_STEPS: [i64; 2] = [60, 600];
pub const RELEARNING_STEPS: [i64; 1] = [600];
pub const MAX_INTERVAL_DAYS: i64 = 36_500;
pub const DAY: i64 = 86_400;

/// Easy does not exist here (S4, rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    Again,
    Hard,
    Good,
}

impl Grade {
    pub fn rating(self) -> u32 {
        match self {
            Grade::Again => 1,
            Grade::Hard => 2,
            Grade::Good => 3,
        }
    }

    pub fn from_rating(rating: u32) -> Option<Self> {
        match rating {
            1 => Some(Grade::Again),
            2 => Some(Grade::Hard),
            3 => Some(Grade::Good),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Grade::Again => "again",
            Grade::Hard => "hard",
            Grade::Good => "good",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "again" => Some(Grade::Again),
            "hard" => Some(Grade::Hard),
            "good" => Some(Grade::Good),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    New,
    Learning,
    Review,
    Relearning,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::New => "new",
            Phase::Learning => "learning",
            Phase::Review => "review",
            Phase::Relearning => "relearning",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "new" => Some(Phase::New),
            "learning" => Some(Phase::Learning),
            "review" => Some(Phase::Review),
            "relearning" => Some(Phase::Relearning),
            _ => None,
        }
    }
}

/// What the schedule knows about one unit. Times are Unix seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardState {
    pub phase: Phase,
    pub step: usize,
    pub stability: Option<f32>,
    pub difficulty: Option<f32>,
    pub due: i64,
    pub last_review: Option<i64>,
}

impl CardState {
    pub fn new(now: i64) -> Self {
        Self {
            phase: Phase::New,
            step: 0,
            stability: None,
            difficulty: None,
            due: now,
            last_review: None,
        }
    }

    /// Probability of recall now; a card never reviewed has none.
    pub fn retrievability(&self, now: i64, parameters: &[f32]) -> Option<f32> {
        let state = MemoryState {
            stability: self.stability?,
            difficulty: self.difficulty?,
        };
        let elapsed = (now - self.last_review?).max(0) as f32 / DAY as f32;
        let decay = parameters
            .get(20)
            .copied()
            .unwrap_or(fsrs::FSRS6_DEFAULT_DECAY);
        Some(fsrs::current_retrievability(state, elapsed, decay))
    }
}

pub fn next_state(
    card: &CardState,
    grade: Grade,
    now: i64,
    parameters: &[f32],
) -> Result<CardState, fsrs::FSRSError> {
    let fsrs = FSRS::new(parameters)?;
    let memory = match (card.stability, card.difficulty) {
        (Some(stability), Some(difficulty)) => Some(MemoryState {
            stability,
            difficulty,
        }),
        _ => None,
    };
    let days_elapsed = card
        .last_review
        .map_or(0, |at| ((now - at).max(0) / DAY) as u32);
    let next = fsrs.next_states(memory, DESIRED_RETENTION, days_elapsed)?;
    let item = match grade {
        Grade::Again => next.again,
        Grade::Hard => next.hard,
        Grade::Good => next.good,
    };
    let interval_days = (item.interval.round() as i64).clamp(1, MAX_INTERVAL_DAYS);
    let review = (Phase::Review, 0, now + interval_days * DAY);
    let (phase, step, due) = match card.phase {
        Phase::New | Phase::Learning => {
            let step = if card.phase == Phase::New {
                0
            } else {
                card.step
            };
            stepped(&LEARNING_STEPS, Phase::Learning, step, grade, now).unwrap_or(review)
        }
        Phase::Relearning => {
            stepped(&RELEARNING_STEPS, Phase::Relearning, card.step, grade, now).unwrap_or(review)
        }
        Phase::Review => match grade {
            Grade::Again => (Phase::Relearning, 0, now + RELEARNING_STEPS[0]),
            Grade::Hard | Grade::Good => review,
        },
    };
    Ok(CardState {
        phase,
        step,
        stability: Some(item.memory.stability),
        difficulty: Some(item.memory.difficulty),
        due,
        last_review: Some(now),
    })
}

/// A step inside learning or relearning, or `None` when the card graduates.
fn stepped(
    steps: &[i64],
    phase: Phase,
    step: usize,
    grade: Grade,
    now: i64,
) -> Option<(Phase, usize, i64)> {
    let step = step.min(steps.len() - 1);
    match grade {
        Grade::Again => Some((phase, 0, now + steps[0])),
        Grade::Hard => {
            let wait = match (step, steps.len()) {
                (0, 1) => steps[0] * 3 / 2,
                (0, _) => (steps[0] + steps[1]) / 2,
                _ => steps[step],
            };
            Some((phase, step, now + wait))
        }
        Grade::Good if step + 1 < steps.len() => Some((phase, step + 1, now + steps[step + 1])),
        Grade::Good => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fsrs::DEFAULT_PARAMETERS;

    const T0: i64 = 1_800_000_000;

    fn run(grades: &[(i64, Grade)]) -> CardState {
        let mut card = CardState::new(T0);
        for (at, grade) in grades {
            card = next_state(&card, *grade, *at, &DEFAULT_PARAMETERS).unwrap();
        }
        card
    }

    #[test]
    fn a_new_unit_walks_the_learning_steps_then_graduates() {
        let first = run(&[(T0, Grade::Good)]);
        assert_eq!(
            (first.phase, first.step, first.due),
            (Phase::Learning, 1, T0 + 600)
        );
        let graduated = run(&[(T0, Grade::Good), (T0 + 600, Grade::Good)]);
        assert_eq!(graduated.phase, Phase::Review);
        assert!(graduated.due >= T0 + 600 + DAY);
    }

    #[test]
    fn again_restarts_learning_and_hard_waits_between_the_steps() {
        let again = run(&[(T0, Grade::Again)]);
        assert_eq!((again.phase, again.due), (Phase::Learning, T0 + 60));
        let hard = run(&[(T0, Grade::Hard)]);
        assert_eq!(hard.due, T0 + 330);
    }

    #[test]
    fn a_lapse_goes_to_relearning_for_ten_minutes() {
        let graduated = run(&[(T0, Grade::Good), (T0 + 600, Grade::Good)]);
        let lapse =
            next_state(&graduated, Grade::Again, graduated.due, &DEFAULT_PARAMETERS).unwrap();
        assert_eq!(
            (lapse.phase, lapse.due),
            (Phase::Relearning, graduated.due + 600)
        );
        assert!(lapse.stability.unwrap() < graduated.stability.unwrap());
        let back = next_state(&lapse, Grade::Good, lapse.due, &DEFAULT_PARAMETERS).unwrap();
        assert_eq!(back.phase, Phase::Review);
    }

    #[test]
    fn successful_reviews_grow_the_interval() {
        let mut card = run(&[(T0, Grade::Good), (T0 + 600, Grade::Good)]);
        let mut previous = card.due - card.last_review.unwrap();
        for _ in 0..3 {
            card = next_state(&card, Grade::Good, card.due, &DEFAULT_PARAMETERS).unwrap();
            let interval = card.due - card.last_review.unwrap();
            assert!(interval > previous, "{interval} <= {previous}");
            previous = interval;
        }
    }

    #[test]
    fn retrievability_falls_with_time() {
        let card = run(&[(T0, Grade::Good), (T0 + 600, Grade::Good)]);
        let soon = card.retrievability(T0 + DAY, &DEFAULT_PARAMETERS).unwrap();
        let later = card
            .retrievability(T0 + 30 * DAY, &DEFAULT_PARAMETERS)
            .unwrap();
        assert!(soon > later);
        assert_eq!(
            CardState::new(T0).retrievability(T0, &DEFAULT_PARAMETERS),
            None
        );
    }
}
