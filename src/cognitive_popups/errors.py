"""One short Russian line for anything that went wrong; the detail goes to the log.

`human_error(exc)` is what a window shows. It never contains a traceback, a URL
path, a Python class name or a raw `str(exc)`: those were the «<urlopen error
…>» and «simple совпадает с term» lines the reader could do nothing with. The
full text is still logged by the caller (event log, obs_requests).

`saved=True` appends «Твой текст сохранён.» — pass it only when the caller
really did save the reader's words before the call.
"""
from __future__ import annotations

import json
import socket
from urllib import error as urlerror
from urllib.parse import urlsplit

from .client import DEFAULT_URL, ENV_MODEL, Web2APIError

SAVED = "Твой текст сохранён."


def _host(url: str | None) -> str:
    try:
        return urlsplit(url or DEFAULT_URL).netloc or "127.0.0.1:8081"
    except ValueError:
        return "127.0.0.1:8081"


def classify(value: object) -> str:
    """transport | timeout | model_not_found | http | empty | json | validation | storage | other."""
    kind = getattr(value, "kind", None)
    if kind in {"transport", "timeout", "model_not_found", "http", "empty", "json"}:
        return kind
    if kind == "invalid":
        return "json"
    cause = getattr(value, "__cause__", None)
    if isinstance(value, Web2APIError) and cause is not None and cause is not value:
        inner = classify(cause)
        if inner != "other":
            return inner
    if isinstance(value, (TimeoutError, socket.timeout)):
        return "timeout"
    if isinstance(value, urlerror.HTTPError):
        return "model_not_found" if 400 <= value.code < 500 else "http"
    if isinstance(value, urlerror.URLError):
        return "timeout" if isinstance(value.reason, TimeoutError) else "transport"
    if isinstance(value, ConnectionError):
        return "transport"
    if isinstance(value, json.JSONDecodeError):
        return "json"
    name = type(value).__name__
    if name in {"RecordError", "NoteError", "OperationalError", "DatabaseError"}:
        return "storage"
    if isinstance(value, ValueError) or name.endswith("ValidationError"):
        return "validation"
    text = str(value).casefold()
    if not text.strip():
        return "other"
    for needle, found in (
        ("не найдена", "model_not_found"), ("http error 404", "model_not_found"),
        ("http error 400", "model_not_found"), ("http error 4", "model_not_found"),
        ("timed out", "timeout"), ("timeout", "timeout"), ("gave up after", "timeout"),
        ("cannot reach", "transport"), ("connection refused", "transport"),
        ("urlopen error", "transport"), ("health-check failed", "transport"),
        ("empty model response", "empty"), ("пуст", "empty"),
        ("json", "json"), ("invalid chat completion", "json"),
        ("bridge call failed", "http"), ("http error", "http"),
        ("cannot write", "storage"), ("cannot open", "storage"), ("database", "storage"),
        ("не удалось", "validation"), ("длиннее", "validation"), ("совпадает", "validation"),
    ):
        if needle in text:
            return found
    return "other"


def human_error(value: object, *, saved: bool = False, url: str | None = None) -> str:
    """One short line for a window (see module docstring)."""
    kind = classify(value)
    host = _host(url or getattr(value, "url", None))
    model = getattr(value, "model", "") or ""
    if kind == "transport":
        line = f"Модель не ответила — мост {host} недоступен."
    elif kind == "timeout":
        line = "Модель не успела ответить — попробуй ещё раз чуть позже."
    elif kind == "model_not_found":
        named = f"«{model}» " if model else ""
        line = f"Модель {named}не найдена — поменяй {ENV_MODEL} в окружении сервиса."
    elif kind == "http":
        status = getattr(value, "http_status", None)
        line = f"Мост {host} ответил ошибкой{f' {status}' if status else ''} — попробуй ещё раз."
    elif kind == "empty":
        line = "Модель вернула пустой ответ — попробуй ещё раз."
    elif kind == "json":
        line = "Модель ответила не в том формате — попробуй ещё раз."
    elif kind == "validation":
        line = "Ответ модели не прошёл проверку — попробуй ещё раз."
    elif kind == "storage":
        line = "Не удалось записать в базу — подробности в журнале."
        return line
    else:
        line = "Что-то пошло не так — подробности в журнале."
    return f"{line} {SAVED}" if saved else line
