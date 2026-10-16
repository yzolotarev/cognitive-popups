"""Обзор: a mirror, not a scoreboard (`cog stats`, Alt+K -> «Обзор»).

What it shows, in this order:

1. your misses, verbatim: the last hypotheses the text did not support, with
   the place they went wrong — material for the next move;
2. where you stumble again: terms of the four words seen three times or more,
   and note anchors written more than once;
3. what comes back soon (spaced returns);
4. open notes: how many, and the oldest one in your words;
5. rhythm: how long usually passes from the four words to your hypothesis;
6. one small line of counts;
7. where the tool let you down: lost presses, model failures, refused captures.

What it never shows: percentages, streaks, minutes of study, week-on-week
arrows, levels. A rate of confirmed guesses rewards safe guesses; the tests
refuse any '%' or streak/minute wording in the output.

`overview(state_dir, since_epoch)` is pure over the files (read-only, `mode=ro`);
`render(data)` turns its dict into text.
"""
from __future__ import annotations

import argparse
import json
import sqlite3
import statistics
import sys
import time
from collections import Counter
from contextlib import closing
from datetime import datetime
from pathlib import Path
from typing import Any

from .paths import state_dir as default_state_dir

DAY = 86400.0
MISSED = ("contradicted", "not_supported", "partially_confirmed", "partially")
MISSES_SHOWN = 5
TERM_REPEATS = 3
#: A hypothesis this long after the four words belongs to something else.
RHYTHM_WINDOW_SECONDS = 1800.0


def _rows(path: Path, sql: str, params: tuple = ()) -> list[sqlite3.Row]:
    if not path.exists():
        return []
    try:
        with closing(sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True, timeout=2)) as conn:
            conn.row_factory = sqlite3.Row
            return conn.execute(sql, params).fetchall()
    except sqlite3.Error:
        return []


def _one(path: Path, sql: str, params: tuple = ()) -> int:
    rows = _rows(path, sql, params)
    return int(rows[0][0] or 0) if rows else 0


def _local_day(epoch: float) -> str:
    return datetime.fromtimestamp(float(epoch)).strftime("%d.%m")


def overview(state_dir: str | Path | None = None, since_epoch: float | None = None,
             *, now: float | None = None) -> dict[str, Any]:
    """Everything the overview shows, as plain data (see module docstring)."""
    root = Path(state_dir) if state_dir is not None else default_state_dir()
    now = time.time() if now is None else float(now)
    since = 0.0 if since_epoch is None else float(since_epoch)
    records, events, notes = root / "records.sqlite3", root / "events.sqlite3", root / "notes.sqlite3"
    marks = ", ".join("?" for _ in MISSED)

    misses = [dict(row) for row in _rows(records,
        "SELECT id, hypothesis, status, mismatch, evidence, created_epoch FROM predictions"
        f" WHERE status IN ({marks}) AND created_epoch >= ? AND trim(hypothesis) != ''"
        " ORDER BY created_epoch DESC LIMIT ?", (*MISSED, since, MISSES_SHOWN))]

    # Folded in Python: SQLite's lower() leaves Cyrillic as it is.
    seen: Counter[str] = Counter()
    for row in _rows(records, "SELECT c.term FROM cues c JOIN fragments f ON f.id = c.fragment_id"
                              " WHERE f.created_epoch >= ? AND trim(c.term) != ''", (since,)):
        seen[" ".join(str(row["term"]).split()).casefold()] += 1
    terms = [{"term": term, "count": hits}
             for term, hits in sorted(seen.items(), key=lambda item: (-item[1], item[0]))
             if hits >= TERM_REPEATS][:8]
    anchors = [{"anchor": row["anchor"], "count": int(row["hits"])} for row in _rows(notes,
        "SELECT min(anchor) AS anchor, count(*) AS hits FROM error_notes"
        " WHERE anchor_key IS NOT NULL AND created_epoch >= ?"
        " GROUP BY anchor_key HAVING hits >= 2 ORDER BY hits DESC, anchor LIMIT 8", (since,))]

    coming = [dict(row) for row in _rows(records,
        "SELECT r.prediction_id, r.due_epoch, p.hypothesis FROM returns r"
        " JOIN predictions p ON p.id = r.prediction_id"
        " WHERE r.due_epoch IS NOT NULL AND r.due_epoch <= ? ORDER BY r.due_epoch LIMIT 5",
        (now + 7 * DAY,))]

    open_notes = _one(notes, "SELECT count(*) FROM error_notes WHERE status = 'open'")
    oldest = _rows(notes, "SELECT anchor, comment, created_epoch FROM error_notes"
                          " WHERE status = 'open' ORDER BY created_epoch, id LIMIT 1")

    rhythm = _rhythm(events, since)

    counts = {
        "сессий": _one(records, "SELECT count(*) FROM sessions WHERE created_epoch >= ?", (since,)),
        "4 слова": _one(records, "SELECT count(*) FROM fragments WHERE created_epoch >= ?", (since,)),
        "гипотез": _one(records, "SELECT count(*) FROM predictions WHERE created_epoch >= ?", (since,)),
        "заметок": _one(notes, "SELECT count(*) FROM error_notes WHERE created_epoch >= ?", (since,)),
        "шагов": _one(records, "SELECT count(*) FROM steps WHERE started_epoch >= ?", (since,)),
    }

    system = {
        "нажатий потеряно": _one(events, "SELECT count(*) FROM events WHERE ts_epoch >= ?"
                                         " AND event = 'action' AND detail LIKE 'busy%'", (since,)),
        "окон с ошибкой": _one(events, "SELECT count(*) FROM events WHERE ts_epoch >= ? AND event = 'error'",
                               (since,)),
        "модель не ответила": _one(records, "SELECT count(*) FROM obs_requests WHERE started_epoch >= ?"
                                            " AND status IN ('transport_error', 'timeout', 'empty_response',"
                                            " 'bridge_error')", (since,)),
        "текст не захвачен": _one(events, "SELECT count(*) FROM events WHERE ts_epoch >= ?"
                                          " AND event = 'action' AND detail IN ('no selection', 'buffer is empty')",
                                  (since,)),
    }

    return {
        "since_epoch": since,
        "misses": misses,
        "repeated_terms": terms,
        "repeated_anchors": anchors,
        "returns": coming,
        "open_notes": open_notes,
        "oldest_note": dict(oldest[0]) if oldest else None,
        "rhythm": rhythm,
        "counts": counts,
        "system": system,
    }


