import json

from cognitive_popups.history import SessionHistory
from cognitive_popups.models import CognitiveSession


def test_fragment_requires_exactly_four_cues():
    session = CognitiveSession()
    fragment = session.add_fragment("A source fragment", ["one", "two", "three", "four"])
    assert fragment.source_text == "A source fragment"
    assert len(session.buffer_context()) > 0


def test_feynman_does_not_clear_buffer():
    session = CognitiveSession()
    session.add_fragment("One", ["a", "b", "c", "d"])
    session.add_fragment("Two", ["e", "f", "g", "h"])

    new_session = session.clear()

    assert len(session.fragments) == 2
    assert len(new_session.fragments) == 0


def test_history_round_trip(tmp_path):
    session = CognitiveSession(title="Test")
    session.add_fragment("Long source text", ["a", "b", "c", "d"])
    path = tmp_path / "history.json"
    history = SessionHistory(path)
    history.append(session)

    restored = history.load()

    assert len(restored) == 1
    assert restored[0].title == "Test"
    assert restored[0].fragments[0].cues == ["a", "b", "c", "d"]
