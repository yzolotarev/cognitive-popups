"""A hotkey pressed again while its own window is open must never stay dead.

12.10: the hypothesis window got lost behind a note; 17 presses of Ctrl+Alt+P
answered "already open" until the reader restarted the app (notes 105, 110).
"""
import subprocess
import sys
import threading
import time


def test_visible_window_is_raised_first(desktop):
    assert desktop.repeat_press(True, None, 100.0) == "raise"
    assert desktop.repeat_press(None, None, 100.0) == "raise"


def test_window_the_compositor_does_not_show_is_replaced(desktop):
    assert desktop.repeat_press(False, None, 100.0) == "replace"


def test_second_press_after_a_raise_replaces(desktop):
    assert desktop.repeat_press(True, 90.0, 100.0) == "replace"
    assert desktop.repeat_press(None, 90.0, 100.0) == "replace"
    assert desktop.repeat_press(True, 50.0, 100.0) == "raise"


def test_repeat_press_raises_then_replaces_the_live_popup(desktop, monkeypatch):
    events, focused = [], []
    monkeypatch.setattr(desktop, "emit", lambda event, **fields: events.append(fields.get("detail")))
    monkeypatch.setattr(desktop, "mapped_window", lambda pid: True)
    monkeypatch.setattr(desktop, "focus_pid", focused.append)
    monkeypatch.setattr(desktop, "idle_add", lambda callback, *args: callback(*args))
    monkeypatch.setattr(desktop, "bind_chain", lambda fn: fn)
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    opened = []

    def action():
        result = desktop._spawn_popup([sys.executable, "-c", "import time; time.sleep(30)"], "", {})
        opened.append(result.returncode)

    app._off_main("prediction", action)
    deadline = time.monotonic() + 5
    while desktop.live_popup("prediction") is None and time.monotonic() < deadline:
        time.sleep(0.02)
    proc = desktop.live_popup("prediction")
    assert proc is not None
    app._off_main("prediction", action)
    assert focused == [proc.pid] and events[-1] == "raised"
    app._off_main("prediction", action)
    assert events[-1] == "stale window replaced"
    deadline = time.monotonic() + 5
    while (not opened or desktop.live_popup("prediction") in (None, proc)) and time.monotonic() < deadline:
        time.sleep(0.02)
    fresh = desktop.live_popup("prediction")
    assert opened and fresh is not None and fresh is not proc
    fresh.kill()
    deadline = time.monotonic() + 5
    while "prediction" in app._windows_running and time.monotonic() < deadline:
        time.sleep(0.02)
    assert "prediction" not in app._windows_running


def test_waiting_for_the_model_is_not_a_dead_key(desktop, monkeypatch):
    events = []
    monkeypatch.setattr(desktop, "emit", lambda event, **fields: events.append(fields.get("detail")))
    monkeypatch.setattr(desktop, "bind_chain", lambda fn: fn)
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    gate = threading.Event()
    app._off_main("clarify", gate.wait)
    app._off_main("clarify", gate.wait)
    assert events == ["already open: waiting"]
    gate.set()
