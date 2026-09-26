import json

from cognitive_popups.nodes import TaskNode
from cognitive_popups.verify import (
    LinearSystemError,
    analyze_linear_system,
    check_attempt,
    rank,
    task_payload,
    verify_payload,
    verify_task,
)

UNIQUE = {"type": "linear_system", "matrix": [[1, 2], [3, 4]], "rhs": [5, 6]}
INCONSISTENT = {"type": "linear_system", "matrix": [[1, 1], [2, 2]], "rhs": [1, 3]}
UNDERDETERMINED = {"type": "linear_system", "matrix": [[1, 1], [2, 2]], "rhs": [1, 2]}


def make_node(kind: str, title: str, deps: tuple[str, ...] = ()) -> TaskNode:
    return TaskNode(
        id=title, kind=kind, title=title, text=title,
        start=0, end=len(title), deps=deps, ops=(),
    )


def test_rank_is_exact_for_a_dependent_matrix():
    assert rank([[1, 2], [2, 4]]) == 1
    assert rank([[1, 2], [3, 4]]) == 2
    assert rank([[0, 0], [0, 0]]) == 0


def test_analysis_names_the_number_of_solutions():
    assert analyze_linear_system(UNIQUE["matrix"], UNIQUE["rhs"])["solutions"] == "1"
    assert analyze_linear_system(INCONSISTENT["matrix"], INCONSISTENT["rhs"])["solutions"] == "0"
    assert analyze_linear_system(UNDERDETERMINED["matrix"], UNDERDETERMINED["rhs"])["solutions"] == "inf"


def test_analysis_rejects_ill_formed_systems():
    for matrix, rhs in (
        ([], [1]),
        ([[1, 2]], [1, 2]),
        ([[1, 2], [3]], [1, 2]),
        ([["x", 2]], [1]),
        ("no", [1]),
        ([[1, 2]], "no"),
    ):
        try:
            analyze_linear_system(matrix, rhs)
        except LinearSystemError:
            continue
        raise AssertionError(f"accepted malformed system: {matrix!r}, {rhs!r}")


def test_verify_payload_passes_a_checkable_task_and_reports_ranks():
    result = verify_payload(UNIQUE)
    assert result.status == "passed"
    assert "rank(A)=2" in result.note


def test_verify_payload_fails_an_inconsistent_system_when_consistency_is_required():
    result = verify_payload({**INCONSISTENT, "require": "consistent"})
    assert result.status == "failed"
    assert "несовместна" in result.note


def test_verify_payload_fails_a_non_unique_system_when_uniqueness_is_required():
    assert verify_payload({**UNDERDETERMINED, "require": "unique"}).status == "failed"


def test_verify_payload_reports_an_unknown_type_as_not_checked():
    result = verify_payload({"type": "poem"})
    assert result.status == "passed"
    assert "не проверяется" in result.note


def test_verify_task_fails_a_malformed_payload():
    # A payload that is not an object is already rejected by task validation;
    # here the reachable defect is an object whose type field is not a string.
    assert verify_task({"payload": {"type": 123}}).status == "failed"
    assert verify_task({"payload": {"type": "linear_system", "matrix": "no"}}).status == "failed"


def test_verify_task_accepts_an_unknown_payload_type():
    assert verify_task({"payload": {"type": "poem"}}).status == "passed"


def test_verify_task_without_a_node_or_payload_passes():
    assert verify_task({}).status == "passed"


def test_verify_task_needs_a_source_proof_for_a_justify_task():
    statement = make_node("statement", "Теорема 1")
    proof = make_node("proof", "Доказательство")

    passed = verify_task({}, node=statement, nodes=[statement, proof], mode="justify")
    assert passed.status == "passed"
    assert "Доказательство" in passed.note

    blocked = verify_task({}, node=statement, nodes=[statement], mode="justify")
    assert blocked.status == "needs_context"

    # No proof in the material: only the model's own high confidence can carry it.
    high = verify_task({"confidence": "high"}, node=statement, nodes=[statement], mode="justify")
    assert high.status == "passed"


def test_check_attempt_judges_by_the_verdict():
    task = {"payload": UNIQUE}
    assert "СТАТУС: correct" in check_attempt(task, "Система совместна и определена.")
    assert "СТАТУС: needs_retry" in check_attempt(task, "Система несовместна.")
    assert "СТАТУС: insufficient_attempt" in check_attempt(task, "Не уверен.")
    assert "rank(A)=2" in check_attempt(task, "совместна и определена")


def test_check_attempt_does_not_confuse_a_negated_verb():
    # "неопределена" contains "определена"; the check must not read one as the other.
    task = {"payload": UNDERDETERMINED}
    assert "СТАТУС: correct" in check_attempt(task, "Система совместна и неопределена.")


def test_check_attempt_defers_to_the_model_when_it_cannot_judge():
    assert check_attempt({}, "любой текст") is None
    assert check_attempt({"payload": {"type": "poem"}}, "любой текст") is None
    assert check_attempt({"payload": {"type": "linear_system", "matrix": [], "rhs": []}}, "x") is None


def test_task_payload_reads_stored_json():
    assert task_payload({"payload_json": json.dumps(UNIQUE)})["type"] == "linear_system"
    assert task_payload({"payload_json": "{"}) == {}
    assert task_payload({"payload": UNIQUE}) == UNIQUE
