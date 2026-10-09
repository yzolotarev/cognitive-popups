"""Durable reading ledger: sessions, fragments (4 words), hypotheses, Feynman.

This is the on-disk projection of the live `CognitiveSession` plus its checks.
Unlike the telemetry log (`event_log.py`), losing a row here is a real loss: a
fragment or a hypothesis is the point of the exercise, not a side effect. The
write path therefore raises `RecordError` instead of swallowing failures, so the
caller can surface "not saved" instead of silently dropping the day.

Lifetimes, one store per kind:

    events.sqlite3   telemetry        append-only, may fail silently
    records.sqlite3  this ledger      durable, failures are visible
    notes.sqlite3    reader's words   sacred, never silent

None of the three is expired by age: the logs are kept and accumulated for years.
The three are joined by shared ids (`session_id`, `fragment_id`) rather than by
merging files, so a broken telemetry database cannot take the ledger or the
reader's words down with it.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .models import FeynmanCheck, Fragment, Intention, PredictionCheck, now_iso
from .notes import NoteStore, local_stamp, normalise_anchor

STATE_ROOT = os.environ.get("COGNITIVE_STATE_DIR") or "~/.local/state/cognitive-popups"
DEFAULT_DB = os.path.join(STATE_ROOT, "records.sqlite3")
ENV_DB = "COGNITIVE_RECORD_DB"

SCHEMA_VERSION = 8

_SCHEMA = """
CREATE TABLE IF NOT EXISTS sessions (
    id            TEXT PRIMARY KEY,
    title         TEXT,
    created_utc   TEXT NOT NULL,
    created_epoch REAL NOT NULL,
    closed_utc    TEXT,
    closed_epoch  REAL,
    close_reason  TEXT
);
CREATE TABLE IF NOT EXISTS fragments (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    position      INTEGER NOT NULL,
    source_text   TEXT,
    source_hash   TEXT,
    n_cues        INTEGER NOT NULL DEFAULT 4,
    created_utc   TEXT NOT NULL,
    created_epoch REAL NOT NULL,
    prompt_hash   TEXT,
    model         TEXT
);
CREATE TABLE IF NOT EXISTS cues (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    fragment_id   TEXT NOT NULL REFERENCES fragments(id) ON DELETE CASCADE,
    position      INTEGER NOT NULL,
    simple        TEXT,
    term          TEXT,
    meaning       TEXT
);
CREATE TABLE IF NOT EXISTS predictions (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    hypothesis    TEXT NOT NULL,
    status        TEXT,
    mismatch      TEXT,
    evidence      TEXT,
    one_delta     TEXT NOT NULL DEFAULT '',
    created_utc   TEXT NOT NULL,
    created_epoch REAL NOT NULL,
    prompt_hash   TEXT,
    model         TEXT
);
CREATE TABLE IF NOT EXISTS prediction_fragments (
    prediction_id TEXT NOT NULL REFERENCES predictions(id) ON DELETE CASCADE,
    fragment_id   TEXT NOT NULL,
    PRIMARY KEY (prediction_id, fragment_id)
);
CREATE TABLE IF NOT EXISTS feynman_checks (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    question      TEXT,
    explanation   TEXT,
    status        TEXT,
    follow_up     TEXT,
    created_utc   TEXT NOT NULL,
    created_epoch REAL NOT NULL,
    question_prompt_hash TEXT,
    check_prompt_hash    TEXT,
    model         TEXT
);
CREATE TABLE IF NOT EXISTS feynman_fragments (
    check_id      TEXT NOT NULL REFERENCES feynman_checks(id) ON DELETE CASCADE,
    fragment_id   TEXT NOT NULL,
    PRIMARY KEY (check_id, fragment_id)
);
CREATE TABLE IF NOT EXISTS feynman_gaps (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    check_id      TEXT NOT NULL REFERENCES feynman_checks(id) ON DELETE CASCADE,
    position      INTEGER NOT NULL,
    location      TEXT,
    type          TEXT,
    description   TEXT
);
CREATE TABLE IF NOT EXISTS tasks (
    id            TEXT PRIMARY KEY,
    session_id    TEXT,
    task_type     TEXT NOT NULL,
    task_subtype  TEXT,
    level         TEXT,
    context       TEXT,
    condition     TEXT NOT NULL,
    solution      TEXT,
    attempt       TEXT,
    status        TEXT NOT NULL DEFAULT 'generated',
    created_utc   TEXT NOT NULL,
    created_epoch REAL NOT NULL,
    prompt_hash   TEXT,
    model         TEXT,
    raw_response  TEXT,
    grounding_json TEXT,
    operations_json TEXT,
    validation_error TEXT,
    attempts      INTEGER NOT NULL DEFAULT 1,
    latency_ms    INTEGER NOT NULL DEFAULT 0,
    domain        TEXT,
    node_id       TEXT,
    node_type     TEXT,
    verification_status TEXT,
    verification_note   TEXT,
    payload_json  TEXT
);
CREATE TABLE IF NOT EXISTS practice_preparations (
    id            TEXT PRIMARY KEY,
    context_key   TEXT NOT NULL,
    context_json  TEXT NOT NULL DEFAULT '{}',
    status        TEXT NOT NULL,
    completed_stage INTEGER NOT NULL DEFAULT 0,
    stage_label   TEXT NOT NULL DEFAULT '',
    practice_kind TEXT,
    analysis_json TEXT NOT NULL DEFAULT '{}',
    task_id       TEXT,
    error_code    TEXT NOT NULL DEFAULT '',
    created_utc   TEXT NOT NULL,
    created_epoch REAL NOT NULL,
    updated_utc   TEXT NOT NULL,
    updated_epoch REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS preparations_context_idx
    ON practice_preparations(context_key);
CREATE INDEX IF NOT EXISTS preparations_epoch_idx
    ON practice_preparations(created_epoch);
CREATE TABLE IF NOT EXISTS steps (
    id            TEXT PRIMARY KEY,
    session_key   TEXT NOT NULL,
    text          TEXT NOT NULL,
    minutes       REAL NOT NULL,
    goal_id       TEXT,
    started_utc   TEXT NOT NULL,
    started_epoch REAL NOT NULL,
    deadline_epoch REAL NOT NULL,
    finished_utc  TEXT,
    finished_epoch REAL,
    outcome       TEXT,
    takeaway      TEXT
);
CREATE INDEX IF NOT EXISTS steps_started_idx ON steps(started_epoch);
CREATE TABLE IF NOT EXISTS step_closures (
    session_key   TEXT PRIMARY KEY,
    goal_id       TEXT,
    outcome       TEXT,
    remaining     TEXT,
    closed_utc    TEXT NOT NULL,
    closed_epoch  REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS intents (
    id            TEXT PRIMARY KEY,
    text          TEXT NOT NULL,
    criterion     TEXT,
    stopped_at    TEXT,
    source        TEXT,
    material      TEXT,
    material_origin TEXT,
    direction     TEXT,
    created_utc   TEXT NOT NULL,
    created_epoch REAL NOT NULL,
    updated_utc   TEXT,
    updated_epoch REAL,
    status        TEXT NOT NULL DEFAULT 'current'
);
CREATE INDEX IF NOT EXISTS records_sessions_epoch_idx ON sessions(created_epoch);
CREATE INDEX IF NOT EXISTS records_fragments_session_idx ON fragments(session_id, position);
CREATE INDEX IF NOT EXISTS records_fragments_hash_idx ON fragments(source_hash);
CREATE INDEX IF NOT EXISTS records_cues_fragment_idx ON cues(fragment_id, position);
CREATE INDEX IF NOT EXISTS records_predictions_session_idx ON predictions(session_id);
CREATE INDEX IF NOT EXISTS records_feynman_session_idx ON feynman_checks(session_id);
CREATE INDEX IF NOT EXISTS records_tasks_session_idx ON tasks(session_id, created_epoch);
CREATE INDEX IF NOT EXISTS records_intents_status_idx ON intents(status, created_epoch);
"""


class RecordError(RuntimeError):
    """Raised when a ledger write cannot be stored; never swallowed by the UI."""


#: Statuses a background preparation can be in. `OPEN` means work is in flight;
#: the rest are terminal and never move again. See `practice.py` for the
#: transitions and `hud_state.py` for what each one shows.
PREPARATION_OPEN = ("queued", "analyzing", "generating", "validating", "repairing")
PREPARATION_TERMINAL = ("ready", "insufficient", "failed", "superseded", "interrupted", "dismissed")
PREPARATION_STATUSES = PREPARATION_OPEN + PREPARATION_TERMINAL

#: Which columns `update_preparation` is allowed to touch. A whitelist, so a
#: caller cannot rewrite the context of a row that has already been worked on.
_PREPARATION_FIELDS = (
    "status", "completed_stage", "stage_label", "practice_kind", "analysis_json",
    "task_id", "error_code",
)


#: Columns added after the first release. `CREATE TABLE IF NOT EXISTS` cannot add
#: them to a database that already exists, so they are applied explicitly.
_ADDED_COLUMNS = (
    ("fragments", "prompt_hash", "TEXT"),
    ("fragments", "model", "TEXT"),
    ("predictions", "prompt_hash", "TEXT"),
    ("predictions", "model", "TEXT"),
    ("predictions", "subject_note", "TEXT"),
    ("predictions", "one_delta", "TEXT NOT NULL DEFAULT ''"),
    ("feynman_checks", "question_prompt_hash", "TEXT"),
    ("feynman_checks", "check_prompt_hash", "TEXT"),
    ("feynman_checks", "model", "TEXT"),
    ("tasks", "raw_response", "TEXT"),
    ("tasks", "grounding_json", "TEXT"),
    ("tasks", "operations_json", "TEXT"),
    ("tasks", "validation_error", "TEXT"),
    ("tasks", "attempts", "INTEGER NOT NULL DEFAULT 1"),
    ("tasks", "latency_ms", "INTEGER NOT NULL DEFAULT 0"),
    ("tasks", "task_subtype", "TEXT"),
    ("tasks", "domain", "TEXT"),
    ("tasks", "node_id", "TEXT"),
    ("tasks", "node_type", "TEXT"),
    ("tasks", "verification_status", "TEXT"),
    ("tasks", "verification_note", "TEXT"),
    ("tasks", "payload_json", "TEXT"),
    ("intents", "material", "TEXT"),
    ("intents", "material_origin", "TEXT"),
    ("intents", "direction", "TEXT"),
)


def _ensure_columns(conn: sqlite3.Connection) -> None:
    for table, column, kind in _ADDED_COLUMNS:
        have = {row["name"] for row in conn.execute(f"PRAGMA table_info({table})")}
        if column not in have:
            conn.execute(f"ALTER TABLE {table} ADD COLUMN {column} {kind}")


def resolve_db_path(path: str | Path | None = None) -> Path:
    raw = path or os.environ.get(ENV_DB) or DEFAULT_DB
    return Path(raw).expanduser()


def _epoch(iso: str | None) -> float:
    if iso:
        try:
            return datetime.fromisoformat(iso).timestamp()
        except ValueError:
            pass
    return time.time()


def local_date(ts_utc: str) -> str:
    try:
        return datetime.fromisoformat(ts_utc).astimezone().strftime("%Y-%m-%d")
    except ValueError:
        return str(ts_utc)[:10]


def _clock(ts_utc: str | None) -> str:
    if not ts_utc:
        return ""
    try:
        return datetime.fromisoformat(ts_utc).astimezone().strftime("%H:%M")
    except ValueError:
        return str(ts_utc)[:5]


def _fold(text: str | None) -> str:
    return normalise_anchor(text) or ""


def match_fragment(fragments: list[Fragment], anchor: str) -> Fragment | None:
    """Return the fragment a note is about, or None when nothing matches.

    A note owns its anchor, so the honest link is by content: the anchor appears
    in the fragment's source, or is one of its cues. Never "the last fragment".
    """
    key = _fold(anchor)
    if not key:
        return None
    for fragment in fragments:
        if key in _fold(fragment.source_text):
            return fragment
    for fragment in fragments:
        for detail in fragment.cue_details:
            for field in ("simple", "term", "meaning"):
                value = detail.get(field, "")
                if value and key in _fold(value):
                    return fragment
    return None


class RecordStore:
    """SQLite home for the reading ledger (see module docstring)."""

    def __init__(self, path: str | Path | None = None):
        self.path = resolve_db_path(path)
        self._schema_ready = False

    def _connect(self) -> sqlite3.Connection:
        try:
            conn = sqlite3.connect(self.path, timeout=3.0)
            conn.row_factory = sqlite3.Row
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("PRAGMA synchronous=NORMAL")
            conn.execute("PRAGMA foreign_keys=ON")
            if not self._schema_ready:
                self._backup_before_migration(conn)
                conn.executescript(_SCHEMA)
                _ensure_columns(conn)
                conn.execute(f"PRAGMA user_version={SCHEMA_VERSION}")
                self._schema_ready = True
            return conn
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot open {self.path}: {exc}") from exc

    def _backup_before_migration(self, conn: sqlite3.Connection) -> None:
        """Copy the ledger before a schema upgrade can touch it.

        `CREATE TABLE IF NOT EXISTS` only adds objects, but the copy is the one
        chance to preserve the previous shape. A database that is empty or
        already current needs no copy, and a fresh file is not a database yet.
        A failed copy stops the open instead of migrating unbacked-up data.
        """
        try:
            version = int(conn.execute("PRAGMA user_version").fetchone()[0] or 0)
            tables = int(conn.execute(
                "SELECT count(*) FROM sqlite_master WHERE type='table'"
            ).fetchone()[0])
        except (sqlite3.Error, TypeError, ValueError):
            return
        if version >= SCHEMA_VERSION or not tables:
            return
        try:
            from .observation import backup_database
            backup_database(self.path)
        except Exception as exc:  # noqa: BLE001 - migration must not proceed silently
            raise RecordError(f"cannot back up {self.path} before migration: {exc}") from exc

    def _ensure_session(self, conn: sqlite3.Connection, session) -> None:
        created_utc = getattr(session, "created_at", None) or now_iso()
        title = getattr(session, "title", None) or "Untitled session"
        conn.execute(
            "INSERT OR IGNORE INTO sessions (id, title, created_utc, created_epoch)"
            " VALUES (?, ?, ?, ?)",
            (session.id, title, created_utc, _epoch(created_utc)),
        )

    def open_session(self, session_id: str, *, title: str | None = None, created_utc: str | None = None) -> None:
        created_utc = created_utc or now_iso()
        conn = self._connect()
        try:
            conn.execute(
                "INSERT OR IGNORE INTO sessions (id, title, created_utc, created_epoch)"
                " VALUES (?, ?, ?, ?)",
                (session_id, title or "Untitled session", created_utc, _epoch(created_utc)),
            )
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def save_task(
        self,
        *,
        task_id: str,
        session_id: str | None,
        task_type: str,
        level: str,
        context: str,
        condition: str,
        task_subtype: str = "",
        solution: str = "",
        attempt: str = "",
        status: str = "generated",
        prompt_hash: str = "",
        model: str = "",
        raw_response: str = "",
        grounding_json: str = "[]",
        operations_json: str = "[]",
        validation_error: str = "",
        attempts: int = 1,
        latency_ms: int = 0,
        domain: str = "",
        node_id: str = "",
        node_type: str = "",
        verification_status: str = "",
        verification_note: str = "",
        payload_json: str = "{}",
        created_utc: str | None = None,
    ) -> None:
        """Persist the task as content, not only as an event containing its length."""
        created_utc = created_utc or now_iso()
        conn = self._connect()
        try:
            conn.execute(
                """INSERT OR REPLACE INTO tasks
                (id, session_id, task_type, task_subtype, level, context, condition, solution,
                 attempt, status, created_utc, created_epoch, prompt_hash, model,
                 raw_response, grounding_json, operations_json, validation_error,
                 attempts, latency_ms, domain, node_id, node_type,
                 verification_status, verification_note, payload_json)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                        ?, ?, ?, ?, ?, ?)""",
                (task_id, session_id, task_type, task_subtype, level, context, condition, solution,
                 attempt, status, created_utc, _epoch(created_utc), prompt_hash, model,
                 raw_response, grounding_json, operations_json, validation_error,
                 attempts, latency_ms, domain, node_id, node_type,
                 verification_status, verification_note, payload_json),
            )
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write task to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def load_tasks(self, session_id: str | None = None) -> list[dict[str, Any]]:
        """Oldest first. Ties on `created_epoch` fall back to `rowid`, not to
        `id`: two tasks saved in the same millisecond used to be ordered by a
        uuid, which is not a time, so "the latest task" was a coin toss.
        `INSERT OR REPLACE` gives a rewritten task a new rowid, so insertion
        order really is "most recently written last".
        """
        conn = self._connect()
        try:
            if session_id:
                rows = conn.execute(
                    "SELECT * FROM tasks WHERE session_id = ? ORDER BY created_epoch, rowid",
                    (session_id,),
                )
            else:
                rows = conn.execute("SELECT * FROM tasks ORDER BY created_epoch, rowid")
            return [dict(row) for row in rows]
        finally:
            conn.close()

    def revisit_sources(self) -> list[dict[str, Any]]:
        """Closed-session source snapshots and hypotheses linked to one source.

        Status/evidence are intentionally absent: a prediction verdict is not a
        readiness signal. A hypothesis with no saved source cannot be revisited
        as a source-grounded task.
        """
        conn = self._connect()
        try:
            rows = conn.execute(
                """SELECT 'f:' || f.id AS id, 'fragment' AS kind, f.source_text,
                          NULL AS hypothesis, f.created_utc, f.created_epoch,
                          s.title, f.id AS fragment_id
                   FROM fragments f JOIN sessions s ON s.id = f.session_id
                   WHERE s.closed_utc IS NOT NULL AND trim(f.source_text) != ''
                   UNION ALL
                   SELECT 'h:' || p.id || ':' || f.id AS id, 'hypothesis' AS kind,
                          f.source_text, p.hypothesis, p.created_utc, p.created_epoch,
                          s.title, f.id AS fragment_id
                   FROM predictions p
                   JOIN sessions s ON s.id = p.session_id
                   JOIN prediction_fragments pf ON pf.prediction_id = p.id
                   JOIN fragments f ON f.id = pf.fragment_id AND f.session_id = p.session_id
                   WHERE s.closed_utc IS NOT NULL AND trim(f.source_text) != ''
                         AND trim(p.hypothesis) != ''
                   ORDER BY 6 DESC, 1"""
            )
            return [dict(row) for row in rows]
        finally:
            conn.close()

    # ── background practice preparation (see practice.py) ────────────────────

    def save_preparation(
        self,
        *,
        preparation_id: str,
        context_key: str,
        status: str,
        context_json: str = "{}",
        analysis_json: str = "{}",
        completed_stage: int = 0,
        stage_label: str = "",
        practice_kind: str = "",
        task_id: str | None = None,
        error_code: str = "",
        created_utc: str | None = None,
    ) -> None:
        """Record one attempt to prepare practice from one exact context.

        One row per `context_key`, for good: this is what stops the same material
        from being sent to the model again on every save. A retry, when it is
        wanted, resets the row rather than adding a second one.
        """
        moment = created_utc or now_iso()
        epoch = _epoch(moment)
        conn = self._connect()
        try:
            conn.execute(
                "INSERT OR REPLACE INTO practice_preparations"
                " (id, context_key, context_json, status, completed_stage, stage_label,"
                "  practice_kind, analysis_json, task_id, error_code,"
                "  created_utc, created_epoch, updated_utc, updated_epoch)"
                " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (preparation_id, context_key, context_json, status, int(completed_stage),
                 stage_label, practice_kind or None, analysis_json, task_id,
                 error_code, moment, epoch, moment, epoch),
            )
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write preparation to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def update_preparation(self, preparation_id: str, **fields: Any) -> None:
        """Advance one preparation. Unknown field names are refused, not ignored."""
        unknown = set(fields) - set(_PREPARATION_FIELDS)
        if unknown:
            raise ValueError(f"unknown preparation field(s): {sorted(unknown)}")
        if not fields:
            return
        moment = now_iso()
        columns = [f"{name} = ?" for name in fields]
        values: list[Any] = [fields[name] for name in fields]
        columns.extend(["updated_utc = ?", "updated_epoch = ?"])
        values.extend([moment, _epoch(moment), preparation_id])
        conn = self._connect()
        try:
            cursor = conn.execute(
                "UPDATE practice_preparations SET " + ", ".join(columns) + " WHERE id = ?",
                tuple(values),
            )
            if not cursor.rowcount:
                raise ValueError(f"no preparation with id {preparation_id}")
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write preparation to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def preparation(self, preparation_id: str) -> dict[str, Any] | None:
        conn = self._connect()
        try:
            row = conn.execute(
                "SELECT * FROM practice_preparations WHERE id = ?", (preparation_id,)
            ).fetchone()
        finally:
            conn.close()
        return dict(row) if row is not None else None

    def preparation_by_context(self, context_key: str) -> dict[str, Any] | None:
        """The row that already covers this exact context, worked on or not."""
        conn = self._connect()
        try:
            row = conn.execute(
                "SELECT * FROM practice_preparations WHERE context_key = ? LIMIT 1",
                (context_key,),
            ).fetchone()
        finally:
            conn.close()
        return dict(row) if row is not None else None

    def latest_preparation(self) -> dict[str, Any] | None:
        """Newest first by insertion order, so the panel shows the current work."""
        conn = self._connect()
        try:
            row = conn.execute(
                "SELECT * FROM practice_preparations ORDER BY created_epoch, rowid DESC LIMIT 1"
            ).fetchone()
        finally:
            conn.close()
        return dict(row) if row is not None else None

    def preparations(self, *, status: str | None = None, tail: int | None = None) -> list[dict[str, Any]]:
        conn = self._connect()
        try:
            if status:
                rows = conn.execute(
                    "SELECT * FROM practice_preparations WHERE status = ?"
                    " ORDER BY created_epoch, rowid",
                    (status,),
                )
            else:
                rows = conn.execute(
                    "SELECT * FROM practice_preparations ORDER BY created_epoch, rowid"
                )
            result = [dict(row) for row in rows]
        finally:
            conn.close()
        return result[-tail:] if tail else result

    def interrupt_preparations(self, *, reason: str = "restart") -> int:
        """Close work that a restart cut in half.

        A preparation left `generating` by a stopped process is not still
        generating; saying so would show a live progress indicator for work that
        is not running. Returns how many rows were closed.
        """
        placeholders = ", ".join("?" for _ in PREPARATION_OPEN)
        moment = now_iso()
        conn = self._connect()
        try:
            cursor = conn.execute(
                "UPDATE practice_preparations SET status = 'interrupted', error_code = ?,"
                " updated_utc = ?, updated_epoch = ? WHERE status IN (" + placeholders + ")",
                (reason, moment, _epoch(moment), *PREPARATION_OPEN),
            )
            conn.commit()
            return int(cursor.rowcount or 0)
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write preparation to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def close_session(
        self,
        session_id: str,
        reason: str,
        *,
        closed_utc: str | None = None,
    ) -> None:
        """Close a session, optionally at a moment other than now.

        An idle split passes the last hotkey instead of the returning press, so
        the recorded span stays time spent rather than time elapsed.
        """
        moment = closed_utc or now_iso()
        conn = self._connect()
        try:
            conn.execute(
                "UPDATE sessions SET closed_utc = ?, closed_epoch = ?, close_reason = ?"
                " WHERE id = ? AND closed_utc IS NULL",
                (moment, _epoch(moment), reason, session_id),
            )
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def close_stale(self, reason: str = "crash") -> int:
        """Mark sessions left open by a previous run; return how many closed."""
        moment = now_iso()
        conn = self._connect()
        try:
            cursor = conn.execute(
                "UPDATE sessions SET closed_utc = ?, closed_epoch = ?, close_reason = ?"
                " WHERE closed_utc IS NULL",
                (moment, _epoch(moment), reason),
            )
            conn.commit()
            return int(cursor.rowcount or 0)
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def save_fragment(self, session, fragment: Fragment, *, position: int | None = None) -> None:
        if position is None:
            position = len(session.fragments)
        conn = self._connect()
        try:
            self._ensure_session(conn, session)
            conn.execute("DELETE FROM fragments WHERE id = ?", (fragment.id,))
            conn.execute(
                "INSERT INTO fragments (id, session_id, position, source_text, source_hash,"
                " n_cues, created_utc, created_epoch, prompt_hash, model)"
                " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    fragment.id,
                    session.id,
                    position,
                    fragment.source_text,
                    fragment.source_hash,
                    len(fragment.cue_details),
                    fragment.created_at or now_iso(),
                    _epoch(fragment.created_at),
                    fragment.prompt_hash or None,
                    fragment.model or None,
                ),
            )
            for index, detail in enumerate(fragment.cue_details):
                conn.execute(
                    "INSERT INTO cues (fragment_id, position, simple, term, meaning)"
                    " VALUES (?, ?, ?, ?, ?)",
                    (
                        fragment.id,
                        index,
                        detail.get("simple", ""),
                        detail.get("term", ""),
                        detail.get("meaning", ""),
                    ),
                )
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def save_prediction(self, session, check: PredictionCheck) -> None:
        conn = self._connect()
        try:
            self._ensure_session(conn, session)
            conn.execute("DELETE FROM predictions WHERE id = ?", (check.id,))
            conn.execute(
                "INSERT INTO predictions (id, session_id, hypothesis, status, mismatch,"
                " evidence, subject_note, one_delta, created_utc, created_epoch, prompt_hash, model)"
                " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    check.id,
                    session.id,
                    check.hypothesis,
                    check.status,
                    check.mismatch,
                    check.evidence,
                    check.subject_note,
                    check.one_delta,
                    check.created_at or now_iso(),
                    _epoch(check.created_at),
                    check.prompt_hash or None,
                    check.model or None,
                ),
            )
            for fragment_id in check.buffer_fragment_ids:
                conn.execute(
                    "INSERT OR IGNORE INTO prediction_fragments (prediction_id, fragment_id)"
                    " VALUES (?, ?)",
                    (check.id, fragment_id),
                )
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def save_feynman(self, session, check: FeynmanCheck) -> None:
        conn = self._connect()
        try:
            self._ensure_session(conn, session)
            conn.execute("DELETE FROM feynman_checks WHERE id = ?", (check.id,))
            conn.execute(
                "INSERT INTO feynman_checks (id, session_id, question, explanation, status,"
                " follow_up, created_utc, created_epoch, question_prompt_hash,"
                " check_prompt_hash, model) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    check.id,
                    session.id,
                    check.question,
                    check.explanation,
                    check.status,
                    check.follow_up,
                    check.created_at or now_iso(),
                    _epoch(check.created_at),
                    check.question_prompt_hash or None,
                    check.check_prompt_hash or None,
                    check.model or None,
                ),
            )
            for fragment_id in check.buffer_fragment_ids:
                conn.execute(
                    "INSERT OR IGNORE INTO feynman_fragments (check_id, fragment_id) VALUES (?, ?)",
                    (check.id, fragment_id),
                )
            for index, gap in enumerate(check.gaps):
                conn.execute(
                    "INSERT INTO feynman_gaps (check_id, position, location, type, description)"
                    " VALUES (?, ?, ?, ?, ?)",
                    (
                        check.id,
                        index,
                        gap.get("location", ""),
                        gap.get("type", ""),
                        gap.get("description", ""),
                    ),
                )
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write to {self.path}: {exc}") from exc
        finally:
            conn.close()

    # ── intentions ──────────────────────────────────────────────────────────

    def save_intention(
        self,
        text: str,
        *,
        criterion: str = "",
        stopped_at: str = "",
        source: str = "",
        material: str = "",
        material_origin: str = "",
        direction: str = "",
        created_utc: str | None = None,
    ) -> Intention:
        """Make a new current bookmark; the previous one becomes 'previous'.

        Replacing never marks the old intention done or abandoned: it stops
        being the current position and can be promoted back by hand.

        `material` and `direction` record what the wording was built from and
        what the reader asked of it. The passage is stored as it was captured;
        it is deliberately not re-wrapped into one line the way the phrase is.
        """
        phrase = " ".join((text or "").split()).strip()
        if not phrase:
            raise ValueError("intention text must not be empty")
        moment = created_utc or now_iso()
        intention = Intention(
            text=phrase,
            criterion=(criterion or "").strip(),
            stopped_at=(stopped_at or "").strip(),
            source=(source or "").strip(),
            material=(material or "").strip(),
            material_origin=(material_origin or "").strip(),
            direction=(direction or "").strip(),
            created_at=moment,
        )
        conn = self._connect()
        try:
            conn.execute("UPDATE intents SET status = 'previous' WHERE status = 'current'")
            conn.execute(
                "INSERT INTO intents (id, text, criterion, stopped_at, source, material,"
                " material_origin, direction, created_utc, created_epoch, updated_utc,"
                " updated_epoch, status)"
                " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, 'current')",
                (
                    intention.id,
                    intention.text,
                    intention.criterion or None,
                    intention.stopped_at or None,
                    intention.source or None,
                    intention.material or None,
                    intention.material_origin or None,
                    intention.direction or None,
                    intention.created_at,
                    _epoch(intention.created_at),
                ),
            )
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write intention to {self.path}: {exc}") from exc
        finally:
            conn.close()
        return intention

    def update_intention(
        self,
        intention_id: str,
        text: str,
        *,
        criterion: str = "",
        stopped_at: str = "",
        source: str | None = None,
        material: str | None = None,
        material_origin: str | None = None,
        direction: str | None = None,
        updated_utc: str | None = None,
    ) -> Intention:
        """Edit the current bookmark in place, keeping its id and history.

        `source`, `material`, `material_origin` and `direction` are `None` by
        default: an edit that does not mention them must not erase what a
        bookmark already records about where its wording came from.
        """
        phrase = " ".join((text or "").split()).strip()
        if not phrase:
            raise ValueError("intention text must not be empty")
        moment = updated_utc or now_iso()
        fields = ["text = ?", "criterion = ?", "stopped_at = ?", "updated_utc = ?", "updated_epoch = ?"]
        values: list[object] = [
            phrase,
            (criterion or "").strip() or None,
            (stopped_at or "").strip() or None,
            moment,
            _epoch(moment),
        ]
        if source is not None:
            fields.append("source = ?")
            values.append((source or "").strip() or None)
        for column, value in (("material", material), ("material_origin", material_origin),
                              ("direction", direction)):
            if value is not None:
                fields.append(f"{column} = ?")
                values.append((value or "").strip() or None)
        values.append(intention_id)
        conn = self._connect()
        try:
            cursor = conn.execute(
                "UPDATE intents SET " + ", ".join(fields) + " WHERE id = ?",
                tuple(values),
            )
            if not cursor.rowcount:
                raise ValueError(f"no intention with id {intention_id}")
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write intention to {self.path}: {exc}") from exc
        finally:
            conn.close()
        loaded = self.intention(intention_id)
        if loaded is None:  # pragma: no cover - the row was just updated
            raise RecordError(f"intention {intention_id} vanished after update")
        return loaded

    def promote_intention(self, intention_id: str) -> Intention:
        """Bring back a previous bookmark without calling the old one finished."""
        conn = self._connect()
        try:
            row = conn.execute("SELECT 1 FROM intents WHERE id = ?", (intention_id,)).fetchone()
            if row is None:
                raise ValueError(f"no intention with id {intention_id}")
            conn.execute(
                "UPDATE intents SET status = 'previous' WHERE status = 'current' AND id != ?",
                (intention_id,),
            )
            conn.execute("UPDATE intents SET status = 'current' WHERE id = ?", (intention_id,))
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write intention to {self.path}: {exc}") from exc
        finally:
            conn.close()
        loaded = self.intention(intention_id)
        if loaded is None:  # pragma: no cover - the row was just updated
            raise RecordError(f"intention {intention_id} vanished after promote")
        return loaded

    def current_intention(self) -> Intention | None:
        rows = self._load_intents("status = 'current' ORDER BY created_epoch DESC, id LIMIT 1")
        return rows[0] if rows else None

    def intention(self, intention_id: str) -> Intention | None:
        rows = self._load_intents("id = ?", (intention_id,))
        return rows[0] if rows else None

    # ── 15-minute steps (Alt+I) ─────────────────────────────────────────────

    def _write(self, sql: str, args: tuple, what: str) -> int:
        conn = self._connect()
        try:
            cursor = conn.execute(sql, args)
            conn.commit()
            return cursor.rowcount
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot write {what} to {self.path}: {exc}") from exc
        finally:
            conn.close()

    def _read(self, sql: str, args: tuple = ()) -> list[dict[str, Any]]:
        conn = self._connect()
        try:
            return [dict(row) for row in conn.execute(sql, args)]
        except (sqlite3.Error, OSError) as exc:
            raise RecordError(f"cannot read {self.path}: {exc}") from exc
        finally:
            conn.close()

    def add_step(self, step_id: str, text: str, *, session_key: str, minutes: float,
                 goal_id: str = "", started_epoch: float, deadline_epoch: float) -> None:
        started = datetime.fromtimestamp(started_epoch, timezone.utc).isoformat(timespec="seconds")
        self._write(
            "INSERT INTO steps (id, session_key, text, minutes, goal_id, started_utc,"
            " started_epoch, deadline_epoch) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (step_id, session_key, " ".join(text.split()), minutes, goal_id or None,
             started, started_epoch, deadline_epoch), "step")

    def finish_step(self, step_id: str, outcome: str, takeaway: str = "",
                    finished_utc: str | None = None) -> bool:
        moment = finished_utc or now_iso()
        return self._write(
            "UPDATE steps SET outcome = ?, takeaway = ?, finished_utc = ?, finished_epoch = ?"
            " WHERE id = ?",
            (outcome, " ".join((takeaway or "").split()) or None, moment, _epoch(moment), step_id),
            "step outcome") == 1

    def step(self, step_id: str) -> dict[str, Any] | None:
        rows = self._read("SELECT * FROM steps WHERE id = ?", (step_id,))
        return rows[0] if rows else None

    def last_step(self) -> dict[str, Any] | None:
        rows = self._read("SELECT * FROM steps ORDER BY started_epoch DESC LIMIT 1")
        return rows[0] if rows else None

    def session_steps(self, session_key: str) -> list[dict[str, Any]]:
        return self._read("SELECT * FROM steps WHERE session_key = ? ORDER BY started_epoch",
                          (session_key,))

    def close_study_session(self, session_key: str, *, goal_id: str = "", outcome: str = "",
                            remaining: str = "", closed_utc: str | None = None) -> None:
        moment = closed_utc or now_iso()
        self._write(
            "INSERT OR REPLACE INTO step_closures (session_key, goal_id, outcome, remaining,"
            " closed_utc, closed_epoch) VALUES (?, ?, ?, ?, ?, ?)",
            (session_key, goal_id or None, outcome or None,
             " ".join((remaining or "").split()) or None, moment, _epoch(moment)), "session closure")

    def closure(self, session_key: str) -> dict[str, Any] | None:
        rows = self._read("SELECT * FROM step_closures WHERE session_key = ?", (session_key,))
        return rows[0] if rows else None

    def last_closure(self) -> dict[str, Any] | None:
        rows = self._read("SELECT * FROM step_closures ORDER BY closed_epoch DESC LIMIT 1")
        return rows[0] if rows else None

    def intention_history(self, *, tail: int | None = None) -> list[Intention]:
        """Replaced bookmarks, newest first; the current one is not repeated."""
        rows = self._load_intents("status = 'previous' ORDER BY created_epoch DESC, id DESC")
        return rows[:tail] if tail else rows

    def _load_intents(self, clause: str = "", params: tuple = ()) -> list[Intention]:
        conn = self._connect()
        try:
            sql = "SELECT * FROM intents"
            if clause:
                sql += " WHERE " + clause
            rows = conn.execute(sql, params).fetchall()
        finally:
            conn.close()
        return [
            Intention(
                text=row["text"] or "",
                criterion=row["criterion"] or "",
                stopped_at=row["stopped_at"] or "",
                source=row["source"] or "",
                material=row["material"] or "",
                material_origin=row["material_origin"] or "",
                direction=row["direction"] or "",
                created_at=row["created_utc"],
                updated_at=row["updated_utc"] or "",
                id=row["id"],
                status=row["status"] or "current",
            )
            for row in rows
        ]

    # ── reads ────────────────────────────────────────────────────────────────

    def sessions(self, *, tail: int | None = None) -> list[dict[str, Any]]:
        conn = self._connect()
        try:
            rows = [dict(r) for r in conn.execute("SELECT * FROM sessions ORDER BY created_epoch, id")]
        finally:
            conn.close()
        return rows[-tail:] if tail else rows

    def session_meta(self, session_id: str) -> dict[str, Any] | None:
        conn = self._connect()
        try:
            row = conn.execute("SELECT * FROM sessions WHERE id = ?", (session_id,)).fetchone()
        finally:
            conn.close()
        return dict(row) if row else None

    def fragment_session_pairs(self) -> dict[str, str]:
        conn = self._connect()
        try:
            return {r["id"]: r["session_id"] for r in conn.execute("SELECT id, session_id FROM fragments")}
        finally:
            conn.close()

    def load_fragments(self, session_id: str) -> list[Fragment]:
        conn = self._connect()
        try:
            fragments: list[Fragment] = []
            for fr in conn.execute(
                "SELECT * FROM fragments WHERE session_id = ? ORDER BY position, created_epoch, id",
                (session_id,),
            ):
                details = [
                    dict(c)
                    for c in conn.execute(
                        "SELECT simple, term, meaning FROM cues WHERE fragment_id = ? ORDER BY position, id",
                        (fr["id"],),
                    )
                ]
                fragments.append(Fragment(
                    source_text=fr["source_text"] or "",
                    cues=[detail["term"] for detail in details],
                    cue_details=details,
                    created_at=fr["created_utc"],
                    id=fr["id"],
                    source_hash=fr["source_hash"] or "",
                    prompt_hash=fr["prompt_hash"] or "",
                    model=fr["model"] or "",
                ))
            return fragments
        finally:
            conn.close()

    def latest_fragment_source(self) -> str:
        """Source text of the most recent fragment, or an empty string.

        A fallback only: the reader's current selection outranks it, and the
        caller shows which of the two was taken, so a wrong guess is visible.
        """
        conn = self._connect()
        try:
            row = conn.execute(
                "SELECT source_text FROM fragments"
                " WHERE source_text IS NOT NULL AND TRIM(source_text) != ''"
                " ORDER BY created_epoch DESC, rowid DESC LIMIT 1"
            ).fetchone()
        finally:
            conn.close()
        return (row["source_text"] or "").strip() if row else ""

    def load_predictions(self, session_id: str) -> list[PredictionCheck]:
        conn = self._connect()
        try:
            checks: list[PredictionCheck] = []
            for row in conn.execute(
                "SELECT * FROM predictions WHERE session_id = ? ORDER BY created_epoch, id",
                (session_id,),
            ):
                ids = [
                    r["fragment_id"]
                    for r in conn.execute(
                        "SELECT fragment_id FROM prediction_fragments WHERE prediction_id = ?", (row["id"],)
                    )
                ]
                checks.append(PredictionCheck(
                    buffer_fragment_ids=ids,
                    hypothesis=row["hypothesis"],
                    status=row["status"],
                    mismatch=row["mismatch"] or "",
                    evidence=row["evidence"] or "",
                    subject_note=row["subject_note"] or "",
                    one_delta=row["one_delta"] or "",
                    created_at=row["created_utc"],
                    id=row["id"],
                    prompt_hash=row["prompt_hash"] or "",
                    model=row["model"] or "",
                ))
            return checks
        finally:
            conn.close()

    def load_feynman(self, session_id: str) -> list[FeynmanCheck]:
        conn = self._connect()
        try:
            checks: list[FeynmanCheck] = []
            for row in conn.execute(
                "SELECT * FROM feynman_checks WHERE session_id = ? ORDER BY created_epoch, id",
                (session_id,),
            ):
                ids = [
                    r["fragment_id"]
                    for r in conn.execute(
                        "SELECT fragment_id FROM feynman_fragments WHERE check_id = ?", (row["id"],)
                    )
                ]
                gaps = [
                    dict(g)
                    for g in conn.execute(
                        "SELECT location, type, description FROM feynman_gaps WHERE check_id = ? ORDER BY position, id",
                        (row["id"],),
                    )
                ]
                checks.append(FeynmanCheck(
                    buffer_fragment_ids=ids,
                    question=row["question"] or "",
                    explanation=row["explanation"] or "",
                    status=row["status"] or "",
                    gaps=gaps,
                    follow_up=row["follow_up"],
                    created_at=row["created_utc"],
                    id=row["id"],
                    question_prompt_hash=row["question_prompt_hash"] or "",
                    check_prompt_hash=row["check_prompt_hash"] or "",
                    model=row["model"] or "",
                ))
            return checks
        finally:
            conn.close()


# ── day/session assembly (ledger + notes joined in Python) ──────────────────


def _resolve_note_session(note, frag_session: dict[str, str]) -> str | None:
    if note.session_id:
        return note.session_id
    if note.fragment_id:
        return frag_session.get(note.fragment_id)
    return None


def day_data(store: RecordStore, notes_store: NoteStore, date: str) -> dict[str, Any]:
    sessions = [s for s in store.sessions() if local_date(s["created_utc"]) == date]
    sessions.sort(key=lambda s: (s["created_epoch"] or 0, s["id"]))
    frag_session = store.fragment_session_pairs()
    notes_on_day = [n for n in notes_store.list() if local_date(n.created_utc) == date]

    out_sessions: list[dict[str, Any]] = []
    for session in sessions:
        session_notes = [
            n for n in notes_on_day
            if _resolve_note_session(n, frag_session) == session["id"]
        ]
        out_sessions.append({
            **session,
            "fragments": [f.to_dict() for f in store.load_fragments(session["id"])],
            "predictions": [p.to_dict() for p in store.load_predictions(session["id"])],
            "feynman": [f.to_dict() for f in store.load_feynman(session["id"])],
            "tasks": store.load_tasks(session["id"]),
            "notes": [n.to_dict() for n in session_notes],
        })

    session_ids = {s["id"] for s in sessions}
    orphan_notes = [
        n.to_dict() for n in notes_on_day
        if _resolve_note_session(n, frag_session) not in session_ids
    ]
    return {"date": date, "sessions": out_sessions, "orphan_notes": orphan_notes}


def session_data(store: RecordStore, notes_store: NoteStore, session_id: str) -> dict[str, Any] | None:
    session = store.session_meta(session_id)
    if session is None:
        return None
    frag_session = store.fragment_session_pairs()
    notes = [n for n in notes_store.list() if _resolve_note_session(n, frag_session) == session_id]
    return {
        **session,
        "fragments": [f.to_dict() for f in store.load_fragments(session_id)],
        "predictions": [p.to_dict() for p in store.load_predictions(session_id)],
        "feynman": [f.to_dict() for f in store.load_feynman(session_id)],
        "tasks": store.load_tasks(session_id),
        "notes": [n.to_dict() for n in notes],
    }


def _fragment_line(fragment: dict[str, Any]) -> str:
    terms = " · ".join(cue["term"] for cue in fragment["cue_details"])
    return f"{_clock(fragment['created_at'])}  4 слова   {terms}"


def render_day(data: dict[str, Any]) -> str:
    lines = [f"── {data['date']} ─────────────────────────────"]
    if not data["sessions"] and not data["orphan_notes"]:
        return f"no records on {data['date']}"
    for session in data["sessions"]:
        span = _clock(session["created_utc"])
        if session.get("closed_utc"):
            span += f"–{_clock(session['closed_utc'])}"
        reason = session.get("close_reason") or ("open" if not session.get("closed_utc") else "closed")
        lines.append(f"\nсессия {session['id'][:8]}  ({span}, {reason})")
        for fragment in session["fragments"]:
            lines.append(f"  {_fragment_line(fragment)}")
        for check in session["predictions"]:
            lines.append(f"  {_clock(check['created_at'])}  гипотеза   [{check['status']}]  {check['hypothesis']}")
            if check.get("one_delta"):
                lines.append(f"    поправка: {check['one_delta']}")
        for check in session["feynman"]:
            lines.append(f"  {_clock(check['created_at'])}  фейнман    [{check['status']}]  {check['question']}")
        for note in session["notes"]:
            lines.append(f"  {_clock(note['created_utc'])}  ошибка     «{note['anchor']}»  [{note['status']}]")
    if data["orphan_notes"]:
        lines.append("\n— заметки без сессии —")
        for note in data["orphan_notes"]:
            lines.append(f"  {_clock(note['created_utc'])}  «{note['anchor']}»  [{note['status']}]  {note['comment'][:60]}")
    return "\n".join(lines)


def render_session(data: dict[str, Any]) -> str:
    session_id = data["id"]
    span = _clock(data["created_utc"])
    if data.get("closed_utc"):
        span += f"–{_clock(data['closed_utc'])}"
    reason = data.get("close_reason") or ("open" if not data.get("closed_utc") else "closed")
    lines = [f"── сессия {session_id}  ({span}, {reason})  «{data.get('title') or 'Untitled session'}» ──"]
    if not data["fragments"] and not data["predictions"] and not data["feynman"] and not data["notes"]:
        lines.append("(пусто)")
    for fragment in data["fragments"]:
        lines.append(f"\n{_clock(fragment['created_at'])}  4 слова")
        for cue in fragment["cue_details"]:
            lines.append(f"    {cue['simple']}  →  {cue['term']}  ({cue['meaning']})")
        lines.append(f"    источник: {fragment['source_text']}")
    for check in data["predictions"]:
        lines.append(f"\n{_clock(check['created_at'])}  гипотеза  [{check['status']}]")
        lines.append(f"    {check['hypothesis']}")
        if check.get("one_delta"):
            lines.append(f"    поправка: {check['one_delta']}")
        if check.get("evidence"):
            lines.append(f"    в тексте: {check['evidence']}")
        if check.get("mismatch"):
            lines.append(f"    расхождение: {check['mismatch']}")
    for check in data["feynman"]:
        lines.append(f"\n{_clock(check['created_at'])}  фейнман  [{check['status']}]")
        lines.append(f"    вопрос: {check['question']}")
        lines.append(f"    объяснение: {check['explanation']}")
        for gap in check.get("gaps", []):
            lines.append(f"    · {gap.get('location') or ''} {gap.get('type') or ''}: {gap.get('description') or ''}".rstrip())
    for note in data["notes"]:
        lines.append(f"\n{local_stamp(note['created_utc'])}  ошибка  «{note['anchor']}»  [{note['status']}]")
        for line in note["comment"].splitlines():
            lines.append(f"    {line}")
        if note.get("resolution"):
            lines.append(f"    -> закрыто: {note['resolution']}")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m cognitive_popups.records",
        description="The durable reading ledger: sessions, 4-word fragments, hypotheses.",
    )
    parser.add_argument("--db", default=None, help=f"ledger path (default {DEFAULT_DB})")
    parser.add_argument("--notes-db", default=None, help="notes path (default ~/.local/state/cognitive-popups/notes.sqlite3)")
    parser.add_argument("--today", action="store_true", help="today's local date (default when no date/session given)")
    parser.add_argument("--date", default=None, help="one local date, YYYY-MM-DD")
    parser.add_argument("--session", default=None, help="one session id, full detail")
    parser.add_argument("--json", action="store_true", help="raw structure as JSON")
    args = parser.parse_args(argv)

    store = RecordStore(args.db)
    notes_store = NoteStore(args.notes_db)

    if args.session:
        data = session_data(store, notes_store, args.session)
        if data is None:
            print(f"no session {args.session}", file=sys.stderr)
            return 1
        print(json.dumps(data, ensure_ascii=False, indent=2) if args.json else render_session(data))
        return 0

    date = args.date
    if date is None:
        date = datetime.now(timezone.utc).astimezone().strftime("%Y-%m-%d")
    data = day_data(store, notes_store, date)
    if args.json:
        print(json.dumps(data, ensure_ascii=False, indent=2))
        return 0
    print(render_day(data))
    return 0


if __name__ == "__main__":  # pragma: no cover - CLI entry point
    raise SystemExit(main())
