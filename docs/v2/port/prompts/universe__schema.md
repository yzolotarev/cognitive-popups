# universe.py: SCHEMA

- Источник: `src/cognitive_popups/universe.py`
- Статус переноса: **PORT**

```text

CREATE TABLE IF NOT EXISTS thoughts (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    text          TEXT NOT NULL,
    anchor        TEXT,
    verdict       TEXT,
    delta         TEXT,
    created_utc   TEXT NOT NULL,
    hidden        INTEGER NOT NULL DEFAULT 0,
    topic         TEXT,
    about_system  INTEGER,
    extracted_utc TEXT
);
CREATE TABLE IF NOT EXISTS thought_concepts (
    thought_id TEXT NOT NULL REFERENCES thoughts(id) ON DELETE CASCADE,
    concept    TEXT NOT NULL,
    PRIMARY KEY (thought_id, concept)
);
CREATE INDEX IF NOT EXISTS universe_concept_idx ON thought_concepts(concept);
CREATE TABLE IF NOT EXISTS links (
    a          TEXT NOT NULL,
    relation   TEXT NOT NULL,
    b          TEXT NOT NULL,
    thought_id TEXT NOT NULL REFERENCES thoughts(id) ON DELETE CASCADE,
    PRIMARY KEY (a, b, thought_id)
);
CREATE TABLE IF NOT EXISTS aliases (
    alias     TEXT PRIMARY KEY,
    canonical TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS passages (
    hash        TEXT PRIMARY KEY,
    concepts    TEXT NOT NULL,
    created_utc TEXT NOT NULL
);

```
