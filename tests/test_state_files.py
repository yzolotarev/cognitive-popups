"""Broken focus.json / history.json never stop the daemon; history stays bounded."""
import json

from cognitive_popups.focus import Focus
from cognitive_popups.history import SessionHistory
from cognitive_popups.models import CognitiveSession


def test_broken_focus_file_is_set_aside_and_focus_starts_clean(tmp_path, capsys):
    path = tmp_path / "focus.json"
    path.write_text('{"id": "x", "status": "run')
    flow = Focus(path, records=object(), client=None)
    assert flow.state is None
    assert not path.exists()
    assert (tmp_path / "focus.json.broken").read_text() == '{"id": "x", "status": "run'
    assert "focus.json.broken" in capsys.readouterr().err


def test_incomplete_focus_state_is_also_set_aside(tmp_path):
    path = tmp_path / "focus.json"
    path.write_text(json.dumps({"id": "x"}))
    assert Focus(path, records=object(), client=None).state is None
    assert (tmp_path / "focus.json.broken").exists()


def _session(index):
    session = CognitiveSession(title=f"буфер {index}")
    session.add_fragment(f"текст {index}", ["a", "b", "c", "d"])
    return session


def test_history_keeps_the_last_fifty_and_writes_atomically(tmp_path):
    history = SessionHistory(tmp_path / "history.json")
    for index in range(53):
        history.append(_session(index))
    loaded = history.load()
    assert len(loaded) == 50
    assert loaded[0].title == "буфер 3" and loaded[-1].title == "буфер 52"
    assert not (tmp_path / "history.json.tmp").exists()


def test_torn_history_file_is_set_aside(tmp_path):
    path = tmp_path / "history.json"
    path.write_text('[{"id": "a", "fragm')
    history = SessionHistory(path)
    assert history.load() == []
    assert (tmp_path / "history.json.broken").exists()
    history.append(_session(1))
    assert [s.title for s in history.load()] == ["буфер 1"]
