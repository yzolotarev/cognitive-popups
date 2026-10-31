//! Versioned learning feed: content relations and support, never mastery levels.
use crate::{
    PracticeError,
    episode::{Assessment, Episode, Task},
};
use cognitive_exoskeleton_ai::{Client, Priority, PromptStore, Request, Transport};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const VERSION: &str = "learning-v1";
pub const GENERATE: &str = "NEW_learning_tasks";
pub const DECOMPOSE: &str = "NEW_learning_support";
pub const VERIFY: &str = "NEW_learning_verify";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Entry,
    Variation,
    LessSupport,
    NewSituation,
    Prepares,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    pub id: String,
    pub skill: String,
    pub action: String,
    pub source_span: String,
    pub requirements: Vec<String>,
    pub relation: Relation,
    pub difference: String,
    #[serde(default)]
    pub warmup: bool,
    pub task: Task,
}
impl Challenge {
    pub fn prompt(&self) -> &str {
        match &self.task {
            Task::Question { question, .. } => question,
            Task::Action { instruction } => instruction,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub components: Vec<String>,
    pub tasks: Vec<Challenge>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub version: String,
    pub try_after_matches: u32,
    pub prepare_after_misses: u32,
    pub matches: u32,
    pub misses: u32,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            version: VERSION.into(),
            try_after_matches: 3,
            prepare_after_misses: 2,
            matches: 0,
            misses: 0,
        }
    }
}
impl Policy {
    pub fn observe(&mut self, grade: Option<Assessment>, helped: bool) {
        if helped || grade.is_none() {
            return;
        }
        match grade.unwrap() {
            Assessment::Matched => {
                self.matches += 1;
                self.misses = 0;
            }
            Assessment::Partial => {
                self.matches = 0;
                self.misses = 0;
            }
            Assessment::Missed => {
                self.matches = 0;
                self.misses += 1;
            }
        }
    }
    pub fn request(&self) -> &'static str {
        if self.misses >= self.prepare_after_misses {
            "prepares"
        } else if self.matches >= self.try_after_matches {
            "less_support"
        } else {
            "variation"
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completed {
    pub root: Challenge,
    pub episode: Episode,
    pub draft: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub root: Challenge,
    pub episode: Episode,
    pub depths: Vec<u8>,
    pub support_count: usize,
    pub queue: Vec<Challenge>,
    pub history: Vec<Challenge>,
    pub policy: Policy,
    pub finished: bool,
    pub serial: u64,
    pub draft: String,
    pub current_depth: u8,
    #[serde(default)]
    pub processed: Vec<String>,
    #[serde(default)]
    pub warmups: usize,
    #[serde(default)]
    pub completed: Vec<Completed>,
    #[serde(default)]
    pub policies: std::collections::BTreeMap<String, Policy>,
    #[serde(default)]
    pub help_pending: bool,
}
impl Session {
    pub fn new(root: Challenge, queue: Vec<Challenge>) -> Self {
        Self {
            episode: Episode::new(root.task.clone()),
            root,
            depths: vec![],
            support_count: 0,
            queue,
            history: vec![],
            policy: Policy::default(),
            finished: false,
            serial: 0,
            draft: String::new(),
            current_depth: 0,
            processed: vec![],
            warmups: 0,
            completed: vec![],
            policies: Default::default(),
            help_pending: false,
        }
    }
    fn take_next(&mut self) -> Challenge {
        let wanted = match self.policy.request() {
            "less_support" => Relation::LessSupport,
            "prepares" => Relation::Prepares,
            _ => Relation::Variation,
        };
        let index = self
            .queue
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| {
                let repeated = self.history.iter().rev().take(3).count() == 3
                    && self.history.iter().rev().take(3).all(|h| {
                        h.skill == c.skill
                            && h.action == c.action
                            && h.requirements == c.requirements
                    });
                (repeated, c.relation != wanted, c.skill != self.root.skill)
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.queue.remove(index)
    }
    pub fn support(&mut self, batch: Batch) -> Result<(), String> {
        if self.current_depth >= 2 || self.support_count + batch.tasks.len() > 10 {
            return Err("Достигнут предел помощи. Нужен короткий пример или возврат.".into());
        }
        if batch.tasks.is_empty() {
            return Err("Нет доступных подготовительных шагов".into());
        }
        let depth = self.current_depth + 1;
        let tasks: Vec<Task> = batch.tasks.iter().map(|c| c.task.clone()).collect();
        if self.episode.preparation.is_empty()
            || self.episode.position >= self.episode.preparation.len()
        {
            self.episode.support(tasks).map_err(str::to_string)?;
            self.depths = vec![depth; batch.tasks.len()];
        } else {
            let at = self.episode.position;
            // Insert children before the stuck parent: it returns after them.
            self.episode.preparation.splice(at..at, tasks);
            self.depths
                .splice(at..at, std::iter::repeat_n(depth, batch.tasks.len()));
            self.episode.helped = true;
            self.episode.revealed = false;
        }
        self.help_pending = false;
        self.support_count += batch.tasks.len();
        self.current_depth = depth;
        self.history.extend(batch.tasks);
        self.draft.clear();
        self.serial += 1;
        Ok(())
    }
    pub fn return_to_root(&mut self) {
        self.help_pending = false;
        self.episode.position = self.episode.preparation.len();
        self.episode.revealed = false;
        self.current_depth = 0;
        self.draft.clear();
        self.serial += 1;
    }
    pub fn advance(&mut self, key: &str, grade: Option<Assessment>) -> Result<bool, String> {
        if self.processed.iter().any(|old| old == key) {
            return Ok(false);
        }
        if self.finished {
            return Err("Эпизод завершён".into());
        }
        let draft = self.draft.clone();
        let root = self.episode.position >= self.episode.preparation.len();
        if !self.episode.advance(key, grade).map_err(str::to_string)? {
            return Ok(false);
        }
        self.help_pending = false;
        self.processed.push(key.into());
        self.serial += 1;
        self.draft.clear();
        if root {
            self.completed.push(Completed {
                root: self.root.clone(),
                episode: self.episode.clone(),
                draft,
            });
            let policy = self.policies.entry(self.root.skill.clone()).or_default();
            policy.observe(grade, self.episode.helped);
            self.policy = policy.clone();
            if self.root.warmup {
                self.warmups += 1;
            }
            self.history.push(self.root.clone());
            if self.warmups >= 5 {
                self.queue.retain(|c| !c.warmup);
            }
            if self.queue.is_empty() {
                self.finished = true;
            } else {
                self.root = self.take_next();
                self.policy = self
                    .policies
                    .get(&self.root.skill)
                    .cloned()
                    .unwrap_or_default();
                self.episode = Episode::new(self.root.task.clone());
                self.depths.clear();
                self.support_count = 0;
                self.current_depth = 0;
            }
        } else {
            self.current_depth = self.depths.get(self.episode.position).copied().unwrap_or(0);
        }
        Ok(true)
    }
    pub fn append(&mut self, tasks: Vec<Challenge>) {
        for c in tasks {
            if c.warmup && self.warmups >= 5 {
                continue;
            }
            if !self
                .history
                .iter()
                .chain(&self.queue)
                .chain(std::iter::once(&self.root))
                .any(|p| p.prompt() == c.prompt())
            {
                self.queue.push(c);
            }
        }
        if self.finished && !self.queue.is_empty() {
            self.root = self.take_next();
            self.policy = self
                .policies
                .get(&self.root.skill)
                .cloned()
                .unwrap_or_default();
            self.episode = Episode::new(self.root.task.clone());
            self.finished = false;
            self.depths.clear();
            self.support_count = 0;
            self.current_depth = 0;
        }
    }
}

pub fn validate(batch: &Batch, source: &str, support: bool) -> Result<(), String> {
    if (support && batch.tasks.is_empty()) || batch.tasks.len() > 5 {
        return Err("Нужно 1–5 содержательных заданий".into());
    }
    if !batch.tasks.is_empty() && batch.components.is_empty() {
        return Err("Нет компонентов исходного ответа".into());
    }
    if batch.tasks.iter().filter(|c| c.warmup).count() > 1 {
        return Err("Не больше одной разминки в пакете".into());
    }
    let mut prompts = std::collections::HashSet::new();
    let mut ids = std::collections::HashSet::new();
    for c in &batch.tasks {
        if c.id.trim().is_empty()
            || c.skill.trim().is_empty()
            || c.action.trim().is_empty()
            || c.difference.trim().is_empty()
            || c.requirements.is_empty()
        {
            return Err("Нет действия, требований или обоснования задания".into());
        }
        if c.source_span.trim().is_empty() || !source.contains(&c.source_span) {
            return Err("Опора не цитирует материал".into());
        }
        if c.prompt().trim().is_empty() || !prompts.insert(c.prompt().trim()) || !ids.insert(&c.id)
        {
            return Err("Пустое или повторное задание".into());
        }
        if let Task::Question { reference, .. } = &c.task
            && reference.trim().is_empty()
        {
            return Err("У вопроса нет опоры для сверки".into());
        }
    }
    Ok(())
}

/// The verifier must account for every component and give a whole-chain synthesis.
fn verify_shape(
    v: &serde_json::Value,
    components: &[String],
    tasks: &[Challenge],
) -> Result<(), String> {
    let accepted = v["accepted"].as_bool().ok_or("Нет accepted")?;
    if !accepted {
        return Err(v["reason"]
            .as_str()
            .unwrap_or("Цепочка не помогает собрать ответ")
            .to_string());
    }
    if v["synthesis"].as_str().unwrap_or("").trim().is_empty() {
        return Err("Нет сборки исходного ответа".into());
    }
    let coverage = v["coverage"].as_array().ok_or("Нет карты покрытия")?;
    for item in coverage {
        let steps = item["steps"].as_array().ok_or("Нет ссылок на шаги")?;
        if steps.iter().any(|id| {
            !tasks
                .iter()
                .any(|task| Some(task.id.as_str()) == id.as_str())
        }) {
            return Err("Покрытие ссылается на несуществующий шаг".into());
        }
    }
    for component in components {
        if !coverage.iter().any(|c| {
            c["component"].as_str() == Some(component.as_str())
                && c["steps"].as_array().is_some_and(|s| !s.is_empty())
        }) {
            return Err(format!("Не покрыт компонент: {component}"));
        }
    }
    Ok(())
}

pub fn generate<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    source: &str,
    root: Option<&Task>,
    history: &[Challenge],
    direction: &str,
    priority: Priority,
) -> Result<Batch, PracticeError> {
    if source.trim().is_empty() {
        return Err(PracticeError::EmptyText);
    }
    let key = if root.is_some() { DECOMPOSE } else { GENERATE };
    let base=serde_json::json!({"source":source,"root":root,"history":history.iter().rev().take(20).collect::<Vec<_>>(),"direction":direction}).to_string();
    let system = prompts
        .with_formulas(key)
        .map_err(|e| PracticeError::Prompt(e.to_string()))?;
    let verifier = prompts
        .get(VERIFY)
        .map_err(|e| PracticeError::Prompt(e.to_string()))?
        .to_string();
    let mut reason = String::new();
    for _ in 0..3 {
        let raw = client.complete_json(
            &Request {
                system: system.clone(),
                input: format!("{base}\nИсправить: {reason}"),
                max_tokens: 3500,
                json: true,
                priority,
            },
            |v| {
                let b: Batch = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
                validate(&b, source, root.is_some())
            },
        )?;
        let b: Batch =
            serde_json::from_value(raw).map_err(|e| PracticeError::Rejected(e.to_string()))?;
        if root.is_none() {
            let mut tail: Vec<&Challenge> = history.iter().rev().take(3).collect();
            tail.reverse();
            let mut bad = false;
            for task in &b.tasks {
                tail.push(task);
                if tail.len() >= 4
                    && tail.iter().rev().take(4).all(|c| {
                        c.skill == task.skill
                            && c.action == task.action
                            && c.requirements == task.requirements
                    })
                {
                    bad = true;
                    break;
                }
            }
            if bad {
                reason="Четыре подряд задания одного мыслительного шаблона: измени действие или ситуацию содержательно".into();
                continue;
            }
        }
        if b.tasks.is_empty() {
            return Ok(b);
        }
        let check=client.complete_json(&Request{system:verifier.clone(),input:serde_json::json!({"source":source,"root":root,"history":history.iter().rev().take(20).collect::<Vec<_>>(),"batch":b}).to_string(),max_tokens:1800,json:true,priority},|v|if v["accepted"].is_boolean(){Ok(())}else{Err("Нет accepted".into())})?;
        match verify_shape(&check, &b.components, &b.tasks) {
            Ok(()) => return Ok(b),
            Err(e) => reason = e,
        }
    }
    Err(PracticeError::Rejected(reason))
}

pub fn context(source: &str, subject: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for b in source.bytes().chain(subject.bytes()) {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{VERSION}:{hash:x}")
}

/// Stable subject scope; opening another lesson does not invent a new skill identity.
pub fn scope(db: &Path, lesson: Option<&str>) -> String {
    lesson
        .and_then(|id| {
            cognitive_exoskeleton_lessons::LessonStore::open(db)
                .ok()?
                .lesson(id)
                .ok()?
        })
        .map(|l| l.subject)
        .unwrap_or_default()
}

pub struct Store {
    db: Connection,
}
impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        Self::from_connection(Connection::open(path).map_err(|e| e.to_string())?)
    }
    pub fn from_connection(db: Connection) -> Result<Self, String> {
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS learning_states(context TEXT PRIMARY KEY,revision INTEGER NOT NULL,json TEXT NOT NULL,updated_at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS learning_events(key TEXT PRIMARY KEY,context TEXT NOT NULL,kind TEXT NOT NULL,json TEXT NOT NULL,at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS learning_recall_history(key TEXT PRIMARY KEY,unit TEXT NOT NULL,question TEXT NOT NULL,at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS learning_feedback(day INTEGER PRIMARY KEY,json TEXT NOT NULL,at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS learning_cache(context TEXT PRIMARY KEY,json TEXT NOT NULL,at INTEGER NOT NULL);").map_err(|e|e.to_string())?;
        Ok(Self { db })
    }
    pub fn load(&self, context: &str) -> Result<Option<(u64, Session)>, String> {
        let row: Option<(i64, String)> = self
            .db
            .query_row(
                "SELECT revision,json FROM learning_states WHERE context=?1",
                [context],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        row.map(|(r, s)| {
            serde_json::from_str(&s)
                .map(|s| (r as u64, s))
                .map_err(|e| e.to_string())
        })
        .transpose()
    }
    /// Optimistic revision and event key are committed together: stale callbacks cannot overwrite progress.
    pub fn save(
        &mut self,
        context: &str,
        revision: u64,
        session: &Session,
        key: &str,
        kind: &str,
        at: i64,
    ) -> Result<u64, String> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let old: Option<i64> = tx
            .query_row(
                "SELECT revision FROM learning_states WHERE context=?1",
                [context],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let duplicate: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM learning_events WHERE key=?1)",
                [key],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if duplicate {
            let owner: String = tx
                .query_row(
                    "SELECT context FROM learning_events WHERE key=?1",
                    [key],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if owner != context {
                return Err("Ключ события уже принадлежит другому эпизоду".into());
            }
            return Ok(old.unwrap_or(0) as u64);
        }
        if old.unwrap_or(0) != revision as i64 {
            return Err("Состояние изменилось: перечитай эпизод".into());
        }
        let json = serde_json::to_string(session).map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO learning_states VALUES(?1,?2,?3,?4) ON CONFLICT(context) DO UPDATE SET revision=excluded.revision,json=excluded.json,updated_at=excluded.updated_at",params![context,revision as i64+1,json,at]).map_err(|e|e.to_string())?;
        tx.execute(
            "INSERT INTO learning_events VALUES(?1,?2,?3,?4,?5)",
            params![key, context, kind, json, at],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(revision + 1)
    }
    pub fn cache(&self, context: &str) -> Result<Option<Batch>, String> {
        let s: Option<String> = self
            .db
            .query_row(
                "SELECT json FROM learning_cache WHERE context=?1",
                [context],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        s.map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .transpose()
    }
    pub fn cache_batch(&self, context: &str, batch: &Batch, at: i64) -> Result<(), String> {
        self.db.execute("INSERT INTO learning_cache VALUES(?1,?2,?3) ON CONFLICT(context) DO UPDATE SET json=excluded.json,at=excluded.at",params![context,serde_json::to_string(batch).map_err(|e|e.to_string())?,at]).map(|_|()).map_err(|e|e.to_string())
    }
    pub fn event(
        &self,
        key: &str,
        context: &str,
        kind: &str,
        value: &serde_json::Value,
        at: i64,
    ) -> Result<(), String> {
        self.db
            .execute(
                "INSERT OR IGNORE INTO learning_events VALUES(?1,?2,?3,?4,?5)",
                params![key, context, kind, value.to_string(), at],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    pub fn recalled(&self, key: &str, unit: &str, question: &str, at: i64) -> Result<(), String> {
        self.db
            .execute(
                "INSERT OR IGNORE INTO learning_recall_history VALUES(?1,?2,?3,?4)",
                params![key, unit, question, at],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    pub fn question_history(&self, unit: &str) -> Result<Vec<String>, String> {
        let mut stmt=self.db.prepare("SELECT question FROM learning_recall_history WHERE unit=?1 ORDER BY at DESC LIMIT 20").map_err(|e|e.to_string())?;
        stmt.query_map([unit], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    pub fn feedback_available(&self, day: i64) -> Result<bool, String> {
        self.db
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM learning_feedback WHERE day=?1)",
                [day],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }
    pub fn feedback_response(
        &self,
        day: i64,
        value: &serde_json::Value,
        at: i64,
    ) -> Result<(), String> {
        self.db
            .execute(
                "UPDATE learning_feedback SET json=?2,at=?3 WHERE day=?1",
                params![day, value.to_string(), at],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    pub fn feedback(&self, day: i64, value: &serde_json::Value, at: i64) -> Result<(), String> {
        self.db
            .execute(
                "INSERT OR IGNORE INTO learning_feedback VALUES(?1,?2,?3)",
                params![day, value.to_string(), at],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn question(id: &str) -> Challenge {
        Challenge {
            id: id.into(),
            skill: "обновлять оценки".into(),
            action: "рассуждать".into(),
            source_span: "Новые данные меняют оценку".into(),
            requirements: vec!["сопоставить".into()],
            relation: Relation::Entry,
            difference: "новая ситуация".into(),
            warmup: false,
            task: Task::Question {
                question: format!("Ситуация {id}?"),
                reference: "Сравнить основания".into(),
            },
        }
    }
    fn batch(tasks: Vec<Challenge>) -> Batch {
        Batch {
            components: vec!["новые данные".into()],
            tasks,
        }
    }
    #[test]
    fn the_entire_support_episode_and_return_are_after_help() {
        let mut s = Session::new(question("root"), vec![]);
        s.support(batch(vec![question("first"), question("second")]))
            .unwrap();
        for key in ["a", "b", "c"] {
            s.episode.reveal().unwrap();
            s.advance(key, Some(Assessment::Matched)).unwrap();
        }
        assert!(s.finished);
        assert_eq!(s.policy.matches, 0);
        assert!(
            s.episode
                .observations
                .iter()
                .all(|o| o.provenance == crate::episode::Provenance::AfterHelp)
        );
    }
    #[test]
    fn nested_support_returns_to_child_then_root_and_is_bounded() {
        let mut s = Session::new(question("root"), vec![]);
        s.support(batch(vec![question("child")])).unwrap();
        s.support(batch(vec![question("grandchild")])).unwrap();
        assert!(s.support(batch(vec![question("too_deep")])).is_err());
        assert_eq!(s.episode.current(), &question("grandchild").task);
        s.advance("skip", None).unwrap();
        assert_eq!(s.episode.current(), &question("child").task);
        s.return_to_root();
        assert_eq!(s.episode.current(), &question("root").task);
        assert!(s.episode.helped);
    }
    #[test]
    fn mind_map_has_no_grade_no_reference_and_no_training_success() {
        let mut action = question("map");
        action.task = Task::Action {
            instruction: "Выпиши четыре слова и соедини в тетради".into(),
        };
        let mut s = Session::new(action, vec![]);
        assert!(s.episode.reveal().is_err());
        assert!(s.advance("invalid", Some(Assessment::Matched)).is_err());
        s.advance("next", None).unwrap();
        assert_eq!(s.policy.matches, 0);
    }
    #[test]
    fn three_matches_only_change_experimental_request_not_provenance() {
        let mut p = Policy::default();
        for _ in 0..3 {
            p.observe(Some(Assessment::Matched), false);
        }
        assert_eq!(p.request(), "less_support");
        let mut e = Episode::new(question("root").task);
        e.reveal().unwrap();
        e.advance("x", Some(Assessment::Matched)).unwrap();
        assert_eq!(
            e.observations[0].provenance,
            crate::episode::Provenance::SelfReport
        );
        p.observe(Some(Assessment::Partial), false);
        assert_eq!(p.request(), "variation");
        for _ in 0..2 {
            p.observe(Some(Assessment::Missed), false);
        }
        assert_eq!(p.request(), "prepares");
    }
    #[test]
    fn store_restores_exact_stage_draft_and_rejects_stale_save() {
        let mut store = Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        let mut s = Session::new(question("root"), vec![]);
        s.support(batch(vec![question("child")])).unwrap();
        s.draft = "Моя попытка".into();
        s.episode.reveal().unwrap();
        assert_eq!(store.save("c", 0, &s, "one", "reveal", 100).unwrap(), 1);
        assert_eq!(store.save("c", 0, &s, "one", "reveal", 100).unwrap(), 1);
        assert!(store.save("c", 0, &s, "two", "advance", 101).is_err());
        let (rev, restored) = store.load("c").unwrap().unwrap();
        assert_eq!(rev, 1);
        assert_eq!(restored.draft, s.draft);
        assert!(restored.episode.revealed);
        assert!(restored.episode.helped);
        let tables: i64 = store
            .db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='memory_reviews'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "episode never creates FSRS reviews");
    }
    #[test]
    fn scheduler_selects_the_requested_relation_not_the_models_order() {
        let mut easier = question("easier");
        easier.relation = Relation::Prepares;
        let mut harder = question("harder");
        harder.relation = Relation::LessSupport;
        let mut s = Session::new(question("root"), vec![easier, harder]);
        let policy = Policy {
            matches: 2,
            ..Policy::default()
        };
        s.policies.insert(s.root.skill.clone(), policy);
        s.episode.reveal().unwrap();
        s.advance("root-done", Some(Assessment::Matched)).unwrap();
        assert_eq!(s.root.id, "harder");
        assert!(
            !s.advance("root-done", Some(Assessment::Matched)).unwrap(),
            "replayed mark never touches next task"
        );
    }
    #[test]
    fn content_filter_requires_a_whole_synthesis_and_actual_step_references() {
        let b = batch(vec![question("one")]);
        let missing = serde_json::json!({"accepted":true,"synthesis":"", "coverage":[]});
        assert!(verify_shape(&missing, &b.components, &b.tasks).is_err());
        let fake = serde_json::json!({"accepted":true,"synthesis":"Сборка","coverage":[{"component":"новые данные","steps":["invented"]}]});
        assert!(verify_shape(&fake, &b.components, &b.tasks).is_err());
        let good = serde_json::json!({"accepted":true,"synthesis":"Сборка","coverage":[{"component":"новые данные","steps":["one"]}]});
        assert!(verify_shape(&good, &b.components, &b.tasks).is_ok());
    }
    #[test]
    fn malformed_foreign_duplicate_steps_do_not_enter_the_feed() {
        let q = question("one");
        assert!(validate(&batch(vec![q.clone()]), "Новые данные меняют оценку", true).is_ok());
        assert!(validate(&batch(vec![q.clone()]), "Другая тема", true).is_err());
        assert!(
            validate(
                &batch(vec![q.clone(), q]),
                "Новые данные меняют оценку",
                true
            )
            .is_err()
        );
    }
    #[test]
    fn feedback_is_optional_offered_once_and_never_a_success_event() {
        let store = Store::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        assert!(store.feedback_available(1).unwrap());
        store
            .feedback(1, &serde_json::json!({"response":null}), 100)
            .unwrap();
        assert!(!store.feedback_available(1).unwrap());
        store
            .feedback_response(
                1,
                &serde_json::json!({"difficulty":"Посильно","feeling":"Интересно"}),
                101,
            )
            .unwrap();
        let n: i64 = store
            .db
            .query_row("SELECT COUNT(*) FROM learning_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;
    use std::cell::RefCell;
    struct Stub {
        replies: RefCell<std::collections::VecDeque<serde_json::Value>>,
    }
    impl Transport for Stub {
        fn post(
            &self,
            _: &str,
            _: &[(&str, String)],
            _: &serde_json::Value,
        ) -> Result<(u16, String), String> {
            let text = self
                .replies
                .borrow_mut()
                .pop_front()
                .ok_or("unexpected request")?
                .to_string();
            Ok((
                200,
                serde_json::json!({"candidates":[{"content":{"parts":[{"text":text}]}}]})
                    .to_string(),
            ))
        }
    }
    fn setup(replies: Vec<serde_json::Value>) -> (Client<Stub>, PromptStore) {
        let config = cognitive_exoskeleton_ai::AiConfig::new(
            cognitive_exoskeleton_ai::ProviderKind::Gemini,
            "test",
            None,
        )
        .with_key(Some("fixture".into()));
        (
            Client::new(
                config,
                Stub {
                    replies: RefCell::new(replies.into()),
                },
            ),
            PromptStore::from_dir(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/v2/port/prompts"),
            )
            .unwrap(),
        )
    }
    fn batch() -> serde_json::Value {
        serde_json::json!({"components":["обновление"],"tasks":[{"id":"one","skill":"пересматривать оценку","action":"рассудить","source_span":"Новые данные","requirements":["сопоставить"],"relation":"prepares","difference":"конкретная ситуация","warmup":false,"task":{"kind":"question","question":"Ключи звенят из сумки. Где теперь искать?","reference":"Сумка стала вероятнее по новому свидетельству."}}]})
    }
    #[test]
    fn accessible_steps_without_a_supported_whole_answer_are_rejected() {
        let rejected = serde_json::json!({"accepted":true,"synthesis":"Вывод","coverage":[{"component":"другой компонент","steps":["one"]}]});
        let (client, prompts) = setup(vec![
            batch(),
            rejected.clone(),
            batch(),
            rejected.clone(),
            batch(),
            rejected,
        ]);
        let root = Task::Question {
            question: "Зачем Байес?".into(),
            reference: "обновление".into(),
        };
        assert!(
            generate(
                &client,
                &prompts,
                "Новые данные",
                Some(&root),
                &[],
                "prepares",
                Priority::Foreground
            )
            .is_err()
        );
    }
    #[test]
    fn repaired_chain_is_the_only_content_returned() {
        let rejected =
            serde_json::json!({"accepted":false,"reason":"Нет моста к исходному ответу"});
        let accepted = serde_json::json!({"accepted":true,"synthesis":"Оценка меняется при новых данных","coverage":[{"component":"обновление","steps":["one"]}]});
        let (client, prompts) = setup(vec![batch(), rejected, batch(), accepted]);
        assert_eq!(
            generate(
                &client,
                &prompts,
                "Новые данные",
                None,
                &[],
                "entry",
                Priority::Foreground
            )
            .unwrap()
            .tasks
            .len(),
            1
        );
    }
    #[test]
    fn action_cannot_smuggle_a_reference_or_grade_into_the_contract() {
        let task = serde_json::json!({"kind":"action","instruction":"Рисуй карту","reference":"готовая карта"});
        assert!(serde_json::from_value::<Task>(task).is_err());
    }
}
