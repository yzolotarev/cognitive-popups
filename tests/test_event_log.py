
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


def test_prune_drops_only_old_events():
    db = temp_db()
    log = EventLog(db)
    log.log("hotkey", session_id="old")
    # Backdate the row instead of waiting a month.
    conn = sqlite3.connect(db)
    try:
        conn.execute("UPDATE events SET ts_epoch = ts_epoch - ?", (40 * 86400,))
        conn.commit()
    finally:
        conn.close()
    log.log("hotkey", session_id="fresh")

    removed = event_log.prune(db, older_than_days=30)

    assert removed == 1
    assert [row["session_id"] for row in fetch(db)] == ["fresh"]
