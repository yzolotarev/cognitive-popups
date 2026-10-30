//! B78 block trial. Independent of the verified-error MRT tables.
use super::*;
use cognitive_exoskeleton_ai::{Client, Priority, PromptStore, Request, Transport};
use cognitive_exoskeleton_memory::Memory;
use serde_json::{Value, json};

pub const AA_FIRST: i64 = 12;
pub const POLICY: &str = "block-v1";
pub const DAY: i64 = 86_400;

pub(crate) fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS block_assignments (
        id TEXT PRIMARY KEY, experiment TEXT NOT NULL, policy TEXT NOT NULL,
        lesson TEXT NOT NULL, subject TEXT, material TEXT NOT NULL, block TEXT NOT NULL,
        role TEXT NOT NULL, allowed TEXT NOT NULL, probabilities TEXT NOT NULL,
        arm TEXT NOT NULL, probability REAL NOT NULL, assigned_at INTEGER NOT NULL,
        shown_at INTEGER, skipped_at INTEGER, completed_at INTEGER, completed_operation TEXT, card_status TEXT NOT NULL DEFAULT 'pending',
        card_reason TEXT, question TEXT, reference TEXT,
        UNIQUE(lesson, material, block));
        CREATE TABLE IF NOT EXISTS block_assignment_checks (
        lesson TEXT NOT NULL, material TEXT NOT NULL, reason TEXT NOT NULL, at INTEGER NOT NULL,
        PRIMARY KEY(lesson, material));
        CREATE TABLE IF NOT EXISTS block_attempts (
        operation TEXT PRIMARY KEY, assignment TEXT NOT NULL, attempt TEXT NOT NULL,
        revealed_at INTEGER NOT NULL);
        CREATE TRIGGER IF NOT EXISTS block_assignment_completion AFTER INSERT ON material_interactions
        BEGIN
          UPDATE block_assignments SET completed_at=NEW.at,completed_operation=NEW.operation_id
          WHERE lesson=NEW.lesson_id AND material=NEW.material_id AND block=NEW.block
          AND assigned_at<NEW.at AND (completed_at IS NULL OR NEW.at<completed_at)
          AND ((arm='suggest_alt_t' AND NEW.window='alt_t') OR (arm='suggest_alt_w' AND NEW.window='alt_w'));
        END;",
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct Assignment {
    pub id: String,
    pub lesson: String,
    pub subject: Option<String>,
    pub material: String,
    pub block: String,
    pub role: String,
    pub arm: String,
    pub probability: f64,
    pub assigned_at: i64,
    pub shown_at: Option<i64>,
    pub skipped_at: Option<i64>,
    pub card_status: String,
    pub card_reason: Option<String>,
    pub question: Option<String>,
    pub reference: Option<String>,
    pub allowed: Vec<String>,
    pub probabilities: Vec<f64>,
}
fn json_column<T: serde::de::DeserializeOwned>(
    r: &rusqlite::Row<'_>,
    col: usize,
) -> rusqlite::Result<T> {
    serde_json::from_str(&r.get::<_, String>(col)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, Box::new(e))
    })
}

impl Assignment {
    pub fn function(&self) -> Option<&'static str> {
        match self.arm.as_str() {
            "suggest_alt_t" => Some(window::ALT_T),
            "suggest_alt_w" => Some(window::ALT_W),
            _ => None,
        }
    }
}

