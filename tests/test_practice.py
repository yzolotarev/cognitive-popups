"""Background preparation: what it accepts, what it refuses, how the scale moves.

The client is a script of replies, so the module's decisions are what is under
test: which material a task may be built from, and which step of the work counts
as finished. No claim about a reader's understanding appears anywhere here.
"""
from __future__ import annotations

import json
import threading
import time

from cognitive_popups import practice, records
from cognitive_popups.practice import PreparationContext, PreparationQueue


threading_event = threading.Event

SOURCE = (
    "Система называется однородной, если все свободные члены равны нулю. "
    "Неоднородная система имеет ненулевой свободный член, поэтому её решения "
    "не образуют линейное подпространство."
)


class FakeClient:
    """Replies in order, whatever the caller asks for."""

    model = "test-model"

    def __init__(self, *replies):
        self.replies = list(replies)
        self.calls: list[dict] = []

    def complete(self, messages, **kwargs):
        self.calls.append({"messages": messages, **kwargs})
        if not self.replies:
            raise AssertionError("the pipeline asked for more replies than the test scripted")
        return self.replies.pop(0)


def analysis_json(status="candidate", kind="distinguish", grounding=("все свободные члены равны нулю",), **extra):
    data = {
        "status": status,
        "practice_kind": kind,
        "target": "различить однородную и неоднородную систему",
        "grounding": list(grounding),
        "missing_context": [],
        "confidence": "high",
    }
    data.update(extra)
    return json.dumps(data, ensure_ascii=False)


def task_json(condition="Различите однородную и неоднородную систему.\n\nТребуется: назвать признак различия.",
              grounding=("свободные члены",), status="ready", payload=None):
    data = {
        "status": status,
        "condition": condition,
        "grounding": list(grounding),
        "required_operations": ["различить"],
        "difficulty": "quick",
        "domain": "алгебра",
        "subtype": "example_non_example",
        "confidence": "high",
    }
    if payload is not None:
        data["payload"] = payload
    return json.dumps(data, ensure_ascii=False)


def _store(tmp_path) -> records.RecordStore:
    store = records.RecordStore(tmp_path / "records.sqlite3")
    store.open_session("s1")
    return store


def _context(**overrides) -> PreparationContext:
    fields = {
        "source_text": SOURCE,
        "source_hash": "hash-1",
        "fragment_ids": ("f1",),
        "model": "test-model",
        "prompt_revision": "rev-1",
    }
    fields.update(overrides)
    return PreparationContext(**fields)


def _prepare(store, client, context=None, **kwargs):
    context = context or _context()
    preparation_id = practice.new_preparation_id()
    store.save_preparation(
        preparation_id=preparation_id,
        context_key=context.key,
        status="queued",
        completed_stage=1,
        stage_label="Ждёт очереди",
    )
    stages: list[str] = []
    outcome = practice.prepare(
        client, store, context, preparation_id=preparation_id,
        on_stage=stages.append, **kwargs,
    )
    return outcome, stages, preparation_id


# ── the question ─────────────────────────────────────────────────────────────

def test_the_prompt_carries_the_material_as_data_and_the_goal_as_goal():
    prompt = practice.analysis_prompt(_context(goal_text="Различать случаи"))

    assert "<MATERIAL>" in prompt and SOURCE in prompt
    assert "<READER_GOAL>" in prompt and "Различать случаи" in prompt
    assert "не инструкции" in practice.analysis_system_text()


def test_the_prompt_says_so_when_there_is_no_goal():
    prompt = practice.analysis_prompt(_context())

    assert "обзорной" in prompt


def test_the_system_prompt_forbids_judging_the_reader():
    system = practice.analysis_system_text()

    assert "Не оценивай самого читателя" in system
    assert "пора ли ему практиковаться" in system


# ── what an analysis may say ─────────────────────────────────────────────────

def test_an_unknown_practice_kind_is_refused():
    analysis, errors = practice.parse_analysis(analysis_json(kind="teach_me"))

    assert errors
    assert any("practice_kind" in error for error in errors)


def test_grounding_that_is_not_in_the_material_is_refused():
    analysis, _parsed = practice.parse_analysis(analysis_json(grounding=("квантовая запутанность",)))
    errors = practice.validate_analysis(analysis, SOURCE)

    assert errors
    assert any("grounding" in error for error in errors)


