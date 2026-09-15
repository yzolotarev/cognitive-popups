"""Error notes: the reader's own account of what they got wrong, and why.

The name follows the project's existing vocabulary. "Error" here already means a
falsified expectation: V2.md defines the mismatch between a hypothesis and the
source as the prediction error. An error note is the reader's own version of
that — the exact phrase that tripped them up and what they thought it meant,
so the gap can be closed later instead of forgotten.

Two properties matter for later review:

- a note carries its own anchor text. Sessions live in RAM and are archived only
  by an explicit Clear, so a fragment a note points at may never reach disk: the
  anchor ("working memory") has to be readable on its own;
- this store never fails silently. Losing telemetry is acceptable, losing the
  reader's own words is not, so `add()` raises instead of returning None.

`anchor_key` is the anchor normalised for lookup, so the same word tripping the
reader again and again can be found with `--repeats`.
"""

from __future__ import annotations

import argparse
import os
import sqlite3
import sys
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

DEFAULT_DB = "~/.local/state/cognitive-popups/notes.sqlite3"
ENV_DB = "COGNITIVE_NOTES_DB"

SCHEMA_VERSION = 1
ANCHOR_LIMIT = 200
STATUS_OPEN = "open"
STATUS_CLOSED = "closed"

_SCHEMA = """
CREATE TABLE IF NOT EXISTS error_notes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    created_utc   TEXT    NOT NULL,
    created_epoch REAL    NOT NULL,
    anchor        TEXT,
    anchor_key    TEXT,
    comment       TEXT    NOT NULL,
    kind          TEXT,
    fragment_id   TEXT,
    source_hash   TEXT,
    status        TEXT    NOT NULL DEFAULT 'open',
    closed_utc    TEXT,
    resolution    TEXT
);
CREATE INDEX IF NOT EXISTS notes_status_idx ON error_notes(status, created_epoch);
CREATE INDEX IF NOT EXISTS notes_anchor_idx ON error_notes(anchor_key);
"""


class NoteError(RuntimeError):
    """Raised when a note cannot be stored or found; never swallowed."""


def resolve_db_path(path: str | Path | None = None) -> Path:
    raw = path or os.environ.get(ENV_DB) or DEFAULT_DB
    return Path(raw).expanduser()


def normalise_anchor(text: str | None) -> str | None:
    """Fold an anchor to a lookup key: case, spacing and edge punctuation."""
    if not text:
        return None
    cleaned = " ".join(str(text).split()).strip().strip(".,;:!?()[]{}«»\"'`—-")
    return cleaned.casefold() or None


@dataclass
class ErrorNote:
    id: int
    created_utc: str
    anchor: str | None
    comment: str
    kind: str | None = None
    fragment_id: str | None = None
    source_hash: str | None = None
    status: str = STATUS_OPEN
    closed_utc: str | None = None
    resolution: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


