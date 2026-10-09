"""The 15-minute step (Alt+I): the reader's unit of study, its end and its session."""
import json
from types import SimpleNamespace

import pytest

from cognitive_popups import steps
from cognitive_popups.records import RecordStore


@pytest.mark.parametrize("raw,expected", [
    ("разберу VLOOKUP", (15, "разберу VLOOKUP")),
    ("25 разберу VLOOKUP", (25, "разберу VLOOKUP")),
    ("10 мин: перечитаю абзац", (10, "перечитаю абзац")),
    ("  25   минут  объясню  X ", (25, "объясню X")),
    ("3 задачи решу", (3, "задачи решу")),
    ("2026", (15, "2026")),
    ("500 решу", (15, "500 решу")),
])
def test_parse_step_reads_minutes_only_in_front(raw, expected):
    assert steps.parse_step(raw) == expected


def test_step_seconds_has_a_test_override(monkeypatch):
    monkeypatch.delenv(steps.ENV_SECONDS, raising=False)
    assert steps.step_seconds(15) == 900
    monkeypatch.setenv(steps.ENV_SECONDS, "60")
    assert steps.step_seconds(15) == 60


def test_session_boundary():
    step = {"finished_epoch": 1000.0, "deadline_epoch": 900.0}
    assert steps.is_new_session(None, None)
    assert steps.is_new_session(step, {"session_key": "s"}, now=1001)
    assert not steps.is_new_session(step, None, now=1000 + 60)
    assert steps.is_new_session(step, None, now=1000 + steps.SESSION_GAP_SECONDS + 1)


def test_bridge_uses_only_the_readers_own_lines():
    last = {"text": "разберу VLOOKUP", "takeaway": "3 = номер столбца", "finished_utc": "2026-10-07T10:00:00+00:00"}
    closure = {"remaining": "XLOOKUP не разобран", "closed_utc": "2026-10-06T20:00:00+00:00"}
    assert steps.bridge(last, closure, new_session=False).text == "3 = номер столбца"
    assert steps.bridge(last, closure, new_session=True).text == "XLOOKUP не разобран"
    assert steps.bridge(last, None, new_session=True).text == "3 = номер столбца"
    assert steps.bridge({"text": "x"}, None, new_session=False) is None


def test_session_summary():
    rows = [{"text": "разберу VLOOKUP", "outcome": "done", "takeaway": "3 = номер столбца"},
            {"text": "решу задачу", "outcome": "partly", "takeaway": None}]
    notes = [SimpleNamespace(comment="как телефонная книга")]
    assert steps.session_summary("стажировка: Excel", rows, notes) == (
        "Ради чего: стажировка: Excel\n\n✓ разберу VLOOKUP\n   → 3 = номер столбца\n"
        "½ решу задачу\n\nТвои заметки:\n✎ как телефонная книга")


def test_end_prompt():
    assert steps.end_prompt("разберу X", 15).startswith("15 минут. Сделал шаг?")
    assert steps.end_prompt("разберу X", 1).startswith("1 минута.")
    assert steps.end_prompt("разберу X", 15, early=True).startswith("Закончить шаг сейчас?")


def test_records_keep_steps_and_closures(tmp_path):
    records = RecordStore(tmp_path / "records.sqlite3")
    records.add_step("a", "разберу  X", session_key="s", minutes=15, started_epoch=100, deadline_epoch=1000)
    assert records.last_step()["text"] == "разберу X"
    assert records.finish_step("a", "done", "вынес  Y")
    assert records.step("a")["takeaway"] == "вынес Y"
    assert [row["id"] for row in records.session_steps("s")] == ["a"]
    records.close_study_session("s", outcome="partly", remaining="осталось Z")
    assert records.closure("s")["remaining"] == "осталось Z"
    assert records.last_closure()["outcome"] == "partly"


# ── the desktop flow, with windows answered by a script ──────────────────────

class _Sync:
    def __init__(self, target, **kwargs):
        self.target = target

    def start(self):
        self.target()


def _app(desktop, monkeypatch, tmp_path, answers, choices):
    from cognitive_popups.focus import Focus
    records = RecordStore(tmp_path / "records.sqlite3")
    monkeypatch.setattr(desktop, "RECORDS", records)
    monkeypatch.setattr(desktop, "threading", SimpleNamespace(Thread=_Sync))
    asked = []

    def popup(prompt, title, *args, **kwargs):
        asked.append((prompt, kwargs.get("quote", "")))
        return answers.pop(0)
    monkeypatch.setattr(desktop, "popup_input", popup)
    shown = []

    def run_popup(payload, **kwargs):
        shown.append(payload["text"])
        return json.dumps({"action": choices.pop(0)}) if choices else ""
    monkeypatch.setattr(desktop, "run_popup", run_popup)
    played = []
    monkeypatch.setattr(desktop.sound, "play", played.append)
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.focus = Focus(tmp_path / "focus.json", records, None)
    app._focus_prompting = False
    app.focus_timer = None
    app._show_step_timer = lambda state: False
    app._close_focus_timer = lambda block_id: False
    app.flash = SimpleNamespace(show_text=lambda *a, **k: shown.append(a[0]))
    return app, records, asked, shown, played


