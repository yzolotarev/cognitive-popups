"""Ready-made directions and the one call that words a goal: headless checks."""
import json
from types import SimpleNamespace

import pytest

from cognitive_popups import goals, prompts, records
from cognitive_popups.models import CognitiveSession
from cognitive_popups.prompt_settings import PromptSettings
from cognitive_popups.service import CognitiveService


@pytest.fixture(autouse=True)
def isolated_state(monkeypatch, tmp_path):
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    monkeypatch.setenv("COGNITIVE_RECORD_DB", str(tmp_path / "records.sqlite3"))
    monkeypatch.setenv("COGNITIVE_EVENT_DB", str(tmp_path / "events.sqlite3"))
    monkeypatch.setenv("COGNITIVE_NOTES_DB", str(tmp_path / "notes.sqlite3"))
    yield


class _StubClient:
    """One scripted model reply; the cache key only needs model and url."""

    model = "test-model"
    url = "http://stub"

    def __init__(self, reply="Понять, почему вычитание сохраняет решения"):
        self.reply = reply
        self.messages = []

    def complete(self, messages, **kwargs):
        self.messages.append(messages)
        return self.reply


def test_direction_draft_prefers_the_readers_own_words():
    label = "Понять, почему работает"

    assert goals.direction_draft(label) == goals.direction_draft(label)
    assert goals.direction_draft(label, "хочу разобраться с шагом") == "хочу разобраться с шагом"
    assert goals.direction_draft("Неизвестное направление") == ""
    assert goals.direction_draft(label)  # a known label always yields something


def test_material_comes_only_from_the_readers_own_capture():
    assert goals.choose_material("  выделено  ", goals.ORIGIN_SELECTION) == (
        "выделено", "выделение")

    # A copy is labelled as a copy: it may be something copied long before.
    assert goals.choose_material("скопировано", goals.ORIGIN_CLIPBOARD) == (
        "скопировано", "буфер обмена")

    assert goals.choose_material("", "") == ("", "")


def test_capture_origin_names_the_probe_that_succeeded():
    assert goals.capture_origin("primary-wayland") == goals.ORIGIN_SELECTION
    assert goals.capture_origin("primary-x11") == goals.ORIGIN_SELECTION
    assert goals.capture_origin("clipboard-wayland") == goals.ORIGIN_CLIPBOARD
    assert goals.capture_origin("clipboard-x11") == goals.ORIGIN_CLIPBOARD


def test_large_material_is_flagged_not_trimmed():
    assert goals.material_notice("x" * (goals.MAX_MATERIAL_CHARS + 1))
    assert goals.material_notice("x" * 10) == ""


def test_latest_fragment_source_reads_the_most_recent(tmp_path):
    store = records.RecordStore(tmp_path / "records.sqlite3")
    assert store.latest_fragment_source() == ""

    session = CognitiveSession()
    store.save_fragment(session, session.add_fragment("первый текст", ["a", "b", "c", "d"]))
    store.save_fragment(session, session.add_fragment("второй текст", ["e", "f", "g", "h"]))

    assert store.latest_fragment_source() == "второй текст"


def test_suggest_goal_uses_material_direction_and_note():
    client = _StubClient()
    service = CognitiveService(client)

    result = service.suggest_goal(
        "Однородная система: свободные члены равны нулю.",
        "Понять, почему работает",
        "почему заменяют нулями",
    )

    assert result.text.startswith("Понять")
    assert result.direction == "Понять, почему работает"
    body = client.messages[0][1]["content"]
    assert "<MATERIAL>" in body
    assert "<DIRECTION>" in body
    assert "<READER_NOTE>" in body


def test_suggest_goal_requires_material():
    service = CognitiveService(_StubClient())

    try:
        service.suggest_goal("   ")
    except ValueError as exc:
        assert "must not be empty" in str(exc)
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected ValueError")


def test_goal_prompt_omits_blocks_that_were_not_given():
    body = prompts.goal_prompt("Материал для чтения.")[1]["content"]

    assert "<MATERIAL>" in body
    assert "<DIRECTION>" not in body
    assert "<READER_NOTE>" not in body


def test_goal_prompt_is_configurable(tmp_path):
    settings = PromptSettings(tmp_path / "prompts.json")

    assert settings.get("goal") == prompts.GOAL_SYSTEM
    settings.set("goal", "Мой собственный промпт для цели.")

    assert settings.get("goal") == "Мой собственный промпт для цели."
    assert settings.is_custom("goal")


