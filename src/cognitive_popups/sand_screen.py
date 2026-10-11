"""What is under the sand: one local screenshot, never stored or sent anywhere.

From it the words window learns three things without any app integration:

* where the selection is (the highlight colour around the pointer), so the
  words can sit at its four corners and the sand can peel off its letters;
* where the letters of the selection are, for the lift and the way back;
* whether each point is light or dark, so grains and text take the opposite
  ink (owner's idea of 10.10.2026: like the Minecraft crosshair).

The screenshot is ``grim`` of the whole layout in physical pixels, which are
the X11 coordinates the helper windows use (XWayland, unscaled).
"""
from __future__ import annotations

import shutil
import subprocess
from dataclasses import dataclass

LIGHT_INK = (0xf2, 0xe6, 0xcf)
DARK_INK = (0x2a, 0x20, 0x16)
#: Green channel above this counts as a light background.
LIGHT_BG = 140
STEP = 4            # flood-fill grid of the selection search, in pixels
MAX_CELLS = 15000   # bigger than this (~240k px²) is a panel or the page itself


@dataclass(frozen=True)
class Shot:
    rgb: bytes
    width: int
    height: int

    def pixel(self, x, y):
        i = (int(y) * self.width + int(x)) * 3
        return self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]

    def inside(self, x, y):
        return 0 <= x < self.width and 0 <= y < self.height


def parse_ppm(data: bytes) -> Shot | None:
    if not data.startswith(b"P6"):
        return None
    fields, index = [], 2
    while len(fields) < 3:
        while data[index:index + 1].isspace():
            index += 1
        if data[index:index + 1] == b"#":
            index = data.index(b"\n", index) + 1
            continue
        end = index
        while not data[end:end + 1].isspace():
            end += 1
        fields.append(int(data[index:end]))
        index = end
    width, height = fields[0], fields[1]
    body = data[index + 1:index + 1 + width * height * 3]
    return Shot(body, width, height) if len(body) == width * height * 3 else None


