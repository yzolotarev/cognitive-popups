"""The 15-minute step (Alt+I): the reader's own unit of study.

A step is one concrete thing the reader means to do in the next minutes, in
their words ("объясню, чем VLOOKUP отличается от XLOOKUP"), with a short
deadline: work expands to the time it is given, so the time given is short.
The step ends with the reader's own mark (done / partly / not) and one line of
what they took away; that line opens the next step as a bridge.

A study session is a run of steps. It starts with an optional one-line goal
("ради чего") and ends when the reader closes it, or silently after a long
pause. Nothing here opens a window or calls a model: the desktop does the
windows, this module only decides and remembers.
"""
from __future__ import annotations

import os
import re
import time
from dataclasses import dataclass
from datetime import datetime

DEFAULT_MINUTES = 15
#: A pause longer than this starts a new study session (and asks for its goal).
SESSION_GAP_SECONDS = 3 * 60 * 60
#: Test hook: a step of this many seconds instead of minutes, so a live check
#: does not have to wait a quarter of an hour.
ENV_SECONDS = "COGNITIVE_STEP_SECONDS"

READING = "читаю"
OUTCOMES = {"done": "да", "partly": "частично", "not": "нет"}
OUTCOME_MARK = {"done": "✓", "partly": "½", "not": "✗"}

_LEADING_MINUTES = re.compile(r"^\s*(\d{1,3})\s*(?:мин(?:ут[аы]?)?\.?|m(?:in)?)?\s*[:,.\-–]?\s+(.+)$",
                              re.IGNORECASE | re.DOTALL)


def parse_step(raw: str, default_minutes: int = DEFAULT_MINUTES) -> tuple[int, str]:
    """"25 разберу X" -> (25, "разберу X"); plain text -> (15, text).

    A number is read as minutes only when text follows it, so a step that is
    itself a number ("3 задачи решу") keeps working when it reads naturally.
    """
    text = " ".join((raw or "").split())
    match = _LEADING_MINUTES.match(text)
    if match:
        minutes = int(match.group(1))
        rest = match.group(2).strip()
        if 1 <= minutes <= 180 and rest:
            return minutes, rest
    return default_minutes, text


def step_seconds(minutes: int) -> float:
    override = os.environ.get(ENV_SECONDS, "").strip()
    if override:
        try:
            value = float(override)
            if value > 0:
                return value
        except ValueError:
            pass
    return minutes * 60.0


def is_new_session(last_step: dict | None, closure_of_last: dict | None, now: float | None = None,
                   gap: float = SESSION_GAP_SECONDS) -> bool:
    if last_step is None or closure_of_last is not None:
        return True
    ended = last_step.get("finished_epoch") or last_step.get("deadline_epoch") or 0
    return ((now if now is not None else time.time()) - float(ended)) > gap


@dataclass(frozen=True)
class Bridge:
    """The reader's own line that opens the next step, with where it came from."""

    text: str
    hint: str


def bridge(last_step: dict | None, last_closure: dict | None, *, new_session: bool) -> Bridge | None:
    """What the reader left for themselves last time.

    Inside a session: the previous step's takeaway. At the start of a session:
    what they said was left when they closed the last one, else the last
    takeaway they wrote. Only the reader's own words; nothing is invented.
    """
    if not new_session and last_step and last_step.get("takeaway"):
        return Bridge(last_step["takeaway"], _hint("прошлый шаг", last_step.get("text", "")))
    if new_session and last_closure and last_closure.get("remaining"):
        return Bridge(last_closure["remaining"], _hint(_day(last_closure.get("closed_utc")), "что осталось"))
    if last_step and last_step.get("takeaway"):
        return Bridge(last_step["takeaway"], _hint(_day(last_step.get("finished_utc")),
                                                   last_step.get("text", "")))
    return None


def _day(ts: str | None) -> str:
    try:
        return datetime.fromisoformat(str(ts)).astimezone().strftime("%d.%m")
    except (TypeError, ValueError):
        return ""


def _hint(*parts: str) -> str:
    return " · ".join(part for part in parts if part)


def end_prompt(text: str, minutes: float, *, early: bool = False) -> str:
    head = "Закончить шаг сейчас?" if early else f"{_minutes(minutes)}. Сделал шаг?"
    return f"{head}\n\n{text}"


def _minutes(minutes: float) -> str:
    whole = int(round(minutes))
    if whole % 10 == 1 and whole % 100 != 11:
        unit = "минута"
    elif whole % 10 in (2, 3, 4) and whole % 100 not in (12, 13, 14):
        unit = "минуты"
    else:
        unit = "минут"
    return f"{whole} {unit}"


def session_summary(goal: str, steps: list[dict], notes: list) -> str:
    """The session handed back to the reader: goal, steps, takeaways, own notes."""
    lines: list[str] = []
    if goal:
        lines += [f"Ради чего: {goal}", ""]
    for step in steps:
        mark = OUTCOME_MARK.get(step.get("outcome") or "", "·")
        lines.append(f"{mark} {step.get('text', '')}")
        if step.get("takeaway"):
            lines.append(f"   → {step['takeaway']}")
    own = [getattr(note, "comment", "") for note in notes if getattr(note, "comment", "")]
    if own:
        lines += ["", "Твои заметки:"]
        lines += [f"✎ {comment}" for comment in own]
    return "\n".join(lines).strip()
