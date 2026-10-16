#!/usr/bin/env python3
"""Run the test suite without pytest.

The test files import pytest, so this runner installs `pytest_stub.py` in its
place: the slice of pytest the suite uses (fixtures, parametrize, raises,
approx, skip, fail, tmp_path, monkeypatch, capsys). `python3 -m pytest` runs
the same suite when pytest is installed.

    python3 scripts/run-tests.py            # everything
    python3 scripts/run-tests.py test_notes # one module
"""
from __future__ import annotations

import contextlib
import importlib.util
import inspect
import itertools
import os
import pathlib
import sys
import tempfile
import traceback

ROOT = pathlib.Path(__file__).resolve().parent.parent
TESTS = ROOT / "tests"

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import pytest_stub  # noqa: E402

BUILTINS = ("tmp_path", "monkeypatch", "capsys")


def load(path: pathlib.Path, name: str | None = None):
    spec = importlib.util.spec_from_file_location(name or path.stem, path)
    if spec is None or spec.loader is None:  # pragma: no cover - defensive
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fixtures_of(module) -> dict:
    return {name: value for name, value in vars(module).items() if hasattr(value, "_fixture")}


def cases(function):
    """One keyword set per parametrize combination; a single empty set if none."""
    groups = getattr(function, "_parametrize", [])
    for combination in itertools.product(*groups):
        params: dict = {}
        for row in combination:
            params.update(row)
        label = "-".join(repr(value)[:30] for value in params.values())
        yield (f"[{label}]" if params else ""), params


def run_one(function, params: dict, fixtures: dict) -> None:
    """Resolve fixtures the way pytest does for function scope, run, tear down."""
    values: dict = dict(params)
    stack = contextlib.ExitStack()

    def resolve(name: str):
        if name in values:
            return values[name]
        if name == "tmp_path":
            values[name] = pathlib.Path(tempfile.mkdtemp())
        elif name == "monkeypatch":
            patch = pytest_stub.MonkeyPatch()
            stack.callback(patch.undo)
            values[name] = patch
        elif name == "capsys":
            capture = pytest_stub.CaptureFixture()
            stack.callback(capture.close)
            values[name] = capture
        elif name in fixtures:
            factory = fixtures[name]
            result = factory(**{arg: resolve(arg) for arg in inspect.signature(factory).parameters})
            if inspect.isgenerator(result):
                stack.callback(next, result, None)
                result = next(result)
            values[name] = result
        else:
            raise LookupError(f"fixture {name!r} not found")
        return values[name]

    with stack:
        for name, factory in fixtures.items():
            if factory._fixture.autouse:
                resolve(name)
        function(**{name: resolve(name) for name in inspect.signature(function).parameters})


def isolate_environment() -> pathlib.Path:
    """Point the app's state at a throwaway directory before anything imports it.

    `conftest.py` does the same and more (per-test directories, module
    constants); this is the floor that holds even if conftest failed to load.
    """
    root = pathlib.Path(tempfile.mkdtemp(prefix="cognitive-run-tests-"))
    os.environ["COGNITIVE_STATE_DIR"] = str(root)
    os.environ["XDG_STATE_HOME"] = str(root / "xdg-state")
    for variable, filename in (("COGNITIVE_RECORD_DB", "records.sqlite3"),
                               ("COGNITIVE_EVENT_DB", "events.sqlite3"),
                               ("COGNITIVE_NOTES_DB", "notes.sqlite3"),
                               ("COGNITIVE_UNIVERSE_DB", "universe.sqlite3")):
        os.environ[variable] = str(root / filename)
    return root


def main(argv: list[str]) -> int:
    isolate_environment()
    sys.modules["pytest"] = pytest_stub
    sys.path.insert(0, str(ROOT / "src"))
    sys.path.insert(0, str(TESTS))
    shared = fixtures_of(load(TESTS / "conftest.py", "conftest"))
    selected = set(argv) or None
    failures: list[str] = []
    passed = skipped = 0
    for path in sorted(TESTS.glob("test_*.py")):
        if selected and path.stem not in selected:
            continue
        module = load(path)
        fixtures = {**shared, **fixtures_of(module)}
        for name in sorted(vars(module)):
            function = getattr(module, name)
            if not name.startswith("test_") or not callable(function) or hasattr(function, "_fixture"):
                continue
            for label, params in cases(function):
                test = f"{path.stem}.{name}{label}"
                saved = dict(os.environ)
                try:
                    run_one(function, params, fixtures)
                    passed += 1
                    print(f"PASS {test}")
                except pytest_stub.Skipped as reason:
                    skipped += 1
                    print(f"SKIP {test} ({reason})")
                except (Exception, pytest_stub.Failed):
                    failures.append(test)
                    print(f"FAIL {test}")
                    traceback.print_exc()
                finally:
                    os.environ.clear()
                    os.environ.update(saved)
    summary = f"{passed} passed, {skipped} skipped, {len(failures)} failed"
    if failures:
        summary += f": {', '.join(failures)}"
    print()
    print(summary)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
