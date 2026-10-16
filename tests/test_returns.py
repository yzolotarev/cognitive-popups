"""Spaced returns of missed hypotheses: 2 -> 5 -> 12 -> 30 days."""
from datetime import datetime

import pytest

from cognitive_popups import returns
from cognitive_popups.models import CognitiveSession, PredictionCheck
from cognitive_popups.records import RecordStore

DAY = returns.DAY


def _epoch(text: str) -> float:
    return datetime.fromisoformat(text).timestamp()


@pytest.fixture
def store(tmp_path):
    records = RecordStore(tmp_path / "records.sqlite3")
    session = CognitiveSession(id="s")
    for pid, created in (("p1", "2026-10-01T09:00:00"), ("p2", "2026-10-01T10:00:00")):
        records.save_prediction(session, PredictionCheck(
            buffer_fragment_ids=[], hypothesis=f"гипотеза {pid}", status="contradicted",
            mismatch=f"расхождение {pid}", id=pid,
            created_at=datetime.fromisoformat(created).astimezone().isoformat()))
    return returns.ReturnStore(records)


def test_advance_is_two_five_twelve_thirty_then_done():
    assert returns.advance(0, "better") == (1, 5)
    assert returns.advance(1, "better") == (2, 12)
    assert returns.advance(2, "better") == (3, 30)
    assert returns.advance(3, "better") == (4, None)
    for outcome in ("same", "skip"):
        assert returns.advance(2, outcome) == (0, 2)
    with pytest.raises(ValueError):
        returns.advance(0, "wrong")


def test_schedule_is_two_days_out_and_idempotent(store):
    now = _epoch("2026-10-01T12:00:00")
    assert store.schedule("p1", now) is True
    assert store.schedule("p1", now + DAY) is False
    assert store.due(now + DAY) is None
    due = store.due(now + 2 * DAY)
    assert due["prediction_id"] == "p1"
    assert due["hypothesis"] == "гипотеза p1"
    assert due["mismatch"] == "расхождение p1"
    assert due["status"] == "contradicted"
    assert "source_text" not in due


def test_oldest_due_first_and_one_per_day(store):
    now = _epoch("2026-10-01T12:00:00")
    store.schedule("p2", now)
    store.schedule("p1", now + 60)
    later = now + 3 * DAY
    assert store.due(later)["prediction_id"] == "p2"
    store.record_answer("p2", "", "skip", later)
    assert store.due(later + 60) is None, "a skip counts as the day's return"
    assert store.due(later + DAY)["prediction_id"] == "p1"


def test_full_ladder_and_restart(store):
    now = _epoch("2026-10-01T12:00:00")
    store.schedule("p1", now)
    moment = now + 2 * DAY
    row = store.record_answer("p1", "держится", "better", moment)
    assert row["step"] == 1 and row["due_epoch"] == pytest.approx(moment + 5 * DAY)
    moment += 5 * DAY
    row = store.record_answer("p1", "плывёт", "same", moment)
    assert row["step"] == 0 and row["due_epoch"] == pytest.approx(moment + 2 * DAY)
    for days in (2, 5, 12, 30):
        moment += days * DAY
        row = store.record_answer("p1", "держится", "better", moment)
    assert row["outcome"] == returns.DONE and row["due_epoch"] is None
    assert store.due(moment + 400 * DAY) is None


def test_confirmed_later_needs_another_calendar_day(store):
    created = _epoch("2026-10-01T09:00:00")
    store.schedule("p1", created)
    store.record_answer("p1", "", "better", created + 3600)
    assert store.confirmed_later("p1") is False
    store.record_answer("p1", "", "better", _epoch("2026-10-02T23:00:00"))
    assert store.confirmed_later("p1") is False
    store.record_answer("p1", "", "better", _epoch("2026-10-03T00:30:00"))
    assert store.confirmed_later("p1") is True
    assert store.confirmed_later("missing") is False


def test_record_answer_for_unknown_prediction_is_refused(store):
    with pytest.raises(ValueError):
        store.record_answer("nope", "", "better", 0)


def test_module_shortcuts_use_the_default_ledger(tmp_path):
    RecordStore().save_prediction(CognitiveSession(id="s"), PredictionCheck(
        buffer_fragment_ids=[], hypothesis="h", status="not_supported", id="x",
        created_at="2026-10-01T09:00:00+00:00"))
    now = _epoch("2026-10-05T09:00:00")
    assert returns.schedule("x", now)
    assert returns.due(now + 2 * DAY)["prediction_id"] == "x"
    returns.record_answer("x", "", "better", now + 2 * DAY)
    assert returns.confirmed_later("x")
