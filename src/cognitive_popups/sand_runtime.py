"""Lifetimes of explicitly requested sand popups, including cancellation."""
import json
import os
import subprocess
import tempfile
import threading
from pathlib import Path

class SandHandle:
    def __init__(self, command, env, root, position=None):
        root = Path(root)
        root.mkdir(parents=True, exist_ok=True)
        fd, name = tempfile.mkstemp(prefix="sand-wait-", suffix=".json", dir=root)
        os.close(fd)
        self.path = Path(name)
        self._lock = threading.Lock()
        self._finished = False
        self._cancelled = False
        self.path.write_text('{"status":"wait"}')
        payload = {"mode": "sand", "control": name}
        if position:
            payload["x"], payload["y"] = position
        try:
            self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, text=True, env=env)
            self.process.stdin.write(json.dumps(payload))
            self.process.stdin.close()
        except Exception:
            self.path.unlink(missing_ok=True)
            raise

    def cancelled(self):
        try:
            return json.loads(self.path.read_text()).get("status") == "cancelled"
        except (OSError, ValueError):
            return False

    def alive(self):
        return self.process.poll() is None and not self.cancelled()

    def finish(self, words=(), status="done", payload=None):
        """Tell the window how waiting ended; with ``payload`` it becomes the words window."""
        with self._lock:
            if self._finished:
                return not self._cancelled
            self._finished = True
            self._cancelled = self.cancelled() or status == "cancelled"
            adopted = payload is not None and not self._cancelled and status == "done"
            message = {"status": "cancelled" if self._cancelled else status, "words": list(words)}
            if adopted:
                message["payload"] = payload
            try:
                staged = self.path.with_suffix(".next")
                staged.write_text(json.dumps(message, ensure_ascii=False))
                staged.replace(self.path)
            except OSError:
                pass
        def reap():
            try:
                # The words window lives as long as the reader keeps it open.
                self.process.wait(timeout=None if adopted else 3)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    self.process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=3)
            finally:
                self.path.unlink(missing_ok=True)
        threading.Thread(target=reap, daemon=True).start()
        return not self._cancelled
