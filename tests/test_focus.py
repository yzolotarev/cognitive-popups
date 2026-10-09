import json
import threading
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest

from cognitive_popups.focus import CHECKER, GENERATOR, Focus, clean_sources, collect_sources


def fragment(id, stamp, text="A causes B."):
    return SimpleNamespace(id=id, created_at=datetime.fromtimestamp(stamp, timezone.utc).isoformat(), source_text=text)


class Records:
    def __init__(self):
        self.data = {"previous": [fragment("old", 99), fragment("a", 100)],
                     "current": [fragment("b", 500, "B enables C."), fragment("future", 1001)]}

    def sessions(self):
        return [{"id": key} for key in self.data]

    def load_fragments(self, id):
        return self.data[id]


READY = {"status": "ready", "fragment_ids": ["a", "b"], "question": "Why B?",
         "targets": ["A causes B"], "rubric": "Explain causality",
         "evidence": "A causes B.", "evidence_quality": "direct"}
CHECK = {"status": "passed", "gaps": [], "text": "The relationship is explained."}


class Client:
    def __init__(self, *answers):
        self.answers = list(answers or (READY, CHECK))
        self.calls = []

    def complete(self, messages, **kwargs):
        self.calls.append(json.loads(messages[-1]["content"]))
        answer = self.answers.pop(0)
        if isinstance(answer, Exception):
            raise answer
        return json.dumps(answer)


@pytest.fixture
def setup(tmp_path):
    now = [100.0]
    client, records = Client(), Records()
    flow = Focus(tmp_path / "focus.json", records, client, clock=lambda: now[0])
    return flow, now, client, records


def test_raw_goal_atomic_and_validation(setup):
    flow, now, client, records = setup
    with pytest.raises(ValueError):
        flow.start(" \n ")
    state = flow.start("  learn\ncausality  ")
    assert state["goal"] == "  learn\ncausality  "
    assert state["deadline"] == 1000
    assert json.loads(flow.path.read_text())["goal"] == state["goal"]
    assert not flow.path.with_name("focus.json.tmp").exists()
    with pytest.raises(ValueError):
        flow.start("another")
    assert flow.generate(state["id"]) is None
    assert client.calls == []


def test_all_sessions_time_filter():
    assert [f["id"] for f in collect_sources(Records(), 100, 1000)] == ["a", "b"]


def test_two_calls_and_frozen_snapshot(setup):
    flow, now, client, records = setup
    state = flow.start("goal")
    now[0] = 1000
    ready = flow.generate(state["id"])
    assert ready["status"] == "ready"
    records.data.clear()
    assert flow.generate(state["id"]) is None
    done = flow.check(state["id"], "  because A  ")
    assert done["status"] == "completed"
    assert len(client.calls) == 2
    assert client.calls[1]["snapshot"] == {"sources": ready["selected_sources"]}
    assert ready["selected_sources"] == [
        {"id": f["id"], "source_text": f["source_text"]} for f in client.calls[0]["sources"]]
    assert "goal" not in client.calls[1]["snapshot"]
    assert client.calls[1]["explanation"] == "  because A  "
    assert flow.check(state["id"], "again") is None
    assert flow.snapshot()["explanation"] == "  because A  "
    # Returned copies cannot mutate persisted state.
    ready["frozen"]["sources"].clear()
    assert len(flow.snapshot()["frozen"]["sources"]) == 2


@pytest.mark.parametrize("answer", [
    {"status": "ready", **{k: v for k, v in READY.items() if k != "question"}},
    {**READY, "fragment_ids": ["made-up"]},
    {**READY, "evidence": "invented"},
    {**READY, "targets": []},
    RuntimeError("offline"),
])
def test_generation_failure_is_terminal(setup, answer):
    flow, now, client, records = setup
    client.answers = [answer]
    state = flow.start("goal")
    now[0] = 1000
    with pytest.raises((ValueError, RuntimeError)):
        flow.generate(state["id"])
    assert flow.snapshot()["status"] == "failed"
    assert flow.generate(state["id"]) is None
    assert len(client.calls) == 1


