# micro_clarify.py: _SCHEMA

- Источник: `src/cognitive_popups/micro_clarify.py`
- Статус переноса: **PORT**

```text

CREATE TABLE IF NOT EXISTS questions (
    question_id   TEXT PRIMARY KEY,
    created_epoch REAL NOT NULL,
    trigger       TEXT NOT NULL,
    mode          TEXT NOT NULL,
    arm           TEXT NOT NULL,
    uncertainty_type TEXT NOT NULL,
    question      TEXT NOT NULL,
    options_json  TEXT NOT NULL,
    decision_affected TEXT,
    reason        TEXT,
    evidence_type TEXT,
    source        TEXT NOT NULL DEFAULT 'template',
    impact_json   TEXT NOT NULL DEFAULT '{}',
    gate          TEXT NOT NULL,                 -- show / shadow / no_question
    gate_reasons_json TEXT NOT NULL DEFAULT '[]',
    session_key   TEXT, step_id TEXT, goal_id TEXT, fragment_id TEXT,
    decision_id   TEXT,                          -- the director decision it was asked about
    shown_epoch   REAL,
    answer_option TEXT,
    answered_epoch REAL,
    skipped_epoch REAL,
    followup_decision_id TEXT,
    followup_action TEXT,
    changed       INTEGER,
    original_action TEXT,
    context_json  TEXT NOT NULL DEFAULT '{}',
    policy_impact_json TEXT NOT NULL DEFAULT '{}',   -- option -> goal profile / recall / preferred order
    impact_basis  TEXT                               -- immediate / policy / both / none
);
CREATE INDEX IF NOT EXISTS questions_type_idx ON questions(uncertainty_type, created_epoch);
CREATE TABLE IF NOT EXISTS facts (
    fact_id       TEXT PRIMARY KEY,
    question_id   TEXT,
    kind          TEXT NOT NULL,                 -- explicit_fact / self_report
    key           TEXT NOT NULL,                 -- the uncertainty type
    value         TEXT NOT NULL,                 -- the option id
    scope         TEXT NOT NULL,                 -- goal / fragment / session / global
    scope_id      TEXT,
    created_epoch REAL NOT NULL,
    expires_epoch REAL,
    revoked_epoch REAL,
    detail_json   TEXT NOT NULL DEFAULT '{}'
);

```