def _rhythm(events: Path, since: float) -> dict[str, Any] | None:
    """Seconds from each four-words result to the next hypothesis press."""
    rows = _rows(events, "SELECT ts_epoch, window FROM events WHERE ts_epoch >= ?"
                         " AND ((event = 'result' AND window = 'seed') OR (event = 'hotkey' AND window = 'prediction'))"
                         " ORDER BY ts_epoch", (since,))
    gaps: list[float] = []
    words_at: float | None = None
    for row in rows:
        if row["window"] == "seed":
            words_at = float(row["ts_epoch"])
        elif words_at is not None:
            gap = float(row["ts_epoch"]) - words_at
            if 0 <= gap <= RHYTHM_WINDOW_SECONDS:
                gaps.append(gap)
            words_at = None
    if not gaps:
        return None
    return {"median_seconds": round(statistics.median(gaps)), "n": len(gaps)}


def render(data: dict[str, Any]) -> str:
    lines: list[str] = ["ОБЗОР"]

    lines += ["", "Твои промахи — материал для следующего хода"]
    if data["misses"]:
        for miss in data["misses"]:
            lines.append(f"  {_local_day(miss['created_epoch'])}  {miss['hypothesis']}")
            if miss.get("mismatch"):
                lines.append(f"         расхождение: {miss['mismatch']}")
            if miss.get("evidence"):
                lines.append(f"         в тексте: {miss['evidence']}")
    else:
        lines.append("  пока нет")

    lines += ["", "Где спотыкаешься повторно"]
    stumbles = [f"{item['term']} ×{item['count']}" for item in data["repeated_terms"]]
    stumbles += [f"«{item['anchor']}» ×{item['count']}" for item in data["repeated_anchors"]]
    lines.append("  " + (" · ".join(stumbles) if stumbles else "повторов нет"))

    lines += ["", "Что вернётся"]
    if data["returns"]:
        for item in data["returns"]:
            lines.append(f"  {_local_day(item['due_epoch'])}  {item['hypothesis']}")
    else:
        lines.append("  ничего на этой неделе")

    lines += ["", f"Открытые заметки: {data['open_notes']}"]
    oldest = data["oldest_note"]
    if oldest:
        anchor = f"«{oldest['anchor']}» " if oldest.get("anchor") else ""
        lines.append(f"  самая старая ({_local_day(oldest['created_epoch'])}): {anchor}{oldest['comment']}")

    rhythm = data["rhythm"]
    lines += ["", "Сколько обычно проходит от 4 слов до твоей гипотезы"]
    lines.append(f"  около {rhythm['median_seconds']} с (по {rhythm['n']} случаям)" if rhythm else "  пока не видно")

    lines += ["", "  " + " · ".join(f"{name} {value}" for name, value in data["counts"].items())]

    lines += ["", "Где подвела система"]
    lines.append("  " + " · ".join(f"{name}: {value}" for name, value in data["system"].items()))
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="cog stats", description="Обзор: зеркало, не табло.")
    period = parser.add_mutually_exclusive_group()
    period.add_argument("--days", type=int, default=7, help="за сколько дней (по умолчанию 7)")
    period.add_argument("--all", action="store_true", help="за всё время")
    parser.add_argument("--json", action="store_true", help="данные в JSON")
    args = parser.parse_args(argv)
    since = None if args.all else time.time() - max(1, args.days) * DAY
    data = overview(None, since)
    print(json.dumps(data, ensure_ascii=False, indent=2) if args.json else render(data))
    return 0


if __name__ == "__main__":  # pragma: no cover - CLI entry point
    sys.exit(main())