def test_insufficient_context_single_call(setup):
    flow, now, client, records = setup
    records.data.clear()
    client.answers = [{"status": "insufficient_context"}]
    state = flow.start("goal")
    now[0] = 1000
    assert flow.generate(state["id"])["status"] == "insufficient_context"
    assert len(client.calls) == 1
    assert flow.check(state["id"], "answer") is None


@pytest.mark.parametrize("phase", ["running", "generating", "ready", "checking"])
def test_crash_resume(setup, phase):
    flow, now, client, records = setup
    state = flow.start("goal")
    with flow.lock:
        flow._save({**state, "status": phase})
    resumed = Focus(flow.path, records, client, clock=lambda: now[0])
    assert resumed.snapshot()["status"] == ("running" if phase == "running" else "interrupted")
    assert not client.calls
    now[0] = 1001
    assert Focus(flow.path, records, client, clock=lambda: now[0]).snapshot()["status"] == "interrupted"


@pytest.mark.parametrize("stage", ["generate", "check"])
def test_stale_worker_cannot_overwrite_new_block(setup, stage):
    flow, now, client, records = setup
    old = flow.start("old")
    now[0] = 1000
    if stage == "check":
        flow.generate(old["id"])
    entered, release = threading.Event(), threading.Event()
    original = client.complete

    def blocking(*args, **kwargs):
        entered.set()
        assert release.wait(3)
        return original(*args, **kwargs)

    client.complete = blocking
    results = []
    thread = threading.Thread(target=lambda: results.append(
        flow.generate(old["id"]) if stage == "generate" else flow.check(old["id"], "answer")))
    thread.start()
    assert entered.wait(3)
    assert flow.finish(old["id"])
    new = flow.start("new")
    release.set()
    thread.join(3)
    assert not thread.is_alive()
    assert results == [None]
    assert flow.snapshot() == new


def test_max_two_gaps(setup):
    flow, now, client, records = setup
    state = flow.start("goal")
    now[0] = 1000
    flow.generate(state["id"])
    client.answers = [{**CHECK, "gaps": ["a", "b", "c"]}]
    with pytest.raises(ValueError):
        flow.check(state["id"], "answer")
    assert flow.snapshot()["status"] == "failed"


def test_signal_dispatch(desktop):
    app = object.__new__(desktop.DesktopApp)
    calls = []
    # The retired Ctrl+Alt+G request lands on the 15-minute step (Alt+I).
    app.start_step = lambda: calls.append("step")
    desktop.take_request = lambda: "focus"
    assert app._signal_menu()
    assert calls == ["step"]
    assert desktop.HUD_ACTIONS["focus"] == "start_step"


def test_actual_record_store_session_boundary(tmp_path):
    from cognitive_popups.records import RecordStore
    from cognitive_popups.models import Fragment
    records = RecordStore(tmp_path / "records.sqlite3")
    for session_id, id, stamp in [("closed", "a", 100), ("closed", "old", 99),
                                  ("current", "b", 500), ("current", "late", 1001)]:
        session = SimpleNamespace(id=session_id, title=session_id, fragments=[])
        records.save_fragment(session, Fragment(
            source_text="B enables C." if id == "b" else "A causes B.",
            cues=["a", "b", "c", "d"], id=id,
            created_at=datetime.fromtimestamp(stamp, timezone.utc).isoformat()))
    records.close_session("closed", reason="clear")
    assert [f["id"] for f in collect_sources(records, 100, 1000)] == ["a", "b"]


def test_raw_input_contract(desktop, monkeypatch):
    monkeypatch.setattr(desktop, "popup_helper_command", lambda: ["helper"])
    monkeypatch.setattr(desktop, "cursor_position", lambda: None)
    monkeypatch.setattr(desktop, "prepare_popup", lambda payload, window: {**payload, "observation": {}})
    monkeypatch.setattr(desktop, "observe_artifact", lambda *args, **kwargs: None)
    monkeypatch.setattr(desktop.subprocess, "run", lambda *args, **kwargs:
                        SimpleNamespace(stdout="  exact goal  ", returncode=0))
    assert desktop.popup_input("goal", "focus", preserve_raw=True) == "  exact goal  "
    assert desktop.popup_input("goal", "other") == "exact goal"


