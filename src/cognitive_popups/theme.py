"""Design tokens: the one place for font, sizes and colours.

Every window owned by the visual layer takes its look from here, so the
product reads as one system: one family, one size scale (nothing under
11 px), one amber, and text that keeps a contrast of at least 4.5:1 on the
paper it sits on, in the dark and in the light variant alike.

Pure stdlib, no GTK: the tokens are plain strings and tuples.
"""
from __future__ import annotations

#: Family list as CSS and Pango both accept it; "Sans" is the fallback.
FONT = "Noto Sans"
FONT_FALLBACK = "Sans"
FONT_FAMILY = f"{FONT}, {FONT_FALLBACK}"
CSS_FONT_FAMILY = f'"{FONT}", "{FONT_FALLBACK}", sans-serif'

#: The only sizes in use, in px. Captions never go below SMALL.
SMALL, TEXT, LARGE, TITLE = 11, 13, 15, 18
SIZES = (SMALL, TEXT, LARGE, TITLE)
MIN_TEXT_PX = SMALL

#: ``paper`` is the surface, ``paper_raised`` a card on it; ``ink`` the text,
#: ``ink_muted`` captions; ``accent`` the single amber; ``own_words`` marks the
#: reader's own text; ``miss`` highlights a saved mistake calmly (not red);
#: ``line`` is for strokes and tracks, never for text.
DARK = {
    "paper": "#17181c",
    "paper_raised": "#222328",
    "ink": "#eee1c7",
    "ink_muted": "#b9ae9a",
    "accent": "#d6ad79",
    "own_words": "#9fc3e8",
    "miss": "#c3b1ee",
    "line": "#3b3c42",
}
LIGHT = {
    "paper": "#f7f3eb",
    "paper_raised": "#fffdf8",
    "ink": "#2a2016",
    "ink_muted": "#5c5040",
    # The same amber, darkened until it reads as text on light paper.
    "accent": "#7d5826",
    "own_words": "#2f5f8f",
    "miss": "#5b3fa6",
    "line": "#d8cfbf",
}
PALETTES = {"dark": DARK, "light": LIGHT}

#: Text tokens and the surfaces they are drawn on; ``tests/test_theme.py``
#: holds each pair to 4.5:1 in both variants.
TEXT_TOKENS = ("ink", "ink_muted", "accent", "own_words", "miss")
SURFACES = ("paper", "paper_raised")
TEXT_PAIRS = tuple((fg, bg) for fg in TEXT_TOKENS for bg in SURFACES)

#: Orbital's material is deliberately hueless graphite (its focus field and
#: nucleus); this is its one light grey, never used for text.
GRAPHITE = "#dddddd"

#: Decorative star tints (never text): the per-session constellation hue and
#: the persisted Initium identity. Kept verbatim so saved identities still match.
STAR_TINTS = ("#f5c4b3", "#b5d4f4", "#fac775", "#cecbf6", "#9fe1cb")
INITIUM_TINTS = ("#f3e0bd", "#cecbf6", "#b5d4f4", "#9fe1cb")

#: Above this mean brightness (0-255, Rec. 709 weights on sRGB values) a page
#: counts as light. The sand used the same threshold before the tokens existed.
LIGHT_PAGE_BRIGHTNESS = 150


def rgb255(colour: str) -> tuple[int, int, int]:
    """``#rrggbb`` → (r, g, b) in 0..255."""
    value = colour.lstrip("#")
    if len(value) != 6:
        raise ValueError(f"not a #rrggbb colour: {colour!r}")
    return tuple(int(value[i:i + 2], 16) for i in (0, 2, 4))


def rgb(colour: str) -> tuple[float, float, float]:
    """``#rrggbb`` → (r, g, b) in 0..1, as cairo wants it."""
    return tuple(v / 255 for v in rgb255(colour))


def css_rgba(colour: str, alpha: float) -> str:
    r, g, b = rgb255(colour)
    return f"rgba({r},{g},{b},{alpha:g})"


def _channel(value: float) -> float:
    value /= 255
    return value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4


def luminance(colour) -> float:
    """WCAG relative luminance of ``#rrggbb`` or an (r, g, b) 0..255 tuple."""
    r, g, b = rgb255(colour) if isinstance(colour, str) else colour
    return 0.2126 * _channel(r) + 0.7152 * _channel(g) + 0.0722 * _channel(b)


def contrast(a, b) -> float:
    """WCAG contrast ratio between two colours, 1..21."""
    la, lb = luminance(a), luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def brightness(colour) -> float:
    """Mean perceived brightness 0..255 (what the sand measures on screenshots)."""
    r, g, b = rgb255(colour) if isinstance(colour, str) else colour
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def is_light(background) -> bool:
    """A colour, an (r, g, b) tuple or a brightness number: is it a light page?"""
    value = background if isinstance(background, (int, float)) else brightness(background)
    return value > LIGHT_PAGE_BRIGHTNESS


def tone(background=None) -> str:
    """``"light"`` or ``"dark"`` for a background; unknown → dark (the desktop)."""
    return "light" if background is not None and is_light(background) else "dark"


def pick(background=None) -> dict[str, str]:
    """The palette that reads on ``background`` (colour, tuple or brightness)."""
    return PALETTES[tone(background)]


def palette(name: str = "dark") -> dict[str, str]:
    return PALETTES["light" if name == "light" else "dark"]


def pango(size: float = TEXT, weight: str = "", *, px: bool = True) -> str:
    """A Pango font description string with the fallback family list.

    ``px=False`` keeps a size in points, for the large display words whose
    scale is set by the sand layout itself.
    """
    unit = "px" if px else ""
    style = f" {weight}" if weight else ""
    return f"{FONT_FAMILY}{style} {size:g}{unit}"


def css_font(size: int = TEXT, weight: str = "normal") -> str:
    """CSS declarations for the family and a size from the scale."""
    return f"font-family: {CSS_FONT_FAMILY}; font-size: {max(MIN_TEXT_PX, size)}px; font-weight: {weight};"
