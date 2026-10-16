"""An old waiter marker is stale even when its pid has been reused."""
import os
import time

from cognitive_popups import request_gate


def test_old_marker_with_a_live_pid_is_removed(tmp_path):
    directory = tmp_path / request_gate.WAITERS_DIR
    directory.mkdir()
    fresh = directory / "fresh"
    fresh.write_text(str(os.getpid()))
    assert request_gate.waiting_manual_requests(tmp_path) is True
    old = time.time() - request_gate.STALE_SECONDS - 5
    os.utime(fresh, (old, old))
    assert request_gate.waiting_manual_requests(tmp_path) is False
    assert not fresh.exists()
