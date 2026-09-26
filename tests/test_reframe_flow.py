"""Opt-in reframing keeps its material and never starts without a reader request."""
from types import SimpleNamespace

from cognitive_popups.models import CognitiveSession, PredictionCheck


def _app(desktop):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app._busy = False
    app.service = SimpleNamespace(session=CognitiveSession())
    shown = []
    app.flash = SimpleNamespace(show_text=lambda *args, **kwargs: shown.append((args, kwargs)))
    return app, shown


def test_reframe_uses_live_selection_without_old_hypothesis(desktop, monkeypatch):
    app, _shown = _app(desktop)
    fragment = app.service.session.add_fragment("старый материал", ["a", "b", "c", "d"])
    app.service.session.add_prediction_check(PredictionCheck(
        buffer_fragment_ids=[fragment.id], hypothesis="старая гипотеза", status="confirmed",
    ))
    monkeypatch.setattr(desktop, "selected_text_source", lambda **kwargs: ("новое выделение", "selection"))
    opened = []
    monkeypatch.setattr(desktop, "popup_input", lambda prompt, title, initial="":
                        opened.append((prompt, initial)) or "новый вопрос")
    calls = []
    app.service.reframe = lambda *args: calls.append(args) or SimpleNamespace(text="ответ", status="proposal")
    monkeypatch.setattr(desktop.threading, "Thread", lambda **kwargs:
                        SimpleNamespace(start=kwargs["target"]))
    monkeypatch.setattr(app, "_log_prompt", lambda *args: None)
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe()

    assert opened[0][1] == ""
    assert calls == [("новое выделение", "новый вопрос", "")]


def test_reframe_uses_only_fragments_of_last_prediction(desktop, monkeypatch):
    app, shown = _app(desktop)
    one = app.service.session.add_fragment("источник гипотезы", ["a", "b", "c", "d"])
    app.service.session.add_fragment("посторонний фрагмент", ["e", "f", "g", "h"])
    app.service.session.add_prediction_check(PredictionCheck(
        buffer_fragment_ids=[one.id], hypothesis="моя связь", status="partially_confirmed",
    ))
    monkeypatch.setattr(desktop, "selected_text_source", lambda **kwargs: ("", ""))
    opened = []
    monkeypatch.setattr(desktop, "popup_input", lambda prompt, title, initial="":
                        opened.append((prompt, initial)) or "пересмотреть связь")
    calls = []
    app.service.reframe = lambda *args: calls.append(args) or SimpleNamespace(text="ответ", status="proposal")
    monkeypatch.setattr(desktop.threading, "Thread", lambda **kwargs:
                        SimpleNamespace(start=kwargs["target"]))
    monkeypatch.setattr(app, "_log_prompt", lambda *args: None)
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe()

    assert opened[0][1] == "моя связь"
    assert calls == [("источник гипотезы", "пересмотреть связь", "моя связь")]
    assert shown[-1][1]["note"] == "По материалу последней гипотезы."


def test_reframe_cancel_does_not_call_model(desktop, monkeypatch):
    app, _shown = _app(desktop)
    app.service.session.add_fragment("материал", ["a", "b", "c", "d"])
    monkeypatch.setattr(desktop, "selected_text_source", lambda **kwargs: ("", ""))
    monkeypatch.setattr(desktop, "popup_input", lambda *args, **kwargs: "")
    app.service.reframe = lambda *args: (_ for _ in ()).throw(AssertionError("unexpected call"))
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe()

    assert app._busy is False


def test_prediction_result_continues_only_after_explicit_click(desktop, monkeypatch):
    app, shown = _app(desktop)
    one = app.service.session.add_fragment("источник", ["a", "b", "c", "d"])
    app.service.session.add_fragment("другой текст", ["e", "f", "g", "h"])
    check = PredictionCheck(buffer_fragment_ids=[one.id], hypothesis="моя гипотеза", status="unclear")
    actions = []
    app.start_reframe = lambda **kwargs: actions.append(kwargs)
    app._split_idle_session = lambda: None
    monkeypatch.setattr(desktop.threading, "Thread", lambda **kwargs:
                        SimpleNamespace(start=kwargs["target"]))
    payloads = []
    monkeypatch.setattr(desktop, "run_popup", lambda payload, **kwargs:
                        payloads.append(payload) or "")
    desktop.begin_interaction("hotkey", window="prediction")

    app._prediction_done(check, None)

    assert payloads[0]["actions"] == [{"label": "Другой ракурс", "action": "reframe"}]
    assert actions == []
    assert shown == []

    monkeypatch.setattr(desktop, "run_popup", lambda payload, **kwargs:
                        '{"action":"reframe"}')
    app._prediction_done(check, None)

    assert actions == [{"from_check": check, "material_snapshot": "источник"}]
