
from cognitive_popups.client import Web2APIError
from cognitive_popups.models import CognitiveSession
from cognitive_popups.service import CognitiveService


class FakeClient:
    def __init__(self):
        self.calls = []
        self.responses = iter([
            '{"cues":[{"simple":"память","term":"memory","meaning":"удержание информации"},{"simple":"предел","term":"limit","meaning":"ограничение количества"},{"simple":"группы","term":"chunking","meaning":"объединение элементов"},{"simple":"заметка","term":"note","meaning":"внешняя опора"}]}',
            '{"question":"Why does the limit make chunking useful?"}',
            '{"status":"needs_retry","gaps":[{"location":"limit -> chunking","type":"missing_causal_link","description":"The explanation names both ideas but does not connect them."},{"location":"note","type":"term_without_mechanism","description":"A term is used without describing its role."}],"follow_up":"How does chunking change the effective number of units?"}',
        ])

    def complete(self, messages, **kwargs):
        self.calls.append(messages)
        return next(self.responses)


def test_full_vertical_flow_and_context():
    client = FakeClient()
    service = CognitiveService(client)
    fragment = service.add_fragment("Working memory has a limited capacity.")
    question = service.create_feynman_question()
    check = service.check_feynman(question, "It is about memory and chunking.")

    assert fragment.cues[0] == "memory"
    assert question.startswith("Why")
    assert check.status == "needs_retry"
    assert len(check.gaps) == 2
    assert len(service.session.fragments) == 1
    assert "Working memory" in client.calls[1][1]["content"]
    assert "It is about memory" in client.calls[2][1]["content"]


def test_prediction_check_uses_buffer_and_persists():
    class PredictionClient:
        def __init__(self):
            self.calls = []

        def complete(self, messages, **kwargs):
            self.calls.append(messages)
            return '{"status":"partially_confirmed","mismatch":"Связь не доказана напрямую.","evidence":"Текст связывает оба понятия."}'

    client = PredictionClient()
    service = CognitiveService(client)
    service.add_fragment_with_cues(
        "The source connects memory and chunking.",
        [
            {"simple": "память", "term": "memory", "meaning": "удержание"},
            {"simple": "группы", "term": "chunking", "meaning": "объединение"},
            {"simple": "предел", "term": "limit", "meaning": "ограничение"},
            {"simple": "заметка", "term": "note", "meaning": "внешняя опора"},
        ],
    )

    check = service.check_prediction("Chunking reduces the effective memory limit.")

    assert check.status == "partially_confirmed"
    assert check.hypothesis.startswith("Chunking")
    assert "SOURCE_BUFFER" in client.calls[0][1]["content"]
    assert "Chunking reduces" in client.calls[0][1]["content"]
    saved = service.session.to_dict()
    restored = CognitiveSession.from_dict(saved)
    assert restored.prediction_checks[0].mismatch == "Связь не доказана напрямую."


class ScriptedClient:
    """Returns a fixed sequence of raw model replies, one per call."""

    def __init__(self, *replies):
        self.replies = list(replies)
        self.calls = 0

    def complete(self, messages, **kwargs):
        reply = self.replies[min(self.calls, len(self.replies) - 1)]
        self.calls += 1
        return reply


CUE = '{{"simple":"{s}","term":"{t}","meaning":"{m}"}}'


def four_cues() -> str:
    return (
        "{\"cues\":["
        + CUE.format(s="память", t="memory", m="удержание информации") + ","
        + CUE.format(s="предел", t="limit", m="ограничение количества") + ","
        + CUE.format(s="группы", t="chunking", m="объединение элементов") + ","
        + CUE.format(s="заметка", t="note", m="внешняя опора")
        + "]}"
    )


def test_extract_cues_retries_malformed_json():
    # A malformed reply is a format violation, so the retry prompt applies to it too.
    client = ScriptedClient("not json at all", four_cues())
    service = CognitiveService(client)

    cues = service.extract_cues("Working memory has a limited capacity.")

    assert [cue["term"] for cue in cues] == ["memory", "limit", "chunking", "note"]
    assert client.calls == 2


def test_extract_cues_reports_which_rule_was_broken():
    # Two words in `simple` is rejected; the error must name the offending rule
    # instead of a bare "error" the popup cannot explain.
    bad = (
        "{\"cues\":["
        + CUE.format(s="две слова", t="memory", m="удержание") + ","
        + CUE.format(s="предел", t="limit", m="ограничение") + ","
        + CUE.format(s="группы", t="chunking", m="объединение") + ","
        + CUE.format(s="заметка", t="note", m="внешняя опора")
        + "]}"
    )
    client = ScriptedClient(bad)
    service = CognitiveService(client)

    try:
        service.extract_cues("Working memory has a limited capacity.")
    except Web2APIError as exc:
        message = str(exc)
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected Web2APIError")

    assert "simple не одно слово" in message
    assert "попытка 1" in message and "попытка 2" in message
    assert client.calls == 2


def test_extract_cues_reports_wrong_cue_count():
    one_cue = '{"cues":[' + CUE.format(s="процесс", t="Инфляция", m="рост уровня цен") + "]}"
    client = ScriptedClient(one_cue)
    service = CognitiveService(client)

    try:
        service.extract_cues("Инфляция")
    except Web2APIError as exc:
        message = str(exc)
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected Web2APIError")

    assert "ожидалось 4 объекта, получено 1" in message
