//! Ctrl+Alt+P — гипотеза (F-13, F-14, F-85; B21, B32). The reader's guess is
//! written down before it is checked; the answer is short and corrective, with
//! a verbatim quote. A vague guess gets one «смелее». A refuted one becomes a
//! unit in FSRS that asks how it really is (S4 rule 9). The verdict is
//! bookkeeping for the code, never a grade of the reader (INV-10).

use std::path::Path;

use cognitive_exoskeleton_ai::{AiError, Client, Priority, PromptStore, Request, Transport};
use cognitive_exoskeleton_understand::verbatim;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

pub const CHECK_PROMPT: &str = "NEW_hypothesis_check";
pub const NODES_PROMPT: &str = "nodes_system";
pub const REFRAME_PROMPT: &str = "reframe_system";

/// The subject used until Alt+S goals exist for the material.
pub const DEFAULT_SUBJECT: &str = "general";

#[derive(Debug, thiserror::Error)]
pub enum HypothesisError {
    #[error(transparent)]
    Ai(#[from] AiError),
    #[error("prompt: {0}")]
    Prompt(String),
    #[error("нет текста: выдели абзац")]
    NoMaterial,
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("units: {0}")]
    Units(#[from] cognitive_exoskeleton_units::StoreError),
    #[error("memory: {0}")]
    Memory(#[from] cognitive_exoskeleton_memory::MemoryError),
}

impl HypothesisError {
    pub fn user_message(&self, provider: &str) -> String {
        match self {
            HypothesisError::Ai(error) => error.user_message(provider),
            HypothesisError::NoMaterial => "Нет текста: выдели абзац.".into(),
            other => other.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Match,
    Partly,
    Miss,
    NotInText,
    Broken,
}

impl Verdict {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "match" => Verdict::Match,
            "partly" => Verdict::Partly,
            "miss" => Verdict::Miss,
            "not_in_text" => Verdict::NotInText,
            "broken" => Verdict::Broken,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Match => "match",
            Verdict::Partly => "partly",
            Verdict::Miss => "miss",
            Verdict::NotInText => "not_in_text",
            Verdict::Broken => "broken",
        }
    }

    /// About the guess, not the person (B21).
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Match => "Подтверждается",
            Verdict::Partly => "Подтверждается частично",
            Verdict::Miss => "Не подтверждается",
            Verdict::NotInText => "В тексте этого нет",
            Verdict::Broken => "Текст не читается — сверить нельзя",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Verdict::Match => "✓",
            Verdict::Partly => "≈",
            Verdict::Miss => "✗",
            Verdict::NotInText | Verdict::Broken => "?",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub vague: bool,
    pub verdict: Verdict,
    pub reply: String,
    /// Verbatim from the text, or nothing.
    pub quote: Option<String>,
}

/// F-14: what the window shows instead of an answer, once.
pub const BOLDER: &str = "Смелее: назови, что именно с чем связано и как. \
Догадка может оказаться неверной — так и узнаем.";

/// Answers meant for reading get the formula rule (B53); the nodes are words.
fn system(prompts: &PromptStore, key: &str) -> Result<String, HypothesisError> {
    let prompt = if key == NODES_PROMPT {
        prompts.get(key).map(str::to_string)
    } else {
        prompts.with_formulas(key)
    };
    prompt.map_err(|e| HypothesisError::Prompt(e.to_string()))
}

fn json_request(system: &str, input: String) -> Request {
    Request {
        system: system.to_string(),
        input,
        max_tokens: 700,
        json: true,
        priority: Priority::Foreground,
    }
}

pub fn check_shape(value: &Value) -> Result<(), String> {
    if !value["vague"].is_boolean() {
        return Err("vague: true или false".into());
    }
    if value["verdict"].as_str().and_then(Verdict::parse).is_none() {
        return Err("verdict: match|partly|miss|not_in_text|broken".into());
    }
    if value["reply"].as_str().unwrap_or("").trim().is_empty() {
        return Err("reply пусто".into());
    }
    Ok(())
}

pub fn check<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    hypothesis: &str,
    material: &str,
    insist: bool,
) -> Result<Check, HypothesisError> {
    let material = material.trim();
    if material.is_empty() {
        return Err(HypothesisError::NoMaterial);
    }
    let input = format!(
        "<USER_HYPOTHESIS>\n{}\n</USER_HYPOTHESIS>\n<SOURCE_BUFFER>\n{material}\n</SOURCE_BUFFER>\n<INSIST>\n{}\n</INSIST>",
        hypothesis.trim(),
        if insist { "yes" } else { "no" }
    );
    let value = client.complete_json(
        &json_request(&system(prompts, CHECK_PROMPT)?, input),
        check_shape,
    )?;
    Ok(Check {
        vague: value["vague"].as_bool().unwrap_or(false),
        verdict: value["verdict"]
            .as_str()
            .and_then(Verdict::parse)
            .unwrap_or(Verdict::Broken),
        reply: value["reply"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
        quote: value["quote"].as_str().and_then(|q| verbatim(q, material)),
    })
}

/// [Подробнее] (B21): where the guess parts from the text, and nodes nearby.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Nodes {
    pub gap_quote: Option<String>,
    pub words: Vec<String>,
}

fn nodes_shape(value: &Value) -> Result<(), String> {
    if !value["words"].is_array() {
        return Err("words — массив".into());
    }
    Ok(())
}

pub fn nodes<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    hypothesis: &str,
    material: &str,
    used: &[String],
) -> Result<Nodes, HypothesisError> {
    let input = format!(
        "<HYPOTHESIS>\n{}\n</HYPOTHESIS>\n<SELECTED>\n{}\n</SELECTED>\n<USED>\n{}\n</USED>\n<MINE>\n</MINE>",
        hypothesis.trim(),
        material.trim(),
        used.join(", ")
    );
    let value = client.complete_json(
        &json_request(&system(prompts, NODES_PROMPT)?, input),
        nodes_shape,
    )?;
    let words = value["words"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| w.as_str())
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty() && !used.contains(w))
        .take(4)
        .collect();
    Ok(Nodes {
        gap_quote: value["gap_quote"]
            .as_str()
            .and_then(|q| verbatim(q, material)),
        words,
    })
}