def test_grounding_from_the_material_is_accepted():
    analysis, errors = practice.parse_analysis(analysis_json())

    assert errors == []
    assert practice.validate_analysis(analysis, SOURCE) == []
    assert analysis.practice_kind == "distinguish"
    assert analysis.target


def test_an_empty_target_is_refused():
    analysis, _errors = practice.parse_analysis(analysis_json(target=""))

    assert practice.validate_analysis(analysis, SOURCE)


def test_a_refusal_needs_no_fields():
    analysis, errors = practice.parse_analysis(analysis_json(status="insufficient", kind="", grounding=()))

    assert errors == []
    assert analysis.status == "insufficient"


# ── the analysis call ────────────────────────────────────────────────────────

def test_an_invented_quote_is_asked_for_once_more():
    client = FakeClient(
        analysis_json(grounding=("квантовая запутанность",)),
        analysis_json(),
    )

    analysis, errors = practice.analyze(client, _context())

    assert errors == []
    assert analysis.practice_kind == "distinguish"
    assert len(client.calls) == 2
    assert "не прошёл проверку" in client.calls[1]["messages"][1]["content"]


def test_a_refusal_is_not_asked_again():
    client = FakeClient(analysis_json(status="insufficient", kind="", grounding=()))

    analysis, errors = practice.analyze(client, _context())

    assert errors == []
    assert analysis.status == "insufficient"
    assert len(client.calls) == 1


def test_analysis_never_leaves_the_background_priority():
    client = FakeClient(analysis_json())

    practice.analyze(client, _context())

    assert all(call["priority"] == "background" for call in client.calls)


# ── the whole preparation ────────────────────────────────────────────────────

def test_a_good_run_ends_ready_with_a_saved_task(tmp_path):
    store = _store(tmp_path)
    client = FakeClient(analysis_json(), task_json())

    outcome, stages, preparation_id = _prepare(store, client)

    assert outcome.status == "ready"
    assert outcome.task_id
    row = store.preparation(preparation_id)
    assert row["status"] == "ready"
    assert row["completed_stage"] == 4
    assert row["task_id"] == outcome.task_id
    tasks = store.load_tasks()
    assert [task["status"] for task in tasks if task["id"] == outcome.task_id] == ["generated"]


def test_the_scale_exposes_its_stages_in_order(tmp_path):
    store = _store(tmp_path)
    client = FakeClient(analysis_json(), task_json())

    _outcome, stages, _preparation_id = _prepare(store, client)

    assert stages[0] == "analyzing"
    assert "generating" in stages
    assert stages[-1] == "ready"
    assert stages.index("analyzing") < stages.index("generating") < stages.index("ready")


def test_a_repair_in_the_generator_is_reported_as_its_own_stage(tmp_path):
    store = _store(tmp_path)
    client = FakeClient(analysis_json(), "не json", task_json())

    _outcome, stages, _preparation_id = _prepare(store, client)

    assert "repairing" in stages
    assert stages[-1] == "ready"


def test_material_without_a_usable_action_generates_nothing(tmp_path):
    store = _store(tmp_path)
    client = FakeClient(analysis_json(status="insufficient", kind="", grounding=()))

    outcome, stages, preparation_id = _prepare(store, client)

    assert outcome.status == "insufficient"
    assert len(client.calls) == 1
    assert store.load_tasks() == []
    row = store.preparation(preparation_id)
    assert row["status"] == "insufficient"
    assert row["completed_stage"] == 1


def test_a_task_the_generator_refuses_is_not_offered(tmp_path):
    store = _store(tmp_path)
    client = FakeClient(analysis_json(), task_json(condition="", status="needs_context"))

    outcome, _stages, preparation_id = _prepare(store, client)

    assert outcome.status == "insufficient"
    # The refusal is kept in the ledger with the shape the interactive path uses,
    # so a silence never hides why nothing was offered.
    assert [task["status"] for task in store.load_tasks()] == ["invalid"]
    assert store.preparation(preparation_id)["status"] == "insufficient"


def test_a_task_that_fails_verification_twice_stops_halfway(tmp_path):
    store = _store(tmp_path)
    # A system that cannot be consistent: the verifier refuses it in code.
    payload = {"type": "linear_system", "matrix": [[1, 1], [2, 2]], "rhs": [1, 3],
               "require": "consistent"}
    client = FakeClient(
        analysis_json(),
        task_json(payload=payload),
        task_json(payload=payload),
    )

    outcome, _stages, preparation_id = _prepare(store, client)

    assert outcome.status == "failed"
    row = store.preparation(preparation_id)
    # The condition existed (three stages) but the check never passed.
    assert row["completed_stage"] < practice.PROGRESS["ready"]
    assert row["status"] == "failed"


