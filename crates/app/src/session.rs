//! The independent session window (Alt+I).
//!
//! The product treats a session as its own surface: a timer, a plan the user
//! owns, and the transition between draft → active → paused → completed. It is
//! not a page of the control panel: the panel is an auxiliary surface for
//! subjects, memory, checks, experiment and settings.

use domain::OperationId;
use services::{
    learning::{PLAN_PROMPT, Plan, Session, SessionStatus, Step, Subject},
    provider::Provider,
    store::{Record, Store},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};
use time::OffsetDateTime;
use ui::SessionWindow;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn strings(values: impl IntoIterator<Item = String>) -> ModelRc<slint::SharedString> {
    Rc::new(VecModel::from(
        values.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
    .into()
}
fn session_status_ru(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Paused => "пауза",
        SessionStatus::Active => "идёт",
        SessionStatus::Completed => "завершено",
    }
}

/// Plan drafts belong to the session surface; the control panel keeps its own
/// draft for captured material and the user's own note.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Draft {
    subject_id: Option<String>,
    session_id: Option<String>,
    objective: String,
    purpose: String,
    prerequisites: String,
    criteria: String,
    steps: String,
    results: String,
}

struct Controller {
    store: Store,
    workspace_path: std::path::PathBuf,
    journal_path: std::path::PathBuf,
    subjects: Vec<Record<Subject>>,
    sessions: Vec<Record<Session>>,
    subject: Option<Record<Subject>>,
    session: Option<Record<Session>>,
    draft_revision: i64,
    last_draft: Option<Draft>,
    last_outbox_sync: i64,
}

impl Controller {
    fn open(directory: &std::path::Path) -> Result<Self> {
        let workspace_path = directory.join("workspace.sqlite3");
        let mut store = Store::open(&workspace_path)?;
        let journal_path = directory.join("journal.sqlite3");
        let _ = store.flush_journal(&journal_path);
        Ok(Self {
            store,
            workspace_path,
            journal_path,
            subjects: vec![],
            sessions: vec![],
            subject: None,
            session: None,
            draft_revision: 0,
            last_draft: None,
            last_outbox_sync: 0,
        })
    }

    fn provider(&self, kind: &str) -> Result<Provider> {
        let record = self
            .store
            .get::<Provider>("provider", kind)?
            .or(self.store.get("provider", "default")?);
        Ok(record.map(|r| r.value).unwrap_or_default())
    }

    fn refresh(&mut self, ui: &SessionWindow) -> Result<()> {
        self.subjects = self.store.list("subject")?;
        self.sessions = self.store.list("session")?;
        ui.set_subject_names(strings(self.subjects.iter().map(|r| r.value.name.clone())));
        ui.set_session_names(strings(self.sessions.iter().map(|r| {
            let objective = r.value.plan.objective.trim();
            if objective.is_empty() {
                session_status_ru(r.value.status).to_string()
            } else {
                format!("{} · {objective}", session_status_ru(r.value.status))
            }
        })));
        if let Some(index) = self.subject.as_ref().and_then(|current| {
            self.subjects
                .iter()
                .position(|record| record.id == current.id)
        }) {
            ui.set_subject_index(index as i32);
        }
        // A draft that was just generated opens under the timer, so the user
        // reviews it where the plan belongs instead of hunting in the list.
        let pending = ui.get_pending_session().to_string();
        if !pending.is_empty() {
            ui.set_pending_session("".into());
            if let Some(index) = self.sessions.iter().position(|record| record.id == pending) {
                self.select_session(ui, index as i32)?;
            }
        }
        if let Some(index) = self.session.as_ref().and_then(|current| {
            self.sessions
                .iter()
                .position(|record| record.id == current.id)
        }) {
            ui.set_session_index(index as i32);
        }
        let goal = self
            .subject
            .as_ref()
            .map(|record| record.value.goal.clone())
            .unwrap_or_default();
        ui.set_subject_goal(if goal.trim().is_empty() {
            "Личная цель предмета не задана. Заполните её в панели управления → «Предметы».".into()
        } else {
            format!("Личная цель: {goal}").into()
        });
        let plan = self.provider("plan")?;
        ui.set_provider_label(
            format!(
                "План по кнопке: {} / {} · черновик ИИ нужно проверить и принять самому",
                plan.endpoint, plan.model
            )
            .into(),
        );
        Ok(())
    }

