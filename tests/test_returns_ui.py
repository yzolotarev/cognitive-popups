"""A missed guess comes back once a day, before Ctrl+Alt+P or Alt+I, without the text."""
import time
from datetime import datetime

from cognitive_popups import returns
from cognitive_popups.models import CognitiveSession, PredictionCheck
from cognitive_popups.records import RecordStore


def _app(desktop, monkeypatch, tmp_path):
    records = RecordStore(tmp_path / "records.sqlite3")
    monkeypatch.setattr(desktop, "RECORDS", records)
    monkeypatch.setattr(desktop, "emit", lambda *a, **k: None)
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.service = type("S", (), {"session": CognitiveSession(id="s")})()
    return app, records


def _missed(app, desktop, created):
    check = PredictionCheck(buffer_fragment_ids=[], hypothesis="тепло течёт к холодному",
                            status="contradicted", mismatch="про энтропию", id="p1",
                            created_at=datetime.fromtimestamp(created).astimezone().isoformat())
    assert app._commit_prediction(check)


def test_prompt_shows_the_guess_and_the_split_never_the_source(desktop):
    text = desktop.return_prompt({"hypothesis": "моя мысль", "mismatch": "вот тут", "created_epoch": 0})
    assert "моя мысль" in text and "вот тут" in text and "без текста" in text


def test_missed_guess_is_scheduled_and_answered(desktop, monkeypatch, tmp_path):
    app, records = _app(desktop, monkeypatch, tmp_path)
    now = time.time()
    _missed(app, desktop, now - 3 * returns.DAY)
    store = returns.ReturnStore(records)
    monkeypatch.setattr(desktop.time, "time", lambda: now + 3 * returns.DAY)
    prompts = []
    monkeypatch.setattr(desktop, "popup_input", lambda text, title, **k: prompts.append(text) or "моё объяснение")
    monkeypatch.setattr(app, "_choose", lambda *a, **k: "better", raising=False)
    app._offer_return()
    assert "тепло течёт к холодному" in prompts[0]
    assert store.confirmed_later("p1")
    # Only one return a day: the next press goes straight to the action.
    prompts.clear()
    app._offer_return()
    assert prompts == []


def test_closing_the_return_counts_as_not_now(desktop, monkeypatch, tmp_path):
    app, records = _app(desktop, monkeypatch, tmp_path)
    now = time.time()
    _missed(app, desktop, now - 3 * returns.DAY)
    monkeypatch.setattr(desktop.time, "time", lambda: now + 3 * returns.DAY)
    monkeypatch.setattr(desktop, "popup_input", lambda *a, **k: None)
    app._offer_return()
    conn = records._connect()
    try:
        assert conn.execute("SELECT outcome FROM return_answers").fetchone()[0] == "skip"
    finally:
        conn.close()
