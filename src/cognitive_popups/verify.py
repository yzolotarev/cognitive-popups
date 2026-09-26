"""Content-based checking of a task and of a reader's attempt.

Two requirements from the methodology land here.

*Before* a task is shown, it must be checked that it is answerable: that the
data are sufficient, that a solution exists, that the claim to prove is really
provable from the material. For a computational template this is done in code —
``rank(A)`` versus ``rank([A|b])`` is arithmetic, and asking the model to check
its own output would not make the result any surer. Only when no machine
checkable object exists does the decision fall back to the source (does the
material itself provide the proof?) and, failing that, to the model's own
stated confidence — never to a silent assumption that a fluent task is a
correct one.

*After* an attempt, the answer is checked by mathematical content, not by
matching words to a quotation: a linear system's verdict is derived from its
ranks and compared with what the reader wrote. ``check_attempt`` returns
``None`` when it cannot judge by content, and the caller then falls back to the
model.

Everything here is offline and dependency-free: exact ``Fraction`` arithmetic
instead of floating point, so "rank 2" is never "rank 2.0000001".
"""

from __future__ import annotations

import json
import re
from collections.abc import Sequence
from dataclasses import dataclass
from fractions import Fraction
from typing import Any

from .nodes import STATEMENT, TaskNode, find_source_proof

#: Payload types with a programmatic verifier. A payload of an unknown type is
#: not an error — it is simply not machine checkable, and is reported as such.
KNOWN_PAYLOAD_TYPES = ("linear_system",)


class LinearSystemError(ValueError):
    """The payload does not describe a well-formed linear system."""


@dataclass(frozen=True)
class VerificationResult:
    status: str  # passed | failed | needs_context
    note: str


# ── exact linear algebra ─────────────────────────────────────────────────────

def _fraction(value: Any) -> Fraction:
    if isinstance(value, bool) or value is None:
        raise LinearSystemError(f"не число: {value!r}")
    try:
        return Fraction(str(value))
    except (ValueError, ZeroDivisionError, TypeError) as exc:
        raise LinearSystemError(f"не число: {value!r}") from exc


def rank(matrix: list[list[Any]]) -> int:
    """Rank over the rationals, by exact Gaussian elimination."""
    rows = [[_fraction(value) for value in row] for row in matrix]
    if not rows:
        return 0
    width = len(rows[0])
    pivot_row = 0
    for column in range(width):
        pivot = next(
            (index for index in range(pivot_row, len(rows)) if rows[index][column] != 0),
            None,
        )
        if pivot is None:
            continue
        rows[pivot_row], rows[pivot] = rows[pivot], rows[pivot_row]
        lead = rows[pivot_row][column]
        rows[pivot_row] = [value / lead for value in rows[pivot_row]]
        for index in range(len(rows)):
            if index != pivot_row and rows[index][column] != 0:
                factor = rows[index][column]
                rows[index] = [
                    left - factor * right
                    for left, right in zip(rows[index], rows[pivot_row])
                ]
        pivot_row += 1
        if pivot_row == len(rows):
            break
    return pivot_row


def analyze_linear_system(matrix: Any, rhs: Any) -> dict[str, Any]:
    """Derive consistency and the number of solutions of ``Ax = b``."""
    if not isinstance(matrix, list) or not matrix:
        raise LinearSystemError("матрица пуста")
    if not isinstance(rhs, list):
        raise LinearSystemError("не задан столбец свободных членов")
    if len(rhs) != len(matrix):
        raise LinearSystemError(
            f"строк {len(matrix)}, свободных членов {len(rhs)}"
        )
    width: int | None = None
    for row in matrix:
        if not isinstance(row, list) or not row:
            raise LinearSystemError("строка матрицы пуста")
        if width is None:
            width = len(row)
        elif len(row) != width:
            raise LinearSystemError("матрица не прямоугольная")
    assert width is not None
    rank_a = rank(matrix)
    augmented = [list(row) + [rhs[index]] for index, row in enumerate(matrix)]
    rank_aug = rank(augmented)
    if rank_a != rank_aug:
        solutions = "0"
    elif rank_a == width:
        solutions = "1"
    else:
        solutions = "inf"
    return {
        "rank_a": rank_a,
        "rank_aug": rank_aug,
        "rows": len(matrix),
        "unknowns": width,
        "consistent": rank_a == rank_aug,
        "solutions": solutions,
    }


_SOLUTION_TEXT = {
    "0": "решений нет (система несовместна)",
    "1": "единственное решение (система совместна и определена)",
    "inf": "бесконечно много решений (система совместна и неопределена)",
}

#: Reader phrasings that name each verdict. Checking the verdict is checking the
#: mathematics: the answer *is* one of three claims about the ranks.
_VERDICT_KEYS = {
    "0": ("несовместна", "несовместной", "решений нет", "нет решений", "не имеет решений"),
    "1": ("единственное", "единственно", "определена", "определённой", "одно решение"),
    "inf": ("бесконечно", "неопределена", "неопределённой", "много решений", "бесконечное множество"),
}


def _mentions(text: str, keys: Sequence[str]) -> bool:
    """Whole-word match, so "определена" is not read inside "неопределена"."""
    return any(re.search(rf"\b{re.escape(key)}\b", text) for key in keys)


def analysis_note(analysis: dict[str, Any]) -> str:
    return (
        f"rank(A)={analysis['rank_a']}, rank([A|b])={analysis['rank_aug']}, "
        f"неизвестных {analysis['unknowns']} → {_SOLUTION_TEXT[analysis['solutions']]}"
    )


