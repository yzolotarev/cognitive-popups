//! Which Alt+T routes can be drawn now (F-18 step 2, B64). The answer comes
//! from the real stores, never from a guess:
//! - no selected text → probes, method choice, Feynman and hypothesis are out;
//! - no saved unfinished task → «продолжить задачу» is out;
//! - no old fragment in the archive → «старый материал» is out;
//! - no admitted unit → «умение» is out;
//! - the reader's own switches (Q-49) are out of the draw only.
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
    /// Routes the reader switched off (Q-49).
    pub switched_off: Vec<Route>,
}

impl Availability {
    /// The routes `routes::pick` must leave out: unavailable and switched off.
    pub fn excluded(&self) -> Vec<Route> {
        let mut out: Vec<Route> = self.unavailable.iter().map(|(route, _)| *route).collect();
        for route in &self.switched_off {
            if !out.contains(route) {
                out.push(*route);
            }
        }
        out
    }

    pub fn reason(&self, route: Route) -> Option<&Unavailable> {
        self.unavailable
            .iter()
            .find(|(r, _)| *r == route)
            .map(|(_, why)| why)
    }
}

/// Reads the stores. `text` is the selection, if any.
pub fn compute(db: impl AsRef<Path>, now: i64, text: &str) -> Availability {
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

    match TaskStore::open(db).and_then(|s| Ok((s.latest_open()?, s.count_open()?))) {
        Ok((Some(task), open)) => {
            out.stock.task = Some(task);
            out.stock.open_tasks = open;
        }
        Ok((None, _)) => out
            .unavailable
            .push((Route::ContinueTask, Unavailable::NoSavedTask)),
        Err(e) => out.unavailable.push((Route::ContinueTask, failed(&e))),
    }

    match Journal::open(db).and_then(|j| revisit::fragments(&j, now, text)) {
        Ok(mut fragments) if !fragments.is_empty() => {
            // Where the tie cannot be read, the route works as without it.
            let _ = revisit::link_to_skills(db, &mut fragments);
            out.stock.fragments = fragments;
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
}
