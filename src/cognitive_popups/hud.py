"""The resident side panel: the same actions as the hotkeys, plus one quiet hint.

The panel is a second way to reach the tools — useful when a hand is on the
mouse — and the place where a prepared practice task becomes noticeable.  It
never grades the reader, never counts reading time, and never starts a model
call on its own: the only thing that can appear without a click is a highlight
on a task that already exists in the ledger (see `hud_state`).

It draws in its own process and never takes keyboard focus, so reading on
underneath is untouched: a click is delivered to the panel, the typing goes to
whatever window had it.  The window is placed by XWayland geometry (the popups
already run that way); `config/hypr-v2.lua` carries the matching window rule
that keeps it floating, unfocused and borderless.

    panel button        what happens
    Цель                the existing goal window (Alt+I)
    Слова               four cues from the selection (Alt+W)
    Пример              one concrete example (Alt+G)
    Заметка             note about a misreading (Alt+E)
    Практика            the prepared task, or the task menu; right click
                        clears the highlight
    Ещё                 the mode menu: Feynman, hypothesis, question

`SIGUSR1` toggles visibility, `SIGTERM`/`SIGINT` stop the panel.  Nothing is
written to the ledger by this process: it asks over the socket and the daemon
decides (see `desktop.hud_request`).
"""
from __future__ import annotations

import argparse
import os
import signal
import subprocess
import sys
from pathlib import Path

from .hud_ipc import resolve_socket_path, send as hud_send

try:
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("Gdk", "3.0")
    from gi.repository import Gdk, GLib, Gtk
except (ImportError, ValueError) as exc:  # pragma: no cover - desktop dependency
    raise SystemExit(f"GTK3/PyGObject is required for the side panel: {exc}") from exc


STATE_DIR = Path(os.environ.get("COGNITIVE_STATE_DIR", "~/.local/state/cognitive-popups")).expanduser()
PID_PATH = STATE_DIR / "hud.pid"

#: Fixed positions; captions show only shortcuts that perform the button's action.
#: Unmapped actions use their names instead of an invented key.
BUTTONS = (
    ("goal", "◎", "Alt+I", "Сейчас хочу: цель сессии (Alt+I)"),
    ("four_words", "▦", "Alt+W", "Четыре слова из выделенного текста (Alt+W)"),
    ("example", "◇", "Alt+G", "Показать на примере (Alt+G)"),
    ("note", "✎", "Alt+E", "Своя заметка к тексту (Alt+E)"),
    ("practice", "▶", "Alt+T", "Меню задач (Alt+T)"),
    ("keys", "⌘", "Alt+K", "Справка: что делает каждая клавиша (Alt+K)"),
    ("menu", "⋯", "Ещё", "Остальные режимы: Фейнман, гипотеза, другой ракурс (Alt+R), вопрос"),
    ("background", "⚙", "Фон", "Готовить практику в фоне"),
)

#: The one setting the panel itself can turn on. Shown as a button because a
#: feature that starts model calls has to be visible, not buried in a file.
BACKGROUND_SETTING = "prepare_in_background"

SEGMENT_HEIGHT = 4
SEGMENT_SPACING = 2

PANEL_WIDTH = 76
PANEL_MARGIN = 12
TOP_MARGIN = int(os.environ.get("COGNITIVE_HUD_OFFSET") or 96)
REFRESH_MS = 3000
#: After an action the daemon (or the tasks process) needs a moment before the
#: next state read sees the result; one delayed read is enough.
SETTLE_MS = 900