def _app(desktop):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app._busy = False
    app._goal_token = 0
    return app


def test_goal_material_uses_the_captured_passage_and_its_source(desktop, monkeypatch):
    app = _app(desktop)

    monkeypatch.setattr(desktop, "selected_text_source",
                        lambda primary_only=False: ("выделенное", goals.ORIGIN_SELECTION))
    assert app._goal_material() == ("выделенное", "выделение")

    monkeypatch.setattr(desktop, "selected_text_source",
                        lambda primary_only=False: ("скопированное", goals.ORIGIN_CLIPBOARD))
    assert app._goal_material() == ("скопированное", "буфер обмена")

    # Nothing was captured: no fragment from the ledger is substituted silently.
    monkeypatch.setattr(desktop, "selected_text_source", lambda primary_only=False: ("", ""))
    assert app._goal_material() == ("", "")


def test_last_fragment_is_offered_but_never_substituted(desktop, monkeypatch):
    app = _app(desktop)
    monkeypatch.setattr(desktop.RECORDS, "latest_fragment_source", lambda: "старый фрагмент")
    captured = []
    monkeypatch.setattr(desktop, "run_popup",
                        lambda payload, **kwargs: captured.append(payload) or "")

    app._goal_input("", "")

    assert captured[0]["material"] == ""
    assert captured[0]["last_fragment"] == "старый фрагмент"


def test_first_goal_opens_the_picker_not_an_empty_editor(desktop, monkeypatch):
    app = _app(desktop)
    monkeypatch.setattr(desktop.RECORDS, "current_intention", lambda: None)
    monkeypatch.setattr(desktop.RECORDS, "intention_history", lambda tail=None: [])
    monkeypatch.setattr(desktop, "selected_text_source",
                        lambda primary_only=False: ("абзац про матрицы", goals.ORIGIN_SELECTION))
    opened = []
    monkeypatch.setattr(desktop, "run_popup", lambda *args, **kwargs: opened.append(args) or "")
    assisted = []
    monkeypatch.setattr(app, "_goal_assist",
                        lambda material, origin: assisted.append((material, origin)))
    desktop.begin_interaction("hotkey", window="intent")

    app.show_intent()

    assert opened == []  # the bookmark editor never opened
    assert assisted == [("абзац про матрицы", "выделение")]


def test_existing_goal_opens_the_card_without_calling_the_model(desktop, monkeypatch):
    app = _app(desktop)
    app.flash = SimpleNamespace(show_text=lambda *args, **kwargs: None)
    current = SimpleNamespace(id="i1", text="понять, зачем нужна матрица", criterion="",
                              stopped_at="", material="абзац", material_origin="выделение")
    monkeypatch.setattr(desktop.RECORDS, "current_intention", lambda: current)
    monkeypatch.setattr(desktop.RECORDS, "intention_history", lambda tail=None: [])
    monkeypatch.setattr(app, "_goal_assist",
                        lambda *args: pytest.fail("a saved goal must not start a wording"))
    captured = []
    monkeypatch.setattr(desktop, "run_popup",
                        lambda payload, **kwargs: captured.append(payload)
                        or json.dumps({"action": "keep"}))
    desktop.begin_interaction("hotkey", window="intent")

    app.show_intent()

    assert captured[0]["mode"] == "intent"
    assert captured[0]["intent"]["text"] == "понять, зачем нужна матрица"
    assert captured[0]["intent"]["material"] == "абзац"
    assert captured[0]["intent"]["material_origin"] == "выделение"


def test_new_goal_from_the_card_keeps_the_accepted_one_until_replaced(desktop, monkeypatch):
    app = _app(desktop)
    app.flash = SimpleNamespace(show_text=lambda *args, **kwargs: None)
    saved = desktop.RECORDS.save_intention(
        "старая цель", material="первый абзац", material_origin="выделение")
    monkeypatch.setattr(desktop, "selected_text_source",
                        lambda primary_only=False: ("новый абзац", goals.ORIGIN_SELECTION))
    monkeypatch.setattr(desktop, "run_popup",
                        lambda payload, **kwargs: json.dumps({"action": "assist"}))
    assisted = []
    monkeypatch.setattr(app, "_goal_assist",
                        lambda material, origin: assisted.append((material, origin)))
    desktop.begin_interaction("hotkey", window="intent")

    app.show_intent()

    assert assisted == [("новый абзац", "выделение")]
    # Starting a new goal writes nothing: the accepted one still stands.
    current = desktop.RECORDS.current_intention()
    assert current.id == saved.id
    assert current.text == "старая цель"


