"""Cognitive slack: background work feeds only on attention nobody is using.

web2api is one browser session of one account, so requests to it run one at a
time and a request already sent cannot be interrupted (parallel requests set
off the null-answer avalanche, see the web2api RCA). A background request that
starts just before the reader presses a key makes them wait for it. So the
question is not "has it been quiet for N minutes" but "how likely is the reader
to need the model within the next few seconds".

The reader's own logs answer it, and they say time-since-last-key barely
predicts anything while studying (about 5% at any pause under ten minutes),
while three signals do:

    busy     a step is running, a popup is open, a manual request waits,
             or a key / popup was used moments ago      -> background: nothing
    trickle  none of that, but the reader may be back    -> one small piece
    free     quiet longer than the learned "gone" pause  -> background at will

The "gone" pause is relearned from the event log (see `learn_free_after`), so
the threshold follows the reader's rhythm instead of a guess.
"""
from __future__ import annotations

import json
import os
import sqlite3
import time
from pathlib import Path

from . import request_gate

BUSY, TRICKLE, FREE = "busy", "trickle", "free"

ACTIVITY_FILE = "activity"          # touched on every hotkey and popup close
POPUPS_DIR = "popups-open"          # one marker per running popup process
RHYTHM_FILE = "slack.json"          # the learned threshold, refreshed daily

#: Right after an action the next one often follows (a result read, a
#: follow-up asked): no background inside this short window.
JUST_ACTED_SECONDS = 20
#: Until the logs say otherwise: after ten quiet minutes nobody came back.
DEFAULT_FREE_AFTER = 600
#: One background request plus web2api's pacing: the wait a person can hit.
BACKGROUND_SPAN_SECONDS = 8.0
#: "Gone" means a key within the next request is this unlikely, on enough cases.
GONE_HAZARD, MIN_CASES = 0.01, 20
#: Hotkeys whose action calls the model (others cannot collide with background).
MODEL_WINDOWS = {"seed", "seed_batch", "prediction", "clarify", "feynman", "example",
                 "reframe", "task", "summary", "goal", "intent", "step"}


def state_root(root: str | Path | None = None) -> Path:
    return Path(root or os.environ.get("COGNITIVE_STATE_DIR")
                or "~/.local/state/cognitive-popups").expanduser()


# ── signals written by the app ───────────────────────────────────────────────

def touch_activity(root: str | Path | None = None) -> None:
    path = state_root(root) / ACTIVITY_FILE
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()
    except OSError:
        pass


def mark_popup_open(root: str | Path | None = None) -> Path | None:
    directory = state_root(root) / POPUPS_DIR
    marker = directory / str(os.getpid())
    try:
        directory.mkdir(parents=True, exist_ok=True)
        marker.write_text(str(os.getpid()), encoding="utf-8")
    except OSError:
        return None
    return marker


def mark_popup_closed(marker: Path | None, root: str | Path | None = None) -> None:
    if marker is not None:
        try:
            marker.unlink()
        except OSError:
            pass
    touch_activity(root)


def popups_open(root: str | Path | None = None) -> bool:
    """A live popup process; markers left by a crashed one are cleaned up."""
    directory = state_root(root) / POPUPS_DIR
    try:
        entries = list(directory.iterdir())
    except OSError:
        return False
    alive = False
    for entry in entries:
        try:
            pid = int(entry.name)
        except ValueError:
            pid = 0
        if pid and request_gate._alive(pid):
            alive = True
        else:
            try:
                entry.unlink()
            except OSError:
                pass
    return alive


def step_running(root: str | Path | None = None, now: float | None = None) -> bool:
    try:
        focus = json.loads((state_root(root) / "focus.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    # A step past its deadline still waits for the reader's mark: still busy.
    return isinstance(focus, dict) and focus.get("status") == "running"


def quiet_seconds(root: str | Path | None = None, now: float | None = None) -> float:
    try:
        last = (state_root(root) / ACTIVITY_FILE).stat().st_mtime
    except OSError:
        return float("inf")
    return max(0.0, (now if now is not None else time.time()) - last)


# ── the learned rhythm ───────────────────────────────────────────────────────

def hazard_table(events_db: str | Path, candidates=(60, 120, 180, 300, 600, 900, 1200),
                 span: float = BACKGROUND_SPAN_SECONDS) -> list[tuple[int, float, int]]:
    """(quiet seconds, chance of a model hotkey within `span`, cases) from the log."""
    try:
        connection = sqlite3.connect(f"file:{events_db}?mode=ro", uri=True)
        try:
            rows = connection.execute(
                "select ts_epoch, window from events where event = 'hotkey' order by ts_epoch").fetchall()
        finally:
            connection.close()
    except sqlite3.Error:
        return []
    times = [ts for ts, window in rows if (window or "") in MODEL_WINDOWS]
    gaps = [b - a for a, b in zip(times, times[1:]) if b - a < 3 * 3600]
    table = []
    for quiet in candidates:
        alive = [gap for gap in gaps if gap > quiet]
        hits = sum(1 for gap in alive if gap <= quiet + span)
        table.append((quiet, hits / len(alive) if alive else 0.0, len(alive)))
    return table


def learn_free_after(events_db: str | Path) -> int:
    """The shortest pause after which the reader, by their own record, is gone.

    "Gone" must hold from that pause on: a low chance at one checkpoint and a
    high one later only means the reader comes back at a fixed rhythm (always
    after three minutes, say), not that they left. Checkpoints with too few
    cases cannot vouch either way and do not break the run.
    """
    table = [(quiet, hazard) for quiet, hazard, cases in hazard_table(events_db) if cases >= MIN_CASES]
    for index, (quiet, _hazard) in enumerate(table):
        if all(later <= GONE_HAZARD for _q, later in table[index:]):
            return quiet
    return DEFAULT_FREE_AFTER


def free_after(root: str | Path | None = None, now: float | None = None) -> int:
    """The learned threshold, recomputed at most once a day."""
    base = state_root(root)
    path = base / RHYTHM_FILE
    moment = now if now is not None else time.time()
    try:
        saved = json.loads(path.read_text(encoding="utf-8"))
        if moment - float(saved["computed"]) < 24 * 3600:
            return int(saved["free_after"])
    except (OSError, ValueError, KeyError, TypeError):
        pass
    events = Path(os.environ.get("COGNITIVE_EVENT_DB") or base / "events.sqlite3")
    learned = learn_free_after(events) if events.is_file() else DEFAULT_FREE_AFTER
    try:
        path.write_text(json.dumps({"free_after": learned, "computed": moment,
                                    "table": hazard_table(events) if events.is_file() else []}),
                        encoding="utf-8")
    except OSError:
        pass
    return learned


# ── the decision ─────────────────────────────────────────────────────────────

def state(root: str | Path | None = None, now: float | None = None) -> str:
    if step_running(root) or popups_open(root) or request_gate.waiting_manual_requests(root):
        return BUSY
    quiet = quiet_seconds(root, now)
    if quiet < JUST_ACTED_SECONDS:
        return BUSY
    if quiet >= free_after(root, now):
        return FREE
    return TRICKLE
