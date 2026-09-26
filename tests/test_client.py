import json
import pytest


@pytest.fixture(autouse=True)
def isolated_storage(tmp_path, monkeypatch):
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    monkeypatch.setenv("COGNITIVE_RECORD_DB", str(tmp_path / "records.sqlite3"))
    monkeypatch.delenv("COGNITIVE_EVENT_DISABLE", raising=False)
    monkeypatch.delenv("COGNITIVE_OBSERVATION_DISABLE", raising=False)

from unittest.mock import patch

from cognitive_popups import client as client_module
from cognitive_popups.client import GeminiWeb2API, Web2APIError, parse_json_object


class Response:
    def __init__(self, payload):
        self.payload = payload

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def read(self):
        return json.dumps(self.payload).encode()


def test_health_uses_local_endpoint_and_reports_latency():
    seen = []

    def open_url(req, timeout):
        seen.append((req, timeout))
        return Response({"status": "ok", "version": "test"})

    with patch.object(client_module.request, "urlopen", open_url):
        data = GeminiWeb2API().health()
    assert seen[0][0] == "http://127.0.0.1:8081/health"
    assert data["status"] == "ok"
    assert data["latency_ms"] >= 0


def test_complete_records_success_metrics_and_request_id():
    headers = {}

    def open_url(req, timeout):
        headers.update(dict(req.header_items()))
        return Response({"choices": [{"message": {"content": "answer"}}]})

    api = GeminiWeb2API()
    with patch.object(client_module.request, "urlopen", open_url):
        assert api.complete([{"role": "user", "content": "hello"}]) == "answer"
    assert api.last_metrics["status"] == "ok"
    assert api.last_metrics["request_id"]
    assert headers["X-request-id"] == api.last_metrics["request_id"]


def test_complete_marks_empty_response():
    api = GeminiWeb2API()
    with patch.object(
        client_module.request,
        "urlopen",
        lambda req, timeout: Response({"choices": [{"message": {"content": ""}}]}),
    ):
        try:
            api.complete([{"role": "user", "content": "hello"}])
        except Web2APIError:
            pass
        else:
            raise AssertionError("expected Web2APIError")
    assert api.last_metrics["status"] == "empty_response"


def test_started_before_network_and_messages_not_overwritten():
    api = GeminiWeb2API()
    messages = [{"role": "user", "content": "original"}]

    def open_url(req, timeout):
        with api.observation_store.connect() as conn:
            row = dict(conn.execute("SELECT * FROM obs_requests").fetchone())
        assert row["status"] == "started"
        assert row["finished_epoch"] is None
        assert json.loads(row["messages_json"]) == json.loads(req.data)["messages"]
        messages[0]["content"] = "mutated"
        return Response({"choices": [{"message": {"content": " answer "}}]})

    with patch.object(client_module.request, "urlopen", open_url):
        assert api.complete(messages) == "answer"
    with api.observation_store.connect() as conn:
        row = dict(conn.execute("SELECT * FROM obs_requests").fetchone())
    assert json.loads(row["messages_json"])[0]["content"] == "original"
    assert row["response_text"] == " answer "
    assert json.loads(row["raw_response"])["choices"]
    assert row["duration_seconds"] == row["finished_mono"] - row["started_mono"]


@pytest.mark.parametrize("case,status", [
    ("timeout", "timeout"), ("url_timeout", "timeout"),
    ("transport", "transport_error"), ("http", "bridge_error"),
    ("json", "invalid_response"), ("envelope", "invalid_response"),
    ("content", "invalid_response"), ("interrupt", "interrupted"),
])
def test_terminal_states(case, status):
    import io
    from urllib.error import URLError, HTTPError
    api = GeminiWeb2API()

    def open_url(req, timeout):
        if case == "timeout":
            raise TimeoutError("late")
        if case == "url_timeout":
            raise URLError(TimeoutError("late"))
        if case == "transport":
            raise URLError("offline")
        if case == "http":
            raise HTTPError(req.full_url, 503, "unavailable", {}, io.BytesIO(b"bridge body"))
        if case == "interrupt":
            raise KeyboardInterrupt()
        if case == "json":
            response = Response(None)
            response.read = lambda: b"not JSON\xff"
            return response
        if case == "content":
            return Response({"choices": [{"message": {"content": 42}}]})
        return Response({"choices": []})

    with patch.object(client_module.request, "urlopen", open_url):
        with pytest.raises(KeyboardInterrupt if case == "interrupt" else Web2APIError):
            api.complete([{"role": "user", "content": "hello"}])
    assert api.last_metrics["status"] == status
    with api.observation_store.connect() as conn:
        row = dict(conn.execute("SELECT * FROM obs_requests").fetchone())
    assert row["status"] == status
    assert row["finished_epoch"] is not None
    assert row["error_type"]
    if case == "json":
        assert row["raw_response"] == b"not JSON\xff"
    if case == "http":
        assert row["raw_response"] == b"bridge body"
        assert row["http_status"] == 503


