//! Routes the reader switched off (Q-49, B64): a personal setting can only
//! take a route out of the random choice of Alt+T; it never adds weight to
//! another one (`routes::shares` renormalises). The reader's manual choice in
//! the window stays possible: «выключен» means «не выпадает», not «запрещён».

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::routes::Route;

#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
}

/// What a goal says about one route (B68).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalSwitch {
    /// «Как у всех»: the global switch decides.
    Inherit,
    /// The goal draws the route even if the global switch is off.
    On,
    /// The goal never draws the route.
    Off,
}

impl GoalSwitch {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inherit" => Some(Self::Inherit),
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

pub struct RouteSwitches {
    connection: Connection,
}

impl RouteSwitches {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SwitchError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, SwitchError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, SwitchError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS practice_route_switches (
                 route TEXT PRIMARY KEY,
                 off   INTEGER NOT NULL,
                 at    INTEGER NOT NULL
             );
             -- B68: a goal may override the global switch for one route. No row
             -- means «как у всех»; added next to the global table, nothing moves.
             CREATE TABLE IF NOT EXISTS practice_goal_route_switches (
                 goal_id TEXT NOT NULL,
                 route   TEXT NOT NULL,
                 off     INTEGER NOT NULL,
                 at      INTEGER NOT NULL,
                 PRIMARY KEY (goal_id, route)
             );",
        )?;
        Ok(Self { connection })
    }

    pub fn set_off(&mut self, route: Route, off: bool, now: i64) -> Result<(), SwitchError> {
        self.connection.execute(
            "INSERT INTO practice_route_switches (route, off, at) VALUES (?1, ?2, ?3)
             ON CONFLICT (route) DO UPDATE SET off = ?2, at = ?3",
            params![route.name(), off, now],
        )?;
        Ok(())
    }

    /// Sets the goal's own state for a route; `Inherit` removes the override.
    pub fn set_goal(
        &mut self,
        goal_id: &str,
        route: Route,
        state: GoalSwitch,
        now: i64,
    ) -> Result<(), SwitchError> {
        match state {
            GoalSwitch::Inherit => {
                self.connection.execute(
                    "DELETE FROM practice_goal_route_switches WHERE goal_id = ?1 AND route = ?2",
                    params![goal_id, route.name()],
                )?;
            }
            GoalSwitch::On | GoalSwitch::Off => {
                self.connection.execute(
                    "INSERT INTO practice_goal_route_switches (goal_id, route, off, at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (goal_id, route) DO UPDATE SET off = ?3, at = ?4",
                    params![goal_id, route.name(), state == GoalSwitch::Off, now],
                )?;
            }
        }
        Ok(())
    }

    pub fn goal_state(&self, goal_id: &str, route: Route) -> Result<GoalSwitch, SwitchError> {
        let off: Option<bool> = self
            .connection
            .query_row(
                "SELECT off FROM practice_goal_route_switches WHERE goal_id = ?1 AND route = ?2",
                params![goal_id, route.name()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match off {
            None => GoalSwitch::Inherit,
            Some(true) => GoalSwitch::Off,
            Some(false) => GoalSwitch::On,
        })
    }

    /// The routes out of the draw for a goal: its own word where it has one,
    /// the global switch otherwise. `None` (material of no known goal) is global.
    pub fn off_for(&self, goal_id: Option<&str>) -> Result<Vec<Route>, SwitchError> {
        let global = self.off()?;
        let Some(goal_id) = goal_id else {
            return Ok(global);
        };
        let mut out = Vec::new();
        for route in Route::ALL {
            let off = match self.goal_state(goal_id, route)? {
                GoalSwitch::Inherit => global.contains(&route),
                GoalSwitch::On => false,
                GoalSwitch::Off => true,
            };
            if off {
                out.push(route);
            }
        }
        Ok(out)
    }

    pub fn is_off(&self, route: Route) -> Result<bool, SwitchError> {
        Ok(self.off()?.contains(&route))
    }

    /// The routes switched off, in the order of `Route::ALL`.
    pub fn off(&self) -> Result<Vec<Route>, SwitchError> {
        let mut statement = self
            .connection
            .prepare("SELECT route FROM practice_route_switches WHERE off = 1")?;
        let names: Vec<String> = statement
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(Route::ALL
            .into_iter()
            .filter(|route| names.iter().any(|n| n == route.name()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes;

    #[test]
    fn nothing_is_off_until_the_reader_says_so_and_it_can_be_undone() {
        let mut switches = RouteSwitches::open_in_memory().unwrap();
        assert!(switches.off().unwrap().is_empty());
        switches.set_off(Route::Skill, true, 1).unwrap();
        switches.set_off(Route::Feynman, true, 2).unwrap();
        assert_eq!(switches.off().unwrap(), vec![Route::Feynman, Route::Skill]);
        assert!(switches.is_off(Route::Skill).unwrap());
        switches.set_off(Route::Skill, false, 3).unwrap();
        assert_eq!(switches.off().unwrap(), vec![Route::Feynman]);
    }

    #[test]
    fn q49_switching_off_only_removes_weight() {
        let mut switches = RouteSwitches::open_in_memory().unwrap();
        switches.set_off(Route::Hypothesis, true, 1).unwrap();
        let off = switches.off().unwrap();
        let shares = routes::shares(&off);
        assert!(shares.iter().all(|(r, _)| *r != Route::Hypothesis));
        let total: f64 = shares.iter().map(|(_, s)| s).sum();
        assert!((total - 100.0).abs() < 1e-9);
        // every other route got more, none got less
        for (route, share) in shares {
            let before = routes::shares(&[])
                .into_iter()
                .find(|(r, _)| *r == route)
                .unwrap()
                .1;
            assert!(share > before);
        }
    }

    #[test]
    fn b68_a_goal_inherits_the_global_switches_until_it_says_otherwise() {
        let mut s = RouteSwitches::open_in_memory().unwrap();
        s.set_off(Route::Skill, true, 1).unwrap();
        assert_eq!(
            s.goal_state("g1", Route::Skill).unwrap(),
            GoalSwitch::Inherit
        );
        assert_eq!(s.off_for(Some("g1")).unwrap(), vec![Route::Skill]);
        assert_eq!(s.off_for(None).unwrap(), vec![Route::Skill]);
        // a later global change reaches a goal that inherits
        s.set_off(Route::Feynman, true, 2).unwrap();
        assert_eq!(
            s.off_for(Some("g1")).unwrap(),
            vec![Route::Feynman, Route::Skill]
        );
    }

    #[test]
    fn b68_a_goal_override_beats_the_global_one_only_for_that_goal() {
        let mut s = RouteSwitches::open_in_memory().unwrap();
        s.set_off(Route::Skill, true, 1).unwrap();
        s.set_goal("g1", Route::Skill, GoalSwitch::On, 2).unwrap();
        s.set_goal("g1", Route::Hypothesis, GoalSwitch::Off, 3)
            .unwrap();
        assert_eq!(s.off_for(Some("g1")).unwrap(), vec![Route::Hypothesis]);
        assert_eq!(s.off_for(Some("g2")).unwrap(), vec![Route::Skill]);
        assert_eq!(s.off_for(None).unwrap(), vec![Route::Skill]);
        s.set_goal("g1", Route::Skill, GoalSwitch::Inherit, 4)
            .unwrap();
        assert_eq!(
            s.goal_state("g1", Route::Skill).unwrap(),
            GoalSwitch::Inherit
        );
        assert_eq!(
            s.off_for(Some("g1")).unwrap(),
            vec![Route::Hypothesis, Route::Skill]
        );
    }

    #[test]
    fn b68_a_goal_switch_only_removes_weight_from_the_others() {
        let mut s = RouteSwitches::open_in_memory().unwrap();
        s.set_goal("g1", Route::Feynman, GoalSwitch::Off, 1)
            .unwrap();
        let shares = routes::shares(&s.off_for(Some("g1")).unwrap());
        assert!(shares.iter().all(|(r, _)| *r != Route::Feynman));
        let before = routes::shares(&[]);
        for (route, share) in shares {
            let was = before.iter().find(|(r, _)| *r == route).unwrap().1;
            assert!(share > was);
        }
    }

    #[test]
    fn b68_an_existing_database_keeps_its_global_switches() {
        let path = std::env::temp_dir().join(format!("cp-sw-mig-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE practice_route_switches (
                     route TEXT PRIMARY KEY, off INTEGER NOT NULL, at INTEGER NOT NULL);
                 INSERT INTO practice_route_switches VALUES ('skill', 1, 5);",
            )
            .unwrap();
        }
        let mut s = RouteSwitches::open(&path).unwrap();
        assert_eq!(s.off().unwrap(), vec![Route::Skill]);
        assert_eq!(s.off_for(Some("g")).unwrap(), vec![Route::Skill]);
        s.set_goal("g", Route::Skill, GoalSwitch::On, 6).unwrap();
        assert!(s.off_for(Some("g")).unwrap().is_empty());
        assert_eq!(s.off().unwrap(), vec![Route::Skill]);
        drop(s);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn route_names_round_trip() {
        for route in Route::ALL {
            assert_eq!(Route::parse(route.name()), Some(route));
        }
        assert_eq!(Route::parse("tasks"), None);
    }
}
