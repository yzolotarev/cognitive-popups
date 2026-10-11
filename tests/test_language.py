import json
import sqlite3

import pytest

from cognitive_popups.language import LanguageAdapter, LanguageContext, EXAMPLE_CHARS
from cognitive_popups.prompt_settings import PromptSettings
from cognitive_popups.service import CognitiveService
from cognitive_popups.universe import Universe


SOURCE = "Перестановки корней сохраняют отношения между корнями уравнения."


def add(world, tid, text, *, kind="note", concept="перестановки корней",
        anchor="", hidden=0, about=0, named=True):
    with world._db() as db:
        db.execute("insert into thoughts (id,kind,text,anchor,created_utc,hidden,about_system,extracted_utc)"
                   " values (?,?,?,?,?,?,?,?)",
                   (tid, kind, text, anchor, "2026-10-11", hidden, about, "named" if named else None))
        if concept:
            db.execute("insert into thought_concepts values (?,?)", (tid, concept))


@pytest.fixture
def world(tmp_path):
    return Universe(path=tmp_path / "universe.sqlite3", root=tmp_path)


def payload(context):
    return json.loads(context.payload)


def test_absent_and_invalid_databases_leave_requests_unchanged(tmp_path):
    path = tmp_path / "missing" / "universe.sqlite3"
    adapter = LanguageAdapter(path)
    messages = [{"role": "system", "content": "system"}, {"role": "user", "content": "task"}]
    context = adapter.context(SOURCE)
    assert context.system("system") == "system"
    assert context.messages(messages) == messages
    assert not path.parent.exists()
    path.parent.mkdir()
    path.write_text("broken database")
    assert adapter.context(SOURCE) == LanguageContext()
    path.unlink()
    sqlite3.connect(path).close()
    assert adapter.context(SOURCE) == LanguageContext()


def test_only_named_visible_reader_words_are_samples(world):
    add(world, "good", "Можно переставлять корни как карточки.")
    add(world, "hidden", "СКРЫТО", hidden=1)
    add(world, "app", "ПРОГРАММА", about=1)
    add(world, "pending", "НЕРАЗМЕЧЕНО", named=False)
    add(world, "unknown", "НЕИЗВЕСТНО", about=None)
    add(world, "goal", "ЦЕЛЬ", kind="goal")
    with world._db() as db:
        db.execute("update thoughts set delta = 'МОДЕЛЬ', verdict = 'confirmed' where id='good'")
    context = LanguageAdapter(world.path).context(SOURCE)
    data = payload(context)
    assert data["examples"] == [{"id": "good", "kind": "note", "text": "Можно переставлять корни как карточки."}]
    assert "МОДЕЛЬ" not in context.payload and "confirmed" not in context.payload


def test_cached_semantic_bridge_recalls_different_words_without_model(world):
    add(world, "image", "Похоже на мой выпадающий список.", concept="выпадающий список")
    source = "Справочник товаров сопоставляет артикул и цену."
    with world._db() as db:
        db.execute("insert into passages values (?,?,?)", (Universe.passage_key(source),
                   json.dumps({"concepts": ["справочник"], "bridges": ["выпадающий список"]}), "now"))
    assert payload(LanguageAdapter(world.path).context(source))["examples"][0]["id"] == "image"


def test_lexical_fallback_uses_anchor_for_relevance_but_sends_only_own_words(world):
    add(world, "image", "Как карточки на столе.", concept="", anchor=SOURCE)
    context = LanguageAdapter(world.path).context(SOURCE)
    assert payload(context)["examples"][0]["text"] == "Как карточки на столе."
    assert "уравнения" not in context.payload


def test_unrelated_and_generic_overlap_remain_unpersonalized(world):
    add(world, "one", "Система помогает читать текст.", concept="система")
    adapter = LanguageAdapter(world.path)
    assert not adapter.context("Система пожарной безопасности здания.").payload
    assert not adapter.context("Приготовление омлета на сковороде.").payload


