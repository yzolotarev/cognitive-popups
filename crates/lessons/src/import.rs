//! Subjects from one text file, so a whole study plan is set up at once and
//! can be read back the same way (`subjects import` / `subjects export`):
//!
//! ```text
//! # Теория вероятностей
//! Цель: решать задачи на Байеса и ЦПТ без подсказок
//! Срок: 2026-11-28
//! - Комбинаторика
//! - Формула Байеса
//! ```
//!
//! Importing again changes nothing that is already there: a subject is found
//! by name, its goal and deadline are updated only when given, and the
//! program keeps the reader's marks (`set_program`). Nothing is removed.

use serde::Serialize;

use crate::store::{LessonStore, StoreError, Subject, end_of_day};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubjectDraft {
    pub name: String,
    pub goal: Option<String>,
    pub deadline: Option<String>,
    pub topics: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Imported {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("строка {line}: {message}")]
    Line { line: usize, message: String },
    #[error("«{0}»: у нового предмета нужна строка «Цель:»")]
    NoGoal(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn field<'a>(line: &'a str, names: &[&str]) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    names
        .contains(&key.trim().to_lowercase().as_str())
        .then(|| value.trim())
}

fn topic(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some(rest.trim());
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    (digits > 0)
        .then(|| line[digits..].strip_prefix(". "))
        .flatten()
        .map(str::trim)
}

pub fn parse(text: &str) -> Result<Vec<SubjectDraft>, ImportError> {
    let mut drafts: Vec<SubjectDraft> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let error = |message: &str| ImportError::Line {
            line: n + 1,
            message: message.into(),
        };
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('#') {
            let name = name.trim_start_matches('#').trim();
            if name.is_empty() {
                return Err(error("после # нужно название предмета"));
            }
            drafts.push(SubjectDraft {
                name: name.into(),
                ..SubjectDraft::default()
            });
            continue;
        }
        let Some(draft) = drafts.last_mut() else {
            return Err(error("сначала строка «# Название предмета»"));
        };
        if let Some(goal) = field(line, &["цель", "goal"]) {
            draft.goal = Some(goal.into()).filter(|g: &String| !g.is_empty());
        } else if let Some(date) = field(line, &["срок", "deadline"]) {
            if date.is_empty() {
                continue;
            }
            if end_of_day(date).is_none() {
                return Err(error("срок пишется как ГГГГ-ММ-ДД"));
            }
            draft.deadline = Some(date.into());
        } else if let Some(title) = topic(line) {
            if !title.is_empty() {
                draft.topics.push(title.into());
            }
        } else {
            return Err(error(
                "не понял строку: ждал «Цель:», «Срок:» или тему через «- »",
            ));
        }
    }
    Ok(drafts)
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Writes the drafts; the whole file is checked before anything is written.
pub fn apply(
    store: &mut LessonStore,
    drafts: &[SubjectDraft],
    now: i64,
) -> Result<Imported, ImportError> {
    let existing = store.subjects()?;
    for draft in drafts {
        let known = existing.iter().any(|s| same_name(&s.name, &draft.name));
        if !known && draft.goal.is_none() {
            return Err(ImportError::NoGoal(draft.name.clone()));
        }
    }
    let mut report = Imported::default();
    for draft in drafts {
        let found = store
            .subjects()?
            .into_iter()
            .find(|s| same_name(&s.name, &draft.name));
        let (id, mut changed) = match found {
            Some(subject) => {
                let updated = Subject {
                    goal: draft.goal.clone().unwrap_or(subject.goal.clone()),
                    deadline: draft.deadline.clone().or(subject.deadline.clone()),
                    ..subject.clone()
                };
                let changed = updated != subject;
                if changed {
                    store.update_subject(&updated)?;
                }
                (subject.id, changed)
            }
            None => {
                let goal = draft.goal.as_deref().unwrap_or_default();
                let id = store.add_subject(&draft.name, goal, draft.deadline.as_deref(), now)?;
                report.added.push(draft.name.clone());
                (id, false)
            }
        };
        if !draft.topics.is_empty() {
            let before: Vec<String> = store.program(&id)?.into_iter().map(|t| t.title).collect();
            store.set_program(&id, &draft.topics, now)?;
            let after: Vec<String> = store.program(&id)?.into_iter().map(|t| t.title).collect();
            changed |= before != after;
        }
        if !report.added.contains(&draft.name) {
            if changed {
                report.updated.push(draft.name.clone());
            } else {
                report.unchanged.push(draft.name.clone());
            }
        }
    }
    Ok(report)
}

