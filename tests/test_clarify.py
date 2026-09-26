from types import SimpleNamespace

from cognitive_popups.client import Web2APIError
from cognitive_popups.service import (
    CLARIFY_WORDS,
    MAX_CLARIFY_TERMS,
    CognitiveService,
    looks_like_question,
    parse_terms,
)


class CaptureClient:
    """Returns one fixed reply and keeps what the model was asked."""

    def __init__(self, reply: str):
        self.reply = reply
        self.calls = []

    def complete(self, messages, **kwargs):
        self.calls.append(messages)
        return self.reply


def buffered(reply: str):
    client = CaptureClient(reply)
    service = CognitiveService(client)
    service.add_fragment_with_cues(
        "Working memory holds about four chunks; long-term memory does not.",
        [
            {"simple": "память", "term": "memory", "meaning": "удержание"},
            {"simple": "группы", "term": "chunking", "meaning": "объединение"},
            {"simple": "предел", "term": "limit", "meaning": "ограничение"},
            {"simple": "заметка", "term": "note", "meaning": "внешняя опора"},
        ],
    )
    return service, client


def test_clarify_truncates_long_explanations():
    # The explanation cap keeps a runaway answer usable while allowing a short
    # example and relation, which the old eleven-word cap could not express.
    long_line = " ".join(f"слово{index}" for index in range(20))
    reply = '{"lines":[{"term":"чанкинг","explanation":"' + long_line + '"}]}'
    service, _ = buffered(reply)

    result = service.clarify_terms(["чанкинг"])

    assert len(result.lines[0]["explanation"].split()) == 20
    assert result.omitted == []
    assert result.truncated == []


def test_clarify_reports_when_an_explanation_was_truncated():
    long_line = " ".join(f"слово{index}" for index in range(CLARIFY_WORDS + 5))
    reply = '{"lines":[{"term":"чанкинг","explanation":"' + long_line + '"}]}'
    service, _ = buffered(reply)

    result = service.clarify_terms(["чанкинг"])

    assert len(result.lines[0]["explanation"].split()) == CLARIFY_WORDS
    assert result.truncated == ["чанкинг"]


def test_clarify_sends_the_buffer_and_the_reader_terms():
    reply = '{"lines":[{"term":"чанкинг","explanation":"склейка шагов в один кусок"}]}'
    service, client = buffered(reply)

    service.clarify_terms(["чанкинг"])

    content = client.calls[0][1]["content"]
    assert "SOURCE_BUFFER" in content
    assert "Working memory holds" in content
    assert "чанкинг" in content


def test_clarify_drops_lines_about_terms_nobody_named():
    # An explanation of a word the reader never asked about is not an answer to
    # anything; the term it pretended to answer stays unanswered.
    reply = '{"lines":[{"term":"энтропия","explanation":"мера беспорядка"}]}'
    service, _ = buffered(reply)

    result = service.clarify_terms(["чанкинг"])

    assert result.lines == []
    assert result.omitted == ["чанкинг"]


def test_clarify_caps_the_window_and_names_what_it_left_out():
    terms = ["чанкинг", "память", "предел", "заметка", "энтропия"]
    lines = ",".join(
        '{"term":"%s","explanation":"роль в тексте"}' % term
        for term in terms[:MAX_CLARIFY_TERMS]
    )
    service, client = buffered('{"lines":[' + lines + "]}")

    result = service.clarify_terms(terms)

    assert len(result.lines) == MAX_CLARIFY_TERMS
    assert result.omitted == ["энтропия"]
    # The terms beyond the window are not even offered to the model.
    assert "энтропия" not in client.calls[0][1]["content"]


def test_clarify_keeps_the_readers_spelling_and_folds_duplicates():
    reply = '{"lines":[{"term":"Чанкинг","explanation":"роль в тексте"}]}'
    service, _ = buffered(reply)

    result = service.clarify_terms(["чанкинг", "Чанкинг", " чанкинг "])

    assert len(result.lines) == 1
    # Asking twice about one word is not two questions, and the window shows the
    # word back the way the reader wrote it.
    assert result.lines[0]["term"] == "чанкинг"
    assert result.omitted == []


