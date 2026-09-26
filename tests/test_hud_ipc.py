from __future__ import annotations

import json
import socket

import pytest

from cognitive_popups import hud_ipc
from cognitive_popups.hud_ipc import PROTOCOL_VERSION, HudError, HudServer, decode, encode, send


def _raw_request(path, payload: bytes, timeout: float = 2.0) -> dict:
    """One hand-written request, for the cases `send` would never produce."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
        conn.settimeout(timeout)
        conn.connect(str(path))
        conn.sendall(payload)
        return json.loads(conn.recv(4096).decode("utf-8"))


def test_round_trip_answers_one_request(tmp_path):
    server = HudServer(tmp_path / "hud.sock",
                       handler=lambda request: {"ok": True, "command": request.get("command")})
    server.start()
    try:
        response = send({"command": "state"}, path=server.path)
    finally:
        server.stop()

    assert response["ok"] is True
    assert response["command"] == "state"
    assert response["version"] == PROTOCOL_VERSION


def test_bad_json_is_refused_and_the_socket_survives(tmp_path):
    seen: list[dict] = []
    server = HudServer(tmp_path / "hud.sock", handler=lambda request: seen.append(request) or {"ok": True})
    server.start()
    try:
        refused = _raw_request(server.path, b"{not json}\n")
        # The socket is still usable: a broken client is not a broken daemon.
        answered = send({"command": "ping"}, path=server.path)
    finally:
        server.stop()

    assert refused["ok"] is False
    assert refused["error"] == "bad_request"
    assert answered["ok"] is True
    assert [request["command"] for request in seen] == ["ping"]


def test_version_mismatch_is_refused_before_the_handler_sees_it(tmp_path):
    seen: list[dict] = []
    server = HudServer(tmp_path / "hud.sock", handler=lambda request: seen.append(request) or {"ok": True})
    server.start()
    try:
        response = _raw_request(server.path, encode({"version": 99, "command": "ping"}))
    finally:
        server.stop()

    assert response["ok"] is False
    assert response["error"] == "version_mismatch"
    assert seen == []


def test_handler_failure_keeps_the_socket_alive(tmp_path):
    calls = {"count": 0}

    def handler(request):
        calls["count"] += 1
        if calls["count"] == 1:
            raise RuntimeError("boom")
        return {"ok": True}

    server = HudServer(tmp_path / "hud.sock", handler=handler)
    server.start()
    try:
        first = send({"command": "ping"}, path=server.path)
        second = send({"command": "ping"}, path=server.path)
    finally:
        server.stop()

    assert first["ok"] is False
    assert first["error"] == "internal_error"
    assert second["ok"] is True


def test_send_reports_an_unreachable_daemon(tmp_path):
    response = send({"command": "ping"}, path=tmp_path / "missing.sock", timeout=0.5)

    assert response["ok"] is False
    assert response["error"] == "unreachable"


def test_a_second_server_on_the_same_path_is_refused(tmp_path):
    path = tmp_path / "hud.sock"
    first = HudServer(path, handler=lambda request: {"ok": True})
    first.start()
    try:
        with pytest.raises(HudError):
            HudServer(path, handler=lambda request: {"ok": True}).start()
    finally:
        first.stop()


def test_a_stale_socket_file_is_replaced_and_the_path_is_removed(tmp_path):
    path = tmp_path / "hud.sock"
    # What a crashed run leaves behind: a name with nothing listening on it.
    path.write_text("", encoding="utf-8")

    server = HudServer(path, handler=lambda request: {"ok": True})
    server.start()
    try:
        assert send({"command": "ping"}, path=path)["ok"] is True
    finally:
        server.stop()

    assert not path.exists()


def test_decode_accepts_only_json_objects():
    assert decode(b'{"a": 1}') == {"a": 1}
    for bad in (b"", b"   ", b"[1, 2]", b'"text"', b"{oops}"):
        with pytest.raises(ValueError):
            decode(bad)


def test_main_reports_failure_when_nothing_listens(tmp_path, capsys):
    code = hud_ipc.main(["--socket", str(tmp_path / "missing.sock")])

    assert code == 1
    assert '"ok": false' in capsys.readouterr().out
