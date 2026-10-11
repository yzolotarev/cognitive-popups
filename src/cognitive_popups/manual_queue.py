"""One pending reader request, with its original material and event chain."""
from __future__ import annotations

from functools import wraps
from threading import RLock, local

selection = local()


class LatestRequest:
    def __init__(self):
        self.lock = RLock()
        self.pending = None

    def put(self, action):
        with self.lock:
            replaced = self.pending is not None
            self.pending = action
            return replaced

    def take(self):
        with self.lock:
            action, self.pending = self.pending, None
            return action


def queued_action(method):
    @wraps(method)
    def run(app, *args, **kwargs):
        if app._busy:
            app._queue_manual(method, args, kwargs)
            return
        return method(app, *args, **kwargs)
    return run
