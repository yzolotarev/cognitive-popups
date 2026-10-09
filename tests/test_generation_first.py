import json

import pytest

from cognitive_popups import prompts
from cognitive_popups.client import Web2APIError
from cognitive_popups.models import CognitiveSession, PredictionCheck
from cognitive_popups.service import CognitiveService


class Client:
    def __init__(self, *replies):
        self.replies = iter(replies)
        self.messages = []

    def complete(self, messages, **kwargs):
        self.messages.append(messages)
        return next(self.replies)


def buffered(client):
    service = CognitiveService(client)
    service.session.add_fragment("Memory is limited.", ["a", "b", "c", "d"])
    return service


def test_delta_persists_and_is_primary_without_losing_metadata():
    client = Client(json.dumps({"status": "partially_confirmed", "one_delta": "Only within the limit.",
                               "mismatch": "Too broad.", "evidence": "Memory is limited.",
                               "subject_note": "не из текста: external detail"}))
    service = buffered(client)
    check = service.check_prediction("Memory always holds everything.")
    restored = CognitiveSession.from_dict(service.session.to_dict()).prediction_checks[0]
    assert restored.one_delta == restored.text == "Only within the limit."
    assert restored.secondary_metadata["evidence"] == "Memory is limited."
    assert restored.mismatch == "Too broad."


def test_legacy_prediction_loads_without_delta():
    check = PredictionCheck([], "claim", "contradicted", mismatch="Old correction.")
    saved = check.to_dict()
    saved.pop("one_delta")
    restored = PredictionCheck(**saved)
    assert restored.one_delta == ""
    assert restored.text == "Old correction."


def test_confirmed_prediction_has_no_correction():
    service = buffered(Client('{"status":"confirmed","one_delta":"Unnecessary correction","evidence":"Memory is limited."}'))
    check = service.check_prediction("Memory is limited.")
    assert check.one_delta == ""
    assert check.text == "По тексту поправка не нужна."


def test_delta_does_not_bypass_evidence_verification():
    bad = '{"status":"contradicted","one_delta":"Small correction","evidence":"invented"}'
    service = buffered(Client(bad, bad))
    with pytest.raises(Web2APIError, match="дословной цитатой"):
        service.check_prediction("claim")
    assert service.session.prediction_checks == []


def test_evidence_cannot_bridge_fragments():
    bad = '{"status":"confirmed","evidence":"limited. Another"}'
    service = buffered(Client(bad, bad))
    service.session.add_fragment("Another passage.", ["a", "b", "c", "d"])
    with pytest.raises(Web2APIError):
        service.check_prediction("claim")


def test_hidden_rubric_keeps_string_question_contract():
    client = Client('{"question":"Why does grouping help?","rubric":"Explain fewer effective units."}',
                    '{"status":"passed","gaps":[],"follow_up":null}')
    service = buffered(client)
    question = service.create_feynman_question()
    assert isinstance(question, str)
    assert str(question) == "Why does grouping help?"
    assert "effective units" not in str(question)
    service.check_feynman(question, "It reduces the number of units.")
    assert "<HIDDEN_RUBRIC>" in client.messages[1][1]["content"]
    assert "Explain fewer effective units." in client.messages[1][1]["content"]


def test_four_handles_allow_repeated_simple_labels():
    cues = [{"simple": "формула", "term": f"formula {i}", "meaning": "вычисляет результат"} for i in range(4)]
    service = CognitiveService(Client(json.dumps({"cues": cues})))
    assert list(service.extract_cues("Four formulas.")) == cues
    assert "Не оптимизируй уникальность" in prompts.SEED_SYSTEM
    assert "среди тысячи" not in prompts.SEED_SYSTEM