CSS = b"""
/* The plate lives on the inner box, not on the window: this window is
   app-paintable (so the rounded corners can be genuinely transparent), and GTK
does not draw an app-paintable window's own background. A no-window container
still paints its CSS background, which is where the plate has to go. */
.hud-panel .hud-plate {
    background-color: alpha(@theme_bg_color, 0.94);
    border: 1px solid alpha(@theme_fg_color, 0.18);
    border-radius: 10px;
}
.hud-panel button.hud-button {
    min-width: 54px;
    min-height: 36px;
    padding: 2px 3px;
}
.hud-panel .hud-icon {
    font-size: 17px;
}
.hud-panel .hud-shortcut {
    font-size: 9px;
}
.hud-panel button.hud-ready {
    background-image: none;
    background-color: alpha(@theme_selected_bg_color, 0.92);
    color: @theme_selected_fg_color;
    border-color: @theme_selected_bg_color;
}
.hud-panel button.hud-on {
    background-image: none;
    background-color: alpha(@theme_selected_bg_color, 0.45);
}
.hud-panel .hud-segment {
    min-height: 4px;
    border-radius: 2px;
    background-color: alpha(@theme_fg_color, 0.18);
}
.hud-panel .hud-segment.hud-seg-on {
    background-image: none;
    background-color: @theme_selected_bg_color;
}
.hud-panel .hud-segment.hud-seg-working {
    background-image: none;
    background-color: alpha(@theme_selected_bg_color, 0.5);
}
.hud-panel .hud-status {
    font-size: 9px;
}
"""


class ScaleBar(Gtk.Box):
    """The panel's scale: one segment per finished step of the preparation.

    Plain widgets rather than custom drawing, deliberately: a Cairo draw handler
    needs `python-cairo`, which this install does not have, and GTK then simply
    stops calling the handler — the scale would be silently blank. Widgets with
    CSS backgrounds need nothing beyond what the popups already use.

    Segments are stages, not percentages. A partly-filled bar would claim a
    precision about "how far along" this is that the work does not have.
    """

    def __init__(self, stage_count: int = 4):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=SEGMENT_SPACING)
        self.stage_count = max(1, int(stage_count))
        self.completed = 0
        self.working = False
        self.segments: list[Gtk.Box] = []
        for _index in range(self.stage_count):
            segment = Gtk.Box()
            segment.set_size_request(-1, SEGMENT_HEIGHT)
            context = segment.get_style_context()
            context.add_class("hud-segment")
            self.pack_start(segment, False, False, 0)
            self.segments.append(segment)

    def update(self, completed: int, working: bool) -> None:
        completed = max(0, min(self.stage_count, int(completed or 0)))
        working = bool(working)
        if (completed, working) == (self.completed, self.working):
            return
        self.completed, self.working = completed, working
        self._paint()

    def _paint(self) -> None:
        for index, segment in enumerate(self.segments):
            context = segment.get_style_context()
            context.remove_class("hud-seg-on")
            context.remove_class("hud-seg-working")
            if index < self.completed:
                context.add_class("hud-seg-on")
            elif index == self.completed and self.working:
                # The step being worked on right now, dimmer: not finished yet.
                context.add_class("hud-seg-working")


def monitor_geometry() -> Gdk.Rectangle | None:
    """Geometry of the monitor the pointer is on, in global coordinates.

    'Which monitor' is asked of the compositor rather than guessed: the panel
    belongs where the reader is looking, and `hyprctl` is already how the rest of
    the app finds the cursor.
    """
    display = Gdk.Display.get_default()
    if display is None:
        return None
    x = y = 0
    try:
        result = subprocess.run(
            ["hyprctl", "cursorpos"], capture_output=True, text=True, timeout=1, check=True
        )
        x_text, y_text = result.stdout.strip().replace(" ", "").split(",", 1)
        x, y = int(x_text), int(y_text)
    except (OSError, subprocess.SubprocessError, ValueError):
        pass
    monitor = None
    try:
        monitor = display.get_monitor_at_point(x, y)
    except Exception:  # noqa: BLE001 - placement must never stop the panel
        monitor = None
    if monitor is None:
        monitor = display.get_primary_monitor()
    if monitor is None:
        return None
    return monitor.get_geometry()


def _install_css(widget) -> None:
    screen = Gdk.Screen.get_default()
    if screen is None:
        return
    provider = Gtk.CssProvider()
    try:
        provider.load_from_data(CSS)
    except Exception:  # noqa: BLE001 - a theme without the named colours still runs
        return
    Gtk.StyleContext.add_provider_for_screen(screen, provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)


