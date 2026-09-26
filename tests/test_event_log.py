
import os
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

from cognitive_popups import event_log
from cognitive_popups.event_log import EventChain, EventLog, fetch, render_timeline


def temp_db() -> Path:
    return Path(tempfile.mkdtemp()) / "events.sqlite3"


def src_root() -> str:
    return str(Path(event_log.__file__).resolve().parent.parent)


def test_log_and_fetch_roundtrip():
    db = temp_db()
    log = EventLog(db)

    row_id = log.log("window_open", session_id="s1", origin="popup", window="dual", layer=1)

    assert row_id == 1
    rows = fetch(db)
    assert len(rows) == 1
    assert rows[0]["event"] == "window_open"
    assert rows[0]["window"] == "dual"
    assert rows[0]["layer"] == 1
    assert rows[0]["session_id"] == "s1"
    # ts_utc is ISO with milliseconds; ts_epoch is the same instant as a float.
    assert len(rows[0]["ts_utc"]) == len("2026-09-15T10:00:00.123+00:00")
    assert isinstance(rows[0]["ts_epoch"], float)


def test_chain_links_each_event_to_the_previous():
    db = temp_db()
    chain = EventChain(EventLog(db), "s1", origin="popup")

    first = chain.emit("window_open", window="dual", layer=1)
    second = chain.emit("click", window="dual", layer=1, item_index=3, item_label="chunking")
    third = chain.emit("layer_open", window="dual", layer=2)

    rows = {row["id"]: row for row in fetch(db)}
    assert rows[first]["parent_id"] is None
    assert rows[second]["parent_id"] == first
    assert rows[third]["parent_id"] == second
    assert rows[second]["item_index"] == 3
    assert rows[second]["item_label"] == "chunking"


def test_chain_from_env_continues_the_parent_process():
    saved = dict(os.environ)
    os.environ[event_log.ENV_SESSION] = "parent-session"
    os.environ[event_log.ENV_PARENT] = "42"
    try:
        chain = EventChain.from_env(EventLog(temp_db()), origin="popup")
    finally:
        os.environ.clear()
        os.environ.update(saved)

    assert chain.session_id == "parent-session"
    assert chain.parent_id == 42
    assert chain.env() == {
        event_log.ENV_SESSION: "parent-session",
        event_log.ENV_PARENT: "42",
        event_log.ENV_OPERATION: chain.operation_id,
    }


def test_child_process_attaches_events_to_parent_chain():
    # The desktop spawns popups as separate processes; this is the contract that
    # keeps a click in the child attached to the window the parent opened.
    db = temp_db()
    chain = EventChain(EventLog(db), "s1", origin="desktop")
    parent_id = chain.emit("window_spawn", window="dual", detail="4 cues")

    script = (
        "from cognitive_popups.event_log import EventChain, EventLog\n"
        f"chain = EventChain.from_env(EventLog({str(db)!r}), origin='popup')\n"
        "chain.emit('window_open', window='dual', layer=1)\n"
        "chain.emit('click', window='dual', layer=1, item_index=3, item_label='chunking')\n"
    )
    subprocess.run(
        [sys.executable, "-c", script],
        check=True,
        timeout=60,
        env={**os.environ, "PYTHONPATH": src_root(), **chain.env()},
    )

    rows = fetch(db, session="s1")
    assert [row["origin"] for row in rows] == ["desktop", "popup", "popup"]
    open_row = next(row for row in rows if row["event"] == "window_open")
    click_row = next(row for row in rows if row["event"] == "click")
    assert open_row["parent_id"] == parent_id
    assert click_row["parent_id"] == open_row["id"]


def test_disabled_log_returns_none_and_writes_nothing():
    db = temp_db()
    log = EventLog(db, enabled=False)

    assert log.log("hotkey", session_id="s1") is None
    assert not db.exists()


def test_unwritable_database_never_raises():
    # A directory in place of the file stands in for a broken log location: the
    # UI must keep working and simply lose the events.
    broken = Path(tempfile.mkdtemp()) / "events.sqlite3"
    broken.mkdir()
    log = EventLog(broken)

    assert log.log("hotkey", session_id="s1") is None