def test_timer_expiry_once_and_cleanup():
    from xml.etree import ElementTree
    from cognitive_popups.focus_timer import FocusTimer

    calls, svgs, images = [], [], []
    now = [999]
    label = [""]
    pixbuf = object()

    def loader_for(type):
        assert type == "svg"
        return SimpleNamespace(
            write=lambda data: svgs.append(ElementTree.fromstring(data)),
            close=lambda: None,
            get_pixbuf=lambda: pixbuf,
        )

    timer = object.__new__(FocusTimer)
    timer.state = {"id": "block", "started_at": 100, "deadline": 1000}
    timer.clock = lambda: now[0]
    timer.width, timer.height = FocusTimer.WIDTH, FocusTimer.HEIGHT
    timer.closed, timer.expired, timer.source = False, False, 42
    timer.expiration_source = 43
    # Gtk.Overlay keeps its image and plain-text label; refresh replaces the pixbuf.
    timer.canvas = SimpleNamespace(set_from_pixbuf=images.append)
    timer.countdown = SimpleNamespace(
        get_text=lambda: label[0], set_text=lambda text: label.__setitem__(0, text))
    timer.GdkPixbuf = SimpleNamespace(PixbufLoader=SimpleNamespace(new_with_type=loader_for))
    timer.window = SimpleNamespace(get_mapped=lambda: True, destroy=lambda: calls.append("destroy"))
    timer.GLib = SimpleNamespace(
        source_remove=lambda id: calls.append(id),
        Bytes=SimpleNamespace(new=lambda data: SimpleNamespace(get_data=lambda: data)),
    )
    timer.on_expire = lambda id: calls.append(id)
    circles = "{http://www.w3.org/2000/svg}circle"

    assert timer._tick()
    assert label[0] == "00:01"
    assert images == [pixbuf]
    assert len(svgs[0].findall(circles)) == 2
    assert calls == []

    now[0] = 1000
    assert timer._expire() is False
    assert timer.expired
    assert timer.source is None
    assert timer.expiration_source is None
    assert label[0] == "Время"
    assert images == [pixbuf, pixbuf]
    assert len(svgs[-1].findall(circles)) == 1
    assert timer._expire() is False
    assert calls == [42, "block"]
    assert len(images) == 2  # Repeated expiry must neither redraw nor fire again.
    timer.close()
    timer.close()
    assert calls == [42, "block", "destroy"]
    assert timer._tick() is False
    assert len(images) == 2

    # Closing before expiry must also remove the still-pending deadline callback.
    pending = object.__new__(FocusTimer)
    pending.closed, pending.source, pending.expiration_source = False, 44, 45
    pending.GLib, pending.window = timer.GLib, timer.window
    pending.close()
    pending.close()
    assert pending.closed
    assert pending.source is None and pending.expiration_source is None
    assert calls == [42, "block", "destroy", 44, 45, "destroy"]


def test_desktop_focus_worker_and_hidden_targets(desktop, setup, monkeypatch):
    flow, now, client, records = setup
    state = flow.start("goal")
    now[0] = 1000
    app = object.__new__(desktop.DesktopApp)
    app.focus, app.focus_timer = flow, None
    done = threading.Event()
    inputs, results = [], []
    owner = threading.get_ident()

    def input_worker(question, title, **kwargs):
        assert threading.get_ident() != owner
        inputs.append(question)
        assert "rubric" not in question and "targets" not in question
        return "A causes B"

    monkeypatch.setattr(desktop, "popup_input", input_worker)
    app._focus_result = lambda id, result: (results.append(result), done.set())
    app._focus_message = lambda *args: done.set()
    app._focus_expired_legacy(state["id"])  # frozen 15→1 ending, kept under test
    assert done.wait(3)
    assert inputs == [READY["question"]]
    assert results == [CHECK]
    assert len(client.calls) == 2


def test_feynman_semantic_sounds(desktop, monkeypatch):
    app = object.__new__(desktop.DesktopApp)
    roles = []
    app.flash = SimpleNamespace(show_text=lambda *args, **kwargs: roles.append(kwargs.get("semantic_role")))
    monkeypatch.setattr(desktop.sound, "play", lambda *args: pytest.fail("parent must be silent"))
    monkeypatch.setattr(desktop, "emit", lambda *args, **kwargs: None)
    app._check_done("passed", [], None)
    app._check_done("needs_retry", [{"description": "gap"}], None)
    assert roles == ["resolve", "correction"]