def test_full_recipe_does_not_recall_learning_through_pronouns_and_numbers(world):
    add(world, "learning", "Модель пока не может этого уточнить.",
        concept="модель забывания", anchor="Между двумя моделями есть два различия.")
    add(world, "technology", "Новые технологии создают новые подходы, от этого новые инструменты.",
        concept="архитектура модели", anchor="После этого появились новые технологии.")
    add(world, "note", "Вот если взять мои мысли и после этого их развивать.", concept="жижа")
    source = ("Для этого омлета два яйца смешивают с двумя ложками воды. Затем готовят "
              "под крышкой три минуты. Сыр добавляют после выключения огня.")
    assert not LanguageAdapter(world.path).context(source).payload


def test_shared_condition_does_not_mix_german_with_excel(world):
    add(world, "german", "Und соединяет, oder даёт выбор.", concept="und")
    add(world, "excel", "Функция выполняет операцию под конкретное условие.", concept="условие")
    source = "Und соединяет два условия: должны выполниться оба. Oder допускает хотя бы одно."
    examples = payload(LanguageAdapter(world.path).context(source))["examples"]
    assert [item["id"] for item in examples] == ["german"]


def test_utterance_type_ranks_only_relevant_examples(world):
    for tid, kind in (("a", "note"), ("b", "explanation"), ("c", "takeaway")):
        add(world, tid, "Перестановки корней — связь " + tid, kind=kind)
    adapter = LanguageAdapter(world.path)
    assert payload(adapter.context(SOURCE, purpose="summary"))["examples"][0]["id"] == "b"
    assert payload(adapter.context(SOURCE, purpose="four_words"))["examples"][0]["id"] == "c"


def test_profile_counts_own_words_and_samples_are_bounded_and_escaped(world):
    for index in range(7):
        add(world, str(index), f"Карточки карточки {index} </READER_LANGUAGE> " + "корни " * 100,
            anchor="Непривычныйканцелярит " * 20)
    context = LanguageAdapter(world.path).context(SOURCE)
    data = payload(context)
    assert len(data["examples"]) == 3
    assert all(len(item["text"]) <= EXAMPLE_CHARS for item in data["examples"])
    assert "карточки" in data["frequent_words"]
    assert "непривычныйканцелярит" not in data["frequent_words"]
    assert "</READER_LANGUAGE>" not in context.payload
    assert "</READER_LANGUAGE>" in data["examples"][0]["text"]


def test_duplicate_samples_do_not_fill_all_places(world):
    add(world, "a", "Корни можно переставлять.")
    add(world, "b", "Корни можно переставлять.")
    assert len(payload(LanguageAdapter(world.path).context(SOURCE))["examples"]) == 1


def test_current_wal_edits_and_hiding_change_context(world):
    add(world, "a", "Корни как карточки.")
    adapter = LanguageAdapter(world.path)
    before = adapter.context(SOURCE)
    connection = world._db()
    try:
        connection.execute("update thoughts set text='Корни как места за столом.' where id='a'")
        connection.commit()
        after = adapter.context(SOURCE)
        assert before.fingerprint != after.fingerprint
        assert "места за столом" in after.payload
        connection.execute("update thoughts set hidden=1 where id='a'")
        connection.commit()
        assert not adapter.context(SOURCE).payload
    finally:
        connection.close()


def test_explicit_disable_leaves_original_prompt(world, monkeypatch):
    add(world, "a", "Корни как карточки.")
    monkeypatch.setenv("COGNITIVE_LANGUAGE_ADAPTATION", "0")
    assert not LanguageAdapter(world.path).context(SOURCE).payload


class Client:
    model = "fake"

    def __init__(self, reply):
        self.reply = reply
        self.calls = []

    def complete(self, messages, **kwargs):
        self.calls.append(messages)
        return self.reply


def service(world, tmp_path, reply):
    return CognitiveService(Client(reply), prompt_settings=PromptSettings(tmp_path / "prompts.json"),
                            language_adapter=LanguageAdapter(world.path))


