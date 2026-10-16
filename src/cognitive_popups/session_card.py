"""Cutscene B «Итог сессии»: a still card of the session's own hypotheses.

Stars are the reader's hypotheses: a miss («ошибка, сохранена дословно») is
bright with a ray, a confirmed one is calm, notes are dim. The constellation
is named after the most frequent term passed in; one line below holds the
takeaway. Links are words two hypotheses share: no model, no Universe. The
layout is seeded by the session key, so the same session always draws the
same sky. No scores, no counts, no percentages, no reproach.

:func:`build_card` is pure and returns a standalone SVG; :func:`save_card`
files it under ``<state>/sky/``; :func:`show_card` is an optional GTK viewer.
"""
from __future__ import annotations

import hashlib
import html
import math
import random
import re
import time
from collections import Counter
from datetime import date as _date
from pathlib import Path

from . import motion, theme

WIDTH, HEIGHT = 640, 400
MISSES = {"contradicted", "not_supported"}
CONFIRMED = {"confirmed", "supported"}
EMPTY_TITLE = "сессия без гипотез"
FALLBACK_TITLE = "созвездие сессии"
#: Words too common to count as a shared idea between two hypotheses.
STOPWORDS = frozenset("""
это этот эта эти того тому того тоже также если когда потому чтобы который которая которые
можно нужно надо есть быть было были будет может могут просто очень более менее только
there their this that with from have will what when which into than then they them
""".split())
MAX_LINKS = 24
FADE_IN_S = 1.5


def _key8(session_key: str) -> str:
    return hashlib.sha256(str(session_key).encode("utf-8")).hexdigest()[:8]


def _seed(session_key: str) -> int:
    return int(hashlib.sha256(str(session_key).encode("utf-8")).hexdigest()[:12], 16)


def _text(item: dict) -> str:
    return str(item.get("hypothesis") or item.get("text") or item.get("comment") or "").strip()


def _terms(item: dict) -> list[str]:
    terms = item.get("terms") or []
    if isinstance(terms, str):
        terms = [terms]
    single = item.get("term")
    return [str(t).strip() for t in ([single] if single else []) + list(terms) if str(t).strip()]


#: Russian inflects: «программа» and «программу» are one word. The first six
#: letters stand in for a stem, which is crude but needs no model.
STEM = 6


def words(text: str) -> set[str]:
    """Stems of the content words of a hypothesis (at least four letters)."""
    return {w[:STEM] for w in re.findall(r"[^\W\d_]{4,}", text.lower()) if w not in STOPWORDS}


def constellation_name(hypotheses: list[dict], notes: list[dict]) -> str:
    """Most frequent term passed in; ties go to the term seen first."""
    counts, first = Counter(), {}
    for item in list(hypotheses) + list(notes):
        for term in _terms(item):
            counts[term] += 1
            first.setdefault(term, len(first))
    if not counts:
        return FALLBACK_TITLE if hypotheses else EMPTY_TITLE
    return min(counts, key=lambda term: (-counts[term], first[term]))


def links(hypotheses: list[dict]) -> list[tuple[int, int]]:
    """Pairs of hypotheses that share a content word, strongest first."""
    bags = [words(_text(h)) for h in hypotheses]
    pairs = [(len(bags[i] & bags[j]), i, j) for i in range(len(bags)) for j in range(i + 1, len(bags))
             if bags[i] & bags[j]]
    pairs.sort(key=lambda p: (-p[0], p[1], p[2]))
    return sorted((i, j) for _, i, j in pairs[:MAX_LINKS])


def kind(hypothesis: dict) -> str:
    status = str(hypothesis.get("status") or "").lower()
    return "miss" if status in MISSES else "confirmed" if status in CONFIRMED else "open"


