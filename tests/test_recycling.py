import json
from types import SimpleNamespace

import pytest

from cognitive_popups.context import Pack
from cognitive_popups.models import PredictionCheck
from cognitive_popups import recycling
from cognitive_popups.recycling import RecyclingStore, parse_result
from cognitive_popups.service import CognitiveService


def result():
    return {"text": "Архив хранит документы, и вся история изменений видна. Контроль изменений обеспечивает безопасность процесса.",
            "cues": [{"simple": word, "term": word, "meaning": "Смысл внутри этой версии"}
                     for word in ("архив", "история", "контроль", "безопасность")]}


def checks(n=4):
    statuses = ["unclear", "partially_confirmed", "contradicted", "confirmed"]
    return [PredictionCheck([], f"моя мысль {i}", statuses[i % 4]) for i in range(n)]


def test_four_new_thoughts_regardless_of_verdict_then_next_four(tmp_path):
    store = RecyclingStore(tmp_path / "records.sqlite3")
    thoughts = checks(8)
    assert store.pending("session", thoughts[:3]) == []
    assert store.pending("session", thoughts[:4]) == thoughts[:4]
    pack = Pack("original source")
    store.save("session", thoughts[:4], result(), pack)
    # A newly constructed store keeps the boundary. A step is not a session.
    store = RecyclingStore(store.path)
    assert store.pending("session", thoughts[:7]) == []
    assert store.pending("session", thoughts) == thoughts[4:]
    assert store.pending("new session", checks(3)) == []


def test_retry_is_not_counted_twice_and_failure_does_not_consume_batch(tmp_path):
    store = RecyclingStore(tmp_path / "records.sqlite3")
    thoughts = checks(3)
    duplicate = PredictionCheck([], thoughts[-1].hypothesis, "confirmed")
    assert store.pending("s", thoughts + [duplicate]) == []
    vague = PredictionCheck([], "общие слова", "vague")
    assert store.pending("s", thoughts + [vague]) == []
    fourth = PredictionCheck([], "новая мысль", "unclear")
    batch = store.pending("s", thoughts + [duplicate, fourth])
    assert len(batch) == 4 and batch[-2].id == duplicate.id
    assert store.pending("s", thoughts + [duplicate, fourth]) == batch


def test_original_words_and_source_survive_without_self_confirmation(tmp_path):
    store = RecyclingStore(tmp_path / "records.sqlite3")
    thoughts = checks()
    thoughts[0].hypothesis = "  МОЯ мысль\nбез исправлений  "
    pack = Pack("Original independent source", recent=[{"text": "earlier", "at": 1}])
    store.save("s", thoughts, result(), pack)
    saved = store._read("s")[0]
    assert json.loads(saved["hypotheses_json"])[0]["hypothesis"] == thoughts[0].hypothesis
    original = store.original_material(result()["text"], "s")
    assert original == {"selection": pack.selection, "recent": pack.recent}
    assert store.original_material("архив история контроль безопасность", "s") == original
    assert store.original_material("архив", "s") is None  # unrelated short selection
    assert store.original_material(result()["text"], "other session") is None


def test_generation_receives_only_reader_hypotheses_not_model_verdicts_or_sources():
    calls = []
    class Client:
        def complete(self, messages, **kwargs):
            calls.append(messages)
            return json.dumps(result(), ensure_ascii=False)
    service = CognitiveService(Client())
    thoughts = [c.hypothesis for c in checks()]
    assert service.recycle_hypotheses(thoughts) == result()
    assert json.loads(calls[0][1]["content"]) == {"hypotheses": thoughts}
    assert service.session.fragments == []


@pytest.mark.parametrize("bad", [
    {"text": "А что если?", "cues": result()["cues"]},
    {"text": "Абзац", "cues": result()["cues"][:3]},
    {"text": "Абзац", "cues": [result()["cues"][0]] * 4},
    {"text": "Абзац", "cues": ["bad"] * 4},
])
def test_invalid_response_is_not_presented(bad):
    with pytest.raises(ValueError):
        parse_result(bad)


def test_fourth_submission_replaces_normal_result_and_fifth_does_not(desktop, monkeypatch, tmp_path):
    from cognitive_popups.records import RecordStore
    from cognitive_popups import context
    monkeypatch.setattr(desktop, "STATE_DIR", tmp_path)
    monkeypatch.setattr(desktop, "RECORDS", RecordStore(tmp_path / "records.sqlite3"))
    monkeypatch.setattr(desktop, "emit", lambda *a, **k: None)
    monkeypatch.setattr(desktop, "idle_add", lambda fn, *args: fn(*args))
    class Thread:
        def __init__(self, target, **kwargs): self.target = target
        def start(self): self.target()
    monkeypatch.setattr(desktop.threading, "Thread", Thread)
    class Client:
        def complete(self, messages, **kwargs):
            if '"hypotheses"' in messages[1]["content"]:
                return json.dumps(result(), ensure_ascii=False)
            return json.dumps({"mark": "none", "specific": False, "words": []})
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.service = CognitiveService(Client())
    app._prediction_pack = context.Pack("Source independent of the reader's thoughts")
    app._begin_sand_wait = lambda: None
    app._log_prompt = lambda *a: None
    app._universe = lambda: None
    normal, paragraphs = [], []
    app._prediction_done = lambda *a: (normal.append(a), setattr(app, "_busy", False))
    app.flash = SimpleNamespace(show_text=lambda *a, **kw: paragraphs.append((a, kw)))
    for i in range(4):
        app.run_prediction(f"thought {i}")
    # The verdict never waits for the batch: four normal answers, one offer.
    assert len(normal) == 4 and paragraphs == []
    offer = app._recycle_offer
    assert offer and [c.hypothesis for c in offer[1]] == [f"thought {i}" for i in range(4)]
    app._recycle(offer)
    [(args, kwargs)] = paragraphs
    # None of the four held, so no paragraph is sewn from them: the guesses come back verbatim.
    assert args[0].splitlines() == [f"·  thought {i}" for i in range(4)]
    assert kwargs["note"] == recycling.GROUP_HINT
    assert len(desktop.RECORDS.load_predictions(app.service.session.id)) == 4
    app._recycle_offer = None
    app.run_prediction("fifth thought")
    assert len(normal) == 5 and app._recycle_offer is None


def test_only_guesses_that_held_feed_a_paragraph():
    from types import SimpleNamespace as C
    checks = [C(status="confirmed", hypothesis="a"), C(status="contradicted", hypothesis="b"),
              C(status="partially_confirmed", hypothesis="c"), C(status="not_supported", hypothesis="d")]
    assert [c.hypothesis for c in recycling.usable(checks)] == ["a", "c"]
    assert recycling.verbatim_card(checks).splitlines() == ["✓  a", "?  b", "≈  c", "?  d"]


def test_cue_from_a_lemma_matches_the_inflected_paragraph():
    # Note 111: the model gave «документ», the paragraph said «документы».
    data = result()
    data["cues"][0] = {"simple": "документ", "term": "архив хранит документ", "meaning": "что хранится"}
    assert recycling.parse_result(data)["cues"][0]["simple"] == "документ"