    fn draft(&self, ui: &SessionWindow) -> Draft {
        Draft {
            subject_id: self.subject.as_ref().map(|r| r.id.clone()),
            session_id: self.session.as_ref().map(|r| r.id.clone()),
            objective: ui.get_objective().to_string(),
            purpose: ui.get_purpose().to_string(),
            prerequisites: ui.get_prerequisites().to_string(),
            criteria: ui.get_criteria().to_string(),
            steps: ui.get_steps().to_string(),
            results: ui.get_results().to_string(),
        }
    }

    fn checkpoint(&mut self, ui: &SessionWindow) -> Result<()> {
        if now().saturating_sub(self.last_outbox_sync) >= 30 {
            self.last_outbox_sync = now();
            let _ = self.store.flush_journal(&self.journal_path);
        }
        let draft = self.draft(ui);
        if self.last_draft.as_ref() != Some(&draft) {
            self.draft_revision =
                self.store
                    .save("draft", "session", self.draft_revision, &draft, now())?;
            self.last_draft = Some(draft);
        }
        Ok(())
    }

    fn restore_draft(&mut self, ui: &SessionWindow) -> Result<()> {
        if let Some(record) = self.store.get::<Draft>("draft", "session")? {
            self.draft_revision = record.revision;
            let draft = record.value;
            if let Some(index) = self
                .subjects
                .iter()
                .position(|r| Some(&r.id) == draft.subject_id.as_ref())
            {
                self.select_subject(ui, index as i32)?;
                ui.set_subject_index(index as i32);
            }
            if let Some(index) = self
                .sessions
                .iter()
                .position(|r| Some(&r.id) == draft.session_id.as_ref())
            {
                self.select_session(ui, index as i32)?;
                ui.set_session_index(index as i32);
            }
            ui.set_objective(draft.objective.clone().into());
            ui.set_purpose(draft.purpose.clone().into());
            ui.set_prerequisites(draft.prerequisites.clone().into());
            ui.set_criteria(draft.criteria.clone().into());
            ui.set_steps(draft.steps.clone().into());
            ui.set_results(draft.results.clone().into());
            self.last_draft = Some(draft);
        }
        Ok(())
    }

