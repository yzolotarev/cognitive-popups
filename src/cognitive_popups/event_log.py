"""SQLite event log for UI interactions: which window opened when, what was clicked.

Every popup runs as its own process (the desktop layer spawns the helper), so the
log is the only place where one interaction can be reconstructed end to end.  The
desktop process records the hotkey and the window it spawns; the popup process
records the window it actually opened and every click inside it.  Correlation is
carried through the environment:

    COGNITIVE_EVENT_SESSION   groups one hotkey press and everything it caused
    COGNITIVE_EVENT_PARENT    row id of the event that caused this process to start

`parent_id` turns the rows into a tree instead of a flat list: a click points at
the window it happened in, a window points at the click that opened it.

Timings are stored twice on purpose.  `ts_utc` is human-readable UTC with
milliseconds; `ts_epoch` is the same instant as a float, so ordering and interval
maths never have to parse a string.  `ts_mono` (time.monotonic) is only meaningful
inside one process and exists to measure durations that wall clock changes cannot
distort.

Logging never raises into the UI: a missing, locked, or read-only database
degrades to "no log" rather than "no popup".
"""

from __future__ import annotations

import argparse
import os
import sqlite3
import sys
import threading
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

DEFAULT_DB = "~/.local/state/cognitive-popups/events.sqlite3"

ENV_DB = "COGNITIVE_EVENT_DB"
ENV_DISABLE = "COGNITIVE_EVENT_DISABLE"
ENV_SESSION = "COGNITIVE_EVENT_SESSION"
ENV_PARENT = "COGNITIVE_EVENT_PARENT"

SCHEMA_VERSION = 1

ITEM_LABEL_LIMIT = 120
DETAIL_LIMIT = 400

_SCHEMA = """
CREATE TABLE IF NOT EXISTS events (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_utc       TEXT    NOT NULL,
    ts_epoch     REAL    NOT NULL,
    ts_mono      REAL,
    session_id   TEXT    NOT NULL,
    parent_id    INTEGER,
    origin       TEXT    NOT NULL,
    pid          INTEGER,
    window       TEXT,
    event        TEXT    NOT NULL,
    layer        INTEGER,
    item_index   INTEGER,
    item_label   TEXT,
    detail       TEXT,
    source_hash  TEXT,
    fragment_id  TEXT
);
CREATE INDEX IF NOT EXISTS events_ts_idx      ON events(ts_epoch);
CREATE INDEX IF NOT EXISTS events_session_idx ON events(session_id, id);
CREATE INDEX IF NOT EXISTS events_window_idx  ON events(window, event);
"""

_TRUTHY = {"1", "true", "yes", "on"}


def resolve_db_path(path: str | Path | None = None) -> Path:
    raw = path or os.environ.get(ENV_DB) or DEFAULT_DB
    return Path(raw).expanduser()


def new_session() -> str:
    """Short id for one interaction (a hotkey press and its consequences)."""
    return uuid.uuid4().hex[:12]


def _clip(value: Any, limit: int) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    if not text:
        return None
    return text if len(text) <= limit else text[: limit - 1] + "…"


