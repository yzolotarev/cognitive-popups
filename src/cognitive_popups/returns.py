"""Spaced returns: a missed hypothesis comes back after 2, 5, 12 and 30 days.

The return shows the reader's own hypothesis and the place it went wrong, never
the source text: the point is to rebuild the idea, not to reread it. Answers:

    better   the idea holds now      -> next interval; after 30 days it is done
    same     still shaky             -> back to 2 days
    skip     not now                 -> back to 2 days (and it counts as shown)

At most one return is shown per calendar day. "Confirmed later" is only claimed
when a `better` came on a day at least two calendar days after the hypothesis,
so the same evening's second look never counts as remembering.

Rows live in records.sqlite3 (`returns`, plus the `return_answers` history);
the interval arithmetic is pure and lives at the top of this module.
"""
from __future__ import annotations

import sqlite3
from datetime import date, datetime
from pathlib import Path
from typing import Any

from .records import RecordError, RecordStore

DAY = 86400.0
#: Days until the next return, by step. Step 0 is the first return.
INTERVALS = (2, 5, 12, 30)
OUTCOMES = ("better", "same", "skip")
DONE = "done"
#: Statuses that make a hypothesis worth returning to.
MISSED_STATUSES = ("contradicted", "not_supported", "partially_confirmed", "partially")
CONFIRM_GAP_DAYS = 2


def local_day(epoch: float) -> date:
    return datetime.fromtimestamp(float(epoch)).date()


def advance(step: int, outcome: str) -> tuple[int, int | None]:
    """(next step, days until it) after `outcome` at `step`; days None = done."""
    if outcome not in OUTCOMES:
        raise ValueError(f"unknown return outcome {outcome!r}")
    if outcome != "better":
        return 0, INTERVALS[0]
    following = int(step) + 1
    if following >= len(INTERVALS):
        return following, None
    return following, INTERVALS[following]


def counts_as_confirmed_later(created_epoch: float, better_epochs: list[float]) -> bool:
    start = local_day(created_epoch)
    return any((local_day(epoch) - start).days >= CONFIRM_GAP_DAYS for epoch in better_epochs)


class ReturnStore:
    """The `returns` rows of one ledger file."""

    def __init__(self, path: str | Path | RecordStore | None = None):
        self.records = path if isinstance(path, RecordStore) else RecordStore(path)

    def _conn(self) -> sqlite3.Connection:
        return self.records._connect()

    def schedule(self, prediction_id: str, now: float) -> bool:
        """First return two days from `now`. A second call changes nothing."""
        conn = self._conn()
        try:
            cursor = conn.execute(
                "INSERT OR IGNORE INTO returns (prediction_id, due_epoch, step, updated_epoch)"
                " VALUES (?, ?, 0, ?)", (prediction_id, float(now) + INTERVALS[0] * DAY, float(now)))
            conn.commit()
            return bool(cursor.rowcount)
        except sqlite3.Error as exc:
            raise RecordError(f"cannot schedule a return in {self.records.path}: {exc}") from exc
        finally:
            conn.close()

    def shown_today(self, now: float, conn: sqlite3.Connection | None = None) -> bool:
        own = conn is None
        conn = conn or self._conn()
        try:
            today = local_day(now)
            return any(local_day(row[0]) == today for row in conn.execute(
                "SELECT shown_epoch FROM returns WHERE shown_epoch IS NOT NULL"
                " AND shown_epoch > ?", (float(now) - 2 * DAY,)))
        finally:
            if own:
                conn.close()

    def due(self, now: float) -> dict[str, Any] | None:
        """The oldest due return with its hypothesis, or None (nothing due, or one shown today)."""
        conn = self._conn()
        try:
            if self.shown_today(now, conn):
                return None
            row = conn.execute(
                "SELECT r.prediction_id, r.due_epoch, r.step, p.hypothesis, p.mismatch, p.status,"
                " p.created_epoch FROM returns r JOIN predictions p ON p.id = r.prediction_id"
                " WHERE r.due_epoch IS NOT NULL AND r.due_epoch <= ?"
                " ORDER BY r.due_epoch, r.prediction_id LIMIT 1", (float(now),)).fetchone()
            return dict(row) if row is not None else None
        finally:
            conn.close()

    def upcoming(self, now: float, *, within_days: float = 7, limit: int = 5) -> list[dict[str, Any]]:
        """Returns due within `within_days`, soonest first (for the overview)."""
        conn = self._conn()
        try:
            rows = conn.execute(
                "SELECT r.prediction_id, r.due_epoch, r.step, p.hypothesis, p.mismatch, p.status"
                " FROM returns r JOIN predictions p ON p.id = r.prediction_id"
                " WHERE r.due_epoch IS NOT NULL AND r.due_epoch <= ?"
                " ORDER BY r.due_epoch LIMIT ?", (float(now) + within_days * DAY, int(limit)))
            return [dict(row) for row in rows]
        finally:
            conn.close()

    def record_answer(self, prediction_id: str, answer: str, outcome: str, now: float) -> dict[str, Any]:
        """Store the reader's answer and move the return; returns the new row."""
        conn = self._conn()
        try:
            row = conn.execute("SELECT step FROM returns WHERE prediction_id = ?",
                               (prediction_id,)).fetchone()
            if row is None:
                raise ValueError(f"no return for {prediction_id}")
            step, days = advance(int(row["step"]), outcome)
            due = None if days is None else float(now) + days * DAY
            stored = DONE if days is None else outcome
            text = " ".join(str(answer or "").split()) or None
            conn.execute(
                "UPDATE returns SET due_epoch = ?, step = ?, shown_epoch = ?, answer = ?, outcome = ?,"
                " updated_epoch = ? WHERE prediction_id = ?",
                (due, step, float(now), text, stored, float(now), prediction_id))
            conn.execute(
                "INSERT INTO return_answers (prediction_id, step, answer, outcome, answered_epoch)"
                " VALUES (?, ?, ?, ?, ?)", (prediction_id, int(row["step"]), text, outcome, float(now)))
            conn.commit()
            return dict(conn.execute("SELECT * FROM returns WHERE prediction_id = ?",
                                     (prediction_id,)).fetchone())
        except sqlite3.Error as exc:
            raise RecordError(f"cannot record a return answer in {self.records.path}: {exc}") from exc
        finally:
            conn.close()

    def confirmed_later(self, prediction_id: str) -> bool:
        conn = self._conn()
        try:
            created = conn.execute("SELECT created_epoch FROM predictions WHERE id = ?",
                                   (prediction_id,)).fetchone()
            if created is None:
                return False
            better = [row[0] for row in conn.execute(
                "SELECT answered_epoch FROM return_answers WHERE prediction_id = ? AND outcome = 'better'",
                (prediction_id,))]
        finally:
            conn.close()
        return counts_as_confirmed_later(created[0], better)


# Module-level shortcuts with the default ledger, for callers that hold no store.

def schedule(prediction_id: str, now: float, path: str | Path | None = None) -> bool:
    return ReturnStore(path).schedule(prediction_id, now)


def due(now: float, path: str | Path | None = None) -> dict[str, Any] | None:
    return ReturnStore(path).due(now)


def record_answer(prediction_id: str, answer: str, outcome: str, now: float,
                  path: str | Path | None = None) -> dict[str, Any]:
    return ReturnStore(path).record_answer(prediction_id, answer, outcome, now)


def confirmed_later(prediction_id: str, path: str | Path | None = None) -> bool:
    return ReturnStore(path).confirmed_later(prediction_id)
