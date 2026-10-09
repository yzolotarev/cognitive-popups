"""Display-independent, logical-pixel geometry and disclosure state."""
from dataclasses import dataclass
import math


@dataclass(frozen=True)
class Rect:
    x: int
    y: int
    width: int
    height: int


@dataclass(frozen=True)
class Orbit:
    footprint: Rect
    cues: tuple[Rect, ...]
    nucleus: tuple[float, float]
    details: tuple[Rect, ...]


def layout(workarea, anchor, width=800, height=720):
    """Bounded radial reservation in monitor logical coordinates."""
    ax, ay, aw, ah = map(int, workarea)
    scale = min(1., max(1, aw) / 800, max(1, ah) / 720)
    w, h = max(1, round(800 * scale)), max(1, round(720 * scale))
    x = min(max(ax, round(anchor[0] - w / 2)), ax + aw - w)
    y = min(max(ay, round(anchor[1] - h / 2)), ay + ah - h)
    def scaled(rx, ry, rw, rh):
        left, top = min(w-1, round(rx*scale)), min(h-1, round(ry*scale))
        return Rect(left, top, max(1, min(w-left, round(rw*scale))),
                    max(1, min(h-top, round(rh*scale))))
    cues = tuple(scaled(400+dx-57.5, 360+dy-52.5, 115, 105)
                 for dx, dy in ((0,-128),(128,0),(0,128),(-128,0)))
    details = tuple(scaled(*r) for r in ((260,12,280,230), (518,245,280,230),
                                        (260,478,280,230), (2,245,280,230)))
    return Orbit(Rect(x,y,w,h), cues, (w/2,h/2), details)


def disclosure_rect(orbit, index, fraction=1.):
    """Shrink outward reservation without moving its inward edge or cue."""
    rect = orbit.details[index]
    w, h = max(1, round(rect.width*fraction)), max(1, round(rect.height*fraction))
    x, y = rect.x+(rect.width-w)//2, rect.y+(rect.height-h)//2
    if index == 0:
        y = rect.y+rect.height-h
    elif index == 1:
        x = rect.x
    elif index == 2:
        y = rect.y
    else:
        x = rect.x+rect.width-w
    return Rect(x,y,w,h)


def safe_rect(rect, index, padding=4, cue=False):
    """Inset certified opaque rectangle, with filtering/rounding guard."""
    from .orbital_assets import safe_insets
    l,t,r,b = safe_insets(cue)[index]
    padding = min(padding, max(0, min(rect.width, rect.height)//8))
    x = min(rect.x+rect.width-1, math.ceil(rect.x+l*rect.width)+padding)
    y = min(rect.y+rect.height-1, math.ceil(rect.y+t*rect.height)+padding)
    right = min(rect.x+rect.width, max(x+1, math.floor(rect.x+r*rect.width)-padding))
    bottom = min(rect.y+rect.height, max(y+1, math.floor(rect.y+b*rect.height)-padding))
    return Rect(x,y,right-x,bottom-y)


def blob_points(rect, phase=0, count=64):
    """Deterministic soft organic boundary, strictly inside its allocation."""
    points = []
    for i in range(count):
        a = math.tau * i / count
        radius = .91 + .035 * math.sin(3 * a + phase) + .025 * math.cos(5 * a - phase)
        points.append((rect.x + rect.width / 2 * (1 + radius * math.cos(a)),
                       rect.y + rect.height / 2 * (1 + radius * math.sin(a))))
    return tuple(points)


FOCUS_LEVELS = {'off': 0., 'subtle': .14, 'normal': .28, 'strong': .46}


def focus_level(value):
    value = str(value or '').strip().lower()
    return value if value in FOCUS_LEVELS else 'subtle'


def _rounded_path(points):
    """Quadratic joins soften a continuous cue-to-detail organic silhouette."""
    mids = [((p[0] + points[(i + 1) % len(points)][0]) / 2,
             (p[1] + points[(i + 1) % len(points)][1]) / 2)
            for i, p in enumerate(points)]
    path = [f'M {mids[-1][0]:.2f},{mids[-1][1]:.2f}']
    for p, mid in zip(points, mids):
        path.append(f'Q {p[0]:.2f},{p[1]:.2f} {mid[0]:.2f},{mid[1]:.2f}')
    return ' '.join(path) + ' Z'


def expansion_path(orbit, index):
    """One connected outward extension; the cue's center never moves."""
    cue, detail = orbit.cues[index], orbit.details[index]
    # Upper petals widen upward; lower petals are the mirrored construction.
    upper = index < 2
    outer = detail.y if upper else detail.y + detail.height
    inner = detail.y + detail.height if upper else detail.y
    near = cue.y if upper else cue.y + cue.height
    tip = cue.y + cue.height if upper else cue.y
    return _rounded_path([
        (detail.x, outer), (detail.x + detail.width, outer),
        (detail.x + detail.width, inner), (cue.x + cue.width, near),
        (cue.x + cue.width, tip), (cue.x, tip), (cue.x, near), (detail.x, inner),
    ])


def background_svg(orbit, active=None, focus='subtle'):
    """Sparse graphite material; focus is local, never a screen-wide scrim."""
    box = orbit.footprint
    cx, cy = orbit.nucleus
    peak = FOCUS_LEVELS[focus_level(focus)]
    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{box.width}" '
             f'height="{box.height}">']
    if peak:
        from .orbital_assets import focus_image_uri
        parts.append(f'<image data-focus="{focus_level(focus)}" x="0" y="0" '
                     f'width="{box.width}" height="{box.height}" '
                     f'href="{focus_image_uri(box.width, box.height, peak)}"/>')
    from .orbital_assets import image_uri
    if active is not None:
        rect = orbit.details[active]
        parts.append(f'<image x="{rect.x}" y="{rect.y}" width="{rect.width}" '
                     f'height="{rect.height}" preserveAspectRatio="none" href="{image_uri(active)}"/>')
    for i, rect in enumerate(orbit.cues):
        opacity = '1' if active in (None, i) else '.62'
        parts.append(f'<image data-cue="{i}" x="{rect.x}" y="{rect.y}" '
                     f'width="{rect.width}" height="{rect.height}" preserveAspectRatio="none" '
                     f'opacity="{opacity}" href="{image_uri(i)}"/>')
    parts.append(f'<circle cx="{cx}" cy="{cy}" r="2" fill="#dddddd"/>')
    parts.append('</svg>')
    return ''.join(parts)


class Disclosure:
    """At most one active cue; selecting another restores simple-only cues.

    Disclosure is reversible: clicking a fully open cue collapses it.
    """
    def __init__(self, rows):
        self.rows = tuple(rows)
        self.active = None
        self.layer = 1

    def advance(self, index):
        if not 0 <= index < len(self.rows):
            return ()
        if self.active != index:
            self.active, self.layer = index, 2
            return ('cue_select', 'term_reveal')
        if self.layer == 2 and self.rows[index][2]:
            self.layer = 3
            return ('meaning_reveal',)
        # Fully open: one more click folds it back to the four plain cues.
        self.active, self.layer = None, 1
        return ('cue_collapse',)

    def visible(self):
        return tuple((simple, term if self.active == i and self.layer >= 2 else '',
                      meaning if self.active == i and self.layer >= 3 else '')
                     for i, (simple, term, meaning) in enumerate(self.rows))
