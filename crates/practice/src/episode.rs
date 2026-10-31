//! A learning episode records support, never infers mastery from navigation.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Task {
    Question { question: String, reference: String },
    Action { instruction: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Assessment {
    Missed,
    Partial,
    Matched,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provenance {
    SelfReport,
    AfterHelp,
    Navigation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub key: String,
    pub assessment: Option<Assessment>,
    pub provenance: Provenance,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub original: Task,
    pub preparation: Vec<Task>,
    pub position: usize,
    pub helped: bool,
    pub revealed: bool,
    pub observations: Vec<Observation>,
}
impl Episode {
    pub fn new(original: Task) -> Self {
        Self {
            original,
            preparation: vec![],
            position: 0,
            helped: false,
            revealed: false,
            observations: vec![],
        }
    }
    pub fn current(&self) -> &Task {
        self.preparation
            .get(self.position)
            .unwrap_or(&self.original)
    }
    pub fn support(&mut self, tasks: Vec<Task>) -> Result<(), &'static str> {
        if tasks.is_empty() || tasks.len() > 10 {
            return Err("support needs 1–10 tasks");
        }
        self.preparation = tasks;
        self.position = 0;
        self.helped = true;
        self.revealed = false;
        Ok(())
    }
    pub fn reveal(&mut self) -> Result<(), &'static str> {
        if matches!(self.current(), Task::Action { .. }) {
            return Err("actions have no answer");
        }
        self.revealed = true;
        Ok(())
    }
    pub fn advance(
        &mut self,
        key: &str,
        assessment: Option<Assessment>,
    ) -> Result<bool, &'static str> {
        if self.observations.iter().any(|o| o.key == key) {
            return Ok(false);
        }
        let action = matches!(self.current(), Task::Action { .. });
        if action && assessment.is_some() {
            return Err("actions cannot be assessed");
        }
        if assessment.is_some() && !self.revealed {
            return Err("reveal before self-assessment");
        }
        let provenance = if assessment.is_none() {
            Provenance::Navigation
        } else if self.helped {
            Provenance::AfterHelp
        } else {
            Provenance::SelfReport
        };
        self.observations.push(Observation {
            key: key.into(),
            assessment,
            provenance,
        });
        if self.position < self.preparation.len() {
            self.position += 1;
        }
        self.revealed = false;
        Ok(true)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn question() -> Task {
        Task::Question {
            question: "Зачем Байес?".into(),
            reference: "Обновлять оценки".into(),
        }
    }
    #[test]
    fn action_is_navigation_not_success() {
        let mut e = Episode::new(question());
        e.support(vec![Task::Action {
            instruction: "Нарисуй карту".into(),
        }])
        .unwrap();
        assert!(e.reveal().is_err());
        assert!(e.advance("x", Some(Assessment::Matched)).is_err());
        assert!(e.advance("x", None).unwrap());
        assert_eq!(e.current(), &question());
        assert_eq!(e.observations[0].provenance, Provenance::Navigation);
    }
    #[test]
    fn return_after_support_never_becomes_independent() {
        let mut e = Episode::new(question());
        e.support(vec![question()]).unwrap();
        e.reveal().unwrap();
        e.advance("one", Some(Assessment::Matched)).unwrap();
        let json = serde_json::to_string(&e).unwrap();
        let mut restored: Episode = serde_json::from_str(&json).unwrap();
        restored.reveal().unwrap();
        restored.advance("two", Some(Assessment::Matched)).unwrap();
        assert!(
            restored
                .observations
                .iter()
                .all(|o| o.provenance == Provenance::AfterHelp)
        );
        assert!(!restored.advance("two", Some(Assessment::Matched)).unwrap());
    }
    #[test]
    fn recognition_remains_self_report() {
        let mut e = Episode::new(question());
        e.reveal().unwrap();
        e.advance("one", Some(Assessment::Matched)).unwrap();
        assert_eq!(e.observations[0].provenance, Provenance::SelfReport);
    }
}
