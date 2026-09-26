"""Split one reading fragment into the discrete study nodes it actually contains.

A fragment handed to the task generator is normally several textbook blocks at
once: a section heading, a definition, a theorem with its proof, a worked
example, a remark about the cost of the method. A task built from "the fragment"
silently spans all of them, so its goal turns vague and its answer cannot be
checked against one concrete claim.

``parse_nodes`` cuts a fragment into the smallest blocks that each carry one
mathematical move and labels every block by what it offers the reader:

* ``definition``  — a new notion is being introduced;
* ``method``      — a procedure is described that the reader can carry out;
* ``proof``       — a claim is being justified;
* ``statement``   — a claim is asserted without its proof here;
* ``example``     — a concrete instance is worked out;
* ``application`` — where the theory is used, not how it works;
* ``exercise``    — the textbook's own problem (never a starting point for a
  generated task, or the generator would launder an existing exercise);
* ``orientation`` — framing prose that gives a goal or a transition.

The splitter is rule-based and offline. It has to run before any model call —
a task is checked against a concrete node, not against a retelling — and it has
to be reproducible, so the same fragment always yields the same nodes. The
rules are tuned for Russian mathematical prose with OCR damage: hyphenation
split across a line break is joined, while formula layout is left untouched.

Only segmentation and labelling happen here. Choosing a node for a task,
verifying that its solution exists, and checking a reader's answer are later
steps that consume this output.
"""

from __future__ import annotations

import hashlib
import re
import sys
from dataclasses import dataclass
from pathlib import Path

# ── node kinds ───────────────────────────────────────────────────────────────

DEFINITION = "definition"
METHOD = "method"
PROOF = "proof"
STATEMENT = "statement"
EXAMPLE = "example"
APPLICATION = "application"
EXERCISE = "exercise"
ORIENTATION = "orientation"

KINDS = (
    DEFINITION,
    METHOD,
    PROOF,
    STATEMENT,
    EXAMPLE,
    APPLICATION,
    EXERCISE,
    ORIENTATION,
)

#: A task may be built from a node that offers a mathematical move. Framing
#: prose and the textbook's own exercises are not starting points for new tasks.
TASKABLE_KINDS = (DEFINITION, METHOD, PROOF, STATEMENT, EXAMPLE, APPLICATION)
NON_TASKABLE_KINDS = (ORIENTATION, EXERCISE)

#: Which node a macro mode may be built from. A mode is a cognitive action, and
#: a node offers material for some actions only: "justify" needs a claim, not a
#: worked example; "apply" needs a procedure, not framing prose.
MODE_KINDS: dict[str, tuple[str, ...]] = {
    "apply": (METHOD, EXAMPLE),
    "distinguish": (DEFINITION, APPLICATION, EXAMPLE),
    "justify": (STATEMENT, PROOF),
    "boundary": (STATEMENT, METHOD, DEFINITION),
}

#: A node longer than this is split at sentence boundaries, so that a task can
#: still name a concrete object instead of "the whole paragraph".
MAX_NODE_CHARS = 1400


@dataclass(frozen=True)
class TaskNode:
    """One block of the fragment, labelled by what it offers the reader."""

    id: str
    kind: str
    title: str
    text: str
    start: int
    end: int
    deps: tuple[str, ...]
    ops: tuple[str, ...]

    def to_dict(self) -> dict[str, object]:
        return {
            "id": self.id,
            "kind": self.kind,
            "title": self.title,
            "text": self.text,
            "span": [self.start, self.end],
            "deps": list(self.deps),
            "ops": list(self.ops),
        }


# ── OCR repair ───────────────────────────────────────────────────────────────

#: A Cyrillic word hyphenated across a line break. Latin and formula lines are
#: left alone: "x³ + ax²\n  + bx" is layout, not hyphenation.
_HYPHEN_BREAK = re.compile(r"(?<=[а-яё])-\s*\n\s*(?=[а-яё])", re.IGNORECASE)

#: The scan of this textbook repeats the trailing syllable on the next line
#: ("идей-" / "идейное", "ра-" / "радикалы"). Joining naively would double it,
#: so the repeated prefix is collapsed to one copy instead.
_HYPHEN_DUPLICATED = re.compile(r"(?P<prefix>[а-яё]+)-\s*\n\s*(?P=prefix)", re.IGNORECASE)