def test_prediction_primary_delta_and_evidence(desktop, monkeypatch):
    from cognitive_popups.models import PredictionCheck
    app = object.__new__(desktop.DesktopApp)
    app.service = SimpleNamespace(session=SimpleNamespace(fragments=[fragment("a", 100)]))
    check = PredictionCheck(["a"], "hypothesis", "contradicted",
                            mismatch="old mismatch", one_delta="one correction", evidence="A causes B.")
    payloads, done = [], threading.Event()
    monkeypatch.setattr(desktop, "emit", lambda *args, **kwargs: None)
    monkeypatch.setattr(desktop, "bind_chain", lambda worker: worker)

    def popup(payload, **kwargs):
        payloads.append(payload)
        done.set()
        return ""

    monkeypatch.setattr(desktop, "run_popup", popup)
    app._prediction_done(check, None)
    assert done.wait(3)
    assert payloads[0]["text"] == check.text == "one correction"
    assert payloads[0]["one_delta"] == "one correction"
    assert payloads[0]["evidence"] == "A causes B."
    assert "expanded" not in payloads[0]
    assert "actions" not in payloads[0]  # "another angle" is frozen


def test_failed_atomic_replace_preserves_previous_state(setup, monkeypatch):
    from cognitive_popups import focus
    flow, now, client, records = setup
    state = flow.start("goal")
    previous = flow.path.read_bytes()

    def fail_replace(*args):
        raise OSError("disk failure")

    monkeypatch.setattr(focus.os, "replace", fail_replace)
    with pytest.raises(OSError):
        flow.finish(state["id"])
    assert flow.snapshot() == state
    assert flow.path.read_bytes() == previous
    assert not flow.path.with_name("focus.json.tmp").exists()


def source(id, text):
    return {"id": id, "session_id": "s", "created_at": "2026-10-05T10:00:00+00:00",
            "source_text": text}


def test_cleanup_duplicates_repeated_clipboard_blocks_and_stability():
    sources = [source("first", "A causes B.\nB enables C."),
               source("duplicate", "A causes B.\nB enables C."),
               source("repeated", "A causes B.\nB enables C.\nA causes B.\nB enables C."),
               source("new", "C limits D.\nC limits D.")]
    original = json.loads(json.dumps(sources))
    cleaned = clean_sources(sources)
    assert [(f["id"], f["source_text"]) for f in cleaned] == [
        ("first", "A causes B.\nB enables C."), ("new", "C limits D.")]
    assert sources == original
    assert clean_sources(sources) == cleaned
    assert clean_sources(cleaned) == cleaned


def test_cleanup_adjacent_multiline_overlap_keeps_new_content():
    overlap = ("Pressure increases when the volume is reduced at constant temperature.\n"
               "The number of particles remains unchanged during compression.")
    cleaned = clean_sources([source("a", "Earlier context.\n" + overlap),
                             source("b", overlap + "\nThis explains the measured pressure.")])
    assert cleaned[0]["source_text"] == "Earlier context.\n" + overlap
    assert cleaned[1]["source_text"] == "This explains the measured pressure."
    assert cleaned[1]["id"] == "b"


@pytest.mark.parametrize("text", ["", "...", "???", "asdf", "qwerty",
    "https://example.com", "https://example.com\nwww.example.org",
    "Copied to clipboard", "No text selected", "Copy\nPaste\nCancel",
    "wl-paste: error: clipboard is unavailable", "Скопировано в буфер обмена"])
def test_cleanup_contentless_noise(text):
    assert clean_sources([source("noise", text)]) == []


@pytest.mark.parametrize("text", ["x", "F=ma", "E=mc²", "∇×E=0", "∫", "a → b",
    "P(A|B)", "Copy", "Home", "A", "A causes B.",
    "See https://example.com for the derivation."])
def test_cleanup_preserves_short_formulas_and_ambiguous_text(text):
    assert clean_sources([source("real", text)])[0]["source_text"] == text


def test_cleanup_retains_small_formula_overlap():
    sources = [source("a", "F=ma\nE=mc²"), source("b", "F=ma\nE=mc²\np=mv")]
    assert clean_sources(sources) == sources


