"""Cutscene A «на бумагу»: the four words leave for the corners, the turn passes to the hand.

The reader writes the four words in the corners of a squared sheet and draws
the links by hand (photos, F §5). After the words are revealed the screen
does the same once: the words travel from the cluster to the four corners,
the centre empties and holds one line, «связи — твои · Ctrl+Alt+P, когда
будет гипотеза», then everything fades. A handover, not a reward.

:func:`frame` is pure (no GTK) and fully determines every frame; the thin
renderer :func:`play` draws it in a transparent, input-transparent window
that never takes focus and closes itself.
"""
from __future__ import annotations

import time
from dataclasses import dataclass

from . import motion, theme

CENTRE_LINE = "связи — твои · Ctrl+Alt+P, когда будет гипотеза"
#: 600 ms of ease-out travel, then a 300 ms fade: t = 1 is 900 ms.
TRAVEL_S, FADE_S = 0.6, 0.3
DURATION_S = TRAVEL_S + FADE_S
#: The words have arrived and the line is fully shown; the fade starts here.
SETTLED_T = TRAVEL_S / DURATION_S
#: The centre line fades in during the second half of the travel.
LINE_IN = (0.3 / DURATION_S, SETTLED_T)
#: Under reduced motion the settled composition stands still this long, then
#: the window closes without any fade.
REDUCED_HOLD_S = 1.5
#: Default window around the cluster centre, clamped to the monitor.
BOX = (560, 360)

MARGIN = 20
GAP = 8
WORD_SIZE = theme.LARGE
LINE_SIZE = theme.TEXT
PAD_X, PAD_Y = 10, 6
#: Average glyph advance as a share of the font size (Noto Sans, Cyrillic).
ADVANCE = 0.58


@dataclass(frozen=True)
class Shape:
    """One thing to draw: a word card or the centre line, in box pixels."""
    kind: str        # "word" | "line"
    text: str
    x: float
    y: float
    w: float
    h: float
    alpha: float
    size: int
    ink: str         # theme token of the text
    plate: str = ""  # theme token of the card under it ("" = none)

    def rect(self) -> tuple[float, float, float, float]:
        return self.x, self.y, self.w, self.h


def ease_out(u: float) -> float:
    u = min(1.0, max(0.0, u))
    return 1.0 - (1.0 - u) ** 3


def _fit(text: str, size: int, limit: float) -> tuple[str, float]:
    """The text (ellipsised if needed) and its estimated card width."""
    text = " ".join(str(text).split())
    width = len(text) * size * ADVANCE + 2 * PAD_X
    if width <= limit:
        return text, width
    keep = max(1, int((limit - 2 * PAD_X) / (size * ADVANCE)) - 1)
    text = text[:keep].rstrip() + "…"
    return text, min(limit, len(text) * size * ADVANCE + 2 * PAD_X)


def _cards(words, box):
    w, h = box
    limit = max(40.0, (w - 3 * MARGIN) / 2)
    height = WORD_SIZE * 1.4 + 2 * PAD_Y
    return [(*_fit(word, WORD_SIZE, limit), height) for word in list(words)[:4]]


def _corners(cards, box):
    """Top-left, top-right, bottom-left, bottom-right: the sheet's corners."""
    w, h = box
    out = []
    for i, (_, cw, ch) in enumerate(cards):
        right, bottom = i % 2 == 1, i >= 2
        out.append((w - MARGIN - cw if right else MARGIN, h - MARGIN - ch if bottom else MARGIN))
    return out


def _cluster(cards, box):
    """The words as the reveal left them: a tight 2×2 around the centre."""
    w, h = box
    cx, cy = w / 2, h / 2
    out = []
    for i, (_, cw, ch) in enumerate(cards):
        right, bottom = i % 2 == 1, i >= 2
        out.append((cx + GAP / 2 if right else cx - GAP / 2 - cw,
                    cy + GAP / 2 if bottom else cy - GAP / 2 - ch))
    return out