def _enable_transparency(window) -> None:
    """Rounded corners need an alpha visual; without a compositor the panel just
    stays opaque, which is plain rather than broken."""
    try:
        screen = window.get_screen()
        visual = screen.get_rgba_visual() if screen is not None else None
        if visual is not None and screen.is_composited():
            window.set_visual(visual)
        window.set_app_paintable(True)
    except Exception:  # noqa: BLE001
        pass


class HudPanel:
    """The panel window and the state read from the daemon."""

    def __init__(self, socket_path):
        self.socket_path = socket_path
        self.state: dict = {}
        self.reachable = False
        #: Dismissals made in this process, so a read that raced the write cannot
        #: light the same task up again.
        self.dismissed_locally: set[str] = set()
        self.visible = True
        self.window = Gtk.Window()
        self.buttons: dict[str, Gtk.Button] = {}
        self.key_labels: dict[str, Gtk.Label] = {}
        self.status = Gtk.Label(label="")
        self.scale = ScaleBar()
        self._build()

    # ── construction ─────────────────────────────────────────────────────────

    def _build(self) -> None:
        window = self.window
        # The X11 class Hyprland matches in config/hypr-v2.lua. `set_wmclass` is
        # deprecated for the multi-backend era, but it is still the only way to
        # name the X11 class, and this window is deliberately an X11 one.
        window.set_wmclass("cognitive-hud", "cognitive-hud")
        window.set_decorated(False)
        window.set_resizable(False)
        window.set_skip_taskbar_hint(True)
        window.set_skip_pager_hint(True)
        window.set_accept_focus(False)
        window.set_focus_on_map(False)
        window.set_keep_above(True)
        try:
            window.set_type_hint(Gdk.WindowTypeHint.DOCK)
        except (AttributeError, TypeError):
            pass
        window.set_default_size(PANEL_WIDTH, -1)
        window.get_style_context().add_class("hud-panel")
        _enable_transparency(window)
        _install_css(window)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        box.set_border_width(6)
        box.get_style_context().add_class("hud-plate")
        window.add(box)
        for action, icon, caption, tooltip in BUTTONS:
            if action == "practice":
                # The scale sits directly above the button it is about.
                box.pack_start(self.scale, False, False, 0)
            button = Gtk.Button()
            face = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            glyph = Gtk.Label(label=icon)
            glyph.get_style_context().add_class("hud-icon")
            key_label = Gtk.Label(label=caption)
            key_label.get_style_context().add_class("hud-shortcut")
            face.pack_start(glyph, False, False, 0)
            face.pack_start(key_label, False, False, 0)
            button.add(face)
            button.set_relief(Gtk.ReliefStyle.NONE)
            button.set_tooltip_text(tooltip)
            button.get_style_context().add_class("hud-button")
            button.connect("clicked", self._on_clicked, action)
            button.connect("button-press-event", self._on_press, action)
            box.pack_start(button, False, False, 0)
            self.buttons[action] = button
            self.key_labels[action] = key_label

        self.status.get_style_context().add_class("hud-status")
        self.status.set_line_wrap(True)
        self.status.set_max_width_chars(12)
        self.status.set_no_show_all(True)
        box.pack_start(self.status, False, False, 0)

        window.connect("map-event", self._on_map)
        window.connect("destroy", lambda *_: Gtk.main_quit())
        window.show_all()
        self.status.set_visible(False)

    # ── placement ────────────────────────────────────────────────────────────

    def _on_map(self, *_args):
        self._place()
        return False

    def _place(self) -> None:
        try:
            geometry = monitor_geometry()
            if geometry is None:
                return
            x = geometry.x + geometry.width - PANEL_WIDTH - PANEL_MARGIN
            y = geometry.y + TOP_MARGIN
            self.window.move(x, y)
        except Exception:  # noqa: BLE001 - a placed-by-the-compositor window is fine
            pass

    # ── interaction ──────────────────────────────────────────────────────────

    def _on_clicked(self, _widget, action: str):
        if action == "practice":
            self._practice(attempt=True)
        elif action == "background":
            self._toggle_background()
        else:
            self._dispatch(action)

    def _on_press(self, _widget, event, action: str) -> bool:
        """Right click on «Практика» clears the highlight without opening anything."""
        if action == "practice" and event.button == 3:
            self._practice(attempt=False)
            return True
        return False

    def _opportunity(self) -> dict | None:
        practice = self.state.get("practice")
        if not isinstance(practice, dict):
            return None
        task_id = str(practice.get("task_id") or "")
        if not task_id or task_id in self.dismissed_locally:
            return None
        return practice

    def _toggle_background(self) -> None:
        """Turn background preparation on or off. Nothing else changes."""
        current = bool((self.state.get("settings") or {}).get(BACKGROUND_SETTING))
        response = hud_send(
            {"command": "settings", "key": BACKGROUND_SETTING, "value": not current},
            path=self.socket_path,
        )
        if not response.get("ok"):
            self._set_status("нет связи с сервисом")
            return
        self._set_status("")
        self._refresh_once()

    def _practice(self, *, attempt: bool) -> None:
        opportunity = self._opportunity()
        if opportunity is None:
            # Nothing prepared: the button keeps its plain meaning and opens the
            # task menu, exactly as Alt+T does.
            if attempt:
                self._dispatch("task_menu")
            return
        if attempt:
            self._dispatch("task_practice", task_id=opportunity["task_id"])
        else:
            self._dismiss(opportunity["task_id"])

    def _dismiss(self, task_id: str) -> None:
        self.dismissed_locally.add(task_id)
        self._apply_state()
        response = hud_send({"command": "dismiss", "task_id": task_id}, path=self.socket_path)
        if response.get("ok"):
            self._set_status("")
        else:
            self._set_status("подсветка не сохранилась")
        self._refresh_once()

    def _dispatch(self, action: str, task_id: str = "") -> None:
        response = hud_send(
            {"command": "action", "action": action, "task_id": task_id},
            path=self.socket_path,
        )
        if not response.get("ok"):
            self._set_status("нет связи с сервисом")
            return
        self._set_status("")
        GLib.timeout_add(SETTLE_MS, self._refresh_once)

    # ── state ────────────────────────────────────────────────────────────────

    def refresh(self) -> bool:
        """Poll from the GTK loop. The daemon answers on its socket thread, so a
        popup left open on its side does not stall this read."""
        self._refresh_once()
        return True

    def _refresh_once(self) -> bool:
        response = hud_send({"command": "state"}, path=self.socket_path, timeout=0.8)
        if response.get("ok"):
            self.state = response
            self.reachable = True
        else:
            self.state = {}
            self.reachable = False
        self._apply_state()
        return False

    def _apply_state(self) -> None:
        opportunity = self._opportunity()
        practice = self.buttons.get("practice")
        if practice is not None:
            context = practice.get_style_context()
            if opportunity is not None:
                context.add_class("hud-ready")
            else:
                context.remove_class("hud-ready")
            self.key_labels["practice"].set_text("Задача" if opportunity is not None else "Alt+T")
            practice.set_tooltip_text(
                self._practice_tooltip(opportunity, self.state.get("preparation")))

        self._apply_scale(opportunity)
        self._apply_background()

        goal = self.state.get("goal")
        goal_button = self.buttons.get("goal")
        if goal_button is not None:
            text = str(goal.get("text") or "") if isinstance(goal, dict) else ""
            goal_button.set_tooltip_text(
                f"Сейчас хочу: {text} (Alt+I)" if text else "Сейчас хочу: цели пока нет (Alt+I)"
            )

        if not self.reachable:
            self._set_status("нет связи с сервисом")
        elif self.state.get("busy"):
            self._set_status("занят")
        else:
            self._set_status("")

    def _apply_scale(self, opportunity: dict | None) -> None:
        """The scale appears only when there is a preparation to show.

        It measures the preparation, so it has nothing to say about a task the
        reader asked for by hand: a full scale there would be a claim about work
        that was never done. What the segments mean is spelled out in the
        practice button's tooltip, which is the thing the reader already reads.
        """
        preparation = self.state.get("preparation")
        if not isinstance(preparation, dict):
            self.scale.set_visible(False)
            return
        self.scale.update(preparation.get("completed_stage", 0), preparation.get("working"))
        self.scale.set_visible(True)

    def _apply_background(self) -> None:
        button = self.buttons.get("background")
        if button is None:
            return
        enabled = bool((self.state.get("settings") or {}).get(BACKGROUND_SETTING))
        context = button.get_style_context()
        if enabled:
            context.add_class("hud-on")
        else:
            context.remove_class("hud-on")
        state_text = "включена" if enabled else "выключена"
        button.set_tooltip_text(
            f"Подготовка практики в фоне: {state_text}.\n"
            "Когда включена, после сохранённого материала готовится одна задача "
            "на случай, если захочется попробовать. Ничего не открывается само."
        )

    def _practice_tooltip(self, opportunity: dict | None, preparation: dict | None = None) -> str:
        lines: list[str] = []
        if opportunity is None:
            lines.append("Задача по тому, что читаешь (Alt+T)")
            lines.append("Пока ничего не готово — откроется меню задач.")
        else:
            day = str(opportunity.get("day") or "")
            where = "эта сессия" if opportunity.get("from_current_session") else "прошлая сессия"
            when = f"готова {day}, {where}" if day else f"готова ({where})"
            lines.append(f"Есть что попробовать — {when}.")
            lines.append(str(opportunity.get("summary") or ""))
            lines.append("Нажми, чтобы попробовать именно эту задачу. Правая кнопка — убрать подсветку.")
            lines.append("Alt+Y открывает последнюю сгенерированную задачу — она может быть другой.")
        lines.append("Alt+Shift+T — вручную выбрать старый материал для новой задачи.")
        if isinstance(preparation, dict):
            label = str(preparation.get("label") or preparation.get("status") or "")
            stage = int(preparation.get("completed_stage") or 0)
            total = int(preparation.get("stage_count") or 0)
            if label:
                lines.append(f"Подготовка: {label} ({stage} из {total}).")
            target = str(preparation.get("target") or "")
            if target:
                lines.append(target)
            lines.append("Отрезок — завершённый шаг подготовки, а не проценты понимания.")
        return "\n".join(line for line in lines if line)

    def _set_status(self, text: str) -> None:
        self.status.set_text(text)
        self.status.set_visible(bool(text))

    # ── window control ───────────────────────────────────────────────────────

    def toggle(self) -> bool:
        self.visible = not self.visible
        if self.visible:
            self.window.show_all()
            self.status.set_visible(bool(self.status.get_text()))
            self._place()
            self._refresh_once()
        else:
            self.window.hide()
        return True

    def quit(self) -> bool:
        Gtk.main_quit()
        return False


