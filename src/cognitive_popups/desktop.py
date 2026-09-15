from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import threading
import traceback
from pathlib import Path
from collections.abc import Sequence

from .client import GeminiWeb2API, Web2APIError
from .event_log import EventChain, EventLog, new_session
from .history import SessionHistory
from .notes import NoteStore
from .service import CognitiveService

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

MODE_MENU = [
    {"label": "4 слова", "action": "four_words"},
    {"label": "Фейнман", "action": "feynman"},
    {"label": "Моя гипотеза", "action": "prediction"},
]


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


CURRENT: EventChain | None = None
_CURRENT_LOCK = threading.Lock()


def current_chain() -> EventChain:
    """The active interaction, starting an empty one when there is none yet."""
    global CURRENT
    with _CURRENT_LOCK:
        if CURRENT is None:
            CURRENT = EventChain(EVENTS, new_session(), origin="desktop", pid=os.getpid())
        return CURRENT


def begin_interaction(event: str, **fields) -> EventChain:
    """Start a fresh interaction: one hotkey press and everything it causes."""
    global CURRENT
    with _CURRENT_LOCK:
        chain = EventChain(EVENTS, new_session(), origin="desktop", pid=os.getpid())
        CURRENT = chain
    chain.emit(event, **fields)
    return chain


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
    if CURRENT is not None:
        env.update(CURRENT.env())
    return env


def panel_click(label: str, action) -> None:
    """Record a click on the resident panel, then run the action it triggers."""
    emit("click", window="panel", item_label=label)
    action()


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


def cursor_position() -> tuple[int, int] | None:
    """Return Hyprland's global cursor coordinates, not GTK window coordinates."""
    try:
        result = subprocess.run(
            ["hyprctl", "cursorpos"],
            capture_output=True,
            text=True,
            timeout=1,
            check=True,
        )
        x_text, y_text = result.stdout.strip().replace(" ", "").split(",", 1)
        return int(x_text), int(y_text)
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
    emit("window_spawn", window=window, detail=detail)
    try:
        result = subprocess.run(
            command,
            input=json.dumps(payload, ensure_ascii=False),
            capture_output=True,
            text=True,
            env=popup_env(),
            timeout=3600,
        )
        return result.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def popup_input(prompt: str, title: str) -> str:
    payload: dict[str, object] = {"mode": "input", "prompt": prompt, "title": title}
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
            text=True,
            timeout=2,
            check=False,
        )
    except FileNotFoundError as exc:
        return None, "", f"not installed ({exc.filename})"
    except subprocess.TimeoutExpired:
        return None, "", "timed out"
    except OSError as exc:
        return None, "", str(exc)
    return result.returncode, result.stdout or "", result.stderr or ""


def diagnose_selection(stream=None) -> int:
    """Print what every selection source returns right now."""
    out = stream or sys.stdout
    for name, command in selection_probes():
        code, stdout, stderr = run_probe(command)
        preview = stdout.strip().replace("\n", "\\n")[:120]
        status = "missing" if code is None else f"rc={code}"
        print(f"{name:20s} {status:8s} {preview or ('<- ' + stderr.strip()[:80] if stderr.strip() else '(empty)')}", file=out)
    return 0