def frame(t: float, words: list[str], box: tuple[int, int] = BOX) -> list[Shape]:
    """Every shape at time ``t`` ∈ [0, 1] (0 = cluster, 1 = corners, faded out).

    Deterministic: the same input always gives the same shapes. Words keep
    their card size while they move, so only position and opacity change.
    """
    t = min(1.0, max(0.0, float(t)))
    cards = _cards(words, box)
    start, end = _cluster(cards, box), _corners(cards, box)
    travel = ease_out(t / SETTLED_T)
    fade = 1.0 - min(1.0, max(0.0, (t - SETTLED_T) / (1.0 - SETTLED_T)))
    shapes = []
    for (text, cw, ch), (x0, y0), (x1, y1) in zip(cards, start, end):
        shapes.append(Shape("word", text, round(x0 + (x1 - x0) * travel, 2), round(y0 + (y1 - y0) * travel, 2),
                            round(cw, 2), round(ch, 2), round(fade, 3), WORD_SIZE, "ink", "paper_raised"))
    w, h = box
    line, lw = _fit(CENTRE_LINE, LINE_SIZE, w - 2 * MARGIN)
    lh = LINE_SIZE * 1.4 + 2 * PAD_Y
    shown = min(1.0, max(0.0, (t - LINE_IN[0]) / (LINE_IN[1] - LINE_IN[0])))
    shapes.append(Shape("line", line, round((w - lw) / 2, 2), round((h - lh) / 2, 2), round(lw, 2), round(lh, 2),
                        round(shown * fade, 3), LINE_SIZE, "ink", "paper_raised"))
    return shapes


def playhead(elapsed: float, *, reduced: bool = False, skipped: bool = False) -> float:
    """``t`` for a wall-clock ``elapsed`` (seconds since start).

    A key or click (``skipped``) goes straight to the final frame. Under
    reduced motion nothing moves: the settled composition (identical in
    position to ``frame(1)``) holds, then the final frame replaces it.
    """
    if skipped:
        return 1.0
    if reduced:
        return SETTLED_T if elapsed < REDUCED_HOLD_S else 1.0
    return min(1.0, max(0.0, elapsed / DURATION_S))


def overlaps(a: Shape, b: Shape) -> bool:
    ax, ay, aw, ah = a.rect()
    bx, by, bw, bh = b.rect()
    return ax < bx + bw and bx < ax + aw and ay < by + bh and by < ay + ah


def place_box(anchor, workarea, box=BOX):
    """Window rectangle (x, y, w, h) centred on the cluster, inside the monitor."""
    ax, ay = anchor
    wx, wy, ww, wh = workarea
    w, h = min(box[0], ww), min(box[1], wh)
    x = min(max(wx, round(ax - w / 2)), wx + ww - w)
    y = min(max(wy, round(ay - h / 2)), wy + wh - h)
    return x, y, w, h


# --- GTK renderer ----------------------------------------------------------

