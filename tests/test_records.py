from __future__ import annotations

import contextlib
import io
import json
import sqlite3

import pytest
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
        mismatch="legacy mismatch",
        evidence="verbatim evidence",
        subject_note="outside the material",
        one_delta="  Поправка: предел относится к чанкам, не словам.\nСохрани пробелы.  ",
    )
    session.add_prediction_check(check)
    store.save_prediction(session, check)

    loaded = records.RecordStore(store.path).load_predictions(session.id)
    assert len(loaded) == 1
    assert loaded[0].hypothesis == "chunking explains the limit"
    assert loaded[0].buffer_fragment_ids == [fragment.id]
    assert loaded[0].one_delta == check.one_delta
    assert loaded[0].mismatch == check.mismatch
    assert loaded[0].evidence == check.evidence
    assert loaded[0].subject_note == check.subject_note
    with sqlite3.connect(store.path) as conn:
        assert conn.execute("SELECT one_delta FROM predictions").fetchone()[0] == check.one_delta
        assert conn.execute("PRAGMA user_version").fetchone()[0] == records.SCHEMA_VERSION

    check.one_delta = "Обновлённая поправка"
    store.save_prediction(session, check)
    assert records.RecordStore(store.path).load_predictions(session.id)[0].one_delta == check.one_delta
    check.one_delta = ""
    store.save_prediction(session, check)
    assert records.RecordStore(store.path).load_predictions(session.id)[0].one_delta == ""


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


def _legacy_prediction_database(path):
    """Create the version-6 layout directly, without opening the current store."""
    with sqlite3.connect(path) as conn:
        conn.row_factory = sqlite3.Row
        conn.executescript(records._SCHEMA.replace("    one_delta     TEXT NOT NULL DEFAULT '',\n", ""))
        for table, column, kind in records._ADDED_COLUMNS:
            if column == "one_delta":
                continue
            have = {row["name"] for row in conn.execute(f"PRAGMA table_info({table})")}
            if column not in have:
                conn.execute(f"ALTER TABLE {table} ADD COLUMN {column} {kind}")
        conn.execute("PRAGMA user_version=6")
        conn.execute(
            "INSERT INTO sessions (id, created_utc, created_epoch) VALUES (?, ?, ?)",
            ("legacy-session", "2026-09-17T08:22:59+00:00", 1),
        )
        conn.execute(
            "INSERT INTO predictions (id, session_id, hypothesis, status, mismatch, evidence,"
            " subject_note, created_utc, created_epoch) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            ("legacy-check", "legacy-session", "old hypothesis", "not_supported",
             "old mismatch", "old evidence", "old subject note", "2026-09-17T08:23:00+00:00", 2),
        )
        conn.execute("INSERT INTO prediction_fragments VALUES (?, ?)", ("legacy-check", "old-fragment"))


def test_prediction_migration_preserves_history_and_backs_up_once(tmp_path):
    path = tmp_path / "records.sqlite3"
    _legacy_prediction_database(path)

    store = records.RecordStore(path)
    check = store.load_predictions("legacy-session")[0]
    assert check.one_delta == ""
    assert check.mismatch == "old mismatch"
    assert check.evidence == "old evidence"
    assert check.subject_note == "old subject note"
    assert check.buffer_fragment_ids == ["old-fragment"]
    assert check.id == "legacy-check"
    assert check.created_at == "2026-09-17T08:23:00+00:00"
    with sqlite3.connect(path) as conn:
        assert conn.execute("PRAGMA user_version").fetchone()[0] == records.SCHEMA_VERSION
        assert conn.execute("SELECT one_delta FROM predictions").fetchone()[0] == ""
        column = next(row for row in conn.execute("PRAGMA table_info(predictions)") if row[1] == "one_delta")
        assert column[3:5] == (1, "''")

    backups = list(tmp_path.glob("records.sqlite3.backup-*.sqlite3"))
    assert len(backups) == 1
    with sqlite3.connect(backups[0]) as conn:
        assert conn.execute("PRAGMA user_version").fetchone()[0] == 6
        assert "one_delta" not in {row[1] for row in conn.execute("PRAGMA table_info(predictions)")}
        assert conn.execute("SELECT mismatch FROM predictions").fetchone()[0] == "old mismatch"
        assert conn.execute("SELECT fragment_id FROM prediction_fragments").fetchone()[0] == "old-fragment"

    session = CognitiveSession(id="legacy-session")
    check.one_delta = "Новая поправка после миграции"
    store.save_prediction(session, check)
    assert records.RecordStore(path).load_predictions(session.id)[0].one_delta == check.one_delta
    assert list(tmp_path.glob("records.sqlite3.backup-*.sqlite3")) == backups


def test_prediction_migration_stops_if_backup_fails(tmp_path, monkeypatch):
    from cognitive_popups import observation

    path = tmp_path / "records.sqlite3"
    _legacy_prediction_database(path)

    def fail_backup(path):
        raise OSError("backup unavailable")

    monkeypatch.setattr(observation, "backup_database", fail_backup)
    with pytest.raises(records.RecordError, match="before migration: backup unavailable"):
        records.RecordStore(path).load_predictions("legacy-session")
    with sqlite3.connect(path) as conn:
        assert conn.execute("PRAGMA user_version").fetchone()[0] == 6
        assert "one_delta" not in {row[1] for row in conn.execute("PRAGMA table_info(predictions)")}
        assert conn.execute("SELECT mismatch FROM predictions").fetchone()[0] == "old mismatch"


def test_prediction_exports_and_rendering_keep_delta_separate(tmp_path):
    store = _store(tmp_path)
    notes = NoteStore(tmp_path / "notes.sqlite3")
    session = CognitiveSession()
    check = PredictionCheck(
        buffer_fragment_ids=[], hypothesis="a hypothesis", status="not_supported",
        one_delta="  Одна поправка.\nТочно как сохранено.  ",
        mismatch="historical mismatch", evidence="source quotation", subject_note="outside knowledge",
    )
    store.save_prediction(session, check)
    store = records.RecordStore(store.path)
    data = records.session_data(store, notes, session.id)
    day = records.day_data(store, notes, _today())
    for exported in (data["predictions"][0], day["sessions"][0]["predictions"][0]):
        assert exported == check.to_dict()
        assert json.loads(json.dumps(exported))["one_delta"] == check.one_delta
    rendered = records.render_session(data)
    assert f"поправка: {check.one_delta}" in rendered
    assert rendered.index("поправка:") < rendered.index("в тексте:") < rendered.index("расхождение:")
    assert "в тексте: source quotation" in rendered
    assert "расхождение: historical mismatch" in rendered
    assert f"поправка: {check.one_delta}" in records.render_day(day)

    # Old exported dictionaries have no delta field; do not invent one from metadata.
    del data["predictions"][0]["one_delta"]
    historical = records.render_session(data)
    assert "поправка:" not in historical
    assert "расхождение: historical mismatch" in historical
    day["sessions"][0]["predictions"][0]["one_delta"] = ""
    assert "поправка:" not in records.render_day(day)
