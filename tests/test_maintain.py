"""Retention ages out telemetry only; the reader's words are never touched."""
import os
import sqlite3
import time

import pytest

from cognitive_popups import maintain, observation
from cognitive_popups.event_log import EventLog
from cognitive_popups.models import CognitiveSession, PredictionCheck
from cognitive_popups.notes import NoteStore
from cognitive_popups.observation import ObservationStore
from cognitive_popups.operation_context import OperationContext
from cognitive_popups.records import RecordStore
from cognitive_popups.returns import ReturnStore

NOW = time.time()
DAY = maintain.DAY


def _age(path, table, column, days):
    with sqlite3.connect(path) as conn:
        conn.execute(f"UPDATE {table} SET {column} = ?", (NOW - days * DAY,))


def _counts(path, tables):
    with sqlite3.connect(path) as conn:
        return {t: conn.execute(f"SELECT count(*) FROM {t}").fetchone()[0] for t in tables}


@pytest.fixture
def state(tmp_path, monkeypatch):
    monkeypatch.delenv("COGNITIVE_OBSERVATION_DISABLE", raising=False)
    monkeypatch.delenv("COGNITIVE_EVENT_DISABLE", raising=False)
    records = RecordStore()
    session = CognitiveSession(id="s")
    records.save_prediction(session, PredictionCheck(buffer_fragment_ids=[], hypothesis="моя гипотеза",
                                                     status="contradicted", id="p"))
    ReturnStore(records).schedule("p", NOW)
    records.add_step("st", "шаг", session_key="k", minutes=15, started_epoch=NOW, deadline_epoch=NOW + 900)
    records.save_intention("цель")
    NoteStore().add("моя заметка", anchor="слово")
    obs = ObservationStore()
    context = OperationContext(operation_id="op")
    obs.start_request("old", context, [{"role": "user", "content": "длинный текст"}], {"model": "m"})
    obs.finish_request("old", "ok", raw_response=b"raw", response_text="ответ")
    obs.record_artifact("cues_input", {"buffer": "текст"})
    obs.record_artifact("selected_source", {"text": "текст"})
    obs.record_artifact("cues_result", {"cues": []})
    obs.record_presentation(None, window_instance_id="w", event="shown")
    EventLog().log("hotkey", session_id="e")
    path = records.path
    _age(path, "obs_requests", "started_epoch", 31)
    _age(path, "obs_artifacts", "created_epoch", 61)
    _age(path, "obs_presentations", "created_epoch", 91)
    _age(tmp_path / "events.sqlite3", "events", "ts_epoch", 181)
    return tmp_path


def test_dry_run_counts_and_changes_nothing(state):
    before = os.path.getsize(state / "records.sqlite3")
    report = maintain.maintain(now=NOW, dry_run=True)
    rows = {entry["what"]: entry["rows"] for entry in report["rules"]}
    assert rows["obs_requests: текст запроса и ответа"] == 1
    assert rows["obs_artifacts: копии входа"] == 2
    assert rows["obs_presentations"] == 1
    assert rows["events"] == 1
    assert _counts(state / "events.sqlite3", ["events"])["events"] == 1
    assert os.path.getsize(state / "records.sqlite3") == before
    assert "--dry-run" in maintain.render(report)


def test_retention_applies_and_protects_the_ledger(state):
    protected = ["predictions", "returns", "steps", "intents", "sessions"]
    before = _counts(state / "records.sqlite3", protected)
    notes_before = _counts(state / "notes.sqlite3", ["error_notes"])

    report = maintain.maintain(now=NOW)

    assert report["errors"] == []
    assert _counts(state / "records.sqlite3", protected) == before
    assert _counts(state / "notes.sqlite3", ["error_notes"]) == notes_before
    with sqlite3.connect(state / "records.sqlite3") as conn:
        row = conn.execute("SELECT messages_json, raw_response, status, response_text FROM obs_requests").fetchone()
        assert row == ("null", None, "ok", "ответ")
        kinds = [r[0] for r in conn.execute("SELECT kind FROM obs_artifacts")]
        assert kinds == ["cues_result"]
        assert conn.execute("SELECT count(*) FROM obs_presentations").fetchone()[0] == 0
    assert _counts(state / "events.sqlite3", ["events"])["events"] == 0
    assert str(state / "records.sqlite3") in report["vacuumed"]
    # A second run finds nothing left to do.
    again = maintain.maintain(now=NOW)
    assert all(entry["rows"] == 0 for entry in again["rules"])


def test_recent_telemetry_stays(state):
    report = maintain.maintain(now=NOW - 40 * DAY)
    assert all(entry["rows"] == 0 for entry in report["rules"])


def test_backups_are_pruned_to_three(state):
    existing = len(observation.list_backups(state / "records.sqlite3"))
    for index in range(5):
        (state / f"records.sqlite3.backup-v{index}-x.sqlite3").write_bytes(b"x")
    report = maintain.maintain(now=NOW, vacuum=False)
    assert len(report["backups_removed"]) == existing + 5 - 3
    assert len(observation.list_backups(state / "records.sqlite3")) == 3


def test_missing_databases_are_fine(tmp_path):
    report = maintain.maintain(now=NOW)
    assert report["errors"] == [] and all(entry["rows"] == 0 for entry in report["rules"])


def test_rules_never_name_a_protected_table():
    for _, _, count_sql, change_sql, _ in maintain.RULES:
        for table in maintain.PROTECTED_TABLES:
            assert f" {table} " not in f" {maintain._table_of(change_sql)} "
            assert maintain._table_of(count_sql) == maintain._table_of(change_sql)


def test_cli_dry_run(state, capsys):
    assert maintain.main(["--dry-run"]) == 0
    assert "затронет" in capsys.readouterr().out