def clean_fragment(text: str) -> str:
    """Repair OCR hyphenation while preserving formulas and line layout."""
    text = _HYPHEN_DUPLICATED.sub(lambda match: match.group("prefix"), text)
    return _HYPHEN_BREAK.sub("", text)


#: The reading context arrives wrapped in descriptive tags
#: (``<SOURCE_FRAGMENT …>``, ``<CURRENT_PASSAGE …>``). Tags are structure the
#: splitter must not read as prose.
_WRAPPER_TAG = re.compile(r"</?[A-Za-z_][^>]*>")


def strip_wrapper(text: str) -> str:
    """Remove the context's XML tags, keeping their text content."""
    return _WRAPPER_TAG.sub("\n", text)


# ── structural markers ───────────────────────────────────────────────────────

#: A marker counts only at the start of a sentence, so "в следующем утверждении"
#: or "для определения" never open a new block.
_BOUNDARY = r"(?:^|(?<=[\n.!?;:»)]))[ \t]*"
_LEFT = r"(?<![а-яёa-z0-9])"
_RIGHT = r"(?![а-яёa-z0-9])"

_MARKER_BODY = (
    r"(?:"
    r"определени[ея]|"
    r"назов[её]м|"
    r"теорема\s*\d*['′]?|"
    r"лемма\s*\d*|"
    r"следстви[ея]\s*\d*['′]?|"
    r"утверждени[ея]\s*\d*|"
    r"доказательство|"
    r"пример\s*\d*|"
    r"замечани[ея]\s*\d*|"
    r"упражнени[яе]"
    r")"
)

_BLOCK_MARKER = re.compile(
    _BOUNDARY + _LEFT + r"(" + _MARKER_BODY + r")" + _RIGHT,
    re.IGNORECASE | re.MULTILINE,
)
_SECTION_MARKER = re.compile(_BOUNDARY + r"(§\s*\d+)", re.MULTILINE)
#: A numbered subsection stays on one line ("1. Терминология. Следует…"): a
#: digit followed by a dot and a same-line capital. Requiring `[ \t]+` instead
#: of `\s+` keeps "… < 5.\nЗдесь" (a formula ending) from looking like a heading.
_SUBSECTION_MARKER = re.compile(r"(?:^|\n)[ \t]*(\d{1,2})\.[ \t]+(?=[А-ЯЁ])")

_SENTENCE = re.compile(r"(?<=[.!?])\s+(?=[А-ЯЁA-Z«(])")


def _marker_positions(text: str) -> list[int]:
    """Offsets where a new structural block begins, in document order.

    A subsection number sitting inside a block marker ("Следствие 1.") is the
    marker's own number, not a heading, so it is dropped.
    """
    candidates: list[int] = []
    occupied: list[tuple[int, int]] = []
    for match in _BLOCK_MARKER.finditer(text):
        candidates.append(match.start(1))
        occupied.append((match.start(1), match.end(1)))
    for match in _SECTION_MARKER.finditer(text):
        candidates.append(match.start(1))
        occupied.append((match.start(1), match.end(1)))
    for match in _SUBSECTION_MARKER.finditer(text):
        start = match.start(1)
        if any(open_at <= start < close_at for open_at, close_at in occupied):
            continue
        candidates.append(start)
        occupied.append((start, match.end(1)))
    return sorted(set(candidates))


def _segment(text: str) -> list[tuple[int, int]]:
    """Slice the fragment at every marker, keeping any leading prose."""
    positions = _marker_positions(text)
    if not positions:
        return [(0, len(text))] if text.strip() else []
    bounds: list[tuple[int, int]] = []
    if positions[0] > 0 and text[: positions[0]].strip():
        bounds.append((0, positions[0]))
    for index, start in enumerate(positions):
        end = positions[index + 1] if index + 1 < len(positions) else len(text)
        if text[start:end].strip():
            bounds.append((start, end))
    return bounds


def _split_long(block: str, start: int) -> list[tuple[int, int]]:
    """Bound a node's size by grouping whole sentences up to ``MAX_NODE_CHARS``."""
    if len(block) <= MAX_NODE_CHARS:
        return [(start, start + len(block))]
    sentences: list[tuple[int, int]] = []
    cursor = 0
    for match in _SENTENCE.finditer(block):
        sentences.append((cursor, match.start()))
        cursor = match.end()
    sentences.append((cursor, len(block)))
    groups: list[tuple[int, int]] = []
    group_start: int | None = None
    group_end = 0
    for sentence_start, sentence_end in sentences:
        if group_start is None:
            group_start, group_end = sentence_start, sentence_end
        elif sentence_end - group_start <= MAX_NODE_CHARS:
            group_end = sentence_end
        else:
            groups.append((group_start, group_end))
            group_start, group_end = sentence_start, sentence_end
    if group_start is not None:
        groups.append((group_start, group_end))
    return [(start + open_at, start + close_at) for open_at, close_at in groups]