class EventLog:
    """Append-only writer for the event database.

    A connection is opened per write: interactions are rare (a few rows per
    keypress) and this keeps writers in different processes from sharing or
    inheriting a handle.  WAL mode lets the desktop and the popup write at the
    same time without one blocking the other.
    """

    def __init__(self, path: str | Path | None = None, *, enabled: bool = True):
        self.path = resolve_db_path(path)
        self.enabled = enabled and os.environ.get(ENV_DISABLE, "").lower() not in _TRUTHY
        self._lock = threading.Lock()
        self._schema_ready = False

    def _connect(self) -> sqlite3.Connection:
        conn = sqlite3.connect(self.path, timeout=3.0)
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA synchronous=NORMAL")
        if not self._schema_ready:
            conn.executescript(_SCHEMA)
            conn.execute(f"PRAGMA user_version={SCHEMA_VERSION}")
            self._schema_ready = True
        return conn

    def log(
        self,
        event: str,
        *,
        session_id: str,
        origin: str = "app",
        pid: int | None = None,
        parent_id: int | None = None,
        window: str | None = None,
        layer: int | None = None,
        item_index: int | None = None,
        item_label: str | None = None,
        detail: str | None = None,
        source_hash: str | None = None,
        fragment_id: str | None = None,
    ) -> int | None:
        """Write one row; return its id, or None when logging is off or failed."""
        if not self.enabled:
            return None
        moment = datetime.now(timezone.utc)
        row = (
            moment.isoformat(timespec="milliseconds"),
            moment.timestamp(),
            time.monotonic(),
            session_id,
            parent_id,
            origin,
            os.getpid() if pid is None else pid,
            _clip(window, 40),
            str(event),
            layer,
            item_index,
            _clip(item_label, ITEM_LABEL_LIMIT),
            _clip(detail, DETAIL_LIMIT),
            source_hash,
            fragment_id,
        )
        try:
            with self._lock:
                self.path.parent.mkdir(parents=True, exist_ok=True)
                conn = self._connect()
                try:
                    cursor = conn.execute(
                        "INSERT INTO events ("
                        "ts_utc, ts_epoch, ts_mono, session_id, parent_id, origin, pid,"
                        " window, event, layer, item_index, item_label, detail,"
                        " source_hash, fragment_id"
                        ") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        row,
                    )
                    conn.commit()
                    return int(cursor.lastrowid or 0) or None
                finally:
                    conn.close()
        except Exception:
            # The UI must survive a broken log; see the module docstring.
            return None


class EventChain:
    """Threads `parent_id` from each event to the next one it caused.

    Usage: the process that starts an interaction creates a chain (from the
    environment when it was spawned by another process) and emits events in
    order.  Each successful write becomes the parent of the following event, so
    the timeline reads as a causal sequence.
    """

    def __init__(
        self,
        log: EventLog,
        session_id: str,
        *,
        origin: str = "app",
        pid: int | None = None,
        parent_id: int | None = None,
    ):
        self.log = log
        self.session_id = session_id
        self.origin = origin
        self.pid = pid
        self.parent_id = parent_id
        self._lock = threading.Lock()

    @classmethod
    def from_env(cls, log: EventLog, *, origin: str = "app", pid: int | None = None) -> "EventChain":
        """Continue the interaction the parent process started, if there is one."""
        session = os.environ.get(ENV_SESSION) or new_session()
        raw = os.environ.get(ENV_PARENT, "")
        parent = int(raw) if raw.isdigit() else None
        return cls(log, session, origin=origin, pid=pid, parent_id=parent)

    def emit(self, event: str, **fields: Any) -> int | None:
        with self._lock:
            row = self.log.log(
                event,
                session_id=self.session_id,
                origin=self.origin,
                pid=self.pid,
                parent_id=self.parent_id,
                **fields,
            )
            if row is not None:
                self.parent_id = row
            return row

    def env(self) -> dict[str, str]:
        """Environment for a child process so its events attach to this chain."""
        with self._lock:
            env = {ENV_SESSION: self.session_id}
            if self.parent_id is not None:
                env[ENV_PARENT] = str(self.parent_id)
            return env


_default: EventLog | None = None
_default_lock = threading.Lock()


def default_log() -> EventLog:
    global _default
    with _default_lock:
        if _default is None:
            _default = EventLog()
        return _default


def fetch(
    path: str | Path | None = None,
    *,
    session: str | None = None,
    tail: int | None = None,
    since: float | None = None,
) -> list[dict[str, Any]]:
    """Read rows ordered by time (then insertion order)."""
    db = resolve_db_path(path)
    if not db.exists():
        return []
    conn = sqlite3.connect(db, timeout=3.0)
    conn.row_factory = sqlite3.Row
    try:
        sql = "SELECT * FROM events"
        clauses: list[str] = []
        params: list[Any] = []
        if session:
            clauses.append("session_id = ?")
            params.append(session)
        if since is not None:
            clauses.append("ts_epoch >= ?")
            params.append(float(since))
        if clauses:
            sql += " WHERE " + " AND ".join(clauses)
        sql += " ORDER BY ts_epoch, id"
        rows = [dict(row) for row in conn.execute(sql, params)]
    finally:
        conn.close()
    return rows[-tail:] if tail else rows


