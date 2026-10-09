"""The study timeline merges every store onto one local clock, read-only."""
import sqlite3

from cognitive_popups import timeline


def make(tmp_path):
    events = sqlite3.connect(tmp_path / "events.sqlite3")
    events.execute("create table events (id integer primary key, ts_utc text, window text,"
                   " event text, detail text)")
    events.executemany("insert into events (ts_utc, window, event, detail) values (?,?,?,?)", [
        ("2026-09-20T10:00:00+00:00", "seed", "hotkey", "4 слова"),
        ("2026-09-20T10:00:01+00:00", "seed", "result", "4 cues"),
        ("2026-09-20T10:00:02+00:00", "feynman", "action", "buffer is empty"),
        ("2026-09-20T10:00:03+00:00", "input", "window_open", "Какие термины не понял? Через запятую."),
        ("2026-09-20T10:00:04+00:00", "input", "window_open", "Объяснить: буфер"),
        ("2026-09-20T10:00:09+00:00", "hud", "error", "boom"),
    ])
    events.commit()
    records = sqlite3.connect(tmp_path / "records.sqlite3")
    records.execute("create table predictions (created_utc text, hypothesis text, status text,"
                    " mismatch text, one_delta text)")
    records.execute("insert into predictions values ('2026-09-20T10:00:05+00:00', 'моя версия',"
                    " 'contradicted', '', 'расхождение')")
    records.commit()
    notes = sqlite3.connect(tmp_path / "notes.sqlite3")
    notes.execute("create table error_notes (created_utc text, comment text, anchor text)")
    notes.execute("insert into error_notes values ('2026-09-20T10:00:06+00:00', 'моя мысль', 'абзац')")
    notes.commit()
    return {name: tmp_path / f"{name}.sqlite3" for name in ("events", "records", "notes")}


def test_merges_stores_in_time_order_and_keeps_only_meaningful(tmp_path):
    entries = timeline.collect(make(tmp_path))
    assert [e.kind for e in entries] == [
        "клавиша", "сорвалось", "окно", "ГИПОТЕЗА", "ЗАМЕТКА", "СБОЙ"]
    assert entries[1].text == "feynman: буфер пуст"
    assert entries[2].text == "Объяснить: буфер"
    assert entries[3].extra == "расхождение"
    assert entries[4].extra == "к: абзац"
    assert all(e.at.utcoffset() is not None for e in entries)


def test_full_keeps_technical_events(tmp_path):
    kinds = [e.kind for e in timeline.collect(make(tmp_path), full=True)]
    assert kinds.count("·") == 2  # the generic prompt window and the result


def test_missing_stores_and_tables_contribute_nothing(tmp_path):
    paths = make(tmp_path)
    (tmp_path / "records.sqlite3").unlink()
    paths["notes"] = tmp_path / "absent.sqlite3"
    kinds = [e.kind for e in timeline.collect(paths)]
    assert "ГИПОТЕЗА" not in kinds and "ЗАМЕТКА" not in kinds and "клавиша" in kinds


def test_day_summary_counts_own_words(tmp_path):
    lines = timeline.day_summary(timeline.collect(make(tmp_path)))
    assert len(lines) == 2
    assert lines[1].split()[2:] == ["1", "1", "0", "1", "1", "1"]


def test_cli_day_and_empty_day(tmp_path, monkeypatch, capsys):
    paths = make(tmp_path)
    monkeypatch.setattr(timeline, "state_paths", lambda: paths)
    day = timeline.collect(paths)[0].at.date().isoformat()
    assert timeline.main([day]) == 0
    assert "моя мысль" in capsys.readouterr().out
    assert timeline.main(["2001-01-01"]) == 1
