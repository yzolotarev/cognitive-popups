//! Background mechanisms (`docs/v2/SPEC.md` §5, P8): when the program may
//! speak first, and the micro-randomized trial that tests whether it helps.
//!
//! - No consent, no initiative and no background recording (INV-09).
//! - Opportunity and Daily share one attention budget: nothing while the reader
//!   is busy or watching a video or playing; at most 3 a day, 2 h apart, 24 h of
//!   quiet after «Не сейчас», the same topic not twice in 24 h (Q-56 «б»).
//! - Two kinds of silence are never mixed (B12): a meaningful one (conditions
//!   are wrong) is not randomized; an experimental one is a recorded arm.
//! - MRT (OD-06): decision point — a verified error; action — a short
//!   diagnostic or silence; outcome — an independent verified check of the
//!   same unit 1–7 days later, else `missing`. Engagement is kept apart from
//!   the verified result (B13).

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

pub const DAY: i64 = 86_400;
pub const HOUR: i64 = 3_600;
pub const DAILY_LIMIT: usize = 3;
pub const MIN_GAP: i64 = 2 * HOUR;
pub const SNOOZE: i64 = 24 * HOUR;
pub const TOPIC_REPEAT: i64 = 24 * HOUR;
pub const OUTCOME_FROM: i64 = DAY;
pub const OUTCOME_UNTIL: i64 = 7 * DAY;
pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum NudgeError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("nudges schema {found} is newer than this build ({supported})")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("corrupt row: {0}")]
    Corrupt(String),
}

/// What the reader agreed to, one switch each (INV-09).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Consent {
    /// Opportunity cards: «есть учебная польза сейчас».
    Opportunity,
    /// The daily invitation.
    Daily,
    /// Keystroke rhythm and window classes for «занят / свободен».
    BackgroundRecording,
    /// Taking part in micro-randomized trials.
    Experiments,
}

impl Consent {
    pub fn as_str(self) -> &'static str {
        match self {
            Consent::Opportunity => "opportunity",
            Consent::Daily => "daily",
            Consent::BackgroundRecording => "background_recording",
            Consent::Experiments => "experiments",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "opportunity" => Some(Consent::Opportunity),
            "daily" => Some(Consent::Daily),
            "background_recording" => Some(Consent::BackgroundRecording),
            "experiments" => Some(Consent::Experiments),
            _ => None,
        }
    }
}

/// The two interrupting mechanisms; they share one budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Opportunity,
    Daily,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Opportunity => "opportunity",
            Kind::Daily => "daily",
        }
    }

    fn consent(self) -> Consent {
        match self {
            Kind::Opportunity => Consent::Opportunity,
            Kind::Daily => Consent::Daily,
        }
    }
}

/// v1 `slack.py` states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Slack {
    /// A step is running, a window is open, or a manual request is waiting.
    Busy,
    Trickle,
    Free,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub slack: Slack,
    /// A video or a game is on screen.
    pub media: bool,
}

impl Context {
    pub const FREE: Context = Context {
        slack: Slack::Free,
        media: false,
    };
}

/// Why nothing is shown: a meaningful silence, never randomized (B12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Silence {
    NoConsent,
    Busy,
    Media,
    DailyLimit,
    TooSoon { next_at: i64 },
    Snoozed { until: i64 },
    RepeatTopic { until: i64 },
    AlreadyToday,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Opened,
    /// «Не сейчас»: 24 h of quiet for every interrupting card.
    NotNow,
    Ignored,
}

impl Response {
    fn as_str(self) -> &'static str {
        match self {
            Response::Opened => "opened",
            Response::NotNow => "not_now",
            Response::Ignored => "ignored",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "opened" => Some(Response::Opened),
            "not_now" => Some(Response::NotNow),
            "ignored" => Some(Response::Ignored),
            _ => None,
        }
    }
}

/// The MRT action at a decision point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    Diagnostic,
    Silence,
    /// A/A check: two labels for the same silence, to test the pipeline
    /// before the first real conclusion.
    AaFirst,
    AaSecond,
}

