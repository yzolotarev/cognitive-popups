"""Headless checks of the "show an example" flow: routing, token, cache.

The plan keeps this path minimal: a selection goes straight to the model, a
missing selection opens an input window instead of inventing material, a late
answer never seizes the screen, and a repeat of the same context is served from
the cache. Each of those is one assertion below.
"""
import json
from types import SimpleNamespace

import pytest

from cognitive_popups.service import CognitiveService


@pytest.fixture(autouse=True)
def isolated_state(monkeypatch, tmp_path):
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    monkeypatch.setenv("COGNITIVE_RECORD_DB", str(tmp_path / "records.sqlite3"))
    monkeypatch.setenv("COGNITIVE_EVENT_DB", str(tmp_path / "events.sqlite3"))
    monkeypatch.setenv("COGNITIVE_NOTES_DB", str(tmp_path / "notes.sqlite3"))
    yield


def _app(desktop, monkeypatch):
    """A DesktopApp whose result window only records what it was shown."""
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app._busy = False
    app._example_cache = {}
    app._example_token = 0
    app._last_example = None
    app._shown = []
    app.flash = SimpleNamespace(
        show_text=lambda *args, **kwargs: app._shown.append((args, kwargs))
    )
    return app


class _StubClient:
    """Enough of a client that the cache key can be built without a network."""

    model = "test-model"
    url = "http://stub"

    def __getattr__(self, name):  # pragma: no cover - a call here is a bug
        raise AssertionError(f"the model must not be reached: {name}")


def test_selection_shows_an_example_without_any_form(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    monkeypatch.setattr(desktop, "selected_text", lambda: "  выделенный материал  ")
    app._example_input = lambda material: pytest.fail("a selection must not open a form")
    calls = []
    app._run_example = lambda material, query, intent: calls.append((material, query, intent))
    desktop.begin_interaction("hotkey", window="example")

    app.show_example()

    # Direct pass: the selection is the material, nothing else is mixed in.
    assert calls == [("выделенный материал", "", "")]


def test_missing_selection_offers_input_instead_of_substituting_text(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    monkeypatch.setattr(desktop, "selected_text", lambda: "   ")
    opened = []
    app._example_input = lambda material: opened.append(material) or None
    app._run_example = lambda *args: pytest.fail("there is nothing to illustrate")
    desktop.begin_interaction("hotkey", window="example")

    app.show_example()

    assert opened == ["   "]


def test_example_uses_the_intent_only_when_explicitly_checked(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    monkeypatch.setattr(desktop, "selected_text", lambda: "")
    current = SimpleNamespace(text="понять предел")
    monkeypatch.setattr(desktop.RECORDS, "current_intention", lambda: current)
    calls = []
    app._run_example = lambda material, query, intent: calls.append((material, query, intent))
    desktop.begin_interaction("hotkey", window="example")

    app._example_input = lambda material: {
        "material": "материал", "query": "мой запрос", "use_intent": False,
    }
    app.show_example(with_request=True)
    # The saved bookmark exists, but an unchecked box means it stays out.
    assert calls == [("материал", "мой запрос", "")]

    app._example_input = lambda material: {"material": "материал", "query": "", "use_intent": True}
    app.show_example(with_request=True)
    assert calls[-1] == ("материал", "", "понять предел")


def test_late_example_answer_is_dropped(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    app._busy = True
    app._example_token = 2
    desktop.begin_interaction("hotkey", window="example")

    app._example_done(1, ("stale",), SimpleNamespace(text="old", intent_snapshot=""), None)

    assert app._shown == []
    assert app._example_cache == {}
    assert app._busy is True


def test_matching_example_answer_is_shown_and_cached(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    app._busy = True
    app._example_token = 7
    result = SimpleNamespace(text="маленький пример", intent_snapshot="понять предел")
    desktop.begin_interaction("hotkey", window="example")

    app._example_done(7, ("key",), result, None)

    assert app._busy is False
    assert app._example_cache[("key",)] is result
    assert app._shown[0][0][0] == "маленький пример"
    assert app._shown[0][1]["note"] == "Учтена цель: понять предел"
    assert app._last_example == {"text": "маленький пример",
                                 "note": "Учтена цель: понять предел"}


def test_cached_example_skips_the_model(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    app.service = CognitiveService(_StubClient())
    result = SimpleNamespace(text="из кеша", intent_snapshot="")
    key = app.service.example_cache_key("материал", "", "")
    app._example_cache[key] = result
    desktop.begin_interaction("hotkey", window="example")

    app._run_example("материал", "", "")

    assert app._shown[0][0][0] == "из кеша"
    assert app._busy is False


def test_example_note_is_optional_in_the_result_payload(desktop, monkeypatch):
    sent = []

    class Sink:
        def write(self, text):
            sent.append(json.loads(text))

        def close(self):
            pass

    monkeypatch.setattr(desktop, "popup_helper_command", lambda: ["helper"])
    monkeypatch.setattr(desktop, "cursor_position", lambda: None)
    monkeypatch.setattr(desktop.subprocess, "Popen", lambda *a, **kw: SimpleNamespace(stdin=Sink()))
    desktop.begin_interaction("hotkey", window="example")
    flash = desktop.FlashWindow()

    flash.show_text("пример", "Покажи на примере")
    assert "note" not in sent[0]

    flash.show_text("пример", "Покажи на примере", note="Учтена цель: X")
    assert sent[1]["note"] == "Учтена цель: X"


def test_mode_menu_routes_every_action(desktop):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    seen = []
    for name in ("seed", "start_feynman", "start_prediction", "start_reframe",
                 "explain_terms", "show_example", "show_intent", "show_keys"):
        setattr(app, name, (lambda label: lambda *a, **k: seen.append(label))(name))

    actions = [item["action"] for item in desktop.MODE_MENU]
    assert set(actions) == {"example", "intent", "four_words", "feynman",
                            "prediction", "reframe", "clarify", "keys"}
    for action in actions:
        app.run_mode(action)

    assert seen == [
        "show_example", "show_intent", "seed", "start_feynman",
        "start_prediction", "start_reframe", "explain_terms", "show_keys",
    ]
