"""One study timeline from the three stores: events, records and notes.

The stores were built tool by tool, so no single one tells what a session
looked like. This reads all three (read-only), puts every row on one local
clock and prints the story of a day:

    python -m cognitive_popups.timeline              # today
    python -m cognitive_popups.timeline 2026-09-20   # one day (local date)
    python -m cognitive_popups.timeline --days       # one line per day
    python -m cognitive_popups.timeline --full       # keep technical events

Only the reader's own words (hypotheses, notes, goals, answers) and moments
where the flow broke (errors, empty buffer, dropped or cancelled requests) are
kept by default; window bookkeeping is noise for this question.
"""
from __future__ import annotations

import argparse
import os
import sqlite3
import sys
from collections import Counter
from dataclasses import dataclass
from datetime import date, datetime
from pathlib import Path

#: Outcomes that mean "the reader wanted something and did not get it".
FRICTION = {
    "buffer is empty": "буфер пуст",
    "busy, request dropped": "занято, нажатие потеряно",
    "cancelled": "отменено",
    "closed without saving": "закрыто без сохранения",
    "no selection": "нет выделения",
    "practice cancelled": "практика отменена",
    "retry offered": "предложена повторная попытка",
}
#: Windows whose title is the question the reader was shown.
TITLED_WINDOWS = {"input", "goal", "intent", "task_attempt", "keys"}
#: Generic prompts that repeat on every call and say nothing about the moment.
GENERIC_TITLES = ("Какие термины не понял", "Сформулируй одну конкретную гипотезу",
                  "Что не понял или что спросить")


@dataclass(frozen=True)
class Entry:
    at: datetime
    kind: str
    text: str
    extra: str = ""

    def line(self) -> str:
        tail = f"  · {self.extra}" if self.extra else ""
        return f"{self.at:%H:%M:%S}  {self.kind:<9} {self.text}{tail}"


def state_paths() -> dict[str, Path]:
    root = Path(os.environ.get("COGNITIVE_STATE_DIR") or "~/.local/state/cognitive-popups").expanduser()
    return {
        "events": Path(os.environ.get("COGNITIVE_EVENT_DB") or root / "events.sqlite3").expanduser(),
        "records": Path(os.environ.get("COGNITIVE_RECORD_DB") or root / "records.sqlite3").expanduser(),
        "notes": Path(os.environ.get("COGNITIVE_NOTES_DB") or root / "notes.sqlite3").expanduser(),
    }


def local(ts: str) -> datetime:
    return datetime.fromisoformat(ts.replace("Z", "+00:00")).astimezone()


def short(value, limit: int) -> str:
    text = " ".join(str(value or "").split())
    return text if len(text) <= limit else text[:limit - 1] + "…"


def _rows(path: Path, sql: str) -> list[tuple]:
    if not path.is_file():
        return []
    try:
        connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        try:
            return connection.execute(sql).fetchall()
        finally:
            connection.close()
    except sqlite3.Error:
        # An older store without a table or column simply contributes nothing.
        return []


def _event_entry(ts, window, event, detail, full: bool) -> Entry | None:
    window = window or ""
    detail = detail or ""
    if event == "error":
        return Entry(local(ts), "СБОЙ", f"{window}: {short(detail, 160)}")
    if event == "action" and detail in FRICTION:
        return Entry(local(ts), "сорвалось", f"{window}: {FRICTION[detail]}")
    if event == "hotkey":
        return Entry(local(ts), "клавиша", detail or window)
    if event == "panel":
        return Entry(local(ts), "панель", window.removeprefix("hud:"))
    if event == "session_split":
        return Entry(local(ts), "пауза", detail)
    if (event == "window_open" and window in TITLED_WINDOWS and detail
            and not detail.startswith(GENERIC_TITLES)):
        return Entry(local(ts), "окно", short(detail, 140))
    if full:
        return Entry(local(ts), "·", f"{window}:{event}", short(detail, 100))
    return None