def test_the_step_starts_without_a_goal_and_an_empty_step_is_reading(desktop, monkeypatch, tmp_path):
    app, records, asked, _, _ = _app(desktop, monkeypatch, tmp_path, ["25 разберу VLOOKUP"], [])
    app.start_step()
    assert [prompt for prompt, _ in asked] == [desktop.STEP_PROMPT]   # no goal question up front
    state = app.focus.snapshot()
    assert state["goal"] == "разберу VLOOKUP" and state["deadline"] - state["started_at"] == 25 * 60
    assert records.last_step()["goal_id"] is None
    (tmp_path / "b").mkdir()
    app2, _, _, _, _ = _app(desktop, monkeypatch, tmp_path / "b", [""], [])
    app2.start_step()
    assert app2.focus.snapshot()["goal"] == "читаю"


def test_step_end_marks_bridges_and_closes_the_session(desktop, monkeypatch, tmp_path):
    app, records, asked, shown, played = _app(
        desktop, monkeypatch, tmp_path,
        ["разберу VLOOKUP", "3 = номер столбца", "XLOOKUP", "", "стажировка", "осталось: XLOOKUP"],
        ["done", "next", "partly", "close", "partly"])
    app.start_step()
    first = app.focus.snapshot()["id"]

    app._step_end(first)            # done -> takeaway -> next step (bridge shown)
    second = app.focus.snapshot()["id"]
    assert second != first
    assert asked[2] == (desktop.STEP_PROMPT, "3 = номер столбца")
    app._step_end(second)           # partly -> skipped takeaway -> close session

    assert records.step(first)["outcome"] == "done" and records.step(first)["takeaway"] == "3 = номер столбца"
    assert records.step(second)["outcome"] == "partly"
    closure = records.closure(records.step(first)["session_key"])
    assert closure["outcome"] == "partly" and closure["remaining"] == "осталось: XLOOKUP"
    assert any("Ради чего: стажировка" in text and "✓ разберу VLOOKUP" in text for text in shown)
    assert played == ["step_done", "session_close"]


def test_closing_the_end_window_keeps_the_step_open_for_alt_i(desktop, monkeypatch, tmp_path):
    app, records, asked, shown, played = _app(desktop, monkeypatch, tmp_path,
                                              ["разберу X", "вынес Y"], ["", "done", "break"])
    app.start_step()
    block = app.focus.snapshot()["id"]
    app._step_end(block)            # the deadline window closed without a mark
    assert records.step(block)["outcome"] is None
    assert app.focus.snapshot()["status"] == "running"
    monkeypatch.setattr(desktop.time, "time", lambda: app.focus.snapshot()["deadline"] + 5)
    app.start_step()                # Alt+I after the deadline: the same end window again
    assert shown[-2].startswith("15 минут. Сделал шаг?")
    assert records.step(block)["outcome"] == "done" and records.step(block)["takeaway"] == "вынес Y"
    assert played == ["step_done"]


def test_second_press_ends_early_or_continues(desktop, monkeypatch, tmp_path):
    app, records, _, _, _ = _app(desktop, monkeypatch, tmp_path, ["разберу X"], ["continue"])
    app.start_step()
    block = app.focus.snapshot()["id"]
    app.start_step()                # second Alt+I -> early end window -> "continue"
    assert app.focus.snapshot()["status"] == "running"
    assert records.step(block)["outcome"] is None


def test_every_window_carries_the_running_step(desktop, monkeypatch, tmp_path):
    focus_file = desktop.STATE_DIR / "focus.json"
    focus_file.write_text(json.dumps({"id": "a", "status": "running", "goal": "разберу X",
                                      "started_at": 0, "deadline": 1000}), encoding="utf-8")
    assert desktop.active_step_line(now=1000 - 577) == "разберу X · 9:37"
    assert desktop.active_step_line(now=1001) == ""
    monkeypatch.setattr(desktop, "active_step_line", lambda: "разберу X · 9:37")
    desktop.begin_interaction("hotkey", window="clarify")
    assert desktop.prepare_popup({"mode": "input"}, "input")["step_line"] == "разберу X · 9:37"
    focus_file.write_text(json.dumps({"id": "a", "status": "completed", "goal": "x",
                                      "started_at": 0, "deadline": 10**12}), encoding="utf-8")
    monkeypatch.undo()
    assert desktop.active_step_line() == ""


def test_cursor_is_converted_to_window_pixels_of_a_scaled_monitor(desktop, monkeypatch):
    monkeypatch.setattr(desktop, "monitor_scale_at", lambda x, y: (1.5, 0.0, 0.0))
    assert desktop.to_window_pixels(408, 208) == (612, 312)
    monkeypatch.setattr(desktop, "monitor_scale_at", lambda x, y: (1.0, 0.0, 0.0))
    assert desktop.to_window_pixels(408, 208) == (408, 208)


def test_window_hotkeys_run_off_the_main_loop_and_a_second_press_does_nothing(desktop, monkeypatch):
    import threading
    started, release, calls = threading.Event(), threading.Event(), []
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)

    def slow():
        calls.append("open")
        started.set()
        release.wait(3)
    desktop.begin_interaction("hotkey", window="note")
    app._off_main("note", slow)
    assert started.wait(3)
    app._off_main("note", slow)           # the same window is still open
    app._off_main("clarify", lambda: calls.append("clarify"))   # another key goes straight through
    for _ in range(50):
        if "clarify" in calls:
            break
        threading.Event().wait(.05)
    release.set()
    assert calls.count("open") == 1 and "clarify" in calls
