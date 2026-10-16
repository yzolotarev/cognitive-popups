"""One model config for every entry point; failures become one human line."""
import io
import json
import socket
from unittest.mock import patch
from urllib.error import HTTPError, URLError

import pytest

from cognitive_popups import client as client_module
from cognitive_popups import tasks, universe
from cognitive_popups.client import Web2APIError, client_from_env
from cognitive_popups.errors import SAVED, classify, human_error


class Response:
    def __init__(self, payload):
        self.payload = payload

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def read(self):
        return json.dumps(self.payload).encode()


@pytest.fixture
def model_env(monkeypatch):
    monkeypatch.setenv("COGNITIVE_API_URL", "https://example.test/v1/chat/completions")
    monkeypatch.setenv("COGNITIVE_MODEL", "other-model")
    monkeypatch.setenv("COGNITIVE_API_KEY", "secret-key")
    monkeypatch.setenv("COGNITIVE_TIMEOUT", "17")
    monkeypatch.setenv("COGNITIVE_OBSERVATION_DISABLE", "1")


def test_client_from_env_reads_all_four(model_env):
    api = client_from_env()
    assert api.url == "https://example.test/v1/chat/completions"
    assert api.model == "other-model"
    assert api.api_key == "secret-key"
    assert api.timeout == 17.0
    assert client_from_env(model="explicit").model == "explicit"


def test_defaults_without_environment(monkeypatch):
    for name in ("COGNITIVE_API_URL", "COGNITIVE_MODEL", "COGNITIVE_API_KEY", "COGNITIVE_TIMEOUT"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("COGNITIVE_TIMEOUT", "not a number")
    api = client_from_env()
    assert api.url == client_module.DEFAULT_URL
    assert api.model == client_module.DEFAULT_MODEL
    assert api.timeout == client_module.DEFAULT_TIMEOUT
    assert api.api_key == ""


def test_bearer_header_is_sent_only_with_a_key(model_env, monkeypatch):
    seen = {}

    def open_url(req, timeout):
        seen.update(dict(req.header_items()))
        seen["url"] = req.full_url
        seen["model"] = json.loads(req.data)["model"]
        return Response({"choices": [{"message": {"content": "ok"}}]})

    with patch.object(client_module.request, "urlopen", open_url):
        assert client_from_env().complete([{"role": "user", "content": "x"}]) == "ok"
    assert seen["Authorization"] == "Bearer secret-key"
    assert seen["url"] == "https://example.test/v1/chat/completions"
    assert seen["model"] == "other-model"
    seen.clear()
    monkeypatch.delenv("COGNITIVE_API_KEY")
    with patch.object(client_module.request, "urlopen", open_url):
        client_from_env().complete([{"role": "user", "content": "x"}])
    assert "Authorization" not in seen


def test_tasks_and_universe_use_the_shared_config(model_env, monkeypatch):
    assert universe.client_from_env().model == "other-model"
    assert universe.client_from_env().timeout == 17.0
    monkeypatch.delenv("COGNITIVE_TIMEOUT")
    assert universe.client_from_env().timeout == 120.0
    assert tasks._model_client().model == "other-model"
    source = open(tasks.__file__, encoding="utf-8").read()
    assert source.count("GeminiWeb2API()") == 1, "only the stub path builds a bare client"


def test_404_says_the_model_was_not_found_and_what_to_change(model_env):
    def open_url(req, timeout):
        raise HTTPError(req.full_url, 404, "Not Found", {}, io.BytesIO(b'{"error":"no such model"}'))

    with patch.object(client_module.request, "urlopen", open_url):
        with pytest.raises(Web2APIError) as caught:
            client_from_env().complete([{"role": "user", "content": "x"}])
    assert caught.value.kind == "model_not_found"
    assert caught.value.http_status == 404
    assert "other-model" in str(caught.value) and "COGNITIVE_MODEL" in str(caught.value)
    line = human_error(caught.value)
    assert line == "Модель «other-model» не найдена — поменяй COGNITIVE_MODEL в окружении сервиса."


def test_5xx_stays_a_bridge_error(model_env):
    def open_url(req, timeout):
        raise HTTPError(req.full_url, 503, "busy", {}, io.BytesIO(b""))

    with patch.object(client_module.request, "urlopen", open_url):
        with pytest.raises(Web2APIError) as caught:
            client_from_env().complete([{"role": "user", "content": "x"}])
    assert caught.value.kind == "http"
    assert "503" in human_error(caught.value)


@pytest.mark.parametrize("value,kind", [
    (Web2APIError("x", kind="transport", url="http://127.0.0.1:8081/v1/chat/completions"), "transport"),
    (URLError("refused"), "transport"),
    (ConnectionRefusedError(), "transport"),
    (TimeoutError(), "timeout"),
    (socket.timeout(), "timeout"),
    (URLError(TimeoutError()), "timeout"),
    (Web2APIError("empty model response"), "empty"),
    (Web2APIError("model returned malformed JSON", kind="json"), "json"),
    (json.JSONDecodeError("x", "doc", 0), "json"),
    (ValueError("simple совпадает с term: 'реестр'"), "validation"),
    (Web2APIError("не удалось получить четыре объекта: meaning длиннее"), "validation"),
    ("bridge call failed (0s, http://127.0.0.1:8081/...): HTTP Error 404", "model_not_found"),
    ("<urlopen error [Errno 111] Connection refused>", "transport"),
    ("", "other"),
])
def test_classify(value, kind):
    assert classify(value) == kind


def test_human_error_is_one_short_line_without_internals():
    samples = [
        Web2APIError("cannot reach the bridge (0s, http://127.0.0.1:8081/v1/x): <urlopen error>",
                     kind="transport", url="http://127.0.0.1:8081/v1/chat/completions"),
        RuntimeError("Traceback (most recent call last):\n  File ..."),
        ValueError("simple совпадает с term: 'реестр'"),
        "",
    ]
    for sample in samples:
        line = human_error(sample)
        assert "\n" not in line
        assert "Traceback" not in line and "urlopen" not in line and "/v1/" not in line
        assert "Error" not in line and len(line) < 140
    assert human_error(samples[0], saved=True) == (
        "Модель не ответила — мост 127.0.0.1:8081 недоступен. " + SAVED)