class NoteStore:
    """SQLite home for error notes, separate from the event log.

    Telemetry and the reader's own words have different lifetimes and different
    tolerance for failure, so they live in different files.
    """

    def __init__(self, path: str | Path | None = None):
        self.path = resolve_db_path(path)
        self._schema_ready = False

    def _connect(self) -> sqlite3.Connection:
        try:
            conn = sqlite3.connect(self.path, timeout=3.0)
            conn.row_factory = sqlite3.Row
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("PRAGMA synchronous=NORMAL")
            if not self._schema_ready:
                conn.executescript(_SCHEMA)
                conn.execute(f"PRAGMA user_version={SCHEMA_VERSION}")
                self._schema_ready = True
            return conn
        except (sqlite3.Error, OSError) as exc:
            # One exception type for every failure: the UI reports NoteError.
            raise NoteError(f"cannot open {self.path}: {exc}") from exc

    def add(
        self,
        comment: str,
        *,
        anchor: str | None = None,
        kind: str | None = None,
        fragment_id: str | None = None,
        source_hash: str | None = None,
    ) -> ErrorNote:
        text = (comment or "").strip()
        if not text:
            raise NoteError("note text must not be empty")
        cleaned_anchor = " ".join(str(anchor).split()).strip() if anchor else ""
        if len(cleaned_anchor) > ANCHOR_LIMIT:
            cleaned_anchor = cleaned_anchor[: ANCHOR_LIMIT - 1] + "…"
        moment = datetime.now(timezone.utc)
        created_utc = moment.isoformat(timespec="milliseconds")
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            conn = self._connect()
            try:
                cursor = conn.execute(
                    "INSERT INTO error_notes ("
                    "created_utc, created_epoch, anchor, anchor_key, comment, kind,"
                    " fragment_id, source_hash, status"
                    ") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    (
                        created_utc,
                        moment.timestamp(),
                        cleaned_anchor or None,
                        normalise_anchor(cleaned_anchor),
                        text,
                        kind,
                        fragment_id,
                        source_hash,
                        STATUS_OPEN,
                    ),
                )
                conn.commit()
                note_id = int(cursor.lastrowid or 0)
            finally:
                conn.close()
        except (sqlite3.Error, OSError) as exc:
            raise NoteError(f"cannot write to {self.path}: {exc}") from exc
        note = self.get(note_id)
        if note is None:  # pragma: no cover - the row was just inserted
            raise NoteError(f"note {note_id} vanished after insert")
        return note

    def get(self, note_id: int) -> ErrorNote | None:
        conn = self._connect()
        try:
            row = conn.execute("SELECT * FROM error_notes WHERE id = ?", (note_id,)).fetchone()
        finally:
            conn.close()
        return ErrorNote(**_note_fields(row)) if row else None

    def list(
        self,
        *,
        status: str | None = None,
        kind: str | None = None,
        anchor_key: str | None = None,
        tail: int | None = None,
    ) -> list[ErrorNote]:
        if not self.path.exists():
            return []
        sql = "SELECT * FROM error_notes"
        clauses: list[str] = []
        params: list[Any] = []
        if status:
            clauses.append("status = ?")
            params.append(status)
        if kind:
            clauses.append("kind = ?")
            params.append(kind)
        if anchor_key:
            clauses.append("anchor_key = ?")
            params.append(normalise_anchor(anchor_key))
        if clauses:
            sql += " WHERE " + " AND ".join(clauses)
        sql += " ORDER BY created_epoch, id"
        conn = self._connect()
        try:
            rows = conn.execute(sql, params).fetchall()
        finally:
            conn.close()
        notes = [ErrorNote(**_note_fields(row)) for row in rows]
        return notes[-tail:] if tail else notes

    def close(self, note_id: int, resolution: str | None = None) -> ErrorNote:
        """Mark a gap as handled; the note itself stays for the record."""
        if self.get(note_id) is None:
            raise NoteError(f"no note with id {note_id}")
        closed_utc = datetime.now(timezone.utc).isoformat(timespec="milliseconds")
        conn = self._connect()
        try:
            conn.execute(
                "UPDATE error_notes SET status = ?, closed_utc = ?, resolution = ? WHERE id = ?",
                (STATUS_CLOSED, closed_utc, (resolution or "").strip() or None, note_id),
            )
            conn.commit()
        except (sqlite3.Error, OSError) as exc:  # pragma: no cover - defensive
            raise NoteError(f"cannot update {self.path}: {exc}") from exc
        finally:
            conn.close()
        note = self.get(note_id)
        if note is None:  # pragma: no cover - the row was just updated
            raise NoteError(f"note {note_id} vanished after update")
        return note

    def delete(self, note_id: int) -> ErrorNote:
        """Remove a note for good; returns it so the caller can name what went.

        Closing is the ordinary way out (the note stays for the record); deleting
        exists because a mis-recorded note has no value and would only add noise.
        """
        note = self.get(note_id)
        if note is None:
            raise NoteError(f"no note with id {note_id}")
        conn = self._connect()
        try:
            conn.execute("DELETE FROM error_notes WHERE id = ?", (note_id,))
            conn.commit()
        except (sqlite3.Error, OSError) as exc:
            raise NoteError(f"cannot delete from {self.path}: {exc}") from exc
        finally:
            conn.close()
        return note

    def repeats(self, *, status: str | None = None, min_count: int = 2) -> list[tuple[str, int]]:
        """Anchors that tripped the reader more than once - the real holes."""
        if not self.path.exists():
            return []
        sql = (
            "SELECT anchor_key, COUNT(*) AS hits FROM error_notes "
            "WHERE anchor_key IS NOT NULL"
        )
        params: list[Any] = []
        if status:
            sql += " AND status = ?"
            params.append(status)
        sql += " GROUP BY anchor_key HAVING hits >= ? ORDER BY hits DESC, anchor_key"
        params.append(min_count)
        conn = self._connect()
        try:
            rows = conn.execute(sql, params).fetchall()
        finally:
            conn.close()
        return [(row["anchor_key"], int(row["hits"])) for row in rows]

    def by_kind(self, *, status: str | None = None) -> list[tuple[str, int]]:
        if not self.path.exists():
            return []
        sql = "SELECT COALESCE(NULLIF(kind, ''), '(без категории)') AS bucket, COUNT(*) AS hits FROM error_notes"
        params: list[Any] = []
        if status:
            sql += " WHERE status = ?"
            params.append(status)
        sql += " GROUP BY bucket ORDER BY hits DESC, bucket"
        conn = self._connect()
        try:
            rows = conn.execute(sql, params).fetchall()
        finally:
            conn.close()
        return [(row["bucket"], int(row["hits"])) for row in rows]


