from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import threading
from pathlib import Path
from collections.abc import Sequence

from .client import GeminiWeb2API, Web2APIError
from .history import SessionHistory
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
FLASH_MS = 1500
WATCH_MS = 800
FLASH_MARGIN = 28
WATCH_POPUP_HELPER = Path(
    os.environ.get(
        "COGNITIVE_POPUP_HELPER",
        "~/.local/bin/objects-tooltip-popup.py",
    )
).expanduser()
POPUP_PYTHON = os.environ.get("COGNITIVE_POPUP_PYTHON", sys.executable)

MODE_MENU = [
    {"label": "4 слова", "action": "four_words"},
    {"label": "Фейнман", "action": "feynman"},
]


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


def popup_input(prompt: str, title: str) -> str:
    if not WATCH_POPUP_HELPER.is_file():
        return ""
    payload: dict[str, object] = {"mode": "input", "prompt": prompt, "title": title}
    position = cursor_position()
    if position:
        payload["x"], payload["y"] = position
    try:
        result = subprocess.run(
            [POPUP_PYTHON, str(WATCH_POPUP_HELPER)],
            input=json.dumps(payload, ensure_ascii=False),
            capture_output=True,
            text=True,
            env={**os.environ, "GDK_BACKEND": "x11"},
            timeout=3600,
        )
        return result.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def selected_text(primary_only: bool = False) -> str:
    probes = [
        ["wl-paste", "--primary", "--no-newline"],
        ["xclip", "-selection", "primary", "-o"],
    ]
    if not primary_only:
        probes += [["wl-paste", "--no-newline"], ["xclip", "-selection", "clipboard", "-o"]]
    for command in probes:
        try:
            result = subprocess.run(
                command,
                capture_output=True,
                text=True,
                timeout=2,
                check=False,
            )
        except (FileNotFoundError, subprocess.SubprocessError):
            continue
        if result.returncode != 0:
            continue
        text = (result.stdout or "").strip()
        if text:
            return text
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
        if not WATCH_POPUP_HELPER.is_file():
            return
        try:
            # Capture the pointer before spawning the popup.  After Popen the
            # new X11 window may become active and no longer be the right
            # reference for placement.
            payload: dict[str, object] = {"mode": "dual", "items": items}
            position = cursor_position()
            if position:
                payload["x"], payload["y"] = position
            proc = subprocess.Popen(
                [POPUP_PYTHON, str(WATCH_POPUP_HELPER)],
                stdin=subprocess.PIPE,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
                start_new_session=True,
                env={**os.environ, "GDK_BACKEND": "x11"},
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
        feynman.connect("clicked", lambda *_: self.app.start_feynman())
        header.pack_start(feynman, False, False, 0)
        clear = Gtk.Button(label="Clear buffer")
        clear.connect("clicked", lambda *_: self.app.clear_buffer())
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
            f"{len(session.feynman_checks)} Feynman checks"
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
                f"{len(archived.fragments)} fragments · {len(archived.feynman_checks)} checks · {archived.created_at}",
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
        if not WATCH_POPUP_HELPER.is_file():
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

        def worker():
            try:
                result = subprocess.run(
                    [POPUP_PYTHON, str(WATCH_POPUP_HELPER)],
                    input=json.dumps(payload, ensure_ascii=False),
                    capture_output=True,
                    text=True,
                    env={**os.environ, "GDK_BACKEND": "x11"},
                    timeout=3600,
                )
                if result.stdout.strip():
                    action = json.loads(result.stdout).get("action", "")
                    if action in {item["action"] for item in MODE_MENU}:
                        GLib.idle_add(self.run_mode, action)
            except (OSError, ValueError, subprocess.SubprocessError):
                self.flash.show_cues(["menu error"])

        threading.Thread(target=worker, daemon=True).start()

    def run_mode(self, mode: str):
        if mode == "four_words":
            self.seed()
        elif mode == "feynman":
            self.start_feynman()
        return False

    def seed(self) -> None:
        text = selected_text(primary_only=True)
        if not text:
            self.flash.show_cues(["no selection"])
            return
        cues = self._cue_cache.get(text)
        if cues:
            self.service.add_fragment_with_cues(text, cues)
            self.flash.show_cues(cues)

            return
        if self._busy:
            return
        self._busy = True

        def worker():
            try:
                cues = self.service.extract_cues(text)
                self._cue_cache[text] = cues
                fragment = self.service.add_fragment_with_cues(text, cues)
                GLib.idle_add(self._seed_done, fragment.cue_details, None)
            except Exception as exc:  # noqa: BLE001
                GLib.idle_add(self._seed_done, ["error"], str(exc))

        threading.Thread(target=worker, daemon=True).start()

    def _seed_done(self, cues: list[dict[str, str]], error: str | None):
        self._busy = False
        self.flash.show_cues(cues)
        return False

    def watch_selection(self):
        text = selected_text(primary_only=True)
        if text and text != self._last_selection:
            self._last_selection = text
            if text not in self._cue_cache and text not in self._pending_keys and not self._busy:
                self._pending_keys.add(text)

                def worker():
                    try:
                        self._cue_cache[text] = self.service.extract_cues(text)
                    except Exception:
                        pass
                    finally:
                        self._pending_keys.discard(text)

                threading.Thread(target=worker, daemon=True).start()
        return True

    def start_feynman(self) -> None:
        if not self.service.session.fragments:
            self.flash.show_cues(["buffer is empty"])
            return
        if self._busy:
            return
        self._busy = True

        def worker():
            try:
                question = self.service.create_feynman_question()
                GLib.idle_add(self._show_feynman, question, None)
            except Exception as exc:  # noqa: BLE001
                GLib.idle_add(self._show_feynman, "", str(exc))

        threading.Thread(target=worker, daemon=True).start()

    def _show_feynman(self, question: str, error: str | None):
        self._busy = False
        if error:
            self.flash.show_cues(["Feynman error"])
            return False
        explanation = popup_input(question, "Feynman check")
        if not explanation:
            return False
        self._busy = True

        def worker():
            try:
                check = self.service.check_feynman(question, explanation)
                GLib.idle_add(self._check_done, check.status, check.gaps, check.follow_up)
            except Exception:
                GLib.idle_add(self._check_done, "error", [], None)

        threading.Thread(target=worker, daemon=True).start()
        return False

    def _check_done(self, status: str, gaps: list[dict[str, str]], follow_up: str | None):
        self._busy = False
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
        explanation = popup_input(f"{gap_text}\n\n{prompt}", "Feynman retry")
        if not explanation:
            return
        self._busy = True

        def worker():
            try:
                check = self.service.check_feynman(prompt, explanation)
                GLib.idle_add(self._retry_done, check.status)
            except Exception:
                GLib.idle_add(self._retry_done, "error")

        threading.Thread(target=worker, daemon=True).start()

    def _retry_done(self, status: str):
        self._busy = False
        self.flash.show_cues(["retry passed"] if status == "passed" else ["retry not passed"])
        return False

    def clear_buffer(self) -> None:
        if not self.service.session.fragments:
            return
        archived = self.service.clear_buffer()
        self.history.append(archived)
        self.flash.show_cues(["buffer cleared"])

    def run(self) -> None:
        GLib.timeout_add(WATCH_MS, self.watch_selection)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, self._signal_seed)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGWINCH, self._signal_menu)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR2, self._signal_feynman)

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
        self.seed()
        return True

    def _signal_menu(self):
        self.show_mode_menu()
        return True

    def _signal_feynman(self):
        self.start_feynman()
        return True


    def _signal_quit(self):
        Gtk.main_quit()
        return False


def main() -> int:
    parser = argparse.ArgumentParser(description="Cognitive Popups desktop layer")
    parser.add_argument("--api-url", default=os.environ.get("COGNITIVE_API_URL", "http://127.0.0.1:8081/v1/chat/completions"))
    parser.add_argument("--model", default=os.environ.get("COGNITIVE_MODEL", "gemini-flash-lite"))
    args = parser.parse_args()
    app = DesktopApp(GeminiWeb2API(url=args.api_url, model=args.model))
    app.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
