//! Guards the P0 requirement registry against drift from the specification.
//!
//! The specification is versioned in `docs/spec/`, the traceability table in
//! `docs/requirements/registry.md`. Every requirement ID from the specification
//! must appear exactly once with the same status. New decisions may add rows,
//! but they must not silently restate a requirement.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

const SPEC: &str = "docs/spec/cognitive_exoskeleton_v0_2_full_spec.md";
const REGISTRY: &str = "docs/requirements/registry.md";

const PREFIXES: [&str; 7] = ["PR-", "CT-", "LI-", "MM-", "EX-", "DS-", "INV-"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

fn requirement_id(cell: &str) -> Option<String> {
    let candidate = cell.split_whitespace().next()?;
    PREFIXES
        .iter()
        .any(|prefix| candidate.starts_with(prefix))
        .then(|| candidate.to_owned())
}

struct Row {
    id: String,
    status: String,
}

fn requirement_rows(markdown: &str, registry_layout: bool) -> Vec<Row> {
    markdown
        .lines()
        .filter(|line| line.starts_with('|'))
        .filter_map(|line| {
            let cells = cells(line);
            if cells.len() < 3 {
                return None;
            }
            let id = requirement_id(&cells[0])?;
            if registry_layout {
                return Some(Row {
                    id,
                    status: cells[2].clone(),
                });
            }
            if id.starts_with("INV-") {
                // INV table: ID | test | related requirements | stage
                Some(Row {
                    id,
                    status: "ПРИНЯТО".to_owned(),
                })
            } else {
                Some(Row {
                    id,
                    status: cells[1].clone(),
                })
            }
        })
        .collect()
}

fn read(relative: &str) -> String {
    fs::read_to_string(root().join(relative))
        .unwrap_or_else(|error| panic!("cannot read {relative}: {error}"))
}

#[test]
fn every_spec_requirement_is_registered_with_the_same_status() {
    let spec = read(SPEC);
    let registry = read(REGISTRY);

    let mut expected = BTreeMap::new();
    for row in requirement_rows(&spec, false) {
        assert!(
            expected
                .insert(row.id.clone(), row.status.to_owned())
                .is_none(),
            "duplicate requirement {} in specification",
            row.id
        );
    }

    let mut found = BTreeMap::new();
    for row in requirement_rows(&registry, true) {
        assert!(
            found
                .insert(row.id.clone(), row.status.to_owned())
                .is_none(),
            "duplicate requirement {} in registry",
            row.id
        );
    }

    let missing: Vec<&String> = expected
        .keys()
        .filter(|id| !found.contains_key(*id))
        .collect();
    let extra: Vec<&String> = found
        .keys()
        .filter(|id| !expected.contains_key(*id))
        .collect();
    assert!(
        missing.is_empty(),
        "requirements missing from registry: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "registry contains IDs absent from specification: {extra:?}"
    );

    for (id, status) in &expected {
        assert_eq!(
            found.get(id),
            Some(status),
            "status of {id} differs from the specification"
        );
    }
}

#[test]
fn invariants_and_stages_are_complete() {
    let registry = read(REGISTRY);
    let ids: BTreeSet<String> = requirement_rows(&registry, true)
        .into_iter()
        .map(|row| row.id)
        .collect();

    for number in 1..=12 {
        let id = format!("INV-{number:02}");
        assert!(ids.contains(&id), "missing obligatory invariant {id}");
    }

    for prefix in ["PR-", "CT-", "LI-", "MM-", "EX-", "DS-"] {
        assert!(
            ids.iter().any(|id| id.starts_with(prefix)),
            "no registered requirements for {prefix}"
        );
    }
}

#[test]
fn every_registry_row_declares_a_known_state() {
    let registry = read(REGISTRY);
    let header = registry
        .lines()
        .find(|line| line.starts_with("| ID | Источник | Статус | Этап |"))
        .expect("registry header is missing");
    let state_column = cells(header)
        .iter()
        .position(|cell| cell == "Состояние")
        .expect("registry has no state column");

    let known = [
        "не начато",
        "в работе",
        "ждёт решения",
        "реализовано",
        "принято на desktop",
        "отклонено автором",
    ];

    for line in registry.lines().filter(|line| line.starts_with('|')) {
        let cells = cells(line);
        if requirement_id(&cells[0]).is_none() || cells.len() <= state_column {
            continue;
        }
        let state = &cells[state_column];
        assert!(
            known.contains(&state.as_str()),
            "row {} has unknown state {state:?}",
            cells[0]
        );
    }
}