def start_grab():
    """Begin the screenshot early; ``finish_grab`` collects it (overlaps GTK start-up)."""
    if not shutil.which("grim"):
        return None
    try:
        return subprocess.Popen(["grim", "-t", "ppm", "-"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    except OSError:
        return None


def finish_grab(process, timeout=1.0) -> Shot | None:
    if process is None:
        return None
    try:
        data, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        return None
    return parse_ppm(data) if data else None


def _quant(colour):
    return tuple(c >> 4 for c in colour)


def _near(a, b, tolerance=20):
    return abs(a[0] - b[0]) <= tolerance and abs(a[1] - b[1]) <= tolerance and abs(a[2] - b[2]) <= tolerance


def _mode(shot, points):
    counts = {}
    for x, y in points:
        if shot.inside(x, y):
            q = _quant(shot.pixel(x, y))
            counts[q] = counts.get(q, 0) + 1
    return counts


def _flood(shot, cx, cy, colour, limit, tolerance=24):
    """Cells of ``colour`` connected to the pointer line; None when it spills over ``limit``."""
    cache = {}

    def match(gx, gy):
        key = (gx, gy)
        if key not in cache:
            x, y = gx * STEP, gy * STEP
            cache[key] = shot.inside(x, y) and _near(shot.pixel(x, y), colour, tolerance)
        return cache[key]

    def filled(gx, gy):
        # glyphs inside the highlight, and the thin gap some apps leave between
        # highlighted lines (up to ~8 px), are bridged; a paragraph gap is not
        return (match(gx, gy) or (match(gx - 1, gy) and match(gx + 1, gy))
                or any(match(gx, gy - a) and match(gx, gy + b) for a, b in ((1, 1), (1, 2), (2, 1))))

    start = next(((gx, gy) for r in range(16) for gx, gy in
                  ((cx // STEP + d, cy // STEP + e) for d in range(-r, r + 1) for e in (-1, 0, 1))
                  if match(gx, gy)), None)
    if start is None:
        return None
    seen, stack = {start}, [start]
    x0 = x1 = start[0]
    y0 = y1 = start[1]
    while stack:
        gx, gy = stack.pop()
        x0, x1, y0, y1 = min(x0, gx), max(x1, gx), min(y0, gy), max(y1, gy)
        for nx, ny in ((gx + 1, gy), (gx - 1, gy), (gx, gy + 1), (gx, gy - 1)):
            if (nx, ny) not in seen and filled(nx, ny):
                seen.add((nx, ny))
                stack.append((nx, ny))
                if len(seen) > limit:
                    return None
    return len(seen), (x0 * STEP, y0 * STEP, (x1 + 1) * STEP, (y1 + 1) * STEP)


def _clusters(shot, points, tolerance=16):
    """Colours of ``points`` merged within ``tolerance``: [(count, colour)], commonest first."""
    clusters = []
    for x, y in points:
        if not shot.inside(x, y):
            continue
        pixel = shot.pixel(x, y)
        for cluster in clusters:
            if _near(cluster[1], pixel, tolerance):
                cluster[0] += 1
                break
        else:
            clusters.append([1, pixel])
    return sorted(clusters, key=lambda c: -c[0])


def find_selection(shot: Shot, cx: int, cy: int):
    """``((x0, y0, x1, y1), highlight colour)`` of the selection at the pointer, or None.

    Candidates are the commonest colours right at the pointer line. Each is
    flood-filled: glyph colours give small blobs, the page spills over the
    limit, the highlight gives the biggest region that stays bounded.
    """
    if shot is None or not shot.inside(cx, cy):
        return None
    probe = [(cx + dx, cy + dy) for dx in range(-60, 61, 2) for dy in range(-6, 7, 2)]
    near = _clusters(shot, probe)
    total = sum(c[0] for c in near) or 1
    limit = min(MAX_CELLS, int(shot.width * shot.height / STEP / STEP * .35))
    best = None
    for count, colour in near[:5]:
        if count / total < .08:
            break
        region = _flood(shot, cx, cy, colour, limit)
        if region is None:
            continue
        cells, box = region
        if box[2] - box[0] >= 16 and box[3] - box[1] >= 8 and (best is None or cells > best[0]):
            best = (cells, box, colour)
    return (best[1], best[2]) if best else None


def glyph_points(shot, box, highlight, n, rnd):
    """Points on the letters inside the selection: neither highlight nor page."""
    x0, y0, x1, y1 = box
    points = []
    for _ in range(n * 12):
        x, y = rnd.uniform(x0, x1), rnd.uniform(y0, y1)
        if shot.inside(x, y) and not _near(shot.pixel(x, y), highlight, 40):
            points.append((x, y))
            if len(points) >= n:
                break
    return points


def light_map(shot, rect):
    """One byte per pixel of ``rect`` (x, y, w, h): 1 where the background is light."""
    x, y, w, h = (int(v) for v in rect)
    table = bytes(1 if v > LIGHT_BG else 0 for v in range(256))
    rows = []
    blank = bytes(w)
    for row in range(y, y + h):
        if 0 <= row < shot.height:
            a, b = max(0, x), min(shot.width, x + w)
            start = (row * shot.width + a) * 3 + 1
            line = shot.rgb[start:start + (b - a) * 3:3].translate(table)
            rows.append(bytes(a - x) + line + bytes(x + w - b) if (a, b) != (x, x + w) else line)
        else:
            rows.append(blank)
    return b"".join(rows)


def contrast_bgra(lights: bytes, alpha=255):
    """Premultiplied ARGB32 (little endian BGRA) with the ink opposite to the page."""
    n = len(lights)
    out = bytearray(4 * n)
    for channel, (dark, light) in enumerate(zip(DARK_INK[::-1], LIGHT_INK[::-1])):
        # light background (1) → dark ink, dark background (0) → light ink
        table = bytes(light if v == 0 else dark for v in range(256))
        out[channel::4] = lights.translate(table)
    out[3::4] = bytes([alpha]) * n
    return out


def mean_colour(shot, rect, step=3, exclude=None):
    """Average page colour inside ``rect`` (x, y, w, h); None outside the screenshot.

    ``exclude`` (the selection's highlight) is not page: it is skipped, unless
    it is all there is.
    """
    x, y, w, h = (int(v) for v in rect)
    total = [0, 0, 0]
    count = skipped = 0
    for yy in range(max(0, y), min(shot.height, y + h), step):
        for xx in range(max(0, x), min(shot.width, x + w), step):
            r, g, b = shot.pixel(xx, yy)
            if exclude is not None and _near((r, g, b), exclude, 30):
                skipped += 1
                continue
            total[0] += r
            total[1] += g
            total[2] += b
            count += 1
    if not count and skipped:
        return mean_colour(shot, rect, step)
    return tuple(v / count for v in total) if count else None


def ink_for(colour):
    """Dark ink on a light page, light ink on a dark one."""
    r, g, b = colour
    return DARK_INK if .2126 * r + .7152 * g + .0722 * b > LIGHT_BG else LIGHT_INK
