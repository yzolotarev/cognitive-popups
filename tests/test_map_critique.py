"""The pause for the reader's own map: the map is captured and read back.

The map is the reader's work, so it is kept word for word and only critiqued on
grouping, hierarchy and relationships; skipping the window costs nothing and the
series still ends with its thesis.
"""
import json
from types import SimpleNamespace

import pytest

from cognitive_popups.client import Web2APIError
from cognitive_popups.prompt_settings import PROMPT_KEYS, PromptSettings
from cognitive_popups.service import CognitiveService

SOURCE = "Память ограничена; группировка сокращает число единиц, а заметки выносят их наружу."
OUTLINE = "Память\n  предел\n  группировка\nЗаметки\nпредел -> группировка: поэтому группируем"


class ScriptedClient:
    def __init__(self, reply):
        self.reply = reply
        self.messages = []

    def complete(self, messages, **kwargs):
        self.messages.append(messages)
        return self.reply


def _reply(**overrides):
    data = {
        "grouping": {"anchor": "Заметки", "note": "Заметки стоят отдельно, хотя решают ту же проблему предела."},
        "hierarchy": {"anchor": "Память", "note": "Наверху тема, а организует всё предел."},
        "relationships": {"anchor": "", "note": "Связь есть, и сказано почему: это держится."},
        "question": "Что общего у группировки и заметок?",
    }
    data.update(overrides)
    return json.dumps(data, ensure_ascii=False)


def test_map_critique_is_an_editable_prompt():
    assert "map_critique" in PROMPT_KEYS


def test_critique_reads_the_map_against_its_passage(tmp_path):
    client = ScriptedClient(_reply())
    settings = PromptSettings(tmp_path / "prompts.json")
    service = CognitiveService(client, prompt_settings=settings)
    cues = [{"simple": "предел", "term": "ёмкость памяти", "meaning": "сколько удерживается"}]

    result = service.critique_map(SOURCE, OUTLINE, cues)

    sent = json.loads(client.messages[0][1]["content"])
    # Only the labels: the meanings would turn the cues into an answer key.
    assert sent == {"material": SOURCE, "cues": ["ёмкость памяти"], "map": OUTLINE}
    assert client.messages[0][0]["content"] == settings.get("map_critique")
    assert result.outline == OUTLINE
    assert list(result.findings) == ["grouping", "hierarchy", "relationships"]
    assert "Группировка: «Заметки» —" in result.text
    assert "Иерархия: «Память» —" in result.text
    assert result.text.endswith("Вопрос к карте: Что общего у группировки и заметок?")
    assert result.prompt_hash == service.prompt_id("map_critique")["prompt_hash"]


def test_an_anchor_that_is_not_on_the_map_is_dropped():
    reply = _reply(hierarchy={"anchor": "Рабочая память Баддели", "note": "Главное внизу."})
    result = CognitiveService(ScriptedClient(reply)).critique_map(SOURCE, OUTLINE)

    assert result.findings["hierarchy"] == {"anchor": "", "note": "Главное внизу."}
    assert "Баддели" not in result.text


def test_a_critique_with_no_finding_is_an_error():
    reply = json.dumps({"grouping": {"note": ""}, "question": "?"})
    with pytest.raises(Web2APIError):
        CognitiveService(ScriptedClient(reply)).critique_map(SOURCE, OUTLINE)


@pytest.mark.parametrize("material, outline", [("", OUTLINE), (SOURCE, "  \n ")])
def test_critique_needs_a_passage_and_a_map(material, outline):
    client = ScriptedClient(_reply())
    with pytest.raises(ValueError):
        CognitiveService(client).critique_map(material, outline)
    assert client.messages == []


def _app(desktop, monkeypatch, outline, critique=None):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    windows = []

    def run_popup(payload, *, window, detail=""):
        windows.append((window, dict(payload)))
        return outline if window == "map" else ""

    monkeypatch.setattr(desktop, "run_popup", run_popup)
    calls = []

    def critique_map(*args):
        calls.append(args)
        if isinstance(critique, Exception):
            raise critique
        return SimpleNamespace(text=critique)

    app.service = SimpleNamespace(critique_map=critique_map)
    return app, windows, calls


def test_the_map_window_opens_and_its_critique_follows(desktop, monkeypatch):
    app, windows, calls = _app(desktop, monkeypatch, OUTLINE, "Группировка: разбор")
    cues = [{"term": "предел"}]

    app._critique_batch_map(SOURCE, cues)

    assert calls == [(SOURCE, OUTLINE, cues)]
    assert [w for w, _ in windows] == ["map", "text"]
    assert windows[0][1]["prompt"] == desktop.MAP_PROMPT
    assert windows[1][1]["text"] == "Группировка: разбор"


def test_skipping_the_map_asks_the_model_nothing(desktop, monkeypatch):
    app, windows, calls = _app(desktop, monkeypatch, "")

    app._critique_batch_map(SOURCE, [])

    assert calls == []
    assert [w for w, _ in windows] == ["map"]


def test_a_failed_critique_is_shown_and_does_not_raise(desktop, monkeypatch):
    app, windows, _calls = _app(desktop, monkeypatch, OUTLINE, Web2APIError("timeout"))

    app._critique_batch_map(SOURCE, [])

    assert "Карта записана" in windows[-1][1]["text"]
    assert "timeout" in windows[-1][1]["text"]