def test_parse_terms_splits_folds_and_keeps_the_readers_order():
    # The reader's order is the only relevance signal there is, so it survives.
    assert parse_terms("чанкинг, предел\nпамять") == ["чанкинг", "предел", "память"]
    assert parse_terms("чанкинг, Чанкинг,  чанкинг ") == ["чанкинг"]
    assert parse_terms(" , \n ") == []


def test_clarify_refuses_an_empty_buffer():
    service = CognitiveService(CaptureClient('{"lines":[]}'))

    try:
        service.clarify_terms(["чанкинг"])
    except ValueError as exc:
        assert "buffer is empty" in str(exc)
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected ValueError")


def test_clarify_asks_from_the_selection_when_the_buffer_is_empty():
    # A reader who has not seeded four cues yet must still be able to ask about the
    # page in front of them: seeding is a model call, and asking a question should
    # not require one first.
    reply = '{"lines":[{"term":"энтропия","explanation":"мера беспорядка"}]}'
    client = CaptureClient(reply)
    service = CognitiveService(client)

    result = service.clarify_terms(["энтропия"], "Entropy measures disorder in a closed system.")

    assert result.lines[0]["term"] == "энтропия"
    content = client.calls[0][1]["content"]
    assert "SOURCE_BUFFER" in content
    assert "Entropy measures disorder" in content


def test_clarify_adds_the_selection_to_the_buffer():
    # The buffer holds what the reader chose to work with, so it comes first; the
    # passage on screen is added too, because the sentence that defines the term
    # may be highlighted and not yet committed. Without it the model falls back to
    # a dictionary sense of the word.
    reply = '{"lines":[{"term":"чанкинг","explanation":"роль в тексте"}]}'
    service, client = buffered(reply)

    service.clarify_terms(["чанкинг"], "Chunking is the grouping of items.")

    content = client.calls[0][1]["content"]
    assert "Working memory holds" in content
    assert "Chunking is the grouping of items." in content
    assert content.index("Working memory holds") < content.index("Chunking is the grouping")


def test_clarify_skips_a_selection_already_in_the_buffer():
    reply = '{"lines":[{"term":"чанкинг","explanation":"роль в тексте"}]}'
    service, client = buffered(reply)

    service.clarify_terms(
        ["чанкинг"], "Working memory holds about four chunks; long-term memory does not."
    )

    content = client.calls[0][1]["content"]
    assert content.count("Working memory holds") == 1


def test_clarify_refuses_an_empty_term_list():
    service, _ = buffered('{"lines":[]}')

    try:
        service.clarify_terms([" , ", ""])
    except ValueError as exc:
        assert "no terms" in str(exc)
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected ValueError")


# --- Alt+C question mode -------------------------------------------------------


def test_looks_like_question_tells_a_question_from_a_term_list():
    # An independently authored synthesis question is not a list of terms,
    # even without a question mark or a verbatim phrase from the source.
    assert looks_like_question(
        "если бы из устройства игрушечного моста взять одну идею для бумажной башни, "
        "что бы это было"
    )
    assert looks_like_question("почему критерий не работает")
    assert looks_like_question("что такое группа Галуа")
    assert looks_like_question("как связаны память и предел?")

    # A glossary request stays a glossary request.
    assert not looks_like_question("чанкинг")
    assert not looks_like_question("группа Галуа, автоморфизм, разрешимость в радикалах")
    assert not looks_like_question("метод неопределённых коэффициентов")
    assert not looks_like_question("")


def test_a_question_phrase_is_matched_by_word_not_by_substring():
    # "если быстрее" must not be read as the question opening "если бы".
    assert not looks_like_question("если быстрее предел")


def test_answer_question_asks_the_question_mode_prompt_about_the_buffer():
    reply = (
        '{"grounding":"partly_in_text","answer":"Чанкинг расширяет полезный объём памяти.",'
        '"basis":"Working memory holds about four chunks"}'
    )
    service, client = buffered(reply)

    result = service.answer_question("Что важнее всего для нейробиологии?")

    assert result.text == "Чанкинг расширяет полезный объём памяти."
    assert result.grounding == "partly_in_text"
    assert result.basis == "Working memory holds about four chunks"
    assert result.question == "Что важнее всего для нейробиологии?"
    content = client.calls[0][1]["content"]
    assert "SOURCE_BUFFER" in content
    assert "READER_QUESTION" in content
    assert "Что важнее всего для нейробиологии?" in content