    /// Keep the surface useful on open: one subject is selected implicitly, and
    /// an interrupted session is offered instead of an empty plan.
    fn initial_selection(&mut self, ui: &SessionWindow) -> Result<()> {
        if self.subject.is_none() && self.subjects.len() == 1 {
            self.select_subject(ui, 0)?;
            ui.set_subject_index(0);
        }
        if self.session.is_none() {
            let interrupted = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, record)| record.value.status == SessionStatus::Active)
                .map(|(index, _)| index)
                .next_back();
            let paused = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, record)| record.value.status == SessionStatus::Paused)
                .map(|(index, _)| index)
                .next_back();
            if let Some(index) = interrupted.or(paused) {
                self.select_session(ui, index as i32)?;
                ui.set_session_index(index as i32);
            }
        }
        Ok(())
    }

    fn select_subject(&mut self, ui: &SessionWindow, index: i32) -> Result<()> {
        let record = self
            .subjects
            .get(index as usize)
            .ok_or("Выберите предмет")?
            .clone();
        ui.set_subject_goal(if record.value.goal.trim().is_empty() {
            "Личная цель предмета не задана. Заполните её в панели управления → «Предметы».".into()
        } else {
            format!("Личная цель: {}", record.value.goal).into()
        });
        self.subject = Some(record);
        Ok(())
    }

    fn new_plan(&mut self, ui: &SessionWindow) -> Result<()> {
        let subject = self
            .subject
            .clone()
            .ok_or("Выберите предмет с личной целью в панели управления → «Предметы»")?;
        if self.session.is_some() {
            self.action(ui, "pause")?;
        }
        let value = Session {
            subject_id: subject.id.clone(),
            plan: Plan {
                objective: String::new(),
                purpose: subject.value.goal.clone(),
                prerequisites: "Подготовка неизвестна; уточнить перед началом".into(),
                success_criteria: String::new(),
                steps: vec![],
                gaps: vec![],
            },
            status: SessionStatus::Paused,
            elapsed_seconds: 0,
            active_since: None,
            own_results: String::new(),
            material: String::new(),
        };
        ui.set_objective("".into());
        ui.set_purpose(value.plan.purpose.clone().into());
        ui.set_prerequisites(value.plan.prerequisites.clone().into());
        ui.set_criteria("".into());
        ui.set_steps("".into());
        ui.set_results("".into());
        ui.set_gaps("".into());
        let session_id = id();
        let revision = self.store.save("session", &session_id, 0, &value, now())?;
        self.session = Some(Record {
            id: session_id,
            revision,
            value,
        });
        self.refresh(ui)?;
        self.tick(ui);
        self.checkpoint(ui)?;
        ui.set_status(
            "Собственный план: заполните цель и критерии, затем сохраните. ИИ не требуется.".into(),
        );
        Ok(())
    }

    fn generate_plan(&mut self, ui: &SessionWindow) -> Result<()> {
        let subject = self
            .subject
            .clone()
            .ok_or("Сначала выберите предмет с личной целью")?;
        let provider = self.provider("plan")?;
        provider.validate()?;
        let sessions: Vec<_> = self
            .sessions
            .iter()
            .filter(|r| r.value.subject_id == subject.id)
            .map(|r| &r.value)
            .collect();
        let context = serde_json::to_string(
            &serde_json::json!({"subject":subject.value,"past_sessions":sessions}),
        )?;
        let path = self.workspace_path.clone();
        let journal_path = self.journal_path.clone();
        let operation = OperationId::new(id())?;
        // Persist the exact planning request before provider failure can occur.
        self.store
            .save("planning_request", operation.as_str(), 0, &context, now())?;
        let weak = ui.as_weak();
        ui.set_busy(true);
        ui.set_status("Генерируется черновик плана. Текущий план не перезаписывается.".into());
        std::thread::spawn(move || {
            let result = (|| -> Result<String> {
                let text = provider.complete(PLAN_PROMPT, &context)?;
                Store::open(&path)?.record_model_response(
                    &journal_path,
                    operation,
                    &provider.endpoint,
                    &provider.model,
                    &text,
                )?;
                let plan: Plan = services::json::extract(&text).map_err(
                    |_| "Ответ модели не соответствует JSON плана; текущие данные сохранены",
                )?;
                let mut session = Session::new(subject.id, plan, String::new())?;
                // Generated checkmarks cannot stand in for the user's work.
                for step in &mut session.plan.steps {
                    step.worked = false;
                }
                let draft = id();
                Store::open(path)?.save("session", &draft, 0, &session, now())?;
                Ok(draft)
            })();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_busy(false);
                match result {
                    Ok(draft) => {
                        ui.set_pending_session(draft.into());
                        ui.invoke_refresh();
                        ui.set_status(
                            "Черновик ИИ открыт под таймером: проверьте цель и критерии, отредактируйте и начните явно.".into(),
                        );
                    }
                    Err(error) => ui.set_status(error.to_string().into()),
                }
            });
        });
        Ok(())
    }

    fn select_session(&mut self, ui: &SessionWindow, index: i32) -> Result<()> {
        let selected = self
            .sessions
            .get(index as usize)
            .ok_or("Выберите занятие")?
            .clone();
        if self
            .session
            .as_ref()
            .is_some_and(|record| record.id == selected.id)
        {
            return Ok(());
        }
        if self.session.is_some() {
            self.action(ui, "pause")?;
        }
        let mut record = self
            .store
            .get::<Session>("session", &selected.id)?
            .ok_or("Занятие не найдено")?;
        if record.value.status == SessionStatus::Active {
            record.value.recover();
            record.revision =
                self.store
                    .save("session", &record.id, record.revision, &record.value, now())?;
        }
        ui.set_objective(record.value.plan.objective.clone().into());
        ui.set_purpose(record.value.plan.purpose.clone().into());
        ui.set_prerequisites(record.value.plan.prerequisites.clone().into());
        ui.set_criteria(record.value.plan.success_criteria.clone().into());
        ui.set_steps(
            record
                .value
                .plan
                .steps
                .iter()
                .map(|s| format!("[{}] {}", if s.worked { "x" } else { " " }, s.text))
                .collect::<Vec<_>>()
                .join("\n")
                .into(),
        );
        ui.set_gaps(
            record
                .value
                .plan
                .gaps
                .iter()
                .map(|g| {
                    format!(
                        "{}: {} (основание: {})",
                        if g.blocking {
                            "Отдельная работа?"
                        } else {
                            "Релевантный пробел?"
                        },
                        g.description,
                        g.basis
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
                .into(),
        );
        ui.set_results(record.value.own_results.clone().into());
        self.session = Some(record);
        self.tick(ui);
        Ok(())
    }

    fn save_plan(&mut self, ui: &SessionWindow) -> Result<()> {
        let record = self.session.as_mut().ok_or("Выберите занятие из списка")?;
        let mut value = record.value.clone();
        value.plan.objective = ui.get_objective().to_string();
        value.plan.purpose = ui.get_purpose().to_string();
        value.plan.prerequisites = ui.get_prerequisites().to_string();
        value.plan.success_criteria = ui.get_criteria().to_string();
        value.plan.steps = ui
            .get_steps()
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(|s| Step {
                text: s
                    .strip_prefix("[x] ")
                    .or_else(|| s.strip_prefix("[ ] "))
                    .unwrap_or(s)
                    .to_owned(),
                worked: s.starts_with("[x] "),
            })
            .collect();
        value.own_results = ui.get_results().to_string();
        // Incomplete edits are durable drafts too. Only starting/completing a
        // plan requires all accepted planning fields to be present.
        if value.status == SessionStatus::Active {
            value.pause(now());
            value.resume(now())?;
        }
        record.revision = self
            .store
            .save("session", &record.id, record.revision, &value, now())?;
        record.value = value;
        self.refresh(ui)?;
        self.tick(ui);
        ui.set_status("План и собственные результаты сохранены. Это не FSRS-оценка.".into());
        Ok(())
    }

    fn action(&mut self, ui: &SessionWindow, action: &str) -> Result<()> {
        self.save_plan(ui)?;
        let record = self.session.as_mut().ok_or("Выберите занятие")?;
        let mut value = record.value.clone();
        match action {
            "resume" => {
                value.plan.validate()?;
                value.resume(now())?;
            }
            "complete" => {
                value.plan.validate()?;
                value.complete(now());
            }
            _ => value.pause(now()),
        }
        record.revision = self
            .store
            .save("session", &record.id, record.revision, &value, now())?;
        record.value = value;
        self.refresh(ui)?;
        self.tick(ui);
        Ok(())
    }

    fn tick(&self, ui: &SessionWindow) {
        if let Some(record) = &self.session {
            let elapsed = record.value.elapsed(now());
            let remaining = (900 - elapsed).max(0);
            ui.set_timer(
                format!(
                    "{:02}:{:02} · прошло {} мин",
                    remaining / 60,
                    remaining % 60,
                    elapsed / 60
                )
                .into(),
            );
            ui.set_session_state(
                format!(
                    "{}{}",
                    session_status_ru(record.value.status),
                    if elapsed >= 900 {
                        " · ориентир времени истёк; завершение только вручную"
                    } else {
                        ""
                    }
                )
                .into(),
            );
        }
    }
}

pub fn run() -> Result<()> {
    let _instance = services::InstanceGuard::acquire("session")?;
    let ui = SessionWindow::new()?;
    let controller = Rc::new(RefCell::new(Controller::open(&services::data_dir()?)?));
    controller.borrow_mut().refresh(&ui)?;
    controller.borrow_mut().restore_draft(&ui)?;
    controller.borrow_mut().initial_selection(&ui)?;
    // Selecting a session may recover it from Active to Paused, so the list is
    // rebuilt to keep the selector and the status line in agreement.
    controller.borrow_mut().refresh(&ui)?;
    controller.borrow().tick(&ui);

    macro_rules! wire {
        ($callback:ident, $method:ident) => {{
            let state = controller.clone();
            let weak = ui.as_weak();
            ui.$callback(move || {
                if let Some(ui) = weak.upgrade() {
                    if let Err(error) = state.borrow_mut().$method(&ui) {
                        ui.set_status(error.to_string().into());
                    }
                }
            });
        }};
        ($callback:ident, $method:ident, arg) => {{
            let state = controller.clone();
            let weak = ui.as_weak();
            ui.$callback(move |arg| {
                if let Some(ui) = weak.upgrade() {
                    if let Err(error) = state.borrow_mut().$method(&ui, arg) {
                        ui.set_status(error.to_string().into());
                    }
                }
            });
        }};
    }
    wire!(on_select_subject, select_subject, arg);
    wire!(on_new_session, new_plan);
    wire!(on_generate_plan, generate_plan);
    wire!(on_save_session, save_plan);
    wire!(on_select_session, select_session, arg);
    wire!(on_refresh, refresh);
    {
        let state = controller.clone();
        let weak = ui.as_weak();
        ui.on_draft(move || {
            if let Some(ui) = weak.upgrade()
                && let Err(error) = state.borrow_mut().checkpoint(&ui)
            {
                ui.set_status(format!("Черновик не сохранён: {error}").into());
            }
        });
    }
    {
        let state = controller.clone();
        let weak = ui.as_weak();
        ui.on_session_action(move |action| {
            if let Some(ui) = weak.upgrade()
                && let Err(error) = state.borrow_mut().action(&ui, &action)
            {
                ui.set_status(error.to_string().into());
            }
        });
    }
    let timer = slint::Timer::default();
    let state = controller.clone();
    let weak = ui.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(1),
        move || {
            if let Some(ui) = weak.upgrade() {
                state.borrow().tick(&ui);
                if let Err(error) = state.borrow_mut().checkpoint(&ui) {
                    ui.set_status(format!("Черновик не сохранён: {error}").into());
                }
            }
        },
    );
    ui.run()?;
    controller.borrow_mut().checkpoint(&ui)?;
    // Closing the window pauses the lesson without pretending it ended.
    if controller.borrow().session.is_some() {
        controller.borrow_mut().action(&ui, "pause")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn setup() -> (tempfile::TempDir, Controller, SessionWindow) {
        i_slint_backend_testing::init_no_event_loop();
        let dir = tempfile::tempdir().unwrap();
        let mut controller = Controller::open(dir.path()).unwrap();
        let ui = SessionWindow::new().unwrap();
        controller.refresh(&ui).unwrap();
        let subject = Subject {
            name: "Математика".into(),
            goal: "Считать маржу без ошибок".into(),
            context: String::new(),
            preferences: Default::default(),
        };
        let subject_id = id();
        controller
            .store
            .save("subject", &subject_id, 0, &subject, now())
            .unwrap();
        controller.refresh(&ui).unwrap();
        controller.select_subject(&ui, 0).unwrap();
        (dir, controller, ui)
    }

    #[test]
    fn session_plan_cannot_start_until_objective_and_criteria_exist() {
        let (_dir, mut controller, ui) = setup();
        controller.new_plan(&ui).unwrap();
        assert!(controller.action(&ui, "resume").is_err());
        ui.set_objective("Различать маржу и наценку".into());
        ui.set_criteria("Объяснить разницу своими словами".into());
        controller.action(&ui, "resume").unwrap();
        assert_eq!(
            controller.session.as_ref().unwrap().value.status,
            SessionStatus::Active
        );
        assert_eq!(ui.get_session_state(), "идёт");
        assert!(ui.get_session_names().row_data(0).unwrap().contains("идёт"));
        controller.action(&ui, "complete").unwrap();
        assert_eq!(ui.get_session_state(), "завершено");
        assert_eq!(controller.store.review_count().unwrap(), 0);
    }

    #[test]
    fn a_generated_draft_opens_under_the_timer_without_hiding_in_the_list() {
        let (_dir, mut controller, ui) = setup();
        let subject_id = controller.subject.as_ref().unwrap().id.clone();
        let draft_id = id();
        let draft = Session::new(
            subject_id,
            Plan {
                objective: "Различать маржу и наценку".into(),
                purpose: "Считать маржу без ошибок".into(),
                prerequisites: "Нет".into(),
                success_criteria: "Объяснить разницу".into(),
                steps: vec![Step {
                    text: "Прочитать исходник".into(),
                    worked: false,
                }],
                gaps: vec![],
            },
            String::new(),
        )
        .unwrap();
        controller
            .store
            .save("session", &draft_id, 0, &draft, now())
            .unwrap();
        controller.refresh(&ui).unwrap();
        assert!(controller.session.is_none());
        ui.set_pending_session(draft_id.clone().into());
        controller.refresh(&ui).unwrap();
        assert_eq!(controller.session.as_ref().unwrap().id, draft_id);
        assert_eq!(ui.get_objective(), "Различать маржу и наценку");
        assert_eq!(ui.get_pending_session(), "");
    }

    #[test]
    fn session_restart_recovers_to_paused_with_the_plan_intact() {
        let (dir, mut controller, ui) = setup();
        controller.new_plan(&ui).unwrap();
        ui.set_objective("Различать маржу и наценку".into());
        ui.set_criteria("Самостоятельно выбрать знаменатель".into());
        ui.set_steps("[x] Прочитать исходник\n[ ] Проверить пример".into());
        controller.save_plan(&ui).unwrap();
        controller.action(&ui, "resume").unwrap();
        let session_id = controller.session.as_ref().unwrap().id.clone();
        controller.checkpoint(&ui).unwrap();
        drop(controller);

        let mut restored = Controller::open(dir.path()).unwrap();
        restored.refresh(&ui).unwrap();
        restored.restore_draft(&ui).unwrap();
        restored.initial_selection(&ui).unwrap();
        restored.tick(&ui);
        let record = restored.session.as_ref().expect("recovered session");
        assert_eq!(record.id, session_id);
        assert_eq!(record.value.status, SessionStatus::Paused);
        assert_eq!(record.value.plan.objective, "Различать маржу и наценку");
        assert_eq!(ui.get_objective(), "Различать маржу и наценку");
        assert_eq!(ui.get_session_state(), "пауза");
    }
}