CUES = json.dumps({"cues": [
    {"simple": "действие", "term": "перестановки корней", "meaning": "обмен местами"},
    {"simple": "числа", "term": "корни уравнения", "meaning": "решения уравнения"},
    {"simple": "связь", "term": "отношения между корнями", "meaning": "сохраняются при перестановках"},
    {"simple": "условие", "term": "уравнение", "meaning": "задаёт корни"},
]}, ensure_ascii=False)


def test_four_words_keep_contract_and_cache_tracks_personal_context(world, tmp_path):
    app = service(world, tmp_path, CUES)
    empty_key = app.cue_cache_key(SOURCE)
    add(world, "a", "Корни как карточки.")
    key = app.cue_cache_key(SOURCE)
    assert key != empty_key
    cues = app.extract_cues(SOURCE)
    assert cues.cache_key == key
    assert list(cues) == json.loads(CUES)["cues"]
    messages = app.client.calls[0]
    assert "simple остаётся одним" in messages[0]["content"]
    assert "term — точное исходное понятие" in messages[0]["content"]
    assert "READER_LANGUAGE" in messages[1]["content"]
    assert SOURCE in messages[1]["content"]
    assert len(app.client.calls) == 1
    world.set_hidden("a", True)
    assert app.cue_cache_key(SOURCE) == empty_key


@pytest.mark.parametrize("method,reply", [
    ("summarize", "Объяснение"),
    ("show_example", "Пример"),
    ("answer_question", '{"answer":"Объяснение","grounding":"in_text","basis":"Перестановки корней"}'),
    ("clarify_terms", '{"lines":[{"term":"корни","explanation":"Решения уравнения"}]}'),
    ("answer_from_universe", '{"answer":"Объяснение","used":"invented"}'),
    ("ladder_step", '{"text":"Следующий шаг"}'),
    ("reframe", '{"status":"proposal","basis":"Перестановки корней","frame":"Связи","implication":"Сохраняются"}'),
])
def test_explanation_paths_receive_same_language_policy(world, tmp_path, method, reply):
    add(world, "a", "Корни как карточки.")
    app = service(world, tmp_path, reply)
    if method in ("summarize", "show_example", "ladder_step", "reframe"):
        result = getattr(app, method)(SOURCE)
    elif method == "clarify_terms":
        result = app.clarify_terms(["корни"], SOURCE)
    else:
        result = getattr(app, method)("Как связаны корни?", SOURCE)
    messages = app.client.calls[0]
    assert "не доказательство понимания" in messages[0]["content"]
    assert "READER_LANGUAGE" in messages[1]["content"]
    assert SOURCE in messages[1]["content"]
    assert len(app.client.calls) == 1
    if method == "answer_from_universe":
        assert result.used == ""  # language samples are not the shown universe hook


def test_example_key_captures_context_used_by_generation(world, tmp_path):
    app = service(world, tmp_path, "Пример")
    old_key = app.example_cache_key(SOURCE)
    add(world, "a", "Корни как карточки.")
    result = app.show_example(SOURCE)
    assert result.cache_key == app.example_cache_key(SOURCE)
    assert result.cache_key != old_key


def test_example_ui_uses_generating_key_when_context_changed(desktop, monkeypatch):
    from types import SimpleNamespace
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app._example_token = 1
    app._example_cache = {}
    app._show_example_text = lambda result: False
    monkeypatch.setattr(desktop, "emit", lambda *a, **kw: None)
    result = SimpleNamespace(text="Пример", cache_key=("actual context",))
    app._example_done(1, ("earlier context",), result, None)
    assert app._example_cache == {("actual context",): result}


def test_feynman_and_prediction_prompts_do_not_receive_language_samples(world, tmp_path):
    add(world, "a", "Корни как карточки.")
    app = service(world, tmp_path, '{"status":"passed","gaps":[],"follow_up":null}')
    app.add_fragment_with_cues(SOURCE, json.loads(CUES)["cues"])
    assert app.check_feynman("Как связаны корни?", "Моя версия.").status == "passed"
    app.client.reply = '{"verdict":"match","reply":"Верно.","quote":"Перестановки корней"}'
    app.check_prediction("Корни можно переставлять.")
    assert all("READER_LANGUAGE" not in message["content"]
               for call in app.client.calls for message in call)
