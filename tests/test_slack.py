"""Background model work feeds only on the reader's free attention."""
import json
import os
import sqlite3
import time

from cognitive_popups import slack


def test_busy_while_a_step_runs_a_popup_lives_or_just_after_an_action(tmp_path):
    assert slack.state(tmp_path) == slack.FREE          # nothing ever happened here
    slack.touch_activity(tmp_path)
    assert slack.state(tmp_path) == slack.BUSY          # just acted
    old = time.time() - 120
    os.utime(tmp_path / slack.ACTIVITY_FILE, (old, old))
    assert slack.state(tmp_path) == slack.TRICKLE       # back soon, maybe
    marker = slack.mark_popup_open(tmp_path)
    assert slack.state(tmp_path) == slack.BUSY          # a window is open
    slack.mark_popup_closed(marker, tmp_path)
    assert not slack.popups_open(tmp_path)
    (tmp_path / "focus.json").write_text(json.dumps({"status": "running"}), encoding="utf-8")
    old = time.time() - 3600
    os.utime(tmp_path / slack.ACTIVITY_FILE, (old, old))
    assert slack.state(tmp_path) == slack.BUSY          # a step is running


def test_markers_of_dead_popups_are_cleaned(tmp_path):
    directory = tmp_path / slack.POPUPS_DIR
    directory.mkdir()
    (directory / "999999999").write_text("x")
    assert not slack.popups_open(tmp_path)
    assert not any(directory.iterdir())


def _events(path, gaps, window="seed"):
    db = sqlite3.connect(path)
    db.execute("create table events (ts_epoch real, window text, event text)")
    moment = 1000.0
    rows = [(moment, window, "hotkey")]
    for gap in gaps:
        moment += gap
        rows.append((moment, window, "hotkey"))
    db.executemany("insert into events values (?, ?, ?)", rows)
    db.commit()


def test_the_gone_pause_is_learned_from_the_readers_own_log(tmp_path):
    # Back within seconds after 3 minutes of quiet, never after 5.
    _events(tmp_path / "events.sqlite3", [185] * 30 + [400] * 30)
    assert slack.learn_free_after(tmp_path / "events.sqlite3") == 300
    os.environ.pop("COGNITIVE_EVENT_DB", None)
    assert slack.free_after(tmp_path) == 300
    saved = json.loads((tmp_path / slack.RHYTHM_FILE).read_text())
    assert saved["free_after"] == 300 and saved["table"]


def test_without_enough_evidence_the_default_holds(tmp_path):
    _events(tmp_path / "events.sqlite3", [30, 40])
    assert slack.learn_free_after(tmp_path / "events.sqlite3") == slack.DEFAULT_FREE_AFTER
