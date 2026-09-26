from __future__ import annotations

import contextlib
import io
from datetime import datetime, timezone

from cognitive_popups import records
from cognitive_popups.models import CognitiveSession, FeynmanCheck, PredictionCheck
from cognitive_popups.notes import NoteStore


def _today() -> str:
    return datetime.now(timezone.utc).astimezone().strftime("%Y-%m-%d")


def _store(tmp_path) -> records.RecordStore:
    return records.RecordStore(tmp_path / "records.sqlite3")


def _fragment(session: CognitiveSession):
    return session.add_fragment(
        "working memory is limited",
        cues=["memory", "limit", "chunking", "note"],
        cue_details=[
            {"simple": "память", "term": "memory", "meaning": "удержание"},
            {"simple": "предел", "term": "limit", "meaning": "граница"},
            {"simple": "группы", "term": "chunking", "meaning": "объединение"},
            {"simple": "заметка", "term": "note", "meaning": "опора"},
        ],
    )


def test_fragment_roundtrip(tmp_path):
    store = _store(tmp_path)
    session = CognitiveSession()
    fragment = _fragment(session)
    store.save_fragment(session, fragment)

    loaded = store.load_fragments(session.id)
    assert len(loaded) == 1
    assert loaded[0].id == fragment.id
    assert loaded[0].source_text == "working memory is limited"
    assert loaded[0].cues == ["memory", "limit", "chunking", "note"]
    assert loaded[0].cue_details[0]["term"] == "memory"
    assert store.session_meta(session.id)["id"] == session.id


def test_prediction_roundtrip(tmp_path):
    store = _store(tmp_path)
    session = CognitiveSession()
    fragment = _fragment(session)
    store.save_fragment(session, fragment)

    check = PredictionCheck(
        buffer_fragment_ids=[fragment.id],
        hypothesis="chunking explains the limit",
        status="confirmed",
        mismatch="",
        evidence="",
    )
    session.add_prediction_check(check)
    store.save_prediction(session, check)

    loaded = store.load_predictions(session.id)
    assert len(loaded) == 1
    assert loaded[0].hypothesis == "chunking explains the limit"
    assert loaded[0].buffer_fragment_ids == [fragment.id]


def test_task_roundtrip_keeps_condition_context_and_attempt(tmp_path):
    store = _store(tmp_path)
    session = CognitiveSession()
    store.save_task(
        task_id="task-1",
        session_id=session.id,
        task_type="7",
        level="базовый",
        context="теория Галуа",
        condition="Проверь перестановку корней.",
        attempt="a и b поменялись местами",
        status="attempted",
        prompt_hash="abc123",
        model="test-model",
        raw_response='{"status":"ready"}',
        grounding_json='["теория Галуа"]',
        operations_json='["сравнить"]',
        attempts=2,
        latency_ms=321,
    )

    loaded = store.load_tasks(session.id)
    assert loaded[0]["condition"] == "Проверь перестановку корней."
    assert loaded[0]["attempt"] == "a и b поменялись местами"
    assert loaded[0]["task_type"] == "7"
    assert loaded[0]["attempts"] == 2
    assert loaded[0]["latency_ms"] == 321


def test_feynman_roundtrip_with_gaps(tmp_path):
    store = _store(tmp_path)
    session = CognitiveSession()
    fragment = _fragment(session)
    store.save_fragment(session, fragment)

    check = FeynmanCheck(
        buffer_fragment_ids=[fragment.id],
        question="why does the limit matter",
        explanation="because chunks fill it",
        status="needs_retry",
        gaps=[{"location": "limit -> chunking", "type": "missing_causal_link", "description": "no link"}],
    )
    store.save_feynman(session, check)

    loaded = store.load_feynman(session.id)
    assert len(loaded) == 1
    assert loaded[0].status == "needs_retry"
    assert loaded[0].gaps[0]["location"] == "limit -> chunking"
    assert loaded[0].buffer_fragment_ids == [fragment.id]


def test_close_and_close_stale(tmp_path):
    store = _store(tmp_path)
    store.open_session("s1")
    store.open_session("s2")
    store.close_session("s1", "clear")

    assert store.close_stale("crash") == 1
    metas = {s["id"]: s for s in store.sessions()}
    assert metas["s1"]["close_reason"] == "clear"
    assert metas["s2"]["close_reason"] == "crash"