/// The same format back, oldest subject first. Marks stay in the database:
/// the file carries the plan, not the progress.
pub fn export(store: &LessonStore) -> Result<String, StoreError> {
    let mut out = String::new();
    let mut subjects = store.subjects()?;
    subjects.reverse();
    for (i, subject) in subjects.into_iter().enumerate() {
        // A blank line between subjects, none after the last: a plan written
        // in this format reads back byte for byte.
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("# {}\nЦель: {}\n", subject.name, subject.goal));
        if let Some(deadline) = &subject.deadline {
            out.push_str(&format!("Срок: {deadline}\n"));
        }
        for topic in store.program(&subject.id)? {
            out.push_str(&format!("- {}\n", topic.title));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "# Теория вероятностей\n\
        Цель: решать задачи на Байеса без подсказок\n\
        Срок: 2026-11-28\n\
        - Комбинаторика\n\
        - Формула Байеса\n\
        \n\
        ## SQL\n\
        goal: JOIN, GROUP BY, оконные функции\n\
        1. SELECT и WHERE\n\
        2. JOIN\n";

    #[test]
    fn a_plan_is_read_with_goals_deadlines_and_topics() {
        let drafts = parse(PLAN).unwrap();
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].deadline.as_deref(), Some("2026-11-28"));
        assert_eq!(drafts[0].topics, ["Комбинаторика", "Формула Байеса"]);
        assert_eq!(drafts[1].name, "SQL");
        assert_eq!(
            drafts[1].goal.as_deref(),
            Some("JOIN, GROUP BY, оконные функции")
        );
        assert_eq!(drafts[1].topics, ["SELECT и WHERE", "JOIN"]);
    }

    #[test]
    fn mistakes_name_their_line() {
        let error = parse("- тема без предмета").unwrap_err().to_string();
        assert!(error.contains("строка 1"), "{error}");
        let error = parse("# A\nЦель: x\nСрок: 28.11.2026")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("строка 3") && error.contains("ГГГГ-ММ-ДД"),
            "{error}"
        );
        let error = parse("# A\nкакой-то текст").unwrap_err().to_string();
        assert!(error.contains("строка 2"), "{error}");
    }

    #[test]
    fn importing_twice_changes_nothing_and_keeps_marks() {
        let mut store = LessonStore::open_in_memory().unwrap();
        let first = apply(&mut store, &parse(PLAN).unwrap(), 1).unwrap();
        assert_eq!(first.added, ["Теория вероятностей", "SQL"]);
        let id = store.subjects().unwrap()[1].id.clone();
        let topic = store.program(&id).unwrap()[0].id;
        store.mark_topic(topic, true, 2).unwrap();
        let again = apply(&mut store, &parse(PLAN).unwrap(), 3).unwrap();
        assert!(again.added.is_empty() && again.updated.is_empty());
        assert_eq!(again.unchanged.len(), 2);
        assert_eq!(store.subjects().unwrap().len(), 2);
        assert!(store.program(&id).unwrap()[0].done());
    }

    #[test]
    fn a_known_subject_is_updated_only_where_the_file_says_so() {
        let mut store = LessonStore::open_in_memory().unwrap();
        apply(&mut store, &parse(PLAN).unwrap(), 1).unwrap();
        let report = apply(&mut store, &parse("# sql\nСрок: 2026-12-20").unwrap(), 2).unwrap();
        assert_eq!(report.updated, ["sql"]);
        let sql = store
            .subjects()
            .unwrap()
            .into_iter()
            .find(|s| s.name == "SQL")
            .unwrap();
        assert_eq!(sql.goal, "JOIN, GROUP BY, оконные функции");
        assert_eq!(sql.deadline.as_deref(), Some("2026-12-20"));
        assert_eq!(store.program(&sql.id).unwrap().len(), 2);
    }

    #[test]
    fn a_new_subject_without_a_goal_stops_the_whole_import() {
        let mut store = LessonStore::open_in_memory().unwrap();
        let error = apply(&mut store, &parse("# A\nЦель: x\n# B\n- t").unwrap(), 1).unwrap_err();
        assert!(error.to_string().contains("«B»"));
        assert!(store.subjects().unwrap().is_empty(), "nothing written");
    }

    #[test]
    fn export_reads_back_as_the_same_plan() {
        let mut store = LessonStore::open_in_memory().unwrap();
        apply(&mut store, &parse(PLAN).unwrap(), 1).unwrap();
        let text = export(&store).unwrap();
        let back = parse(&text).unwrap();
        assert_eq!(back, parse(PLAN).unwrap());
    }

    #[test]
    fn a_plan_in_the_export_format_reads_back_byte_for_byte() {
        let plan = "# A\nЦель: x\nСрок: 2026-12-14\n- t1\n- t2\n\n# B\nЦель: y\n";
        let mut store = LessonStore::open_in_memory().unwrap();
        apply(&mut store, &parse(plan).unwrap(), 1).unwrap();
        assert_eq!(export(&store).unwrap(), plan);
    }
}