def test_no_material_is_refused_without_calling_the_model(tmp_path):
    store = _store(tmp_path)
    client = FakeClient()

    outcome, _stages, _preparation_id = _prepare(store, client, context=_context(source_text="   "))

    assert outcome.status == "insufficient"
    assert client.calls == []


def test_a_bridge_failure_is_reported_as_failed_not_as_ready(tmp_path):
    store = _store(tmp_path)

    class Broken:
        model = "test-model"

        def complete(self, messages, **kwargs):
            from cognitive_popups.client import Web2APIError
            raise Web2APIError("bridge is down")

    outcome, _stages, preparation_id = _prepare(store, Broken())

    assert outcome.status == "failed"
    assert store.preparation(preparation_id)["status"] == "failed"


# ── the context key ──────────────────────────────────────────────────────────

def test_the_same_material_and_goal_share_a_key():
    assert _context().key == _context().key


def test_a_different_goal_is_a_different_preparation():
    assert _context(goal_id="g1", goal_text="одна цель").key != _context(goal_id="g2", goal_text="другая").key


def test_a_reread_fragment_is_the_same_material():
    first = _context(fragment_ids=("f1",))
    second = _context(fragment_ids=("f1", "f2"))

    # The id list is provenance, not content: re-saving the same text under a new
    # fragment id must not start the work again.
    assert first.key == second.key


# ── the queue ────────────────────────────────────────────────────────────────

def _wait_for(predicate, timeout: float = 8.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.01)
    return predicate()


def _queue(store, client, results: list, stages: list | None = None) -> PreparationQueue:
    return PreparationQueue(
        store, client,
        session_id="s1",
        on_stage=lambda status, preparation_id: (stages if stages is not None else []).append(status),
        on_result=lambda outcome, error: results.append((outcome, error)),
        poll_seconds=0.01,
    )


def test_a_disabled_queue_does_nothing(tmp_path):
    store = _store(tmp_path)
    queue = _queue(store, FakeClient(analysis_json(), task_json()), [])
    queue.start()
    try:
        assert queue.submit(_context(), enabled=False) is None
    finally:
        queue.stop()

    assert store.preparations() == []


def test_the_same_context_is_never_prepared_twice(tmp_path):
    store = _store(tmp_path)
    results: list = []
    queue = _queue(store, FakeClient(analysis_json(), task_json()), results)
    queue.start()
    try:
        first = queue.submit(_context())
        assert first is not None
        assert _wait_for(lambda: results)
        # A fragment re-saved with the same text must not start the work again.
        assert queue.submit(_context()) is None
    finally:
        queue.stop()

    assert len(store.preparations()) == 1


def test_a_run_that_newer_material_replaced_is_not_offered(tmp_path):
    store = _store(tmp_path)
    results: list = []
    release = threading_event()

    class Gated(FakeClient):
        """Holds the first call open so the test can submit newer material."""

        def complete(self, messages, **kwargs):
            if not release.is_set():
                release.wait(timeout=5)
            return super().complete(messages, **kwargs)

    client = Gated(analysis_json(), task_json(), analysis_json(), task_json())
    queue = _queue(store, client, results)
    queue.start()
    try:
        first_id = queue.submit(_context())
        assert first_id is not None
        assert _wait_for(lambda: store.preparation(first_id)["status"] == "analyzing")
        # The reader moves on while the first preparation is still working.
        queue.submit(_context(source_hash="hash-2", source_text=SOURCE + " Ещё абзац."))
        release.set()
        assert _wait_for(lambda: len(results) >= 1)
    finally:
        queue.stop()

    assert _wait_for(lambda: len(store.preparations()) == 2)
    assert store.preparation(first_id)["status"] == "superseded"
    statuses = {row["status"] for row in store.preparations()}
    assert "ready" in statuses


def test_a_preparation_that_raises_is_recorded_as_failed(tmp_path):
    store = _store(tmp_path)
    results: list = []

    class Exploding:
        model = "test-model"

        def complete(self, messages, **kwargs):
            raise RuntimeError("unexpected")

    queue = _queue(store, Exploding(), results)
    queue.start()
    try:
        queue.submit(_context())
        assert _wait_for(lambda: results)
    finally:
        queue.stop()

    assert store.preparations()[0]["status"] == "failed"