class PaperScene:
    """One playing scene inside the caller's GTK main loop (no nested loop).

    ``finish()`` jumps to the final frame and closes: the desktop calls it on
    the next hotkey, since an input-transparent window receives no keys.
    """

    def __init__(self, words, anchor_xy, on_done=None):
        import gi
        gi.require_version("Gtk", "3.0")
        from gi.repository import Gdk, GLib, Gtk, Pango
        from .sand_canvas import connect_draw
        from .sand_surface import LIGHT, configure, palette
        self.Gtk, self.GLib, self.Pango = Gtk, GLib, Pango
        self.words, self.on_done = [str(w) for w in words][:4], on_done
        self.reduced = motion.reduced(Gtk)
        self.closed = self.skipped = False
        self.started = time.monotonic()
        display = Gdk.Display.get_default()
        monitor = display.get_monitor_at_point(int(anchor_xy[0]), int(anchor_xy[1])) or display.get_monitor(0)
        area = monitor.get_workarea()
        x, y, w, h = place_box(anchor_xy, (area.x, area.y, area.width, area.height))
        self.box = (w, h)
        window = self.window = Gtk.Window(title="На бумагу")
        configure(window, Gtk, Gdk, force=True)
        try:
            window.set_wmclass("cognitive-sand", "cognitive-sand")
        except Exception:  # noqa: BLE001 - deprecated API, naming is optional
            pass
        window.set_decorated(False)
        window.set_resizable(False)
        window.set_keep_above(True)
        window.set_skip_taskbar_hint(True)
        window.set_skip_pager_hint(True)
        window.set_accept_focus(False)
        window.set_focus_on_map(False)
        window.set_type_hint(Gdk.WindowTypeHint.NOTIFICATION)
        try:
            tone = "light" if palette(Gdk, x, y, w, h) is LIGHT else "dark"
        except Exception:  # noqa: BLE001 - tone is a nicety
            tone = "dark"
        self.colours = theme.palette(tone)
        self.area = Gtk.DrawingArea()
        self.area.set_size_request(w, h)
        window.add(self.area)
        self.layouts = {}
        self.handle = connect_draw(self.area, self.paint)
        window.connect("map", lambda *_: self._pass_input())
        window.connect("delete-event", lambda *_: (self.close(), True)[1])
        window.move(x, y)
        window.resize(w, h)
        window.show_all()
        self.tick_id = self.area.add_tick_callback(self.tick)

    def _pass_input(self):
        """An empty input shape: clicks and keys go to the page underneath."""
        try:
            from .orbital_assets import _shape_api
            C, cairo, gdk, _ = _shape_api()
            region = cairo.cairo_region_create()
            try:
                gdk.gdk_window_input_shape_combine_region(hash(self.window.get_window()), region, 0, 0)
            finally:
                cairo.cairo_region_destroy(region)
        except Exception:  # noqa: BLE001 - worst case the window takes clicks for 0.9 s
            pass

    def _layout(self, text, size):
        key = (text, size)
        if key not in self.layouts:
            layout = self.area.create_pango_layout(text)
            layout.set_font_description(self.Pango.FontDescription.from_string(theme.pango(size)))
            self.layouts[key] = layout
        return self.layouts[key]

    def t(self):
        return playhead(time.monotonic() - self.started, reduced=self.reduced, skipped=self.skipped)

    def paint(self, painter):
        if self.closed:
            return
        painter.clear()
        for shape in frame(self.t(), self.words, self.box):
            if shape.alpha <= 0.003:
                continue
            painter.plate(shape.x, shape.y, shape.w, shape.h, 6, theme.rgb(self.colours[shape.plate]), .94 * shape.alpha)
            layout = self._layout(shape.text, shape.size)
            tw, th = layout.get_pixel_size()
            painter.text(layout, shape.x + (shape.w - tw) / 2, shape.y + (shape.h - th) / 2,
                         theme.rgb(self.colours[shape.ink]), shape.alpha)

    def tick(self, *_):
        if self.closed:
            return False
        self.area.queue_draw()
        if self.t() >= 1.0:
            self.tick_id = None
            self.close()
            return False
        return True

    def finish(self):
        """Any key or click: straight to the final frame, which is empty."""
        self.skipped = True
        self.close()

    def close(self):
        if self.closed:
            return
        self.closed = True
        if self.tick_id is not None:
            try:
                self.area.remove_tick_callback(self.tick_id)
            except Exception:  # noqa: BLE001
                pass
            self.tick_id = None
        self.window.destroy()
        if self.on_done:
            self.GLib.idle_add(lambda: (self.on_done(), False)[1])


def play(words: list[str], anchor_xy: tuple[float, float], on_done=None) -> PaperScene | None:
    """Start the scene in the running GTK main loop; returns its handle.

    Never raises: without GTK or a display it calls ``on_done`` at once and
    returns None, so the caller's flow does not depend on the picture.
    """
    try:
        return PaperScene(words, anchor_xy, on_done)
    except Exception:  # noqa: BLE001 - a missing display must not break the step
        if on_done:
            on_done()
        return None
