from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import threading
import time
import traceback
import uuid
from datetime import datetime
from contextvars import ContextVar, copy_context
from functools import wraps
from pathlib import Path
from collections.abc import Sequence

from .client import GeminiWeb2API, Web2APIError
from .event_log import EventChain, EventLog, new_session
from .history import SessionHistory
from .notes import NoteStore
from . import universe as universe_mod
from .models import text_hash
from .records import RecordError, RecordStore, match_fragment
from .service import CLARIFY_WORDS, CognitiveService, looks_like_question, parse_terms, output_artifact, prompt_hash
from . import observation
from . import goals
from . import hud_state
from . import practice
from . import context, focus, slack, sound, steps
from . import prediction_nodes as nodes
from .focus_timer import FocusTimer
from .hud_ipc import HudError, HudServer
from .operation_context import OperationContext, bind_operation, operation_scope

try:
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("Gdk", "3.0")
    from gi.repository import Gdk, GLib, Gtk
except (ImportError, ValueError) as exc:  # pragma: no cover - desktop dependency
    raise SystemExit(f"GTK3/PyGObject is required for the desktop UI: {exc}") from exc


STATE_DIR = Path(os.environ.get("COGNITIVE_STATE_DIR", "~/.local/state/cognitive-popups")).expanduser()
HISTORY_PATH = STATE_DIR / "history.json"
PID_PATH = STATE_DIR / "desktop.pid"
REQUEST_PATH = STATE_DIR / "request"
FLASH_MS = 1500
WATCH_MS = 800
FLASH_MARGIN = 28
POPUP_PYTHON = os.environ.get("COGNITIVE_POPUP_PYTHON", sys.executable)

# The v2 popups live in this package, so clicks land in the event log.  Setting
# COGNITIVE_POPUP_HELPER still pins an external helper, which logs nothing.
PACKAGED_POPUP_HELPER = Path(__file__).resolve().parent / "popup_helper.py"
EXTERNAL_POPUP_HELPER = Path(
    os.environ.get(
        "COGNITIVE_POPUP_HELPER",
        "~/.local/bin/objects-tooltip-popup.py",
    )
).expanduser()
EVENTS = EventLog(os.environ.get("COGNITIVE_EVENT_DB") or STATE_DIR / "events.sqlite3")
# Notes are the reader's own words, so they get their own file: different
# lifetime and different tolerance for failure than telemetry.
NOTES = NoteStore(os.environ.get("COGNITIVE_NOTES_DB") or STATE_DIR / "notes.sqlite3")
# The reading ledger: sessions, fragments, hypotheses. Durable; a failed write
# is a lost day, so its methods raise instead of swallowing (see records.py).
RECORDS = RecordStore(os.environ.get("COGNITIVE_RECORD_DB") or STATE_DIR / "records.sqlite3")

#: The shortcut reference opened by Alt+K, and offered in the panel. Order is the
#: order a reading session tends to meet them in, not alphabetical: orient, ask,
#: then practise. A row with no key of its own names where the action lives instead
#: of inventing a combination, so nothing here promises a key that does not work.
KEYS_REFERENCE = (
    ("Alt+W", "четыре слова из выделенного текста"),
    ("Ctrl+Alt+W", "4-words-batch: три ракурса, mindmap, итоговый тезис"),
    ("Alt+F", "проверка понимания по Фейнману"),
    ("Alt+E", "заметка к выделенному месту"),
    ("Alt+C", "объяснить слово или задать вопрос"),
    ("Ctrl+Q", "сжать выделение до сути"),
    ("Alt+I", "закладка «сейчас хочу»"),
    ("Alt+G", "один конкретный пример"),
    ("Alt+Shift+G", "пример со своим запросом"),
    ("Alt+T", "меню тренировочных задач"),
    ("Alt+Shift+T", "новая задача по старому материалу"),
    ("Alt+Y", "попытка последней задачи"),
    ("Alt+H", "боковая панель (сейчас заморожена)"),
    ("—", "проверка гипотезы: кнопка «…» в панели"),
    ("Alt+K", "эта справка по клавишам"),
)

MODE_MENU = [
    {"label": "Покажи на примере", "action": "example"},
    {"label": "Шаг на 15 минут", "action": "intent"},
    {"label": "4 слова", "action": "four_words"},
    {"label": "Фейнман", "action": "feynman"},
    {"label": "Моя гипотеза", "action": "prediction"},
    {"label": "Спросить / объяснить", "action": "clarify"},
    {"label": "Клавиши", "action": "keys"},
]

#: Where the side panel's actions land (see hud.py). A fixed set of names: the
#: panel sends a name and never a command line, the daemon decides what the name
#: means, and an unknown one is refused rather than executed.
HUD_ACTIONS = {
    "focus": "start_step",
    "goal": "show_intent",
    "four_words": "seed",
    "example": "show_example",
    "note": "capture_error_note",
    "clarify": "explain_terms",
    "feynman": "start_feynman",
    "prediction": "start_prediction",
    "summary": "summarize_text",
    "keys": "show_keys",
    "menu": "show_mode_menu",
}

#: Generating and attempting a task belong to a separate process
#: (`scripts/cognitive-tasks.sh`), because a task owns its own window and ledger
#: writes. The panel asks for one of these two by name as well.
HUD_TASK_ACTIONS = ("task_menu", "task_practice")

#: The checkout this file lives in; `COGNITIVE_PROJECT` pins another one.
PROJECT_ROOT = Path(
    os.environ.get("COGNITIVE_PROJECT") or Path(__file__).resolve().parents[2]
).expanduser()
TASKS_SCRIPT = PROJECT_ROOT / "scripts" / "cognitive-tasks.sh"

#: Short provenance line under an answer, in the reader's words: how what they
#: got stands to the text. Rendered as a note, so it is not copied with the
#: answer itself.
ANSWER_GROUNDING_NOTE = {
    "in_text": "Из текста.",
    "partly_in_text": "Часть — из текста, часть — общий вывод, которого в тексте нет.",
    "not_in_text": "Этого в тексте нет: общий ответ.",
}


def debug(message: str) -> None:
    """Log to stderr (captured by the user journal) when COGNITIVE_DEBUG=1."""
    if os.environ.get("COGNITIVE_DEBUG") == "1":
        print(f"[cognitive-popups] {message}", file=sys.stderr, flush=True)


def popup_helper_command() -> list[str] | None:
    """Command that renders one popup, or None when no helper is available.

    The packaged helper is preferred: it is the only one that reports clicks.
    """
    if os.environ.get("COGNITIVE_POPUP_HELPER"):
        if EXTERNAL_POPUP_HELPER.is_file():
            return [POPUP_PYTHON, str(EXTERNAL_POPUP_HELPER)]
        return None
    if PACKAGED_POPUP_HELPER.is_file():
        return [POPUP_PYTHON, "-m", "cognitive_popups.popup_helper"]
    if EXTERNAL_POPUP_HELPER.is_file():
        return [POPUP_PYTHON, str(EXTERNAL_POPUP_HELPER)]
    return None


CURRENT: ContextVar[EventChain | None] = ContextVar("desktop_chain", default=None)


def current_chain() -> EventChain:
    """The active interaction, starting an empty one when there is none yet."""
    chain = CURRENT.get()
    if chain is None:
        chain = EventChain(EVENTS, new_session(), origin="desktop", pid=os.getpid())
        CURRENT.set(chain)
    return chain


def begin_interaction(event: str, **fields) -> EventChain:
    """Start a fresh interaction: one hotkey press and everything it causes."""
    chain = EventChain(EVENTS, new_session(), origin="desktop", pid=os.getpid())
    CURRENT.set(chain)
    output_artifact.set(None)
    chain.emit(event, **fields)
    return chain


def bind_chain(callback):
    """Capture the chain AND result provenance at scheduling time."""
    chain = current_chain()
    context = copy_context()

    @wraps(callback)
    def bound(*args, **kwargs):
        return context.copy().run(chain.bind(callback), *args, **kwargs)
    return bound


def idle_add(callback, *args):
    return GLib.idle_add(bind_chain(callback), *args)


def observe_artifact(kind, payload):
    with operation_scope(current_chain().context):
        return observation.ObservationStore(enabled=EVENTS.enabled).record_artifact(kind, payload)