def _note_fields(row: sqlite3.Row) -> dict[str, Any]:
    return {
        "id": int(row["id"]),
        "created_utc": row["created_utc"],
        "anchor": row["anchor"],
        "comment": row["comment"],
        "kind": row["kind"],
        "fragment_id": row["fragment_id"],
        "source_hash": row["source_hash"],
        "status": row["status"],
        "closed_utc": row["closed_utc"],
        "resolution": row["resolution"],
    }


def local_stamp(created_utc: str) -> str:
    try:
        return datetime.fromisoformat(created_utc).astimezone().strftime("%d.%m %H:%M")
    except ValueError:
        return str(created_utc)


def render(notes: list[ErrorNote]) -> str:
    blocks: list[str] = []
    for note in notes:
        head = f"#{note.id}  {local_stamp(note.created_utc)}  [{note.status}]"
        if note.anchor:
            head += f"  «{note.anchor}»"
        if note.kind:
            head += f"  ({note.kind})"
        lines = [head]
        for line in note.comment.splitlines():
            lines.append(f"    {line}")
        if note.resolution:
            lines.append(f"    -> закрыто: {note.resolution}")
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m cognitive_popups.notes",
        description="Review and close error notes.",
    )
    parser.add_argument("--db", default=None, help=f"database path (default {DEFAULT_DB})")
    parser.add_argument("--open", dest="open_only", action="store_true", help="only open notes (default)")
    parser.add_argument("--all", dest="all_notes", action="store_true", help="open and closed notes")
    parser.add_argument("--repeats", action="store_true", help="anchors seen more than once")
    parser.add_argument("--by-kind", dest="by_kind", action="store_true", help="group by category")
    parser.add_argument("--close", type=int, metavar="ID", default=None, help="mark a note as closed")
    parser.add_argument("--delete", type=int, metavar="ID", default=None, help="remove a note for good")
    parser.add_argument("-m", "--message", default=None, help="what was done about it")
    parser.add_argument("--json", action="store_true", help="raw notes as JSON")
    args = parser.parse_args(argv)

    store = NoteStore(args.db)
    try:
        if args.close is not None:
            note = store.close(args.close, args.message)
            print(f"#{note.id} closed")
            return 0
        if args.delete is not None:
            note = store.delete(args.delete)
            print(f"#{note.id} deleted")
            return 0
        if args.repeats:
            rows = store.repeats(status=None if args.all_notes else STATUS_OPEN)
            if not rows:
                print("no repeated anchors", file=sys.stderr)
                return 1
            for key, hits in rows:
                print(f"{hits:>3}×  {key}")
            return 0
        if args.by_kind:
            rows = store.by_kind(status=None if args.all_notes else STATUS_OPEN)
            if not rows:
                print("no notes yet", file=sys.stderr)
                return 1
            for bucket, hits in rows:
                print(f"{hits:>3}  {bucket}")
            return 0
        notes = store.list(status=None if args.all_notes else STATUS_OPEN)
    except NoteError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    if not notes:
        print(f"no notes in {store.path}", file=sys.stderr)
        return 1
    if args.json:
        import json

        print(json.dumps([note.to_dict() for note in notes], ensure_ascii=False, indent=2))
        return 0
    print(render(notes))
    return 0


if __name__ == "__main__":  # pragma: no cover - CLI entry point
    raise SystemExit(main())
