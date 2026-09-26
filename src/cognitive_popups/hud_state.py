"""What the side panel may show, and how a hint stops being shown.

Two things live here, and nothing else:

* a read-only view of the ledger — the accepted goal (see `records.intents`) and
  the newest saved task that has not been attempted yet;
* the one piece of panel state that is not in the ledger: the reader's decision
  to stop highlighting that task.

There is no score, no "readiness" and no reading time.  A highlight means one
concrete thing: a task is already saved and can be opened.  Whether the reader
takes it up is their business, and whether it was any good is decided by the
session, not by this file.
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass
from datetime import date, datetime
from pathlib import Path

STATE_FILENAME = "hud.json"
DEFAULT_STATE_DIR = "~/.local/state/cognitive-popups"

#: Panel settings, all of them opt-in. `prepare_in_background` is off until the
#: reader turns it on: preparing practice spends model calls and time on its own
#: initiative, and nobody should have that happen to them by default.
DEFAULT_SETTINGS: dict[str, object] = {
    "prepare_in_background": False,
}
SETTING_KEYS = tuple(DEFAULT_SETTINGS)

#: Dismissed task ids are kept so a dismissed highlight cannot come back on the
#: next refresh.  Older ones are dropped: only the newest task can ever be
#: highlighted, so a long memory would serve nothing.
DISMISSED_LIMIT = 100

#: A task condition is shown as a short line; the whole condition is in the task
#: itself and in the attempt window, so the panel never has to carry it.
SUMMARY_LIMIT = 120

#: Only a task in this state is offered.  `practice_task` moves it to
#: `attempt_checked`, so a task stops being highlighted by being used.
OFFERED_STATUS = "generated"


@dataclass(frozen=True)
class GoalView:
    id: str
    text: str
    criterion: str


@dataclass(frozen=True)
class PracticeView:
    """A task that is already saved and can be opened right now."""

    task_id: str
    summary: str
    task_type: str
    created_utc: str
    from_current_session: bool

    def payload(self) -> dict:
        return {
            "task_id": self.task_id,
            "summary": self.summary,
            "task_type": self.task_type,
            "created_utc": self.created_utc,
            "from_current_session": self.from_current_session,
            "day": day_label(self.created_utc),
        }


@dataclass(frozen=True)
class PreparationView:
    """How far the background preparation has come, as the panel shows it."""

    preparation_id: str
    status: str
    completed_stage: int
    stage_count: int
    label: str
    practice_kind: str
    target: str
    task_id: str

    def payload(self) -> dict:
        return {
            "preparation_id": self.preparation_id,
            "status": self.status,
            "completed_stage": self.completed_stage,
            "stage_count": self.stage_count,
            "label": self.label,
            "practice_kind": self.practice_kind,
            "target": self.target,
            "task_id": self.task_id,
            "working": self.status in PREPARING_STATUSES,
        }


#: Statuses that mean work is happening right now, as opposed to a finished or
#: refused preparation. Imported by value rather than from the ledger module to
#: keep this file readable on its own; `records` remains the source of truth.
PREPARING_STATUSES = frozenset({"queued", "analyzing", "generating", "validating", "repairing"})


def state_path(state_dir: str | Path | None = None) -> Path:
    if state_dir is None:
        state_dir = os.environ.get("COGNITIVE_STATE_DIR") or DEFAULT_STATE_DIR
    return Path(state_dir).expanduser() / STATE_FILENAME


def read_state(path: str | Path | None = None) -> dict:
    """The whole panel file: the dismissed ids and the settings together.

    Read and written as one object so a settings change cannot drop the
    dismissals (or the other way round): the panel file has two halves and only
    one of them is being edited at a time.
    """
    target = Path(path) if path else state_path()
    try:
        raw = json.loads(target.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        raw = {}
    if isinstance(raw, list):  # the first release wrote a bare list of ids
        raw = {"dismissed": raw}
    if not isinstance(raw, dict):
        raw = {}
    dismissed = raw.get("dismissed")
    settings = raw.get("settings")
    merged = dict(DEFAULT_SETTINGS)
    if isinstance(settings, dict):
        for key in SETTING_KEYS:
            if key in settings:
                merged[key] = bool(settings[key])
    return {
        "dismissed": [str(item) for item in dismissed if str(item).strip()][-DISMISSED_LIMIT:]
        if isinstance(dismissed, list) else [],
        "settings": merged,
    }


def write_state(state: dict, path: str | Path | None = None) -> Path:
    target = Path(path) if path else state_path()
    payload = {
        "dismissed": [str(item) for item in state.get("dismissed", []) if str(item).strip()]
        [-DISMISSED_LIMIT:],
        "settings": {key: bool(state.get("settings", {}).get(key, DEFAULT_SETTINGS[key]))
                     for key in SETTING_KEYS},
    }
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.parent / (target.name + ".tmp")
    temporary.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    os.replace(temporary, target)
    return target


def load_settings(path: str | Path | None = None) -> dict:
    return dict(read_state(path)["settings"])


def set_setting(key: str, value, path: str | Path | None = None) -> dict:
    """Turn one panel setting on or off. Unknown keys are refused, not stored."""
    if key not in SETTING_KEYS:
        raise KeyError(key)
    state = read_state(path)
    state["settings"][key] = bool(value)
    write_state(state, path)
    return dict(state["settings"])


def load_dismissed(path: str | Path | None = None) -> list[str]:
    """Dismissed ids, oldest first. A missing or unreadable file reads as empty:
    a lost dismissal costs a stray highlight, never a failed start."""
    return list(read_state(path)["dismissed"])


def save_dismissed(ids, path: str | Path | None = None) -> Path:
    state = read_state(path)
    state["dismissed"] = [str(item) for item in ids if str(item).strip()]
    return write_state(state, path)


def dismiss(task_id: str, path: str | Path | None = None) -> list[str]:
    """Stop highlighting this task. Idempotent: dismissing twice stays dismissed."""
    task_id = str(task_id or "").strip()
    ids = load_dismissed(path)
    if task_id and task_id not in ids:
        ids.append(task_id)
    save_dismissed(ids, path)
    return ids[-DISMISSED_LIMIT:]


def summary_of(condition: str, *, limit: int = SUMMARY_LIMIT) -> str:
    """First non-empty line of a task, folded to one line and cut to fit."""
    for line in str(condition or "").splitlines():
        text = " ".join(line.split())
        if text:
            return text if len(text) <= limit else text[: limit - 1].rstrip() + "…"
    return ""


def day_label(created_utc: str, *, today: date | None = None) -> str:
    """«сегодня» / «вчера» / «18 сентября» — so an old task cannot read as fresh."""
    try:
        moment = datetime.fromisoformat(str(created_utc)).astimezone()
    except (TypeError, ValueError):
        return ""
    today = today or datetime.now().astimezone().date()
    delta = (today - moment.date()).days
    if delta == 0:
        return "сегодня"
    if delta == 1:
        return "вчера"
    months = ("января", "февраля", "марта", "апреля", "мая", "июня",
              "июля", "августа", "сентября", "октября", "ноября", "декабря")
    return f"{moment.day} {months[moment.month - 1]}"


def goal_view(store) -> GoalView | None:
    current = store.current_intention()
    if current is None or not (current.text or "").strip():
        return None
    return GoalView(id=current.id, text=current.text, criterion=current.criterion or "")


def _preparation_status_by_task(store) -> dict[str, str]:
    """Which task each preparation produced, and how that preparation ended."""
    mapping: dict[str, str] = {}
    try:
        rows = store.preparations()
    except Exception:  # noqa: BLE001 - a missing column must not hide a saved task
        return mapping
    for row in rows:
        task_id = str(row.get("task_id") or "")
        if task_id:
            mapping[task_id] = str(row.get("status") or "")
    return mapping


def latest_practice(store, *, dismissed=(), current_session_id: str = "") -> PracticeView | None:
    """The newest saved, still unattempted task — or None.

    Deliberately picks the newest one and then stops: a task the reader has
    already waved off must not be replaced by the one before it, which would
    make «убрать подсветку» mean «покажи следующую».  A newer task is a
    genuinely new possibility and does light up again.

    A task prepared in the background is offered only while its preparation is
    `ready`. A preparation that was superseded by newer material still left a
    valid task in the ledger, but presenting it as current would attach it to a
    passage the reader has already left. Tasks made by hand have no preparation
    row and are offered as before.
    """
    offered = [
        row for row in store.load_tasks()
        if str(row.get("status") or "") == OFFERED_STATUS
        and str(row.get("condition") or "").strip()
        and str(row.get("id") or "").strip()
    ]
    if not offered:
        return None
    row = offered[-1]  # load_tasks orders by created_epoch, rowid
    task_id = str(row.get("id"))
    if task_id in {str(item) for item in dismissed}:
        return None
    prepared = _preparation_status_by_task(store).get(task_id)
    if prepared is not None and prepared != "ready":
        return None
    return PracticeView(
        task_id=task_id,
        summary=summary_of(str(row.get("condition"))),
        task_type=str(row.get("task_type") or ""),
        created_utc=str(row.get("created_utc") or ""),
        from_current_session=bool(current_session_id)
        and str(row.get("session_id") or "") == current_session_id,
    )


#: Statuses that mean a conclusion about the material was reached, or that work
#: is running. A transport failure or a replaced context is not one of them: the
#: scale must not show a verdict about text nobody managed to read.
SHOWN_PREPARATION_STATUSES = frozenset(
    {"queued", "analyzing", "generating", "validating", "repairing", "ready", "insufficient"}
)


def preparation_view(store, *, offered_task_id: str = "", stage_count: int = 4) -> PreparationView | None:
    """The newest preparation, as the panel's scale.

    Returns None when there is nothing to show: no preparation at all, one that
    reached no conclusion about the material (`failed`, `interrupted`), one that
    was replaced by newer material, one the reader waved off — or a `ready`
    preparation whose task is no longer the offered one. A full scale would claim
    an opportunity that is not there any more.
    """
    try:
        row = store.latest_preparation()
    except Exception:  # noqa: BLE001
        return None
    if not row:
        return None
    status = str(row.get("status") or "")
    if status not in SHOWN_PREPARATION_STATUSES:
        return None
    task_id = str(row.get("task_id") or "")
    if status == "ready" and (not task_id or task_id != offered_task_id):
        return None
    target = ""
    try:
        analysis = json.loads(str(row.get("analysis_json") or "{}"))
        if isinstance(analysis, dict):
            target = str(analysis.get("target") or "")
    except ValueError:
        target = ""
    return PreparationView(
        preparation_id=str(row.get("id") or ""),
        status=status,
        completed_stage=int(row.get("completed_stage") or 0),
        stage_count=stage_count,
        label=str(row.get("stage_label") or ""),
        practice_kind=str(row.get("practice_kind") or ""),
        target=target,
        task_id=task_id,
    )
