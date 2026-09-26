from __future__ import annotations

import json
import re
import time
import uuid
import os
import threading
from typing import Any

from .observation import ObservationStore, timestamp, failure_counts
from .operation_context import OperationContext, current_operation
from . import request_gate
from urllib import error, request


#: Priority values accepted by `complete`. Anything else is treated as manual:
#: the safe direction, because a request nobody marked as background gets to go
#: first rather than wait behind one that was.
PRIORITY_MANUAL = "foreground"
PRIORITY_BACKGROUND = "background"


class Web2APIError(RuntimeError):
    pass


class GeminiWeb2API:
    def __init__(self, url: str = "http://127.0.0.1:8081/v1/chat/completions", model: str = "gemini-flash-lite", timeout: float = 60.0, *, observation_store: ObservationStore | None = None):
        self.url = url
        self.model = model
        self.timeout = timeout
        self._metrics = threading.local()
        self.observation_store = observation_store if observation_store is not None else ObservationStore()

    @property
    def last_metrics(self) -> dict[str, Any]:
        if not hasattr(self._metrics, "value"):
            self._metrics.value = {}
        return self._metrics.value

    @last_metrics.setter
    def last_metrics(self, value):
        self._metrics.value = value

    def health(self, *, timeout: float = 3.0) -> dict[str, Any]:
        """Check the local bridge without sending a generation upstream."""
        url = self.url.split("/v1/", 1)[0].rstrip("/") + "/health"
        started = time.monotonic()
        try:
            with request.urlopen(url, timeout=timeout) as response:
                data = json.loads(response.read().decode("utf-8"))
        except Exception as exc:
            elapsed = int((time.monotonic() - started) * 1000)
            raise Web2APIError(f"bridge health-check failed ({elapsed}ms, {url}): {exc}") from exc
        if not isinstance(data, dict) or data.get("status") != "ok":
            raise Web2APIError(f"bridge health-check returned invalid status: {data!r}")
        data["latency_ms"] = int((time.monotonic() - started) * 1000)
        return data

    def complete(self, messages: list[dict[str, str]], *, temperature: float = 0.0, max_tokens: int = 800,
                 priority: str = PRIORITY_MANUAL) -> str:
        request_id = uuid.uuid4().hex
        context = current_operation() or OperationContext(
            operation_id=os.environ.get("COGNITIVE_OPERATION_ID") or request_id,
            interaction_id=os.environ.get("COGNITIVE_EVENT_SESSION"))
        parameters = {"model": self.model, "temperature": temperature, "max_tokens": max_tokens}
        # Snapshot once: the bytes sent and persisted messages cannot diverge if
        # another thread mutates the caller's list during network IO.
        payload = json.dumps({**parameters, "messages": messages}, ensure_ascii=False).encode("utf-8")
        actual = json.loads(payload)
        started = timestamp()
        store = self.observation_store
        store.start_request(request_id, context, actual["messages"], parameters, started=started)
        self.last_metrics = {"request_id": request_id, "operation_id": context.operation_id,
                             "status": "started"}
        raw = None
        content = None
        http_status = None
        status = "transport_error"
        failure = None
        timeout_reason = (f"client gave up after timeout={self.timeout:.0f}s; "
                          "the bridge may still be waiting for the upstream model")
        try:
            req = request.Request(self.url, data=payload,
                headers={"Content-Type": "application/json", "X-Request-ID": request_id}, method="POST")
            # One request of ours reaches the bridge at a time, and a request the
            # reader made goes before background work that has not started yet
            # (see request_gate). An in-flight background call is not interrupted:
            # the gate cannot preempt a bridge that is already answering.
            gate = (request_gate.background_request()
                    if priority == PRIORITY_BACKGROUND else request_gate.manual_request())
            try:
                with gate:
                    with request.urlopen(req, timeout=self.timeout) as response:
                        http_status = getattr(response, "status", None)
                        raw = response.read()
            except error.HTTPError as exc:
                status = "bridge_error"
                http_status = exc.code
                try:
                    raw = exc.read()
                except Exception:
                    pass
                raise
            except error.URLError as exc:
                status = "timeout" if isinstance(exc.reason, TimeoutError) else "transport_error"
                raise
            except TimeoutError:
                status = "timeout"
                raise
            except Exception:
                status = "transport_error"
                raise
            status = "invalid_response"
            try:
                data = json.loads(raw.decode("utf-8"))
                candidate = data["choices"][0]["message"]["content"]
                if not isinstance(candidate, str):
                    raise ValueError("content must be a string")
                content = candidate
            except (ValueError, KeyError, IndexError, TypeError, AttributeError) as exc:
                raise Web2APIError("invalid chat completion response") from exc
            if not content.strip():
                status = "empty_response"
                raise Web2APIError("empty model response")
            status = "ok"
            return content.strip()
        except BaseException as exc:
            failure = exc
            if not isinstance(exc, Exception):
                status = "interrupted"
                raise
            if isinstance(exc, Web2APIError):
                raise
            reason = timeout_reason if status == "timeout" else (
                "cannot reach the bridge" if status == "transport_error" else "bridge call failed")
            raise Web2APIError(f"{reason} ({time.monotonic() - started[2]:.0f}s, {self.url}): {exc}") from exc
        finally:
            finished = timestamp()
            store.finish_request(request_id, status, raw_response=raw, response_text=content,
                                 error=failure, http_status=http_status, finished=finished)
            self.last_metrics = {"request_id": request_id, "operation_id": context.operation_id,
                "status": status, "latency_ms": int((finished[2] - started[2]) * 1000),
                "observation_failures": failure_counts()}


