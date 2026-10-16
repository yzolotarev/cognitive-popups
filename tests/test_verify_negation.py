"""Denied verdicts do not count as claimed ones."""
import pytest

from cognitive_popups.verify import check_attempt

UNIQUE = {"payload": {"type": "linear_system", "matrix": [[1, 0], [0, 1]], "rhs": [1, 2]}}


@pytest.mark.parametrize("attempt", [
    "Совместна и имеет единственное решение, а не бесконечно много решений",
    "Единственное решение. Решений не бесконечно много.",
    "Одно решение; нет, не бесконечно много и не несовместна",
])
def test_negated_wrong_verdict_is_not_a_wrong_answer(attempt):
    assert "СТАТУС: correct" in check_attempt(UNIQUE, attempt)


def test_negated_correct_verdict_is_not_a_right_answer():
    assert "СТАТУС: needs_retry" in check_attempt(UNIQUE, "Решение не единственное, их бесконечно много")


def test_two_asserted_verdicts_go_to_the_model():
    assert check_attempt(UNIQUE, "Единственное решение или бесконечно много решений") is None