impl Materials {
    pub fn experiments_consented(&self) -> Result<bool, MaterialError> {
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='nudge_consent')",
            [],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(false);
        }
        Ok(self
            .connection
            .query_row(
                "SELECT granted FROM nudge_consent WHERE kind='experiments'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Uniform rolls are injected; assignments and eligibility are committed once.
    pub fn assign(
        &mut self,
        lesson: &str,
        subject: Option<&str>,
        material: &Material,
        now: i64,
        mut roll: impl FnMut() -> f64,
    ) -> Result<(), MaterialError> {
        if !self.experiments_consented()? {
            return Ok(());
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let done: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM block_assignment_checks WHERE lesson=?1 AND material=?2)",
            params![lesson, material.id],
            |r| r.get(0),
        )?;
        if done {
            return Ok(());
        }
        // Cap the entire lesson, including replacement materials, at two cards.
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM block_assignments WHERE lesson=?1",
            [lesson],
            |r| r.get(0),
        )?;
        let candidates: Vec<_> = material
            .blocks
            .iter()
            .filter(|b| matches!(b.role.as_str(), "механизм" | "пример"))
            .take((2 - count).max(0) as usize)
            .collect();
        tx.execute(
            "INSERT INTO block_assignment_checks VALUES (?1,?2,?3,?4)",
            params![
                lesson,
                material.id,
                if candidates.is_empty() {
                    if count >= 2 {
                        "lesson_limit"
                    } else {
                        "no_eligible_roles"
                    }
                } else {
                    "assigned"
                },
                now
            ],
        )?;
        let total: i64 =
            tx.query_row("SELECT COUNT(*) FROM block_assignments", [], |r| r.get(0))?;
        for (offset, b) in candidates.into_iter().enumerate() {
            let r = roll();
            assert!((0.0..1.0).contains(&r), "uniform roll must be in [0,1)");
            let (arms, probabilities, arm, p) = if total + (offset as i64) < AA_FIRST {
                (
                    "[\"aa_first\",\"aa_second\"]",
                    "[0.5,0.5]",
                    if r < 0.5 { "aa_first" } else { "aa_second" },
                    0.5,
                )
            } else {
                (
                    "[\"suggest_alt_t\",\"suggest_alt_w\",\"silence\"]",
                    "[0.3333333333333333,0.3333333333333333,0.3333333333333333]",
                    if r < 1.0 / 3.0 {
                        "suggest_alt_t"
                    } else if r < 2.0 / 3.0 {
                        "suggest_alt_w"
                    } else {
                        "silence"
                    },
                    1.0 / 3.0,
                )
            };
            tx.execute("INSERT INTO block_assignments (id,experiment,policy,lesson,subject,material,block,role,allowed,probabilities,arm,probability,assigned_at) VALUES (?1,'fragment_function',?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![format!("block:{lesson}:{}:{}",material.id,b.block),POLICY,lesson,subject,material.id,b.block,b.role,arms,probabilities,arm,p,now])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn assignments(&self) -> Result<Vec<Assignment>, MaterialError> {
        let mut s = self.connection.prepare("SELECT id,lesson,subject,material,block,role,arm,probability,assigned_at,shown_at,skipped_at,card_status,card_reason,question,reference,allowed,probabilities FROM block_assignments ORDER BY assigned_at,rowid")?;
        Ok(s.query_map([], |r| {
            Ok(Assignment {
                id: r.get(0)?,
                lesson: r.get(1)?,
                subject: r.get(2)?,
                material: r.get(3)?,
                block: r.get(4)?,
                role: r.get(5)?,
                arm: r.get(6)?,
                probability: r.get(7)?,
                assigned_at: r.get(8)?,
                shown_at: r.get(9)?,
                skipped_at: r.get(10)?,
                card_status: r.get(11)?,
                card_reason: r.get(12)?,
                question: r.get(13)?,
                reference: r.get(14)?,
                allowed: json_column(r, 15)?,
                probabilities: json_column(r, 16)?,
            })
        })?
        .collect::<Result<_, _>>()?)
    }

    pub fn completion(&self, a: &Assignment) -> Result<Option<Interaction>, MaterialError> {
        let Some(function) = a.function() else {
            return Ok(None);
        };
        Ok(self.interactions(&a.lesson)?.into_iter().find(|i| {
            i.material.as_deref() == Some(&a.material)
                && i.block.as_deref() == Some(&a.block)
                && i.window == function
                && i.at > a.assigned_at
        }))
    }

    pub fn hint(&self, lesson: &str) -> Result<Option<Assignment>, MaterialError> {
        if !self.experiments_consented()? {
            return Ok(None);
        }
        for a in self.assignments()? {
            if a.lesson == lesson
                && a.function().is_some()
                && a.skipped_at.is_none()
                && self.completion(&a)?.is_none()
            {
                return Ok(Some(a));
            }
        }
        Ok(None)
    }
    pub fn mark_shown(&self, id: &str, now: i64) -> Result<(), MaterialError> {
        if self.experiments_consented()? {
            self.connection.execute("UPDATE block_assignments SET shown_at=COALESCE(shown_at,?2) WHERE id=?1 AND shown_at IS NULL AND arm IN ('suggest_alt_t','suggest_alt_w')", params![id,now])?;
        }
        Ok(())
    }
    pub fn skip(&self, id: &str, now: i64) -> Result<(), MaterialError> {
        self.connection.execute("UPDATE block_assignments SET skipped_at=COALESCE(skipped_at,?2) WHERE id=?1 AND shown_at IS NOT NULL", params![id,now])?;
        Ok(())
    }
    pub fn card_failed(&self, id: &str, reason: &str) -> Result<(), MaterialError> {
        self.connection.execute("UPDATE block_assignments SET card_status='card_failed',card_reason=?2 WHERE id=?1 AND card_status!='created'",params![id,reason])?;
        Ok(())
    }
    /// Strict, case-sensitive verbatim check against the registered full block.
    pub fn accept_card(
        &self,
        id: &str,
        question: &str,
        reference: &str,
    ) -> Result<bool, MaterialError> {
        let a = self
            .assignments()?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| MaterialError::NotFound(id.into()))?;
        if a.card_status == "created" {
            return Ok(true);
        }
        let material = self.material(&a.material)?;
        let text = material
            .as_ref()
            .and_then(|m| m.block(&a.block))
            .map(|b| b.text.as_str());
        let reason = if text.is_none_or(|t| t.is_empty()) {
            Some("missing_full_block")
        } else if question.trim().is_empty() || reference.trim().is_empty() {
            Some("empty_card")
        } else if !text.unwrap().contains(reference) {
            Some("quote_not_verbatim")
        } else if question.contains(reference) {
            Some("question_reveals_reference")
        } else {
            None
        };
        if let Some(reason) = reason {
            self.card_failed(id, reason)?;
            return Ok(false);
        }
        self.connection.execute("UPDATE block_assignments SET card_status='created',card_reason=NULL,question=?2,reference=?3 WHERE id=?1",params![id,question,reference])?;
        Ok(true)
    }

    pub fn generate_cards<T: Transport>(
        &self,
        db: &Path,
        client: &Client<T>,
        prompts: &PromptStore,
    ) -> Result<(), String> {
        if !self.experiments_consented().map_err(|e| e.to_string())? {
            return Ok(());
        }
        let mut memory = Memory::open(db).map_err(|e| e.to_string())?;
        for a in self.assignments().map_err(|e| e.to_string())? {
            if !self.experiments_consented().map_err(|e| e.to_string())? {
                return Ok(());
            }
            if a.card_status == "pending" {
                let result = (|| -> Result<(), String> {
                    let material = self
                        .material(&a.material)
                        .map_err(|e| e.to_string())?
                        .ok_or("missing_full_material")?;
                    let block = material.block(&a.block).ok_or("missing_full_block")?;
                    let request = Request {
                        system: prompts
                            .get("NEW_block_card")
                            .map_err(|e| e.to_string())?
                            .to_string(),
                        input: block.text.clone(),
                        max_tokens: 700,
                        json: true,
                        priority: Priority::Background,
                    };
                    let raw = client
                        .complete_json(&request, |v| {
                            if v["question"].is_string() && v["reference"].is_string() {
                                Ok(())
                            } else {
                                Err("question/reference strings required".into())
                            }
                        })
                        .map_err(|e| e.to_string())?;
                    if !self.experiments_consented().map_err(|e| e.to_string())? {
                        return Ok(());
                    }
                    self.accept_card(
                        &a.id,
                        raw["question"].as_str().unwrap(),
                        raw["reference"].as_str().unwrap(),
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(())
                })();
                if !self.experiments_consented().map_err(|e| e.to_string())? {
                    return Ok(());
                }
                if let Err(e) = result {
                    self.card_failed(&a.id, &e).map_err(|e| e.to_string())?;
                }
            }
            // Reconcile after interrupted enrollment without changing the original due.
            let created: bool = self
                .connection
                .query_row(
                    "SELECT card_status='created' FROM block_assignments WHERE id=?1",
                    [&a.id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if created {
                memory
                    .enroll(
                        &a.id,
                        a.subject.as_deref().unwrap_or("unknown"),
                        a.assigned_at + DAY,
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// CLI equivalent of the answer button: only a nonempty own attempt reveals a quote.
    pub fn attempt(
        &self,
        id: &str,
        operation: &str,
        text: &str,
        now: i64,
    ) -> Result<Value, String> {
        let a = self
            .assignments()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("unknown assignment")?;
        if now < a.assigned_at + DAY || a.card_status != "created" {
            return Err("card is not due or unavailable".into());
        }
        if text.trim().is_empty() {
            return Err("own attempt required before reference".into());
        }
        self.connection
            .execute(
                "INSERT OR IGNORE INTO block_attempts VALUES(?1,?2,?3,?4)",
                params![operation, id, text, now],
            )
            .map_err(|e| e.to_string())?;
        let matches: bool = self
            .connection
            .query_row(
                "SELECT assignment=?2 AND attempt=?3 FROM block_attempts WHERE operation=?1",
                params![operation, id, text],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !matches {
            return Err("operation already belongs to another attempt".into());
        }
        Ok(
            json!({"question":a.question,"reference":a.reference,"attempt":text,"grade_source":"self_graded"}),
        )
    }
    pub fn grade_attempt(
        &self,
        db: &Path,
        id: &str,
        operation: &str,
        grade: cognitive_exoskeleton_memory::Grade,
        now: i64,
    ) -> Result<(), String> {
        let revealed:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM block_attempts WHERE operation=?1 AND assignment=?2 AND revealed_at<=?3)",params![operation,id,now],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !revealed {
            return Err("make your attempt and reveal reference first".into());
        }
        Memory::open(db)
            .and_then(|mut m| {
                m.review(
                    id,
                    grade,
                    cognitive_exoskeleton_memory::GradeSource::SelfGraded,
                    &format!("block-answer:{operation}"),
                    now,
                )
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn export(&self) -> Result<Value, MaterialError> {
        let mut rows = Vec::new();
        let mut summary = serde_json::Map::new();
        for arm in [
            "aa_first",
            "aa_second",
            "suggest_alt_t",
            "suggest_alt_w",
            "silence",
        ] {
            summary.insert(arm.into(),json!({"assigned":0,"shown":0,"completed":0,"checked":0,"matched":0,"partial":0,"missed":0,"missing":0}));
        }
        let reviews_exist: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='memory_reviews')",
            [],
            |r| r.get(0),
        )?;
        for a in self.assignments()? {
            let mut reviews = Vec::new();
            if reviews_exist {
                let mut s=self.connection.prepare("SELECT r.at,r.rating,r.source,
                    (SELECT b.revealed_at FROM block_attempts b WHERE b.assignment=r.unit_id
                     AND b.operation=CASE WHEN substr(r.key,1,13)='block-answer:' THEN substr(r.key,14) ELSE r.key END)
                    FROM memory_reviews r WHERE r.unit_id=?1 ORDER BY r.at,r.review_id")?;
                for r in s.query_map([&a.id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                })? {
                    let (at, rating, source, answered_at) = r?;
                    let interval = answered_at
                        .map(|t| json!(t - a.assigned_at))
                        .unwrap_or(json!("unknown"));
                    reviews.push(json!({"at":at,"answered_at":answered_at,"rating":rating,"source":source,"interval_seconds":interval,"graded_interval_seconds":at-a.assigned_at}));
                }
            }
            let completion = self.completion(&a)?;
            let first = reviews.first().cloned().unwrap_or(json!("missing"));
            let s=summary.entry(a.arm.clone()).or_insert(json!({"assigned":0,"shown":0,"completed":0,"checked":0,"matched":0,"partial":0,"missed":0,"missing":0}));
            for (key, yes) in [
                ("assigned", true),
                ("shown", a.shown_at.is_some()),
                ("completed", completion.is_some()),
                ("checked", !reviews.is_empty()),
                ("missing", reviews.is_empty()),
                ("matched", first["rating"] == 3),
                ("partial", first["rating"] == 2),
                ("missed", first["rating"] == 1),
            ] {
                if yes {
                    s[key] = json!(s[key].as_u64().unwrap() + 1);
                }
            }
            rows.push(json!({"assignment":a,"experiment":"fragment_function","policy":POLICY,"completion":completion.map(|i|json!(i)).unwrap_or(json!("unknown")),"first_check":first,"subsequent_reviews":reviews.iter().skip(1).collect::<Vec<_>>(),"reading":"unknown"}));
        }
        let mut s = self.connection.prepare(
            "SELECT lesson,material,reason,at FROM block_assignment_checks ORDER BY at,rowid",
        )?;
        let eligibility=s.query_map([],|r|Ok(json!({"lesson":r.get::<_,String>(0)?,"material":r.get::<_,String>(1)?,"reason":r.get::<_,String>(2)?,"at":r.get::<_,i64>(3)?})))?.collect::<Result<Vec<_>,_>>()?;
        Ok(
            json!({"assignments":rows,"eligibility":eligibility,"summary":summary,"limitations":"n=1; измеряется эффект назначения, а не выполнения; анализ intention-to-treat; чтение без выделения unknown; null = missing/unknown"}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cognitive_exoskeleton_ai::{AiConfig, ProviderKind};
    use cognitive_exoskeleton_memory::{Grade, GradeSource};
    const TEXT: &str = "⟦b1 · механизм⟧\nУмножаем вероятность на убыток.\n⟦b2 · пример⟧\nУмножаем вероятность на убыток.";
    fn consent(m: &Materials, yes: bool) {
        m.connection.execute_batch("CREATE TABLE IF NOT EXISTS nudge_consent(kind TEXT PRIMARY KEY,granted INTEGER,changed_at INTEGER);").unwrap();
        m.connection
            .execute(
                "INSERT OR REPLACE INTO nudge_consent VALUES('experiments',?1,0)",
                [yes],
            )
            .unwrap();
    }
    fn assign(m: &mut Materials, lesson: &str, r: f64) {
        consent(m, false);
        let material = m
            .register(Some(lesson), Some("subject"), TEXT, 100)
            .unwrap()
            .0;
        consent(m, true);
        m.assign(lesson, Some("subject"), &material, 100, || r)
            .unwrap();
    }
    #[test]
    fn fixed_rolls_aa_then_three_arms_and_idempotence() {
        let mut m = Materials::open_in_memory().unwrap();
        for n in 0..6 {
            assign(
                &mut m,
                &format!("aa{n}"),
                if n % 2 == 0 { 0.1 } else { 0.9 },
            );
        }
        for (l, r) in [("t", 0.0), ("w", 0.5), ("s", 0.99)] {
            assign(&mut m, l, r);
        }
        let rows = m.assignments().unwrap();
        assert_eq!(rows.len(), 18);
        assert_eq!(rows[0].arm, "aa_first");
        assert_eq!(rows[2].arm, "aa_second");
        assert_eq!(rows[12].arm, "suggest_alt_t");
        assert_eq!(rows[14].arm, "suggest_alt_w");
        assert_eq!(rows[16].arm, "silence");
        assert_eq!(rows[12].probability, 1.0 / 3.0);
        assign(&mut m, "t", 0.9);
        assert_eq!(m.assignments().unwrap().len(), 18);
        assert!(m.hint("aa0").unwrap().is_none());
        assert!(m.hint("s").unwrap().is_none());
        assert!(m.hint("t").unwrap().is_some());
        consent(&m, false);
        assert!(m.hint("t").unwrap().is_none());
        m.register(Some("no-consent"), Some("s"), TEXT, 100)
            .unwrap();
        assert_eq!(m.assignments().unwrap().len(), 18);
    }
    #[test]
    fn completion_needs_exact_material_block_function_and_later_time() {
        let mut m = Materials::open_in_memory().unwrap();
        for n in 0..6 {
            assign(&mut m, &format!("aa{n}"), 0.1);
        }
        assign(&mut m, "target", 0.1);
        let a = m.hint("target").unwrap().unwrap();
        for (op, block, function, at) in [
            ("same-time", "b1", "alt_t", 100),
            ("wrong-function", "b1", "alt_w", 101),
            ("wrong-block", "b3", "alt_t", 102),
        ] {
            m.record_selection(
                "target",
                op,
                function,
                &[Part {
                    block: Some(block.into()),
                    text: "text".into(),
                }],
                None,
                at,
            )
            .unwrap();
            assert!(m.completion(&a).unwrap().is_none());
        }
        m.mark_shown(&a.id, 103).unwrap();
        m.mark_shown(&a.id, 104).unwrap();
        assert_eq!(m.assignments().unwrap()[12].shown_at, Some(103));
        m.skip(&a.id, 104).unwrap();
        assert_eq!(m.hint("target").unwrap().unwrap().block, "b2");
        m.record_selection(
            "target",
            "correct",
            "alt_t",
            &[Part {
                block: Some("b1".into()),
                text: "text".into(),
            }],
            None,
            105,
        )
        .unwrap();
        assert_eq!(m.completion(&a).unwrap().unwrap().operation, "correct");
        assert!(m.export().unwrap()["assignments"][12]["completion"].is_object());
    }
    struct Model;
    impl Transport for Model {
        fn post(&self, _: &str, _: &[(&str, String)], _: &Value) -> Result<(u16, String), String> {
            let card = json!({"question":"Как вычислить ожидаемую потерю?","reference":"Умножаем вероятность на убыток."});
            Ok((
                200,
                json!({"candidates":[{"content":{"parts":[{"text":card.to_string()}]}}]})
                    .to_string(),
            ))
        }
    }
    #[test]
    fn cards_in_every_arm_due_after_one_day_actual_interval_and_export() {
        let db = std::env::temp_dir().join(format!("block-trial-{}.db", uuid::Uuid::new_v4()));
        {
            let mut m = Materials::open(&db).unwrap();
            for n in 0..6 {
                assign(
                    &mut m,
                    &format!("aa{n}"),
                    if n % 2 == 0 { 0.1 } else { 0.9 },
                );
            }
            for (l, r) in [("t", 0.1), ("w", 0.5), ("s", 0.9)] {
                assign(&mut m, l, r);
            }
            let client = Client::new(
                AiConfig::new(ProviderKind::Gemini, "test", None).with_key(Some("fixture".into())),
                Model,
            );
            let prompts = PromptStore::from_dir(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/v2/port/prompts"),
            )
            .unwrap();
            m.generate_cards(&db, &client, &prompts).unwrap();
            let mut memory = Memory::open(&db).unwrap();
            let rows = m.assignments().unwrap();
            assert_eq!(rows.len(), 18);
            assert!(rows.iter().all(|a| a.card_status == "created"));
            assert!(memory.due(100 + DAY - 1).unwrap().is_empty());
            assert_eq!(memory.due(100 + DAY).unwrap().len(), 18);
            for a in &rows {
                assert_eq!(memory.card(&a.id).unwrap().due, 100 + DAY);
            }
            assert!(
                memory
                    .review(
                        &rows[0].id,
                        Grade::Good,
                        GradeSource::SelfGraded,
                        "early",
                        100 + DAY - 1
                    )
                    .is_err()
            );
            m.attempt(
                &rows[0].id,
                "attempt",
                "Умножить вероятность на убыток",
                100 + DAY + 321,
            )
            .unwrap();
            m.grade_attempt(&db, &rows[0].id, "attempt", Grade::Hard, 100 + DAY + 900)
                .unwrap();
            memory
                .review(
                    &rows[0].id,
                    Grade::Good,
                    GradeSource::SelfGraded,
                    "repeat",
                    100 + 2 * DAY,
                )
                .unwrap();
            let export = m.export().unwrap();
            assert_eq!(
                export["assignments"][0]["first_check"]["interval_seconds"],
                DAY + 321
            );
            assert_eq!(
                export["assignments"][0]["first_check"]["graded_interval_seconds"],
                DAY + 900
            );
            assert_eq!(
                export["assignments"][0]["subsequent_reviews"][0]["interval_seconds"],
                "unknown"
            );
            assert_eq!(
                export["assignments"][0]["first_check"]["source"],
                "self_graded"
            );
            assert_eq!(
                export["assignments"][0]["subsequent_reviews"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(export["summary"]["aa_first"]["partial"], 1);
            assert_eq!(export["assignments"][1]["first_check"], "missing");
            // Enrollment and generation retry keep one card per assigned block.
            m.generate_cards(&db, &client, &prompts).unwrap();
            assert_eq!(memory.review_count(&rows[0].id).unwrap(), 2);
        }
        std::fs::remove_file(db).unwrap();
    }
    #[test]
    fn invalid_quote_missing_block_and_no_roles_are_explicit() {
        let mut m = Materials::open_in_memory().unwrap();
        assign(&mut m, "l", 0.1);
        let a = m.assignments().unwrap().remove(0);
        assert!(
            !m.accept_card(&a.id, "Вопрос?", "выдуманная цитата")
                .unwrap()
        );
        assert_eq!(
            m.assignments().unwrap()[0].card_reason.as_deref(),
            Some("quote_not_verbatim")
        );
        m.connection
            .execute(
                "DELETE FROM material_blocks WHERE material_id=?1",
                [&a.material],
            )
            .unwrap();
        assert!(!m.accept_card(&a.id, "Вопрос?", "Умножаем").unwrap());
        assert_eq!(
            m.assignments().unwrap()[0].card_reason.as_deref(),
            Some("missing_full_block")
        );
        m.register(
            Some("no-roles"),
            Some("s"),
            "⟦b1 · проблема⟧\nЧто делать?",
            200,
        )
        .unwrap();
        let export = m.export().unwrap();
        assert!(
            export["eligibility"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["reason"] == "no_eligible_roles")
        );
    }
}
