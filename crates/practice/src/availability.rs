//! Which Alt+T routes can be drawn now (F-18 step 2, B64). The answer comes
//! from the real stores, never from a guess:
//! - no selected text → probes, method choice, Feynman and hypothesis are out;
//! - no saved unfinished task → «продолжить задачу» is out;
//! - no old fragment in the archive → «старый материал» is out;
//! - no admitted unit → «умение» is out;
//! - the reader's own switches (Q-49) are out of the draw only.
//!
//! Context check (F-18 step 3, B69): stored material is offered only when the
//! window the reader is in now is the window it came from. Unknown on either
//! side behaves as before. A route held back this way is *deferred*: it leaves
//! the random draw with a reason, but a manual choice still opens it.
//!
//! `routes::pick` and `routes::shares` renormalise what is left.

use std::path::Path;

use cognitive_exoskeleton_journal::Journal;
use serde::Serialize;

use crate::revisit::{self, Fragment};
use crate::routes::Route;
use crate::skill::{self, Candidate, Refused};
use crate::switches::RouteSwitches;
use crate::tasks::{SavedTask, TaskStore};

/// Why a route cannot be drawn (F-18 step 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "why", rename_all = "snake_case")]
pub enum Unavailable {
    NoMaterial,
    NoSavedTask,
    NoArchivedFragment,
    NoAdmittedSkill {
        refused: usize,
    },
    /// A store could not be read: the route is left out rather than faked.
    StoreError {
        message: String,
    },
}

impl Unavailable {
    /// One line for the window and the CLI.
    pub fn reason(&self) -> String {
        match self {
            Unavailable::NoMaterial => "Выдели материал — без текста этого маршрута нет.".into(),
            Unavailable::NoSavedTask => {
                "Незавершённых задач нет: отложенная «Собери вместе» появится здесь.".into()
            }
            Unavailable::NoArchivedFragment => {
                "В архиве нет отрывка старше получаса, короче 11 000 знаков.".into()
            }
            Unavailable::NoAdmittedSkill { refused: 0 } => {
                "Умений нет: они появятся из уроков Alt+I.".into()
            }
            Unavailable::NoAdmittedSkill { refused } => {
                format!(
                    "Сейчас ни одно умение не допущено ({refused} не подошли): проверено кодом."
                )
            }
            Unavailable::StoreError { message } => format!("Не прочитать данные: {message}"),
        }
    }
}

/// The window of the Alt+T press, from the capture that already happens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Window {
    class: Option<String>,
}

impl Window {
    /// No capture, no class: nothing is known and nothing is held back.
    pub fn unknown() -> Self {
        Self::default()
    }

    pub fn known(class: Option<&str>) -> Self {
        Self {
            class: class
                .map(|c| c.trim().to_lowercase())
                .filter(|c| !c.is_empty()),
        }
    }

    /// Does stored material from `stored` belong to this window? `None` when
    /// either side is unknown. The title is not compared: it changes with every
    /// page and tab, the class does not.
    pub fn same_as(&self, stored: Option<&str>) -> Option<bool> {
        let now = self.class.as_deref()?;
        let then = stored.map(|c| c.trim().to_lowercase())?;
        if then.is_empty() {
            return None;
        }
        Some(now == then)
    }
}

/// Why a route is held back from the draw although its stock exists (B69).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "why", rename_all = "snake_case")]
pub enum Deferred {
    /// Every open task was made in another window.
    TaskElsewhere { made_in: String },
    /// Every archived fragment came from another window.
    FragmentsElsewhere { now_in: String },
}

impl Deferred {
    pub fn reason(&self) -> String {
        match self {
            Deferred::TaskElsewhere { made_in } => format!(
                "Контекст изменился: задача начата в окне «{made_in}». Вернись туда или выбери маршрут вручную."
            ),
            Deferred::FragmentsElsewhere { now_in } => format!(
                "Контекст изменился: отрывки в архиве не из окна «{now_in}». Выбери маршрут вручную, если нужно."
            ),
        }
    }
}

/// What the stores hold that the routes need.
#[derive(Debug, Default)]
pub struct Stock {
    pub task: Option<SavedTask>,
    pub open_tasks: usize,
    pub fragments: Vec<Fragment>,
    pub skills: Vec<Candidate>,
    pub refused: Vec<Refused>,
}

#[derive(Debug, Default)]
pub struct Availability {
    pub stock: Stock,
    /// Routes that cannot be drawn, with the reason.
    pub unavailable: Vec<(Route, Unavailable)>,
    /// Routes held back because the context changed (F-18 step 3, B69).
    pub deferred: Vec<(Route, Deferred)>,
    /// Routes the reader switched off (Q-49).
    pub switched_off: Vec<Route>,
}