def prune(path: str | Path | None = None, *, older_than_days: float = 30) -> int:
    """Drop telemetry older than a cutoff; return how many rows went.

    Only events are pruned. Error notes are the reader's own words and are never
    expired automatically (see notes.py).
    """
    db = resolve_db_path(path)
    if not db.exists():
        return 0
    cutoff = time.time() - older_than_days * 86400
    conn = sqlite3.connect(db, timeout=3.0)
    try:
        cursor = conn.execute("DELETE FROM events WHERE ts_epoch < ?", (cutoff,))
        conn.commit()
        return int(cursor.rowcount or 0)
    finally:
        conn.close()


def local_time(ts_utc: str) -> str:
    try:
        return datetime.fromisoformat(ts_utc).astimezone().strftime("%H:%M:%S.%f")[:-3]
    except ValueError:
        return str(ts_utc)


def describe(row: dict[str, Any]) -> str:
    """Human-readable payload: window, layer, which item, what was clicked."""
    parts: list[str] = []
    if row.get("window"):
        parts.append(str(row["window"]))
    if row.get("layer") is not None:
        parts.append(f"L{row['layer']}")
    if row.get("item_index") is not None:
        parts.append(f"#{int(row['item_index']) + 1}")
    if row.get("item_label"):
        parts.append(f"«{row['item_label']}»")
    if row.get("detail"):
        parts.append(str(row["detail"]))
    if row.get("fragment_id"):
        parts.append(f"frag={str(row['fragment_id'])[:8]}")
    return "  ".join(parts)


def _depth(row: dict[str, Any], by_id: dict[int, dict[str, Any]]) -> int:
    depth = 0
    seen: set[int] = set()
    parent = row.get("parent_id")
    while isinstance(parent, int) and parent in by_id and parent not in seen:
        seen.add(parent)
        depth += 1
        parent = by_id[parent].get("parent_id")
    return depth


def render_timeline(rows: list[dict[str, Any]]) -> str:
    """One line per event, indented by causal depth, grouped by session."""
    by_id = {int(row["id"]): row for row in rows if row.get("id") is not None}
    lines: list[str] = []
    session: str | None = None
    for row in rows:
        if row["session_id"] != session:
            session = row["session_id"]
            lines.append("")
            lines.append(f"── session {session} ──")
        depth = _depth(row, by_id)
        marker = "" if depth == 0 else "  " * (depth - 1) + "└ "
        head = (
            f"{local_time(row['ts_utc'])}  {marker}{str(row['event']):<12} "
            f"{str(row['origin']):<8} {describe(row)}"
        )
        lines.append(head.rstrip())
    return "\n".join(lines).lstrip("\n")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m cognitive_popups.event_log",
        description="Show the recorded window/click timeline.",
    )
    parser.add_argument("--db", default=None, help=f"database path (default {DEFAULT_DB})")
    parser.add_argument("--session", default=None, help="only this session id")
    parser.add_argument("--tail", type=int, default=40, help="last N events (0 = all)")
    parser.add_argument("--json", action="store_true", help="raw rows as JSON")
    parser.add_argument(
        "--prune-days",
        type=float,
        default=None,
        help="delete events older than N days and exit (notes are never pruned)",
    )
    args = parser.parse_args(argv)

    if args.prune_days is not None:
        removed = prune(args.db, older_than_days=args.prune_days)
        print(f"removed {removed} events from {resolve_db_path(args.db)}")
        return 0

    rows = fetch(args.db, session=args.session, tail=args.tail or None)
    if not rows:
        print(f"no events in {resolve_db_path(args.db)}", file=sys.stderr)
        return 1
    if args.json:
        import json

        print(json.dumps(rows, ensure_ascii=False, indent=2))
        return 0
    print(render_timeline(rows))
    return 0


if __name__ == "__main__":  # pragma: no cover - CLI entry point
    raise SystemExit(main())