def test_editing_the_card_keeps_the_stored_material(desktop, monkeypatch):
    app = _app(desktop)
    app.flash = SimpleNamespace(show_text=lambda *args, **kwargs: None)
    saved = desktop.RECORDS.save_intention(
        "первая цель", material="абзац", material_origin="выделение",
        direction="Понять общую идею")
    monkeypatch.setattr(desktop, "run_popup",
                        lambda payload, **kwargs: json.dumps(
                            {"action": "save", "text": "уточнённая цель"}))
    desktop.begin_interaction("hotkey", window="intent")

    app.show_intent()

    current = desktop.RECORDS.current_intention()
    assert current.id == saved.id
    assert current.text == "уточнённая цель"
    assert current.material == "абзац"
    assert current.material_origin == "выделение"
    assert current.direction == "Понять общую идею"


def test_goal_call_records_its_input_for_later_audit():
    from cognitive_popups import observation

    service = CognitiveService(_StubClient())
    service.suggest_goal("Материал для чтения.", "Понять, почему работает", "заметка")

    with observation.ObservationStore().connect() as conn:
        kinds = [row["kind"] for row in conn.execute("SELECT kind FROM obs_artifacts ORDER BY rowid")]

    assert "goal_input" in kinds
    assert "goal_generated" in kinds


def test_picker_payload_carries_direction_labels(desktop, monkeypatch):
    app = _app(desktop)
    captured = []
    monkeypatch.setattr(desktop, "run_popup",
                        lambda payload, **kwargs: captured.append(payload) or "")
    desktop.begin_interaction("hotkey", window="goal")

    app._goal_input("материал", "выделение")

    # The popup renders these as button labels: plain strings, not option dicts.
    assert captured[0]["directions"] == list(goals.DIRECTION_LABELS)
    assert all(isinstance(item, str) for item in captured[0]["directions"])


def test_late_goal_answer_is_dropped(desktop, monkeypatch):
    app = _app(desktop)
    app._goal_token = 3
    called = []
    monkeypatch.setattr(app, "_goal_result",
                        lambda *args, **kwargs: called.append((args, kwargs)))
    desktop.begin_interaction("hotkey", window="goal")

    app._goal_done(2, SimpleNamespace(text="старое", direction=""), None)

    assert called == []


def test_goal_answer_offers_the_wording_for_acceptance(desktop, monkeypatch):
    app = _app(desktop)
    app._busy = True
    app._goal_token = 4
    called = []
    monkeypatch.setattr(app, "_goal_result",
                        lambda *args, **kwargs: called.append((args, kwargs)))
    desktop.begin_interaction("hotkey", window="goal")

    app._goal_done(4, SimpleNamespace(text="Понять, почему убирают слагаемое",
                                      direction="Понять, почему работает"),
                   None, "абзац", "Понять, почему работает", "выделение", "")

    assert app._busy is False
    args, kwargs = called[0]
    assert args[0] == "Понять, почему убирают слагаемое"
    assert args[1] == "Понять, почему работает"
    assert kwargs["material"] == "абзац"
    assert kwargs["origin"] == "выделение"
    assert kwargs["notice"] == "Предложено по: Понять, почему работает"


def test_goal_error_offers_a_retry_without_losing_the_material(desktop, monkeypatch):
    app = _app(desktop)
    app._busy = True
    app._goal_token = 1
    called = []
    monkeypatch.setattr(app, "_goal_result",
                        lambda *args, **kwargs: called.append((args, kwargs)))
    desktop.begin_interaction("hotkey", window="goal")

    app._goal_done(1, None, "нет сети", "абзац", "Понять, почему работает", "выделение", "")

    assert app._busy is False
    args, kwargs = called[0]
    # The general wording for the direction is there to accept instead.
    assert args[0] == goals.direction_draft("Понять, почему работает")
    assert kwargs["retryable"] is True
    assert kwargs["material"] == "абзац"
    assert "нет сети" in kwargs["error"]


