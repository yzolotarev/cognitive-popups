"""Headless integration of service, desktop scheduling, helper and task observations."""
import importlib.util
import io
import json
import sys
import threading
from pathlib import Path
from types import ModuleType, SimpleNamespace

import pytest

from cognitive_popups import client as client_module, observation, popup_helper, tasks
from cognitive_popups.client import GeminiWeb2API, Web2APIError
from cognitive_popups.event_log import EventChain, EventLog, fetch
from cognitive_popups.models import CognitiveSession
from cognitive_popups.operation_context import OperationContext, current_operation, operation_scope
from cognitive_popups.prompt_settings import PromptSettings
from cognitive_popups.records import RecordStore
from cognitive_popups.service import CognitiveService, output_artifact, prompt_hash


CUES = [
    {"simple": "память", "term": "memory", "meaning": "удержание информации"},
    {"simple": "предел", "term": "limit", "meaning": "ограничение количества"},
    {"simple": "группы", "term": "chunking", "meaning": "объединение элементов"},
    {"simple": "заметка", "term": "note", "meaning": "внешняя опора"},
]


@pytest.fixture(autouse=True)
def isolated_state(monkeypatch, tmp_path):
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    for variable, filename in (("COGNITIVE_RECORD_DB", "records.sqlite3"),
                               ("COGNITIVE_EVENT_DB", "events.sqlite3"),
                               ("COGNITIVE_NOTES_DB", "notes.sqlite3")):
        monkeypatch.setenv(variable, str(tmp_path / filename))
    monkeypatch.delenv("COGNITIVE_EVENT_DISABLE", raising=False)
    monkeypatch.delenv("COGNITIVE_OBSERVATION_DISABLE", raising=False)
    monkeypatch.delenv("COGNITIVE_EVENT_SESSION", raising=False)
    monkeypatch.delenv("COGNITIVE_OPERATION_ID", raising=False)
    token = output_artifact.set(None)
    with operation_scope(None):
        yield
    output_artifact.reset(token)


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
    spec = importlib.util.spec_from_file_location("cognitive_popups._test_desktop", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def rows(table):
    with observation.ObservationStore().connect() as conn:
        return [dict(row) for row in conn.execute(f"SELECT * FROM {table} ORDER BY rowid")]


def payload(row):
    return json.loads(row["payload_json"])


def transport(monkeypatch, *responses, before=None):
    replies = iter(responses)
    calls = []

    def urlopen(request, **kwargs):
        calls.append((current_operation(), json.loads(request.data)))
        if before:
            before()
        reply = next(replies)
        if isinstance(reply, BaseException):
            raise reply
        raw = json.dumps({"choices": [{"message": {"content": reply}}]}).encode()
        response = io.BytesIO(raw)
        response.status = 200
        return response

    monkeypatch.setattr(client_module.request, "urlopen", urlopen)
    return calls


def service(tmp_path):
    return CognitiveService(GeminiWeb2API(model="test-model"),
                            prompt_settings=PromptSettings(tmp_path / "prompts.json"))


@pytest.mark.parametrize("source", ["новый clipboard без W", "Длинный текст\n" * 5000, ""])
def test_note_snapshots_new_clipboard_without_seeding(desktop, monkeypatch, tmp_path, source):
    from cognitive_popups.models import Fragment, text_hash
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    old = Fragment(source_text="старый источник", cues=["a", "b", "c", "d"])
    session = SimpleNamespace(id="before-popup", fragments=[old])
    app.service = SimpleNamespace(session=session)
    app.flash = SimpleNamespace(show_cues=lambda *args: None, show_notice=lambda *args, **kwargs: None)
    calls = []

    def probe(command):
        calls.append(command)
        return (0, "" if "--primary" in command or "primary" in command else source, "")

    monkeypatch.setattr(desktop, "run_probe", probe)

    def popup(*args, **kwargs):
        assert calls  # Snapshot must precede opening the window.
        monkeypatch.setattr(desktop, "run_probe", lambda *args: pytest.fail("late clipboard read"))
        app.service.session = SimpleNamespace(id="after-popup", fragments=[])
        return "комментарий"

    monkeypatch.setattr(desktop, "run_popup", popup)
    desktop.begin_interaction("hotkey", window="note")
    app.capture_error_note()
    note = desktop.NOTES.list()[0]
    captured = source.strip()
    assert note.source_text == (captured or None)
    assert note.source_hash == (text_hash(captured) if captured else None)
    assert note.fragment_id is None
    assert note.session_id == "before-popup"
    assert session.fragments == [old]
    contents = [payload(row) for row in rows("obs_artifacts") if row["kind"] == "note_content"]
    assert contents[0]["source_text"] == (captured or None)
    assert contents[0]["fragment_id"] is None


@pytest.mark.parametrize("sources, expected", [
    (["prefix selected suffix"], True),
    (["unrelated"], False),
    (["selected one", "selected two"], False),
    ([], False),
])
def test_note_fragment_link_requires_unambiguous_source(desktop, monkeypatch, sources, expected):
    from cognitive_popups.models import Fragment, text_hash
    fragments = [Fragment(source_text=text, cues=["selected", "b", "c", "d"]) for text in sources]
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.service = SimpleNamespace(session=SimpleNamespace(id="session", fragments=fragments))
    app.flash = SimpleNamespace(show_cues=lambda *args: None, show_notice=lambda *args, **kwargs: None)
    monkeypatch.setattr(desktop, "selected_text", lambda: "selected")
    monkeypatch.setattr(desktop, "run_popup", lambda *args, **kwargs: "comment")
    desktop.begin_interaction("hotkey", window="note")
    app.capture_error_note()
    note = desktop.NOTES.list()[0]
    assert note.fragment_id == (fragments[0].id if expected else None)
    assert note.source_hash == text_hash("selected")


def test_worker_and_idle_callback_keep_origin_after_another_hotkey(desktop, monkeypatch, tmp_path):
    calls = transport(monkeypatch, "not JSON", json.dumps({"cues": CUES}))
    app_service = service(tmp_path)
    pending = []
    monkeypatch.setattr(desktop.GLib, "idle_add", lambda callback, *args: pending.append((callback, args)))
    first = desktop.begin_interaction("hotkey", window="seed")
    ready, release = threading.Event(), threading.Event()
    results, errors = [], []

    def worker():
        try:
            ready.set()
            assert release.wait(5)
            cues = app_service.extract_cues("Source text " * 100)
            desktop.idle_add(done, cues)
        except BaseException as exc:
            errors.append(exc)

    def done(cues):
        results.append((desktop.current_chain(), current_operation(),
                        desktop.prepare_popup({"mode": "dual", "items": list(cues)}, "dual")))

    thread = threading.Thread(target=desktop.bind_chain(worker))
    thread.start()
    assert ready.wait(5)
    second = desktop.begin_interaction("hotkey", window="note")
    release.set()
    thread.join(5)
    assert not thread.is_alive() and not errors
    callback, args = pending.pop()
    callback(*args)
    assert results[0][0] is first
    assert results[0][1].operation_id == first.operation_id
    assert desktop.current_chain() is second
    assert len(calls) == 2
    assert {ctx.operation_id for ctx, _ in calls} == {first.operation_id}
    assert {ctx.interaction_id for ctx, _ in calls} == {first.session_id}
    assert {ctx.buffer_session_id for ctx, _ in calls} == {app_service.session.id}
    generated = next(r for r in rows("obs_artifacts") if r["kind"] == "cues_generated")
    assert len(json.loads(generated["request_ids_json"])) == 2
    rendered = next(r for r in rows("obs_artifacts") if r["kind"] == "rendered_payload")
    assert rendered["source_artifact_id"] == generated["id"]
    assert rendered["operation_id"] == first.operation_id
    assert payload(rendered)["items"] == CUES
    assert [r["event"] for r in rows("obs_presentations")] == ["spawn"]


def test_background_cache_keeps_generation_identity_and_new_prompt_misses(desktop, monkeypatch, tmp_path):
    calls = transport(monkeypatch, json.dumps({"cues": CUES}), json.dumps({"cues": CUES}))
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.service = service(tmp_path)
    app._busy = False
    app._last_selection = ""
    app._cue_cache = {}
    app._pending_keys = set()
    app._commit_fragment = lambda fragment: None
    displays = []
    app.flash = SimpleNamespace(show_cues=lambda cues: displays.append(
        desktop.prepare_popup({"mode": "dual", "items": list(cues)}, "dual")))
    monkeypatch.setattr(desktop, "selected_text", lambda **kwargs: "Full selected source")

    class ImmediateThread:
        def __init__(self, target, **kwargs):
            self.target = target

        def start(self):
            self.target()

    monkeypatch.setattr(desktop, "threading", SimpleNamespace(Thread=ImmediateThread))
    desktop.begin_interaction("hotkey", window="unrelated")
    app.watch_selection()
    old_cues = next(iter(app._cue_cache.values()))
    assert calls[0][0].kind == "background"
    assert calls[0][0].interaction_id is None
    assert not displays
    assert not rows("obs_presentations")
    origin = desktop.begin_interaction("hotkey", window="seed")
    app.seed()
    assert len(calls) == 1
    fragment = app.service.session.fragments[-1]
    assert fragment.artifact_id == old_cues.artifact_id
    assert fragment.generating_operation_id == calls[0][0].operation_id
    assert fragment.request_ids == old_cues.request_ids
    cache_hit = next(payload(r) for r in rows("obs_annotations") if r["kind"] == "cache_hit")
    assert cache_hit["artifact_id"] == old_cues.artifact_id
    assert rows("obs_presentations")[-1]["operation_id"] == origin.operation_id

    app.service.prompt_settings.set("four_words", "A completely new system prompt")
    original = app.service.add_fragment_with_cues("Old source", old_cues)
    assert original.prompt_hash == old_cues.prompt_hash
    assert original.prompt_hash != prompt_hash("A completely new system prompt")
    desktop.begin_interaction("hotkey", window="seed")
    app.seed()
    assert len(calls) == 2
    assert app.service.session.fragments[-1].prompt_hash == prompt_hash("A completely new system prompt")
    requests = rows("obs_requests")
    assert requests[0]["config_id"] != requests[1]["config_id"]
    assert app.service.session.fragments[-1].artifact_id != old_cues.artifact_id
    plain = app.service.add_fragment_with_cues("Unknown provenance", list(CUES))
    assert plain.prompt_hash == plain.model == ""
    assert plain.artifact_id is None


def test_submitted_input_survives_model_failure_and_retry_scope(monkeypatch, tmp_path):
    app_service = service(tmp_path)
    app_service.add_fragment_with_cues("Memory is limited.", CUES)
    raw = "  My hypothesis\nwith exact spacing  "

    def input_already_saved():
        inputs = [r for r in rows("obs_artifacts") if r["kind"] == "prediction_input"]
        assert payload(inputs[-1])["arguments"]["hypothesis"] == raw

    transport(monkeypatch, TimeoutError("offline"), before=input_already_saved)
    with operation_scope(OperationContext(interaction_id="submitted")):
        with pytest.raises(Web2APIError):
            app_service.check_prediction(raw)
    assert not any(r["kind"] == "prediction_generated" for r in rows("obs_artifacts"))
    assert rows("obs_requests")[0]["interaction_id"] == "submitted"
    assert any(r["kind"] == "operation_failed" for r in rows("obs_annotations"))


def test_popup_spawn_actual_open_layers_and_full_payload_are_distinct(desktop, monkeypatch):
    chain = desktop.begin_interaction("hotkey")
    text = "precise rendered text\n" * 100
    monkeypatch.setattr(desktop, "popup_helper_command", lambda: ["external-helper"])
    monkeypatch.setattr(desktop, "cursor_position", lambda: (12, 34))
    spawned = []

    def run(command, **kwargs):
        spawned.append((json.loads(kwargs["input"]), kwargs["env"]))
        return SimpleNamespace(stdout="", returncode=0)

    monkeypatch.setattr(desktop.subprocess, "run", run)
    desktop.run_popup({"mode": "text", "text": text, "title": "Result"}, window="text")
    sent, env = spawned[0]
    assert env["COGNITIVE_OPERATION_ID"] == chain.operation_id
    assert env["COGNITIVE_RECORD_DB"] == str(observation.ObservationStore().path)
    assert [r["event"] for r in rows("obs_presentations")] == ["spawn"]
    rendered = next(r for r in rows("obs_artifacts") if r["id"] == sent["observation"]["artifact_id"])
    assert payload(rendered) == {"mode": "text", "text": text, "title": "Result", "x": 12, "y": 34}

    helper_chain = EventChain(chain.log, chain.session_id, origin="popup", operation_id=chain.operation_id)
    helper = popup_helper.PopupObservation(helper_chain, sent)
    helper.content = {"text": text.strip(), "title": "Result", "expanded": False}
    helper.emit("window_open", window="text")
    helper.emit("window_close", window="text")
    helper.emit("window_close", window="text")
    presentations = rows("obs_presentations")
    assert [r["event"] for r in presentations] == ["spawn", "window_open", "window_close"]
    assert {r["artifact_id"] for r in presentations} == {rendered["id"]}
    assert {r["window_instance_id"] for r in presentations} == {sent["observation"]["window_instance_id"]}
    assert payload(presentations[1])["rendered"]["text"] == text.strip()
    helper_events = [r for r in fetch(chain.log.path) if r["origin"] == "popup"]
    assert all(r["artifact_id"] == rendered["id"] for r in helper_events)


def test_helper_without_display_never_records_open(monkeypatch):
    chain = EventChain(EventLog(), "no-display", origin="popup")
    monkeypatch.setattr(popup_helper, "_CHAIN", chain)
    monkeypatch.setattr(popup_helper, "load_payload", lambda: {"mode": "text", "text": "hello"})
    monkeypatch.setattr(popup_helper, "_dispatch", lambda *args: 1)
    monkeypatch.setattr(popup_helper, "_PRESENTATION", None)
    assert popup_helper.main() == 1
    assert rows("obs_presentations") == []


def test_helper_submission_is_raw_and_cancel_does_not_capture_draft():
    chain = EventChain(EventLog(), "input", origin="popup")
    helper = popup_helper.PopupObservation(chain, {"mode": "input", "prompt": "Question"})
    helper.emit("window_open", window="input")
    helper.submitted("  raw\nsubmitted words  ", "input")
    helper.emit("window_close", window="input")
    cancelled = popup_helper.PopupObservation(chain, {"mode": "note", "anchor": "source"})
    cancelled.emit("window_open", window="note")
    cancelled.emit("window_close", window="note", detail="cancel")
    inputs = [r for r in rows("obs_artifacts") if r["kind"] == "submitted_input"]
    assert len(inputs) == 1
    assert payload(inputs[0])["text"] == "  raw\nsubmitted words  "
    assert helper.instance != cancelled.instance


def test_task_menu_generation_popup_share_chain_and_source(desktop, monkeypatch):
    store = RecordStore()
    session = CognitiveSession()
    fragment = session.add_fragment("Материал про перестановку корней.",
                                    [cue["term"] for cue in CUES], CUES)
    store.save_fragment(session, fragment)
    result = json.dumps({"status": "ready", "condition": "Сравните корень и перестановку корней.",
        "grounding": ["перестановку корней"], "required_operations": ["сравнить"], "difficulty": "quick"})
    calls = transport(monkeypatch, "invalid JSON", result)
    monkeypatch.setattr(GeminiWeb2API, "health", lambda self: {"status": "ok"})
    monkeypatch.setitem(sys.modules, "cognitive_popups.desktop", desktop)
    monkeypatch.setattr(desktop, "popup_helper_command", lambda: ["packaged-helper"])
    monkeypatch.setattr(desktop, "cursor_position", lambda: None)
    spawned = []

    def run(command, **kwargs):
        if "input" not in kwargs:
            return SimpleNamespace(stdout="", stderr="", returncode=0)
        data = json.loads(kwargs["input"])
        spawned.append((data, kwargs["env"]))
        # A different desktop action must not replace the task invocation's chain.
        desktop.begin_interaction("hotkey", window="other")
        return SimpleNamespace(stdout='{\"action\":\"8\"}' if data["mode"] == "menu" else "", stderr="", returncode=0)

    monkeypatch.setattr(tasks.subprocess, "run", run)
    assert tasks.main(["--menu"]) == 0
    assert len(spawned) == 2 and len(calls) == 2
    operation = calls[0][0].operation_id
    assert {env["COGNITIVE_OPERATION_ID"] for _, env in spawned} == {operation}
    assert {ctx.operation_id for ctx, _ in calls} == {operation}
    artifacts = rows("obs_artifacts")
    source = next(r for r in artifacts if r["kind"] == "task_source")
    assert payload(source)["technical_buffer"][0]["source_text"] == fragment.source_text
    assert payload(source)["fragment_ids"] == [fragment.id]
    generated = next(r for r in artifacts if r["kind"] == "task_generated")
    assert len(json.loads(generated["request_ids_json"])) == 2
    shown = spawned[1][0]
    assert shown["text"] == json.loads(result)["condition"]
    rendered = next(r for r in artifacts if r["id"] == shown["observation"]["artifact_id"])
    assert rendered["source_artifact_id"] == generated["id"]
    assert any(r["kind"] == "ledger_link" and payload(r).get("artifact_id") == generated["id"]
               for r in rows("obs_annotations"))
    assert all(r["event"] == "spawn" for r in rows("obs_presentations"))


def test_solution_cli_exports_prompt_not_model_answer(monkeypatch, capsys):
    monkeypatch.setattr(tasks, "read_task_text", lambda source: "Exact condition")
    assert tasks.main(["--type", "3", "--solution", "-", "--no-context"]) == 0
    exported = capsys.readouterr().out
    artifacts = rows("obs_artifacts")
    prompt = next(r for r in artifacts if r["kind"] == "prompt_export")
    assert payload(prompt)["prompt"] + "\n" == exported
    assert payload(prompt)["kind"] == "solution"
    assert rows("obs_requests") == rows("obs_presentations") == []
    assert not any("generated" in r["kind"] for r in artifacts)
    assert any(r["kind"] == "prompt_exported" for r in rows("obs_annotations"))


def test_rendered_check_is_ui_text_not_the_unfiltered_model_result(desktop, monkeypatch):
    desktop.begin_interaction("hotkey", window="feynman")
    generated = desktop.observe_artifact("feynman_check_generated", {"status": "needs_retry"})
    output_artifact.set(generated)
    sent = []

    class Sink:
        def write(self, text):
            sent.append(json.loads(text))

        def close(self):
            pass

    monkeypatch.setattr(desktop, "popup_helper_command", lambda: ["helper"])
    monkeypatch.setattr(desktop, "cursor_position", lambda: None)
    monkeypatch.setattr(desktop.subprocess, "Popen", lambda *a, **kw: SimpleNamespace(stdin=Sink()))
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.flash = desktop.FlashWindow()
    app._check_done("needs_retry", [
        {"description": "First exact gap"}, {"location": "Second exact gap"},
        {"description": "This third gap is not rendered"},
    ], "Exact follow-up")
    expected = "• First exact gap\n• Second exact gap\n\nExact follow-up"
    assert sent[0]["text"] == expected
    artifact = next(r for r in rows("obs_artifacts") if r["kind"] == "rendered_payload")
    assert payload(artifact) == {"mode": "text", "text": expected,
                                 "title": "Фейнман: пробел", "expanded": False,
                                 "semantic_role": "correction"}
    assert artifact["source_artifact_id"] == generated
    assert all(r["event"] == "spawn" for r in rows("obs_presentations"))


def test_question_provenance_is_carried_by_each_question(monkeypatch, tmp_path):
    app_service = service(tmp_path)
    app_service.add_fragment_with_cues("Memory has a limit.", CUES)
    transport(monkeypatch,
              json.dumps({"question": "How does memory relate to limit?"}),
              json.dumps({"question": "How does chunking relate to note?"}),
              json.dumps({"status": "passed"}))
    app_service.prompt_settings.set("feynman_question", "First question prompt")
    first = app_service.create_feynman_question()
    app_service.prompt_settings.set("feynman_question", "Second question prompt")
    app_service.create_feynman_question()
    check = app_service.check_feynman(first, "Submitted explanation")
    assert check.question_prompt_hash == prompt_hash("First question prompt")


def test_layers_reference_exact_visible_view_artifacts():
    chain = EventChain(EventLog(), "layers", origin="popup")
    helper = popup_helper.PopupObservation(chain, {"mode": "dual", "items": CUES})
    helper.content = {"simple": [c["simple"] for c in CUES], "terms": [], "meanings": []}
    helper.emit("window_open", window="dual", layer=1)
    helper.content = {"simple": [c["simple"] for c in CUES],
                      "terms": [CUES[0]["term"], "", "", ""], "meanings": []}
    helper.emit("layer_open", window="dual", layer=2, item_index=0)
    views = [r for r in rows("obs_artifacts") if r["kind"] == "rendered_view"]
    assert payload(views[0])["terms"] == []
    assert payload(views[1])["terms"] == ["memory", "", "", ""]
    assert payload(rows("obs_presentations")[-1])["rendered_artifact_id"] == views[1]["id"]
    assert all(r["source_artifact_id"] == helper.artifact for r in views)


def test_task_menu_cancel_does_not_generate(monkeypatch):
    monkeypatch.setattr(tasks, "choose_type", lambda: None)
    assert tasks.main(["--menu", "--no-context"]) == 0
    assert rows("obs_requests") == []
    assert any(r["kind"] == "task_choice" and payload(r)["outcome"] == "cancelled"
               for r in rows("obs_annotations"))


def test_disable_propagates_without_observation_writes(desktop, monkeypatch, tmp_path):
    monkeypatch.setenv("COGNITIVE_EVENT_DISABLE", "1")
    desktop.EVENTS = EventLog(tmp_path / "disabled-events.sqlite3")
    desktop.begin_interaction("hotkey")
    desktop.prepare_popup({"mode": "text", "text": "not recorded"}, "text")
    assert desktop.popup_env()["COGNITIVE_EVENT_DISABLE"] == "1"
    assert not observation.ObservationStore().path.exists()
    assert not desktop.EVENTS.path.exists()