def selected_text(primary_only: bool = False) -> str:
    for name, command in selection_probes(primary_only=primary_only):
        code, stdout, stderr = run_probe(command)
        if code == 0:
            text = stdout.strip()
            if text:
                debug(f"selection from {name} ({len(text)} chars): {text[:60]!r}")
                return text
        debug(f"selection probe {name} -> rc={code} stderr={stderr.strip()[:80] or '-'}")
    debug("selection probes returned nothing")
    return ""


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
            emit("window_spawn", window="dual", detail=f"{len(items)} cues")
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

    def show_text(self, text: str, title: str = "Result") -> None:
        command = popup_helper_command()
        if not text or command is None:
            return
        try:
            payload: dict[str, object] = {"mode": "text", "text": text, "title": title}
            position = cursor_position()
            if position:
                payload["x"], payload["y"] = position
            emit("window_spawn", window="text", detail=title)
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

    @staticmethod
    def _row(title: str, body: str) -> Gtk.ListBoxRow:
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

        self._clear(self.cues_list)
        all_fragments = list(session.fragments)
        for archived in self.app.history.load():
            all_fragments.extend(archived.fragments)
        for fragment in reversed(all_fragments):
            self.cues_list.add(self._row(" · ".join(fragment.cues), fragment.created_at))
        if not all_fragments:
            self.cues_list.add(self._row("No cues yet", ""))

        archived_sessions = self.app.history.load()
        self._clear(self.history_list)
        for archived in reversed(archived_sessions):
            self.history_list.add(self._row(
                archived.title,
                f"{len(archived.fragments)} fragments · {len(archived.feynman_checks)} Feynman checks · "
                f"{len(archived.prediction_checks)} hypotheses · {archived.created_at}",
            ))
        if not archived_sessions:
            self.history_list.add(self._row("No archived buffers", "Clear the current buffer to archive it here."))
        self.show_all()