def test_answer_drops_a_quotation_that_is_not_in_the_text():
    # An invented quotation is never shown as if it stood in the text.
    reply = (
        '{"grounding":"in_text","answer":"Память ограничена.",'
        '"basis":"рабочая память хранит бесконечно много"}'
    )
    service, _ = buffered(reply)

    result = service.answer_question("Ограничена ли память?")

    assert result.basis == ""


def test_answer_ignores_an_unknown_grounding_value():
    service, _ = buffered('{"grounding":"maybe","answer":"Ответ.","basis":""}')

    result = service.answer_question("Вопрос?")

    assert result.grounding == ""
    assert result.text == "Ответ."


def test_answer_refuses_an_empty_question():
    service, _ = buffered('{"answer":"x"}')

    try:
        service.answer_question("   ")
    except ValueError as exc:
        assert "question must not be empty" in str(exc)
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected ValueError")


def test_answer_refuses_an_empty_response():
    service, _ = buffered('{"grounding":"in_text","answer":"  "}')

    try:
        service.answer_question("Вопрос?")
    except Web2APIError:
        pass
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected Web2APIError")


def test_answer_asks_from_the_selection_when_the_buffer_is_empty():
    # A reader who has not seeded four cues yet can still ask about the page in
    # front of them, exactly as a glossary request can.
    reply = '{"grounding":"in_text","answer":"Мера беспорядка.","basis":""}'
    client = CaptureClient(reply)
    service = CognitiveService(client)

    service.answer_question("Что такое энтропия?", "Entropy measures disorder.")

    content = client.calls[0][1]["content"]
    assert "Entropy measures disorder." in content


class _SyncThread:
    """Run the worker inline, so routing can be checked without a race."""

    def __init__(self, target=None, daemon=None, **kwargs):
        self._target = target

    def start(self):
        self._target()


def _clarify_app(desktop, monkeypatch, raw_input, service, shown):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.service = service
    app._busy = False
    app.flash = SimpleNamespace(
        show_text=lambda text, title="Result", **kwargs: shown.append((text, kwargs))
    )
    monkeypatch.setattr(desktop, "selected_text", lambda: "Working memory holds about four chunks.")
    monkeypatch.setattr(desktop, "popup_input", lambda *args, **kwargs: raw_input)
    monkeypatch.setattr(desktop, "threading", SimpleNamespace(Thread=_SyncThread))
    return app


def test_explain_terms_sends_a_question_to_the_question_mode(desktop, monkeypatch):
    service = CognitiveService(CaptureClient('{"grounding":"not_in_text","answer":"X","basis":""}'))
    shown = []
    app = _clarify_app(desktop, monkeypatch, "почему критерий не работает", service, shown)
    desktop.begin_interaction("hotkey", window="clarify")
    asked = []
    app._ask_question = lambda question, source: asked.append((question, source))

    app.explain_terms()

    assert asked == [("почему критерий не работает", "Working memory holds about four chunks.")]


def test_explain_terms_keeps_a_term_list_a_glossary_request(desktop, monkeypatch):
    service = CognitiveService(CaptureClient('{"lines":[]}'))
    shown = []
    app = _clarify_app(desktop, monkeypatch, "чанкинг, предел", service, shown)
    desktop.begin_interaction("hotkey", window="clarify")
    asked = []
    app._ask_question = lambda question, source: asked.append(question)

    app.explain_terms()

    # Not a question: the glossary path answered and named what it could not cover.
    assert asked == []
    assert app._busy is False
    assert shown and "Опора для объяснения не найдена" in shown[0][0]
    assert "Упущено" in shown[0][0]


def test_explain_terms_reports_the_grounding_of_an_answer(desktop, monkeypatch):
    reply = '{"grounding":"not_in_text","answer":"Общий ответ.","basis":""}'
    service = CognitiveService(CaptureClient(reply))
    shown = []
    app = _clarify_app(desktop, monkeypatch, "что важнее всего?", service, shown)
    desktop.begin_interaction("hotkey", window="clarify")

    app.explain_terms()

    assert shown[0][0] == "Общий ответ."
    assert shown[0][1]["note"] == "Этого в тексте нет: общий ответ."