def test_cleanup_uses_actual_timestamp_order_for_duplicate_owner():
    records = Records()
    early = fragment("early", 100)
    # Lexical ISO order differs from instant order; a naive timestamp is UTC.
    early.created_at = "1970-01-01T01:01:40+01:00"
    late = fragment("late", 200)
    late.created_at = "1970-01-01T00:03:20"
    records.data = {"late_session": [late], "early_session": [early]}
    assert [f["id"] for f in collect_sources(records, 100, 1000)] == ["early"]


def test_collection_cleanup_across_sessions(setup):
    flow, now, client, records = setup
    records.data["current"].extend([
        fragment("duplicate", 600, "A causes B."),
        fragment("noise", 700, "https://example.com"),
        fragment("formula", 800, "F=ma")])
    assert [f["id"] for f in collect_sources(records, 100, 1000)] == ["a", "b", "formula"]


@pytest.mark.parametrize("quality", ["good", "partial", "direct"])
def test_accepted_quality_and_legacy_direct(setup, quality):
    flow, now, client, records = setup
    client.answers = [{**READY, "evidence_quality": quality}, CHECK]
    state = flow.start("goal")
    now[0] = 1000
    ready = flow.generate(state["id"])
    assert ready["generation"]["evidence_quality"] == ("good" if quality == "direct" else quality)
    assert flow.check(state["id"], "answer")["status"] == "completed"
    assert len(client.calls) == 2


def test_bad_quality_is_insufficient_context_without_check(setup):
    flow, now, client, records = setup
    # A bad 'ready' must be closed safely even if other ready fields are absent.
    client.answers = [{"status": "ready", "evidence_quality": "bad"}]
    state = flow.start("goal")
    now[0] = 1000
    result = flow.generate(state["id"])
    assert result["status"] == "insufficient_context"
    assert result["generation"]["evidence_quality"] == "bad"
    assert flow.check(state["id"], "answer") is None
    assert len(client.calls) == 1


@pytest.mark.parametrize("changes", [{"evidence_quality": "excellent"},
    {"fragment_ids": ["a", "a"]}, {"core_target": []}, {"target_type": ""}])
def test_invalid_quality_or_selected_contract_fails_without_retry(setup, changes):
    flow, now, client, records = setup
    client.answers = [{**READY, **changes}]
    state = flow.start("goal")
    now[0] = 1000
    with pytest.raises(ValueError):
        flow.generate(state["id"])
    assert flow.snapshot()["status"] == "failed"
    assert len(client.calls) == 1


def test_checker_only_selected_sources_and_persisted_frozen_text(setup):
    flow, now, client, records = setup
    records.data["current"].append(fragment("unrelated", 900, "UNRELATED PRIVATE CAPTURE"))
    client.answers = [{**READY, "fragment_ids": ["a"], "core_target": "A causes B",
                       "target_type": "causal"}, CHECK]
    state = flow.start("goal mentioning unrelated material")
    now[0] = 1000
    ready = flow.generate(state["id"])
    assert len(ready["frozen"]["sources"]) == 3
    expected = [{"id": "a", "source_text": "A causes B."}]
    assert ready["selected_sources"] == expected
    assert json.loads(flow.path.read_text())["selected_sources"] == expected
    records.data.clear()
    ready["selected_sources"][0]["source_text"] = "mutation of returned copy"
    flow.check(state["id"], "because A")
    checker = client.calls[1]
    assert checker["snapshot"] == {"sources": expected}
    serialized = json.dumps(checker)
    assert "UNRELATED PRIVATE CAPTURE" not in serialized
    assert "B enables C." not in serialized
    assert "goal mentioning" not in serialized
    assert "session_id" not in serialized
    assert len(client.calls) == 2


def test_prompt_policies_are_explicit():
    for policy in ("ONE simple topic", "intersection", "answer-structure leak", "OCR",
                   '"good"', '"partial"', '"bad"', "smallest", "hidden"):
        assert policy in GENERATOR
    for policy in ("selected IDs", "two concrete causal gaps", "No style criticism",
                   "rewrite", "confidence score", "equivalent wording", "OCR-damaged"):
        assert policy in CHECKER
