"""(prediction_nodes.py; not nodes.py, which holds the practice-task nodes.)

The reader's guess, marked, and four nodes to think from (2026-10-07).

    prediction -> ✓ / ≈ / ✗ -> four nodes -> the reader draws the edges, asks
    their own next question and makes a new prediction.

The model gives nodes, never links or an answer. What it writes is checked here
by script, not trusted: a node is dropped when it echoes the guess, sits in the
very sentence where the answer lies, or has been issued so often it fits any
paragraph. Fewer than three left means silence, which is a normal answer. A
doubtful "≈" is withheld rather than shown as a muddy verdict.
"""
from __future__ import annotations

import json
import os
import re
from collections import Counter
from pathlib import Path

OFF_FILE = "prediction-nodes.off"
HISTORY_FILE = "nodes-history.json"
MARK_SYMBOL = {"ok": "✓", "partly": "≈", "wrong": "✗"}
STATUS_OF_MARK = {"ok": "confirmed", "partly": "partially_confirmed", "wrong": "contradicted", "broken": "unclear"}
TEMPLATE_AFTER = 3          # a stem issued this many times already is generic
SHOWN_NODES = 4          # the window holds exactly four
CANDIDATES = 6
_WORD = re.compile(r"[A-Za-zА-Яа-яЁё]{3,}")


def enabled(root: str | Path | None = None) -> bool:
    base = Path(root or os.environ.get("COGNITIVE_STATE_DIR") or "~/.local/state/cognitive-popups").expanduser()
    return not (base / OFF_FILE).exists()


def _stem(word: str) -> str:
    return word.lower().replace("ё", "е")[:4]   # short enough that счёт and счёту meet


def stems(text: str) -> set[str]:
    return {_stem(w) for w in _WORD.findall(text or "")}


def _norm(text: str) -> str:
    return re.sub(r"\s+", " ", str(text or "").lower().replace("ё", "е")).strip(" .,;:!?«»\"'")


def load_history(root: str | Path) -> dict:
    try:
        data = json.loads((Path(root) / HISTORY_FILE).read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def remember(root: str | Path, words: list[str], limit: int = 200) -> None:
    history = load_history(root)
    issued = (history.get("issued") or [])[-limit:] + [list(words)]
    try:
        (Path(root) / HISTORY_FILE).write_text(json.dumps({"issued": issued}, ensure_ascii=False), encoding="utf-8")
    except OSError:
        pass


def recent_words(history: dict, sets: int = 3) -> list[str]:
    return [w for group in (history.get("issued") or [])[-sets:] for w in group]


def _gap_stems(gap: str, source: str) -> set[str]:
    """Stems of the quoted gap and of its whole sentence: that is where the answer lies."""
    s, g = _norm(source), _norm(gap)
    if not g or g not in s:
        return set()
    i = s.find(g)
    lo = max(s.rfind(".", 0, i), 0)
    hi = s.find(".", i + len(g))
    return stems(g) | stems(s[lo:hi if hi > 0 else len(s)])


def assemble(answer: dict, hypothesis: str, source: str, history: dict) -> dict:
    """Checked nodes and mark; `dropped` says what the script removed and why."""
    mark = answer.get("mark") if answer.get("mark") in {"ok", "partly", "wrong", "broken"} else "wrong"
    words = [w.strip() for w in (answer.get("words") or []) if isinstance(w, str) and w.strip()] \
        if answer.get("specific") else []
    freq = Counter(_stem(w) for group in (history.get("issued") or []) for w in group)
    barred = _gap_stems(str(answer.get("gap_quote") or ""), source)
    echo = stems(hypothesis)
    kept, dropped = [], {}
    for word in words[:CANDIDATES]:
        found = stems(word)
        if found & echo:
            dropped[word] = "эхо догадки"
        elif found & barred:
            dropped[word] = "из фразы с ответом"
        elif any(freq[s] >= TEMPLATE_AFTER for s in found):
            dropped[word] = "шаблон: уже много раз выдавалось"
        else:
            kept.append(word)
    shown_mark = "" if mark == "broken" or (mark == "partly" and not answer.get("sure")) else mark
    return {"mark": shown_mark, "raw_mark": mark,
            "words": kept[:SHOWN_NODES] if len(kept) >= SHOWN_NODES else [],
            "dropped": dropped}


def describe(mark: str, words: list[str]) -> str:
    """The text for the log and for the plain window: mark, then the nodes."""
    parts = [MARK_SYMBOL.get(mark, "")] if mark else []
    if words:
        parts.append(" · ".join(words))
    return "\n".join(parts) or "нечего добавить"
