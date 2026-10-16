"""GTK-only passive timer. The callback launches work, never performs it."""
from __future__ import annotations

import math
import time

from . import theme

#: How to end the step, always under the clock (note 126: «как остановиться?»).
STOP_HINT = "Alt+I — закончить шаг"


class FocusTimer:
    WIDTH, HEIGHT = 200, 148

    def __init__(self, state, on_expire, on_cancel, *, clock=time.time):
        import gi
        gi.require_version("Gtk", "3.0")
        gi.require_version("GdkPixbuf", "2.0")
        from gi.repository import Gtk, GLib, Gdk, Pango, GdkPixbuf
        self.Gtk, self.GLib, self.Gdk, self.clock = Gtk, GLib, Gdk, clock
        self.GdkPixbuf = GdkPixbuf
        self.Pango = Pango
        self.expanded = False
        self.goal_height = 24
        self.state, self.on_expire, self.on_cancel = state, on_expire, on_cancel
        self.goal = state.get("goal", "")
        self.far_edge = state.get("far_edge", "")
        self.expired, self.closed, self.source = False, False, None
        self.expiration_source = None
        self.tone = "dark"
        self.label_styles = []
        self.window = Gtk.Window(title="15 → 1")
        from .sand_surface import configure, enabled
        self.sand_mode = enabled()
        configure(self.window, Gtk, Gdk)
        if self.far_edge:
            self.window.set_tooltip_text("Далёкое ребро: " + self.far_edge)
        self.window.set_wmclass("cognitive-focus-timer", "cognitive-focus-timer")
        self.window.set_accept_focus(False)
        self.window.set_focus_on_map(False)
        self.window.set_decorated(False)
        self.window.set_resizable(False)
        self.window.set_keep_above(True)
        self.window.set_skip_taskbar_hint(True)
        self.window.set_skip_pager_hint(True)
        self.window.set_app_paintable(True)
        visual = self.window.get_screen().get_rgba_visual()
        if visual is not None:
            self.window.set_visual(visual)
        transparent = Gtk.CssProvider()
        transparent.load_from_data(b"window { background-color: transparent; box-shadow: none; }")
        self.window.get_style_context().add_provider(transparent, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        self.window.set_type_hint(Gdk.WindowTypeHint.NOTIFICATION)
        self.window.set_default_size(self.WIDTH, self.HEIGHT)
        self.width, self.height = self.WIDTH, self.HEIGHT
        self.area = Gtk.Overlay()
        self.area.set_size_request(self.width, self.height)
        self.canvas = Gtk.Image()
        self.area.add(self.canvas)
        self.labels = Gtk.Fixed()
        self.area.add_overlay(self.labels)
        self.area.set_overlay_pass_through(self.labels, True)
        self.countdown = self._styled(Gtk, Pango, "", theme.TITLE, "ink")
        self.goal_label = self._styled(Gtk, Pango, self.goal, theme.TEXT, "ink_muted")
        self.hint_label = self._styled(Gtk, Pango, STOP_HINT, theme.SMALL, "ink_muted")
        self.labels.put(self.countdown, 10, 37)
        self.labels.put(self.goal_label, 10, 96)
        self.labels.put(self.hint_label, 10, 124)
        if self.far_edge:
            self.edge_label = self._styled(Gtk, Pango, "☆", theme.SMALL, "accent")
            self.edge_label.set_tooltip_text(self.far_edge)
            self.labels.put(self.edge_label, self.WIDTH - 26, 8)
        self.window.add(self.area)
        self.window.connect("delete-event", self._cancel)
        self.window.connect("map", self._mapped)
        self.window.connect("unmap", self._unmapped)
        # Hover opens the whole step text; the corner shows only its start.
        self.window.add_events(Gdk.EventMask.ENTER_NOTIFY_MASK | Gdk.EventMask.LEAVE_NOTIFY_MASK)
        self.window.connect("enter-notify-event", lambda _w, _e: self._expand(True))
        self.window.connect("leave-notify-event", self._leave)
        self._place()
        self._apply_tone(self._page_tone())
        self._layout_labels()
        self._refresh()
        self.window.show_all()
        self._schedule_expiration()

    def _styled(self, Gtk, Pango, text, size, token):
        label = self._label(Gtk, Pango, text, size, theme.palette(self.tone)[token])
        self.label_styles.append((label, size, token))
        return label

    def _page_tone(self):
        """Light plate over a light page, dark otherwise; never fails the timer."""
        try:
            from .sand_surface import LIGHT, palette
            x, y = getattr(self, "position", (0, 0))
            return "light" if palette(self.Gdk, x, y, self.width, self.height) is LIGHT else "dark"
        except Exception:  # noqa: BLE001 - tone is a nicety
            return "dark"

    def _apply_tone(self, tone):
        if tone == self.tone:
            return
        self.tone = tone
        for label, size, token in self.label_styles:
            provider = self.Gtk.CssProvider()
            provider.load_from_data(self._label_css(size, theme.palette(tone)[token]))
            # Above the label's own provider, so the new tone wins.
            label.get_style_context().add_provider(provider, self.Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 1)

    @staticmethod
    def _label_css(size, color):
        return (f"label {{ background-color: transparent; color: {color}; "
                f"{theme.css_font(size)} font-feature-settings: 'tnum'; }}").encode("utf-8")

    @staticmethod
    def _label(Gtk, Pango, text, size, color):
        label = Gtk.Label()
        label.set_text(text)
        label.set_use_markup(False)
        label.set_single_line_mode(True)
        label.set_line_wrap(False)
        label.set_ellipsize(Pango.EllipsizeMode.END)
        # The exact text must not increase the window's natural width.
        label.set_max_width_chars(1)
        label.set_xalign(.5)
        provider = Gtk.CssProvider()
        provider.load_from_data(FocusTimer._label_css(size, color))
        label.get_style_context().add_provider(provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        return label

    def _layout_labels(self):
        base = self.HEIGHT if self.expanded else self.height
        cy = min(51, base / 2)
        self.countdown.set_size_request(max(1, self.width - 20), 28)
        self.goal_label.set_size_request(max(1, self.width - 20), self.goal_height)
        self.labels.move(self.countdown, 10, max(0, int(cy - 14)))
        self.labels.move(self.goal_label, 10, max(0, base - self.goal_height - 28))
        hint = getattr(self, "hint_label", None)
        if hint is not None:
            hint.set_size_request(max(1, self.width - 20), 18)
            self.labels.move(hint, 10, max(0, base - 24))

    def _leave(self, _widget, event):
        # Moving onto a child widget is not leaving the clock.
        if event.detail != self.Gdk.NotifyType.INFERIOR:
            self._expand(False)
        return False

    def _expand(self, on: bool):
        """Grow down to show the whole step text, or fold back to one line."""
        if self.closed or on == self.expanded:
            return False
        label, Pango = self.goal_label, self.Pango
        if on:
            label.set_single_line_mode(False)
            label.set_line_wrap(True)
            label.set_line_wrap_mode(Pango.WrapMode.WORD_CHAR)
            label.set_ellipsize(Pango.EllipsizeMode.NONE)
            layout = label.create_pango_layout(self.goal)
            layout.set_width(max(1, self.width - 20) * Pango.SCALE)
            layout.set_wrap(Pango.WrapMode.WORD_CHAR)
            self.goal_height = max(24, layout.get_pixel_size()[1] + 4)
            self.height = self.HEIGHT - 24 + self.goal_height + 6
        else:
            label.set_line_wrap(False)
            label.set_single_line_mode(True)
            label.set_ellipsize(Pango.EllipsizeMode.END)
            self.goal_height = 24
            self.height = self.HEIGHT
        self.expanded = on
        self.area.set_size_request(self.width, self.height)
        self._layout_labels()
        self.window.resize(self.width, self.height)
        self._refresh()
        return False

    @staticmethod
    def _peripheral_position(workarea, width, height):
        margin = 16
        x = max(workarea.x, workarea.x + workarea.width - width - margin)
        y = max(workarea.y, workarea.y + margin)
        y = min(y, max(workarea.y, workarea.y + workarea.height - height))
        return x, y

    def _place(self):
        display = self.Gdk.Display.get_default()
        if display is None:
            return
        seat = display.get_default_seat()
        pointer = seat.get_pointer() if seat else None
        monitor = None
        if pointer:
            _, x, y = pointer.get_position()
            monitor = display.get_monitor_at_point(x, y)
        if monitor is None:
            monitor = display.get_primary_monitor()
        if monitor is None and display.get_n_monitors() > 0:
            monitor = display.get_monitor(0)
        if monitor is None:
            return
        workarea = monitor.get_workarea()
        # A tiny workarea must not force the timer outside its bounds.
        width, height = min(self.WIDTH, workarea.width), min(self.HEIGHT, workarea.height)
        self.width, self.height = width, height
        self.area.set_size_request(width, height)
        self._layout_labels()
        self.window.resize(width, height)
        self.position = self._peripheral_position(workarea, width, height)
        self.window.move(*self.position)

    def _mapped(self, *_):
        if not self.closed:
            self._place()
            self._refresh()
            if not self.expired and self.source is None:
                self.source = self.GLib.timeout_add_seconds(1, self._tick)

    def _unmapped(self, *_):
        if self.source is not None:
            self.GLib.source_remove(self.source)
            self.source = None

    def _schedule_expiration(self):
        if not self.closed and not self.expired:
            delay = max(1, math.ceil((self.state["deadline"] - self.clock()) * 1000))
            self.expiration_source = self.GLib.timeout_add(delay, self._expire)

    def _expire(self):
        self.expiration_source = None
        if self.closed or self.expired:
            return False
        # Wall-clock adjustments can make a scheduled callback arrive early.
        if self.clock() < self.state["deadline"]:
            self._schedule_expiration()
            return False
        self.expired = True
        self._unmapped()
        if self.window.get_mapped():
            self._refresh()
        self.on_expire(self.state["id"])
        return False

    def _cancel(self, *_):
        self.on_cancel(self.state["id"])
        self.close()
        return True

    def _tick(self):
        if self.closed or self.expired or not self.window.get_mapped():
            self.source = None
            return False
        self._refresh()
        return True

    @staticmethod
    def _background_svg(width, height, remaining, duration, expired=False, tone="dark"):
        progress = min(1, max(0, remaining / max(1, duration)))
        salience = max(0, 1 - remaining / 60) if not expired else 0
        cx, cy = width / 2, min(51, height / 2)
        radius = max(1, min(39, width / 2 - 12, height / 2 - 12))
        circumference = 2 * math.pi * radius
        stroke = 2 + .5 * salience
        colours = theme.palette(tone)
        # The ring brightens on dark paper and darkens on light as the end nears.
        shade = round(255 * ((.70 + .18 * salience) if tone != "light" else (.45 - .18 * salience)))
        color = f"#{shade:02x}{shade:02x}{shade:02x}"
        ring = ""
        if not expired and progress > 0:
            ring = (
                f'<circle cx="{cx}" cy="{cy}" r="{radius}" fill="none" '
                f'stroke="{color}" stroke-width="{stroke}" '
                f'stroke-dasharray="{progress * circumference:.4f} {circumference:.4f}" '
                f'transform="rotate(-90 {cx} {cy})"/>')
        # No text enters SVG: GTK/Pango owns the two persistent plain-text labels.
        return (
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
            f'viewBox="0 0 {width} {height}">'
            f'<rect width="{width}" height="{height}" rx="{min(24, width / 2, height / 2)}" '
            f'fill="{colours["paper"]}" fill-opacity="0.94"/>'
            f'<circle cx="{cx}" cy="{cy}" r="{radius}" fill="none" '
            f'stroke="{colours["line"]}" stroke-width="{stroke}"/>{ring}</svg>')

    @staticmethod
    def _sand_svg(width, height, remaining, duration, expired=False, tone="dark"):
        """A static granular hourglass, refreshed once per second, no opaque plate."""
        colours = theme.palette(tone)
        progress = min(1, max(0, remaining / max(1,duration)))
        cx, cy = min(width-20,30), min(51,height/2)
        parts = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">']
        parts.append(f'<path d="M{cx-12} {cy-22} L{cx+12} {cy-22} L{cx} {cy} L{cx+12} {cy+22} L{cx-12} {cy+22} L{cx} {cy} Z" fill="none" stroke="{colours["accent"]}" opacity=".5"/>')
        for i in range(100):
            upper = i < round(progress*100)
            n = i if upper else i-round(progress*100)
            layer = n//10
            x = cx + ((n%10)-4.5)*2*(1-layer/12)
            y = cy-19+layer*1.6 if upper else cy+19-layer*1.6
            parts.append(f'<circle cx="{x:.2f}" cy="{y:.2f}" r=".65" fill="{colours["accent"]}" data-chamber="{"upper" if upper else "lower"}"/>')
        if not expired and progress>0:
            parts.append(f'<circle cx="{cx}" cy="{cy+4}" r=".7" fill="{colours["ink"]}"/>')
        return ''.join(parts)+'</svg>'

    def _refresh(self):
        remaining = max(0, math.ceil(self.state["deadline"] - self.clock()))
        duration = self.state["deadline"] - self.state.get("started_at", self.state["deadline"] - 900)
        renderer = self._sand_svg if getattr(self,"sand_mode",False) else self._background_svg
        svg = renderer(self.width, self.height, remaining, duration, self.expired, getattr(self, "tone", "dark"))
        # Use the same system SVG loader as the orbital popup; no Python Cairo.
        loader = self.GdkPixbuf.PixbufLoader.new_with_type("svg")
        loader.write(self.GLib.Bytes.new(svg.encode("utf-8")).get_data())
        loader.close()
        self.canvas.set_from_pixbuf(loader.get_pixbuf())
        text = "Время" if self.expired else f"{remaining // 60:02}:{remaining % 60:02}"
        if self.countdown.get_text() != text:
            self.countdown.set_text(text)

    def peek(self, seconds: float = 1.3):
        """Show the clock next to the pointer for a moment, then return it to the corner.

        Called at the reader's own transitions (each hotkey): context is injected
        when attention moves anyway, instead of standing still and fading from view.
        """
        if self.closed or not self.window.get_mapped():
            return
        display = self.Gdk.Display.get_default()
        seat = display.get_default_seat() if display else None
        pointer = seat.get_pointer() if seat else None
        if pointer is None:
            return
        _, x, y = pointer.get_position()
        self.window.move(int(x) + 24, max(0, int(y) - self.height - 12))
        if getattr(self, "peek_source", None):
            self.GLib.source_remove(self.peek_source)
        self.peek_source = self.GLib.timeout_add(int(seconds * 1000), self._end_peek)

    def _end_peek(self):
        self.peek_source = None
        if not self.closed:
            self._place()
        return False

    def close(self):
        if self.closed:
            return
        if getattr(self, "peek_source", None):
            self.GLib.source_remove(self.peek_source)
            self.peek_source = None
        self.closed = True
        self._unmapped()
        if self.expiration_source is not None:
            self.GLib.source_remove(self.expiration_source)
            self.expiration_source = None
        self.window.destroy()