impl Arm {
    fn as_str(self) -> &'static str {
        match self {
            Arm::Diagnostic => "diagnostic",
            Arm::Silence => "silence",
            Arm::AaFirst => "aa_first",
            Arm::AaSecond => "aa_second",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "diagnostic" => Some(Arm::Diagnostic),
            "silence" => Some(Arm::Silence),
            "aa_first" => Some(Arm::AaFirst),
            "aa_second" => Some(Arm::AaSecond),
            _ => None,
        }
    }

    /// Only the diagnostic arm shows anything.
    pub fn shows(self) -> bool {
        self == Arm::Diagnostic
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Decision {
    /// Randomized; the arm says what to do.
    Randomized { id: i64, arm: Arm },
    /// Not eligible: a meaningful silence, recorded but outside the trial.
    NotEligible { id: i64, silence: Silence },
    /// The same decision point was already recorded (INV-04).
    Duplicate,
}

pub struct Nudges {
    connection: Connection,
}

impl Nudges {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, NudgeError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, NudgeError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, NudgeError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let nudges = Self { connection };
        nudges.migrate()?;
        Ok(nudges)
    }

    fn migrate(&self) -> Result<(), NudgeError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS nudge_meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);",
        )?;
        let version: i64 = self
            .connection
            .query_row(
                "SELECT value FROM nudge_meta WHERE key = 'schema'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if version > SCHEMA_VERSION {
            return Err(NudgeError::UnsupportedSchema {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if version < 1 {
            self.connection.execute_batch(
                "
                BEGIN IMMEDIATE;
                CREATE TABLE nudge_consent (
                    kind       TEXT PRIMARY KEY,
                    granted    INTEGER NOT NULL,
                    changed_at INTEGER NOT NULL
                );
                CREATE TABLE nudge_shows (
                    show_id  INTEGER PRIMARY KEY AUTOINCREMENT,
                    kind     TEXT NOT NULL,
                    topic    TEXT NOT NULL,
                    at       INTEGER NOT NULL,
                    response TEXT,
                    answered_at INTEGER
                );
                CREATE TABLE mrt_decisions (
                    decision_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    key         TEXT NOT NULL UNIQUE,
                    unit_id     TEXT NOT NULL,
                    at          INTEGER NOT NULL,
                    eligible    INTEGER NOT NULL,
                    silence     TEXT,
                    arm         TEXT,
                    p_treat     REAL,
                    engaged     INTEGER,
                    outcome     TEXT,
                    outcome_at  INTEGER
                );
                CREATE INDEX mrt_by_unit ON mrt_decisions (unit_id, at);
                INSERT OR REPLACE INTO nudge_meta (key, value) VALUES ('schema', 1);
                COMMIT;
                ",
            )?;
        }
        Ok(())
    }

    pub fn set_consent(
        &mut self,
        kind: Consent,
        granted: bool,
        now: i64,
    ) -> Result<(), NudgeError> {
        self.connection.execute(
            "INSERT INTO nudge_consent (kind, granted, changed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (kind) DO UPDATE SET granted = ?2, changed_at = ?3",
            params![kind.as_str(), granted, now],
        )?;
        Ok(())
    }

    /// Default is «no»: nothing in the background without an explicit «yes».
    pub fn consented(&self, kind: Consent) -> Result<bool, NudgeError> {
        Ok(self
            .connection
            .query_row(
                "SELECT granted FROM nudge_consent WHERE kind = ?1",
                [kind.as_str()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// May the window classes and keystroke rhythm be recorded at all?
    pub fn may_record(&self) -> Result<bool, NudgeError> {
        self.consented(Consent::BackgroundRecording)
    }

    /// The shared attention budget. `day_start` is the local midnight of `now`.
    pub fn check(
        &self,
        kind: Kind,
        topic: &str,
        now: i64,
        day_start: i64,
        context: Context,
    ) -> Result<Option<Silence>, NudgeError> {
        if !self.consented(kind.consent())? {
            return Ok(Some(Silence::NoConsent));
        }
        if context.slack == Slack::Busy {
            return Ok(Some(Silence::Busy));
        }
        if context.media {
            return Ok(Some(Silence::Media));
        }
        let snoozed: Option<i64> = self.connection.query_row(
            "SELECT MAX(answered_at) FROM nudge_shows WHERE response = 'not_now'",
            [],
            |r| r.get(0),
        )?;
        if let Some(at) = snoozed
            && now < at + SNOOZE
        {
            return Ok(Some(Silence::Snoozed { until: at + SNOOZE }));
        }
        if kind == Kind::Daily {
            let shown_today: bool = self.connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM nudge_shows WHERE kind = 'daily' AND at >= ?1)",
                [day_start],
                |r| r.get(0),
            )?;
            if shown_today {
                return Ok(Some(Silence::AlreadyToday));
            }
        }
        let today: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM nudge_shows WHERE at >= ?1",
            [day_start],
            |r| r.get(0),
        )?;
        if today as usize >= DAILY_LIMIT {
            return Ok(Some(Silence::DailyLimit));
        }
        let last: Option<i64> =
            self.connection
                .query_row("SELECT MAX(at) FROM nudge_shows", [], |r| r.get(0))?;
        if let Some(at) = last
            && now < at + MIN_GAP
        {
            return Ok(Some(Silence::TooSoon {
                next_at: at + MIN_GAP,
            }));
        }
        let same_topic: Option<i64> = self.connection.query_row(
            "SELECT MAX(at) FROM nudge_shows WHERE topic = ?1",
            [topic],
            |r| r.get(0),
        )?;
        if let Some(at) = same_topic
            && now < at + TOPIC_REPEAT
        {
            return Ok(Some(Silence::RepeatTopic {
                until: at + TOPIC_REPEAT,
            }));
        }
        Ok(None)
    }

    pub fn record_shown(&mut self, kind: Kind, topic: &str, now: i64) -> Result<i64, NudgeError> {
        self.connection.execute(
            "INSERT INTO nudge_shows (kind, topic, at) VALUES (?1, ?2, ?3)",
            params![kind.as_str(), topic, now],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// The first answer counts.
    pub fn record_response(
        &mut self,
        show: i64,
        response: Response,
        now: i64,
    ) -> Result<bool, NudgeError> {
        Ok(self.connection.execute(
            "UPDATE nudge_shows SET response = ?2, answered_at = ?3
             WHERE show_id = ?1 AND response IS NULL",
            params![show, response.as_str(), now],
        )? == 1)
    }

    /// MRT decision at a verified error (OD-06). `roll` is a uniform number in
    /// [0, 1) from the caller, so tests and the A/A check are reproducible;
    /// `p_treat` is the probability of the diagnostic arm.
    #[allow(clippy::too_many_arguments)]
    pub fn decide(
        &mut self,
        key: &str,
        unit: &str,
        now: i64,
        context: Context,
        roll: f64,
        p_treat: f64,
        aa: bool,
    ) -> Result<Decision, NudgeError> {
        let seen: bool = self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM mrt_decisions WHERE key = ?1)",
            [key],
            |r| r.get(0),
        )?;
        if seen {
            return Ok(Decision::Duplicate);
        }
        let silence = if !self.consented(Consent::Experiments)? {
            Some(Silence::NoConsent)
        } else if context.slack == Slack::Busy {
            Some(Silence::Busy)
        } else if context.media {
            Some(Silence::Media)
        } else {
            None
        };
        if let Some(silence) = silence {
            self.connection.execute(
                "INSERT INTO mrt_decisions (key, unit_id, at, eligible, silence)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![key, unit, now, silence_name(&silence)],
            )?;
            return Ok(Decision::NotEligible {
                id: self.connection.last_insert_rowid(),
                silence,
            });
        }
        let arm = match (aa, roll < p_treat) {
            (false, true) => Arm::Diagnostic,
            (false, false) => Arm::Silence,
            (true, true) => Arm::AaFirst,
            (true, false) => Arm::AaSecond,
        };
        self.connection.execute(
            "INSERT INTO mrt_decisions (key, unit_id, at, eligible, arm, p_treat)
             VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            params![key, unit, now, arm.as_str(), p_treat],
        )?;
        Ok(Decision::Randomized {
            id: self.connection.last_insert_rowid(),
            arm,
        })
    }

    /// Engagement — the reader opened the diagnostic. Kept apart from the
    /// verified result (B13).
    pub fn record_engaged(&mut self, decision: i64, engaged: bool) -> Result<(), NudgeError> {
        self.connection.execute(
            "UPDATE mrt_decisions SET engaged = ?2 WHERE decision_id = ?1 AND engaged IS NULL",
            params![decision, engaged],
        )?;
        Ok(())
    }

    /// An independent verified check of a unit: the outcome of every open
    /// randomized decision on it made 1–7 days earlier. Returns how many.
    pub fn record_check(
        &mut self,
        unit: &str,
        correct: bool,
        now: i64,
    ) -> Result<usize, NudgeError> {
        Ok(self.connection.execute(
            "UPDATE mrt_decisions SET outcome = ?2, outcome_at = ?3
             WHERE unit_id = ?1 AND eligible = 1 AND outcome IS NULL
               AND ?3 >= at + ?4 AND ?3 <= at + ?5",
            params![
                unit,
                if correct { "correct" } else { "incorrect" },
                now,
                OUTCOME_FROM,
                OUTCOME_UNTIL
            ],
        )?)
    }

    /// Randomized decisions with no check within 7 days become `missing`.
    pub fn close_missing(&mut self, now: i64) -> Result<usize, NudgeError> {
        Ok(self.connection.execute(
            "UPDATE mrt_decisions SET outcome = 'missing', outcome_at = ?1
             WHERE eligible = 1 AND outcome IS NULL AND at + ?2 < ?1",
            params![now, OUTCOME_UNTIL],
        )?)
    }

    /// Per arm: decisions, correct, incorrect, missing, engaged. The analysis
    /// itself is `tools/mrt_analysis.py` over the same tables.
    pub fn summary(&self) -> Result<Vec<ArmSummary>, NudgeError> {
        let mut statement = self.connection.prepare(
            "SELECT arm, COUNT(*),
                    SUM(outcome = 'correct'), SUM(outcome = 'incorrect'),
                    SUM(outcome = 'missing'), SUM(engaged = 1)
             FROM mrt_decisions WHERE eligible = 1 GROUP BY arm ORDER BY arm",
        )?;
        let rows = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(arm, n, c, i, m, e)| {
                Ok(ArmSummary {
                    arm: Arm::parse(&arm).ok_or(NudgeError::Corrupt(arm))?,
                    decisions: n as usize,
                    correct: c.unwrap_or(0) as usize,
                    incorrect: i.unwrap_or(0) as usize,
                    missing: m.unwrap_or(0) as usize,
                    engaged: e.unwrap_or(0) as usize,
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArmSummary {
    pub arm: Arm,
    pub decisions: usize,
    pub correct: usize,
    pub incorrect: usize,
    pub missing: usize,
    pub engaged: usize,
}

fn silence_name(silence: &Silence) -> &'static str {
    match silence {
        Silence::NoConsent => "no_consent",
        Silence::Busy => "busy",
        Silence::Media => "media",
        Silence::DailyLimit => "daily_limit",
        Silence::TooSoon { .. } => "too_soon",
        Silence::Snoozed { .. } => "snoozed",
        Silence::RepeatTopic { .. } => "repeat_topic",
        Silence::AlreadyToday => "already_today",
    }
}

/// Card texts inform, they do not order (HIG; B16). Details come after the click.
pub fn card_text(kind: Kind) -> &'static str {
    match kind {
        Kind::Opportunity => "Есть материал, к которому сейчас удобно вернуться",
        Kind::Daily => "Есть план на сегодня",
    }
}

#[cfg(test)]
mod tests;
