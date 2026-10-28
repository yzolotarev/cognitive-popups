//! Alt+T routes (S5 rule 9): what Alt+T opens when the reader does not choose.
//! v1 weights without «целая задача» and «повторение» (both are categories
//! now), normalised to 100. A personal profile may only switch a route off
//! (Q-49); it never adds weight to another one.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Probes,
    ChooseMethod,
    Feynman,
    Hypothesis,
    ContinueTask,
    OldMaterial,
    Skill,
}

/// v1 weights: probes 70, choose method 5, Feynman 5, hypothesis 3,
/// continue task 2, old material 2, skill 2.
pub const WEIGHTS: [(Route, u32); 7] = [
    (Route::Probes, 70),
    (Route::ChooseMethod, 5),
    (Route::Feynman, 5),
    (Route::Hypothesis, 3),
    (Route::ContinueTask, 2),
    (Route::OldMaterial, 2),
    (Route::Skill, 2),
];

/// Shares in percent after switching routes off; the rest keep their proportions.
pub fn shares(disabled: &[Route]) -> Vec<(Route, f64)> {
    let active: Vec<(Route, u32)> = WEIGHTS
        .iter()
        .copied()
        .filter(|(route, _)| !disabled.contains(route))
        .collect();
    let total: u32 = active.iter().map(|(_, w)| w).sum();
    active
        .into_iter()
        .map(|(route, w)| (route, f64::from(w) * 100.0 / f64::from(total.max(1))))
        .collect()
}

/// The route for a roll in `[0, 1)`; None when every route is off.
pub fn pick(roll: f64, disabled: &[Route]) -> Option<Route> {
    let shares = shares(disabled);
    let mut edge = 0.0;
    for (route, share) in &shares {
        edge += share / 100.0;
        if roll < edge {
            return Some(*route);
        }
    }
    shares.last().map(|(route, _)| *route)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_are_normalised_and_probes_dominate() {
        let shares = shares(&[]);
        let total: f64 = shares.iter().map(|(_, s)| s).sum();
        assert!((total - 100.0).abs() < 1e-9);
        assert!((shares[0].1 - 70.0 / 89.0 * 100.0).abs() < 1e-9);
        assert_eq!(pick(0.0, &[]), Some(Route::Probes));
        assert_eq!(pick(0.999, &[]), Some(Route::Skill));
    }

    #[test]
    fn a_profile_can_only_switch_routes_off() {
        let off = [Route::Feynman, Route::Hypothesis];
        let shares = shares(&off);
        assert!(shares.iter().all(|(r, _)| !off.contains(r)));
        let probes = shares.iter().find(|(r, _)| *r == Route::Probes).unwrap().1;
        assert!(
            (probes - 70.0 / 81.0 * 100.0).abs() < 1e-9,
            "the rest keep their proportions"
        );
        let all: Vec<Route> = WEIGHTS.iter().map(|(r, _)| *r).collect();
        assert_eq!(pick(0.5, &all), None);
    }
}