#: Every escape the JSON format itself defines. A backslash followed by anything
#: else is a syntax error, not a different character.
_JSON_ESCAPES = set('"\\/bfnrt')


def _escape_invalid_backslashes(text: str) -> str:
    """Double the backslashes a model left unescaped inside JSON strings.

    A quoted label can contain a literal backslash followed by a comma.
    If a model copies it into a JSON string without escaping the backslash,
    strict parsing rejects the answer. The synthetic parser tests exercise
    this case without relying on reading material.

    Doubling such a backslash is the only reading that preserves what the model
    meant — a literal backslash. Legal escapes, keys, numbers and structure are
    left exactly as they are, so an answer that is broken for any other reason
    still fails to parse.
    """
    out: list[str] = []
    in_string = False
    index = 0
    length = len(text)
    while index < length:
        char = text[index]
        if not in_string:
            in_string = char == '"'
            out.append(char)
            index += 1
        elif char == '"':
            in_string = False
            out.append(char)
            index += 1
        elif char == "\\":
            following = text[index + 1] if index + 1 < length else ""
            if following in _JSON_ESCAPES:
                out.append(text[index:index + 2])
                index += 2
            elif following == "u" and re.fullmatch(r"[0-9a-fA-F]{4}", text[index + 2:index + 6]):
                out.append(text[index:index + 6])
                index += 6
            else:
                out.append("\\\\")
                index += 1
        else:
            out.append(char)
            index += 1
    return "".join(out)


def _decode_object(candidate: str) -> Any:
    """Decode one JSON object, repairing unescaped backslashes if those are the fault.

    Only an escape error is repaired. Any other defect is reported as it was, so
    a genuinely broken or truncated answer is never guessed at.
    """
    try:
        return json.loads(candidate)
    except json.JSONDecodeError as exc:
        if "escape" not in exc.msg:
            raise Web2APIError("model returned malformed JSON") from exc
        try:
            return json.loads(_escape_invalid_backslashes(candidate))
        except json.JSONDecodeError as retry_exc:
            raise Web2APIError("model returned malformed JSON") from retry_exc


def parse_json_object(raw: str) -> dict[str, Any]:
    text = raw.strip()
    text = re.sub(r"^```(?:json)?\s*", "", text, flags=re.IGNORECASE)
    text = re.sub(r"\s*```$", "", text)
    try:
        value = json.loads(text)
    except json.JSONDecodeError:
        match = re.search(r"\{.*\}", text, re.DOTALL)
        if not match:
            raise Web2APIError("model did not return a JSON object") from None
        value = _decode_object(match.group(0))
    if not isinstance(value, dict):
        raise Web2APIError("model JSON result is not an object")
    return value