# ── classification ───────────────────────────────────────────────────────────

_DEFINITION_WORDS = (
    "называется",
    "называются",
    "называют",
    "назовём",
    "назовем",
    "будем называть",
    "по определению",
    "определяется как",
)
_METHOD_WORDS = (
    "метод",
    "алгоритм",
    "процедур",
    "приведени",
    "исключени",
    "подберём",
    "подберем",
    "вычтем",
    "умножим",
    "сложим",
    "последовательн",
    "для этого нужно",
    "путём",
    "процесс",
)
_APPLICATION_WORDS = (
    "применени",
    "применяется",
    "применяются",
    "применяют",
    "используется",
    "используют",
    "играет роль",
    "задача о",
    "в качестве",
    "необходимый атрибут",
    "кристаллограф",
    "молекул",
    "кодировани",
    "симметри",
)
_PROOF_WORDS = ("доказательство", "докажем", "доказано", "доказательств", "что и требовалось")
_STATEMENT_WORDS = ("теорема", "лемма", "следствие", "утверждение", "признак", "критерий")
_CONDITION_WORDS = (
    "если ",
    "при условии",
    "только тогда",
    "равносильно",
    "в противном случае",
    "не следует",
    "неверно",
    "контрпример",
)

_STATEMENT_HEADS = ("теорема", "лемма", "следствие", "утверждение", "признак", "критерий")


def _normalize(text: str) -> str:
    return " ".join(text.casefold().split())


def _starts(head: str, *prefixes: str) -> bool:
    return any(head.startswith(prefix) for prefix in prefixes)


def _has(text: str, words: tuple[str, ...]) -> bool:
    return any(word in text for word in words)


def _classify(block: str) -> str:
    """Label one block by the move it carries; framing prose stays ``orientation``.

    Mixed blocks are the norm — a "terminology" subsection introduces a dozen
    notions and states a claim about them. Signals are therefore scored instead
    of tried in order: the heading sets the intent, the body vocabulary confirms
    or overrides it, and the strongest kind wins. A tie falls to the earlier
    kind in ``KINDS``.
    """
    normalized = _normalize(block)
    head = normalized[:120]
    lead = normalized[:400]
    scores = {kind: 0 for kind in KINDS}

    # Heading-level intent: for a numbered subsection the first line is what the
    # block is *about*, even when its body defines objects along the way.
    if head.startswith("§"):
        scores[ORIENTATION] += 6
    if _starts(head, "упражнени"):
        scores[EXERCISE] += 10
    if _starts(head, "доказательство"):
        scores[PROOF] += 10
    if _starts(head, "пример", "например"):
        scores[EXAMPLE] += 10
    if _starts(head, "определени", "назов"):
        scores[DEFINITION] += 8
    if _starts(head, *_STATEMENT_HEADS):
        scores[PROOF if _has(lead, _PROOF_WORDS) else STATEMENT] += 9
    if _starts(head, "замечани"):
        scores[ORIENTATION] += 3
    if re.match(r"^\d{1,2}\.\s*задача\b", head):
        # "4. Задача о нагретой пластинке" is an application overview, whatever
        # notions it names on the way.
        scores[APPLICATION] += 9
    if re.match(r"^\d{1,2}\.\s*отдельные замечани", head):
        scores[ORIENTATION] += 5

    # Body vocabulary.
    if _has(lead, _DEFINITION_WORDS):
        scores[DEFINITION] += 3
    if _has(lead, _PROOF_WORDS):
        scores[PROOF] += 3
    if _has(lead, _STATEMENT_WORDS):
        scores[STATEMENT] += 2
    if _has(lead, _METHOD_WORDS):
        scores[METHOD] += 3
    if _has(lead, _APPLICATION_WORDS):
        # Application is the weaker signal: "в качестве", "используется" and
        # "применение" occur inside methods and proofs too.
        scores[APPLICATION] += 2

    best = max(scores.values())
    if best == 0:
        return ORIENTATION
    return next(kind for kind in KINDS if scores[kind] == best)


