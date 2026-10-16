"""Shared fixtures for the suite.

`desktop` imports `desktop.py` with a stub GTK so window construction and click
handling can be checked without a display.

State isolation is unconditional. A test run must never write the reader's real
state, even when the shell exported `COGNITIVE_STATE_DIR` (the systemd units do,
and people copy that into their profile). Two layers:

- at import, before any `cognitive_popups` module computes a path constant, the
  environment is pointed at a throwaway directory for the whole run;
- `isolate_state` then gives every test its own `tmp_path` and rewrites every
  module-level path constant that still points at a real state root.

A test that needs a different path sets it itself, after this fixture.
"""
import importlib.util
import os
import sys
import tempfile
from pathlib import Path
from types import ModuleType, SimpleNamespace

#: Roots a test must never write to: whatever the shell pointed at, the XDG
#: default and the checkout's `var/`. Captured before the environment changes.
REAL_STATE_ROOTS = tuple(sorted({
    str(Path(raw).expanduser().resolve())
    for raw in (os.environ.get("COGNITIVE_STATE_DIR"),
                os.path.join(os.environ.get("XDG_STATE_HOME") or "~/.local/state", "cognitive-popups"),
                "~/.local/state/cognitive-popups",
                str(Path(__file__).resolve().parent.parent / "var"))
    if raw
}, key=len, reverse=True))

_STATE_VARIABLES = (("COGNITIVE_RECORD_DB", "records.sqlite3"),
                    ("COGNITIVE_EVENT_DB", "events.sqlite3"),
                    ("COGNITIVE_NOTES_DB", "notes.sqlite3"),
                    ("COGNITIVE_UNIVERSE_DB", "universe.sqlite3"))


def _point_environment_at(root: Path, setter) -> None:
    setter("COGNITIVE_STATE_DIR", str(root))
    setter("XDG_STATE_HOME", str(root / "xdg-state"))
    for variable, filename in _STATE_VARIABLES:
        setter(variable, str(root / filename))
    setter("COGNITIVE_PROMPTS_PATH", str(root / "prompts.json"))
    setter("COGNITIVE_REQUEST_GATE_DIR", str(root))


SESSION_STATE = Path(tempfile.mkdtemp(prefix="cognitive-test-state-"))
_point_environment_at(SESSION_STATE, os.environ.__setitem__)

import pytest  # noqa: E402

from cognitive_popups import popup_helper  # noqa: E402


def _redirect(value, roots: tuple[str, ...], target: Path):
    """`value` moved under `target` when it lives under one of `roots`, else None."""
    if not isinstance(value, (str, Path)) or not str(value):
        return None
    try:
        resolved = str(Path(value).expanduser().resolve())
    except (OSError, RuntimeError, ValueError):
        return None
    for root in roots:
        if resolved == root or resolved.startswith(root + os.sep):
            moved = target / os.path.relpath(resolved, root)
            return moved if isinstance(value, Path) else str(moved)
    return None


def redirect_module_constants(target: Path, setter) -> list[str]:
    """Rewrite path constants frozen at import that point at a real state root."""
    roots = REAL_STATE_ROOTS + (str(SESSION_STATE.resolve()),)
    changed = []
    for name, module in list(sys.modules.items()):
        if not name.startswith("cognitive_popups") or module is None:
            continue
        for attribute, value in list(vars(module).items()):
            if not attribute.isupper():
                continue
            moved = _redirect(value, roots, target)
            if moved is not None and moved != value:
                setter(module, attribute, moved)
                changed.append(f"{name}.{attribute}")
    return changed


@pytest.fixture(autouse=True)
def isolate_state(tmp_path, monkeypatch):
    """Point every state path at `tmp_path`, unconditionally (see module docstring)."""
    _point_environment_at(tmp_path, monkeypatch.setenv)
    redirect_module_constants(tmp_path, monkeypatch.setattr)


@pytest.fixture
def desktop(monkeypatch):
    gi = ModuleType("gi")
    gi.require_version = lambda *args: None
    repo = ModuleType("gi.repository")
    repo.Gtk = SimpleNamespace(Window=object)
    repo.Gdk = SimpleNamespace()
    repo.GLib = SimpleNamespace(idle_add=lambda callback, *args: callback(*args))
    monkeypatch.setitem(sys.modules, "gi", gi)
    monkeypatch.setitem(sys.modules, "gi.repository", repo)
    path = Path(popup_helper.__file__).with_name("desktop.py")
    spec = importlib.util.spec_from_file_location("cognitive_popups._ui_desktop", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module
