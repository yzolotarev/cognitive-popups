"""A replaced queued preparation is closed; interrupted work may run again; background priority."""
from cognitive_popups import tasks
from cognitive_popups.client import PRIORITY_BACKGROUND
from cognitive_popups.practice import PreparationContext, PreparationQueue
from cognitive_popups.records import RecordStore


def _context(text: str, digest: str) -> PreparationContext:
    return PreparationContext(source_text=text, source_hash=digest, fragment_ids=("f1",),
                              model="m", prompt_revision="r")


def test_a_queued_row_replaced_before_it_ran_is_superseded(tmp_path):
    store = RecordStore(tmp_path / "records.sqlite3")
    queue = PreparationQueue(store, client=None)  # never started: submissions stay queued
    first = queue.submit(_context("Первый абзац о группах.", "h1"))
    second = queue.submit(_context("Второй абзац о кольцах.", "h2"))
    assert store.preparation(first)["status"] == "superseded"
    assert store.preparation(second)["status"] == "queued"
    assert store.latest_preparation()["id"] == second
    # The superseded material may be prepared again later, on the same row.
    again = queue.submit(_context("Первый абзац о группах.", "h1"))
    assert again == first and store.preparation(first)["status"] == "queued"
    assert len(store.preparations()) == 2


def test_interrupted_material_can_be_prepared_again_but_finished_cannot(tmp_path):
    store = RecordStore(tmp_path / "records.sqlite3")
    queue = PreparationQueue(store, client=None)
    first = queue.submit(_context("Абзац.", "h1"))
    store.interrupt_preparations()
    assert queue.submit(_context("Абзац.", "h1")) == first
    store.update_preparation(first, status="ready")
    assert queue.submit(_context("Абзац.", "h1")) is None


def test_generate_task_passes_background_priority_to_the_client(monkeypatch):
    seen = []

    class Client:
        model = "m"

        def complete(self, messages, **kwargs):
            seen.append(kwargs)
            return "{}"

    try:
        tasks.generate_task(Client(), "apply", "prompt", "Материал.", priority=PRIORITY_BACKGROUND)
    except Exception:  # noqa: BLE001 - an empty task fails validation; only the call matters
        pass
    assert seen and all(call.get("priority") == PRIORITY_BACKGROUND for call in seen)
    seen.clear()
    try:
        tasks.generate_task(Client(), "apply", "prompt", "Материал.")
    except Exception:  # noqa: BLE001
        pass
    assert seen and all("priority" not in call for call in seen)
