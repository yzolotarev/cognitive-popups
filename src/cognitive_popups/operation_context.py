"""Explicit operation propagation across callbacks; no mutable global context."""
from __future__ import annotations

from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import dataclass, field
from functools import wraps
from inspect import iscoroutinefunction
from typing import Optional
import uuid

RUN_ID = uuid.uuid4().hex


@dataclass(frozen=True)
class OperationContext:
    operation_id: str = field(default_factory=lambda: uuid.uuid4().hex)
    interaction_id: Optional[str] = None
    kind: str = "interaction"
    buffer_session_id: Optional[str] = None


_current: ContextVar[OperationContext | None] = ContextVar("operation", default=None)


def current_operation() -> OperationContext | None:
    return _current.get()


@contextmanager
def operation_scope(ctx: OperationContext | None):
    token = _current.set(ctx)
    try:
        yield ctx
    finally:
        _current.reset(token)


def bind_operation(callback, context=None):
    captured = context if context is not None else current_operation()

    if iscoroutinefunction(callback):
        @wraps(callback)
        async def async_bound(*args, **kwargs):
            with operation_scope(captured):
                return await callback(*args, **kwargs)
        return async_bound

    @wraps(callback)
    def bound(*args, **kwargs):
        with operation_scope(captured):
            return callback(*args, **kwargs)

    return bound