def _positions(count: int, rnd: random.Random, box) -> list[list[float]]:
    x0, y0, x1, y1 = box
    points = [[rnd.uniform(x0, x1), rnd.uniform(y0, y1)] for _ in range(count)]
    # Spread them so each star reads (and hovers) on its own.
    min_d = 34.0
    for _ in range(60):
        for i in range(count):
            for j in range(i + 1, count):
                dx, dy = points[j][0] - points[i][0], points[j][1] - points[i][1]
                d = math.hypot(dx, dy) or 1e-3
                if d < min_d:
                    push = (min_d - d) / 2
                    points[i][0] -= dx / d * push; points[i][1] -= dy / d * push
                    points[j][0] += dx / d * push; points[j][1] += dy / d * push
        for p in points:
            p[0] = min(x1, max(x0, p[0])); p[1] = min(y1, max(y0, p[1]))
    return [[round(x, 1), round(y, 1)] for x, y in points]


def _esc(text: str) -> str:
    """XML-escaped; '%' becomes a character reference so no card ever shows a
    percentage-looking token in its source (the reader's own text is kept)."""
    return html.escape(str(text)).replace("%", "&#37;")


def _one_line(text: str, limit: int = 72) -> str:
    text = " ".join(str(text).split())
    return text if len(text) <= limit else text[:limit - 1].rstrip() + "…"


def build_card(hypotheses: list[dict], notes: list[dict], session_key: str, takeaway: str | None) -> str:
    """The session card as a standalone SVG string (dark paper, theme tokens)."""
    c = theme.DARK
    hypotheses = [h for h in hypotheses if _text(h)]
    notes = [n for n in notes if _text(n)]
    rnd = random.Random(_seed(session_key))
    sky = (48, 92, WIDTH - 48, HEIGHT - 96)
    stars = _positions(len(hypotheses) + len(notes), rnd, sky)
    title = constellation_name(hypotheses, notes)
    font = f'font-family="{html.escape(theme.FONT_FAMILY)}"'
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" '
           f'viewBox="0 0 {WIDTH} {HEIGHT}" data-key="{_key8(session_key)}">',
           f'<title>{_esc(title)}</title>',
           f'<rect x="0" y="0" width="{WIDTH}" height="{HEIGHT}" rx="14" fill="{c["paper"]}"/>']
    # Links first, under the stars: shared words, nothing inferred.
    for i, j in links(hypotheses):
        (ax, ay), (bx, by) = stars[i], stars[j]
        out.append(f'<line x1="{ax}" y1="{ay}" x2="{bx}" y2="{by}" stroke="{c["ink_muted"]}" '
                   f'stroke-width="0.8" opacity="0.4"/>')
    for n, note in enumerate(notes):
        x, y = stars[len(hypotheses) + n]
        out.append(f'<circle cx="{x}" cy="{y}" r="1.6" fill="{c["ink_muted"]}" opacity="0.45">'
                   f'<title>{_esc(_text(note))}</title></circle>')
    for h, hypothesis in enumerate(hypotheses):
        x, y = stars[h]
        what = kind(hypothesis)
        label = f'<title>{_esc(_text(hypothesis))}</title>'
        if what == "miss":
            angle = rnd.uniform(0, math.tau)
            rays = "".join(
                f'<line x1="{x}" y1="{y}" x2="{round(x + math.cos(angle + k * math.pi / 2) * (16 if k % 2 == 0 else 10), 1)}" '
                f'y2="{round(y + math.sin(angle + k * math.pi / 2) * (16 if k % 2 == 0 else 10), 1)}" '
                f'stroke="{c["miss"]}" stroke-width="0.9" opacity="0.7"/>' for k in range(4))
            out.append(f'<g data-kind="miss">{label}<circle cx="{x}" cy="{y}" r="11" fill="{c["miss"]}" opacity="0.12"/>'
                       f'{rays}<circle cx="{x}" cy="{y}" r="4.2" fill="{c["miss"]}"/></g>')
        elif what == "confirmed":
            out.append(f'<g data-kind="confirmed">{label}<circle cx="{x}" cy="{y}" r="3" fill="{c["ink"]}" opacity="0.85"/></g>')
        else:
            out.append(f'<g data-kind="open">{label}<circle cx="{x}" cy="{y}" r="2.6" fill="{c["ink_muted"]}"/></g>')
    out.append(f'<text x="32" y="50" fill="{c["ink"]}" {font} font-size="{theme.TITLE}">{_esc(title)}</text>')
    if takeaway and takeaway.strip():
        out.append(f'<text x="32" y="{HEIGHT - 36}" fill="{c["ink_muted"]}" {font} font-size="{theme.TEXT}">'
                   f'{_esc(_one_line(takeaway))}</text>')
    out.append('</svg>')
    return "".join(out)