def test_concurrent_context_and_metrics_are_isolated():
    from concurrent.futures import ThreadPoolExecutor
    from threading import Barrier
    from cognitive_popups.operation_context import OperationContext, operation_scope
    api = GeminiWeb2API()
    barrier = Barrier(2)

    def open_url(req, timeout):
        barrier.wait(timeout=5)
        return Response({"choices": [{"message": {"content": "answer"}}]})

    def worker(name):
        with operation_scope(OperationContext(operation_id=name, interaction_id="i-" + name)):
            api.complete([{"role": "user", "content": name}])
            barrier.wait(timeout=5)
            return dict(api.last_metrics)

    with patch.object(client_module.request, "urlopen", open_url), ThreadPoolExecutor(2) as pool:
        metrics = list(pool.map(worker, ["a", "b"]))
    assert [m["operation_id"] for m in metrics] == ["a", "b"]
    assert len({m["request_id"] for m in metrics}) == 2
    assert api.last_metrics == {}
    with api.observation_store.connect() as conn:
        rows = list(conn.execute("SELECT * FROM obs_requests"))
    assert len(rows) == 2
    for row in rows:
        assert json.loads(row["messages_json"])[0]["content"] == row["operation_id"]
        assert row["interaction_id"] == "i-" + row["operation_id"]


def test_client_survives_broken_store(tmp_path):
    from cognitive_popups.observation import ObservationStore
    broken = tmp_path / "directory"
    broken.mkdir()
    api = GeminiWeb2API(observation_store=ObservationStore(broken))
    with patch.object(client_module.request, "urlopen", return_value=Response(
        {"choices": [{"message": {"content": "ok"}}]})):
        assert api.complete([]) == "ok"
    assert api.last_metrics["observation_failures"]["observation.write"] >= 2


@pytest.mark.parametrize("flag", ["COGNITIVE_EVENT_DISABLE", "COGNITIVE_OBSERVATION_DISABLE"])
def test_disabled_client_still_completes_without_storage(flag, monkeypatch):
    api = GeminiWeb2API()
    monkeypatch.setenv(flag, "true")
    with patch.object(client_module.request, "urlopen", return_value=Response(
        {"choices": [{"message": {"content": "ok"}}]})):
        assert api.complete([]) == "ok"
    assert not api.observation_store.path.exists()


def test_monotonic_duration_ignores_wall_clock_jump():
    api = GeminiWeb2API()
    stamps = [("2030-01-01T00:00:00+00:00", 1000., 20.),
              ("2029-01-01T00:00:00+00:00", 900., 22.5)]
    with patch.object(client_module, "timestamp", side_effect=stamps), patch.object(
        client_module.request, "urlopen", return_value=Response({"choices": [{"message": {"content": "ok"}}]})):
        api.complete([])
    assert api.last_metrics["latency_ms"] == 2500
    with api.observation_store.connect() as conn:
        row = conn.execute("SELECT * FROM obs_requests").fetchone()
    assert row["duration_seconds"] == 2.5
    assert row["finished_epoch"] < row["started_epoch"]


def test_an_unescaped_backslash_in_quoted_material_is_repaired():
    # A synthetic label contains an invalid JSON escape that must stay literal.
    raw = r'{"condition":"Метка детали: q\, z*"}'

    assert parse_json_object(raw)["condition"] == r"Метка детали: q\, z*"


def test_a_fenced_answer_with_an_unescaped_backslash_is_repaired():
    raw = '```json\n' + r'{"status":"ready","grounding":["q\, z*"]}' + '\n```'

    assert parse_json_object(raw)["grounding"] == [r"q\, z*"]


def test_legal_escapes_survive_the_repair():
    parsed = parse_json_object(r'{"a":"one\ntwo\t\"quoted\" Ж \u0416"}')

    assert parsed["a"] == 'one\ntwo\t"quoted" Ж Ж'


def test_a_broken_answer_is_still_refused():
    for raw in ('{"a" "b"}', '{"a": 1,}', 'no object here'):
        with pytest.raises(Web2APIError):
            parse_json_object(raw)