def _another_panel_runs() -> bool:
    """A second panel would only overlap the first: one strip per session.

    The pid file alone proves nothing — a crashed run leaves it behind — so the
    process is asked whether it is still there.
    """
    try:
        pid = int(PID_PATH.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return False
    if pid == os.getpid():
        return False
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Cognitive Popups side panel")
    parser.add_argument("--socket", default=None, help="daemon socket path (default: the state directory)")
    parser.add_argument("--hidden", action="store_true", help="start hidden; SIGUSR1 shows the panel")
    args = parser.parse_args(argv)

    # The panel is placed by its own geometry, which Wayland does not allow; the
    # popups and the daemon already run on XWayland for the same reason.
    os.environ.setdefault("GDK_BACKEND", "x11")

    if _another_panel_runs():
        print("cognitive-hud is already running", file=sys.stderr)
        return 0

    panel = HudPanel(resolve_socket_path(args.socket))
    if args.hidden:
        panel.visible = False
        panel.window.hide()

    STATE_DIR.mkdir(parents=True, exist_ok=True)
    PID_PATH.write_text(str(os.getpid()), encoding="utf-8")
    GLib.timeout_add(REFRESH_MS, panel.refresh)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, panel.toggle)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, panel.quit)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGINT, panel.quit)
    panel._refresh_once()
    try:
        Gtk.main()
    finally:
        try:
            PID_PATH.unlink()
        except OSError:
            pass
    return 0


if __name__ == "__main__":  # pragma: no cover - GUI entry point
    sys.exit(main())
