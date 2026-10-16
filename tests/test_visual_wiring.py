"""The two short scenes are wired in: «на бумагу» once per session, the card at close."""
import sys
from types import ModuleType, SimpleNamespace

from cognitive_popups.records import RecordStore


def test_paper_scene_plays_once_per_session(desktop, monkeypatch):
    played = []
    fake = ModuleType("cognitive_popups.paper_scene")
    fake.play = lambda words, anchor: played.append(words) or SimpleNamespace(finish=lambda: None)
    monkeypatch.setitem(sys.modules, "cognitive_popups.paper_scene", fake)
    import cognitive_popups
    monkeypatch.setattr(cognitive_popups, "paper_scene", fake, raising=False)
    monkeypatch.setattr(desktop, "cursor_position", lambda: (10, 20))
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.service = SimpleNamespace(session=SimpleNamespace(id="s1"))
    cues = [{"simple": w, "term": w} for w in ("тепло", "энтропия", "порядок", "время")]
    app._handover(cues)
    app._handover(cues)
    assert played == [["тепло", "энтропия", "порядок", "время"]]
    app.service.session.id = "s2"
    app._handover(cues)
    assert len(played) == 2


def test_session_card_lands_in_sky(desktop, monkeypatch, tmp_path):
    monkeypatch.setattr(desktop, "RECORDS", RecordStore(tmp_path / "records.sqlite3"))
    monkeypatch.setattr(desktop, "STATE_DIR", tmp_path)
    monkeypatch.setattr(desktop, "emit", lambda *a, **k: None)
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    note = SimpleNamespace(comment="перепутал знак")
    app._save_session_card("key", [{"started_epoch": 0.0, "finished_epoch": 10.0}], [note])
    cards = list((tmp_path / "sky").glob("*.svg"))
    assert len(cards) == 1 and "<svg" in cards[0].read_text(encoding="utf-8")