# ── dependencies and available operations ────────────────────────────────────

_REFERENCE = re.compile(
    # A cross-reference carries a number: bare "следствия" in "иной форме
    # следствия" is ordinary prose, not a pointer to Следствие 1.
    r"(?:§\s*\d+|гл\.?\s*\d+|п\.\s*\d+"
    r"|[Тт]еорем\w*\s*\d+['′]?"
    r"|[Лл]емм\w*\s*\d+['′]?"
    r"|[Сс]ледстви\w*\s*\d+['′]?)"
)

_BASE_OPS: dict[str, tuple[str, ...]] = {
    DEFINITION: ("distinguish",),
    METHOD: ("compute", "check_boundary"),
    PROOF: ("prove",),
    STATEMENT: ("prove", "check_boundary"),
    EXAMPLE: ("compute",),
    APPLICATION: ("distinguish",),
}


def _deps(text: str, title: str) -> tuple[str, ...]:
    """Named material this node leans on, as far as the text itself states it.

    Only explicit references are recorded ("по теореме 1", "§ 3"). A full
    prerequisite graph would need the whole book, not one fragment. A reference
    matching the node's own title ("Теорема 4" inside the theorem) is dropped:
    it is a self-label, not a dependency.
    """
    own = " ".join(title.casefold().split())
    seen: list[str] = []
    for match in _REFERENCE.finditer(text):
        value = " ".join(match.group(0).casefold().split())
        if value == own or value in seen:
            continue
        seen.append(value)
    return tuple(seen[:6])


def _ops(text: str, kind: str) -> tuple[str, ...]:
    """The reader operations the node supports, from its kind and its wording."""
    ops = list(_BASE_OPS.get(kind, ()))
    normalized = _normalize(text)
    if any(word in normalized for word in _CONDITION_WORDS) and "check_boundary" not in ops:
        ops.append("check_boundary")
    if any(word in normalized for word in ("докаж", "обосну", "доказыва")) and "prove" not in ops:
        ops.append("prove")
    if any(word in normalized for word in ("вычисли", "найти", "решить", "подсчита", "выразить")) and (
        "compute" not in ops
    ):
        ops.append("compute")
    if any(word in normalized for word in ("различи", "сравни", "отлича", "сопостав")) and (
        "distinguish" not in ops
    ):
        ops.append("distinguish")
    return tuple(ops)


# ── selection ────────────────────────────────────────────────────────────────

_DEP_KEY = re.compile(
    r"(?P<kind>§|гл|п|теорем\w*|лемм\w*|следстви\w*)\s*(?P<num>\d+['′]?)",
    re.IGNORECASE,
)


def _dep_key(value: str) -> tuple[str, str] | None:
    """Reduce a reference or a node title to (kind, number) for matching."""
    match = _DEP_KEY.search(value.casefold())
    if match is None:
        return None
    kind = match.group("kind")
    if kind.startswith("теорем"):
        kind = "теорем"
    elif kind.startswith("лемм"):
        kind = "лемм"
    elif kind.startswith("следстви"):
        kind = "следстви"
    number = match.group("num").replace("′", "'")
    return kind, number


def unmet_deps(node: TaskNode, prior_nodes: list[TaskNode]) -> tuple[str, ...]:
    """References the node makes that the material so far does not provide.

    Structural pointers into the book (§, гл, п) are taken as available: the
    reader has the book. A pointer to a numbered claim must resolve to an
    earlier node, or the task cannot be attempted from what is on screen.
    """
    available = {key for key in (_dep_key(n.title) for n in prior_nodes) if key}
    missing: list[str] = []
    for dep in node.deps:
        key = _dep_key(dep)
        if key is None or key[0] in ("§", "гл", "п"):
            continue
        if key not in available:
            missing.append(dep)
    return tuple(missing)


def select_node(nodes: list[TaskNode], mode: str, *, skip: tuple[str, ...] = ()) -> TaskNode | None:
    """The first node a mode can be built from, given the material before it.

    One node per task: the point of the split is that a task names a concrete
    claim or procedure instead of "the fragment".
    """
    wanted = MODE_KINDS.get(mode, ())
    for index, node in enumerate(nodes):
        if node.kind not in wanted or node.id in skip:
            continue
        if unmet_deps(node, nodes[:index]):
            continue
        return node
    return None


