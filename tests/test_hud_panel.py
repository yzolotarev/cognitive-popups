"""The daemon's end of the panel: what it accepts, what it refuses, what it shows.

`desktop` (see conftest) imports desktop.py against a stub GTK, so a click can be
followed without a display, and `isolate_state` keeps the ledger in `tmp_path`.
"""
from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from cognitive_popups import hud_ipc


@pytest.fixture
def hud(desktop, monkeypatch):
    # Import the resident panel without requiring a display or PyGObject.
    monkeypatch.setattr(sys.modules["gi.repository"].Gtk, "Box", object, raising=False)
    path = Path(desktop.__file__).with_name("hud.py")
    spec = importlib.util.spec_from_file_location("cognitive_popups._test_hud", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class _Button:
    def __init__(self):
        self.tooltip = ""
        self.classes = set()

    def get_style_context(self):
        return self

    def add_class(self, name):
        self.classes.add(name)

    def remove_class(self, name):
        self.classes.discard(name)

    def set_tooltip_text(self, text):
        self.tooltip = text


def _app(desktop, monkeypatch):
    """A DesktopApp without a window, a client or a pid file."""
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app._busy = False
    app.flash = SimpleNamespace(
        show_text=lambda *args, **kwargs: None,
        show_cues=lambda *args, **kwargs: None,
    )
    app.service = SimpleNamespace(session=SimpleNamespace(id="s1", fragments=[]))
    # `__init__` is bypassed, so the pieces it builds are stood in for here.
    app.client = SimpleNamespace(model="test-model")
    app.preparer = SimpleNamespace(submit=lambda context: None)
    monkeypatch.setattr(app, "_split_idle_session", lambda: None)
    return app


def _save_task(desktop, task_id: str = "t1", **overrides) -> None:
    fields = dict(
        task_id=task_id,
        session_id="s1",
        task_type="apply",
        level="базовый",
        context="материал сессии",
        condition="Первый случай.\nВторой случай.",
        status="generated",
    )
    fields.update(overrides)
    desktop.RECORDS.save_task(**fields)


# ── the action table ─────────────────────────────────────────────────────────


def test_hud_captions_match_existing_shortcuts(hud):
    captions = {action: caption for action, _icon, caption, _tip in hud.BUTTONS}
    assert captions == {
        "goal": "Alt+I", "four_words": "Alt+W", "example": "Alt+G",
        "note": "Alt+E", "practice": "Alt+T", "menu": "Ещё", "background": "Фон",
    }
    assert all(icon for _action, icon, _caption, _tip in hud.BUTTONS)


def test_practice_caption_and_tooltip_follow_its_click_target(hud):
    panel = hud.HudPanel.__new__(hud.HudPanel)
    panel.state = {"practice": None}
    panel.dismissed_locally = set()
    panel.reachable = True
    button = _Button()
    captions = []
    panel.buttons = {"practice": button}
    panel.key_labels = {"practice": SimpleNamespace(set_text=captions.append)}
    panel._apply_scale = lambda _opportunity: None
    panel._apply_background = lambda: None
    panel._set_status = lambda _text: None

    panel._apply_state()
    assert captions[-1] == "Alt+T"
    assert "hud-ready" not in button.classes
    assert "Alt+T" in button.tooltip
    panel.state = {"practice": {"task_id": "highlighted", "summary": "Задача"}}
    panel._apply_state()
    assert captions[-1] == "Задача"
    assert "hud-ready" in button.classes
    assert "Alt+T" not in button.tooltip
    assert "Alt+Y" in button.tooltip
    assert "последнюю" in button.tooltip
    assert "может быть другой" in button.tooltip

    panel.dismissed_locally.add("highlighted")
    panel._apply_state()
    assert captions[-1] == "Alt+T"
    assert "hud-ready" not in button.classes


def test_practice_click_keeps_menu_and_exact_task_separate(hud):
    panel = hud.HudPanel.__new__(hud.HudPanel)
    panel.dismissed_locally = set()
    dispatched = []
    panel._dispatch = lambda action, task_id="": dispatched.append((action, task_id))
    panel.state = {"practice": None}
    panel._practice(attempt=True)
    panel.state = {"practice": {"task_id": "highlighted"}}
    panel._practice(attempt=True)
    assert dispatched == [("task_menu", ""), ("task_practice", "highlighted")]


def test_every_mapped_action_names_a_real_method(desktop):
    for name in desktop.HUD_ACTIONS.values():
        assert callable(getattr(desktop.DesktopApp, name, None)), name


def test_every_panel_action_is_known(desktop):
    for name in list(desktop.HUD_ACTIONS) + list(desktop.HUD_TASK_ACTIONS):
        assert name in desktop.HUD_ACTIONS or name in desktop.HUD_TASK_ACTIONS


# ── dispatch ─────────────────────────────────────────────────────────────────

def test_dispatch_reaches_the_method_a_hotkey_uses(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    seen: list[str] = []
    monkeypatch.setattr(app, "seed", lambda: seen.append("seed"))

    assert app.dispatch_action("four_words") is True
    assert seen == ["seed"]


def test_dispatch_refuses_an_unknown_action(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)

    assert app.dispatch_action("rm -rf /") is False


def test_practice_carries_the_task_id_the_panel_was_showing(desktop, monkeypatch, tmp_path):
    script = tmp_path / "cognitive-tasks.sh"
    script.write_text("#!/bin/sh\n", encoding="utf-8")
    monkeypatch.setattr(desktop, "TASKS_SCRIPT", script)
    app = _app(desktop, monkeypatch)
    spawned: list[list[str]] = []
    monkeypatch.setattr(desktop.subprocess, "Popen",
                        lambda argv, **kwargs: spawned.append(argv) or None)

    assert app.dispatch_action("task_practice", task_id="task-abc") is True
    assert spawned == [[str(script), "--practice", "task-abc"]]


def test_practice_without_a_task_id_opens_nothing(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    spawned: list[list[str]] = []
    monkeypatch.setattr(desktop.subprocess, "Popen",
                        lambda argv, **kwargs: spawned.append(argv) or None)

    assert app.dispatch_action("task_practice") is False
    assert spawned == []


def test_the_task_menu_reaches_the_insert_script(desktop, monkeypatch, tmp_path):
    script = tmp_path / "cognitive-tasks.sh"
    script.write_text("#!/bin/sh\n", encoding="utf-8")
    monkeypatch.setattr(desktop, "TASKS_SCRIPT", script)
    app = _app(desktop, monkeypatch)
    spawned: list[list[str]] = []
    monkeypatch.setattr(desktop.subprocess, "Popen",
                        lambda argv, **kwargs: spawned.append(argv) or None)

    assert app.dispatch_action("task_menu") is True
    assert spawned == [[str(script), "--menu"]]


def test_a_missing_tasks_script_is_reported_not_raised(desktop, monkeypatch, tmp_path):
    monkeypatch.setattr(desktop, "TASKS_SCRIPT", tmp_path / "absent.sh")
    app = _app(desktop, monkeypatch)

    assert app.dispatch_action("task_menu") is False


# ── requests over the socket ─────────────────────────────────────────────────

def test_ping_answers_without_reading_the_ledger(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)

    response = app.hud_request({"command": "ping"})

    assert response["ok"] is True
    assert response["status"] == "ready"


def test_state_reports_the_goal_the_buffer_and_the_offered_task(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    app.service = SimpleNamespace(session=SimpleNamespace(id="s1", fragments=[1, 2]))
    desktop.RECORDS.save_intention("различать случаи", criterion="назову два примера")
    _save_task(desktop)

    state = app.hud_request({"command": "state"})

    assert state["ok"] is True
    assert state["buffer"] == 2
    assert state["goal"]["text"] == "различать случаи"
    assert state["goal"]["criterion"] == "назову два примера"
    assert state["practice"]["task_id"] == "t1"
    assert state["practice"]["summary"] == "Первый случай."
    assert state["practice"]["from_current_session"] is True


def test_state_without_a_goal_or_a_task_offers_nothing(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)

    state = app.hud_request({"command": "state"})

    assert state["goal"] is None
    assert state["practice"] is None


def test_only_practice_buttons_read_as_a_ready_highlight(desktop, monkeypatch):
    """The highlight is a fact about the ledger, not about the reader."""
    app = _app(desktop, monkeypatch)
    assert app.hud_request({"command": "state"})["practice"] is None

    _save_task(desktop)
    assert app.hud_request({"command": "state"})["practice"] is not None

    desktop.RECORDS.save_task(
        task_id="t1", session_id="s1", task_type="apply", level="базовый",
        context="материал сессии", condition="Первый случай.", attempt="моя попытка",
        status="attempt_checked",
    )
    assert app.hud_request({"command": "state"})["practice"] is None


def test_dismiss_stops_the_highlight(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    _save_task(desktop)
    assert app.hud_request({"command": "state"})["practice"] is not None

    assert app.hud_request({"command": "dismiss", "task_id": "t1"})["ok"] is True

    # With the stub GLib, idle_add runs the write inline.
    assert app.hud_request({"command": "state"})["practice"] is None


def test_dismiss_without_a_task_id_is_refused(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)

    response = app.hud_request({"command": "dismiss", "task_id": ""})

    assert response["ok"] is False


def test_an_action_request_runs_the_mapped_method(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    seen: list[str] = []
    monkeypatch.setattr(app, "seed", lambda: seen.append("seed"))

    response = app.hud_request({"command": "action", "action": "four_words"})

    assert response["ok"] is True
    assert response["accepted"] is True
    assert seen == ["seed"]


def test_an_unknown_action_never_reaches_a_method(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)

    response = app.hud_request({"command": "action", "action": "seed; rm -rf /"})

    assert response["ok"] is False
    assert "unknown action" in response["error"]


def test_an_unknown_command_is_refused(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)

    assert app.hud_request({"command": "eval"})["ok"] is False


# ── background preparation as the panel sees it ──────────────────────────────

def test_the_background_switch_is_reported_and_changeable(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    assert app.hud_request({"command": "state"})["settings"]["prepare_in_background"] is False

    response = app.hud_request(
        {"command": "settings", "key": "prepare_in_background", "value": True}
    )

    assert response["ok"] is True
    assert response["settings"]["prepare_in_background"] is True
    # With the stub GLib the row is written inline; the next read must see it.
    assert app.hud_request({"command": "state"})["settings"]["prepare_in_background"] is True


def test_an_unknown_setting_is_refused(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)

    response = app.hud_request({"command": "settings", "key": "auto_practice", "value": True})

    assert response["ok"] is False
    assert "unknown setting" in response["error"]


def test_state_reports_how_far_the_preparation_has_come(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    desktop.RECORDS.save_preparation(
        preparation_id="p1",
        context_key="ctx-1",
        status="generating",
        completed_stage=2,
        stage_label="Готовлю задачу",
        analysis_json=json.dumps({"target": "различить два случая", "status": "candidate"}),
    )

    preparation = app.hud_request({"command": "state"})["preparation"]

    assert preparation["status"] == "generating"
    assert preparation["completed_stage"] == 2
    assert preparation["stage_count"] == len(desktop.practice.STAGES)
    assert preparation["working"] is True
    assert preparation["target"] == "различить два случая"


def test_no_preparation_means_no_scale_in_the_state(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)

    assert app.hud_request({"command": "state"})["preparation"] is None


def test_a_task_from_an_unfinished_preparation_is_not_offered(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    _save_task(desktop, "t1")
    desktop.RECORDS.save_preparation(
        preparation_id="p1", context_key="ctx-1", status="failed",
        completed_stage=2, stage_label="", task_id="t1",
    )

    state = app.hud_request({"command": "state"})

    assert state["practice"] is None


def test_turning_the_background_switch_on_uses_material_already_in_hand(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    app.service = SimpleNamespace(session=SimpleNamespace(id="s1", fragments=[
        SimpleNamespace(id="f1", source_text="однородная система и её свойства",
                        source_hash="h1"),
    ]))
    submitted: list = []
    app.preparer = SimpleNamespace(submit=lambda context: submitted.append(context))
    desktop.RECORDS.save_intention("различать случаи")

    app.hud_request({"command": "settings", "key": "prepare_in_background", "value": True})

    # Switching it on with material in hand has to do something visible.
    assert len(submitted) == 1
    assert submitted[0].source_text == "однородная система и её свойства"
    assert submitted[0].goal_text == "различать случаи"


def test_the_fragment_that_was_saved_is_what_gets_prepared(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    submitted: list = []
    app.preparer = SimpleNamespace(submit=lambda context: submitted.append(context))
    monkeypatch.setattr(desktop.RECORDS, "save_fragment", lambda *args, **kwargs: None)
    desktop.hud_state.set_setting("prepare_in_background", True)

    fragment = SimpleNamespace(id="f9", source_text="материал про однородную систему", source_hash="h9")
    app._commit_fragment(fragment)

    assert len(submitted) == 1
    assert submitted[0].source_hash == "h9"


def test_nothing_is_prepared_while_the_switch_is_off(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    submitted: list = []
    app.preparer = SimpleNamespace(submit=lambda context: submitted.append(context))
    saved: list = []
    monkeypatch.setattr(desktop.RECORDS, "save_fragment", lambda *args, **kwargs: saved.append(args))

    app._commit_fragment(SimpleNamespace(id="f9", source_text="материал", source_hash="h9"))

    assert saved  # the fragment itself is always recorded
    assert submitted == []


# ── the socket the daemon publishes ──────────────────────────────────────────

def test_the_published_socket_answers_and_is_then_removed(desktop, monkeypatch):
    app = _app(desktop, monkeypatch)
    server = app._start_panel_socket()
    assert server is not None
    try:
        state = hud_ipc.send({"command": "state"}, path=server.path)
    finally:
        server.stop()

    assert state["ok"] is True
    assert not server.path.exists()
