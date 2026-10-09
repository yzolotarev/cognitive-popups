"""What the model sees for one request: only what this moment needs.

The constant instructions stay small; before each request the app hands over a
small pack: the passage selected now (what the question points at), up to three
passages the reader took just before (the surroundings, so "this principle"
can find what it refers to), and the running step's goal.

The recent passages come from a short ring the app fills whenever the reader
takes text: four words, a question, a hypothesis, a note. Only passages from the
running step count, or, with no step, from the last half hour, so a paragraph
from another article or from an unrelated chat does not slip into the answer.
The order given is the order the reader took them, which is not always the
order of the text: the pack says so instead of pretending otherwise.
"""
from __future__ import annotations

import json
import os
import re
import time
from dataclasses import dataclass, field
from pathlib import Path

RING_FILE = "recent-passages.json"
RING_SIZE = 12
WINDOW_SECONDS = 30 * 60
MIN_PASSAGE_CHARS = 40
#: Two passages with this many identical opening characters are one passage.
SAME_START = 120
RECENT_LIMIT = 3


def state_root(root: str | Path | None = None) -> Path:
    return Path(root or os.environ.get("COGNITIVE_STATE_DIR")
                or "~/.local/state/cognitive-popups").expanduser()


def _fold(text: str) -> str:
    return re.sub(r"\s+", " ", (text or "").casefold()).strip()


def same_passage(a: str, b: str) -> bool:
    """The same piece taken twice: one inside the other, or the same opening.

    Re-selecting a paragraph rarely gives identical text (a trailing word, a
    line break), so exact equality would keep both copies.
    """
    left, right = _fold(a), _fold(b)
    if not left or not right:
        return False
    return left in right or right in left or left[:SAME_START] == right[:SAME_START]


def running_step(root: str | Path | None = None) -> dict:
    try:
        state = json.loads((state_root(root) / "focus.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return state if isinstance(state, dict) and state.get("status") == "running" else {}


def _load(path: Path) -> list[dict]:
    try:
        items = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    return [item for item in items if isinstance(item, dict) and item.get("text")] if isinstance(items, list) else []


def remember(text: str, tool: str, *, root: str | Path | None = None, now: float | None = None) -> None:
    """Note a passage the reader just took. Short picks (a word) are not passages."""
    passage = " ".join((text or "").split())
    if len(passage) < MIN_PASSAGE_CHARS:
        return
    path = state_root(root) / RING_FILE
    items = [item for item in _load(path) if not same_passage(item["text"], passage)]
    items.append({"text": passage, "at": now if now is not None else time.time(), "tool": tool,
                  "step": running_step(root).get("id", "")})
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(path.name + ".tmp")
        temporary.write_text(json.dumps(items[-RING_SIZE:], ensure_ascii=False), encoding="utf-8")
        os.replace(temporary, path)
    except OSError:
        pass


def ago(seconds: float) -> str:
    minutes = int(seconds // 60)
    return "только что" if minutes < 1 else f"{minutes} мин назад"


@dataclass
class Pack:
    selection: str
    recent: list[dict] = field(default_factory=list)
    step: str = ""
    now: float = 0.0

    def passages(self) -> list[str]:
        """Every text the model may quote from: the recent ones and the selection."""
        return [item["text"] for item in self.recent] + ([self.selection] if self.selection else [])

    def recent_block(self) -> str:
        """Oldest first, with how long ago, labelled as the reader's order."""
        lines = [f"{number}) {ago(self.now - item['at'])}: {item['text']}"
                 for number, item in enumerate(self.recent, 1)]
        return "\n\n".join(lines)

    def material(self) -> str:
        """One text for a check that quotes verbatim: surroundings, then the point."""
        parts = []
        if self.recent:
            parts.append("=== РАНЬШЕ (в том порядке, как читатель их брал; не обязательно порядок текста) ===\n"
                         + self.recent_block())
        if self.selection:
            parts.append("=== СЕЙЧАС ВЫДЕЛЕНО (на это указывает догадка) ===\n" + self.selection)
        return "\n\n".join(parts)


def pack(selection: str, *, root: str | Path | None = None, now: float | None = None,
         limit: int = RECENT_LIMIT) -> Pack:
    moment = now if now is not None else time.time()
    chosen = " ".join((selection or "").split())
    step = running_step(root)
    key = _fold(chosen)
    recent = []
    for item in _load(state_root(root) / RING_FILE):
        if step.get("id"):
            if item.get("step") != step["id"]:
                continue
        elif moment - float(item.get("at", 0)) > WINDOW_SECONDS:
            continue
        text = _fold(item["text"])
        if key and text in key:
            continue  # the selection itself, or wholly inside it; a larger
            # paragraph that merely contains the selection stays: it is the surroundings
        recent.append(item)
    return Pack(selection=chosen, recent=recent[-limit:], step=str(step.get("goal", "")), now=moment)