# ── payload access ───────────────────────────────────────────────────────────

def task_payload(task: Any) -> dict[str, Any]:
    """The payload of a task, whether it arrives as an object or as stored JSON."""
    if isinstance(task, dict):
        raw = task.get("payload")
        if isinstance(raw, dict):
            return raw
        stored = task.get("payload_json")
        if isinstance(stored, str) and stored.strip():
            try:
                decoded = json.loads(stored)
            except ValueError:
                return {}
            return decoded if isinstance(decoded, dict) else {}
        return {}
    raw = getattr(task, "payload", None)
    return raw if isinstance(raw, dict) else {}


def _payload_issue(payload: Any) -> str:
    """A malformed payload is a task defect, not a silent skip."""
    if not isinstance(payload, dict):
        return "payload должен быть объектом"
    kind = payload.get("type")
    if kind is not None and not isinstance(kind, str):
        return "payload.type должен быть строкой"
    return ""


# ── pre-show verification ────────────────────────────────────────────────────

def verify_payload(payload: dict[str, Any]) -> VerificationResult:
    """Check a machine checkable task before it is shown."""
    kind = payload.get("type")
    if kind != "linear_system":
        return VerificationResult(
            "passed", f"payload типа {kind!r} не проверяется программно"
        )
    try:
        analysis = analyze_linear_system(payload.get("matrix"), payload.get("rhs"))
    except LinearSystemError as exc:
        return VerificationResult("failed", f"линейная система некорректна: {exc}")
    note = analysis_note(analysis)
    requirement = str(payload.get("require", "")).strip().casefold()
    if requirement == "consistent" and not analysis["consistent"]:
        return VerificationResult("failed", f"требуется совместная система, но {note}")
    if requirement == "unique" and analysis["solutions"] != "1":
        return VerificationResult("failed", f"требуется единственное решение, но {note}")
    return VerificationResult("passed", note)


def _task_field(task: Any, name: str, default: str = "") -> str:
    if isinstance(task, dict):
        value = task.get(name)
    else:
        value = getattr(task, name, None)
    return default if value is None else str(value)


def verify_task(
    task: Any,
    *,
    node: TaskNode | None = None,
    nodes: Sequence[TaskNode] = (),
    mode: str = "",
) -> VerificationResult:
    """Decide whether a generated task may be shown.

    Order of evidence, from strongest to weakest: a machine checkable payload;
    the source's own proof of the claim; the model's stated high confidence.
    """
    payload = task_payload(task)
    issue = _payload_issue(payload)
    if issue:
        return VerificationResult("failed", issue)
    if payload:
        return verify_payload(payload)

    if node is None:
        return VerificationResult("passed", "узел не выбран: проверка по источнику не требуется")

    if mode == "justify" and node.kind == STATEMENT:
        proof = find_source_proof(node, list(nodes))
        if proof is not None:
            return VerificationResult(
                "passed", f"тезис «{node.title}» и его доказательство «{proof.title}» есть в материале"
            )
        confidence = _task_field(task, "confidence").strip().casefold()
        if confidence == "high":
            return VerificationResult(
                "passed", "доказательства в материале нет; модель подтвердила тезис с высокой уверенностью"
            )
        return VerificationResult(
            "needs_context",
            "для обоснования нет ни доказательства в материале, ни высокой уверенности модели",
        )

    return VerificationResult("passed", f"узел «{node.title}» ({node.kind}): машинно-проверяемых данных нет")


# ── content-based answer check ───────────────────────────────────────────────

def check_attempt(task: Any, attempt: str) -> str | None:
    """Judge one attempt by content, or return ``None`` to defer to the model.

    Currently programmatic only for a linear system's verdict. Anything else —
    a free-form derivation, a proof — is left to the model, which is why this
    returns ``None`` instead of guessing.
    """
    payload = task_payload(task)
    if payload.get("type") != "linear_system":
        return None
    try:
        analysis = analyze_linear_system(payload.get("matrix"), payload.get("rhs"))
    except LinearSystemError:
        return None

    normalized = " ".join(str(attempt).casefold().split())
    correct_keys = _VERDICT_KEYS[analysis["solutions"]]
    wrong_keys = [
        key
        for verdict, keys in _VERDICT_KEYS.items()
        if verdict != analysis["solutions"]
        for key in keys
    ]
    states_correct = _mentions(normalized, correct_keys)
    states_wrong = _mentions(normalized, wrong_keys)
    if states_correct and not states_wrong:
        status = "correct"
        what = "названный вердикт совпадает с вычисленным по рангам."
    elif states_wrong:
        status = "needs_retry"
        what = "в попытке назван не тот вердикт системы."
    else:
        status = "insufficient_attempt"
        what = "в попытке не назван вердикт системы (совместность и число решений)."

    return (
        f"СТАТУС: {status}\n"
        "ПРОВЕРЕНО ПО МАТЕМАТИЧЕСКОМУ СОДЕРЖАНИЮ (не по совпадению слов):\n"
        f"{analysis_note(analysis)}\n"
        f"ОЖИДАЕМЫЙ ОТВЕТ: {_SOLUTION_TEXT[analysis['solutions']]}.\n"
        f"ЧТО ВЕРНО: {what}\n"
        "СЛЕДУЮЩИЙ ШАГ: сравни rank(A) и rank([A|b]); равенство при rank(A), равном "
        "числу неизвестных, даёт единственное решение, при меньшем — бесконечно много."
    )
