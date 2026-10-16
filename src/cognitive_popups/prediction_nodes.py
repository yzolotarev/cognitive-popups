"""(prediction_nodes.py; not nodes.py, which holds the practice-task nodes.)

The reader's guess, marked, and four nodes to think from (2026-10-07).

    prediction -> ✓ / ≈ / ? -> four nodes -> the reader draws the edges, asks
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
from . import paths

OFF_FILE = "prediction-nodes.off"
HISTORY_FILE = "nodes-history.json"
# A possible mismatch invites another thought; it is not a prohibition or a
# reliable verdict on the reader. Keep the model's raw mark in the ledger.
MARK_SYMBOL = {"ok": "✓", "partly": "≈", "wrong": "?"}
MARK_WORD = {"ok": "держится", "partly": "частично", "wrong": "расходится с текстом"}
STATUS_OF_MARK = {"ok": "confirmed", "partly": "partially_confirmed", "wrong": "contradicted", "broken": "unclear",
                  "none": "unclear"}
#: A guess nothing in the text could refute is not marked (2026-10-09): the window
#: asks for a bolder one once, then marks whatever comes. The guess itself is kept.
VAGUE_STATUS = "vague"
BOLDER = "Смелее: что именно? Скажи то, в чём можно ошибиться."
TEMPLATE_AFTER = 3          # a stem issued this many times already is generic
SHOWN_NODES = 4          # the window holds exactly four
CANDIDATES = 6
_WORD = re.compile(r"[A-Za-zА-Яа-яЁё]{3,}")


def enabled(root: str | Path | None = None) -> bool:
    base = Path(root).expanduser() if root else paths.state_dir()
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
    mark = answer.get("mark") if answer.get("mark") in {"ok", "partly", "wrong", "broken", "none"} else "wrong"
    # A refuted guess had something to refute: the error is worth more than "bolder".
    vague = bool(answer.get("vague")) and mark in {"ok", "partly"}
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
    shown_mark = "" if mark in {"broken", "none"} or (mark == "partly" and not answer.get("sure")) else mark
    return {"mark": shown_mark, "raw_mark": mark, "vague": vague,
            "words": kept[:SHOWN_NODES] if len(kept) >= SHOWN_NODES else [],
            "dropped": dropped}


#: A selection shorter than this is a word or a phrase, not a passage: checking a
#: guess against it checks it against the reader's own pointer (context.py).
MIN_GROUND_CHARS = 40
#: Words a guess is built from when it says nothing anyone could refute.
_VAGUE = re.compile(r"(?<![а-яё])(определ[её]нн\w*|как\w*-то|что-то|кто-то|где-то|разн(ые|ых|ыми)|"
                    r"некотор\w+|имеет в себе|включает в себя|какой-либо|всяк\w+)(?![а-яё])", re.IGNORECASE)
_FILLER = frozenset({"это", "этот", "эта", "эти", "того", "так", "как", "что", "чтобы", "если", "когда",
                     "где", "для", "или", "при", "под", "над", "без", "его", "она", "они", "оно", "мы",
                     "там", "тут", "ещё", "еще", "уже", "все", "всё", "the", "and", "for", "with", "that"})


def content_stems(text: str) -> set[str]:
    return {_stem(w) for w in _WORD.findall(text or "") if w.lower() not in _FILLER}


def is_question(hypothesis: str) -> bool:
    """A question is the reader's own and worth keeping, but it is not a claim to mark."""
    return "?" in (hypothesis or "")


def choose_ground(selection: str, last_fragment: str, recent: list[str]) -> tuple[str, str]:
    """(ground, focus): what a guess is checked against, and what it points at.

    11-12.10: the selection was often the four nodes of the previous turn copied
    from their window, so a guess made of those nodes was "confirmed" by those
    nodes - 4 of the 6 ticks on 12.10. A short selection, or one made of words the
    tool itself issued, is the reader's pointer; the passage is the last fragment.
    """
    point = (selection or "").strip()
    passage = (last_fragment or "").strip()
    issued = {_stem(w) for w in recent}
    words = content_stems(point)
    echo = bool(words) and (len(words & issued) >= 3 or words <= issued)
    if point and len(point) >= MIN_GROUND_CHARS and not echo:
        return point, ""
    if passage:
        return passage, point
    return point, ""


def gate(mark: str, vague: bool, hypothesis: str, ground: str, recent: list[str]) -> tuple[str, bool, str]:
    """Script checks over the model's mark: (mark, vague, kind).

    The model alone passed glued labels as "ok" (12 of 15 hand-checked verdicts
    were wrong or doubtful). Here a question is never marked, and a guess built
    from vague words with less than two content words of this passage is vague
    whatever the model said. Words the tool issued last turn do not count.
    """
    if is_question(hypothesis):
        return "none", False, "question"
    issued = {_stem(w) for w in recent}
    shared = (content_stems(hypothesis) & content_stems(ground)) - issued
    if mark in {"ok", "partly"} and _VAGUE.search(hypothesis or "") and len(shared) < 2:
        return mark, True, "claim"
    if mark == "ok" and not shared:
        # Nothing of this passage in the guess: the tick would be about something else.
        return "partly", vague, "claim"
    return mark, vague, "claim"


QUESTION_REPLY = "Это вопрос, а не догадка. Какой твой ответ на него? Его и проверим."


def describe(mark: str, words: list[str]) -> str:
    """The text for the log and for the plain window: mark, then the nodes."""
    parts = [MARK_SYMBOL.get(mark, "")] if mark else []
    if words:
        parts.append(" · ".join(words))
    return "\n".join(parts) or "нечего добавить"


def answer_text(mark: str, right: str, quote: str, words: list[str], *, kind: str = "claim") -> str:
    """The verdict as it stays on screen: never a sign that vanishes (notes 31, 75).

    The mark, then what holds in the reader's own words, then where to look
    (a verbatim pointer, not the answer), then the nodes as plain words.
    """
    if kind == "question":
        return QUESTION_REPLY
    lines = [MARK_SYMBOL.get(mark, "") + (" " + MARK_WORD[mark] if mark in MARK_WORD else "")] if mark else []
    if right:
        lines.append("верно у тебя: " + right)
    if quote and mark in {"partly", "wrong"}:
        lines.append(f"смотри: «{quote}»")
    if words:
        lines.append("узлы: " + " · ".join(words))
    return "\n".join(line for line in lines if line.strip()) or "Сверить не с чем: в этом месте нет опоры для оценки."
