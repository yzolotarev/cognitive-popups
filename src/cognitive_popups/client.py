from __future__ import annotations

import json
import re
from typing import Any
from urllib import request


class Web2APIError(RuntimeError):
    pass


class GeminiWeb2API:
    def __init__(self, url: str = "http://127.0.0.1:8081/v1/chat/completions", model: str = "gemini-flash-lite", timeout: float = 60.0):
        self.url = url
        self.model = model
        self.timeout = timeout

    def complete(self, messages: list[dict[str, str]], *, temperature: float = 0.0, max_tokens: int = 800) -> str:
        payload = json.dumps({
            "model": self.model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
        }, ensure_ascii=False).encode("utf-8")
        req = request.Request(self.url, data=payload, headers={"Content-Type": "application/json"}, method="POST")
        try:
            with request.urlopen(req, timeout=self.timeout) as response:
                data: dict[str, Any] = json.loads(response.read().decode("utf-8"))
        except Exception as exc:
            raise Web2APIError(str(exc)) from exc
        try:
            content = data["choices"][0]["message"].get("content", "")
        except (KeyError, IndexError, TypeError) as exc:
            raise Web2APIError("invalid chat completion response") from exc
        if not isinstance(content, str) or not content.strip():
            raise Web2APIError("empty model response")
        return content.strip()


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
        try:
            value = json.loads(match.group(0))
        except json.JSONDecodeError as exc:
            raise Web2APIError("model returned malformed JSON") from exc
    if not isinstance(value, dict):
        raise Web2APIError("model JSON result is not an object")
    return value
