"""The overview is a mirror: the reader's own words, no rates, streaks or minutes."""
import re
import sqlite3
import time

import pytest

from cognitive_popups import overview
from cognitive_popups.event_log import EventLog
from cognitive_popups.models import CognitiveSession, PredictionCheck
from cognitive_popups.notes import NoteStore
from cognitive_popups.records import RecordStore
from cognitive_popups.returns import ReturnStore

NOW = time.time()


def _set_epoch(path, table, column, value, where="1"):
    with sqlite3.connect(path) as conn:
        conn.execute(f"UPDATE {table} SET {column} = ? WHERE {where}", (value,))


@pytest.fixture
def state(tmp_path, monkeypatch):
    monkeypatch.delenv("COGNITIVE_EVENT_DISABLE", raising=False)
    records = RecordStore()
    session = CognitiveSession(id="s")
    for index in range(4):
        fragment = session.add_fragment(f"текст {index}", cues=["индекс", "ячейка", f"x{index}", f"y{index}"],
                                        cue_details=[{"simple": "s", "term": term, "meaning": "m"}
                                                     for term in ("Индекс", "ячейка", f"x{index}", f"y{index}")])
        records.save_fragment(session, fragment)
    for index, status in enumerate(("contradicted", "confirmed", "not_supported", "partially_confirmed",
                                    "contradicted", "contradicted", "not_supported")):
        records.save_prediction(session, PredictionCheck(
            buffer_fragment_ids=[], hypothesis=f"моя гипотеза {index}", status=status,
            mismatch=f"место {index}", evidence=f"цитата {index}", id=f"p{index}"))
        _set_epoch(records.path, "predictions", "created_epoch", NOW - 100 + index, f"id = 'p{index}'")
    ReturnStore(records).schedule("p0", NOW)
    notes = NoteStore()
    notes.add("думал, что это прилагательное", anchor="rigorous argument")
    notes.add("опять он", anchor="VLOOKUP")
    notes.add("и снова", anchor="vlookup")
    _set_epoch(notes.path, "error_notes", "created_epoch", NOW - 3600, "anchor = 'rigorous argument'")
    log = EventLog()
    for start, gap in ((NOW - 900, 40), (NOW - 600, 60), (NOW - 300, 90)):
        log.log("result", session_id="e", window="seed", detail="4 cues")
        log.log("hotkey", session_id="e", window="prediction")
        with sqlite3.connect(log.path) as conn:
            ids = [row[0] for row in conn.execute("SELECT id FROM events ORDER BY id DESC LIMIT 2")]
            conn.execute("UPDATE events SET ts_epoch = ? WHERE id = ?", (start, ids[1]))
            conn.execute("UPDATE events SET ts_epoch = ? WHERE id = ?", (start + gap, ids[0]))
    log.log("action", session_id="e", window="seed", detail="busy, request dropped")
    log.log("action", session_id="e", window="seed", detail="no selection")
    log.log("error", session_id="e", window="seed", detail="не удалось получить четыре объекта")
    return tmp_path


def test_blocks_hold_the_readers_words(state):
    data = overview.overview(state, NOW - 7 * overview.DAY, now=NOW + 3 * overview.DAY)
    assert [m["hypothesis"] for m in data["misses"]] == [f"моя гипотеза {i}" for i in (6, 5, 4, 3, 2)]
    assert data["misses"][0]["mismatch"] == "место 6" and data["misses"][0]["evidence"] == "цитата 6"
    assert {"term": "индекс", "count": 4} in data["repeated_terms"]
    assert data["repeated_anchors"] == [{"anchor": "VLOOKUP", "count": 2}]
    assert [r["prediction_id"] for r in data["returns"]] == ["p0"]
    assert data["open_notes"] == 3
    assert data["oldest_note"]["comment"] == "думал, что это прилагательное"
    assert data["rhythm"] == {"median_seconds": 60, "n": 3}
    assert data["counts"] == {"сессий": 1, "4 слова": 4, "гипотез": 7, "заметок": 3, "шагов": 0}
    assert data["system"]["нажатий потеряно"] == 1
    assert data["system"]["текст не захвачен"] == 1
    assert data["system"]["окон с ошибкой"] == 1


def test_render_has_no_rates_streaks_or_minutes(state):
    text = overview.render(overview.overview(state, None, now=NOW + 3 * overview.DAY))
    assert "%" not in text
    for word in ("сери", "подряд", "streak", "мин", "процент", "лучше", "хуже", "уровень"):
        assert not re.search(word, text, re.IGNORECASE), word
    assert "моя гипотеза 6" in text and "расхождение: место 6" in text
    assert "«rigorous argument» думал, что это прилагательное" in text
    assert "около 60 с" in text
    assert "сессий 1 · 4 слова 4 · гипотез 7 · заметок 3 · шагов 0" in text


def test_empty_state_renders_quietly(tmp_path):
    data = overview.overview(tmp_path, None)
    text = overview.render(data)
    assert data["misses"] == [] and data["rhythm"] is None and data["oldest_note"] is None
    assert "пока нет" in text and "%" not in text
    assert list(tmp_path.iterdir()) == [], "the overview never creates files"


def test_period_filters_old_misses(state):
    data = overview.overview(state, NOW + 10)
    assert data["misses"] == [] and data["counts"]["гипотез"] == 0


def test_cli(state, capsys):
    assert overview.main(["--all"]) == 0
    assert "ОБЗОР" in capsys.readouterr().out
    assert overview.main(["--days", "7", "--json"]) == 0
    assert '"misses"' in capsys.readouterr().out