def test_match_fragment_by_source_and_cue(tmp_path):
    session = CognitiveSession()
    fragment = _fragment(session)
    other = session.add_fragment(
        "sanctions hurt the economy",
        cues=["sanctions", "economy", "cost", "trade"],
        cue_details=[
            {"simple": "санкции", "term": "sanctions", "meaning": "давление"},
            {"simple": "экономика", "term": "economy", "meaning": "хозяйство"},
            {"simple": "цена", "term": "cost", "meaning": "издержка"},
            {"simple": "торговля", "term": "trade", "meaning": "обмен"},
        ],
    )

    assert records.match_fragment([fragment, other], "working memory") is fragment
    assert records.match_fragment([fragment, other], "chunking") is fragment
    assert records.match_fragment([fragment, other], "economy") is other
    assert records.match_fragment([fragment, other], "unrelated") is None
    assert records.match_fragment([], "anything") is None


def test_day_data_joins_notes_by_fragment(tmp_path):
    store = _store(tmp_path)
    notes = NoteStore(tmp_path / "notes.sqlite3")
    session = CognitiveSession()
    fragment = _fragment(session)
    store.save_fragment(session, fragment)

    note = notes.add("assumed it was a description", anchor="working memory", fragment_id=fragment.id)

    data = records.day_data(store, notes, _today())
    assert len(data["sessions"]) == 1
    assert data["sessions"][0]["notes"][0]["id"] == note.id
    assert data["orphan_notes"] == []


def test_day_data_lists_unassigned_notes_separately(tmp_path):
    store = _store(tmp_path)
    notes = NoteStore(tmp_path / "notes.sqlite3")
    store.open_session("s1")

    note = notes.add("no fragment, no session", anchor="orphan")

    data = records.day_data(store, notes, _today())
    assert [n["id"] for n in data["orphan_notes"]] == [note.id]


def test_cli_renders_day(tmp_path):
    store = _store(tmp_path)
    notes = NoteStore(tmp_path / "notes.sqlite3")
    session = CognitiveSession()
    fragment = _fragment(session)
    store.save_fragment(session, fragment)

    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        code = records.main(["--db", str(store.path), "--notes-db", str(notes.path), "--date", _today()])

    assert code == 0
    assert "4 слова" in buf.getvalue()
    assert "working memory" not in buf.getvalue()  # day view is a summary, not the source


def test_cli_session_shows_source(tmp_path):
    store = _store(tmp_path)
    notes = NoteStore(tmp_path / "notes.sqlite3")
    session = CognitiveSession()
    fragment = _fragment(session)
    store.save_fragment(session, fragment)

    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        code = records.main(["--db", str(store.path), "--notes-db", str(notes.path), "--session", session.id])

    assert code == 0
    assert "working memory is limited" in buf.getvalue()


def test_tasks_saved_in_the_same_millisecond_keep_insertion_order(tmp_path):
    """Ordering must not lean on `id`: a uuid is not a time, so two tasks saved
    in the same millisecond used to come back in an arbitrary order and "the
    latest task" — what the tasks CLI and the side panel read — was a coin toss.
    """
    store = _store(tmp_path)
    for task_id in ("zzz-first", "aaa-second"):
        store.save_task(
            task_id=task_id,
            session_id="s1",
            task_type="apply",
            level="базовый",
            context="материал сессии",
            condition="Проверь перестановку корней.",
            status="generated",
        )

    assert [row["id"] for row in store.load_tasks()] == ["zzz-first", "aaa-second"]


def test_a_rewritten_task_moves_to_the_end(tmp_path):
    store = _store(tmp_path)
    for task_id in ("first", "second"):
        store.save_task(
            task_id=task_id,
            session_id="s1",
            task_type="apply",
            level="базовый",
            context="материал сессии",
            condition="Проверь перестановку корней.",
            status="generated",
        )

    store.save_task(
        task_id="first",
        session_id="s1",
        task_type="apply",
        level="базовый",
        context="материал сессии",
        condition="Проверь перестановку корней.",
        attempt="моя попытка",
        status="attempt_checked",
    )

    assert [row["id"] for row in store.load_tasks()] == ["second", "first"]


def test_close_session_records_an_explicit_end(tmp_path):
    store = _store(tmp_path)
    store.open_session("s1")

    store.close_session("s1", "idle", closed_utc="2026-09-17T10:30:00+00:00")

    meta = store.session_meta("s1")
    assert meta["close_reason"] == "idle"
    assert meta["closed_utc"] == "2026-09-17T10:30:00+00:00"
