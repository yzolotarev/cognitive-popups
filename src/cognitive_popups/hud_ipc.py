"""Local transport between the side panel and the desktop layer.

The panel is its own process: it has to keep drawing while a popup is open, so
it never runs the reading work itself.  It asks over a Unix socket and the
desktop layer answers (`HudServer`).  One request per connection, one JSON line
each way — the same shape as the popup helper, so a truncated or half-written
message can never leave the two processes out of step.

Only fixed command names travel over the socket.  Nothing here builds a shell
command out of a client's string: the panel and the daemon exchange names, and
the daemon decides what a name means (see `desktop.HUD_ACTIONS`).

The socket lives in the state directory, so it moves with `COGNITIVE_STATE_DIR`
and never opens a network port.  `COGNITIVE_HUD_SOCKET` overrides the path for a
test or a second checkout.
"""
from __future__ import annotations

import argparse
import json
import os
import socket
import sys
import threading
from pathlib import Path

#: Bumped when the request or response shape changes. A panel left over from an
#: older checkout is refused instead of being answered with fields it will
#: misread; the panel then shows "нет связи" and the hotkeys keep working.
PROTOCOL_VERSION = 2

SOCKET_FILENAME = "hud.sock"
ENV_SOCKET = "COGNITIVE_HUD_SOCKET"
DEFAULT_STATE_DIR = "~/.local/state/cognitive-popups"

#: One request is a command and a task id; anything near this size is already a
#: broken client, so the line is cut off instead of being buffered forever.
MAX_MESSAGE_BYTES = 64 * 1024

#: How long `accept` waits before the serve loop looks at the stop flag.
ACCEPT_TIMEOUT = 0.5

#: A client-side socket timeout. The daemon answers from its own thread, so a
#: slow popup in the UI never delays this reply.
DEFAULT_TIMEOUT = 2.0


class HudError(RuntimeError):
    """Raised when the panel socket cannot be published."""


def resolve_socket_path(path: str | Path | None = None) -> Path:
    if path:
        return Path(path).expanduser()
    override = os.environ.get(ENV_SOCKET)
    if override:
        return Path(override).expanduser()
    root = os.environ.get("COGNITIVE_STATE_DIR") or DEFAULT_STATE_DIR
    return Path(root).expanduser() / SOCKET_FILENAME


def encode(payload: dict) -> bytes:
    return (json.dumps(payload, ensure_ascii=False) + "\n").encode("utf-8")


def decode(raw: bytes | bytearray | str) -> dict:
    text = bytes(raw).decode("utf-8") if isinstance(raw, (bytes, bytearray)) else raw
    if not text.strip():
        raise ValueError("empty message")
    payload = json.loads(text)
    if not isinstance(payload, dict):
        raise ValueError("message must be a JSON object")
    return payload


def _read_line(conn: socket.socket) -> bytes:
    """Read up to one newline. The peer may close without sending one."""
    chunks = bytearray()
    while len(chunks) <= MAX_MESSAGE_BYTES:
        chunk = conn.recv(4096)
        if not chunk:
            break
        chunks.extend(chunk)
        if b"\n" in chunk:
            break
    if len(chunks) > MAX_MESSAGE_BYTES:
        raise ValueError("message too large")
    line, _, _ = bytes(chunks).partition(b"\n")
    return line


def _write(conn: socket.socket, payload: dict) -> None:
    try:
        conn.sendall(encode(payload))
    except OSError:
        pass  # the client went away first; nothing to report to anybody


def _listening(path: Path) -> bool:
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
            probe.settimeout(0.3)
            probe.connect(str(path))
        return True
    except OSError:
        return False


def send(request: dict, *, path: str | Path | None = None, timeout: float = DEFAULT_TIMEOUT) -> dict:
    """Ask the desktop layer one question. Never raises: an unreachable daemon
    is an answer the panel has to show, not an exception it has to survive."""
    target = resolve_socket_path(path)
    payload = {"version": PROTOCOL_VERSION, **request}
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
            conn.settimeout(timeout)
            conn.connect(str(target))
            conn.sendall(encode(payload))
            raw = _read_line(conn)
    except OSError as exc:
        return {"ok": False, "error": "unreachable", "detail": str(exc)}
    try:
        return decode(raw)
    except ValueError as exc:
        return {"ok": False, "error": "bad_response", "detail": str(exc)}


