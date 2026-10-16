"""Schema never goes down, backups stay few, crashed sessions close honestly."""
from __future__ import annotations

import io
import json
import os
import sqlite3
import time
from contextlib import redirect_stderr

from cognitive_popups import observation, records
from cognitive_popups.models import CognitiveSession, PredictionCheck


def _old_database(path, version=11):
    """A current-layout ledger that claims an older (or newer) version."""
    with sqlite3.connect(path) as conn:
        conn.executescript(records._SCHEMA)
        for table, column, kind in records._ADDED_COLUMNS:
            have = {row[1] for row in conn.execute(f"PRAGMA table_info({table})")}
            if column not in have:
                conn.execute(f"ALTER TABLE {table} ADD COLUMN {column} {kind}")
        conn.execute(f"PRAGMA user_version={version}")
        conn.execute("INSERT INTO sessions (id, created_utc, created_epoch) VALUES ('s', '2026-10-01T08:00:00+00:00', 1000)")


def _version(path) -> int:
    with sqlite3.connect(path) as conn:
        return conn.execute("PRAGMA user_version").fetchone()[0]


def test_a_newer_database_keeps_its_version_and_warns(tmp_path):
    path = tmp_path / "records.sqlite3"
    _old_database(path, version=records.SCHEMA_VERSION + 3)
    stderr = io.StringIO()
    with redirect_stderr(stderr):
        records.RecordStore(path).sessions()
    assert _version(path) == records.SCHEMA_VERSION + 3
    assert "newer than this code" in stderr.getvalue()
    assert not list(tmp_path.glob("records.sqlite3.backup-*"))


def test_upgrade_backs_up_with_version_in_the_name_and_keeps_three(tmp_path):
    path = tmp_path / "records.sqlite3"
    _old_database(path, version=11)
    for index in range(5):
        old = tmp_path / f"records.sqlite3.backup-{index:032x}.sqlite3"
        old.write_bytes(b"old")
        (tmp_path / (old.name + "-wal")).write_bytes(b"")
        os.utime(old, (time.time() - 1000 + index, time.time() - 1000 + index))
    by_hand = tmp_path / "records.sqlite3.backup-before-obs-excluded"
    by_hand.write_bytes(b"mine")

    records.RecordStore(path).sessions()

    assert _version(path) == records.SCHEMA_VERSION
    kept = observation.list_backups(path)
    assert len(kept) == 3
    assert kept[0].name.startswith("records.sqlite3.backup-v11-")
    assert by_hand.exists()
    assert not (tmp_path / f"records.sqlite3.backup-{0:032x}.sqlite3-wal").exists()


def test_prune_backups_dry_run_deletes_nothing(tmp_path):
    path = tmp_path / "events.sqlite3"
    for index in range(4):
        (tmp_path / f"events.sqlite3.backup-v1-{index}.sqlite3").write_bytes(b"x")
    would = observation.prune_backups(path, keep=1, dry_run=True)
    assert len(would) == 3
    assert len(observation.list_backups(path)) == 4


def test_backup_timeout_grows_with_size(tmp_path):
    small = tmp_path / "small"
    small.write_bytes(b"x")
    assert observation.backup_timeout(small) == observation.BACKUP_MIN_SECONDS
    big = tmp_path / "big"
    with open(big, "wb") as handle:
        handle.truncate(100 << 20)
    assert observation.backup_timeout(big) > observation.BACKUP_MIN_SECONDS


def test_latest_preparation_is_the_newest(tmp_path):
    store = records.RecordStore(tmp_path / "records.sqlite3")
    store.save_preparation(preparation_id="old", context_key="a", status="ready",
                           created_utc="2026-10-01T08:00:00+00:00")
    store.save_preparation(preparation_id="new", context_key="b", status="queued",
                           created_utc="2026-10-10T08:00:00+00:00")
    assert store.latest_preparation()["id"] == "new"


def test_close_stale_uses_the_last_activity_not_now(tmp_path):
    store = records.RecordStore(tmp_path / "records.sqlite3")
    session = CognitiveSession(id="crashed", created_at="2026-10-01T08:00:00+00:00")
    fragment = session.add_fragment("text", cues=list("abcd"),
                                    cue_details=[{"simple": c, "term": c, "meaning": c} for c in "abcd"])
    fragment.created_at = "2026-10-01T08:20:00+00:00"
    store.save_fragment(session, fragment)
    store.save_prediction(session, PredictionCheck(buffer_fragment_ids=[], hypothesis="h", status="confirmed",
                                                   created_at="2026-10-01T08:40:00+00:00"))
    store.open_session("empty", created_utc="2026-10-02T09:00:00+00:00")

    assert store.close_stale("crash") == 2
    metas = {row["id"]: row for row in store.sessions()}
    assert metas["crashed"]["closed_utc"] == "2026-10-01T08:40:00+00:00"
    assert metas["crashed"]["close_reason"] == "crash"
    assert metas["empty"]["closed_utc"] == "2026-10-02T09:00:00+00:00"


def test_node_words_move_out_of_one_delta_once(tmp_path):
    path = tmp_path / "records.sqlite3"
    _old_database(path, version=11)
    rows = (
        ("words", "округление · отрицательные · усечение", "[]"),
        ("two-word-nodes", "дробная часть · целое число · знак", "[]"),
        ("already", "поле · строка", '["поле", "строка"]'),
        ("sentence", "Здесь не так: формула · другая часть.", "[]"),
        ("plain", "Одна поправка без точки", "[]"),
    )
    with sqlite3.connect(path) as conn:
        for pid, delta, nodes in rows:
            conn.execute("INSERT INTO predictions (id, session_id, hypothesis, status, one_delta, nodes_json,"
                         " created_utc, created_epoch) VALUES (?, 's', 'h', 'contradicted', ?, ?, 'x', 1)",
                         (pid, delta, nodes))

    loaded = {check.id: check for check in records.RecordStore(path).load_predictions("s")}
    assert loaded["words"].nodes == ["округление", "отрицательные", "усечение"]
    assert loaded["words"].one_delta == ""
    assert loaded["two-word-nodes"].nodes == ["дробная часть", "целое число", "знак"]
    assert loaded["already"].one_delta == ""
    assert loaded["sentence"].one_delta == "Здесь не так: формула · другая часть."
    assert loaded["sentence"].nodes == []
    assert loaded["plain"].one_delta == "Одна поправка без точки"


def test_looks_like_node_words():
    assert records.looks_like_node_words("a · b") == ["a", "b"]
    assert records.looks_like_node_words("single") == []
    assert records.looks_like_node_words("one two three four · b") == []
    assert records.looks_like_node_words("a ·  · b") == []
    assert json.dumps(records.looks_like_node_words("x · y"))