def active_step_line(now: float | None = None) -> str:
    """"text · 9:37" while a 15-minute step runs, else "" (read from focus.json)."""
    try:
        state = json.loads((STATE_DIR / "focus.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return ""
    if not isinstance(state, dict) or state.get("status") != "running":
        return ""
    left = float(state.get("deadline", 0)) - (now if now is not None else time.time())
    if left <= 0:
        return ""
    seconds = int(left)
    return f"{state.get('goal', '')} · {seconds // 60}:{seconds % 60:02d}"


def prepare_popup(payload, window):
    """Persist the exact renderer input, not a claim that it was displayed."""
    payload = dict(payload)
    line = active_step_line()
    if line and "step_line" not in payload:
        payload["step_line"] = line
    instance = uuid.uuid4().hex
    with operation_scope(current_chain().context):
        store = observation.ObservationStore(enabled=EVENTS.enabled)
        artifact = store.record_artifact("rendered_payload", payload,
            source_artifact_id=output_artifact.get())
        store.record_presentation(artifact, window_instance_id=instance,
            event="spawn", payload={"window": window})
    payload["observation"] = {"artifact_id": artifact, "window_instance_id": instance}
    emit("window_spawn", window=window, artifact_id=artifact, window_instance_id=instance)
    return payload


def emit(event: str, **fields) -> int | None:
    """Record one event in the current interaction, starting one if needed."""
    return current_chain().emit(event, **fields)


def popup_env() -> dict[str, str]:
    """Environment for a popup: X11 backend, package import path, chain link."""
    env = {**os.environ, "GDK_BACKEND": "x11"}
    root = str(Path(__file__).resolve().parent.parent)
    pythonpath = [entry for entry in env.get("PYTHONPATH", "").split(os.pathsep) if entry]
    if root not in pythonpath:
        pythonpath.insert(0, root)
    env["PYTHONPATH"] = os.pathsep.join(pythonpath)
    # Pin the database explicitly: the popup must write to the same log as the
    # desktop layer even when the state directory is overridden.
    env["COGNITIVE_EVENT_DB"] = str(EVENTS.path)
    if not EVENTS.enabled:
        env["COGNITIVE_EVENT_DISABLE"] = "1"
    env.update(current_chain().env())
    return env


def panel_click(label: str, action) -> None:
    """Record a click on the resident panel, then run the action it triggers."""
    begin_interaction("click", window="panel", item_label=label)
    bind_chain(action)()


def keep_listening(handler):
    """Wrap a hotkey signal handler so one failed action cannot deafen the key.

    GLib drops a signal source whose callback raises, and the signal then reverts
    to its default disposition: SIGWINCH is ignored, so Alt+E, Alt+C, Alt+I and
    every other queued request go silent until the daemon restarts. The failure
    is recorded in the event log instead, traceback included, because the
    daemon's stderr is not always readable afterwards.
    """
    def run() -> bool:
        try:
            handler()
        except Exception as exc:  # noqa: BLE001 - the hotkey must survive its action
            trace = traceback.format_exc()
            debug(f"hotkey action failed: {exc}\n{trace}")
            try:
                emit("error", window="hotkey", detail=f"{type(exc).__name__}: {exc}"[:200],
                     payload_json=json.dumps({"traceback": trace[-4000:]}, ensure_ascii=False))
            except Exception:  # noqa: BLE001 - logging must not re-raise into GLib
                pass
        return True
    return run


def take_request() -> str:
    """Read and clear a queued action name.

    GLib.unix_signal_add accepts exactly six signals (HUP, INT, TERM, USR1,
    USR2, WINCH) and this daemon already uses all of them, so a new entry point
    queues its action here and wakes the loop with SIGWINCH instead of
    pretending there is a seventh signal to claim.
    """
    try:
        value = REQUEST_PATH.read_text(encoding="utf-8").strip()
        REQUEST_PATH.unlink(missing_ok=True)
        return value
    except OSError:
        return ""


def monitor_scale_at(x: float, y: float) -> tuple[float, float, float]:
    """(scale, left, top) of the monitor under a logical point; (1, 0, 0) if unknown."""
    try:
        result = subprocess.run(["hyprctl", "monitors", "-j"], capture_output=True, text=True,
                                timeout=1, check=True)
        monitors = json.loads(result.stdout)
    except (OSError, subprocess.SubprocessError, ValueError):
        return 1.0, 0.0, 0.0
    for monitor in monitors:
        scale = float(monitor.get("scale") or 1.0)
        left, top = float(monitor.get("x", 0)), float(monitor.get("y", 0))
        if left <= x < left + monitor.get("width", 0) / scale and top <= y < top + monitor.get("height", 0) / scale:
            return scale, left, top
    return 1.0, 0.0, 0.0


def to_window_pixels(x: float, y: float) -> tuple[int, int]:
    """Hyprland's logical point as the pixels our XWayland windows are placed in.

    The windows run with zero scaling, so on a scaled monitor (1.5 here) a window
    asked to sit at the cursor's logical coordinates lands that many times too
    close to the corner. Measured 07.10: asked 612,312, Hyprland saw 408,208.
    """
    scale, left, top = monitor_scale_at(x, y)
    return round(left + (x - left) * scale), round(top + (y - top) * scale)


def cursor_position() -> tuple[int, int] | None:
    """The cursor in window pixels. Hyprland is asked, not GTK: a window's own
    idea of the pointer goes stale while the pointer is over a Wayland window."""
    try:
        result = subprocess.run(["hyprctl", "cursorpos"], capture_output=True, text=True,
                                timeout=1, check=True)
        x_text, y_text = result.stdout.strip().replace(" ", "").split(",", 1)
        return to_window_pixels(int(x_text), int(y_text))
    except (OSError, subprocess.SubprocessError, ValueError):
        return None


def run_popup(payload: dict[str, object], *, window: str, detail: str = "") -> str:
    """Open one popup and return its stdout; empty when the reader cancels."""
    command = popup_helper_command()
    if command is None:
        emit("error", window=window, detail="popup helper missing")
        return ""
    position = cursor_position()
    if position:
        payload["x"], payload["y"] = position
    payload = prepare_popup(payload, window)
    try:
        result = subprocess.run(
            command,
            input=json.dumps(payload, ensure_ascii=False),
            capture_output=True,
            text=True,
            env=popup_env(),
            timeout=3600,
        )
        if payload.get("mode") in {"input", "note"}:
            if result.stdout:
                observe_artifact("submitted_input", {"text": result.stdout,
                    "mode": payload.get("mode"), "prompt": payload.get("prompt"),
                    "anchor": payload.get("anchor"), "window": payload["observation"]})
            else:
                with operation_scope(current_chain().context):
                    observation.record_annotation("input_ended", {
                        "outcome": "no_stdout", "returncode": result.returncode,
                        "window": payload["observation"],
                    })
        return result.stdout if payload.get("preserve_raw") else result.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


UNIVERSE_PROMPT = "Твой вопрос или мысль:"
STEP_PROMPT = ("Какой шаг сделаешь за эти минуты? Можно просто Enter: «читаю».\n"
               "Например: «объясню, чем X отличается от Y». Не 15 минут, а 25: «25 разберу ...».")
STEP_GOAL_PROMPT = "Ради чего это было? Одной строкой. Можно пропустить: Enter."
STEP_TAKEAWAY_PROMPT = "Одной строкой: что вынес? Можно пропустить."
SESSION_REMAINING_PROMPT = "Что осталось на следующий раз? Одной строкой, можно пропустить."


def _utc_epoch(ts: str) -> float:
    try:
        return datetime.fromisoformat(ts).timestamp()
    except (TypeError, ValueError):
        return 0.0
UNIVERSE_QUOTE_CHARS = 180


def universe_body(answer) -> str:
    """Question on top, then the bridge and the finding (if any).

    The next-step question is not here: it stays hidden until the reader clicks
    the window (`more` in the text popup); shown up front it only distracted.
    """
    return f"{answer.question}\n\n{answer.bridge}"


def popup_input(prompt: str, title: str, initial: str = "", *, preserve_raw: bool = False,
                quote: str = "", quote_hint: str = "") -> str:
    payload: dict[str, object] = {"mode": "input", "prompt": prompt, "title": title}
    if quote:
        # The reader's own past thought, shown above the field (see universe.py).
        payload["quote"] = quote
        if quote_hint:
            payload["quote_hint"] = quote_hint
    if preserve_raw:
        payload["preserve_raw"] = True
    if initial:
        # A failed check must not cost the reader their wording: the window opens
        # with it already in the field, so resending is one keypress.
        payload["initial"] = initial
    return run_popup(payload, window="input", detail=title)


PRIMARY_PROBES = [
    ("primary-wayland", ["wl-paste", "--primary", "--no-newline"]),
    ("primary-x11", ["xclip", "-selection", "primary", "-o"]),
]
CLIPBOARD_PROBES = [
    ("clipboard-wayland", ["wl-paste", "--no-newline"]),
    ("clipboard-x11", ["xclip", "-selection", "clipboard", "-o"]),
]


def selection_probes(primary_only: bool = False) -> list[tuple[str, list[str]]]:
    """Selection sources in priority order, primary before clipboard.

    Falling back to the clipboard mirrors the established Ctrl+Q popup: some
    applications publish a mouse selection only to the clipboard, and the
    XWayland bridge does not always forward the X11 primary selection.
    """
    if primary_only:
        return list(PRIMARY_PROBES)
    return list(PRIMARY_PROBES) + list(CLIPBOARD_PROBES)


def run_probe(command: list[str]) -> tuple[int | None, str, str]:
    """Return (returncode, stdout, stderr) for one selection probe."""
    try:
        result = subprocess.run(
            command,
            capture_output=True,
            timeout=2,
            check=False,
        )
    except FileNotFoundError as exc:
        return None, "", f"not installed ({exc.filename})"
    except subprocess.TimeoutExpired:
        return None, "", "timed out"
    except OSError as exc:
        return None, "", str(exc)
    stderr = (result.stderr or b"").decode("utf-8", "replace")
    try:
        stdout = (result.stdout or b"").decode("utf-8")
    except UnicodeDecodeError:
        # A copied image (PNG starts with 0x89) is not a passage; treat it as
        # no selection instead of letting the decode error kill the hotkey.
        return None, "", "not text (binary clipboard content)"
    return result.returncode, stdout, stderr


def diagnose_selection(stream=None) -> int:
    """Print what every selection source returns right now."""
    out = stream or sys.stdout
    for name, command in selection_probes():
        code, stdout, stderr = run_probe(command)
        preview = stdout.strip().replace("\n", "\\n")[:120]
        status = "missing" if code is None else f"rc={code}"
        print(f"{name:20s} {status:8s} {preview or ('<- ' + stderr.strip()[:80] if stderr.strip() else '(empty)')}", file=out)
    return 0


def selected_text_source(primary_only: bool = False) -> tuple[str, str]:
    """Capture a passage and name the source that actually produced it.

    The probes fall back from the primary selection to the clipboard, so the
    text returned may be an older copy rather than a live highlight. The origin
    travels with the text so a window can say which one it got instead of
    calling every capture a selection.
    """
    for name, command in selection_probes(primary_only=primary_only):
        code, stdout, stderr = run_probe(command)
        if code == 0:
            text = stdout.strip()
            if text:
                debug(f"selection from {name} ({len(text)} chars): {text[:60]!r}")
                return text, goals.capture_origin(name)
        debug(f"selection probe {name} -> rc={code} stderr={stderr.strip()[:80] or '-'}")
    debug("selection probes returned nothing")
    return "", ""


def selected_text(primary_only: bool = False) -> str:
    text, _ = selected_text_source(primary_only=primary_only)
    return text


class FlashWindow:
    """Compatibility facade for status messages rendered by the shared helper."""

    def __init__(self):
        pass

    def show_cues(self, cues: Sequence[object]) -> None:
        """Show the cue menu through the same renderer as the Ctrl+Q script.

        The popup helper owns the window, placement, input handling, and Copy
        action.  Keeping that responsibility in one script prevents the v2 UI
        from drifting away from the established Ctrl+Q appearance.
        """
        items = []
        for cue in cues:
            if isinstance(cue, dict):
                simple_value = cue.get("simple")
                term_value = cue.get("term")
                simple = "" if simple_value is None else str(simple_value).strip()
                term = "" if term_value is None else str(term_value).strip()
                if simple and term:
                    items.append(dict(cue, simple=simple, term=term))
            elif str(cue).strip():
                value = str(cue).strip()
                items.append({"simple": value, "term": value, "meaning": value})
        if not items:
            return
        command = popup_helper_command()
        if command is None:
            emit("error", window="dual", detail="popup helper missing")
            return
        try:
            # Capture the pointer before spawning the popup.  After Popen the
            # new X11 window may become active and no longer be the right
            # reference for placement.
            payload: dict[str, object] = {"mode": "dual", "items": items}
            position = cursor_position()
            if position:
                payload["x"], payload["y"] = position
            payload = prepare_popup(payload, "dual")
            proc = subprocess.Popen(
                command,
                stdin=subprocess.PIPE,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
                start_new_session=True,
                env=popup_env(),
            )
            if proc.stdin is not None:
                proc.stdin.write(json.dumps(payload, ensure_ascii=False))
                proc.stdin.close()
        except (OSError, subprocess.SubprocessError):
            return

    def show_notice(self, text: str, *, sound_event: str = "") -> None:
        """A status line that fades by itself (never the four-word window)."""
        if sound_event:
            sound.play(sound_event)
        command = popup_helper_command()
        if command is None or not str(text).strip():
            return
        try:
            payload: dict[str, object] = {"mode": "notice", "text": str(text).strip()}
            position = cursor_position()
            if position:
                payload["x"], payload["y"] = position
            payload = prepare_popup(payload, "notice")
            proc = subprocess.Popen(
                command, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, text=True, start_new_session=True, env=popup_env(),
            )
            if proc.stdin is not None:
                proc.stdin.write(json.dumps(payload, ensure_ascii=False))
                proc.stdin.close()
        except (OSError, subprocess.SubprocessError):
            return

    def show_keys(self, rows: Sequence[Sequence[str]]) -> None:
        """Open the shortcut reference in its own window.

        Nothing is reported back — the reader looks and closes it — so the window
        is spawned like the cue flash rather than waited on. The rows are already
        in memory, so this is the one action that costs no model call and no
        ledger write at all, and it opens instantly even mid-request.
        """
        command = popup_helper_command()
        if command is None:
            emit("error", window="keys", detail="popup helper missing")
            return
        try:
            payload: dict[str, object] = {
                "mode": "keys",
                "rows": [list(row) for row in rows],
            }
            position = cursor_position()
            if position:
                payload["x"], payload["y"] = position
            payload = prepare_popup(payload, "keys")
            proc = subprocess.Popen(
                command,
                stdin=subprocess.PIPE,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
                start_new_session=True,
                env=popup_env(),
            )
            if proc.stdin is not None:
                proc.stdin.write(json.dumps(payload, ensure_ascii=False))
                proc.stdin.close()
        except (OSError, subprocess.SubprocessError):
            return

    def show_text(self, text: str, title: str = "Result", *, expanded: bool = False, note: str = "",
                  semantic_role: str | None = None, more: str = "") -> None:
        command = popup_helper_command()
        if not text or command is None:
            return
        try:
            payload: dict[str, object] = {
                "mode": "text",
                "text": text,
                "title": title,
                "expanded": expanded,
            }
            # Only recorded when there is something to show, so the payload of an
            # ordinary result stays exactly what it was before.
            if note:
                payload["note"] = note
            if more:
                payload["more"] = more
            if semantic_role:
                payload["semantic_role"] = semantic_role
            position = cursor_position()
            if position:
                payload["x"], payload["y"] = position
            payload = prepare_popup(payload, "text")
            proc = subprocess.Popen(
                command,
                stdin=subprocess.PIPE,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
                start_new_session=True,
                env=popup_env(),
            )
            if proc.stdin is not None:
                proc.stdin.write(json.dumps(payload, ensure_ascii=False))
                proc.stdin.close()
        except (OSError, subprocess.SubprocessError):
            return



class PanelWindow(Gtk.Window):
    def __init__(self, app: "DesktopApp"):
        super().__init__(title="Cognitive Popups")
        self.app = app
        self._window_instance = uuid.uuid4().hex
        self._render_artifact = None
        self._render_chain = current_chain()
        self._render_rows = []
        self._render_section = "buffer"
        self.connect("map-event", lambda *_: self._observe_panel("window_open"))
        self.connect("unmap-event", lambda *_: self._observe_panel("window_close"))
        self.set_decorated(False)
        self.set_keep_above(True)
        self.set_skip_taskbar_hint(True)
        self.set_skip_pager_hint(True)
        self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
        self.set_default_size(440, 360)
        self.set_border_width(12)
        self.set_position(Gtk.WindowPosition.CENTER)
        self.connect("delete-event", lambda *_: self.hide() or True)
        try:
            self.set_wmclass("cognitive-panel", "cognitive-panel")
        except Exception:
            pass
        self._build()
        self.refresh()

    def _build(self) -> None:
        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.add(root)
        header = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        self.summary = Gtk.Label()
        self.summary.set_xalign(0)
        header.pack_start(self.summary, True, True, 0)
        feynman = Gtk.Button(label="Feynman")
        feynman.connect("clicked", lambda *_: panel_click("Feynman", self.app.start_feynman))
        header.pack_start(feynman, False, False, 0)
        clear = Gtk.Button(label="Clear buffer")
        clear.connect("clicked", lambda *_: panel_click("Clear buffer", self.app.clear_buffer))
        header.pack_start(clear, False, False, 0)
        root.pack_start(header, False, False, 0)

        self.stack = Gtk.Stack()
        self.stack.connect("notify::visible-child", lambda *_: self._observe_panel("layer_open"))
        switcher = Gtk.StackSwitcher(stack=self.stack)
        root.pack_start(switcher, False, False, 0)
        root.pack_start(self.stack, True, True, 0)

        self.buffer_list = Gtk.ListBox()
        self.buffer_list.set_selection_mode(Gtk.SelectionMode.NONE)
        buffer_scroll = Gtk.ScrolledWindow()
        buffer_scroll.add(self.buffer_list)
        self.stack.add_titled(buffer_scroll, "buffer", "Current buffer")

        self.history_list = Gtk.ListBox()
        self.history_list.set_selection_mode(Gtk.SelectionMode.NONE)
        history_scroll = Gtk.ScrolledWindow()
        history_scroll.add(self.history_list)
        self.stack.add_titled(history_scroll, "history", "Buffer history")

        self.cues_list = Gtk.ListBox()
        self.cues_list.set_selection_mode(Gtk.SelectionMode.NONE)
        cues_scroll = Gtk.ScrolledWindow()
        cues_scroll.add(self.cues_list)
        self.stack.add_titled(cues_scroll, "cues", "Cue history")

    def _observe_panel(self, event):
        if self._render_artifact is not None:
            with operation_scope(self._render_chain.context):
                observation.ObservationStore(enabled=EVENTS.enabled).record_presentation(
                    self._render_artifact, window_instance_id=self._window_instance,
                    event=event, payload={"window": "panel", "tab": self.stack.get_visible_child_name()})
        return False

    def _row(self, title: str, body: str) -> Gtk.ListBoxRow:
        self._render_rows.append({"tab": self._render_section, "title": title, "body": body})
        row = Gtk.ListBoxRow()
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        box.set_margin_top(6)
        box.set_margin_bottom(6)
        box.set_margin_start(8)
        box.set_margin_end(8)
        heading = Gtk.Label(label=title)
        heading.set_xalign(0)
        heading.get_style_context().add_class("row-title")
        text = Gtk.Label(label=body)
        text.set_xalign(0)
        text.set_line_wrap(True)
        text.set_selectable(True)
        box.pack_start(heading, False, False, 0)
        box.pack_start(text, False, False, 0)
        row.add(box)
        return row

    def _clear(self, widget: Gtk.ListBox) -> None:
        for child in widget.get_children():
            widget.remove(child)

    def refresh(self) -> None:
        self._render_chain = current_chain()
        self._render_rows = []
        self._render_section = "buffer"
        session = self.app.service.session
        self.summary.set_text(
            f"{len(session.fragments)} fragments · {len(session.fragments) * 4} cues · "
            f"{len(session.feynman_checks)} Feynman checks · "
            f"{len(session.prediction_checks)} hypotheses"
        )
        self._clear(self.buffer_list)
        for index, fragment in enumerate(session.fragments, 1):
            self.buffer_list.add(self._row(
                f"Fragment {index}",
                "   ".join(fragment.cues) + "\n\n" + fragment.source_text,
            ))
        if not session.fragments:
            self.buffer_list.add(self._row("Empty", "Select text and trigger the four-cue action."))

        self._render_section = "cues"
        self._clear(self.cues_list)
        all_fragments = list(session.fragments)
        for archived in self.app.history.load():
            all_fragments.extend(archived.fragments)
        for fragment in reversed(all_fragments):
            self.cues_list.add(self._row(" · ".join(fragment.cues), fragment.created_at))
        if not all_fragments:
            self.cues_list.add(self._row("No cues yet", ""))

        archived_sessions = self.app.history.load()
        self._render_section = "history"
        self._clear(self.history_list)
        for archived in reversed(archived_sessions):
            self.history_list.add(self._row(
                archived.title,
                f"{len(archived.fragments)} fragments · {len(archived.feynman_checks)} Feynman checks · "
                f"{len(archived.prediction_checks)} hypotheses · {archived.created_at}",
            ))
        if not archived_sessions:
            self.history_list.add(self._row("No archived buffers", "Clear the current buffer to archive it here."))
        self._render_artifact = observe_artifact("rendered_payload", {
            "mode": "panel", "summary": self.summary.get_text(), "rows": self._render_rows,
            "buffer_session_id": session.id,
        })
        if self.get_mapped():
            self._observe_panel("render_update")
        self.show_all()


class DesktopApp:
    def __init__(self, client: GeminiWeb2API):
        STATE_DIR.mkdir(parents=True, exist_ok=True)
        PID_PATH.write_text(str(os.getpid()), encoding="utf-8")
        self.service = CognitiveService(client)
        # A session left open by a previous run was interrupted: close it as a
        # crash. Its fragments are already on disk (write-through), so a restart
        # loses nothing, only labels the boundary.
        try:
            RECORDS.close_stale(reason="crash")
        except RecordError as exc:
            debug(f"record reconcile failed: {exc}")
        self.history = SessionHistory(HISTORY_PATH)
        self.flash = FlashWindow()
        self._busy = False
        self._last_selection = ""
        self._cue_cache: dict[tuple, list[dict[str, str]]] = {}
        self._pending_keys: set[tuple] = set()
        #: Examples keep their inputs in the key, so a cached answer can never
        #: reappear under a different material, request or intent.
        self._example_cache: dict[tuple, object] = {}
        self._example_token = 0
        self._last_example: dict[str, str] | None = None
        #: Goal wording is suggested, never assigned: a newer request invalidates
        #: an answer the reader has already moved past.
        self._goal_token = 0
        #: The panel socket, published in run(). None when the panel cannot be
        #: served; the hotkeys never depend on it.
        self.hud: HudServer | None = None
        self.client = client
        self.focus = focus.Focus(STATE_DIR / "focus.json", RECORDS, client)
        self.focus_timer = None
        self._focus_prompting = False
        #: Background preparation of one practice task (see practice.py). Built
        #: here so a restart can close what the previous run left in flight, but
        #: started in run(); it does nothing until the reader turns it on.
        self.preparer = practice.PreparationQueue(
            RECORDS,
            client,
            session_id=self.service.session.id,
            on_stage=lambda status, preparation_id: GLib.idle_add(
                self._preparation_stage, status, preparation_id),
            on_result=lambda outcome, error: GLib.idle_add(
                self._preparation_result, outcome, error),
        )
        try:
            interrupted = RECORDS.interrupt_preparations(reason="restart")
        except RecordError as exc:
            interrupted = 0
            debug(f"preparation reconcile failed: {exc}")
        if interrupted:
            debug(f"preparations interrupted on start: {interrupted}")

    def start_focus(self) -> None:
        if self._focus_prompting:
            return
        state = self.focus.snapshot()
        if state and state["status"] in focus.ACTIVE:
            # An explicit second press cancels the active block, never starts another.
            self.focus.finish(state["id"])
            self._close_focus_timer(state["id"])
            return
        self._focus_prompting = True

        def worker():
            try:
                goal = popup_input("Цель этих 15 минут (отправить = начать):", "15 → 1", preserve_raw=True)
                if goal.strip():
                    state = self.focus.start(goal)
                    GLib.idle_add(self._show_focus_timer, state)
            except Exception as exc:
                GLib.idle_add(self.flash.show_text, str(exc), "15 → 1: ошибка")
            finally:
                GLib.idle_add(self._focus_prompt_done)
        threading.Thread(target=worker, daemon=True).start()

    # ── 15-minute step (Alt+I) ───────────────────────────────────────────────

    def start_step(self) -> None:
        """Alt+I: one concrete step with a short deadline, opened by last time's line.

        A second press while a step runs ends it early through the same end
        window. The session goal is asked once per study session and may be
        skipped; the step itself is the only thing required.
        """
        if self._focus_prompting:
            return
        state = self.focus.snapshot()
        if state and state["status"] in focus.ACTIVE:
            # Before the deadline: end early. After it: the end window was closed
            # without a mark, and Alt+I brings it back.
            self._step_end(state["id"], early=time.time() < state["deadline"])
            return
        self._focus_prompting = True

        def worker():
            try:
                last = RECORDS.last_step()
                new_session = steps.is_new_session(
                    last, RECORDS.closure(last["session_key"]) if last else None)
                session_key = uuid.uuid4().hex if new_session else last["session_key"]
                goal_id = "" if new_session else (last.get("goal_id") or "")
                bridge = steps.bridge(last, RECORDS.last_closure(), new_session=new_session)
                # The goal is not asked here: for an unknown text it comes after the
                # start (owner, 07.10). It is asked once, when the session closes.
                raw = popup_input(STEP_PROMPT, "Шаг", quote=bridge.text if bridge else "",
                                  quote_hint=bridge.hint if bridge else "")
                minutes, text = steps.parse_step(raw)
                text = text or steps.READING
                state = self.focus.start(text, steps.step_seconds(minutes))
                RECORDS.add_step(state["id"], text, session_key=session_key, minutes=minutes,
                                 goal_id=goal_id, started_epoch=state["started_at"],
                                 deadline_epoch=state["deadline"])
                emit("action", window="step",
                     detail=f"step started, {minutes} min, new_session={new_session}")
                GLib.idle_add(self._show_step_timer, state)
            except Exception as exc:  # noqa: BLE001 - shown, not swallowed
                debug(f"step start failed: {exc}\n{traceback.format_exc()}")
                GLib.idle_add(self.flash.show_text, str(exc), "Шаг: ошибка")
            finally:
                GLib.idle_add(self._focus_prompt_done)

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _with_peek(self, handler):
        """Every hotkey is a moment of attention: show the step's clock by the cursor."""
        def run():
            # Every hotkey also tells background work that a person is here.
            slack.touch_activity(STATE_DIR)
            self._peek_timer()
            return handler()
        return run

    def _show_step_timer(self, state):
        self._show_focus_timer(state)
        self._peek_timer()
        return False

    def _peek_timer(self):
        timer = getattr(self, "focus_timer", None)
        if timer is not None:
            try:
                timer.peek()
            except Exception as exc:  # noqa: BLE001 - the clock is a courtesy
                debug(f"timer peek failed: {exc}")
        return False

    def _choose(self, text: str, title: str, options: list[tuple[str, str]], window: str = "step") -> str:
        """A text window with buttons; returns the chosen action or "" when closed."""
        payload = {"mode": "text", "text": text, "title": title,
                   "actions": [{"label": label, "action": action} for label, action in options]}
        response = run_popup(payload, window=window, detail=title)
        try:
            return str(json.loads(response).get("action") or "") if response else ""
        except (ValueError, AttributeError):
            return ""

    def _step_end(self, block_id: str, *, early: bool = False) -> None:
        """The step's end: the reader's mark, one line taken away, then what next.

        This is the one window that may arrive on its own, and only because the
        reader started the timer. Closing it leaves the step unmarked, not failed.
        """
        def worker():
            finished = False
            try:
                state = self.focus.snapshot()
                if not state or state["id"] != block_id or state["status"] not in focus.ACTIVE:
                    return
                minutes = (state["deadline"] - state["started_at"]) / 60
                options = [("Да", "done"), ("Частично", "partly"), ("Нет", "not")]
                if early:
                    options.append(("Продолжить шаг", "continue"))
                if state["goal"] == steps.READING:
                    # Nothing was set to do, so nothing to mark: only what was taken away.
                    choice = "read"
                else:
                    choice = self._choose(steps.end_prompt(state["goal"], minutes, early=early),
                                          "Шаг", options)
                if early and choice in ("", "continue"):
                    return
                if choice not in steps.OUTCOMES and choice != "read":
                    # Closed without a mark: the step stays open and the clock keeps
                    # showing "Время"; Alt+I opens this window again.
                    emit("action", window="step", detail="end window closed without a mark")
                    return
                outcome = choice
                takeaway = popup_input(STEP_TAKEAWAY_PROMPT, "Шаг")
                self.focus.complete(block_id)
                finished = True
                RECORDS.finish_step(block_id, outcome, takeaway)
                GLib.idle_add(self._close_focus_timer, block_id)
                emit("result", window="step",
                     detail=f"step {outcome}, takeaway={len(takeaway.split())} words")
                if outcome == "done":
                    sound.play("step_done")
                step = RECORDS.step(block_id)
                after = self._choose("Дальше?", "Шаг", [("Следующий шаг", "next"),
                                                        ("Перерыв", "break"),
                                                        ("Закончить сессию", "close")])
                emit("action", window="step", detail=f"after step: {after or 'closed'}")
                if after == "next":
                    GLib.idle_add(self._start_step_idle)
                elif after == "close" and step:
                    self._close_study_session(step["session_key"])
            except Exception as exc:  # noqa: BLE001
                debug(f"step end failed: {exc}\n{traceback.format_exc()}")
                GLib.idle_add(self.flash.show_text, f"Не удалось закончить шаг: {exc}", "Шаг")
            finally:
                if finished:
                    GLib.idle_add(self._close_focus_timer, block_id)

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _start_step_idle(self):
        self.start_step()
        return False

    def _close_study_session(self, session_key: str) -> None:
        """Hand the session back: goal, steps, takeaways, own notes; one question."""
        rows = RECORDS.session_steps(session_key)
        if not rows:
            return
        goal_id = next((row["goal_id"] for row in rows if row.get("goal_id")), "")
        intention = RECORDS.intention(goal_id) if goal_id else None
        if intention is None:
            # The goal, in hindsight: what it turned out to be for.
            looked_back = popup_input(STEP_GOAL_PROMPT, "Сессия")
            if looked_back.strip():
                intention = RECORDS.save_intention(looked_back, source="step")
                goal_id = intention.id
        started = rows[0]["started_epoch"]
        notes = [note for note in NOTES.list()
                 if note.created_utc and _utc_epoch(note.created_utc) >= started]
        summary = steps.session_summary(intention.text if intention else "", rows, notes)
        question = "Получил то, ради чего начинал?" if intention else "Сессия дала то, что было нужно?"
        outcome = self._choose(f"{summary}\n\n{question}", "Сессия",
                               [("Да", "done"), ("Частично", "partly"), ("Нет", "not")],
                               window="session")
        remaining = popup_input(SESSION_REMAINING_PROMPT, "Сессия")
        RECORDS.close_study_session(session_key, goal_id=goal_id, outcome=outcome,
                                    remaining=remaining)
        emit("result", window="session",
             detail=f"session closed: {outcome or 'unmarked'}, steps={len(rows)}")
        sound.play("session_close")

    def _focus_prompt_done(self):
        self._focus_prompting = False
        return False

    def _show_focus_timer(self, state):
        current = self.focus.snapshot()
        if current and current["id"] == state["id"] and current["status"] == "running":
            self.focus_timer = FocusTimer(state, self._focus_expired, self.focus.finish)
        return False

    def _close_focus_timer(self, block_id):
        if self.focus_timer and self.focus_timer.state["id"] == block_id:
            self.focus_timer.close()
            self.focus_timer = None
        return False

    def _focus_expired(self, block_id):
        """The timer ran out: the step's own end, not the old generated question."""
        self._step_end(block_id)

    def _focus_expired_legacy(self, block_id):
        """Frozen 2026-10-07: 15→1 ended with a generated one-minute question."""
        def worker():
            try:
                state = self.focus.generate(block_id)
                if not state:
                    return
                if state["status"] == "insufficient_context":
                    GLib.idle_add(self._focus_message, block_id,
                                  "Недостаточно сохранённого исходного текста для вопроса.")
                    return
                current = self.focus.snapshot()
                if current["id"] != block_id or current["status"] != "ready":
                    return
                GLib.idle_add(self._close_focus_timer, block_id)
                # Only the question is visible: hidden targets stay in focus.json.
                explanation = popup_input(state["generation"]["question"], "15 → 1: объясни за минуту", preserve_raw=True)
                if not explanation.strip():
                    self.focus.finish(block_id)
                    return
                result = self.focus.check(block_id, explanation)
                if result:
                    GLib.idle_add(self._focus_result, block_id, result["check"])
            except Exception as exc:
                GLib.idle_add(self._focus_message, block_id, f"Не удалось завершить блок: {exc}")
            finally:
                GLib.idle_add(self._close_focus_timer, block_id)
        threading.Thread(target=worker, daemon=True).start()

    def _focus_message(self, block_id, text):
        state = self.focus.snapshot()
        if state and state["id"] == block_id and state["status"] != "cancelled":
            self.flash.show_text(text, "15 → 1")
        return False

    def _focus_result(self, block_id, result):
        state = self.focus.snapshot()
        if not state or state["id"] != block_id or state["status"] != "completed":
            return False
        role = {"passed": "resolve", "needs_retry": "correction"}.get(result["status"])
        gaps = [g if isinstance(g, str) else g.get("description", g.get("location", "Пробел"))
                for g in result["gaps"]]
        self.flash.show_text("\n".join([result["text"], *gaps]), "15 → 1", semantic_role=role)
        return False

    def show_mode_menu(self) -> None:
        command = popup_helper_command()
        if command is None:
            emit("error", window="menu", detail="popup helper missing")
            self.flash.show_notice("окно не открылось: нет помощника отрисовки")
            return
        position = cursor_position()
        payload: dict[str, object] = {
            "mode": "menu",
            "title": "Cognitive modes",
            "items": MODE_MENU,
        }
        if position:
            payload["x"], payload["y"] = position
        payload = prepare_popup(payload, "menu")

        def worker():
            try:
                result = subprocess.run(
                    command,
                    input=json.dumps(payload, ensure_ascii=False),
                    capture_output=True,
                    text=True,
                    env=popup_env(),
                    timeout=3600,
                )
                if result.stdout.strip():
                    action = json.loads(result.stdout).get("action", "")
                    if action in {item["action"] for item in MODE_MENU}:
                        emit("menu_choose", window="menu", item_label=action)
                        idle_add(self.run_mode, action)
            except (OSError, ValueError, subprocess.SubprocessError) as exc:
                debug(f"mode menu failed: {exc}")
                emit("error", window="menu", detail=str(exc))
                self.flash.show_notice("меню не открылось")

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def run_mode(self, mode: str):
        if mode == "four_words":
            self.seed()
        elif mode == "feynman":
            self.start_feynman()
        elif mode == "prediction":
            self.start_prediction()
        elif mode == "reframe":
            self.start_reframe()
        elif mode == "clarify":
            self.explain_terms()
        elif mode == "keys":
            self.show_keys()
        elif mode == "example":
            self.show_example()
        elif mode == "intent":
            self.start_step()
        return False

    def show_keys(self) -> None:
        """Alt+K: the shortcut reference, from a table already in memory.

        The hotkeys are the primary way in, so a key the reader cannot recall is a
        feature they do not have. This asks the model for nothing and writes
        nothing to the ledger, and it works with an empty buffer.
        """
        self.flash.show_keys(KEYS_REFERENCE)
        emit("action", window="keys", detail=f"{len(KEYS_REFERENCE)} rows")

    def _log_prompt(self, window: str, key: str) -> None:
        """Record which prompt version the next model call uses.

        Prompts are edited while the daemon runs, so without this line two prompt
        versions cannot be told apart when the log is read back.
        """
        identity = self.service.prompt_id(key)
        emit(
            "prompt",
            window=window,
            detail=(
                f"{identity['prompt_key']} {identity['prompt_hash']} "
                f"{identity['prompt_source']}, model={identity['model']}"
            ),
        )

    def seed(self) -> None:
        text = selected_text()
        if not text:
            debug("seed: no selection from any source")
            emit("action", window="seed", detail="no selection")
            self.flash.show_notice("нет выделения")
            return
        debug(f"seed: text ({len(text)} chars) {text[:60]!r}")
        observe_artifact("selected_source", {"action": "seed", "text": text})
        context.remember(text, "seed", root=STATE_DIR)
        cache_key = self.service.cue_cache_key(text)
        cues = self._cue_cache.get(cache_key)
        if cues:
            debug("seed: cache hit")
            fragment = bind_chain(self.service.add_fragment_with_cues)(text, cues)
            output_artifact.set(getattr(cues, "artifact_id", None))
            with operation_scope(current_chain().context):
                observation.record_annotation("cache_hit", {
                    "artifact_id": fragment.artifact_id,
                    "generating_operation_id": fragment.generating_operation_id,
                    "fragment_id": fragment.id, "request_ids": fragment.request_ids,
                })
            self._commit_fragment(fragment)
            emit(
                "action",
                window="seed",
                detail=f"cache hit, {len(text)} chars",
                fragment_id=fragment.id,
                source_hash=fragment.source_hash,
            )
            self.flash.show_cues(cues)

            return
        debug("seed: cache miss, asking the model")
        if self._busy:
            debug("seed: busy, dropping the request")
            emit("action", window="seed", detail="busy, request dropped")
            self.flash.show_notice("ещё думаю над прошлым")
            return
        self._busy = True
        self._log_prompt("seed", "four_words")

        def worker():
            try:
                cues = self.service.extract_cues(text)
                self._cue_cache[getattr(cues, "cache_key", None) or cache_key] = cues
                fragment = self.service.add_fragment_with_cues(text, cues)
                self._commit_fragment(fragment)
                idle_add(self._seed_done, fragment.cue_details, None, fragment.id, fragment.source_hash)
            except Exception as exc:  # noqa: BLE001
                debug(f"seed failed: {exc}\n{traceback.format_exc()}")
                idle_add(self._seed_done, [], str(exc), None, None)

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _seed_done(self, cues: list[dict[str, str]], error: str | None, fragment_id=None, source_hash=None):
        self._busy = False
        if error:
            emit("error", window="seed", detail=str(error))
            self.flash.show_text(f"Ошибка четырёх слов:\n\n{error}", "4 слова")
            return False
        emit("result", window="seed", detail=f"{len(cues)} cues", fragment_id=fragment_id, source_hash=source_hash)
        self.flash.show_cues(cues)
        return False

    def four_words_batch(self) -> None:
        """Run three reader-paced cue passes, then offer one synthesis thesis."""
        text = selected_text()
        if not text:
            self.flash.show_text("Выделите исходный текст для 4-words-batch.", "4 слова — серия")
            return
        if self._busy:
            emit("action", window="seed_batch", detail="busy, request dropped")
            self.flash.show_notice("ещё думаю над прошлым")
            return
        self._busy = True
        source = text.strip()
        perspectives = (
            "основные понятия и их роли",
            "связи, причины и механизмы между понятиями",
            "условия, границы и отличия от похожих идей",
        )
        self._log_prompt("seed_batch", "four_words")

        def worker():
            try:
                all_cues = []
                for index, perspective in enumerate(perspectives, start=1):
                    cues = self.service.extract_cues(source, perspective=perspective)
                    all_cues.extend(cues)
                    run_popup(
                        {"mode": "dual", "items": list(cues),
                         "title": f"4 слова — ракурс {index}/3"},
                        window="dual", detail=f"4-words-batch {index}/3",
                    )
                thesis_prompt = [
                    {"role": "system", "content": (
                        "Сформулируй по исходному тексту и трём наборам опор ровно один "
                        "содержательный тезис, который связывает ключевые идеи и выдерживает "
                        "проверку с разных сторон. Не добавляй фактов, которых нет в источнике. "
                        "Верни только тезис, одним предложением."
                    )},
                    {"role": "user", "content": (
                        f"<SOURCE_TEXT>\n{source}\n</SOURCE_TEXT>\n"
                        f"<CUE_PASSES>\n{json.dumps(all_cues, ensure_ascii=False)}\n</CUE_PASSES>"
                    )},
                ]
                thesis = self.service.client.complete(thesis_prompt, max_tokens=180).strip()
                if not thesis:
                    raise ValueError("пустой итоговый тезис")
                run_popup(
                    {"mode": "text", "title": "Итог — один тезис", "text": thesis},
                    window="text", detail="4-words-batch thesis",
                )
            except Exception as exc:  # noqa: BLE001
                debug(f"4-words-batch failed: {exc}\n{traceback.format_exc()}")
                run_popup(
                    {"mode": "text", "title": "4 слова — серия", "text": f"Ошибка серии:\n\n{exc}"},
                    window="text",
                )
            finally:
                idle_add(self._batch_done)

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _batch_done(self):
        self._busy = False
        return False

    def watch_selection(self):
        text = selected_text(primary_only=True)
        cache_key = self.service.cue_cache_key(text) if text else None
        if text and cache_key != self._last_selection:
            self._last_selection = cache_key
            debug(f"watch: new primary selection ({len(text)} chars) {text[:60]!r}")
            if cache_key not in self._cue_cache and cache_key not in self._pending_keys and not self._busy:
                self._pending_keys.add(cache_key)

                def worker():
                    try:
                        cues = self.service.extract_cues(text)
                        self._cue_cache[getattr(cues, "cache_key", None) or cache_key] = cues
                    except Exception as exc:
                        debug(f"watch precompute failed: {exc}")
                    finally:
                        self._pending_keys.discard(cache_key)

                background = OperationContext(kind="background", buffer_session_id=self.service.session.id)
                threading.Thread(target=bind_operation(worker, background), daemon=True).start()
        return True

    def start_prediction(self) -> None:
        """The reader's own claim, checked against what this moment needs.

        The ground is the selection plus the few passages taken just before
        (context.py), not the whole buffer: a selection alone is enough, so the
        hypothesis no longer waits for Alt+W.
        """
        if self._busy:
            emit("action", window="prediction", detail="busy, request dropped")
            self.flash.show_notice("ещё думаю над прошлым")
            return
        selection = selected_text()
        fragments = self.service.session.fragments
        point = selection.strip() or (fragments[-1].source_text if fragments else "")
        if not point:
            emit("action", window="prediction", detail="buffer is empty")
            self.flash.show_notice("нет текста: выдели абзац")
            return
        self._prediction_pack = context.pack(point, root=STATE_DIR)
        context.remember(selection, "prediction", root=STATE_DIR)
        emit("action", window="prediction",
             detail=f"context: selection={len(point)} chars, recent={len(self._prediction_pack.recent)}")
        hypothesis = popup_input(
            "Сформулируй одну конкретную гипотезу о связи между объектами.",
            "Моя гипотеза",
        )
        if not hypothesis:
            return
        self.run_prediction(hypothesis)

    def run_prediction(self, hypothesis: str) -> None:
        """Send one already-written hypothesis, without asking for it again.

        Split out so a check that failed for a technical reason can be repeated on
        the same wording: the reader asked to always get an answer to their
        hypothesis, and retyping is not the work that was interrupted.
        """
        self._busy = True
        self._log_prompt("prediction", "prediction")

        material = getattr(self, "_prediction_pack", None)

        nodes_mode = nodes.enabled(STATE_DIR)

        def worker():
            try:
                if nodes_mode:
                    store = self._universe()
                    mine = []
                    if store is not None and material is not None:
                        try:
                            mine = [h.text for h in store.instant(f"{hypothesis}\n{material.selection}", k=3)]
                        except Exception as exc:  # noqa: BLE001 - no thought is not a failure
                            debug(f"universe recall failed: {exc}")
                    check = self.service.check_prediction_nodes(
                        hypothesis, material, history=nodes.load_history(STATE_DIR), mine=mine)
                    if check.nodes:
                        nodes.remember(STATE_DIR, check.nodes)
                else:
                    check = self.service.check_prediction(hypothesis, material)
                self._commit_prediction(check)
                idle_add(self._prediction_done, check, None, hypothesis)
            except Exception as exc:  # noqa: BLE001
                idle_add(self._prediction_done, None, str(exc), hypothesis)

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _prediction_done(self, check, error: str | None, hypothesis: str = ""):
        self._busy = False
        if error:
            emit("error", window="prediction", detail=str(error))
            self.flash.show_text(
                f"Гипотеза: {hypothesis}\n\nПроверить не удалось: {error}\n\n"
                "Формулировка сохранена: окно откроется с ней же, достаточно отправить ещё раз.",
                "Моя гипотеза",
            )
            self._retry_prediction(hypothesis)
            return False

        emit("result", window="prediction", detail=check.status)
        if getattr(check, "mark", "") or getattr(check, "nodes", None) or check.display == "нечего добавить":
            self._show_nodes(check)
            return False
        # Keep the result open with an optional, explicit continuation. A click on
        # the action returns a choice; closing the popup is just closing it.
        ids = set(check.buffer_fragment_ids)
        source_snapshot = "\n\n".join(
            f.source_text for f in self.service.session.fragments if f.id in ids
        )
        payload = {
            "mode": "text", "text": check.text, "title": "Моя гипотеза",
            "one_delta": check.one_delta, "evidence": check.evidence,
        }
        if check.display:
            # The 2026-10-07 answer already shows where to look; the question
            # for the reader's own move waits behind a click, as in Alt+C.
            payload["evidence"] = ""
            if check.ask:
                payload["more"] = "→ " + check.ask

        if check.status in {"contradicted", "partially_confirmed"} and (
            (check.one_delta or "").strip() or (check.mismatch or "").strip()
        ):
            payload["semantic_role"] = "correction"

        def await_choice():
            response = run_popup(payload, window="prediction_result", detail=check.status)
            try:
                action = json.loads(response).get("action") if response else ""
            except (ValueError, AttributeError):
                action = ""
            if action == "reframe":
                idle_add(self._reframe_from_result, check, source_snapshot)

        threading.Thread(target=bind_chain(await_choice), daemon=True).start()
        return False

    def _show_nodes(self, check) -> None:
        """The mark, then four nodes around the cursor; or the mark alone, or silence."""
        def worker():
            if check.mark:
                self.flash.show_notice(nodes.MARK_SYMBOL.get(check.mark, ""))
            if check.nodes:
                time.sleep(1.4 if check.mark else 0)
                self.flash.show_cues([{"simple": w, "term": w, "meaning": ""} for w in check.nodes])
            elif not check.mark:
                self.flash.show_notice("нечего добавить")
        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _reframe_from_result(self, check, source_snapshot: str):
        begin_interaction("popup", window="reframe", detail="после гипотезы")
        self._split_idle_session()
        self.start_reframe(from_check=check, material_snapshot=source_snapshot)
        return False

    def _retry_prediction(self, hypothesis: str) -> None:
        """Offer one repeat of the same hypothesis after a failed check.

        This is not the system proposing a new activity: it is the reader's own
        request, kept alive. Escape, or an empty field, leaves everything as it is.
        """
        if not hypothesis:
            return
        again = popup_input(
            "Проверка не прошла. Отправь ту же формулировку ещё раз или уточни её.",
            "Моя гипотеза",
            initial=hypothesis,
        )
        if again.strip():
            self.run_prediction(again)

    def _current_goal_text(self) -> str:
        """The accepted goal in the reader's words, or an empty string."""
        try:
            current = RECORDS.current_intention()
        except RecordError as exc:
            debug(f"intent read failed: {exc}")
            return ""
        return current.text if current else ""

    def start_reframe(self, *, from_check=None, material_snapshot: str = "", ask: bool = False) -> None:
        """One alternative view, generated as soon as the key is pressed.

        The material and the focus come from what the session already holds — the
        exact fragments of the last hypothesis, its wording, or the accepted goal
        — so pressing the key is the whole request and no window stands between
        the reader and the answer. When there is nothing to derive a focus from,
        the model picks the organising principle itself.

        `ask` (Alt+Shift+R) is the one exception: it opens the field for a reader
        who wants to steer the frame in their own words. A live primary selection
        wins for the material; without one, a prior hypothesis keeps the exact
        fragments it was checked against. Clipboard text is not treated as a live
        selection here, since it may be an unrelated old copy.
        """
        if self._busy:
            emit("action", window="reframe", detail="busy, request dropped")
            self.flash.show_notice("ещё думаю над прошлым")
            return
        previous = from_check
        if from_check is not None:
            material = material_snapshot
            label = "материалу выбранной гипотезы"
        else:
            material, _origin = selected_text_source(primary_only=True)
            if material:
                label = "выделенному тексту"
            elif self.service.session.prediction_checks:
                previous = self.service.session.prediction_checks[-1]
                ids = set(previous.buffer_fragment_ids)
                material = "\n\n".join(
                    f.source_text for f in self.service.session.fragments if f.id in ids
                )
                label = "материалу последней гипотезы"
            else:
                material = "\n\n".join(f.source_text for f in self.service.session.fragments)
                label = "сохранённому буферу"
        if not material.strip():
            self.flash.show_text(
                "Нет материала: выделите текст или сохраните фрагмент через Alt+W.",
                "Другой ракурс",
            )
            return
        # What the reader already holds: their last claim, else their goal.
        focus = previous.hypothesis if previous else self._current_goal_text()
        if ask:
            asked = popup_input(
                f"Другой ракурс по {label}. Какой вопрос или связь рассмотреть иначе?",
                "Другой ракурс",
                initial=focus,
            )
            if not asked.strip():
                return
            focus = asked
        current_frame = previous.hypothesis if previous else ""
        self._busy = True
        self._log_prompt("reframe", "reframe")

        def worker():
            try:
                result = self.service.reframe(material, focus, current_frame)
                idle_add(self._reframe_done, result, None, label)
            except Exception as exc:  # noqa: BLE001
                idle_add(self._reframe_done, None, str(exc), label)

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _reframe_done(self, result, error: str | None, label: str):
        self._busy = False
        if error:
            emit("error", window="reframe", detail=error)
            self.flash.show_text(f"Не удалось предложить другой ракурс:\n\n{error}", "Другой ракурс")
        else:
            emit("result", window="reframe", detail=result.status)
            self.flash.show_text(result.text, "Другой ракурс", note=f"По {label}.")
        return False

    def start_feynman(self) -> None:
        if not self.service.session.fragments:
            emit("action", window="feynman", detail="buffer is empty")
            self.flash.show_notice("буфер пуст: сначала Alt+W или выдели текст")
            return
        if self._busy:
            emit("action", window="feynman", detail="busy, request dropped")
            self.flash.show_notice("ещё думаю над прошлым")
            return
        self._busy = True
        self._log_prompt("feynman", "feynman_question")

        def worker():
            try:
                question = self.service.create_feynman_question()
                idle_add(self._show_feynman, question, None)
            except Exception as exc:  # noqa: BLE001
                debug(f"feynman question failed: {exc}\n{traceback.format_exc()}")
                idle_add(self._show_feynman, "", str(exc))

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _show_feynman(self, question: str, error: str | None):
        self._busy = False
        if error:
            emit("error", window="feynman", detail=str(error))
            self.flash.show_text(f"Ошибка вопроса Фейнмана:\n\n{error}", "Фейнман")
            return False
        explanation = popup_input(question, "Feynman check")
        if not explanation:
            return False
        self._busy = True
        self._log_prompt("feynman", "feynman_check")

        def worker():
            try:
                check = self.service.check_feynman(question, explanation)
                self._commit_feynman(check)
                idle_add(self._check_done, check.status, check.gaps, check.follow_up)
            except Exception as exc:
                debug(f"feynman check failed: {exc}\n{traceback.format_exc()}")
                idle_add(self._check_done, "error", [], None, str(exc))

        threading.Thread(target=bind_chain(worker), daemon=True).start()
        return False

    def _check_done(
        self,
        status: str,
        gaps: list[dict[str, str]],
        follow_up: str | None,
        error: str | None = None,
    ):
        self._busy = False
        if error:
            emit("error", window="feynman", detail=str(error))
            self.flash.show_text(f"Ошибка проверки Фейнмана:\n\n{error}", "Фейнман")
            return False
        emit("result", window="feynman", detail=f"{status}, {len(gaps)} gaps")
        if status == "passed":
            self.flash.show_text("Достаточно.", title="Фейнман", semantic_role="resolve")
        elif status == "needs_retry":
            gap_text = "\n".join(
                f"• {gap.get('description') or gap.get('location') or 'Пробел'}"
                for gap in gaps[:2]
            )
            if follow_up:
                gap_text += f"\n\n{follow_up}"
            self.flash.show_text(gap_text or "Найден пробел в объяснении.", "Фейнман: пробел", semantic_role="correction")
        else:
            self.flash.show_notice("Фейнман: не удалось")
        return False

    def clear_buffer(self) -> None:
        if not self.service.session.fragments:
            return
        archived = self.service.clear_buffer()
        self.history.append(archived)
        self._close_session(archived.id, "clear")
        emit("action", window="panel", detail=f"buffer cleared, {len(archived.fragments)} fragments archived")
        self.flash.show_notice("буфер очищен")

    def capture_error_note(self) -> None:
        """Record the reader's own account of a misreading (see notes.py)."""
        source_text = selected_text()
        context.remember(source_text, "note", root=STATE_DIR)
        anchor = source_text.strip()
        session_id = self.service.session.id
        # Freeze provenance before the popup; a cue match is not a source match.
        key = " ".join(anchor.split()).casefold()
        matches = [fragment for fragment in self.service.session.fragments
                   if key and key in " ".join(fragment.source_text.split()).casefold()]
        fragment_id = matches[0].id if len(matches) == 1 else None
        source_hash = text_hash(source_text) if source_text else None
        observe_artifact("selected_source", {"action": "note", "text": source_text,
            "source_hash": source_hash, "fragment_id": fragment_id,
            "buffer_session_id": session_id})
        comment = run_popup(
            {"mode": "note", "anchor": anchor},
            window="note",
            detail=anchor[:60] or "no selection",
        )
        if not comment:
            emit("action", window="note", detail="cancelled")
            return
        artifact = observe_artifact("note_content", {"comment": comment, "anchor": anchor,
            "source_text": source_text or None, "source_hash": source_hash,
            "fragment_id": fragment_id, "buffer_session_id": session_id})
        try:
            note = NOTES.add(
                comment,
                anchor=anchor or None,
                fragment_id=fragment_id,
                source_hash=source_hash,
                source_text=source_text or None,
                session_id=session_id,
            )
        except Exception as exc:  # noqa: BLE001 - a lost note must be visible
            debug(f"error note failed: {exc}")
            emit("error", window="note", detail=str(exc))
            self.flash.show_text(f"Заметка не записана:\n\n{exc}", "Ошибка чтения")
            return
        with operation_scope(current_chain().context):
            observation.record_annotation("ledger_link", {"kind": "note", "note_id": note.id,
                "artifact_id": artifact, "fragment_id": note.fragment_id,
                "source_hash": note.source_hash, "buffer_session_id": session_id})
        emit(
            "result",
            window="note",
            detail=f"note {note.id} saved, {len(comment)} chars",
            fragment_id=note.fragment_id,
            source_hash=note.source_hash,
        )
        self.flash.show_notice("заметка сохранена", sound_event="note_saved")

    def explain_terms(self) -> None:
        """Alt+C: explain the words the reader did not understand, or answer a question.

        One hotkey covers both: a list of terms is a glossary request, anything
        that reads as a question goes to the question mode. Which is which is
        decided by the reader's own words (see `looks_like_question`), never by a
        revealed cue layer: which word was missed, or what they want to ask, is a
        fact only they hold. The answer is input for reading on, never evidence —
        it reaches the event log and nowhere else.

        The ground is the buffer plus the current selection. This is the one mode
        whose answer is not evidence, so it is the one mode allowed to lean on
        what is highlighted right now: the sentence that defines the term, or
        answers the question, may be on screen and not yet committed. Refusing
        until four cues have been seeded would demand a model call to ask about
        the page already in front of the reader. The checks stay bound to the
        buffer — they weigh an explanation against material the reader committed to.
        """
        source = selected_text()
        observe_artifact("clarify_source", {"selection": source,
            "buffer": self.service.session.to_dict()})
        if not source and not self.service.session.fragments:
            emit("action", window="clarify", detail="no buffer and no selection")
            self.flash.show_text(
                "Ни выделения, ни буфера. Выделите текст или нажмите Alt+W.",
                "Спросить / объяснить",
            )
            return
        if self._busy:
            emit("action", window="clarify", detail="busy, request dropped")
            self.flash.show_notice("ещё думаю над прошлым")
            return
        if universe_mod.clarify_enabled(STATE_DIR):
            self._ask_from_universe(source)
            return
        raw = popup_input(
            "Что не понял или что спросить? Слова — через запятую.",
            "Спросить / объяснить",
        )
        if not raw:
            emit("action", window="clarify", detail="cancelled")
            return
        if looks_like_question(raw):
            self._ask_question(" ".join(raw.split()).strip(), source)
            return
        terms = parse_terms(raw)
        if not terms:
            emit("action", window="clarify", detail="no terms")
            return
        emit("action", window="clarify",
             detail=f"{len(terms)} terms, selection={len(source)} chars, "
                    f"buffer={len(self.service.session.fragments)} fragments")
        self._busy = True
        self._log_prompt("clarify", "clarify")

        def worker():
            try:
                result = self.service.clarify_terms(terms, source)
                idle_add(self._clarify_done, result, None)
            except Exception as exc:  # noqa: BLE001
                debug(f"clarify failed: {exc}\n{traceback.format_exc()}")
                idle_add(self._clarify_done, None, str(exc))

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _universe(self):
        """The reader's universe store, or None when it cannot be opened."""
        if getattr(self, "_universe_store", None) is None:
            try:
                self._universe_store = universe_mod.Universe(root=STATE_DIR)
            except Exception as exc:  # noqa: BLE001 - Alt+C must work without it
                debug(f"universe unavailable: {exc}")
                return None
        return self._universe_store

    def _ask_from_universe(self, source: str) -> None:
        """Alt+C: the reader's own question, answered straight, from their own world.

        The field opens empty: the question is the reader's. Only after it is
        asked is the universe searched (by the question and the passage, tables
        only, no model call), and the found thought is shown with the answer only
        if the answer really leaned on it.
        """
        fragments = self.service.session.fragments
        passage = source.strip() or (fragments[-1].source_text if fragments else "")
        raw = popup_input(UNIVERSE_PROMPT, "Спросить / объяснить")
        question = " ".join((raw or "").split()).strip()
        if not question:
            emit("action", window="clarify", detail="cancelled")
            return
        store = self._universe()
        hook = None
        if store is not None:
            try:
                hits = store.instant(f"{question}\n{passage}")
                hook = hits[0] if hits else None
            except Exception as exc:  # noqa: BLE001 - a missing hook is not a failure
                debug(f"universe recall failed: {exc}")
        emit("action", window="clarify",
             detail=(f"universe hook {hook.thought_id} score={hook.score:.2f} via={','.join(hook.via)[:80]}"
                     if hook else "universe: nothing woke"))
        mine = [{"id": hook.thought_id, "thought": hook.text, "place_in_text": hook.anchor[:300]}] if hook else []
        surroundings = context.pack(passage, root=STATE_DIR)
        context.remember(source, "clarify", root=STATE_DIR)
        emit("action", window="clarify",
             detail=f"universe question, passage={len(passage)} chars, hook={'yes' if hook else 'no'}, "
                    f"recent={len(surroundings.recent)}")
        self._busy = True
        self._log_prompt("clarify", "clarify_universe")
        hook_id = hook.thought_id if hook else ""
        hook_text = hook.text if hook else ""

        def worker():
            try:
                answer = self.service.answer_from_universe(
                    question, passage, mine,
                    recent=surroundings.recent_block(), step=surroundings.step)
                idle_add(self._universe_done, answer, None, hook_id, hook_text)
            except Exception as exc:  # noqa: BLE001
                debug(f"universe answer failed: {exc}\n{traceback.format_exc()}")
                idle_add(self._universe_done, None, str(exc), hook_id, hook_text)

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _universe_done(self, answer, error: str | None, hook_id: str = "", hook_text: str = ""):
        self._busy = False
        if error:
            emit("error", window="clarify", detail=str(error))
            self.flash.show_text(f"Ошибка ответа:\n\n{error}", "Спросить / объяснить")
            return False
        observe_artifact("clarify_universe_answer", {
            "question": answer.question, "bridge": answer.bridge, "gap": answer.gap,
            "nudge": answer.nudge, "used": answer.used, "shown": hook_id})
        emit("result", window="clarify",
             detail=(f"universe answer, {len(answer.bridge.split())} words, "
                     f"used={answer.used or '-'}, shown={hook_id or '-'}, gap={'yes' if answer.gap else 'no'}"))
        body = universe_body(answer)
        if answer.used and hook_text:
            # The thought is shown only because the answer leaned on it.
            body = f"«{hook_text[:UNIVERSE_QUOTE_CHARS]}»\n\n{body}"
        self.flash.show_text(body, "Спросить / объяснить")
        return False

    def _ask_question(self, question: str, source: str) -> None:
        """Send one free-form question to the question mode of Alt+C.

        Kept apart from a glossary request because the answer is prose whose
        standing to the text has to be shown, not a list of explained terms.
        """
        if not question:
            emit("action", window="clarify", detail="empty question")
            return
        emit("action", window="clarify",
             detail=f"question, selection={len(source)} chars, "
                    f"buffer={len(self.service.session.fragments)} fragments")
        self._busy = True
        self._log_prompt("clarify", "clarify_question")

        def worker():
            try:
                answer = self.service.answer_question(question, source)
                idle_add(self._answer_done, answer, None)
            except Exception as exc:  # noqa: BLE001
                debug(f"question failed: {exc}\n{traceback.format_exc()}")
                idle_add(self._answer_done, None, str(exc))

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _answer_done(self, answer, error: str | None):
        self._busy = False
        if error:
            emit("error", window="clarify", detail=str(error))
            self.flash.show_text(f"Ошибка ответа:\n\n{error}", "Спросить / объяснить")
            return False
        note = ANSWER_GROUNDING_NOTE.get(answer.grounding, "")
        emit(
            "result",
            window="clarify",
            detail=(
                f"answer, {len(answer.text.split())} words, "
                f"grounding={answer.grounding or 'unknown'}"
            ),
        )
        body = answer.text
        if answer.basis:
            body = f"{body}\n\nОпора: «{answer.basis}»"
        self.flash.show_text(body, "Спросить / объяснить", note=note)
        return False

    def _clarify_done(self, result, error: str | None):
        self._busy = False
        if error:
            emit("error", window="clarify", detail=str(error))
            self.flash.show_text(f"Ошибка объяснения:\n\n{error}", "Спросить / объяснить")
            return False
        lines = [f"{line['term']} — {line['explanation']}" for line in result.lines]
        if not lines:
            lines = ["Опора для объяснения не найдена в тексте."]
        if result.omitted:
            lines.append(f"Упущено — {', '.join(result.omitted)}")
        if result.truncated:
            lines.append(f"Сокращено до {CLARIFY_WORDS} слов — {', '.join(result.truncated)}")
        emit(
            "result",
            window="clarify",
            detail=(
                f"{len(result.lines)} explained, {len(result.omitted)} omitted, "
                f"{len(result.truncated)} truncated"
            ),
        )
        self.flash.show_text("\n".join(lines), "Спросить / объяснить")
        return False

    def summarize_text(self) -> None:
        """Ctrl+Q: compress the current selection to its gist.

        The passage and the answer are logged together: the input reaches
        `summary_source` and the operation artifacts, so a summary can be
        audited later. The answer is a reading aid, not evidence — it never
        reaches the records ledger or a note's resolution.
        """
        text = selected_text()
        if not text:
            emit("action", window="summary", detail="no selection")
            self.flash.show_text("Нет выделения для сжатия.", "Сжать текст")
            return
        if self._busy:
            emit("action", window="summary", detail="busy, request dropped")
            self.flash.show_notice("ещё думаю над прошлым")
            return
        observe_artifact("summary_source", {"selection": text})
        emit("action", window="summary",
             detail=f"сжат текст (Ctrl+Q): {text[:60]}")
        self._busy = True
        self._log_prompt("summary", "summary")

        def worker():
            try:
                result = self.service.summarize(text)
                idle_add(self._summary_done, result, None)
            except Exception as exc:  # noqa: BLE001
                debug(f"summary failed: {exc}\n{traceback.format_exc()}")
                idle_add(self._summary_done, None, str(exc))

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _summary_done(self, result, error: str | None):
        self._busy = False
        if error:
            emit("error", window="summary", detail=str(error))
            self.flash.show_text(f"Ошибка сжатия:\n\n{error}", "Сжать текст")
            return False
        emit("result", window="summary", detail=f"сжат текст, {len(result.text)} chars")
        self.flash.show_text(result.text, "Сжать текст")
        return False

    def show_intent(self, *, new_request: bool = True, offer_assist: bool = True) -> None:
        """Open the reader's bookmark, or start one when there is none yet.

        With no accepted goal this opens the direction list directly, so the very
        first use never begins with an empty editor to fill in. With a goal it
        shows what is already accepted and returns to reading by default. Storage
        is local and durable, so neither path waits on the model or a session.
        """
        if new_request:
            self._goal_token += 1
        try:
            current = RECORDS.current_intention()
            history = RECORDS.intention_history(tail=8)
        except RecordError as exc:
            debug(f"intent read failed: {exc}")
            emit("error", window="intent", detail=f"ledger: {exc}")
            self.flash.show_text(f"Не удалось открыть закладку:\n\n{exc}", "Сейчас хочу")
            return
        if offer_assist and not (current and current.text):
            # No accepted goal yet: the decision comes first, never the editor.
            material, origin = self._goal_material()
            self._goal_assist(material, origin)
            return
        self._goal_card(current, history)

    def _goal_card(self, current, history) -> None:
        """Show the accepted goal; «Продолжить чтение» is the default action."""
        error = ""
        retry: dict | None = None
        while True:
            if retry is not None:
                shown: dict = retry
            else:
                shown = {
                    "id": current.id if current else None,
                    "text": current.text if current else "",
                    "criterion": current.criterion if current else "",
                    "stopped_at": current.stopped_at if current else "",
                    "material": current.material if current else "",
                    "material_origin": current.material_origin if current else "",
                }
            payload: dict[str, object] = {
                "mode": "intent",
                "intent": shown,
                "history": [{"id": item.id, "text": item.text} for item in history],
                "error": error,
            }
            raw = run_popup(payload, window="intent", detail="намерение")
            if not raw:
                emit("action", window="intent", detail="closed without saving")
                return
            try:
                data = json.loads(raw)
            except ValueError:
                emit("error", window="intent", detail="bad popup payload")
                return
            if not isinstance(data, dict):
                return
            action = str(data.get("action", ""))
            if action == "assist":
                material, origin = self._goal_material()
                self._goal_assist(material, origin)
                return
            if action in ("", "keep"):
                # «Продолжить чтение»: closing without a write is normal.
                emit("action", window="intent", detail="closed without saving")
                return
            try:
                if action == "restore":
                    restored = RECORDS.promote_intention(str(data.get("id", "")))
                    emit("result", window="intent", detail=f"restored {restored.id[:8]}")
                    return
                text = str(data.get("text", "")).strip()
                if not text:
                    return
                if current is None:
                    saved = RECORDS.save_intention(
                        text,
                        criterion=str(data.get("criterion", "")),
                        stopped_at=str(data.get("stopped_at", "")),
                    )
                    emit("result", window="intent", detail=f"new intention {saved.id[:8]}")
                    return
                # Editing the accepted goal keeps its id, history and material.
                saved = RECORDS.update_intention(
                    current.id, text,
                    criterion=str(data.get("criterion", "")),
                    stopped_at=str(data.get("stopped_at", "")),
                )
                emit("result", window="intent", detail=f"saved intention {saved.id[:8]}")
            except (RecordError, ValueError) as exc:
                # Reopen with the typed phrase still in it: a failed write must
                # not cost the reader the words they just wrote.
                error = str(exc)
                retry = {
                    "id": current.id if current else None,
                    "text": str(data.get("text", "")),
                    "criterion": str(data.get("criterion", "")),
                    "stopped_at": str(data.get("stopped_at", "")),
                    "material": current.material if current else "",
                    "material_origin": current.material_origin if current else "",
                }
                continue
            return

    def _goal_material(self) -> tuple[str, str]:
        """Passage the reader just captured, and where it came from."""
        return goals.choose_material(*selected_text_source())

    def _last_fragment(self) -> str:
        """Source text of the most recent «4 слова» fragment, or an empty string."""
        try:
            return RECORDS.latest_fragment_source() or ""
        except RecordError as exc:
            debug(f"latest fragment read failed: {exc}")
            return ""

    def _goal_assist(self, material: str, origin: str) -> None:
        """Pick a direction, then word the goal from the passage in one call."""
        opened = self._goal_input(material, origin)
        if opened is None:
            emit("action", window="goal", detail="cancelled")
            return
        chosen = str(opened.get("material", "")).strip()
        direction = str(opened.get("direction", "")).strip()
        note = str(opened.get("note", "")).strip()
        if not chosen:
            # Nothing to ground a wording in: the reader's own phrase (or the
            # general wording for the direction) is kept and no model is called.
            phrase = goals.direction_draft(direction, note)
            if not phrase:
                emit("action", window="goal", detail="nothing to word a goal from")
                return
            self._goal_result(
                phrase, direction, notice="Без материала: формулировка не уточнена",
            )
            return
        self._run_goal(chosen, direction, note, origin)

    def _goal_input(self, material: str, origin: str) -> dict | None:
        last_fragment = self._last_fragment()
        if last_fragment.strip() == material.strip():
            # Already in front of the reader; offering it again adds nothing.
            last_fragment = ""
        payload: dict[str, object] = {
            "mode": "goal",
            "material": material,
            "origin": origin,
            "last_fragment": last_fragment,
            "directions": list(goals.DIRECTION_LABELS),
            "notice": goals.material_notice(material),
        }
        raw = run_popup(payload, window="goal", detail="подбор цели")
        if not raw:
            return None
        try:
            data = json.loads(raw)
        except ValueError:
            return None
        return data if isinstance(data, dict) else None

    def _run_goal(self, material: str, direction: str, note: str, origin: str = "") -> None:
        if self._busy:
            emit("action", window="goal", detail="busy, request dropped")
            self.flash.show_notice("ещё думаю над прошлым")
            return
        self._busy = True
        self._log_prompt("goal", "goal")
        self._goal_token += 1
        token = self._goal_token
        emit(
            "action",
            window="goal",
            detail=f"goal: {len(material)} chars, direction={direction or 'own'}",
        )

        def worker():
            try:
                result = self.service.suggest_goal(material, direction, note)
                idle_add(self._goal_done, token, result, None,
                         material, direction, origin, note)
            except Exception as exc:  # noqa: BLE001
                debug(f"goal failed: {exc}\n{traceback.format_exc()}")
                idle_add(self._goal_done, token, None, str(exc),
                         material, direction, origin, note)

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _goal_done(
        self,
        token: int,
        result,
        error: str | None,
        material: str = "",
        direction: str = "",
        origin: str = "",
        note: str = "",
    ):
        if token != self._goal_token:
            # A newer request owns the screen; a wording the reader has moved
            # past must not reopen a window.
            debug("goal: dropping a late answer")
            return False
        self._busy = False
        if error:
            emit("error", window="goal", detail=str(error))
            # The passage and the direction stay in hand: retry is one click, and
            # the general wording for the direction is there to accept instead.
            self._goal_result(
                goals.direction_draft(direction, note), direction,
                material=material, origin=origin, note=note,
                error=f"Не удалось уточнить формулировку: {error}",
                retryable=True,
            )
            return False
        emit("result", window="goal", detail=f"goal {len(result.text)} chars")
        self._goal_result(
            result.text, result.direction or direction,
            material=material, origin=origin, note=note,
            notice=f"Предложено по: {result.direction or 'своей формулировке'}",
        )
        return False

    def _goal_result(
        self,
        phrase: str,
        direction: str,
        *,
        material: str = "",
        origin: str = "",
        note: str = "",
        notice: str = "",
        error: str = "",
        retryable: bool = False,
    ) -> None:
        """Offer one wording for acceptance; nothing is stored until accepted.

        Editing is available in the window and never required. The passage and
        the direction stay in hand, so «Другое направление» and «Повторить»
        work from what the reader already chose instead of starting over.
        """
        payload: dict[str, object] = {
            "mode": "goal_result",
            "text": phrase,
            "direction": direction,
            "notice": notice,
            "error": error,
            "retryable": retryable,
            "has_material": bool(material.strip()),
        }
        raw = run_popup(payload, window="goal_result", detail="цель")
        if not raw:
            emit("action", window="goal_result", detail="closed without saving")
            return
        try:
            data = json.loads(raw)
        except ValueError:
            emit("error", window="goal_result", detail="bad popup payload")
            return
        if not isinstance(data, dict):
            return
        action = str(data.get("action", ""))
        if action == "accept":
            self._goal_accept(
                str(data.get("text", "")).strip() or phrase, direction, material, origin,
            )
            return
        if action == "another":
            self._goal_assist(material, origin)
            return
        if action == "retry":
            self._run_goal(material, direction, note, origin)
            return
        emit("action", window="goal_result", detail="closed without saving")

    def _goal_accept(self, phrase: str, direction: str, material: str, origin: str) -> None:
        """Store the accepted wording together with the passage that grounded it."""
        if not phrase:
            emit("action", window="goal_result", detail="empty wording, nothing saved")
            return
        try:
            saved = RECORDS.save_intention(
                phrase, direction=direction, material=material, material_origin=origin,
            )
        except (RecordError, ValueError) as exc:
            emit("error", window="goal_result", detail=f"save failed: {exc}")
            self.flash.show_text(f"Цель не сохранена:\n\n{exc}", "Сейчас хочу")
            return
        emit("result", window="goal_result", detail=f"new intention {saved.id[:8]}")

    def show_example(self, with_request: bool = False) -> None:
        """One concrete example for what is highlighted right now.

        With a selection the call stays direct: highlight, press, read. A window
        appears only when there is nothing to illustrate, or when the reader asked
        to add their own request; only then is the saved bookmark offered, as an
        explicit checkbox that is off by default.
        """
        source = selected_text()
        observe_artifact("example_source", {"selection": source, "requested": with_request})
        if not source.strip() or with_request:
            opened = self._example_input(source)
            if opened is None:
                emit("action", window="example", detail="cancelled")
                return
            if opened.get("action") == "last":
                self._show_last_example()
                return
            source = str(opened.get("material", ""))
            query = str(opened.get("query", ""))
            use_intent = bool(opened.get("use_intent", False))
        else:
            query = ""
            use_intent = False
        material = source.strip()
        if not material:
            emit("action", window="example", detail="no material")
            self.flash.show_text(
                "Нет материала: выделите текст или откройте пример с запросом.",
                "Покажи на примере",
            )
            return
        intent = ""
        if use_intent:
            try:
                current = RECORDS.current_intention()
            except RecordError as exc:
                debug(f"intent read failed: {exc}")
                current = None
            if current:
                intent = current.text
        self._run_example(material, query, intent)

    def _example_input(self, material: str) -> dict | None:
        try:
            current = RECORDS.current_intention()
        except RecordError:
            current = None
        payload: dict[str, object] = {
            "mode": "example",
            "material": material,
            "query": "",
            "has_intent": current is not None,
            "intent_text": current.text if current else "",
            "has_last": self._last_example is not None,
        }
        raw = run_popup(payload, window="example", detail="пример")
        if not raw:
            return None
        try:
            data = json.loads(raw)
        except ValueError:
            return None
        return data if isinstance(data, dict) else None

    def _run_example(self, material: str, query: str, intent: str) -> None:
        if self._busy:
            emit("action", window="example", detail="busy, request dropped")
            self.flash.show_notice("ещё думаю над прошлым")
            return
        key = self.service.example_cache_key(material, query, intent)
        cached = self._example_cache.get(key)
        if cached is not None:
            emit("action", window="example", detail="cache hit")
            self._show_example_text(cached)
            return
        self._busy = True
        self._log_prompt("example", "example")
        self._example_token += 1
        token = self._example_token
        emit(
            "action",
            window="example",
            detail=(
                f"example: {len(material)} chars, query={len(query)}, "
                f"intent={'yes' if intent else 'no'}"
            ),
        )

        def worker():
            try:
                result = self.service.show_example(material, query, intent)
                idle_add(self._example_done, token, key, result, None)
            except Exception as exc:  # noqa: BLE001
                debug(f"example failed: {exc}\n{traceback.format_exc()}")
                idle_add(self._example_done, token, key, None, str(exc))

        threading.Thread(target=bind_chain(worker), daemon=True).start()

    def _example_done(self, token: int, key: tuple, result, error: str | None):
        if token != self._example_token:
            # A newer request owns the screen; a late answer must not reopen a
            # window the reader has already moved on from.
            debug("example: dropping a late answer")
            return False
        self._busy = False
        if error:
            emit("error", window="example", detail=str(error))
            self.flash.show_text(f"Ошибка примера:\n\n{error}", "Покажи на примере")
            return False
        if len(self._example_cache) >= 32:
            self._example_cache.clear()
        self._example_cache[key] = result
        return self._show_example_text(result)

    def _show_example_text(self, result):
        intent = str(getattr(result, "intent_snapshot", "") or "")
        note = f"Учтена цель: {intent}" if intent else ""
        self._last_example = {"text": result.text, "note": note}
        emit("result", window="example", detail=f"example {len(result.text)} chars")
        self.flash.show_text(result.text, "Покажи на примере", note=note)
        return False

    def _show_last_example(self) -> None:
        if not self._last_example:
            return
        emit("action", window="example", detail="reopened last example")
        self.flash.show_text(
            self._last_example["text"],
            "Покажи на примере",
            note=self._last_example.get("note", ""),
        )

    def _commit_fragment(self, fragment) -> None:
        try:
            RECORDS.save_fragment(self.service.session, fragment)
        except RecordError as exc:
            debug(f"record fragment failed: {exc}")
            emit("error", window="seed", detail=f"ledger: {exc}")
            return
        # Only material the reader deliberately saved is a basis for practice; a
        # moving highlight never starts work on its own (see watch_selection).
        self._submit_preparation(fragment)

    def _commit_prediction(self, check) -> None:
        try:
            RECORDS.save_prediction(self.service.session, check)
        except RecordError as exc:
            debug(f"record prediction failed: {exc}")
            emit("error", window="prediction", detail=f"ledger: {exc}")

    def _commit_feynman(self, check) -> None:
        try:
            RECORDS.save_feynman(self.service.session, check)
        except RecordError as exc:
            debug(f"record feynman failed: {exc}")
            emit("error", window="feynman", detail=f"ledger: {exc}")

    def _close_session(self, session_id, reason, closed_utc=None) -> None:
        try:
            RECORDS.close_session(session_id, reason, closed_utc=closed_utc)
        except RecordError as exc:
            debug(f"record close failed: {exc}")

    def _split_idle_session(self) -> None:
        """End the reading session when the pause since the last hotkey was long.

        The split is decided at the moment of use rather than by a timer, and the
        session is closed at the last hotkey instead of at the returning press,
        so the recorded span equals the time actually spent reading.
        """
        retired = self.service.mark_activity()
        if retired is None:
            return
        self._close_session(retired.session.id, "idle", closed_utc=retired.ended_utc)
        debug(
            f"idle split: {len(retired.session.fragments)} fragments, "
            f"ended {retired.ended_utc}"
        )
        emit(
            "session_split",
            window="session",
            detail=f"idle pause, {len(retired.session.fragments)} fragments",
        )

    # ── background preparation (see practice.py) ─────────────────────────────

    def _panel_settings(self) -> dict:
        try:
            return hud_state.load_settings()
        except (OSError, ValueError) as exc:
            debug(f"panel settings unreadable: {exc}")
            return dict(hud_state.DEFAULT_SETTINGS)

    def _submit_preparation(self, fragment) -> None:
        """Offer one saved fragment to the background preparer, when it is on."""
        if self.preparer is None or fragment is None:
            return
        if not self._panel_settings().get("prepare_in_background"):
            return
        try:
            goal = RECORDS.current_intention()
        except RecordError as exc:
            debug(f"preparation goal read failed: {exc}")
            goal = None
        context = practice.context_from_fragment(
            fragment,
            model=getattr(self.client, "model", ""),
            goal=goal,
            prompt_revision=prompt_hash(practice.analysis_system_text())[:16],
        )
        try:
            preparation_id = self.preparer.submit(context)
        except (RecordError, OSError) as exc:
            debug(f"preparation submit failed: {exc}")
            emit("error", window="preparation", detail=str(exc))
            return
        if preparation_id:
            emit(
                "preparation_queued",
                window="preparation",
                detail=f"fragment={str(getattr(fragment, 'id', ''))[:8]} "
                       f"source={len(context.source_text)} chars",
            )

    def _preparation_stage(self, status: str, preparation_id: str):
        """On the main loop: one log line per stage, so the scale is auditable."""
        emit("preparation_stage", window="preparation",
             detail=f"{status} {str(preparation_id)[:8]}")
        return False

    def _preparation_result(self, outcome, error: str = ""):
        if error:
            emit("error", window="preparation", detail=str(error))
            return False
        if outcome is None:
            return False
        detail = f"{outcome.status} {str(outcome.preparation_id)[:8]}"
        if outcome.task_id:
            detail += f" task={outcome.task_id[:8]}"
        if outcome.reason:
            detail += f" ({outcome.reason})"
        emit("result", window="preparation", detail=detail)
        return False

    def _hud_note_setting(self, key: str, value: bool):
        emit("hud_setting", window="hud", detail=f"{key}={value}")
        if key == "prepare_in_background" and value:
            # Turning it on with material already in hand has to do something
            # visible; otherwise the switch reads as broken.
            fragments = list(self.service.session.fragments)
            if fragments:
                self._submit_preparation(fragments[-1])
        return False

    # ── side panel (see hud.py and hud_ipc.py) ───────────────────────────────

    def hud_request(self, request: dict) -> dict:
        """Answer one panel request.

        Called on the socket thread, so it stays short and free of GTK calls.
        Reads are answered here; everything that changes state — opening a
        window, writing the ledger — is handed to the main loop and acknowledged
        at once, because an open popup can keep that loop busy for minutes.
        """
        command = str(request.get("command") or "")
        if command == "ping":
            return {"ok": True, "status": "ready", "pid": os.getpid()}
        if command == "state":
            return self._hud_state()
        if command == "action":
            action = str(request.get("action") or "")
            if action not in HUD_ACTIONS and action not in HUD_TASK_ACTIONS:
                return {"ok": False, "error": f"unknown action {action!r}"}
            GLib.idle_add(self._hud_run_action, action, str(request.get("task_id") or ""))
            return {"ok": True, "accepted": True}
        if command == "dismiss":
            task_id = str(request.get("task_id") or "")
            if not task_id:
                return {"ok": False, "error": "dismiss without a task id"}
            GLib.idle_add(self._hud_dismiss, task_id)
            return {"ok": True, "accepted": True}
        if command == "settings":
            key = str(request.get("key") or "")
            if key not in hud_state.SETTING_KEYS:
                return {"ok": False, "error": f"unknown setting {key!r}"}
            value = bool(request.get("value"))
            try:
                settings = hud_state.set_setting(key, value)
            except (KeyError, OSError, ValueError) as exc:
                return {"ok": False, "error": str(exc)}
            GLib.idle_add(self._hud_note_setting, key, value)
            return {"ok": True, "settings": settings}
        return {"ok": False, "error": f"unknown command {command!r}"}

    def _hud_state(self) -> dict:
        """What the panel may show: the accepted goal and the offered task.

        Read-only and short — the panel asks every few seconds — and a failure
        is reported as an absent field rather than as an error the panel would
        have to interpret.
        """
        state: dict = {
            "ok": True,
            "pid": os.getpid(),
            "busy": bool(self._busy),
            "buffer": len(self.service.session.fragments),
        }
        try:
            goal = hud_state.goal_view(RECORDS)
        except RecordError as exc:
            debug(f"hud goal read failed: {exc}")
            goal = None
        state["goal"] = (
            {"id": goal.id, "text": goal.text, "criterion": goal.criterion} if goal else None
        )
        try:
            offered = hud_state.latest_practice(
                RECORDS,
                dismissed=hud_state.load_dismissed(),
                current_session_id=self.service.session.id,
            )
        except (RecordError, OSError, ValueError) as exc:
            debug(f"hud practice read failed: {exc}")
            offered = None
        state["practice"] = offered.payload() if offered else None
        state["settings"] = self._panel_settings()
        try:
            view = hud_state.preparation_view(
                RECORDS,
                offered_task_id=offered.task_id if offered else "",
                stage_count=len(practice.STAGES),
            )
        except (RecordError, OSError, ValueError) as exc:
            debug(f"hud preparation read failed: {exc}")
            view = None
        state["preparation"] = view.payload() if view else None
        return state

    def _hud_run_action(self, action: str, task_id: str = ""):
        try:
            self.dispatch_action(action, task_id=task_id)
        except Exception as exc:  # noqa: BLE001 - a panel click must not take the daemon down
            debug(f"panel action {action} failed: {exc}\n{traceback.format_exc()}")
            emit("error", window="hud", detail=str(exc))
        return False

    def _hud_dismiss(self, task_id: str):
        try:
            hud_state.dismiss(task_id)
        except OSError as exc:
            debug(f"hud dismiss failed: {exc}")
            emit("error", window="hud", detail=f"dismiss: {exc}")
            return False
        emit("hud_dismissed", window="hud", detail=task_id[:8])
        return False

    def dispatch_action(self, action: str, *, origin: str = "panel", task_id: str = "") -> bool:
        """Run one panel action along the same path a hotkey takes.

        The panel is a second way in, not a second implementation: it reaches
        the method the signal handler calls, so the log, the busy guard and the
        idle-session split behave identically.
        """
        if action not in HUD_ACTIONS and action not in HUD_TASK_ACTIONS:
            emit("error", window="hud", detail=f"unknown action {action!r}")
            return False
        begin_interaction("panel", window=f"hud:{action}", detail=origin)
        self._split_idle_session()
        if action == "task_menu":
            return self._launch_tasks(["--menu"])
        if action == "task_practice":
            if not task_id:
                emit("action", window="hud", detail="practice without a task id")
                return False
            return self._launch_tasks(["--practice", task_id])
        getattr(self, HUD_ACTIONS[action])()
        return True

    def _launch_tasks(self, arguments: list[str]) -> bool:
        """Open the tasks window in its own process, with this chain attached."""
        if not TASKS_SCRIPT.is_file():
            emit("error", window="hud", detail=f"tasks script missing: {TASKS_SCRIPT}")
            self.flash.show_text(f"Не найден скрипт задач:\n\n{TASKS_SCRIPT}", "Практика")
            return False
        try:
            # The script resolves its own checkout from COGNITIVE_PROJECT; pin it
            # to the one this daemon belongs to, not to the default path.
            env = {**popup_env(), "COGNITIVE_PROJECT": str(PROJECT_ROOT)}
            subprocess.Popen(
                [str(TASKS_SCRIPT), *arguments],
                env=env,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
        except OSError as exc:
            emit("error", window="hud", detail=f"tasks failed: {exc}")
            self.flash.show_text(f"Задача не открылась:\n\n{exc}", "Практика")
            return False
        emit("action", window="hud", detail=f"tasks {' '.join(arguments)}")
        return True

    def _start_panel_socket(self) -> HudServer | None:
        """Publish the panel socket.

        A panel that cannot be served is not a failure: the hotkeys are the
        primary way in and keep working without it.
        """
        server = HudServer(handler=self.hud_request, log=debug)
        try:
            server.start()
        except HudError as exc:
            debug(f"panel socket unavailable: {exc}")
            emit("error", window="hud", detail=str(exc))
            return None
        emit("hud_ready", window="hud", detail=str(server.path))
        return server

    def run(self) -> None:
        emit("app_start", detail=f"pid {os.getpid()}")
        self.hud = self._start_panel_socket()
        self.preparer.start()
        state = self.focus.snapshot()
        if state and state["status"] == "running":
            self._show_focus_timer(state)
        GLib.timeout_add(WATCH_MS, self.watch_selection)
        for signum, handler in ((signal.SIGUSR1, self._signal_seed),
                                (signal.SIGWINCH, self._signal_menu),
                                (signal.SIGUSR2, self._signal_feynman),
                                (signal.SIGHUP, self._signal_prediction)):
            GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signum, keep_listening(self._with_peek(handler)))
        # SIGWINCH doubles as the wake-up for queued actions (see take_request):
        # GLib accepts only six signals and the rest above already claim them.

        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, self._signal_quit)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGINT, self._signal_quit)
        try:
            Gtk.main()
        finally:
            # Stopped first: no new model call should start while the daemon is
            # shutting down. Work cut off here is closed as `interrupted` on the
            # next start rather than left looking live.
            self.preparer.stop()
            state = self.focus.snapshot()
            if state and state["status"] in focus.ACTIVE:
                self.focus.finish(state["id"], "interrupted")
                self._close_focus_timer(state["id"])
            if self.hud is not None:
                self.hud.stop()
                self.hud = None
            try:
                RECORDS.close_session(self.service.session.id, reason="shutdown")
            except RecordError as exc:
                debug(f"record close failed: {exc}")
            try:
                PID_PATH.unlink()
            except FileNotFoundError:
                pass

    def _signal_seed(self):
        begin_interaction("hotkey", window="seed", detail="4 слова")
        self._split_idle_session()
        self.seed()
        return True

    def _signal_menu(self):
        request = take_request()
        if request == "focus":
            # Ctrl+Alt+G is retired; an old binding still lands on the step.
            begin_interaction("hotkey", window="step", detail="шаг (старая клавиша)")
            self.start_step()
            return True
        if request == "seed-batch":
            begin_interaction("hotkey", window="seed_batch", detail="4-words-batch")
            self._split_idle_session()
            self.four_words_batch()
            return True
        if request == "note":
            begin_interaction("hotkey", window="note", detail="ошибка чтения")
            self._split_idle_session()
            self._off_main("note", self.capture_error_note)
            return True
        if request == "clarify":
            begin_interaction("hotkey", window="clarify", detail="термины")
            self._split_idle_session()
            self._off_main("clarify", self.explain_terms)
            return True
        if request in ("reframe", "reframe-ask"):
            # Frozen 2026-10-07: "another angle" made the reader reread instead of
            # generating (note 38). The code stays; the hotkey opens nothing.
            begin_interaction("hotkey", window="reframe", detail="другой ракурс")
            emit("action", window="reframe", detail="frozen")
            return True
        if request == "summary":
            begin_interaction("hotkey", window="summary", detail="сжат текст (Ctrl+Q)")
            self._split_idle_session()
            self.summarize_text()
            return True
        if request == "keys":
            begin_interaction("hotkey", window="keys", detail="справка по клавишам")
            self._split_idle_session()
            self.show_keys()
            return True
        if request == "intent":
            # Alt+I is the 15-minute step (2026-10-07); the old goal bookmark and
            # its wording assistant are frozen, their code kept.
            begin_interaction("hotkey", window="step", detail="шаг")
            self._split_idle_session()
            self.start_step()
            return True
        if request == "example":
            begin_interaction("hotkey", window="example", detail="покажи на примере")
            self._split_idle_session()
            self._off_main("example", self.show_example)
            return True
        if request == "example-ask":
            begin_interaction("hotkey", window="example", detail="пример со своим запросом")
            self._split_idle_session()
            self._off_main("example", lambda: self.show_example(with_request=True))
            return True
        begin_interaction("hotkey", window="menu", detail="mode menu")
        self._split_idle_session()
        self.show_mode_menu()
        return True

    def _signal_feynman(self):
        begin_interaction("hotkey", window="feynman", detail="Feynman")
        self._split_idle_session()
        self.start_feynman()
        return True

    def _signal_prediction(self):
        begin_interaction("hotkey", window="prediction", detail="Моя гипотеза")
        self._split_idle_session()
        self._off_main("prediction", self.start_prediction)
        return True

    def _off_main(self, name: str, action) -> None:
        """Run a hotkey action that opens an input window off the main loop.

        These actions wait for the reader to close their window; on the main
        loop that held every other hotkey in the queue (07.10: Alt+C "flew away"
        until the note window was closed). The same key pressed while its own
        window is still open does nothing.
        """
        running = self.__dict__.setdefault("_windows_running", set())
        if name in running:
            emit("action", window=name, detail="already open")
            return

        def worker():
            try:
                action()
            except Exception as exc:  # noqa: BLE001 - one failed window must not kill the key
                debug(f"{name} failed: {exc}\n{traceback.format_exc()}")
                emit("error", window=name, detail=f"{type(exc).__name__}: {exc}"[:200])
            finally:
                running.discard(name)

        running.add(name)
        threading.Thread(target=bind_chain(worker), daemon=True).start()


    def _signal_quit(self):
        Gtk.main_quit()
        return False


def main() -> int:
    parser = argparse.ArgumentParser(description="Cognitive Popups desktop layer")
    parser.add_argument("--api-url", default=os.environ.get("COGNITIVE_API_URL", "http://127.0.0.1:8081/v1/chat/completions"))
    parser.add_argument("--model", default=os.environ.get("COGNITIVE_MODEL", "gemini-flash-lite"))
    parser.add_argument("--diagnose", action="store_true", help="print what each selection source returns and exit")
    args = parser.parse_args()
    if args.diagnose:
        return diagnose_selection()
    app = DesktopApp(GeminiWeb2API(url=args.api_url, model=args.model))
    app.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