class DesktopApp:
    def __init__(self, client: GeminiWeb2API):
        STATE_DIR.mkdir(parents=True, exist_ok=True)
        PID_PATH.write_text(str(os.getpid()), encoding="utf-8")
        self.service = CognitiveService(client)
        self.history = SessionHistory(HISTORY_PATH)
        self.flash = FlashWindow()
        self._busy = False
        self._last_selection = ""
        self._cue_cache: dict[str, list[dict[str, str]]] = {}
        self._pending_keys: set[str] = set()

    def show_mode_menu(self) -> None:
        command = popup_helper_command()
        if command is None:
            emit("error", window="menu", detail="popup helper missing")
            self.flash.show_cues(["popup helper missing"])
            return
        position = cursor_position()
        payload: dict[str, object] = {
            "mode": "menu",
            "title": "Cognitive modes",
            "items": MODE_MENU,
        }
        if position:
            payload["x"], payload["y"] = position
        emit("window_spawn", window="menu", detail="Cognitive modes")

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
                        GLib.idle_add(self.run_mode, action)
            except (OSError, ValueError, subprocess.SubprocessError) as exc:
                debug(f"mode menu failed: {exc}")
                emit("error", window="menu", detail=str(exc))
                self.flash.show_cues(["menu error"])

        threading.Thread(target=worker, daemon=True).start()

    def run_mode(self, mode: str):
        if mode == "four_words":
            self.seed()
        elif mode == "feynman":
            self.start_feynman()
        elif mode == "prediction":
            self.start_prediction()
        return False

    def seed(self) -> None:
        text = selected_text()
        if not text:
            debug("seed: no selection from any source")
            emit("action", window="seed", detail="no selection")
            self.flash.show_cues(["no selection"])
            return
        debug(f"seed: text ({len(text)} chars) {text[:60]!r}")
        cues = self._cue_cache.get(text)
        if cues:
            debug("seed: cache hit")
            fragment = self.service.add_fragment_with_cues(text, cues)
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
            return
        self._busy = True

        def worker():
            try:
                cues = self.service.extract_cues(text)
                self._cue_cache[text] = cues
                fragment = self.service.add_fragment_with_cues(text, cues)
                GLib.idle_add(self._seed_done, fragment.cue_details, None, fragment.id, fragment.source_hash)
            except Exception as exc:  # noqa: BLE001
                debug(f"seed failed: {exc}\n{traceback.format_exc()}")
                GLib.idle_add(self._seed_done, [], str(exc), None, None)

        threading.Thread(target=worker, daemon=True).start()

    def _seed_done(self, cues: list[dict[str, str]], error: str | None, fragment_id=None, source_hash=None):
        self._busy = False
        if error:
            emit("error", window="seed", detail=str(error))
            self.flash.show_text(f"Ошибка четырёх слов:\n\n{error}", "4 слова")
            return False
        emit("result", window="seed", detail=f"{len(cues)} cues", fragment_id=fragment_id, source_hash=source_hash)
        self.flash.show_cues(cues)
        return False

    def watch_selection(self):
        text = selected_text(primary_only=True)
        if text and text != self._last_selection:
            self._last_selection = text
            debug(f"watch: new primary selection ({len(text)} chars) {text[:60]!r}")
            if text not in self._cue_cache and text not in self._pending_keys and not self._busy:
                self._pending_keys.add(text)

                def worker():
                    try:
                        self._cue_cache[text] = self.service.extract_cues(text)
                    except Exception as exc:
                        debug(f"watch precompute failed: {exc}")
                    finally:
                        self._pending_keys.discard(text)

                threading.Thread(target=worker, daemon=True).start()
        return True

    def start_prediction(self) -> None:
        if not self.service.session.fragments:
            emit("action", window="prediction", detail="buffer is empty")
            self.flash.show_cues(["buffer is empty"])
            return
        if self._busy:
            emit("action", window="prediction", detail="busy, request dropped")
            return
        hypothesis = popup_input(
            "Сформулируй одну конкретную гипотезу о связи между объектами.",
            "Моя гипотеза",
        )
        if not hypothesis:
            return
        self._busy = True

        def worker():
            try:
                check = self.service.check_prediction(hypothesis)
                GLib.idle_add(self._prediction_done, check, None)
            except Exception as exc:  # noqa: BLE001
                GLib.idle_add(self._prediction_done, None, str(exc))

        threading.Thread(target=worker, daemon=True).start()

    def _prediction_done(self, check, error: str | None):
        self._busy = False
        if error:
            emit("error", window="prediction", detail=str(error))
            self.flash.show_text(
                f"Ошибка проверки гипотезы:\n\n{error}",
                "Моя гипотеза",
            )
            return False
        labels = {
            "confirmed": "подтверждено",
            "partially_confirmed": "частично подтверждено",
            "not_supported": "не подтверждено",
            "contradicted": "противоречит тексту",
            "unclear": "неясно",
        }
        lines = [
            f"Гипотеза: {check.hypothesis}",
            "",
            f"Результат: {labels.get(check.status, check.status)}",
        ]
        if check.evidence:
            lines.extend(["", f"В тексте: {check.evidence}"])
        if check.mismatch:
            lines.extend(["", f"Расхождение: {check.mismatch}"])
        emit("result", window="prediction", detail=check.status)
        self.flash.show_text("\n".join(lines), "Моя гипотеза")
        return False

    def start_feynman(self) -> None:
        if not self.service.session.fragments:
            emit("action", window="feynman", detail="buffer is empty")
            self.flash.show_cues(["buffer is empty"])
            return
        if self._busy:
            emit("action", window="feynman", detail="busy, request dropped")
            return
        self._busy = True

        def worker():
            try:
                question = self.service.create_feynman_question()
                GLib.idle_add(self._show_feynman, question, None)
            except Exception as exc:  # noqa: BLE001
                debug(f"feynman question failed: {exc}\n{traceback.format_exc()}")
                GLib.idle_add(self._show_feynman, "", str(exc))

        threading.Thread(target=worker, daemon=True).start()

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

        def worker():
            try:
                check = self.service.check_feynman(question, explanation)
                GLib.idle_add(self._check_done, check.status, check.gaps, check.follow_up)
            except Exception as exc:
                debug(f"feynman check failed: {exc}\n{traceback.format_exc()}")
                GLib.idle_add(self._check_done, "error", [], None, str(exc))

        threading.Thread(target=worker, daemon=True).start()
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
            self.flash.show_cues(["Feynman passed"])
        elif status == "needs_retry":
            self.flash.show_cues(["gap found", str(len(gaps))])
            self._offer_retry(gaps, follow_up)
        else:
            self.flash.show_cues(["Feynman error"])
        return False

    def _offer_retry(self, gaps: list[dict[str, str]], follow_up: str | None) -> None:
        gap_text = "\n".join(
            f"• {gap.get('description') or gap.get('location') or 'Gap'}"
            for gap in gaps[:2]
        )
        prompt = (follow_up or "Переформулируй объяснение, устранив найденный пробел.")
        emit("action", window="feynman", detail="retry offered")
        explanation = popup_input(f"{gap_text}\n\n{prompt}", "Feynman retry")
        if not explanation:
            return
        self._busy = True

        def worker():
            try:
                check = self.service.check_feynman(prompt, explanation)
                GLib.idle_add(self._retry_done, check.status)
            except Exception as exc:
                debug(f"feynman retry failed: {exc}\n{traceback.format_exc()}")
                GLib.idle_add(self._retry_done, "error", str(exc))

        threading.Thread(target=worker, daemon=True).start()

    def _retry_done(self, status: str, error: str | None = None):
        self._busy = False
        if error:
            emit("error", window="feynman", detail=f"retry: {error}")
            self.flash.show_text(f"Ошибка повторной проверки:\n\n{error}", "Фейнман")
            return False
        emit("result", window="feynman", detail=f"retry: {status}")
        self.flash.show_cues(["retry passed"] if status == "passed" else ["retry not passed"])
        return False

    def clear_buffer(self) -> None:
        if not self.service.session.fragments:
            return
        archived = self.service.clear_buffer()
        self.history.append(archived)
        emit("action", window="panel", detail=f"buffer cleared, {len(archived.fragments)} fragments archived")
        self.flash.show_cues(["buffer cleared"])

    def capture_error_note(self) -> None:
        """Record the reader's own account of a misreading (see notes.py)."""
        anchor = selected_text().strip()
        comment = run_popup(
            {"mode": "note", "anchor": anchor},
            window="note",
            detail=anchor[:60] or "no selection",
        )
        if not comment:
            emit("action", window="note", detail="cancelled")
            return
        # The buffer is the context the reader is in; note it when there is one.
        # A note still stands on its own through its anchor text if there is not.
        fragment = self.service.session.fragments[-1] if self.service.session.fragments else None
        try:
            note = NOTES.add(
                comment,
                anchor=anchor or None,
                fragment_id=fragment.id if fragment else None,
                source_hash=fragment.source_hash if fragment else None,
            )
        except Exception as exc:  # noqa: BLE001 - a lost note must be visible
            debug(f"error note failed: {exc}")
            emit("error", window="note", detail=str(exc))
            self.flash.show_text(f"Заметка не записана:\n\n{exc}", "Ошибка чтения")
            return
        emit(
            "result",
            window="note",
            detail=f"note {note.id} saved, {len(comment)} chars",
            fragment_id=note.fragment_id,
            source_hash=note.source_hash,
        )
        self.flash.show_cues([f"ошибка №{note.id} записана"])

    def run(self) -> None:
        emit("app_start", detail=f"pid {os.getpid()}")
        GLib.timeout_add(WATCH_MS, self.watch_selection)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, self._signal_seed)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGWINCH, self._signal_menu)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR2, self._signal_feynman)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGHUP, self._signal_prediction)
        # SIGWINCH doubles as the wake-up for queued actions (see take_request):
        # GLib accepts only six signals and the rest above already claim them.

        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, self._signal_quit)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGINT, self._signal_quit)
        try:
            Gtk.main()
        finally:
            try:
                PID_PATH.unlink()
            except FileNotFoundError:
                pass

    def _signal_seed(self):
        begin_interaction("hotkey", window="seed", detail="4 слова")
        self.seed()
        return True

    def _signal_menu(self):
        if take_request() == "note":
            begin_interaction("hotkey", window="note", detail="ошибка чтения")
            self.capture_error_note()
            return True
        begin_interaction("hotkey", window="menu", detail="mode menu")
        self.show_mode_menu()
        return True

    def _signal_feynman(self):
        begin_interaction("hotkey", window="feynman", detail="Feynman")
        self.start_feynman()
        return True

    def _signal_prediction(self):
        begin_interaction("hotkey", window="prediction", detail="Моя гипотеза")
        self.start_prediction()
        return True


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
