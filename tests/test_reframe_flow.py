"""Alt+R generates straight away; material and focus come from the session.

Pressing the key is the whole request. The material is the live selection, or the
exact fragments of the last hypothesis; the focus is that hypothesis, or the
accepted goal, or nothing at all — in which case the model picks the frame. The
only window left is the optional one on Alt+Shift+R.
"""
from types import SimpleNamespace

import pytest

from cognitive_popups.models import CognitiveSession, PredictionCheck


def _app(desktop):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app._busy = False
    app.service = SimpleNamespace(session=CognitiveSession())
    shown = []
    app.flash = SimpleNamespace(
        show_text=lambda *args, **kwargs: shown.append((args, kwargs))
    )
    return app, shown


def _wire(desktop, app, monkeypatch, calls):
    """Run the generation inline and record what the service was asked for."""
    monkeypatch.setattr(desktop.threading, "Thread", lambda **kwargs:
                        SimpleNamespace(start=kwargs["target"]))
    monkeypatch.setattr(app, "_log_prompt", lambda *args: None)
    app.service.reframe = lambda *args: calls.append(args) or SimpleNamespace(
        text="ответ", status="proposal"
    )


def test_reframe_generates_at_once_from_the_last_hypothesis(desktop, monkeypatch):
    app, _shown = _app(desktop)
    fragment = app.service.session.add_fragment("старый материал", ["a", "b", "c", "d"])
    app.service.session.add_prediction_check(PredictionCheck(
        buffer_fragment_ids=[fragment.id], hypothesis="моя связь", status="confirmed",
    ))
    monkeypatch.setattr(desktop, "selected_text_source", lambda **kwargs: ("", ""))
    monkeypatch.setattr(desktop, "popup_input",
                        lambda *a, **k: pytest.fail("no window before the answer"))
    calls = []
    _wire(desktop, app, monkeypatch, calls)
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe()

    # The reader's own claim is both the focus and the frame to differ from.
    assert calls == [("старый материал", "моя связь", "моя связь")]


def test_reframe_uses_a_live_selection_and_keeps_the_goal_as_focus(desktop, monkeypatch):
    app, _shown = _app(desktop)
    app.service.session.add_fragment("буфер", ["a", "b", "c", "d"])
    monkeypatch.setattr(desktop, "selected_text_source",
                        lambda **kwargs: ("выделенный текст", "selection"))
    monkeypatch.setattr(desktop.RECORDS, "current_intention",
                        lambda: SimpleNamespace(text="понять предел"))
    calls = []
    _wire(desktop, app, monkeypatch, calls)
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe()

    assert calls == [("выделенный текст", "понять предел", "")]


def test_reframe_with_nothing_to_derive_leaves_the_frame_to_the_model(desktop, monkeypatch):
    app, _shown = _app(desktop)
    app.service.session.add_fragment("только буфер", ["a", "b", "c", "d"])
    monkeypatch.setattr(desktop, "selected_text_source", lambda **kwargs: ("", ""))
    monkeypatch.setattr(desktop.RECORDS, "current_intention", lambda: None)
    calls = []
    _wire(desktop, app, monkeypatch, calls)
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe()

    # An empty focus means "look at this yourself"; the prompt says what to do.
    assert calls == [("только буфер", "", "")]


def test_reframe_from_a_result_uses_its_exact_source(desktop, monkeypatch):
    app, shown = _app(desktop)
    one = app.service.session.add_fragment("источник гипотезы", ["a", "b", "c", "d"])
    app.service.session.add_fragment("посторонний фрагмент", ["e", "f", "g", "h"])
    check = PredictionCheck(buffer_fragment_ids=[one.id], hypothesis="моя связь",
                            status="unclear")
    monkeypatch.setattr(desktop, "popup_input",
                        lambda *a, **k: pytest.fail("no window before the answer"))
    calls = []
    _wire(desktop, app, monkeypatch, calls)
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe(from_check=check, material_snapshot="источник гипотезы")

    assert calls == [("источник гипотезы", "моя связь", "моя связь")]
    assert shown[-1][1]["note"] == "По материалу выбранной гипотезы."


def test_reframe_ask_opens_the_field_and_uses_what_was_written(desktop, monkeypatch):
    app, _shown = _app(desktop)
    app.service.session.add_fragment("материал", ["a", "b", "c", "d"])
    monkeypatch.setattr(desktop, "selected_text_source", lambda **kwargs: ("", ""))
    monkeypatch.setattr(desktop.RECORDS, "current_intention", lambda: None)
    opened = []
    monkeypatch.setattr(desktop, "popup_input", lambda prompt, title, initial="":
                        opened.append(initial) or "свой вопрос")
    calls = []
    _wire(desktop, app, monkeypatch, calls)
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe(ask=True)

    assert opened == [""]
    assert calls == [("материал", "свой вопрос", "")]


def test_reframe_ask_cancelled_does_not_generate(desktop, monkeypatch):
    app, _shown = _app(desktop)
    app.service.session.add_fragment("материал", ["a", "b", "c", "d"])
    monkeypatch.setattr(desktop, "selected_text_source", lambda **kwargs: ("", ""))
    monkeypatch.setattr(desktop.RECORDS, "current_intention", lambda: None)
    monkeypatch.setattr(desktop, "popup_input", lambda *args, **kwargs: "")
    app.service.reframe = lambda *args: pytest.fail("a cancelled field must not generate")
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe(ask=True)

    assert app._busy is False


def test_reframe_without_material_explains_instead_of_guessing(desktop, monkeypatch):
    app, shown = _app(desktop)
    monkeypatch.setattr(desktop, "selected_text_source", lambda **kwargs: ("", ""))
    monkeypatch.setattr(desktop.RECORDS, "current_intention", lambda: None)
    app.service.reframe = lambda *args: pytest.fail("nothing to reframe")
    desktop.begin_interaction("hotkey", window="reframe")

    app.start_reframe()

    assert "Нет материала" in shown[-1][0][0]


def test_prediction_result_continues_only_after_explicit_click(desktop, monkeypatch):
    app, shown = _app(desktop)
    one = app.service.session.add_fragment("источник", ["a", "b", "c", "d"])
    app.service.session.add_fragment("другой текст", ["e", "f", "g", "h"])
    check = PredictionCheck(buffer_fragment_ids=[one.id], hypothesis="моя гипотеза",
                            status="unclear")
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

    # "Another angle" is frozen: the result offers no continuation, and
    # closing it runs nothing on its own.
    assert "actions" not in payloads[0]
    assert actions == []
    assert shown == []
