"""Weekly upkeep: telemetry retention, backup pruning, VACUUM.

    python3 -m cognitive_popups.maintain [--dry-run] [--no-vacuum] [--json]
    cog maintain

Only telemetry ages out. The reader's own words and the ledger are never
deleted here: notes, predictions, fragments, cues, steps, intents and returns
are not even opened for writing. What goes, and when:

    obs_requests       prompt and raw response text  -> emptied after 30 days
                       (the row, its status and timings stay)
    obs_artifacts      *_input, selected_source, rendered_payload -> after 60 days
    obs_presentations  window shown/closed rows      -> after 90 days
    events             interaction telemetry          -> after 180 days
    *.backup-*.sqlite3 automatic copies              -> three newest kept

`cognitive-maintain.timer` runs this weekly at Nice=19.
"""
from __future__ import annotations

import argparse
import json
import sqlite3
import sys
import time
from contextlib import closing
from pathlib import Path
from typing import Any

from .observation import KEEP_BACKUPS, prune_backups
from .paths import db_path

DAY = 86400.0
REQUEST_TEXT_DAYS = 30
INPUT_ARTIFACT_DAYS = 60
PRESENTATION_DAYS = 90
EVENT_DAYS = 180

#: Tables this module must never write to. Checked by the tests.
PROTECTED_TABLES = ("error_notes", "predictions", "prediction_fragments", "fragments", "cues",
                    "steps", "step_closures", "intents", "returns", "return_answers", "sessions",
                    "feynman_checks", "tasks", "thoughts")

#: (database, label, count sql, change sql, age in days). The change keeps the
#: row where the row itself is evidence (a request happened), and deletes it
#: where the row is only a copy of text the ledger already holds.
RULES = (
    ("records.sqlite3", "obs_requests: текст запроса и ответа",
     "SELECT count(*) FROM obs_requests WHERE started_epoch < ?"
     " AND (messages_json != 'null' OR raw_response IS NOT NULL)",
     "UPDATE obs_requests SET messages_json = 'null', raw_response = NULL WHERE started_epoch < ?"
     " AND (messages_json != 'null' OR raw_response IS NOT NULL)",
     REQUEST_TEXT_DAYS),
    ("records.sqlite3", "obs_artifacts: копии входа",
     "SELECT count(*) FROM obs_artifacts WHERE created_epoch < ? AND (kind LIKE '%\\_input' ESCAPE '\\'"
     " OR kind IN ('selected_source', 'rendered_payload'))",
     "DELETE FROM obs_artifacts WHERE created_epoch < ? AND (kind LIKE '%\\_input' ESCAPE '\\'"
     " OR kind IN ('selected_source', 'rendered_payload'))",
     INPUT_ARTIFACT_DAYS),
    ("records.sqlite3", "obs_presentations",
     "SELECT count(*) FROM obs_presentations WHERE created_epoch < ?",
     "DELETE FROM obs_presentations WHERE created_epoch < ?",
     PRESENTATION_DAYS),
    ("events.sqlite3", "events",
     "SELECT count(*) FROM events WHERE ts_epoch < ?",
     "DELETE FROM events WHERE ts_epoch < ?",
     EVENT_DAYS),
)

DATABASES = ("records.sqlite3", "events.sqlite3", "notes.sqlite3", "universe.sqlite3")


def _tables(conn: sqlite3.Connection) -> set[str]:
    return {row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}


def _table_of(sql: str) -> str:
    words = sql.replace("(", " ").split()
    for keyword in ("FROM", "UPDATE"):
        if keyword in words:
            return words[words.index(keyword) + 1]
    raise ValueError(sql)


def maintain(*, now: float | None = None, dry_run: bool = False, vacuum: bool = True,
             keep_backups: int = KEEP_BACKUPS) -> dict[str, Any]:
    """Apply the retention rules; returns what changed (or would, with `dry_run`)."""
    now = time.time() if now is None else float(now)
    report: dict[str, Any] = {"dry_run": dry_run, "rules": [], "backups_removed": [], "vacuumed": [],
                              "errors": []}
    touched: set[Path] = set()
    for database, label, count_sql, change_sql, days in RULES:
        path = db_path(database)
        entry = {"database": database, "what": label, "older_than_days": days, "rows": 0}
        report["rules"].append(entry)
        if not path.exists():
            continue
        cutoff = now - days * DAY
        try:
            with closing(sqlite3.connect(path, timeout=30)) as conn:
                if _table_of(count_sql) not in _tables(conn):
                    continue
                entry["rows"] = int(conn.execute(count_sql, (cutoff,)).fetchone()[0])
                if entry["rows"] and not dry_run:
                    with conn:
                        conn.execute(change_sql, (cutoff,))
                    touched.add(path)
        except sqlite3.Error as exc:
            report["errors"].append(f"{database}: {exc}")
    for database in DATABASES:
        path = db_path(database)
        removed = prune_backups(path, keep=keep_backups, dry_run=dry_run)
        report["backups_removed"].extend(str(item) for item in removed)
    if vacuum and not dry_run:
        for path in sorted(touched):
            try:
                with closing(sqlite3.connect(path, timeout=30)) as conn:
                    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
                    conn.execute("VACUUM")
                report["vacuumed"].append(str(path))
            except sqlite3.Error as exc:
                report["errors"].append(f"vacuum {path.name}: {exc}")
    return report


def render(report: dict[str, Any]) -> str:
    head = "Проверка без изменений (--dry-run)." if report["dry_run"] else "Обслуживание выполнено."
    lines = [head]
    verb = "затронет" if report["dry_run"] else "затронуто"
    for entry in report["rules"]:
        lines.append(f"  {entry['what']} старше {entry['older_than_days']} дн.: {verb} {entry['rows']}")
    gone = "будет удалено" if report["dry_run"] else "удалено"
    lines.append(f"  старых резервных копий {gone}: {len(report['backups_removed'])}")
    for name in report["backups_removed"]:
        lines.append(f"    {name}")
    if report["vacuumed"]:
        lines.append("  сжато: " + ", ".join(Path(p).name for p in report["vacuumed"]))
    for error in report["errors"]:
        lines.append(f"  ошибка: {error}")
    lines.append("  заметки, гипотезы, журнал и возвраты не трогаются никогда.")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="cog maintain", description=__doc__.split("\n\n")[0])
    parser.add_argument("--dry-run", action="store_true", help="только посчитать, ничего не менять")
    parser.add_argument("--no-vacuum", action="store_true", help="не сжимать базы после чистки")
    parser.add_argument("--json", action="store_true", help="отчёт в JSON")
    args = parser.parse_args(argv)
    report = maintain(dry_run=args.dry_run, vacuum=not args.no_vacuum)
    print(json.dumps(report, ensure_ascii=False, indent=2) if args.json else render(report))
    return 1 if report["errors"] else 0


if __name__ == "__main__":  # pragma: no cover - CLI entry point
    sys.exit(main())
