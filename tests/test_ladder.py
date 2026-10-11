"""Alt+L ladder (experiment 2026-10-09): one rung from what holds toward a far place."""
import json

import pytest

from cognitive_popups.service import CognitiveService


class Client:
    def __init__(self, reply):
        self.reply, self.calls = reply, []

    def complete(self, messages, **kwargs):
        self.calls.append(messages)
        return json.dumps(self.reply, ensure_ascii=False)


def test_rung_sends_target_around_and_known_and_returns_one_step():
    client = Client({"from": "перестановка", "to": "симметрия корней", "text": "Абзац."})
    step = CognitiveService(client).ladder_step(
        "группа Галуа", "рядом текст", ["[верно] корни можно переставлять", "[ошибка] D это корень"])
    sent = client.calls[0][1]["content"]
    assert "<TARGET>\nгруппа Галуа" in sent and "<AROUND>\nрядом текст" in sent
    assert "- [верно] корни можно переставлять" in sent and "- [ошибка] D это корень" in sent
    assert (step.start, step.step, step.text) == ("перестановка", "симметрия корней", "Абзац.")
    assert step.prompt_hash


def test_without_known_there_is_no_known_block_and_an_empty_answer_fails():
    client = Client({"from": "", "to": "", "text": "Абзац."})
    CognitiveService(client).ladder_step("цель")
    assert "<KNOWN>" not in client.calls[0][1]["content"]
    with pytest.raises(Exception):
        CognitiveService(Client({"text": ""})).ladder_step("цель")
    with pytest.raises(ValueError):
        CognitiveService(Client({})).ladder_step("  ")