def collect(paths: dict[str, Path] | None = None, *, full: bool = False) -> list[Entry]:
    paths = paths or state_paths()
    entries: list[Entry] = []
    for row in _rows(paths["events"], "select ts_utc, window, event, detail from events order by id"):
        entry = _event_entry(*row, full=full)
        if entry is not None:
            entries.append(entry)
    records = paths["records"]
    for ts, text, criterion, status in _rows(
            records, "select created_utc, text, criterion, status from intents"):
        entries.append(Entry(local(ts), "ЦЕЛЬ", short(text, 220),
                             short(f"критерий: {criterion}" if criterion else status, 100)))
    for ts, source in _rows(records, "select created_utc, source_text from fragments"):
        entries.append(Entry(local(ts), "фрагмент", short(source, 110)))
    for ts, hypothesis, status, mismatch, delta in _rows(
            records, "select created_utc, hypothesis, status, mismatch, one_delta from predictions"):
        entries.append(Entry(local(ts), "ГИПОТЕЗА", f"[{status}] {short(hypothesis, 200)}",
                             short(delta or mismatch, 140)))
    for ts, question, explanation, status in _rows(
            records, "select created_utc, question, explanation, status from feynman_checks"):
        entries.append(Entry(local(ts), "ФЕЙНМАН", f"[{status}] {short(question, 140)}",
                             "ответ: " + short(explanation, 160)))
    for ts, kind, condition, status, attempt in _rows(
            records, "select created_utc, task_type, condition, status, attempt from tasks"):
        entries.append(Entry(local(ts), "ЗАДАЧА", f"[{kind}/{status}] {short(condition, 160)}",
                             "попытка: " + short(attempt, 100) if attempt else "без попытки"))
    for ts, comment, anchor in _rows(
            paths["notes"], "select created_utc, comment, anchor from error_notes"):
        entries.append(Entry(local(ts), "ЗАМЕТКА", short(comment, 400),
                             "к: " + short(anchor, 70) if anchor else ""))
    entries.sort(key=lambda entry: entry.at)
    return entries


def day_summary(entries: list[Entry]) -> list[str]:
    """One line per local day: where the study (own words) actually happened."""
    days: dict[date, list[Entry]] = {}
    for entry in entries:
        days.setdefault(entry.at.date(), []).append(entry)
    lines = [f"{'день':<10}  {'с':>5}-{'до':<5}  гипот заметк фрагм клав сорв сбой"]
    for day, items in sorted(days.items()):
        count = Counter(entry.kind for entry in items)
        lines.append(
            f"{day:%Y-%m-%d}  {items[0].at:%H:%M}-{items[-1].at:%H:%M}  "
            f"{count['ГИПОТЕЗА']:>5} {count['ЗАМЕТКА']:>6} {count['фрагмент']:>5} "
            f"{count['клавиша']:>4} {count['сорвалось']:>4} {count['СБОЙ']:>4}"
        )
    return lines


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m cognitive_popups.timeline",
        description="Лента учёбы за день из всех хранилищ программы (только чтение).",
    )
    parser.add_argument("day", nargs="?", help="местная дата ГГГГ-ММ-ДД (по умолчанию сегодня)")
    parser.add_argument("--days", action="store_true", help="сводка по дням вместо ленты")
    parser.add_argument("--full", action="store_true", help="включить технические события окон")
    args = parser.parse_args(argv)

    entries = collect(full=args.full)
    if args.days:
        print("\n".join(day_summary(entries)))
        return 0
    try:
        wanted = date.fromisoformat(args.day) if args.day else date.today()
    except ValueError:
        parser.error(f"дата должна быть ГГГГ-ММ-ДД, а не {args.day!r}")
    chosen = [entry for entry in entries if entry.at.date() == wanted]
    if not chosen:
        print(f"{wanted}: записей нет", file=sys.stderr)
        return 1
    print("\n".join(entry.line() for entry in chosen))
    return 0


if __name__ == "__main__":  # pragma: no cover - CLI entry point
    raise SystemExit(main())