class HudServer:
    """The daemon's end of the panel socket.

    `handler(request) -> dict` runs on the accept thread, so it has to be short
    and free of GTK calls.  The daemon answers reads there and marshals anything
    that changes state onto its own main loop (see `desktop.hud_request`).
    """

    def __init__(self, path: str | Path | None = None, *, handler=None, log=None, expected_version: int = PROTOCOL_VERSION):
        self.path = resolve_socket_path(path)
        self._handler = handler
        self._log = log
        self._expected_version = expected_version
        self._listener: socket.socket | None = None
        self._thread: threading.Thread | None = None
        self._stopping = threading.Event()

    def start(self) -> None:
        path = self.path
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists() or path.is_socket():
            if _listening(path):
                raise HudError(f"another panel server is already listening on {path}")
            # A socket file left by a crashed run would refuse the bind.
            try:
                path.unlink()
            except OSError as exc:
                raise HudError(f"cannot replace stale socket {path}: {exc}") from exc
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            listener.bind(str(path))
            listener.listen(8)
            listener.settimeout(ACCEPT_TIMEOUT)
        except OSError as exc:
            listener.close()
            raise HudError(f"cannot listen on {path}: {exc}") from exc
        self._listener = listener
        self._stopping.clear()
        self._thread = threading.Thread(target=self._serve, name="hud-ipc", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stopping.set()
        listener, self._listener = self._listener, None
        if listener is not None:
            try:
                listener.close()
            except OSError:
                pass
        thread, self._thread = self._thread, None
        if thread is not None and thread.is_alive() and thread is not threading.current_thread():
            thread.join(timeout=2.0)
        try:
            self.path.unlink()
        except OSError:
            pass

    def _serve(self) -> None:
        listener = self._listener
        while not self._stopping.is_set() and listener is not None:
            try:
                conn, _ = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                with conn:
                    self._handle(conn)
            except OSError:
                continue

    def _handle(self, conn: socket.socket) -> None:
        conn.settimeout(DEFAULT_TIMEOUT)
        try:
            raw = _read_line(conn)
        except (OSError, ValueError) as exc:
            _write(conn, {"ok": False, "error": "bad_request", "detail": str(exc)})
            return
        try:
            request = decode(raw)
        except ValueError as exc:
            _write(conn, {"ok": False, "error": "bad_request", "detail": str(exc)})
            return
        try:
            version = int(request.get("version") or 0)
        except (TypeError, ValueError):
            version = 0
        if version != self._expected_version:
            _write(conn, {"ok": False, "error": "version_mismatch",
                          "detail": f"expected {self._expected_version}, got {version}"})
            return
        if self._handler is None:
            _write(conn, {"ok": False, "error": "no_handler"})
            return
        try:
            response = self._handler(request)
        except Exception as exc:  # noqa: BLE001 - one bad request must not stop the socket
            if self._log:
                self._log(f"hud handler failed: {exc}")
            _write(conn, {"ok": False, "error": "internal_error", "detail": str(exc)})
            return
        if not isinstance(response, dict):
            response = {"ok": False, "error": "bad_response"}
        response.setdefault("version", self._expected_version)
        _write(conn, response)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m cognitive_popups.hud_ipc",
        description="Ask the running desktop layer what the side panel should show.",
    )
    parser.add_argument("--socket", default=None, help=f"socket path (default {SOCKET_FILENAME} in the state directory)")
    parser.add_argument("--state", action="store_true", help="read the panel state instead of only pinging")
    args = parser.parse_args(argv)

    request = {"command": "state"} if args.state else {"command": "ping"}
    response = send(request, path=args.socket)
    print(json.dumps(response, ensure_ascii=False, indent=2, sort_keys=True))
    return 0 if response.get("ok") else 1


if __name__ == "__main__":  # pragma: no cover - CLI entry point
    sys.exit(main())
