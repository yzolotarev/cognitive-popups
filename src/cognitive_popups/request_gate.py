"""Who reaches the model bridge first, across processes.

A manual request and a background one can be issued from different processes: the
daemon does the reading work, `cognitive-tasks.sh` writes its own tasks, and the
background preparation runs inside the daemon. A lock inside a single process
therefore proves nothing about the others, and there is no queue of our own to
preempt — the bridge is reached directly over HTTP (see `client.py`).

Two things are coordinated through files in the state directory:

* a marker directory, one per waiting manual request, so background work can
  tell that a person is waiting;
* one lock, held **only by background requests**, so background work never runs
  two calls at once.

A manual request takes no lock at all. It never waits for background work, and
manual requests still run alongside each other exactly as they did before — this
only decides whether background work starts, not what a person may do.

The guarantee is deliberately narrow:

    a manual request goes before background requests that have not started yet.

A background request already in flight is **not** interrupted. A client timeout
does not stop the bridge either (see the note in `client.complete`), so promising
preemption here would be a promise the transport cannot keep.

Everything degrades to "no coordination" rather than to "no call": an unwritable
state directory must never turn into a failed request.
"""
from __future__ import annotations

import os
import time
import uuid
from contextlib import contextmanager
from pathlib import Path

try:  # Linux only, like the rest of the app.
    import fcntl
except ImportError:  # pragma: no cover - non-Unix fallback
    fcntl = None  # type: ignore[assignment]

STATE_ROOT = os.environ.get("COGNITIVE_STATE_DIR") or "~/.local/state/cognitive-popups"
ENV_ROOT = "COGNITIVE_REQUEST_GATE_DIR"
ENV_DISABLE = "COGNITIVE_REQUEST_GATE"

LOCK_NAME = "requests.lock"
WAITERS_DIR = "request_waiters"

#: How often a background request re-checks whether somebody is waiting, and how
#: long it is willing to yield before it runs anyway.
POLL_SECONDS = 0.05
MAX_POLLS = 200  # ~10 s: a stale waiter must not hold the app up forever.


def enabled() -> bool:
    return os.environ.get(ENV_DISABLE, "") not in {"0", "off", "false", "no"}


def resolve_root(root: str | Path | None = None) -> Path:
    if root is not None:
        return Path(root).expanduser()
    override = os.environ.get(ENV_ROOT)
    if override:
        return Path(override).expanduser()
    return Path(STATE_ROOT).expanduser()


def _lock_path(root: Path) -> Path:
    return root / LOCK_NAME


def _waiters_dir(root: Path) -> Path:
    return root / WAITERS_DIR


def _alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def waiting_manual_requests(root: str | Path | None = None) -> bool:
    """Whether a person is waiting to make a request right now.

    A marker whose process is gone is a leftover from a crash, not a waiter, and
    is removed instead of stalling background work forever.
    """
    directory = _waiters_dir(resolve_root(root))
    try:
        entries = list(directory.iterdir())
    except OSError:
        return False
    for entry in entries:
        try:
            pid = int(entry.read_text(encoding="utf-8").strip() or 0)
        except (OSError, ValueError):
            pid = 0
        if pid and _alive(pid):
            return True
        try:
            entry.unlink()
        except OSError:
            pass
    return False


def _add_waiter(root: Path) -> Path | None:
    directory = _waiters_dir(root)
    marker = directory / f"{os.getpid()}-{uuid.uuid4().hex}"
    try:
        directory.mkdir(parents=True, exist_ok=True)
        marker.write_text(str(os.getpid()), encoding="utf-8")
    except OSError:
        return None
    return marker


def _remove_waiter(marker: Path | None) -> None:
    if marker is None:
        return
    try:
        marker.unlink()
    except OSError:
        pass


@contextmanager
def _hold(root: Path):
    """Hold the single request lock. Degrades to no locking if unavailable."""
    if fcntl is None or not enabled():
        yield
        return
    path = _lock_path(root)
    handle = None
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        handle = open(path, "a+")
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
    except OSError:
        if handle is not None:
            handle.close()
        yield
        return
    try:
        yield
    finally:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        except OSError:
            pass
        handle.close()


@contextmanager
def manual_request(root: str | Path | None = None):
    """A request a person asked for: announce it, then go.

    No lock is taken. A manual request must never queue behind background work,
    and two manual requests must keep running side by side as they always have.
    The marker is written first so that background work, which does hold the
    lock, sees the waiter and stands down after its current call.
    """
    target = resolve_root(root)
    marker = _add_waiter(target) if enabled() else None
    try:
        yield
    finally:
        _remove_waiter(marker)


@contextmanager
def background_request(root: str | Path | None = None):
    """A request nobody asked for: run it only while no one else is waiting.

    Background calls are serialised by the lock; manual ones are not, so a
    person never queues behind this.
    """
    target = resolve_root(root)
    if not enabled():
        yield
        return
    for _ in range(MAX_POLLS):
        if waiting_manual_requests(target):
            time.sleep(POLL_SECONDS)
            continue
        with _hold(target):
            # Re-checked under the lock: a manual request may have announced
            # itself while this one was queued behind another background call.
            if not waiting_manual_requests(target):
                yield
                return
        time.sleep(POLL_SECONDS)
    # A waiter that never clears must not stop the app; the call runs, but it is
    # still the only one of ours talking to the bridge.
    with _hold(target):
        yield