/// «Другой ракурс» (F-85): one other organising principle, or an honest no.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reframe {
    pub frame: String,
    pub implication: String,
    pub basis: Option<String>,
}

fn reframe_shape(value: &Value) -> Result<(), String> {
    match value["status"].as_str() {
        Some("proposal") if !value["frame"].as_str().unwrap_or("").trim().is_empty() => Ok(()),
        Some("insufficient") => Ok(()),
        _ => Err("status: proposal (с frame) или insufficient".into()),
    }
}

pub fn reframe<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    material: &str,
    hypothesis: &str,
) -> Result<Option<Reframe>, HypothesisError> {
    let material = material.trim();
    if material.is_empty() {
        return Err(HypothesisError::NoMaterial);
    }
    let input = serde_json::json!({
        "material": material,
        "focus": "",
        "current_frame": hypothesis.trim(),
    })
    .to_string();
    let value = client.complete_json(
        &json_request(&system(prompts, REFRAME_PROMPT)?, input),
        reframe_shape,
    )?;
    if value["status"] != "proposal" {
        return Ok(None);
    }
    let field = |n: &str| value[n].as_str().unwrap_or_default().trim().to_string();
    Ok(Some(Reframe {
        frame: field("frame"),
        implication: field("implication"),
        basis: value["basis"].as_str().and_then(|b| verbatim(b, material)),
    }))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hypothesis {
    pub id: i64,
    pub at: i64,
    pub text: String,
    pub material: String,
    pub verdict: Option<Verdict>,
    pub reply: Option<String>,
    pub quote: Option<String>,
    pub unit: Option<String>,
}

/// The hypotheses table: written before the check (F-13), so a failed check
/// never loses the reader's words.
pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HypothesisError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, HypothesisError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, HypothesisError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS hypotheses (
                 hypothesis_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 at            INTEGER NOT NULL,
                 text          TEXT NOT NULL,
                 material      TEXT NOT NULL,
                 verdict       TEXT,
                 reply         TEXT,
                 quote         TEXT,
                 unit_id       TEXT
             );",
        )?;
        Ok(Self { connection })
    }

    /// An empty guess is a cancel: nothing is written.
    pub fn add(
        &mut self,
        text: &str,
        material: &str,
        now: i64,
    ) -> Result<Option<i64>, HypothesisError> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        self.connection.execute(
            "INSERT INTO hypotheses (at, text, material) VALUES (?1, ?2, ?3)",
            params![now, text, material],
        )?;
        Ok(Some(self.connection.last_insert_rowid()))
    }

    /// A «смелее» rewrite replaces the vague wording of the same hypothesis.
    pub fn reword(&mut self, id: i64, text: &str) -> Result<(), HypothesisError> {
        self.connection.execute(
            "UPDATE hypotheses SET text = ?2 WHERE hypothesis_id = ?1",
            params![id, text.trim()],
        )?;
        Ok(())
    }

    pub fn set_check(&mut self, id: i64, check: &Check) -> Result<(), HypothesisError> {
        self.connection.execute(
            "UPDATE hypotheses SET verdict = ?2, reply = ?3, quote = ?4 WHERE hypothesis_id = ?1",
            params![id, check.verdict.as_str(), check.reply, check.quote],
        )?;
        Ok(())
    }

    pub fn set_unit(&mut self, id: i64, unit: &str) -> Result<(), HypothesisError> {
        self.connection.execute(
            "UPDATE hypotheses SET unit_id = ?2 WHERE hypothesis_id = ?1",
            params![id, unit],
        )?;
        Ok(())
    }

    pub fn get(&self, id: i64) -> Result<Option<Hypothesis>, HypothesisError> {
        Ok(self
            .connection
            .query_row(&format!("{SELECT} WHERE hypothesis_id = ?1"), [id], row)
            .optional()?)
    }

    /// Newest first (F-16 history, the ladder's KNOWN).
    pub fn recent(&self, limit: usize) -> Result<Vec<Hypothesis>, HypothesisError> {
        let mut statement = self
            .connection
            .prepare(&format!("{SELECT} ORDER BY hypothesis_id DESC LIMIT ?1"))?;
        let rows = statement.query_map([limit as i64], row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The guesses written in `[from, to]`, oldest first (`lesson export`, B77).
    pub fn between(&self, from: i64, to: i64) -> Result<Vec<Hypothesis>, HypothesisError> {
        let mut statement = self.connection.prepare(&format!(
            "{SELECT} WHERE at BETWEEN ?1 AND ?2 ORDER BY at, hypothesis_id"
        ))?;
        let rows = statement.query_map([from, to], row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

const SELECT: &str =
    "SELECT hypothesis_id, at, text, material, verdict, reply, quote, unit_id FROM hypotheses";

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Hypothesis> {
    Ok(Hypothesis {
        id: row.get(0)?,
        at: row.get(1)?,
        text: row.get(2)?,
        material: row.get(3)?,
        verdict: row
            .get::<_, Option<String>>(4)?
            .as_deref()
            .and_then(Verdict::parse),
        reply: row.get(5)?,
        quote: row.get(6)?,
        unit: row.get(7)?,
    })
}

/// The ladder's marks for a hypothesis (F-09: [верно] [частично] [ошибка] [мысль]).
pub fn known_mark(verdict: Option<Verdict>) -> &'static str {
    match verdict {
        Some(Verdict::Match) => "верно",
        Some(Verdict::Partly) => "частично",
        Some(Verdict::Miss) => "ошибка",
        _ => "мысль",
    }
}

/// S4 rule 9: a refuted hypothesis becomes a unit whose card asks how it
/// really is. `events` are the journal sequences of the guess and the answer;
/// the old guess is shown only after the reader answers (the card's reference
/// quotes the answer, not the guess).
pub fn enroll_refuted(
    db: impl AsRef<Path>,
    hypothesis: &Hypothesis,
    events: &[i64],
    now: i64,
) -> Result<Option<String>, HypothesisError> {
    if hypothesis.verdict != Some(Verdict::Miss) || events.is_empty() || hypothesis.unit.is_some() {
        return Ok(hypothesis.unit.clone());
    }
    let short: String = hypothesis.text.chars().take(80).collect();
    let mut units = cognitive_exoskeleton_units::UnitStore::open(&db)?;
    let unit = units.add_unit(
        DEFAULT_SUBJECT,
        &format!("Как на самом деле: {short}"),
        "сказать, как на самом деле, вместо опровергнутой догадки",
        "recall",
        events,
    )?;
    cognitive_exoskeleton_memory::Memory::open(&db)?.enroll(&unit, DEFAULT_SUBJECT, now)?;
    Store::open(&db)?.set_unit(hypothesis.id, &unit)?;
    Ok(Some(unit))
}

#[cfg(test)]
mod tests;