impl Availability {
    /// The routes `routes::pick` must leave out: unavailable and switched off.
    pub fn excluded(&self) -> Vec<Route> {
        let mut out: Vec<Route> = self.unavailable.iter().map(|(route, _)| *route).collect();
        let held = self.deferred.iter().map(|(route, _)| route);
        for route in self.switched_off.iter().chain(held) {
            if !out.contains(route) {
                out.push(*route);
            }
        }
        out
    }

    pub fn deferred_reason(&self, route: Route) -> Option<&Deferred> {
        self.deferred
            .iter()
            .find(|(r, _)| *r == route)
            .map(|(_, why)| why)
    }

    pub fn reason(&self, route: Route) -> Option<&Unavailable> {
        self.unavailable
            .iter()
            .find(|(r, _)| *r == route)
            .map(|(_, why)| why)
    }
}

/// Reads the stores. `text` is the selection, if any; the window is unknown.
pub fn compute(db: impl AsRef<Path>, now: i64, text: &str) -> Availability {
    compute_in(db, now, text, &Window::unknown())
}

/// Reads the stores and checks the context against `window` (B69).
pub fn compute_in(db: impl AsRef<Path>, now: i64, text: &str, window: &Window) -> Availability {
    let db = db.as_ref();
    let mut out = Availability::default();
    if text.trim().is_empty() {
        for route in [
            Route::Probes,
            Route::ChooseMethod,
            Route::Feynman,
            Route::Hypothesis,
        ] {
            out.unavailable.push((route, Unavailable::NoMaterial));
        }
    }
    out.switched_off = RouteSwitches::open(db)
        .and_then(|s| s.off())
        .unwrap_or_default();

    let failed = |e: &dyn std::fmt::Display| Unavailable::StoreError {
        message: e.to_string(),
    };

    match TaskStore::open(db).and_then(|s| s.list_open()) {
        Ok(tasks) if !tasks.is_empty() => {
            out.stock.open_tasks = tasks.len();
            // The task of this window first; none of it, the last one worked on
            // stays for a manual choice and the route leaves the draw.
            let here = tasks
                .iter()
                .position(|t| window.same_as(t.window_class.as_deref()) != Some(false));
            let index = here.unwrap_or(0);
            if here.is_none() {
                let made_in = tasks[0].window_class.clone().unwrap_or_default();
                out.deferred
                    .push((Route::ContinueTask, Deferred::TaskElsewhere { made_in }));
            }
            out.stock.task = tasks.into_iter().nth(index);
        }
        Ok(_) => out
            .unavailable
            .push((Route::ContinueTask, Unavailable::NoSavedTask)),
        Err(e) => out.unavailable.push((Route::ContinueTask, failed(&e))),
    }

    match Journal::open(db).and_then(|j| revisit::fragments(&j, now, text)) {
        Ok(fragments) if !fragments.is_empty() => {
            let here: Vec<Fragment> = fragments
                .iter()
                .filter(|f| window.same_as(f.application.as_deref()) != Some(false))
                .cloned()
                .collect();
            if here.is_empty() {
                let now_in = window.class.clone().unwrap_or_default();
                out.deferred
                    .push((Route::OldMaterial, Deferred::FragmentsElsewhere { now_in }));
                out.stock.fragments = fragments;
            } else {
                out.stock.fragments = here;
            }
        }
        Ok(_) => out
            .unavailable
            .push((Route::OldMaterial, Unavailable::NoArchivedFragment)),
        Err(e) => out.unavailable.push((Route::OldMaterial, failed(&e))),
    }

    match skill::survey(db, now) {
        Ok(survey) if !survey.admitted.is_empty() => {
            out.stock.skills = survey.admitted;
            out.stock.refused = survey.refused;
        }
        Ok(survey) => {
            out.unavailable.push((
                Route::Skill,
                Unavailable::NoAdmittedSkill {
                    refused: survey.refused.len(),
                },
            ));
            out.stock.refused = survey.refused;
        }
        Err(e) => out.unavailable.push((Route::Skill, failed(&e))),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::NewTask;

    struct Db(std::path::PathBuf);
    impl Drop for Db {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }

    fn db() -> Db {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Db(std::env::temp_dir().join(format!("cp-avail-{}-{nanos}.db", std::process::id())))
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn an_empty_database_without_text_leaves_every_route_out_with_a_reason() {
        let db = db();
        let availability = compute(&db.0, NOW, "");
        assert_eq!(availability.excluded().len(), 7);
        assert_eq!(
            availability.reason(Route::Probes),
            Some(&Unavailable::NoMaterial)
        );
        assert_eq!(
            availability.reason(Route::ContinueTask),
            Some(&Unavailable::NoSavedTask)
        );
        assert_eq!(
            availability.reason(Route::OldMaterial),
            Some(&Unavailable::NoArchivedFragment)
        );
        assert!(matches!(
            availability.reason(Route::Skill),
            Some(Unavailable::NoAdmittedSkill { refused: 0 })
        ));
        for (_, why) in &availability.unavailable {
            assert!(!why.reason().is_empty());
        }
    }

    #[test]
    fn text_brings_back_the_text_routes_and_a_saved_task_its_own() {
        let db = db();
        let mut tasks = TaskStore::open(&db.0).unwrap();
        tasks
            .save(
                "k",
                &NewTask {
                    type_id: 1,
                    type_title: "Отработка метода".into(),
                    operations: vec![],
                    condition: "Требуется: x".into(),
                    solution: "x".into(),
                },
                "черновик",
                NOW - 100,
            )
            .unwrap();
        let availability = compute(&db.0, NOW, "Метод Ньютона сходится быстро.");
        let left: Vec<Route> = crate::routes::shares(&availability.excluded())
            .into_iter()
            .map(|(r, _)| r)
            .collect();
        assert_eq!(
            left,
            vec![
                Route::Probes,
                Route::ChooseMethod,
                Route::Feynman,
                Route::Hypothesis,
                Route::ContinueTask
            ]
        );
        assert_eq!(availability.stock.task.unwrap().draft, "черновик");
    }

    #[test]
    fn a_switch_takes_a_route_out_of_the_draw_but_not_out_of_the_reasons() {
        let db = db();
        RouteSwitches::open(&db.0)
            .unwrap()
            .set_off(Route::Feynman, true, NOW)
            .unwrap();
        let availability = compute(&db.0, NOW, "текст");
        assert!(availability.excluded().contains(&Route::Feynman));
        assert_eq!(
            availability.reason(Route::Feynman),
            None,
            "still pickable by hand"
        );
        // 100 rolls never pick it (Q-49)
        for i in 0..100 {
            let pick = crate::routes::pick(f64::from(i) / 100.0, &availability.excluded());
            assert_ne!(pick, Some(Route::Feynman));
        }
    }

    fn save(db: &Db, key: &str, class: Option<&str>, at: i64) {
        TaskStore::open(&db.0)
            .unwrap()
            .save_in(
                key,
                &NewTask {
                    type_id: 1,
                    type_title: "Отработка метода".into(),
                    operations: vec![],
                    condition: "Требуется: x".into(),
                    solution: "x".into(),
                },
                "черновик",
                at,
                class,
            )
            .unwrap();
    }

    #[test]
    fn b69_a_task_from_another_window_leaves_the_draw_with_a_reason_but_stays_for_hand() {
        let db = db();
        save(&db, "a", Some("Okular"), NOW - 100);
        let here = compute_in(&db.0, NOW, "текст", &Window::known(Some("okular")));
        assert!(here.deferred.is_empty(), "unchanged context: as before");
        assert!(!here.excluded().contains(&Route::ContinueTask));

        let away = compute_in(&db.0, NOW, "текст", &Window::known(Some("firefox")));
        assert!(away.excluded().contains(&Route::ContinueTask));
        assert!(
            away.reason(Route::ContinueTask).is_none(),
            "not out for hand"
        );
        let why = away.deferred_reason(Route::ContinueTask).unwrap();
        assert!(why.reason().starts_with("Контекст изменился"));
        assert!(away.stock.task.is_some(), "a manual choice still opens it");
    }

    #[test]
    fn b69_the_open_task_of_this_window_is_found_behind_a_newer_one_from_elsewhere() {
        let db = db();
        save(&db, "a", Some("okular"), NOW - 200);
        save(&db, "b", Some("firefox"), NOW - 100);
        let at = compute_in(&db.0, NOW, "", &Window::known(Some("okular")));
        assert!(at.deferred.is_empty());
        assert_eq!(
            at.stock.task.unwrap().window_class.as_deref(),
            Some("okular")
        );
    }

    #[test]
    fn b69_unknown_context_on_either_side_behaves_as_before() {
        let db = db();
        save(&db, "old", None, NOW - 100);
        let known_now = compute_in(&db.0, NOW, "текст", &Window::known(Some("firefox")));
        assert!(known_now.deferred.is_empty(), "the task has no window");
        save(&db, "new", Some("okular"), NOW - 50);
        for unknown in [
            Window::unknown(),
            Window::known(None),
            Window::known(Some("  ")),
        ] {
            let a = compute_in(&db.0, NOW, "текст", &unknown);
            assert!(a.deferred.is_empty());
            assert!(!a.excluded().contains(&Route::ContinueTask));
        }
        assert_eq!(Window::known(Some("A")).same_as(None), None);
        assert_eq!(Window::known(Some("A")).same_as(Some("a")), Some(true));
        assert_eq!(Window::known(Some("A")).same_as(Some("b")), Some(false));
    }
}
