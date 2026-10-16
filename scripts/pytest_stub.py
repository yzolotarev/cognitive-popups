"""The slice of pytest the suite uses, for `run-tests.py`.

`run-tests.py` installs this module as `pytest` so the test files import it
unchanged. It covers what the suite needs and nothing more: function-scoped
fixtures (plain, yield and autouse, from `conftest.py` or the test module),
`mark.parametrize`, `raises`, `approx`, `skip`, `fail`, and the `tmp_path`,
`monkeypatch` and `capsys` built-ins. Anything else is an AttributeError, which
is the signal to run that test under real pytest.
"""
from __future__ import annotations

import io
import math
import os
import re
import sys
from types import SimpleNamespace

_MISSING = object()


class Skipped(BaseException):
    """Raised by `skip`. A BaseException, as in pytest, so app code catching
    `Exception` does not swallow it."""


class Failed(BaseException):
    """Raised by `fail`."""


def skip(reason: str = "") -> None:
    raise Skipped(reason)


def fail(reason: str = "") -> None:
    raise Failed(reason)


def fixture(function=None, *, autouse: bool = False):
    def mark(function):
        function._fixture = SimpleNamespace(autouse=autouse)
        return function
    return mark(function) if function is not None else mark


def _parametrize(argnames, argvalues):
    names = [name.strip() for name in argnames.split(",")] if isinstance(argnames, str) else list(argnames)

    def mark(function):
        rows = [dict(zip(names, values if len(names) > 1 else (values,))) for values in argvalues]
        # Decorators apply bottom-up; keep the source order of stacked marks.
        function._parametrize = [rows] + getattr(function, "_parametrize", [])
        return function
    return mark


mark = SimpleNamespace(parametrize=_parametrize)


class raises:
    def __init__(self, expected, *, match: str | None = None):
        self.expected, self.match, self.value = expected, match, None

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        if kind is None:
            raise Failed(f"DID NOT RAISE {self.expected}")
        if not issubclass(kind, self.expected):
            return False
        if self.match is not None and not re.search(self.match, str(value)):
            raise Failed(f"{self.match!r} does not match {str(value)!r}")
        self.value = value
        return True


class approx:
    def __init__(self, expected, *, rel: float | None = None, abs: float | None = None):
        self.expected, self.rel, self.abs = expected, rel, abs

    def _close(self, actual, expected) -> bool:
        if self.rel is None and self.abs is None:
            tolerance = max(1e-6 * math.fabs(expected), 1e-12)
        else:
            tolerance = max((self.rel or 0) * math.fabs(expected), self.abs or 0)
        return actual == expected or math.fabs(actual - expected) <= tolerance

    def __eq__(self, actual) -> bool:
        if isinstance(self.expected, (tuple, list)):
            return (len(actual) == len(self.expected)
                    and all(self._close(a, e) for a, e in zip(actual, self.expected)))
        return self._close(actual, self.expected)

    def __repr__(self) -> str:
        return f"approx({self.expected!r})"


class MonkeyPatch:
    def __init__(self):
        self._undo: list = []

    def setattr(self, target, name, value=_MISSING, raising: bool = True):
        if value is _MISSING:  # setattr("package.module.name", value)
            module, _, attribute = target.rpartition(".")
            target, name, value = __import__(module, fromlist=[attribute]), attribute, name
        old = getattr(target, name, _MISSING)
        if old is _MISSING and raising:
            raise AttributeError(f"{target!r} has no attribute {name!r}")
        self._undo.append(lambda: delattr(target, name) if old is _MISSING else setattr(target, name, old))
        setattr(target, name, value)

    def delattr(self, target, name, raising: bool = True):
        old = getattr(target, name, _MISSING)
        if old is _MISSING:
            if raising:
                raise AttributeError(name)
            return
        self._undo.append(lambda: setattr(target, name, old))
        delattr(target, name)

    def setitem(self, mapping, key, value):
        old = mapping.get(key, _MISSING)
        self._undo.append(lambda: mapping.pop(key, None) if old is _MISSING else mapping.__setitem__(key, old))
        mapping[key] = value

    def delitem(self, mapping, key, raising: bool = True):
        if key not in mapping:
            if raising:
                raise KeyError(key)
            return
        old = mapping[key]
        self._undo.append(lambda: mapping.__setitem__(key, old))
        del mapping[key]

    def setenv(self, name, value, prepend=None):
        value = str(value)
        if prepend and name in os.environ:
            value = value + prepend + os.environ[name]
        self.setitem(os.environ, name, value)

    def delenv(self, name, raising: bool = True):
        self.delitem(os.environ, name, raising=raising)

    def undo(self):
        while self._undo:
            self._undo.pop()()


class CaptureFixture:
    def __init__(self):
        self._saved = sys.stdout, sys.stderr
        sys.stdout, sys.stderr = io.StringIO(), io.StringIO()

    def readouterr(self):
        out, err = sys.stdout.getvalue(), sys.stderr.getvalue()
        sys.stdout.seek(0), sys.stdout.truncate()
        sys.stderr.seek(0), sys.stderr.truncate()
        return SimpleNamespace(out=out, err=err)

    def close(self):
        sys.stdout, sys.stderr = self._saved