def find_source_proof(node: TaskNode, nodes: list[TaskNode]) -> TaskNode | None:
    """The proof that follows a statement, before the next claim or section.

    Used to check that a "justify" task is answerable from the material rather
    than from the model's own authority.
    """
    ids = [candidate.id for candidate in nodes]
    if node.id not in ids:
        return None
    for following in nodes[ids.index(node.id) + 1:]:
        if following.kind == PROOF:
            return following
        if following.kind in (STATEMENT, EXAMPLE, ORIENTATION, EXERCISE):
            return None
    return None


# ── titles and ids ───────────────────────────────────────────────────────────

_SECTION_TITLE = re.compile(r"^\s*(§\s*\d+\.?\s*[^.]{0,60})")
_MARKER_TITLE = re.compile(
    r"^\s*((?:Определени[ея]|Теорема\s*\d*['′]?|Лемма\s*\d*|Следстви[ея]\s*\d*['′]?|"
    r"Утверждени[ея]\s*\d*|Доказательство|Пример\s*\d*|Замечани[ея]\s*\d*|Упражнени[яе]))",
    re.IGNORECASE,
)
_SUBSECTION_TITLE = re.compile(r"^\s*(\d{1,2}\.\s+[А-ЯЁ][^.]{0,60})")


def _title(text: str, kind: str) -> str:
    stripped = text.strip()
    for pattern in (_SECTION_TITLE, _MARKER_TITLE, _SUBSECTION_TITLE):
        match = pattern.match(stripped)
        if match:
            return " ".join(match.group(1).split())
    first = re.split(r"(?<=[.!?])\s", stripped, maxsplit=1)[0]
    if len(first) > 60:
        first = first.split(",", 1)[0]
    return " ".join(first.split())[:80]


def _node_id(kind: str, text: str, start: int) -> str:
    digest = hashlib.sha256(f"{kind}\x00{start}\x00{_normalize(text)}".encode())
    return digest.hexdigest()[:12]


# ── entry point ──────────────────────────────────────────────────────────────

def parse_nodes(text: str, *, min_chars: int = 1) -> list[TaskNode]:
    """Return the labelled study nodes of one fragment, in document order.

    ``min_chars`` drops blocks smaller than a given size; the default keeps
    everything so that the caller decides what is worth a task.
    """
    cleaned = clean_fragment(text)
    nodes: list[TaskNode] = []
    exercise_section = False
    for block_start, block_end in _segment(cleaned):
        block = cleaned[block_start:block_end]
        if len(block.strip()) < min_chars:
            continue
        kind = _classify(block)
        if kind == EXERCISE:
            exercise_section = True
        elif exercise_section and kind == ORIENTATION:
            # Numbered items after "УПРАЖНЕНИЯ" are the textbook's own problems.
            kind = EXERCISE
        for span_start, span_end in _split_long(block, block_start):
            raw = cleaned[span_start:span_end]
            lead = len(raw) - len(raw.lstrip())
            trail = len(raw) - len(raw.rstrip())
            start = span_start + lead
            end = span_end - trail
            body = cleaned[start:end]
            if not body:
                continue
            title = _title(body, kind)
            nodes.append(
                TaskNode(
                    id=_node_id(kind, body, start),
                    kind=kind,
                    title=title,
                    text=body,
                    start=start,
                    end=end,
                    deps=_deps(body, title),
                    ops=_ops(body, kind),
                )
            )
    return nodes


def render_nodes(nodes: list[TaskNode]) -> str:
    """Plain-text listing for inspection and logs; one block per node."""
    lines: list[str] = []
    for index, node in enumerate(nodes):
        preview = " ".join(node.text.split())
        if len(preview) > 160:
            preview = preview[:159] + "…"
        ops = ", ".join(node.ops) or "—"
        deps = ", ".join(node.deps) or "—"
        lines.append(f"[{index}] {node.kind} · {node.title}")
        lines.append(f"    ops: {ops} | deps: {deps} | [{node.start}:{node.end}]")
        lines.append(f"    {preview}")
    return "\n".join(lines)


def _main(argv: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else argv
    source = arguments[0] if arguments else ""
    text = Path(source).expanduser().read_text(encoding="utf-8") if source else sys.stdin.read()
    print(render_nodes(parse_nodes(text)))
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
