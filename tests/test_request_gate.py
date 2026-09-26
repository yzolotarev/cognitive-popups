from __future__ import annotations

import threading
import time

from cognitive_popups import request_gate


def _wait_for(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.01)
    return predicate()


def test_a_manual_request_is_visible_while_it_waits(tmp_path):
    seen = {}
    with request_gate.manual_request(tmp_path):
        seen["inside"] = request_gate.waiting_manual_requests(tmp_path)
    assert seen["inside"] is True
    assert request_gate.waiting_manual_requests(tmp_path) is False


def test_a_manual_request_does_not_block_another(tmp_path):
    """Two manual requests run side by side, exactly as they did before."""
    barrier = threading.Barrier(2, timeout=5)
    results: list[str] = []

    def worker():
        with request_gate.manual_request(tmp_path):
            barrier.wait()
            results.append("ok")

    threads = [threading.Thread(target=worker) for _ in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=6)

    assert results == ["ok", "ok"]


def test_background_waits_while_a_person_is_waiting(tmp_path):
    order: list[str] = []
    started = threading.Event()

    def manual():
        with request_gate.manual_request(tmp_path):
            started.set()
            order.append("manual")

    def background():
        assert started.wait(timeout=5)
        with request_gate.background_request(tmp_path):
            order.append("background")

    manual_thread = threading.Thread(target=manual)
    background_thread = threading.Thread(target=background)
    background_thread.start()
    manual_thread.start()
    manual_thread.join(timeout=6)
    background_thread.join(timeout=10)

    assert order == ["manual", "background"]


def test_a_manual_request_goes_before_the_next_background_one(tmp_path):
    inside = threading.Event()
    marker_held = threading.Event()
    order: list[str] = []

    def background():
        with request_gate.background_request(tmp_path):
            order.append("background-1")
            inside.set()
            # The in-flight call finishes even though a person has now asked for
            # something: preemption is the limit documented in the module.
            assert marker_held.wait(timeout=5)
        with request_gate.background_request(tmp_path):
            order.append("background-2")

    def manual():
        assert inside.wait(timeout=5)
        with request_gate.manual_request(tmp_path):
            order.append("manual")
            marker_held.set()

    background_thread = threading.Thread(target=background)
    manual_thread = threading.Thread(target=manual)
    background_thread.start()
    manual_thread.start()
    manual_thread.join(timeout=10)
    background_thread.join(timeout=10)

    assert order == ["background-1", "manual", "background-2"]


def test_a_marker_left_by_a_dead_process_is_ignored(tmp_path):
    waiters = request_gate.resolve_root(tmp_path) / request_gate.WAITERS_DIR
    waiters.mkdir(parents=True, exist_ok=True)
    (waiters / "999999-dead").write_text("999999", encoding="utf-8")

    assert request_gate.waiting_manual_requests(tmp_path) is False
    assert not (waiters / "999999-dead").exists()


def test_the_gate_can_be_switched_off(tmp_path, monkeypatch):
    monkeypatch.setenv(request_gate.ENV_DISABLE, "0")
    with request_gate.manual_request(tmp_path):
        assert request_gate.waiting_manual_requests(tmp_path) is False
    with request_gate.background_request(tmp_path):
        pass  # no lock, no waiting: it just runs


def test_an_unwritable_root_degrades_to_no_coordination(tmp_path):
    blocked = tmp_path / "not-a-directory"
    blocked.write_text("", encoding="utf-8")

    # Neither call may raise: an unusable gate must not become a failed request.
    with request_gate.manual_request(blocked):
        pass
    with request_gate.background_request(blocked):
        pass
