"""Four words made of sand: one window from the selection to the words and back.

The motion follows the reference demo (sand-kit ``overlay_sand_cues.html``):
grains peel off the selection, breathe in a core while the model thinks, burst
into the words and land one after another, flow along a stream into the
disclosed text and settle back into the selection on Esc. The words themselves
are grains over a faint copy of the text.

Owner's decisions of 10.10.2026: the sand lives while the pointer is near and
calms to stillness a few seconds after it leaves; the words are sand with pale
text beneath; waiting and words are one window, the grains are never reborn.

Drawn with the frame clock through native cairo (``sand_canvas``): no
intermediate images and no fixed frame cap. Frames stop when nothing moves.
"""
from __future__ import annotations

import json
import math
import random
import sys
import time
from dataclasses import dataclass
from pathlib import Path

from .orbital import Disclosure
from .sand_core import sample_mask

#: Grains per word, as in the demo: the word, the reserve that becomes the
#: disclosed text, and the stream that flows between them.
MAIN, RESERVE, STREAM = 380, 200, 60
PER_WORD = MAIN + RESERVE + STREAM
TOTAL = 4 * PER_WORD
LEVELS = 12
LIFT_S, CORE_MIN_S, RETURN_S = .5, .3, 1.2
#: Seconds after the last pointer motion before the sand starts calming down.
AWAKE_S, CALM_S = 2.5, 1.4
COL_W, WORD_H, GAP = 230, 64, 10
DETAIL_W, DETAIL_MAX = 440, 200
ORIGINS = {"copy": "из активного окна", "primary": "из выделения", "clipboard": "из буфера обмена"}


def clamp(value, low=0.0, high=1.0):
    return low if value < low else high if value > high else value


def noise(x, y, t):
    return math.sin(x * .012 + t) * math.cos(y * .014 - t * .7) + math.sin((x + y) * .008 + t * .5)


@dataclass(frozen=True)
class Box:
    x: float
    y: float
    width: float
    height: float

    def contains(self, x, y):
        return self.x <= x <= self.x + self.width and self.y <= y <= self.y + self.height


@dataclass(frozen=True)
class Plan:
    width: int
    height: int
    position: tuple[int, int]
    source: Box                                     # the selection, window-local
    words: tuple[tuple[float, float, float], ...]   # centre x, centre y, column width
    sides: tuple[int, ...]                          # -1 above the selection, +1 below
    core: tuple[float, float]
    footer: tuple[float, float]                     # centre x, top y
    actions_y: float
    found: bool = False

    @property
    def field(self):
        """Size of the area the words live in (scale of the magnetic pull)."""
        return self.source.width + 2 * COL_W, self.source.height + 2 * WORD_H + 40


def _row(count, x0, x1, column):
    """Centres of ``count`` words over [x0, x1]: the outer ones sit at the ends."""
    centre = (x0 + x1) / 2
    if count == 1:
        return [centre]
    left = min(x0 + column / 2, centre - column / 2 - 6)
    right = max(x1 - column / 2, centre + column / 2 + 6)
    return [left + (right - left) * i / (count - 1) for i in range(count)]


def plan(workarea, anchor, count=4, actions=0, selection=None) -> Plan:
    """Words at the corners of the selection: the top ones above it, the bottom ones below.

    Owner, 10.10.2026: the upper-left word over the left end, upper-right over
    the right end, lower ones under them. Without a found selection the
    pointer's line stands in for it. Explanations open outward from a word,
    so the window keeps room above the top row and below the bottom row.
    """
    ax, ay, aw, ah = workarea
    cx = min(max(anchor[0], ax), ax + aw)
    cy = min(max(anchor[1], ay), ay + ah)
    if selection:
        x0, y0, x1, y1 = selection
    else:
        x0, y0, x1, y1 = cx - 150, cy - 13, cx + 150, cy + 13
    x0, x1 = max(ax, x0), min(ax + aw, x1)
    y0, y1 = max(ay, y0), min(ay + ah, y1)
    count = max(1, count)
    top = math.ceil(count / 2)
    bottom = count - top
    half = WORD_H / 2
    top_y = max(ay + 4 + half, y0 - GAP - half)
    bottom_y = min(ay + ah - 4 - half, y1 + GAP + half)
    lo, hi = ax + COL_W / 2 + 4, ax + aw - COL_W / 2 - 4
    words, sides = [], []
    for n, y, side in ((top, top_y, -1), (bottom, bottom_y, 1)):
        for x in _row(n, x0, x1, COL_W):
            words.append((min(max(x, lo), hi), y, COL_W))
            sides.append(side)
    centre = (x0 + x1) / 2
    core = (x1 + 46, (y0 + y1) / 2) if x1 + 90 < ax + aw else (x0 - 46, (y0 + y1) / 2)
    core = (clamp(core[0], ax + 64, ax + aw - 64),
            clamp(core[1], ay + 34, ay + ah - 60))
    xs = [x for x, _, _ in words]
    wx0 = max(ax, min([x0, core[0] - 64, centre - DETAIL_W / 2] + [x - COL_W / 2 for x in xs]) - 8)
    wx1 = min(ax + aw, max([x1, core[0] + 64, centre + DETAIL_W / 2] + [x + COL_W / 2 for x in xs]) + 8)
    below = max(DETAIL_MAX + 14, 30 + 36 * actions)
    wy0 = max(ay, min(top_y - half - DETAIL_MAX - 14, core[1] - 34))
    wy1 = min(ay + ah, max(bottom_y + half + below, core[1] + 60))
    local = [(x - wx0, y - wy0, w) for x, y, w in words]
    return Plan(round(wx1 - wx0), round(wy1 - wy0), (round(wx0), round(wy0)),
                Box(x0 - wx0, y0 - wy0, x1 - x0, y1 - y0), tuple(local), tuple(sides),
                (core[0] - wx0, core[1] - wy0), (centre - wx0, bottom_y + half + 4 - wy0),
                bottom_y + half + 26 - wy0, bool(selection))


def detail_box(plan_: Plan, index: int, height: float) -> Box:
    """Where a word's explanation goes: above a top word, below a bottom word.

    Near a screen edge, when the preferred side has no room, it opens the other way.
    """
    cx, cy, _ = plan_.words[index]
    width = min(DETAIL_W, plan_.width - 16)
    x = min(max(cx - width / 2, 8), plan_.width - width - 8)
    up = cy - WORD_H / 2 - 18
    down = plan_.height - (cy + WORD_H / 2 + 18)
    side = plan_.sides[index]
    want = min(height, 60)
    if side < 0 and up < want and down > up or side > 0 and down < want and up > down:
        side = -side
    room = up if side < 0 else down
    h = max(1, min(height, room, DETAIL_MAX))
    if side < 0:
        return Box(x, max(0, cy - WORD_H / 2 - 10 - h), width, h)
    return Box(x, cy + WORD_H / 2 + 10, width, h)


def bezier(a, c, b, u):
    v = 1 - u
    return v * v * a[0] + 2 * u * v * c[0] + u * u * b[0], v * v * a[1] + 2 * u * v * c[1] + u * u * b[1]


DEPTHS = (1, .6, .75, .45)


