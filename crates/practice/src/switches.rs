//! Routes the reader switched off (Q-49, B64): a personal setting can only
//! take a route out of the random choice of Alt+T; it never adds weight to
//! another one (`routes::shares` renormalises). The reader's manual choice in
//! the window stays possible: «выключен» means «не выпадает», not «запрещён».

use std::path::Path;

use rusqlite::{Connection, params};

use crate::routes::Route;

#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
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
    fn route_names_round_trip() {
        for route in Route::ALL {
            assert_eq!(Route::parse(route.name()), Some(route));
        }
        assert_eq!(Route::parse("tasks"), None);
    }
}
