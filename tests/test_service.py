import json

from cognitive_popups.service import CognitiveService


class FakeClient:
    def __init__(self):
        self.calls = []
        self.responses = iter([
            '{"cues":[{"simple":"память","term":"memory","meaning":"удержание информации"},{"simple":"предел","term":"limit","meaning":"ограничение количества"},{"simple":"группы","term":"chunking","meaning":"объединение элементов"},{"simple":"заметка","term":"external note","meaning":"внешняя опора"}]}',
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