def save_card(svg: str, state_dir, *, day: str | None = None) -> Path:
    """Write the card to ``<state_dir>/sky/<date>-<key8>.svg`` and return the path.

    The same session on the same day overwrites its own card: the sky grows
    by sessions, never by duplicates.
    """
    match = re.search(r'data-key="([0-9a-f]{8})"', svg)
    key8 = match.group(1) if match else hashlib.sha256(svg.encode("utf-8")).hexdigest()[:8]
    folder = Path(state_dir).expanduser() / "sky"
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{day or _date.today().isoformat()}-{key8}.svg"
    tmp = path.with_suffix(".svg.tmp")
    tmp.write_text(svg, encoding="utf-8")
    tmp.replace(path)
    return path


def fade_alpha(elapsed: float, reduced: bool = False) -> float:
    """Window opacity of the viewer: ≤ 1.5 s fade-in, instant when reduced."""
    if reduced:
        return 1.0
    return min(1.0, max(0.0, elapsed / FADE_IN_S))


def show_card(path, on_done=None):
    """Show a saved card inside the running GTK loop; any key or click closes it.

    Returns the window, or None without GTK (then ``on_done`` runs at once).
    """
    try:
        import gi
        gi.require_version("Gtk", "3.0")
        gi.require_version("GdkPixbuf", "2.0")
        from gi.repository import Gdk, GdkPixbuf, GLib, Gtk
    except (ImportError, ValueError):
        if on_done:
            on_done()
        return None
    reduced = motion.reduced(Gtk)
    window = Gtk.Window(title="Итог сессии")
    try:
        window.set_wmclass("cognitive-popup", "cognitive-popup")
    except Exception:  # noqa: BLE001 - deprecated API, naming is optional
        pass
    window.set_decorated(False)
    window.set_resizable(False)
    window.set_keep_above(True)
    window.set_skip_taskbar_hint(True)
    window.set_skip_pager_hint(True)
    image = Gtk.Image.new_from_pixbuf(GdkPixbuf.Pixbuf.new_from_file(str(path)))
    window.add(image)
    state = {"closed": False, "source": None, "started": time.monotonic()}

    def close(*_):
        if state["closed"]:
            return True
        state["closed"] = True
        if state["source"]:
            GLib.source_remove(state["source"])
            state["source"] = None
        window.destroy()
        if on_done:
            on_done()
        return True

    def fade():
        alpha = fade_alpha(time.monotonic() - state["started"], reduced)
        window.set_opacity(alpha)
        if alpha >= 1.0:
            state["source"] = None
            return False
        return True

    window.add_events(Gdk.EventMask.BUTTON_PRESS_MASK | Gdk.EventMask.KEY_PRESS_MASK)
    window.connect("key-press-event", close)
    window.connect("button-press-event", close)
    window.connect("delete-event", close)
    window.set_position(Gtk.WindowPosition.CENTER)
    window.set_opacity(fade_alpha(0.0, reduced))
    window.show_all()
    if not reduced:
        state["source"] = GLib.timeout_add(30, fade)
    return window


def main(argv=None) -> int:
    """``python -m cognitive_popups.session_card <card.svg>``: view one card."""
    import sys
    args = list(sys.argv[1:] if argv is None else argv)
    if not args:
        print("usage: python -m cognitive_popups.session_card <card.svg>", file=sys.stderr)
        return 2
    try:
        import gi
        gi.require_version("Gtk", "3.0")
        from gi.repository import Gtk
    except (ImportError, ValueError):
        print("GTK is not available", file=sys.stderr)
        return 1
    if show_card(args[0], on_done=Gtk.main_quit) is None:
        return 1
    Gtk.main()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
