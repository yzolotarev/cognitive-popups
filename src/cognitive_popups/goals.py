"""Ready-made reading directions and the passage they are grounded in.

The reader decides what they want from a text; this module only removes the work
of finding the words, and never the decision itself. Everything here is local:
no model, no network, no reading of the browser.
"""
from __future__ import annotations


#: Ready-made directions, in the order the window shows them. Variants, not a
#: ladder: choosing any of them is a complete answer to "why am I reading this",
#: and a narrower one ("обозначения", "различить похожее") can be typed by hand.
DIRECTIONS: tuple[dict[str, str], ...] = (
    {
        "key": "overview",
        "label": "Понять общую идею",
        "draft": "Понять общую идею материала и зачем он нужен",
    },
    {
        "key": "explain",
        "label": "Уметь объяснить другому",
        "draft": "Уметь объяснить это своими словами",
    },
    {
        "key": "useful",
        "label": "Где это пригодится",
        "draft": "Понять, где это пригодится",
    },
    {
        "key": "why",
        "label": "Понять, почему работает",
        "draft": "Понять, почему это работает",
    },
    {
        "key": "apply",
        "label": "Научиться применять",
        "draft": "Научиться применять это на примере",
    },
)

DIRECTION_LABELS: tuple[str, ...] = tuple(item["label"] for item in DIRECTIONS)

#: Beyond this the passage stops being "the place the reader is in": either the
#: model gets a section instead of a whole article, or the wording drifts to the
#: entire text. The window shows a notice; nothing is trimmed silently.
MAX_MATERIAL_CHARS = 12000


def direction_draft(direction: str, note: str = "") -> str:
    """Local phrase for a chosen direction, without any model call.

    The reader's own words win: when they typed a formulation, that is the goal,
    even if a ready-made label sits next to it.
    """
    text = " ".join((note or "").split()).strip()
    if text:
        return text
    label = (direction or "").strip()
    for item in DIRECTIONS:
        if item["label"] == label:
            return item["draft"]
    return ""


#: Where a captured passage came from. Shown next to the material so a wrong guess
#: is visible: a "selection" taken from the clipboard may be a copy from long ago.
ORIGIN_SELECTION = "выделение"
ORIGIN_CLIPBOARD = "буфер обмена"
ORIGIN_LAST_FRAGMENT = "последний фрагмент («4 слова»)"


def capture_origin(probe_name: str) -> str:
    """Name the visible source behind one successful capture probe."""
    return ORIGIN_CLIPBOARD if (probe_name or "").startswith("clipboard") else ORIGIN_SELECTION


def choose_material(captured: str, origin: str = "") -> tuple[str, str]:
    """Passage the reader just captured, and an honest label for its source.

    Only the reader's own act — a highlight or a fresh copy — is taken
    automatically. A fragment saved earlier under «4 слова» is never
    substituted silently; the direction window offers it as an explicit choice.
    """
    text = (captured or "").strip()
    if not text:
        return "", ""
    return text, (origin or "").strip() or ORIGIN_SELECTION


def material_notice(material: str) -> str:
    """A short warning when the passage is too large to word one goal from."""
    if len((material or "").strip()) > MAX_MATERIAL_CHARS:
        return "Материал большой: уточните формулировку по выбранному разделу."
    return ""
