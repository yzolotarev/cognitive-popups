//! MRT infrastructure, not an approved experiment protocol. Production callers
//! must supply an explicitly approved versioned protocol and opt-in consent.
//! No screen monitoring, causal claims or automatic policy updates live here.
use crate::Error;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Protocol {
    pub id: String,
    pub version: String,
    pub outcome_window_seconds: i64,
    pub suggestion_probability: f64,
    pub minimum_interval_seconds: i64,
    pub approved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    SuggestDiagnostic,
    DoNothing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Eligibility {
    pub consent: bool,
    pub outside_session_consent: bool,
    pub in_session: bool,
    pub reliable_context: bool,
    pub interfering_window: bool,
    pub verified_error_event_id: Option<String>,
    pub last_decision_at: Option<i64>,
    pub unit_id: String,
    #[serde(default)]
    pub active_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub at: i64,
    pub protocol: Protocol,
    pub eligibility: Eligibility,
    pub alternatives: Vec<(Action, f64)>,
    pub random_draw: u64,
    pub assigned: Action,
    pub delivery: Delivery,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Delivery {
    NotApplicableControl,
    Pending,
    Attempted { at: i64 },
    Delivered { at: i64 },
    Failed,
    CancelledConsentRevoked,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Consent {
    pub enabled: bool,
    pub outside_session: bool,
}

pub fn approved_protocol() -> Protocol {
    Protocol {
        id: "mrt-verified-error".into(),
        version: "1".into(),
        outcome_window_seconds: 86400,
        suggestion_probability: 0.5,
        minimum_interval_seconds: 900,
        approved: true,
    }
}

impl crate::store::Store {
    pub fn mrt_consent(&self) -> Result<Consent, Error> {
        Ok(self
            .get::<Consent>("consent", "mrt")?
            .map(|r| r.value)
            .unwrap_or_default())
    }

    pub fn save_mrt_consent(&mut self, consent: &Consent, at: i64) -> Result<(), Error> {
        let revision = self
            .get::<Consent>("consent", "mrt")?
            .map_or(0, |r| r.revision);
        self.save("consent", "mrt", revision, consent, at)?;
        Ok(())
    }

    /// Called only after a stored attempt. Claims from a model/caller are not
    /// used: outcomes and unit IDs are reconstructed from immutable DB records.
    pub fn consider_mrt(
        &mut self,
        task_id: &str,
        in_session: bool,
        interfering_window: bool,
        at: i64,
    ) -> Result<Option<Decision>, Error> {
        use crate::assessment::{Attempt, Observed};
        let key = format!("mrt:{task_id}");
        let Some(task) = self.task(task_id)? else {
            return Ok(None);
        };
        // Consent, prior assignments, frequency checks and assignment commit
        // are one serialized operation, not a read-then-write race.
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM objects WHERE kind='mrt_decision' AND id=?1)",
            [&key],
            |r| r.get(0),
        )?;
        if exists {
            return Ok(None);
        }
        let json: Option<String> = tx
            .query_row(
                "SELECT json FROM attempts WHERE task_id=?1",
                [task_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(json) = json else { return Ok(None) };
        let attempt: Attempt = serde_json::from_str(&json)?;
        let Some(index) = attempt
            .outcomes
            .iter()
            .position(|o| *o == Observed::Incorrect)
        else {
            return Ok(None);
        };
        let consent: Option<String> = tx
            .query_row(
                "SELECT json FROM objects WHERE kind='consent' AND id='mrt'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let consent: Consent = consent
            .map(|json| serde_json::from_str(&json))
            .transpose()?
            .unwrap_or_default();
        let previous = {
            let mut statement = tx.prepare("SELECT json FROM objects WHERE kind='mrt_decision'")?;
            statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|json| serde_json::from_str::<Decision>(&json))
                .collect::<Result<Vec<_>, _>>()?
        };
        if previous
            .iter()
            .filter(|r| r.at.div_euclid(86400) == at.div_euclid(86400))
            .count()
            >= 3
        {
            return Ok(None);
        }
        let unit_id = task.components[index].unit_id.clone();
        let unit_json: String = tx.query_row(
            "SELECT json FROM objects WHERE kind='unit' AND id=?1",
            [&unit_id],
            |r| r.get(0),
        )?;
        let unit: crate::learning::MemoryUnit = serde_json::from_str(&unit_json)?;
        let sessions = {
            let mut statement = tx.prepare("SELECT id,json FROM objects WHERE kind='session'")?;
            statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut active_session_id = None;
        if in_session {
            for (id, json) in sessions {
                let session: crate::learning::Session = serde_json::from_str(&json)?;
                if session.status == crate::learning::SessionStatus::Active
                    && session.subject_id == unit.subject_id
                {
                    active_session_id = Some(id);
                    break;
                }
            }
        }
        let eligibility = Eligibility {
            consent: consent.enabled,
            outside_session_consent: consent.outside_session,
            in_session: active_session_id.is_some(),
            reliable_context: attempt.at == at,
            interfering_window,
            verified_error_event_id: Some(format!("{}:{}", task_id, index)),
            last_decision_at: previous.iter().map(|r| r.at).max(),
            unit_id,
            active_session_id,
        };
        let mut random = [0u8; 8];
        getrandom::fill(&mut random)
            .map_err(|_| Error::Invalid("OS randomness unavailable; MRT not assigned"))?;
        let decision = match assign(
            key.clone(),
            at,
            approved_protocol(),
            eligibility,
            u64::from_le_bytes(random),
        ) {
            Ok(decision) => decision,
            Err(_) => return Ok(None),
        };
        let json = serde_json::to_string(&decision)?;
        tx.execute(
            "INSERT INTO objects(kind,id,revision,json) VALUES('mrt_decision',?1,1,?2)",
            rusqlite::params![key, json],
        )?;
        tx.execute(
            "INSERT INTO changes(kind,id,revision,at,json) VALUES('mrt_decision',?1,1,?2,?3)",
            rusqlite::params![key, at, json],
        )?;
        tx.commit()?;
        Ok(Some(decision))
    }

    /// Same observation mechanism for treatment and control. Only independent
    /// stored code checks of the exact same target are eligible outcomes.
    pub fn observe_mrt(&mut self, task_id: &str, at: i64) -> Result<(), Error> {
        if self.task(task_id)?.is_none() {
            return Ok(());
        }
        self.reconcile_mrt(at)
    }

    fn reconcile_mrt(&mut self, at: i64) -> Result<(), Error> {
        use crate::assessment::Observed;
        for mut record in self.list::<Decision>("mrt_decision")? {
            let decision = &mut record.value;
            if matches!(decision.outcome, Outcome::IndependentCheck { .. }) {
                continue;
            }
            let before = decision.outcome.clone();
            // Reconcile persisted attempts before expiring, even if processing
            // resumed after the measurement window. Event time is authoritative.
            let evidence =
                self.evidence_for_units(std::slice::from_ref(&decision.eligibility.unit_id))?;
            let observed = evidence
                .into_iter()
                .filter(|e| {
                    e.at > decision.at
                        && e.at.saturating_sub(decision.at)
                            <= decision.protocol.outcome_window_seconds
                        && decision.id != format!("mrt:{}", e.task_id)
                        && matches!(e.observed, Observed::Correct | Observed::Incorrect)
                })
                .min_by_key(|e| (e.at, e.task_id.clone()));
            if let Some(evidence) = observed {
                decision.outcome = Outcome::Pending;
                decision.independent_outcome(
                    format!("{}:{}", evidence.task_id, evidence.unit_id),
                    evidence.at,
                    evidence.observed == Observed::Correct,
                )?;
            } else if at.saturating_sub(decision.at) > decision.protocol.outcome_window_seconds {
                decision.outcome = Outcome::Missing;
            }
            if decision.outcome != before {
                self.save("mrt_decision", &record.id, record.revision, decision, at)?;
            }
        }
        Ok(())
    }

    pub fn mrt_report(&mut self, at: i64) -> Result<serde_json::Value, Error> {
        self.reconcile_mrt(at)?;
        let mut decisions = Vec::new();
        for record in self.list::<Decision>("mrt_decision")? {
            decisions.push(record.value);
        }
        Ok(
            serde_json::json!({"protocol":approved_protocol(),"suggestion":summarize(&decisions,Action::SuggestDiagnostic),"control":summarize(&decisions,Action::DoNothing),
            "method":"descriptive rates and Wilson 95% intervals; repeated personal observations, not independent population data; missing outcomes may bias results; no causal efficacy claim or automatic optimization",
            "decisions":decisions}),
        )
    }
}

/// Best-effort external delivery: persist intent before notifying. A crash after
/// the external side effect leaves Attempted (unknown), never a fabricated
/// Delivered state and never an automatic duplicate notification.
pub fn deliver_pending(path: &std::path::Path, id: &str, at: i64) -> Result<(), Error> {
    let mut store = crate::store::Store::open(path)?;
    let Some(mut record) = store.get::<Decision>("mrt_decision", id)? else {
        return Ok(());
    };
    if record.value.delivery != Delivery::Pending
        || record.value.assigned != Action::SuggestDiagnostic
    {
        return Ok(());
    }
    let consent = store.mrt_consent()?;
    let currently_in_session = if let Some(session_id) = &record.value.eligibility.active_session_id
    {
        store
            .get::<crate::learning::Session>("session", session_id)?
            .is_some_and(|session| session.value.status == crate::learning::SessionStatus::Active)
    } else {
        false
    };
    if !consent.enabled || (!currently_in_session && !consent.outside_session) {
        record.value.delivery = Delivery::CancelledConsentRevoked;
        store.save("mrt_decision", id, record.revision, &record.value, at)?;
        return Ok(());
    }
    record.value.delivery = Delivery::Attempted { at };
    record.revision = store.save("mrt_decision", id, record.revision, &record.value, at)?;
    let delivered=notify_rust::Notification::new().summary("Cognitive Exoskeleton — необязательная диагностика")
        .body("После проверенной ошибки можно позже попробовать отдельную задачу на ту же единицу. Это предложение, не обязательное упражнение. Откройте вкладку «Проверка», когда удобно.")
        .appname("Cognitive Exoskeleton").timeout(6000).show().is_ok();
    record.value.delivery = if delivered {
        Delivery::Delivered { at }
    } else {
        Delivery::Failed
    };
    store.save("mrt_decision", id, record.revision, &record.value, at)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Pending,
    Missing,
    IndependentCheck {
        event_id: String,
        at: i64,
        correct: bool,
    },
}

pub fn assign(
    id: String,
    at: i64,
    protocol: Protocol,
    eligibility: Eligibility,
    random_draw: u64,
) -> Result<Decision, Error> {
    if !protocol.approved
        || protocol.id.trim().is_empty()
        || protocol.version.trim().is_empty()
        || protocol.outcome_window_seconds <= 0
        || protocol.minimum_interval_seconds <= 0
        || !protocol.suggestion_probability.is_finite()
        || protocol.suggestion_probability <= 0.0
        || protocol.suggestion_probability >= 1.0
    {
        return Err(Error::Invalid(
            "an approved, versioned MRT protocol with both alternatives is required",
        ));
    }
    if !eligibility.consent
        || (!eligibility.in_session && !eligibility.outside_session_consent)
        || !eligibility.reliable_context
        || eligibility.interfering_window
        || eligibility
            .verified_error_event_id
            .as_ref()
            .is_none_or(|id| id.trim().is_empty())
        || eligibility
            .last_decision_at
            .is_some_and(|last| at.saturating_sub(last) < protocol.minimum_interval_seconds)
    {
        return Err(Error::Invalid("not an eligible MRT decision point"));
    }
    let p = protocol.suggestion_probability;
    let assigned = if (random_draw as f64) / ((u64::MAX as f64) + 1.0) < p {
        Action::SuggestDiagnostic
    } else {
        Action::DoNothing
    };
    Ok(Decision {
        id,
        at,
        protocol,
        eligibility,
        alternatives: vec![(Action::SuggestDiagnostic, p), (Action::DoNothing, 1.0 - p)],
        random_draw,
        assigned,
        delivery: if assigned == Action::DoNothing {
            Delivery::NotApplicableControl
        } else {
            Delivery::Pending
        },
        outcome: Outcome::Pending,
    })
}

impl Decision {
    /// Consent is checked again at delivery, not only at randomization.
    pub fn delivered(&mut self, at: i64, consent_still_valid: bool) -> Result<(), Error> {
        if self.assigned == Action::DoNothing || self.delivery != Delivery::Pending || at < self.at
        {
            return Err(Error::Invalid(
                "control or already-resolved decision cannot be delivered",
            ));
        }
        self.delivery = if consent_still_valid {
            Delivery::Delivered { at }
        } else {
            Delivery::CancelledConsentRevoked
        };
        Ok(())
    }

    /// Caller must resolve this ID to an actual independent check before using
    /// it. Clicks, elapsed time and LLM opinion are deliberately not outcomes.
    pub fn independent_outcome(
        &mut self,
        event_id: String,
        at: i64,
        correct: bool,
    ) -> Result<(), Error> {
        if self.outcome != Outcome::Pending
            || event_id.trim().is_empty()
            || at <= self.at
            || at.saturating_sub(self.at) > self.protocol.outcome_window_seconds
        {
            return Err(Error::Invalid(
                "outcome missing, already set or outside protocol window",
            ));
        }
        self.outcome = Outcome::IndependentCheck {
            event_id,
            at,
            correct,
        };
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct ArmSummary {
    pub assigned: usize,
    pub delivered: usize,
    pub missing: usize,
    pub pending: usize,
    pub observed: usize,
    pub correct: usize,
    pub wilson_95: Option<(f64, f64)>,
}

pub fn summarize(decisions: &[Decision], action: Action) -> ArmSummary {
    let mut report = ArmSummary {
        assigned: 0,
        delivered: 0,
        missing: 0,
        pending: 0,
        observed: 0,
        correct: 0,
        wilson_95: None,
    };
    for decision in decisions.iter().filter(|d| d.assigned == action) {
        report.assigned += 1;
        if matches!(decision.delivery, Delivery::Delivered { .. }) {
            report.delivered += 1;
        }
        match decision.outcome {
            Outcome::IndependentCheck { correct, .. } => {
                report.observed += 1;
                if correct {
                    report.correct += 1;
                }
            }
            Outcome::Missing => report.missing += 1,
            Outcome::Pending => report.pending += 1,
        }
    }
    if report.observed > 0 {
        let n = report.observed as f64;
        let p = report.correct as f64 / n;
        let z = 1.96_f64;
        let center = (p + z * z / (2.0 * n)) / (1.0 + z * z / n);
        let half = z * ((p * (1.0 - p) + z * z / (4.0 * n)) / n).sqrt() / (1.0 + z * z / n);
        report.wilson_95 = Some(((center - half).max(0.0), (center + half).min(1.0)));
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> (Protocol, Eligibility) {
        (
            Protocol {
                id: "test-only".into(),
                version: "1".into(),
                outcome_window_seconds: 100,
                suggestion_probability: 0.5,
                minimum_interval_seconds: 10,
                approved: true,
            },
            Eligibility {
                consent: true,
                outside_session_consent: false,
                in_session: true,
                reliable_context: true,
                interfering_window: false,
                verified_error_event_id: Some("verified-event".into()),
                last_decision_at: None,
                unit_id: "unit".into(),
                active_session_id: Some("session".into()),
            },
        )
    }
    #[test]
    fn off_unreliable_and_outside_without_separate_consent_are_ineligible() {
        let (p, mut e) = config();
        e.consent = false;
        assert!(assign("d".into(), 100, p.clone(), e.clone(), 0).is_err());
        e.consent = true;
        e.in_session = false;
        assert!(assign("d".into(), 100, p.clone(), e.clone(), 0).is_err());
        e.in_session = true;
        e.reliable_context = false;
        assert!(assign("d".into(), 100, p, e, 0).is_err());
    }
    #[test]
    fn silence_is_a_real_control_not_a_rejected_offer() {
        let (p, e) = config();
        let mut d = assign("d".into(), 100, p, e, u64::MAX).unwrap();
        assert_eq!(d.assigned, Action::DoNothing);
        assert_eq!(d.delivery, Delivery::NotApplicableControl);
        assert!(d.delivered(101, true).is_err());
        d.independent_outcome("check".into(), 110, true).unwrap();
        let summary = summarize(&[d], Action::DoNothing);
        assert_eq!(summary.observed, 1);
        assert_eq!(summary.delivered, 0);
        assert!(summary.wilson_95.unwrap().0 < 0.5);
    }
    #[test]
    fn revocation_between_assignment_and_delivery_cancels_delivery() {
        let (p, e) = config();
        let mut d = assign("d".into(), 100, p, e, 0).unwrap();
        d.delivered(101, false).unwrap();
        assert_eq!(d.delivery, Delivery::CancelledConsentRevoked);
        assert!(d.independent_outcome("check".into(), 201, true).is_err());
        assert_eq!(summarize(&[d], Action::SuggestDiagnostic).pending, 1);
    }

    fn checked_task(store: &mut crate::store::Store, id: &str, at: i64, answer: &str) {
        use crate::assessment::{Answer, Component, Task};
        let task = Task {
            id: id.into(),
            source: "test source".into(),
            criterion_version: "exact-v1".into(),
            created_at: at - 1,
            components: vec![Component {
                unit_id: "unit".into(),
                question: "2+2".into(),
                expected: "4".into(),
                target_confirmed: true,
            }],
        };
        store.create_task(&task).unwrap();
        store
            .submit(id, vec![Answer::Text(answer.into())], at)
            .unwrap();
    }

    fn runtime() -> (tempfile::TempDir, crate::store::Store) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::store::Store::open(dir.path().join("db.sqlite3")).unwrap();
        store
            .save(
                "unit",
                "unit",
                0,
                &crate::learning::MemoryUnit {
                    subject_id: "subject".into(),
                    description: "addition".into(),
                    self_assessment: None,
                    members: vec![],
                    source_operations: vec![],
                },
                1,
            )
            .unwrap();
        let mut session = crate::learning::Session::new(
            "subject".into(),
            crate::learning::Plan {
                objective: "addition".into(),
                purpose: "test".into(),
                prerequisites: "known".into(),
                success_criteria: "exact sum".into(),
                steps: vec![],
                gaps: vec![],
            },
            String::new(),
        )
        .unwrap();
        session.resume(1).unwrap();
        store.save("session", "session", 0, &session, 1).unwrap();
        (dir, store)
    }

    #[test]
    fn persisted_consent_rate_limits_outcomes_and_missing_are_auditable() {
        let (_dir, mut store) = runtime();
        checked_task(&mut store, "off", 100, "5");
        assert!(
            store
                .consider_mrt("off", true, false, 100)
                .unwrap()
                .is_none()
        );
        store
            .save_mrt_consent(
                &Consent {
                    enabled: true,
                    outside_session: false,
                },
                101,
            )
            .unwrap();
        checked_task(&mut store, "first", 200, "5");
        assert!(
            store
                .consider_mrt("first", false, false, 200)
                .unwrap()
                .is_none()
        );
        let decision = store
            .consider_mrt("first", true, false, 200)
            .unwrap()
            .unwrap();
        assert!(
            store
                .consider_mrt("first", true, false, 200)
                .unwrap()
                .is_none()
        );
        checked_task(&mut store, "outcome", 210, "4");
        store.observe_mrt("outcome", 210).unwrap();
        let restored = store
            .get::<Decision>("mrt_decision", &decision.id)
            .unwrap()
            .unwrap()
            .value;
        assert!(matches!(
            restored.outcome,
            Outcome::IndependentCheck { correct: true, .. }
        ));
        checked_task(&mut store, "limited", 220, "5");
        assert!(
            store
                .consider_mrt("limited", true, false, 220)
                .unwrap()
                .is_none()
        );
        for (id, at) in [("second", 1100), ("third", 2000)] {
            checked_task(&mut store, id, at, "5");
            assert!(store.consider_mrt(id, true, false, at).unwrap().is_some());
        }
        checked_task(&mut store, "daily-limit", 2900, "5");
        assert!(
            store
                .consider_mrt("daily-limit", true, false, 2900)
                .unwrap()
                .is_none()
        );
        let report = store.mrt_report(90000).unwrap();
        assert_eq!(report["decisions"].as_array().unwrap().len(), 3);
        let missing = report["suggestion"]["missing"].as_u64().unwrap()
            + report["control"]["missing"].as_u64().unwrap();
        // Later stored independent errors are outcomes too, even if no
        // observe callback ran for them before report generation.
        assert_eq!(missing, 0);
        assert_eq!(store.review_count().unwrap(), 0);
    }

    #[test]
    fn revoked_pending_delivery_does_not_call_a_desktop_or_change_fsrs() {
        let (dir, mut store) = runtime();
        let (p, e) = config();
        let mut decision = assign("pending".into(), 100, p, e, 0).unwrap();
        decision.eligibility.in_session = true;
        store
            .save("mrt_decision", "pending", 0, &decision, 100)
            .unwrap();
        deliver_pending(&dir.path().join("db.sqlite3"), "pending", 101).unwrap();
        assert_eq!(
            store
                .get::<Decision>("mrt_decision", "pending")
                .unwrap()
                .unwrap()
                .value
                .delivery,
            Delivery::CancelledConsentRevoked
        );
        assert_eq!(store.review_count().unwrap(), 0);
    }

    #[test]
    fn simultaneous_candidates_cannot_race_the_frequency_limit() {
        let (dir, mut store) = runtime();
        store
            .save_mrt_consent(
                &Consent {
                    enabled: true,
                    outside_session: false,
                },
                1,
            )
            .unwrap();
        checked_task(&mut store, "a", 100, "5");
        checked_task(&mut store, "b", 100, "5");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = ["a", "b"]
            .into_iter()
            .map(|id| {
                let path = dir.path().join("db.sqlite3");
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut store = crate::store::Store::open(path).unwrap();
                    barrier.wait();
                    store.consider_mrt(id, true, false, 100).unwrap().is_some()
                })
            })
            .collect::<Vec<_>>();
        let assigned = handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>();
        assert_eq!(assigned, 1);
        assert_eq!(store.list::<Decision>("mrt_decision").unwrap().len(), 1);
    }

    #[test]
    fn late_processing_uses_stored_outcome_time_before_expiration() {
        let (_dir, mut store) = runtime();
        store
            .save_mrt_consent(
                &Consent {
                    enabled: true,
                    outside_session: false,
                },
                1,
            )
            .unwrap();
        checked_task(&mut store, "error", 100, "5");
        let decision = store
            .consider_mrt("error", true, false, 100)
            .unwrap()
            .unwrap();
        checked_task(&mut store, "valid-in-window", 200, "4");
        store.mrt_report(86501).unwrap();
        store.observe_mrt("valid-in-window", 86502).unwrap();
        let outcome = store
            .get::<Decision>("mrt_decision", &decision.id)
            .unwrap()
            .unwrap()
            .value
            .outcome;
        assert!(matches!(
            outcome,
            Outcome::IndependentCheck {
                at: 200,
                correct: true,
                ..
            }
        ));
    }

    #[test]
    fn ending_session_cancels_pending_inside_only_delivery() {
        let (dir, mut store) = runtime();
        store
            .save_mrt_consent(
                &Consent {
                    enabled: true,
                    outside_session: false,
                },
                1,
            )
            .unwrap();
        let (p, e) = config();
        let decision = assign("pending-ended".into(), 100, p, e, 0).unwrap();
        store
            .save("mrt_decision", "pending-ended", 0, &decision, 100)
            .unwrap();
        let mut session = store
            .get::<crate::learning::Session>("session", "session")
            .unwrap()
            .unwrap();
        session.value.complete(101);
        store
            .save("session", "session", session.revision, &session.value, 101)
            .unwrap();
        deliver_pending(&dir.path().join("db.sqlite3"), "pending-ended", 102).unwrap();
        assert_eq!(
            store
                .get::<Decision>("mrt_decision", "pending-ended")
                .unwrap()
                .unwrap()
                .value
                .delivery,
            Delivery::CancelledConsentRevoked
        );
    }
}