def test_render_timeline_shows_layers_and_clicked_labels():
    db = temp_db()
    chain = EventChain(EventLog(db), "abc123", origin="desktop")
    chain.emit("hotkey", window="seed", detail="4 слова")
    chain.emit("window_open", window="dual", layer=1, detail="4 cues")

    text = render_timeline(fetch(db))

    assert "session abc123" in text
    assert "hotkey" in text
    assert "dual" in text
    assert "L1" in text
    assert "4 слова" in text


def test_long_labels_are_clipped():
    db = temp_db()
    EventLog(db).log("click", session_id="s1", item_label="x" * 5000, detail="y" * 5000)

    row = fetch(db)[0]
    assert len(row["item_label"]) == event_log.ITEM_LABEL_LIMIT
    assert len(row["detail"]) == event_log.DETAIL_LIMIT
    assert row["item_label"].endswith("…")


def test_schema_is_versioned():
    db = temp_db()
    EventLog(db).log("hotkey", session_id="s1")

    conn = sqlite3.connect(db)
    try:
        version = conn.execute("PRAGMA user_version").fetchone()[0]
    finally:
        conn.close()
    assert version == event_log.SCHEMA_VERSION


def test_legacy_migration_backs_up_and_preserves_version(tmp_path):
    db = tmp_path / "events.sqlite3"
    with sqlite3.connect(db) as conn:
        conn.executescript(event_log._SCHEMA)
        conn.execute("PRAGMA user_version=7")
        conn.execute("INSERT INTO events(ts_utc,ts_epoch,session_id,origin,event) VALUES ('old',1,'s','app','old')")
    log = EventLog(db)
    assert log.log("new", session_id="s", payload_json={"full": "x" * 10000}, operation_id="op")
    rows = fetch(db)
    assert rows[0]["event"] == "old"
    assert rows[1]["operation_id"] == "op"
    import json
    assert len(json.loads(rows[1]["payload_json"])["full"]) == 10000
    backups = list(tmp_path.glob("*.backup-*.sqlite3"))
    assert len(backups) == 1
    with sqlite3.connect(backups[0]) as conn:
        assert conn.execute("SELECT count(*) FROM events").fetchone()[0] == 1
        assert "operation_id" not in {r[1] for r in conn.execute("PRAGMA table_info(events)")}
    assert EventLog(db).log("again", session_id="s")
    assert len(list(tmp_path.glob("*.backup-*.sqlite3"))) == 1
    with sqlite3.connect(db) as conn:
        assert conn.execute("PRAGMA user_version").fetchone()[0] == 7
        assert not conn.execute("SELECT name FROM sqlite_master WHERE name='model_requests'").fetchall()


def test_dynamic_destination_and_chain_context(tmp_path, monkeypatch):
    from cognitive_popups.operation_context import current_operation, RUN_ID
    monkeypatch.delenv("COGNITIVE_EVENT_DB", raising=False)
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path / "one"))
    first = event_log.default_log()
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path / "two"))
    second = event_log.default_log()
    assert first.path != second.path
    chain = EventChain(second, "session", operation_id="operation")
    assert chain.bind(current_operation)() == chain.context
    assert current_operation() is None
    chain.emit("event", operation_id="override", run_id="explicit")
    chain.emit("event")
    rows = fetch(second.path)
    assert rows[0]["operation_id"] == "override"
    assert rows[0]["run_id"] == "explicit"
    assert rows[1]["operation_id"] == "operation"
    assert rows[1]["run_id"] == RUN_ID
    monkeypatch.setenv(event_log.ENV_SESSION, "session")
    monkeypatch.setenv(event_log.ENV_OPERATION, "operation")
    assert EventChain.from_env(second).context == chain.context


def test_disable_after_construction(tmp_path, monkeypatch):
    log = EventLog(tmp_path / "absent" / "events.sqlite3")
    monkeypatch.setenv("COGNITIVE_OBSERVATION_DISABLE", "true")
    assert log.log("event", session_id="s") is None
    assert not log.path.parent.exists()
