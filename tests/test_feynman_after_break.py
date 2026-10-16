"""Alt+F right after a break checks the passage being read, not "buffer is empty"."""
from types import SimpleNamespace


def test_feynman_uses_the_passage_from_before_the_break(desktop, monkeypatch):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    fragment = SimpleNamespace(id="f1", source_text="текст")
    retired = SimpleNamespace(session=SimpleNamespace(id="old", fragments=[fragment]), ended_utc="t")
    app.service = SimpleNamespace(session=SimpleNamespace(fragments=[]), mark_activity=lambda: retired)
    monkeypatch.setattr(app, "_close_session", lambda *a, **k: None, raising=False)
    monkeypatch.setattr(desktop, "emit", lambda *a, **k: None)
    app._split_idle_session()
    started = []
    app._busy = False
    app._log_prompt = lambda *a: None
    app.service.create_feynman_question = lambda: "вопрос"
    app.flash = SimpleNamespace(show_notice=lambda text: started.append(text))
    monkeypatch.setattr(desktop, "idle_add", lambda callback, *args: started.append(args[0]))
    monkeypatch.setattr(desktop, "bind_chain", lambda fn: fn)
    monkeypatch.setattr(desktop.threading, "Thread", lambda target, daemon: SimpleNamespace(start=target))
    app.start_feynman()
    assert app.service.session.fragments == [fragment]
    assert started == ["вопрос"]