def test_accepting_stores_the_wording_with_its_material(desktop, monkeypatch):
    app = _app(desktop)
    app.flash = SimpleNamespace(show_text=lambda *args, **kwargs: None)
    monkeypatch.setattr(desktop, "run_popup",
                        lambda payload, **kwargs: json.dumps(
                            {"action": "accept", "text": "принятая цель"}))
    desktop.begin_interaction("hotkey", window="goal_result")

    app._goal_result("предложенная цель", "Понять, почему работает",
                     material="абзац про матрицы", origin="буфер обмена")

    current = desktop.RECORDS.current_intention()
    assert current.text == "принятая цель"
    assert current.direction == "Понять, почему работает"
    assert current.material == "абзац про матрицы"
    assert current.material_origin == "буфер обмена"


def test_cancelling_the_result_window_stores_nothing(desktop, monkeypatch):
    app = _app(desktop)
    monkeypatch.setattr(desktop, "run_popup", lambda payload, **kwargs: "")
    desktop.begin_interaction("hotkey", window="goal_result")

    app._goal_result("предложенная цель", "Понять общую идею",
                     material="абзац", origin="выделение")

    assert desktop.RECORDS.current_intention() is None


def test_retry_keeps_the_material_and_direction(desktop, monkeypatch):
    app = _app(desktop)
    monkeypatch.setattr(desktop, "run_popup",
                        lambda payload, **kwargs: json.dumps({"action": "retry"}))
    seen = []
    app._run_goal = lambda material, direction, note, origin="": seen.append(
        (material, direction, note, origin))
    desktop.begin_interaction("hotkey", window="goal_result")

    app._goal_result("", "Понять, почему работает", material="абзац", origin="выделение",
                     note="моя заметка", error="нет сети", retryable=True)

    assert seen == [("абзац", "Понять, почему работает", "моя заметка", "выделение")]


def test_another_direction_reopens_the_picker_with_the_same_material(desktop, monkeypatch):
    app = _app(desktop)
    monkeypatch.setattr(desktop, "run_popup",
                        lambda payload, **kwargs: json.dumps({"action": "another"}))
    seen = []
    app._goal_assist = lambda material, origin: seen.append((material, origin))
    desktop.begin_interaction("hotkey", window="goal_result")

    app._goal_result("черновик", "Понять общую идею", material="абзац", origin="выделение")

    assert seen == [("абзац", "выделение")]


def test_goal_without_material_stays_local(desktop, monkeypatch):
    app = _app(desktop)
    monkeypatch.setattr(
        app, "_goal_input",
        lambda material, origin: {"direction": "Понять общую идею", "material": "   "},
    )
    app._run_goal = lambda *args, **kwargs: pytest.fail("no material must not reach the model")
    called = []
    monkeypatch.setattr(app, "_goal_result",
                        lambda *args, **kwargs: called.append((args, kwargs)))
    desktop.begin_interaction("hotkey", window="goal")

    app._goal_assist("", "")

    assert called[0][0][0] == goals.direction_draft("Понять общую идею")
    assert "не уточнена" in called[0][1]["notice"]


def test_goal_with_material_asks_the_model(desktop, monkeypatch):
    app = _app(desktop)
    monkeypatch.setattr(
        app, "_goal_input",
        lambda material, origin: {"direction": "Понять, почему работает", "material": "материал"},
    )
    seen = []
    app._run_goal = lambda material, direction, note, origin="": seen.append(
        (material, direction, note, origin))
    monkeypatch.setattr(app, "_goal_result", lambda *a, **k: pytest.fail("no wording yet"))
    desktop.begin_interaction("hotkey", window="goal")

    app._goal_assist("материал", "выделение")

    assert seen == [("материал", "Понять, почему работает", "", "выделение")]


def test_opening_the_bookmark_cancels_a_pending_wording(desktop, monkeypatch):
    app = _app(desktop)
    monkeypatch.setattr(app, "_goal_material", lambda: ("", ""))
    monkeypatch.setattr(desktop.RECORDS, "current_intention", lambda: None)
    monkeypatch.setattr(desktop.RECORDS, "intention_history", lambda tail=None: [])
    monkeypatch.setattr(desktop, "run_popup", lambda payload, **kwargs: "")
    desktop.begin_interaction("hotkey", window="intent")

    app.show_intent()

    assert app._goal_token == 1