class Magnet:
    """One word's drift toward the pointer: offset, scale, tilt (magnetic demo)."""
    __slots__ = ("depth", "x", "y", "vx", "vy", "s", "vs", "r", "vr")

    def __init__(self, depth=1.0):
        self.depth = depth
        self.x = self.y = self.vx = self.vy = self.vs = self.r = self.vr = 0.0
        self.s = 1.0

    def step(self, dt, centre, pointer, size, act):
        """``pointer`` is None when outside; ``act`` fades the pull after the last motion."""
        tx = ty = tr = 0.0
        ts = 1.0
        if pointer is not None and act > 0:
            dx, dy = pointer[0] - centre[0], pointer[1] - centre[1]
            dist = math.hypot(dx, dy)
            near, d = clamp(1 - dist / 220), self.depth
            ux, uy = (dx / dist, dy / dist) if dist else (0.0, 0.0)
            tx = ((dx / size[0]) * 10 * d + ux * near * 7 * d) * act
            ty = ((dy / size[1]) * 10 * d + uy * near * 7 * d) * act
            ts = 1 + near * .045 * act
            tr = (dx / size[0]) * 2.2 * d * act
        self.vx += (tx - self.x) * .09 * dt
        self.vy += (ty - self.y) * .09 * dt
        self.vs += (ts - self.s) * .12 * dt
        self.vr += (tr - self.r) * .08 * dt
        damp = .78 ** dt
        self.vx, self.vy, self.vs, self.vr = self.vx * damp, self.vy * damp, self.vs * damp, self.vr * damp
        self.x += self.vx * dt
        self.y += self.vy * dt
        self.s += self.vs * dt
        self.r += self.vr * dt

    def resting(self):
        return max(abs(self.x), abs(self.y), abs(self.s - 1) * 50, abs(self.r) * 5,
                   abs(self.vx), abs(self.vy)) < .03

    def place(self, cx, cy, x, y):
        """Where a point of the word at (x, y) is drawn with this magnet."""
        angle = math.radians(self.r)
        dx, dy = (x - cx) * self.s, (y - cy) * self.s
        c, s = math.cos(angle), math.sin(angle)
        return cx + self.x + dx * c - dy * s, cy + self.y + dx * s + dy * c


class Grain:
    __slots__ = ("x", "y", "vx", "vy", "sx", "sy", "g", "r", "ox", "oy", "ax", "ay", "bx", "by",
                 "u0", "born", "d", "s", "ph")


