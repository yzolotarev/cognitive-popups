#!/usr/bin/env python3
"""Run the test suite without pytest.

The suite is plain functions with asserts and no pytest features, so this runner
is enough where pytest is not installed. `python3 -m pytest` works just as well
when it is; a test that needs a fixture is skipped here rather than failed.

    python3 scripts/run-tests.py            # everything
    python3 scripts/run-tests.py test_notes # one module
"""
from __future__ import annotations

import importlib.util
import inspect
import os
import pathlib
import sys
import tempfile
import traceback

ROOT = pathlib.Path(__file__).resolve().parent.parent
TESTS = ROOT / "tests"

# The only fixtures the suite uses. Anything else is skipped rather than failed,
# so an unmet fixture is visible instead of silently passing.
FIXTURES = ("tmp_path",)


def build_kwargs(function) -> dict | None:
    kwargs: dict[str, object] = {}
    for name in inspect.signature(function).parameters:
        if name not in FIXTURES:
            return None
        kwargs[name] = pathlib.Path(tempfile.mkdtemp())
    return kwargs


def load(path: pathlib.Path):
    spec = importlib.util.spec_from_file_location(path.stem, path)
    if spec is None or spec.loader is None:  # pragma: no cover - defensive
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main(argv: list[str]) -> int:
    sys.path.insert(0, str(ROOT / "src"))
    sys.path.insert(0, str(TESTS))
    selected = set(argv) or None
    failures: list[str] = []
    ran = 0
    for path in sorted(TESTS.glob("test_*.py")):
        if selected and path.stem not in selected:
            continue
        module = load(path)
        for name in sorted(vars(module)):
            if not name.startswith("test_") or not callable(getattr(module, name)):
                continue
            function = getattr(module, name)
            kwargs = build_kwargs(function)
            if kwargs is None:
                print(f"SKIP {path.stem}.{name} (needs a fixture: run under pytest)")
                continue
            saved = dict(os.environ)
            ran += 1
            try:
                function(**kwargs)
                print(f"PASS {path.stem}.{name}")
            except Exception:
                failures.append(f"{path.stem}.{name}")
                print(f"FAIL {path.stem}.{name}")
                traceback.print_exc()
            finally:
                os.environ.clear()
                os.environ.update(saved)
    summary = f"{ran - len(failures)}/{ran} passed"
    if failures:
        summary += f", failures: {', '.join(failures)}"
    print()
    print(summary)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
