"""Shared fixtures for the suite.

`desktop` imports `desktop.py` with a stub GTK so window construction and click
handling can be checked without a display.

`isolate_state` keeps test artifacts out of application state. Modules that
set their own state paths keep them; the rest are pointed at a throwaway
directory here.
"""
import importlib.util
import os
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace

import pytest

from cognitive_popups import popup_helper


@pytest.fixture(autouse=True)
def isolate_state(tmp_path, monkeypatch):
    """Redirect the app's state to `tmp_path` unless the module chose its own.

    Guarded rather than unconditional: a module that already points the stores at
    its own `tmp_path` keeps exactly the behaviour it had.
    """
    if os.environ.get("COGNITIVE_STATE_DIR") or os.environ.get("COGNITIVE_RECORD_DB"):
        return
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    for variable, filename in (("COGNITIVE_RECORD_DB", "records.sqlite3"),
                              ("COGNITIVE_EVENT_DB", "events.sqlite3"),
                              ("COGNITIVE_NOTES_DB", "notes.sqlite3")):
        monkeypatch.setenv(variable, str(tmp_path / filename))


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