class WordSand:
    """Demo physics in frames of 1/60 s, independent of GTK.

    ``mode`` goes lift → core → form → return. ``calm`` (0 awake, 1 still) is
    driven by the window from pointer activity; at 1 the sand stops by itself.
    """

    def __init__(self, plan_: Plan, seed=0, count=4, total=TOTAL, points=None):
        self.plan = plan_
        self.rnd = rnd = random.Random(seed)
        self.mode, self.el, self.ef, self.t, self.flow = "lift", 0.0, 0.0, 0.0, 0.0
        self.count, self.landed, self.rings, self.shake = count, [False] * count, [], 0.0
        self.land_at = [0.0] * count
        self.core = plan_.core
        self.ready = False
        self.moving, self.near = total, 0
        self.magnets = [Magnet(DEPTHS[i % 4]) for i in range(count)]
        src = plan_.source
        lines = max(1, int(src.height // 22))
        self.grains = []
        for i in range(total):
            p = Grain()
            if points:
                # the actual letters of the selection, as seen on screen
                p.x, p.y = points[i % len(points)]
                p.x, p.y = p.sx, p.sy = p.x + rnd.random() - .5, p.y + rnd.random() - .5
            else:
                line = rnd.randrange(lines)
                p.x = p.sx = src.x + rnd.random() * src.width
                p.y = p.sy = src.y + 11 + line * 22 + rnd.gauss(0, 3)
            p.vx, p.vy = (rnd.random() - .5) * .8, -.3 - rnd.random() * 1.1
            a, rr = rnd.random() * 6.2832, math.sqrt(rnd.random())
            p.ox, p.oy = math.cos(a) * rr, math.sin(a) * rr
            p.ax = p.ay = p.bx = p.by = 0.0
            p.u0, p.born = rnd.random(), rnd.random() * .35
            p.d = rnd.random() * .3 + (p.x - src.x) / (src.width + 1) * .3
            p.s, p.ph = rnd.random() * 1.0 + 1.0, rnd.random() * 6.28
            self.grains.append(p)
        self.assign(count)

    def assign(self, count):
        """Split grains into words and roles; works for any number of words."""
        self.count = max(1, count)
        self.landed = [False] * self.count
        self.land_at = [0.0] * self.count
        self.magnets = [Magnet(DEPTHS[i % 4]) for i in range(self.count)]
        per = len(self.grains) // self.count
        main, reserve = round(per * MAIN / PER_WORD), round(per * RESERVE / PER_WORD)
        for i, p in enumerate(self.grains):
            p.g, j = i % self.count, i // self.count
            p.r = 0 if j < main else 1 if j < main + reserve else 2

    def words(self, mains):
        """Targets for each word: list of points per group; starts the burst."""
        self.assign(len(mains))
        counters = [0] * self.count
        for p in self.grains:
            pts = mains[p.g]
            p.ax, p.ay = pts[counters[p.g] % len(pts)]
            counters[p.g] += 1
        self.ready = True
        if self.mode in ("lift", "core") and self.el >= LIFT_S + CORE_MIN_S:
            self.form()

    def disclose(self, group, points):
        for p, (x, y) in zip((p for p in self.grains if p.g == group and p.r == 1), points):
            p.bx, p.by = x, y

    def form(self):
        self.mode, self.ef = "form", 0.0

    def back(self):
        if self.mode != "return":
            self.mode, self.el = "return", 0.0

    @property
    def done(self):
        return self.mode == "return" and self.el > RETURN_S

    def land_due(self):
        """Indexes of words that land this frame (for rings and the open event)."""
        due = [g for g in range(self.count) if not self.landed[g] and self.ef > g * .08 + .42]
        for g in due:
            self.landed[g] = True
            self.land_at[g] = self.ef
            self.shake = max(self.shake, 1.2)
        return due

    def step(self, dt, mouse=None, hover=-1, active=None, streams=(), calm=0.0, reduced=False, pull=0.0, crisp=False):
        """Advance one frame; ``streams`` maps group → (a, c, b) bezier of its flow.

        ``pull`` (0..1) is the magnetic attraction of the words to the pointer.
        ``crisp``: the word itself is the sand (sand-only mode), so its grains
        lock onto the letters fast and barely tremble.
        """
        sec = dt / 60
        self.t += sec
        self.el += sec
        if self.mode == "form":
            self.ef += sec
        if self.mode == "lift" and self.el > LIFT_S:
            self.mode = "core"
        if self.mode == "core" and self.ready and self.el > LIFT_S + CORE_MIN_S:
            self.form()
        awake = 1 - calm
        self.flow += sec * .35 * awake
        mode, t, el, ef = self.mode, self.t, self.el, self.ef
        br = 1 + .05 * math.sin(t * 1.7)
        cx, cy = self.core
        words = self.plan.words
        rnd = self.rnd.random
        cos, sin, hypot = math.cos, math.sin, math.hypot
        mx = my = mvx = mvy = None
        if mouse is not None and mode != "return":
            mx, my, mvx, mvy = mouse
        size = self.plan.field
        turns = []
        for g, magnet in enumerate(self.magnets):
            if mode == "form" and not reduced:
                magnet.step(dt, words[g][:2], None if mx is None else (mx, my), size, pull)
            angle = math.radians(magnet.r)
            turns.append((magnet.x, magnet.y, magnet.s, math.cos(angle), math.sin(angle)))
        moving = near = 0
        for p in self.grains:
            a = noise(p.x, p.y, t * .4) * 3.14
            tx, ty, k, K, dm = p.x, p.y, 0.0, .02, .9
            if mode == "lift":
                # pollen rises off the letters and is gone in ~0.5 s (never rests on lines)
                dm = .95
                p.vy -= .035 * dt
            elif mode == "core":
                k = clamp((el - .45 - p.d) * 2)
                tx, ty, dm = cx + p.ox * 36 * br, cy + p.oy * 36 * br, .86
            elif mode == "form":
                g = p.g
                k, K, dm = clamp((ef - g * .08) * 2.4), .03, .8
                stream = streams[g] if g < len(streams) else None
                if stream and p.r == 1:
                    tx, ty = p.bx, p.by
                elif stream and p.r == 2:
                    tx, ty = bezier(*stream, (p.u0 + self.flow) % 1)
                    K, dm = .06, .7
                else:
                    wx, wy = words[g][0], words[g][1]
                    mxo, myo, ms, mc, msn = turns[g]
                    bs = (1 + .02 * awake * sin(t * 1.3 + g * 1.9)) * ms
                    ddx, ddy = (p.ax - wx) * bs, (p.ay - wy) * bs
                    tx, ty = wx + mxo + ddx * mc - ddy * msn, wy + myo + ddx * msn + ddy * mc
                    if crisp:
                        K, dm = .1, .68
                j = (.35 if hover == g else 1.3) * (.08 + .92 * awake) * (.2 if crisp else 1)
                tx += cos(a + p.ph) * j
                ty += sin(a + p.ph) * j
                if not crisp and awake > .5 and self.landed[g] and rnd() < .0005 * dt:
                    p.vx += (rnd() - .5) * 5
                    p.vy += (rnd() - .5) * 5
            else:
                k, K, dm = clamp(el * 1.7), .026, .82
                tx, ty = p.sx, p.sy
            if reduced and mode != "lift":
                p.x, p.y, p.vx, p.vy = tx, ty, 0.0, 0.0
                continue
            p.vx += ((tx - p.x) * K * k + cos(a) * .06 * (1 - k)) * dt
            p.vy += ((ty - p.y) * K * k + sin(a) * .06 * (1 - k)) * dt
            if mx is not None:
                dx, dy = p.x - mx, p.y - my
                d2 = dx * dx + dy * dy
                if .01 < d2 < 19600:
                    d = math.sqrt(d2)
                    if d < 45:
                        near += 1
                        f = 1 - d / 45
                        p.vx += (dx / d * f * 1.8 + mvx * f * .28) * dt
                        p.vy += (dy / d * f * 1.8 + mvy * f * .28) * dt
                    elif mode == "form":
                        f = (1 - (d - 45) / 95) * .035
                        p.vx -= dx / d * f * dt
                        p.vy -= dy / d * f * dt
            damp = dm ** dt
            p.vx *= damp
            p.vy *= damp
            p.x += p.vx * dt
            p.y += p.vy * dt
            if abs(p.vx) + abs(p.vy) > .04:
                moving += 1
        self.moving, self.near = moving, near
        self.shake *= .85 ** dt
        for ring in self.rings:
            ring[2] += 2.6 * dt
            ring[3] *= .93 ** dt
        self.rings = [ring for ring in self.rings if ring[3] > .01]

    def still(self):
        """Nothing visible changes: frames can stop (battery, attention)."""
        return (self.mode == "form" and all(self.landed) and self.moving < 3
                and not self.rings and self.shake < .05 and all(m.resting() for m in self.magnets))

    def buckets(self, hover=-1, active=None, calm=0.0, fade=1.0, lights=None, size=(0, 0), dust=True, word_dark=None):
        """Grains grouped by alpha level: sand is bright in motion, dust at rest.

        Owner, 10.10.2026: the words are read as real text; once a word has
        landed its resting grains settle to ~20% (dust under the letters),
        moving grains still shimmer. Lift pollen fades out by 0.5 s and the
        core grows back in, so nothing lies on the page's lines.
        With ``lights`` (1 = light page) the second list holds grains over a
        light page, drawn in dark ink.
        """
        out = [[] for _ in range(LEVELS)]
        dark = [[] for _ in range(LEVELS)]
        width, height = size
        mode, el, ef = self.mode, self.el, self.ef
        top = LEVELS - 1
        rests = [1 - .8 * clamp((ef - at) / .3) if landed and dust else 1.0
                 for at, landed in zip(self.land_at, self.landed)]
        for p in self.grains:
            sp = abs(p.vx) + abs(p.vy)
            motion = sp / 2.6 if sp < 2.6 else 1.0
            if mode == "lift":
                al = clamp((el - p.born) * 4) * (1 - clamp((el - .25) / .25))
            elif mode == "core":
                al = clamp((el - .5 - p.d) * 2.5)
            elif mode == "return":
                al = clamp(math.hypot(p.x - p.sx, p.y - p.sy) / 30) * (.4 + .6 * motion)
            else:
                rest = rests[p.g] if p.g < len(rests) else 1.0
                al = rest + (1 - rest) * motion
                if hover >= 0 and hover != p.g or active is not None and active != p.g:
                    al *= .5
            al *= fade
            if al > .02:
                target = out
                if lights is not None:
                    if mode == "form" and word_dark and p.g < len(word_dark) and word_dark[p.g] is not None \
                            and self.landed[p.g]:
                        # one ink per word: a pixel-by-pixel flip makes letters patchy
                        if word_dark[p.g]:
                            target = dark
                    else:
                        x, y = int(p.x), int(p.y)
                        if 0 <= x < width and 0 <= y < height and lights[y * width + x]:
                            target = dark
                target[min(top, int(al * LEVELS))].append((p.x, p.y, p.s))
        return (out, dark) if lights is not None else out


def origin_label(origin):
    return ORIGINS.get(origin, origin)


def show(items, px=None, py=None, title="", reduced=None, actions=None, origin=""):
    """Words are already known (cache, Ctrl+Q, batch): short lift, then form."""
    return SandWords(dict(items=items, x=px, y=py, title=title, actions=actions or [], origin=origin),
                     reduced=reduced).run()


def show_wait(payload):
    """Started on Alt+W before the answer; the service sends the words via ``control``."""
    return SandWords(payload, control=Path(payload["control"]) if payload.get("control") else None).run()


class SandWords:
    def __init__(self, payload, control=None, reduced=None):
        self.payload, self.control, self.reduced_flag = payload, control, reduced

    # --- lifecycle -----------------------------------------------------------

    def run(self):
        from . import sand_screen
        self.launched = time.monotonic()
        marks = [("launch", self.launched)]
        grab = None if self.reduced_flag else sand_screen.start_grab()  # overlaps GTK start-up
        import gi
        gi.require_version("Gtk", "3.0")
        from gi.repository import Gtk, Gdk, GLib, Pango
        from . import popup_helper as helper
        from .sand_fx import reduced_motion
        from .sand_surface import configure, palette
        from .sand_canvas import connect_draw, rgb
        self.Gtk, self.Gdk, self.GLib, self.Pango, self.helper = Gtk, Gdk, GLib, Pango, helper
        helper.apply_popup_font(Gtk, Gdk)
        GLib.set_prgname("cognitive-popup")
        payload = self.payload
        self.rows = helper.orbital_rows(payload.get("items") or [])
        if not self.rows and self.control is None:
            return 0
        x, y = payload.get("x"), payload.get("y")
        self.anchor = helper.get_mouse_position() if x is None or y is None else (x, y)
        self.workarea = helper._monitor_workarea(Gdk, *self.anchor)
        self.quiet = reduced_motion(Gtk) or bool(self.reduced_flag)
        self.actions = list(payload.get("actions") or [])
        self.origin, self.title = str(payload.get("origin") or ""), str(payload.get("title") or "")
        count = len(self.rows) or 4
        marks.append(("gtk", time.monotonic()))
        shot = sand_screen.finish_grab(grab)
        marks.append(("screenshot", time.monotonic()))
        found = sand_screen.find_selection(shot, int(self.anchor[0]), int(self.anchor[1])) if shot else None
        marks.append(("selection", time.monotonic()))
        self.selection = found[0] if found else None
        self.highlight = found[1] if found else None
        self.plan = plan(self.workarea, self.anchor, count, len(self.actions), self.selection)
        seed = int(time.time()) & 0xffff
        points = None
        if found:
            ox, oy = self.plan.position
            letters = sand_screen.glyph_points(shot, found[0], found[1], 900, random.Random(seed))
            points = [(x - ox, y - oy) for x, y in letters] or None
        self.lights = self.image = None
        self.shot, self.halos = shot, []
        import os as _os
        state_dir = Path(_os.environ.get("COGNITIVE_STATE_DIR") or "~/.local/state/cognitive-popups").expanduser()
        self.frosted = (state_dir / "sand.frosted").exists()
        # Owner's experiment, 10.10.2026: words made of sand only, no text layer.
        self.grains_only = (state_dir / "sand.grains_only").exists()
        if shot:
            self.lights = sand_screen.light_map(shot, (*self.plan.position, self.plan.width, self.plan.height))
            light = sum(self.lights[::97]) * 97 > len(self.lights) / 2
            self.colours = {"text": "#2a2016" if light else "#f2e6cf"}
        else:
            src = self.plan.source
            self.colours = palette(Gdk, self.plan.position[0] + src.x, self.plan.position[1] + src.y,
                                   max(8, src.width), max(8, src.height))
        self.ink = rgb(self.colours["text"])
        self.light_ink, self.dark_ink = rgb("#f2e6cf"), rgb("#2a2016")
        marks.append(("page", time.monotonic()))
        self.sand = WordSand(self.plan, seed=seed, count=count, points=points)
        self.startup = [(name, round((at - self.launched) * 1000)) for name, at in marks]
        self.state = Disclosure(self.rows)
        self.layouts, self.extents, self.streams = [], [], []
        self.detail_layout, self.detail_h, self.detail_scroll, self.detail_since = None, 0, 0.0, 0.0
        self.detail_box = None
        self.mouse, self.mvx, self.mvy, self.last_move = None, 0.0, 0.0, -99.0
        self.hover, self.calm, self.closed, self.tick_id = -1, 0.0, False, None
        self.last_frame, self.opened, self.buttons, self.poll_id = None, False, [], None
        self.started = time.monotonic()
        self.marks, self.think_text, self.step_layout, self.footer_layout = {}, None, None, None
        self.looks, self.small_looks, self.detail_look = [], {}, None
        self.glow = None
        import os
        self.tracing, self.peak_pull = os.environ.get("COGNITIVE_SAND_TRACE"), 0.0
        self.trace("startup", ms=dict(self.startup), found=bool(found))
        self.stats = {"frames": 0, "paint_s": 0.0, "step_s": 0.0, "first": None, "last": None}

        window = self.window = Gtk.Window(title=self.title or "Четыре слова · песок")
        window.set_wmclass("cognitive-popup", "cognitive-popup")
        configure(window, Gtk, Gdk, force=True)
        window.set_keep_above(True)
        window.set_skip_taskbar_hint(True)
        window.set_skip_pager_hint(True)
        window.set_type_hint(Gdk.WindowTypeHint.UTILITY)
        window.set_resizable(False)
        if self.control is not None:
            # Waiting must not take the keyboard from the page being read.
            window.set_accept_focus(False)
            window.set_focus_on_map(False)
        # One Fixed: the drawing below, buttons above. An Overlay child here
        # covered the whole window and swallowed every click and motion.
        self.fixed = Gtk.Fixed()
        window.add(self.fixed)
        self.area = Gtk.DrawingArea()
        self.area.set_tooltip_text("+ — раскрыть следующий слой; − — свернуть. Esc — закрыть это окно.")
        self.area.set_size_request(self.plan.width, self.plan.height)
        self.fixed.put(self.area, 0, 0)
        self._draw_handle = connect_draw(self.area, self.paint)
        self.area.add_events(Gdk.EventMask.POINTER_MOTION_MASK | Gdk.EventMask.BUTTON_PRESS_MASK
                             | Gdk.EventMask.LEAVE_NOTIFY_MASK | Gdk.EventMask.SCROLL_MASK)
        self.area.connect("motion-notify-event", self.on_motion)
        self.area.connect("leave-notify-event", self.on_leave)
        self.area.connect("button-press-event", self.on_button)
        self.area.connect("scroll-event", self.on_scroll)
        helper.connect_popup_keys(window, Gdk, self.on_key)
        window.connect("delete-event", lambda *_: self.dismiss())
        window.move(*self.plan.position)
        window.resize(self.plan.width, self.plan.height)
        from .sand_audio import Mixer
        self.audio = None if self.quiet else Mixer.create()
        self.last_grain = 0.0
        window.show_all()
        self.startup.append(("shown", round((time.monotonic() - self.launched) * 1000)))
        self.shape()
        if self.audio:
            src = self.plan.source
            p0, p1 = self.pan(src.x), self.pan(src.x + src.width)
            self.audio.play("lift", .5, (p0 + p1) / 2)
            self.audio.pour(.8, 80, p0, p1, gain=.06)
        if self.control is None:
            self.receive(payload, first=True)
        else:
            helper._emit("window_open", window="sand", detail="wait")
            self.poll_id = GLib.timeout_add(80, self.poll)
        self.wake()
        Gtk.main()
        return 0

    def receive(self, payload, first=False):
        """The words exist: build their shapes, start the burst, log the view."""
        helper = self.helper
        if not first:
            helper._emit("window_close", window="sand", detail="became words", silent=True)
            helper._PRESENTATION = helper.PopupObservation(helper._CHAIN, payload)
            helper.STEP_LINE = str(payload.get("step_line", "") or "")
            self.origin = str(payload.get("origin") or self.origin)
            self.actions = list(payload.get("actions") or [])
            self.rows = helper.orbital_rows(payload.get("items") or [])
            if not self.rows:
                return self.dismiss()
            if len(self.rows) != self.sand.count or self.actions:
                self.replan(plan(self.workarea, self.anchor, len(self.rows), len(self.actions), self.selection))
            self.state = Disclosure(self.rows)
        self.step_layout = self.make_layout(helper.STEP_LINE, "Sans 9.5", DETAIL_W, centre=True) if helper.STEP_LINE else None
        self.footer_layout = self.make_layout(self.origin or "1–4: раскрыть · Esc: вернуться к тексту",
                                              "Sans 9", DETAIL_W, centre=True)
        self.build_words()
        self.build_actions()
        self.window.set_accept_focus(True)
        self.shape()
        helper._rendered(title=self.title, layout="sand", cues=self.state.visible(), active=None, layer=1, origin=self.origin)
        helper._emit("window_open", window="dual", layer=1, detail="sand cues")
        ox, oy = self.plan.position
        src = self.plan.source
        self.trace("words", count=len(self.rows), adopted=not first, found=self.plan.found,
            selection=[round(ox + src.x), round(oy + src.y), round(ox + src.x + src.width), round(oy + src.y + src.height)],
            ink=self.colours["text"], halo=bool(self.halos), sides=list(self.plan.sides),
            centres=[[round(ox + cx), round(oy + cy)] for cx, cy, _ in self.plan.words])
        from . import sound
        sound.play("orbital_open")
        self.opened = True
        self.shape()
        self.ready_at = time.monotonic()
        self.ready_painted = False
        self.words_painted = False
        self.wake()
        return False

    def pan(self, x):
        """Stereo position around the selection: left words left, right words right.

        Measured from the selection's centre, not the monitor's: a selection on the
        right half of the screen no longer pulls every sound into the right ear.
        """
        src = self.plan.source
        half = max(1.0, self.plan.width / 2)
        return clamp((x - (src.x + src.width / 2)) / half, -1.0, 1.0) * .7

    def replan(self, fresh):
        """Another number of words: move the window, keep every grain where it is on screen."""
        old = self.plan
        dx, dy = old.position[0] - fresh.position[0], old.position[1] - fresh.position[1]
        for p in self.sand.grains:
            p.x, p.y, p.sx, p.sy = p.x + dx, p.y + dy, p.sx + dx, p.sy + dy
        self.sand.core = (self.sand.core[0] + dx, self.sand.core[1] + dy)
        self.plan = self.sand.plan = fresh
        if self.lights is not None and (old.width, old.height, old.position) != (fresh.width, fresh.height, fresh.position):
            # page colours are known only for the old rectangle: fall back to one ink
            self.lights, self.image = None, None
        self.area.set_size_request(fresh.width, fresh.height)
        self.window.move(*fresh.position)
        self.window.resize(fresh.width, fresh.height)

    # --- text and shapes ----------------------------------------------------

    def make_layout(self, text, font, width, markup=False, centre=False):
        Pango = self.Pango
        layout = self.area.create_pango_layout("")
        layout.set_font_description(Pango.FontDescription.from_string(font))
        layout.set_width(int(width * Pango.SCALE))
        layout.set_wrap(Pango.WrapMode.WORD_CHAR)
        if centre:
            layout.set_alignment(Pango.Alignment.CENTER)
        if markup:
            layout.set_markup(text, -1)
        else:
            layout.set_text(text, -1)
        return layout

    def look(self, layout, x, y, w, h, layout_w, layout_h, radius=6):
        """(ink, page colour, halo, plate colour) for text whose glyphs cover (x, y, w, h)."""
        from . import sand_screen
        from .sand_canvas import Halo
        page = None
        if self.shot is not None:
            ox, oy = self.plan.position
            page = sand_screen.mean_colour(self.shot, (ox + x, oy + y, w, h), exclude=self.highlight)
        if page is None:
            return self.ink, None, None
        ink = tuple(v / 255 for v in sand_screen.ink_for(page))
        colour = tuple(v / 255 for v in page)
        halo = Halo(layout, layout_w, layout_h, colour, radius)
        self.halos.append(halo)
        return ink, colour, halo

    def build_words(self):
        from .text_mask import render
        self.layouts, self.extents, mains, self.looks = [], [], [], []
        per = len(self.sand.grains) // len(self.rows)
        for (simple, _, _), (cx, cy, width) in zip(self.rows, self.plan.words):
            size = 18
            while True:
                # Semi-bold and never under ~20 px: it is read as text, not texture.
                layout = self.make_layout(simple, f"Sans SemiBold {size}", width, centre=True)
                w, h = layout.get_pixel_size()
                if (layout.get_line_count() <= 2 and h <= WORD_H) or size <= 15:
                    break
                size -= 1
            ink = layout.get_pixel_extents()[0]
            left, top = cx - width / 2, cy - h / 2
            mask = render(layout, width, h)
            mains.append(sample_mask(*mask, per, self.sand.rnd, (left, top)))
            self.layouts.append(layout)
            self.extents.append((left, top, ink.x + left, ink.width, h))
            self.looks.append(self.look(layout, ink.x + left - 6, top - 4, ink.width + 12, h + 8, width, h))
        self.sand.words(mains)
        if self.quiet:
            self.sand.form()
            self.sand.ef = 9.0

    def build_actions(self):
        for button, _ in self.buttons:
            button.destroy()
        self.buttons = []
        for i, action in enumerate(self.actions):
            button = self.Gtk.Button(label=str(action.get("label") or "Продолжить"))
            button.set_size_request(220, 32)
            bx = min(max(self.plan.footer[0] - 110, 4), self.plan.width - 224)
            by = self.plan.actions_y + i * 36
            self.fixed.put(button, round(bx), round(by))
            button.connect("clicked", lambda _, action=action: self.action(action))
            button.show()
            self.buttons.append((button, (bx, by, 220, 32)))

    def shape(self):
        """Only controls receive input; the underlying selection stays scrollable."""
        from .input_shapes import apply
        runs = []
        if self.opened:
            for (_, cy, _), (_, _, ix, iw, h), magnet in zip(self.plan.words, self.extents, self.sand.magnets):
                runs.append((ix + magnet.x - 14, cy + magnet.y - h / 2 - 8, iw + 28, h + 16))
        if self.detail_box is not None:
            d = self.detail_box
            runs.append((d.x, d.y, d.width, d.height))
        runs += [rect for _, rect in self.buttons]
        signature = tuple((math.floor(x), math.floor(y), math.ceil(x+w)-math.floor(x),
                           math.ceil(y+h)-math.floor(y)) for x, y, w, h in runs)
        if signature != getattr(self, '_input_runs', None):
            self._input_runs = signature
            apply(self.window.get_window(), signature)

    def word_at(self, x, y):
        if not self.opened or self.sand.mode != "form":
            return -1
        for i, (cx, cy, width) in enumerate(self.plan.words[:len(self.layouts)]):
            _, _, ix, iw, h = self.extents[i]
            magnet = self.sand.magnets[i]
            if abs(x - (ix + iw / 2 + magnet.x)) < iw / 2 + 14 and abs(y - cy - magnet.y) < h / 2 + 8:
                return i
        return -1

    def open_detail(self):
        """Lay out term/meaning of the active word outward from it; send its reserve grains there."""
        state, GLib = self.state, self.GLib
        if state.active is None:
            self.detail_layout, self.streams, self.detail_box = None, [], None
            self.shape()
            return
        _, term, meaning = state.visible()[state.active]
        markup = f"<b>{GLib.markup_escape_text(term)}</b>"
        if meaning:
            markup += "\n\n" + GLib.markup_escape_text(meaning)
        width = min(DETAIL_W, self.plan.width - 16)
        self.detail_layout = self.make_layout(markup, "Sans 12", width, markup=True)
        self.detail_h = self.detail_layout.get_pixel_size()[1]
        d = self.detail_box = detail_box(self.plan, state.active, self.detail_h)
        ink_w = self.detail_layout.get_pixel_extents()[1].width
        # wider glow: it must also cover the page's lines between the explanation's lines
        self.detail_look = self.look(self.detail_layout, d.x, d.y, ink_w, min(self.detail_h, d.height), width,
                                     self.detail_h, radius=9)
        self.detail_scroll, self.detail_since = 0.0, self.sand.t
        from .text_mask import render
        mask = render(self.detail_layout, d.width, max(1, int(min(self.detail_h, d.height))))
        count = sum(1 for p in self.sand.grains if p.g == state.active and p.r == 1)
        self.sand.disclose(state.active, sample_mask(*mask, count, self.sand.rnd, (d.x, d.y)))
        streams = [None] * len(self.rows)
        cx, cy, _ = self.plan.words[state.active]
        h = self.extents[state.active][4]
        if d.y < cy:
            a, b = (cx, cy - h / 2 - 4), (d.x + d.width / 2, d.y + d.height + 4)
        else:
            a, b = (cx, cy + h / 2 + 4), (d.x + d.width / 2, d.y - 4)
        streams[state.active] = (a, (b[0], a[1] + (b[1] - a[1]) * .2), b)
        self.streams = streams
        self.shape()

    # --- frames -------------------------------------------------------------

    def wake(self):
        if self.closed:
            return
        if self.tick_id is None:
            self.last_frame = None
            self.stats["last"] = None  # a pause while still is not a slow frame
            self.tick_id = self.area.add_tick_callback(self.tick)

    def tick(self, _widget, clock):
        now = clock.get_frame_time() / 1e6
        dt = 1.0 if self.last_frame is None else min(2.5, max(.05, (now - self.last_frame) * 60))
        self.last_frame = now
        sand = self.sand
        awake = self.mouse is not None or time.monotonic() - self.last_move < AWAKE_S
        if sand.mode in ("lift", "core", "return") or self.quiet:
            self.calm = 0.0
        else:
            # Wakes quickly under the pointer, calms slowly after it leaves.
            self.calm = clamp(self.calm - dt / 60 / .35) if awake else clamp(self.calm + dt / 60 / CALM_S)
        if sand.mode == "core" and not sand.ready and time.monotonic() - self.started > 20:
            self.calm = clamp(self.calm + dt / 60 / CALM_S)
        mouse = None if self.mouse is None else (*self.mouse, self.mvx, self.mvy)
        landing = time.perf_counter()
        for g in sand.land_due():
            cx, cy, _ = self.plan.words[g]
            sand.rings.append([cx, cy, 8.0, .35])
            if self.audio:
                from .sand_audio import NOTES
                self.audio.play("land", .45, self.pan(cx))
                self.audio.note(NOTES[g % 4], .26, self.pan(cx), .02)
            if g == 0:
                self.take_focus()
        if time.perf_counter() - landing > .01:
            self.trace("slow_landing", ms=round((time.perf_counter() - landing) * 1000, 1))
        began = time.perf_counter()
        since = time.monotonic() - self.last_move
        pull = 0.0 if self.mouse is None or self.quiet else 1 - clamp((since - .9) / 1.2) * .85
        sand.step(dt, mouse, self.hover, self.state.active, self.streams, self.calm, self.quiet, pull,
                  crisp=self.grains_only)
        if self.tracing:
            peak = max((math.hypot(m.x, m.y) for m in sand.magnets), default=0.0)
            if peak > self.peak_pull + .5 or peak < self.peak_pull - 2:
                self.peak_pull = peak
                self.trace("magnet", offset=round(peak, 1), scale=round(max(m.s for m in sand.magnets), 3))
        stats = self.stats
        stats["step_s"] += time.perf_counter() - began
        stats["frames"] += 1
        if stats["last"] is not None and now - stats["last"] < .1:
            stats["active_s"] = stats.get("active_s", 0.0) + now - stats["last"]
            stats["active_frames"] = stats.get("active_frames", 0) + 1
            stats["worst_ms"] = max(stats.get("worst_ms", 0.0), (now - stats["last"]) * 1000)
            if now - stats["last"] > .04:
                self.trace("slow_frame", ms=round((now - stats["last"]) * 1000, 1), mode=sand.mode,
                           active=self.state.active, layer=self.state.layer)
        stats["first"] = stats["first"] or now
        stats["last"] = now
        if self.audio:
            self.sound_frame(dt)
        self.mvx *= .85 ** dt
        self.mvy *= .85 ** dt
        self.shape()
        self.area.queue_draw()
        if sand.done:
            self.tick_id = None
            self.finish()
            return False
        if self.quiet and sand.mode == "form" or sand.still() and self.calm >= 1 \
                or sand.mode == "core" and not sand.ready and self.calm >= 1:
            self.tick_id = None
            self.trace("still", mode=sand.mode, on_target=self.on_target())
            return False
        return True

    def sound_frame(self, dt):
        """Rustle under a moving pointer; a faint trickle while a stream flows."""
        sand, now = self.sand, time.monotonic()
        speed = math.hypot(self.mvx, self.mvy)
        if self.mouse is not None and sand.near > 5 and speed > 1.2 and now - self.last_grain > .02:
            self.last_grain = now
            self.audio.grain(2000 + speed * 260 + sand.rnd.random() * 3000, min(.08, speed * sand.near * .0003),
                             self.pan(self.mouse[0]))
        active = self.state.active
        if active is not None and self.streams and self.calm < .5 and sand.rnd.random() < .1 * dt:
            self.audio.grain(5000 + sand.rnd.random() * 4000, .012, self.pan(self.plan.words[active][0]))

    def on_target(self):
        """Share of word grains drawn where their letter is (live check of the shape)."""
        if self.sand.mode != "form" or not self.layouts:
            return None
        sand, good, total = self.sand, 0, 0
        for p in sand.grains:
            if p.r != 0:
                continue
            cx, cy, _ = self.plan.words[p.g]
            x, y = sand.magnets[p.g].place(cx, cy, p.ax, p.ay)
            total += 1
            good += abs(p.x - x) + abs(p.y - y) < 4
        return round(good / max(1, total), 3)

    def trace(self, kind, **data):
        if not self.tracing:
            return
        with open(self.tracing, "a", encoding="utf-8") as handle:
            handle.write(json.dumps({"t": round(time.monotonic() - self.started, 3), "kind": kind, **data},
                                    ensure_ascii=False) + "\n")

    def paint(self, painter):
        began = time.perf_counter()
        self.draw(painter)
        self.stats["paint_s"] += time.perf_counter() - began
        if self.opened and not self.ready_painted:
            self.ready_painted = True
            self.helper.observation.record_annotation("sand_render_timing", {
                "helper_startup_ms": dict(self.startup),
                "ready_to_draw_ms": round((time.monotonic() - self.ready_at) * 1000, 1),
                "mode": self.sand.mode,
            })
        if self.opened and not self.words_painted and self.sand.mode == "form" and self.sand.ef >= .1:
            self.words_painted = True
            self.helper.observation.record_annotation("sand_words_timing", {
                "ready_to_words_draw_ms": round((time.monotonic() - self.ready_at) * 1000, 1),
            })

    def ink_at(self, x, y):
        """Ink opposite to the page at a window point (one ink when the page is unknown)."""
        lights, plan_ = self.lights, self.plan
        if lights is None:
            return self.ink
        xi, yi = int(x), int(y)
        if 0 <= xi < plan_.width and 0 <= yi < plan_.height and lights[yi * plan_.width + xi]:
            return self.dark_ink
        return self.light_ink

    def write(self, painter, layout, x, y, alpha, look=None, **transform):
        """Real text: page-coloured halo first, then the glyphs in the ink opposite to the page."""
        ink, page, halo = look or (self.ink, None, None)
        if halo is not None:
            painter.halo(halo, x, y, alpha * .95, **transform)
        painter.text(layout, x, y, ink, alpha, **transform)

    def small_look(self, layout, x, y, w, h):
        key = (id(layout), round(x), round(y))
        if key not in self.small_looks:
            self.small_looks[key] = self.look(layout, x, y, w, h, w, h)
        return self.small_looks[key]

    def draw(self, painter):
        painter.clear()
        sand, plan_ = self.sand, self.plan
        if sand.shake > .05:
            painter.translate((sand.rnd.random() - .5) * sand.shake, (sand.rnd.random() - .5) * sand.shake)
        fade = 1.0 if sand.mode != "return" else clamp(1 - sand.el / .4)
        active = self.state.active
        words = []
        if sand.mode in ("form", "return") and self.layouts:
            for g, layout in enumerate(self.layouts):
                if not sand.landed[g]:
                    continue
                left, top, ix, iw, h = self.extents[g]
                magnet = sand.magnets[g]
                cx, cy, width = plan_.words[g]
                # the word surfaces as text ~150 ms after its sand lands
                shown = clamp((sand.ef - sand.land_at[g]) / .15) * fade * (.55 if active not in (None, g) else 1)
                transform = dict(scale=magnet.s, angle=math.radians(magnet.r), pivot=(cx - left, cy - top))
                words.append((g, layout, left + magnet.x, top + magnet.y, shown, transform, ix + iw + 8 + magnet.x,
                              cy - h * .45 + magnet.y))
        words_landed = [False] * len(self.layouts)
        for g, *_ in words:
            words_landed[g] = True
        if self.frosted:
            for g, _, x, y, shown, _, _, _ in words:
                _, page, _ = self.looks[g]
                if page is not None:
                    left, top, ix, iw, h = self.extents[g]
                    painter.plate(ix - 14 + (x - left), y - 8, iw + 28, h + 16, 10, page, .8 * shown)
        if self.grains_only:
            # The halo is made of the grains themselves: pushed sand takes its
            # glow along and the page shows through where the sand has left.
            # Only grains at rest carry a glow: flying sand leaves no shadow,
            # and on the way back the whole glow fades out with the words.
            by_word = {}
            x0 = y0 = 1e9
            x1 = y1 = -1e9
            for p in sand.grains:
                if p.r == 0 and p.g < len(words_landed) and words_landed[p.g]:
                    speed = abs(p.vx) + abs(p.vy)
                    if speed > 2.4:
                        continue
                    x, y = p.x, p.y
                    by_word.setdefault((p.g, speed < .8), []).append((x, y))
                    if x < x0: x0 = x
                    if x > x1: x1 = x
                    if y < y0: y0 = y
                    if y > y1: y1 = y
            if by_word and fade > .01:
                from .sand_canvas import GlowLayer
                if self.glow is None:
                    self.glow = GlowLayer(plan_.width, plan_.height)
                self.glow.begin()
                for (g, resting), points in by_word.items():
                    page = self.looks[g][1] if g < len(self.looks) else None
                    if page is not None:
                        self.glow.add(points, page, alpha=.8 if resting else .3)
                painter.glow_layer(self.glow, (x0 - 14, y0 - 14, x1 + 14, y1 + 14), alpha=fade)
        else:
            for g, _, x, y, shown, transform, _, _ in words:
                halo = self.looks[g][2]
                if halo is not None:
                    painter.halo(halo, x, y, shown * .95, **transform)
        if self.lights is not None:
            word_dark = [look[0] == self.dark_ink if look[1] is not None else None for look in self.looks]
            light_ink, dark_ink = sand.buckets(self.hover, active, self.calm, fade, self.lights,
                                               (plan_.width, plan_.height), dust=not self.grains_only,
                                               word_dark=word_dark)
            painter.grains(light_ink, self.light_ink, LEVELS)
            painter.grains(dark_ink, self.dark_ink, LEVELS)
        else:
            painter.grains(sand.buckets(self.hover, active, self.calm, fade, dust=not self.grains_only), self.ink, LEVELS)
        for g, layout, x, y, shown, transform, mx, my in words:
            ink = self.looks[g][0]
            if not self.grains_only:
                painter.text(layout, x, y, ink, shown, **transform)
            collapses = active == g and (self.state.layer >= 3 or not self.rows[g][2])
            mark = "−" if collapses else "+"
            if mark not in self.marks:
                self.marks[mark] = self.make_layout(mark, "Sans SemiBold 11", 20)
            painter.text(self.marks[mark], mx, my, ink, (.45 + (.45 if self.hover == g else 0)) * shown)
        for x, y, radius, alpha in sand.rings:
            painter.ring(x, y, radius, self.ink_at(x, y), alpha)
        d = self.detail_box
        if self.detail_layout is not None and d is not None and sand.mode == "form":
            alpha = clamp((sand.t - self.detail_since - .35) * 2)
            look = self.detail_look
            if self.frosted and look[1] is not None:
                painter.plate(d.x - 10, d.y - 8, d.width + 20, min(self.detail_h, d.height) + 16, 10, look[1], .8 * alpha)
            painter.clip(d.x - 8, d.y - 8, d.width + 16, d.height + 16)
            self.write(painter, self.detail_layout, d.x, d.y - self.detail_scroll, alpha, look)
            painter.unclip()
        if sand.mode == "core" or sand.mode == "lift" and not self.opened:
            waited = time.monotonic() - self.started
            if waited > 1.2 and not sand.ready:
                key = "ещё думаю…" if waited > 10 else "думаю…"
                if self.think_text != key:
                    self.think_text, self.think = key, self.make_layout(key, "Sans 10", 120, centre=True)
                x, y = sand.core[0] - 60, sand.core[1] + 34
                self.write(painter, self.think, x, y, .7 * clamp(waited - 1.2), self.small_look(self.think, x, y, 120, 20))
        if self.opened and sand.mode == "form":
            footer_alpha = .75 * clamp(sand.ef - .5)
            side = -1 if d is not None and d.y < plan_.words[active][1] else 1 if d is not None else 0
            fx, fy = plan_.footer
            if self.step_layout is not None and side >= 0:
                top_y = min(cy - WORD_H / 2 for _, cy, _ in plan_.words) - 22
                self.write(painter, self.step_layout, fx - DETAIL_W / 2, top_y, footer_alpha,
                           self.small_look(self.step_layout, fx - DETAIL_W / 2, top_y, DETAIL_W, 18))
            if side <= 0:
                self.write(painter, self.footer_layout, fx - DETAIL_W / 2, fy, footer_alpha,
                           self.small_look(self.footer_layout, fx - DETAIL_W / 2, fy, DETAIL_W, 18))

    # --- input --------------------------------------------------------------

    def on_motion(self, _widget, event):
        if self.tracing and time.monotonic() - getattr(self, "_motion_traced", 0) > .25:
            self._motion_traced = time.monotonic()
            self.trace("motion", x=round(event.x), y=round(event.y), root=[round(event.x_root), round(event.y_root)])
        if self.mouse is not None:
            self.mvx = self.mvx * .5 + (event.x - self.mouse[0]) * .5
            self.mvy = self.mvy * .5 + (event.y - self.mouse[1]) * .5
        self.mouse = (event.x, event.y)
        self.last_move = time.monotonic()
        hover = -1 if self.quiet else self.word_at(event.x, event.y)
        if hover != self.hover:
            self.hover = hover
            self.trace("hover", word=hover)
            if self.audio and hover >= 0:
                pan = self.pan(self.plan.words[hover][0])
                self.audio.pour(.25, 10, pan, pan, low=6000, high=9000, gain=.03)
        self.wake()
        return False

    def on_leave(self, *_):
        self.mouse, self.mvx, self.mvy = None, 0.0, 0.0
        self.hover = -1
        self.wake()
        return False

    def on_button(self, _widget, event):
        self.trace("click", button=event.button, x=round(event.x), y=round(event.y), word=self.word_at(event.x, event.y))
        if event.button == 3:
            return self.dismiss()
        if event.button != 1 or not self.opened:
            return False
        index = self.word_at(event.x, event.y)
        if index >= 0:
            return self.select(index)
        if self.state.active is not None and self.detail_box and self.detail_box.contains(event.x, event.y):
            return self.select(self.state.active)
        return False

    def on_scroll(self, _widget, event):
        if self.detail_layout is None or not self.detail_box.contains(event.x, event.y):
            return False
        delta = {self.Gdk.ScrollDirection.UP: -1, self.Gdk.ScrollDirection.DOWN: 1}.get(event.direction)
        if delta is None:
            ok, _, dy = event.get_scroll_deltas()
            delta = dy if ok else 0
        limit = max(0, self.detail_h - self.detail_box.height)
        self.detail_scroll = clamp(self.detail_scroll + delta * 36, 0, limit)
        self.area.queue_draw()
        return True

    def on_key(self, _widget, event):
        Gdk = self.Gdk
        self.trace("key", name=Gdk.keyval_name(event.keyval))
        if event.keyval == Gdk.KEY_Escape:
            return self.dismiss()
        if not self.opened:
            return False
        if Gdk.KEY_1 <= event.keyval < Gdk.KEY_1 + len(self.rows):
            return self.select(event.keyval - Gdk.KEY_1)
        if event.state & Gdk.ModifierType.CONTROL_MASK and event.keyval in (Gdk.KEY_c, Gdk.KEY_C):
            state = self.state
            if state.active is not None:
                _, term, meaning = state.visible()[state.active]
                text = term + ("\n\n" + meaning if meaning else "")
            else:
                text = "\n".join(r[0] for r in self.rows)
            self.Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).set_text(text, -1)
            return True
        return False

    def select(self, index):
        if self.closed or not self.opened:
            return True
        if not all(self.sand.landed):
            self.sand.ef = max(self.sand.ef, .42 + .08 * len(self.rows))
        state = self.state
        events = state.advance(index)
        if not events:
            return True
        self.open_detail()
        box = self.detail_box
        ox, oy = self.plan.position
        self.trace("select", word=index, active=state.active, layer=state.layer, events=list(events),
                   detail=state.visible()[index][1:] if state.active is not None else None,
                   box=[round(ox + box.x), round(oy + box.y), round(box.width), round(box.height)] if box else None)
        simple = self.rows[index][0]
        self.helper._rendered(title=self.title, layout="sand", cues=state.visible(), active=state.active,
                              layer=state.layer, origin=self.origin)
        for event in events:
            self.helper._emit(event, window="dual", layer=state.layer, item_index=index, item_label=simple)
        if self.audio:
            pan = self.pan(self.plan.words[index][0])
            if "cue_collapse" in events:
                self.audio.pour(.5, 30, pan, pan, reverse=True, low=3000, high=8000, gain=.04)
                self.audio.arpeggio(index, up=False, pan=pan)
            else:
                self.audio.pour(.7, 50, pan, pan, low=3000, high=8000, gain=.05)
                self.audio.arpeggio(index, up=True, pan=pan)
        self.last_move = time.monotonic()
        self.wake()
        return True

    def action(self, action):
        self.helper._emit("click", window="dual", item_label=str(action.get("action", "")))
        sys.stdout.write(json.dumps({"action": action.get("action", "")}))
        sys.stdout.flush()
        return self.dismiss()

    def take_focus(self):
        self.window.present()
        # The compositor round trip takes tens of ms: never on the frame thread.
        import threading
        threading.Thread(target=self.helper.take_keyboard_focus, daemon=True).start()

    # --- the way back --------------------------------------------------------

    def poll(self):
        if self.closed or self.control is None:
            self.poll_id = None
            return False
        try:
            message = json.loads(self.control.read_text())
        except (OSError, ValueError):
            return True
        status = message.get("status")
        if status == "wait":
            return True
        self.poll_id = None
        if status == "done" and isinstance(message.get("payload"), dict):
            self.receive(message["payload"])
        else:
            self.dismiss(cancel=False)
        return False

    def dismiss(self, cancel=True):
        if self.closed:
            return True
        self.trace("dismiss", opened=self.opened, cancel=cancel)
        if cancel and self.control is not None and not self.opened:
            try:
                self.control.write_text(json.dumps({"status": "cancelled"}))
                self.trace("cancel_written", wall=round(time.time(), 3), path=str(self.control),
                           now=self.control.read_text())
            except OSError as exc:
                self.trace("cancel_failed", error=str(exc))
        self.detail_layout, self.streams = None, []
        if self.quiet:
            return self.finish()
        if self.audio:
            src = self.plan.source
            self.audio.play("back", .45, self.pan(src.x + src.width / 2))
            self.audio.pour(1, 70, .3, self.pan(src.x + src.width / 2), reverse=True, low=2500, high=7000, gain=.07)
        self.sand.back()
        self.wake()
        return True

    def finish(self):
        if self.closed:
            return True
        self.closed = True
        for source in (self.poll_id,):
            if source:
                self.GLib.source_remove(source)
        if self.tick_id is not None:
            self.area.remove_tick_callback(self.tick_id)
            self.tick_id = None
        if self.opened:
            self.helper._emit("window_close", window="dual", detail="sand returned", silent=True)
        else:
            self.helper._emit("window_close", window="sand", detail="back")
        self.window.destroy()
        self.Gtk.main_quit()
        if self.audio:
            self.audio.close(linger=1.0)
        for halo in self.halos:
            halo.close()
        if self.glow is not None:
            self.glow.close()
        self.trace("closed", opened=self.opened)
        self.report()
        return True

    def report(self):
        """Measured frame timing, only when asked for (live checks)."""
        import os
        target = os.environ.get("COGNITIVE_SAND_STATS")
        stats = self.stats
        if not target or not stats["frames"]:
            return
        Path(target).write_text(json.dumps({
            "frames": stats["frames"],
            "fps_while_animating": round(stats.get("active_frames", 0) / max(1e-6, stats.get("active_s", 0.0)), 1),
            "worst_frame_ms": round(stats.get("worst_ms", 0.0), 1),
            "step_ms": round(stats["step_s"] / stats["frames"] * 1000, 2),
            "paint_ms": round(stats["paint_s"] / stats["frames"] * 1000, 2)}))
