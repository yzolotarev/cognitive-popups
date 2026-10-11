import json
import sqlite3
from types import SimpleNamespace

import pytest

from cognitive_popups import capture, cutscene_model
from cognitive_popups.manual_queue import LatestRequest, queued_action, selection
from cognitive_popups.sand_store import SandStore
from cognitive_popups.sand_fx import scene_svg, svg_particles
from cognitive_popups.sand_core import Sand


@pytest.fixture(autouse=True)
def isolated_capture_stamps(monkeypatch, tmp_path):
    monkeypatch.setattr(capture,"PRIMARY_STAMP",tmp_path / "primary.stamp")
    monkeypatch.setattr(capture,"CLIP_STAMP",tmp_path / "clipboard.stamp")


def test_latest_request_retains_only_last_and_can_be_consumed_once():
    queue = LatestRequest()
    first, last = lambda: 1, lambda: 2
    assert not queue.put(first)
    assert queue.put(last)
    assert queue.take() is last
    assert queue.take() is None


def test_direct_helper_close_does_not_send_presentation_option_to_event_log(monkeypatch):
    from cognitive_popups import popup_helper
    monkeypatch.setattr(popup_helper, "_PRESENTATION", None)
    seen = []
    monkeypatch.setattr(popup_helper, "_CHAIN", SimpleNamespace(emit=lambda event, **fields: seen.append(fields)))
    popup_helper._emit("window_close", window="dual", silent=True)
    assert seen == [{"window": "dual"}]


def test_busy_requests_wait_then_last_request_runs_on_release(desktop, monkeypatch):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.flash = SimpleNamespace(show_notice=lambda *_: None)
    app._busy = True
    monkeypatch.setattr(desktop, "selected_text_source", lambda: ("исходное выделение", "primary"))
    monkeypatch.setattr(app, "_off_main", lambda _, action: action())
    seen = []
    @queued_action
    def request(app, label):
        seen.append((label, selection.snapshot))
    request(app, "first")
    request(app, "last")
    assert seen == []
    monkeypatch.setattr(desktop, "selected_text_source", lambda: ("другой текст", "clipboard"))
    app._busy = False
    assert seen == [("last", ("исходное выделение", "primary"))]
    assert not hasattr(selection, "snapshot")


def test_queue_callback_carries_the_press_time_snapshot(desktop, monkeypatch):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.flash = SimpleNamespace(show_notice=lambda *_: None)
    monkeypatch.setattr(desktop, "selected_text_source", lambda: ("A", "primary"))
    seen = []
    def action(app):
        seen.append(selection.snapshot)
    app._queue_manual(action, (), {})
    monkeypatch.setattr(desktop, "selected_text_source", lambda: ("B", "clipboard"))
    app._manual_queue.take()()
    assert seen == [("A", "primary")]
    assert not hasattr(selection, "snapshot")


def test_rung_status_is_explicit_and_is_scoped_to_its_edge(tmp_path):
    store = SandStore(tmp_path / "records.sqlite3")
    edge = store.pin("s", "  текст ребра  ", "copy")
    step = SimpleNamespace(start="A", step="B", text="Текст ступеньки")
    rung = store.add_rung("s", step, edge["id"])
    assert store.edge("s")["text"] == "  текст ребра  "
    store.bind_step(rung, "block")
    assert store.rungs("s")[0]["status"] == "read"
    store.mark(rung, "known")
    assert store.rungs("s")[0]["status"] == "known"
    with pytest.raises(ValueError):
        store.mark(rung, "done")
    store.pin("s", "новое ребро")
    assert store.rungs("s") == []
    assert store.edge("other") is None


def test_both_guesses_survive_verbatim_and_scene_can_be_replayed(tmp_path):
    store = SandStore(tmp_path / "records.sqlite3")
    edge = store.pin("s", "ребро")
    store.guess(edge["id"], "open", "  первая\nдогадка  ", "contradicted")
    store.guess(edge["id"], "close", "вторая", "confirmed")
    assert [g["text"] for g in store.guesses(edge["id"])] == ["  первая\nдогадка  ", "вторая"]
    store.save_scene("s", {"stars": [{"text": "<моя мысль>"}]})
    with store.db() as db:
        assert json.loads(db.execute("SELECT payload FROM sand_scenes").fetchone()[0])["stars"][0]["text"] == "<моя мысль>"
    store.close("s")
    assert store.edge("s") is None


def test_empty_graph_can_have_a_far_edge_and_repeated_links_do_not_invent_a_cycle():
    scene = cutscene_model.build_scene(cutscene_model.DaySummary("x", [], [], far_edge="дальше"))
    assert scene.stars[-1].label == "дальше"
    scene = cutscene_model.build_scene(cutscene_model.DaySummary("x", ["a", "b", "c"], [("a","b"),("b","a"),("b","c")]))
    assert scene.form == "chain"
    assert len(scene.edges) == 2


def test_scene_escapes_markup_and_never_includes_reader_text_as_markup():
    from dataclasses import asdict
    day = cutscene_model.DaySummary("x", ["<script>"], [], thoughts=[cutscene_model.Thought("<script>", "<private text>")])
    scene = asdict(cutscene_model.build_scene(day))
    svg = scene_svg(scene, 100)
    assert "<script>" not in svg and "&lt;script&gt;" in svg
    assert "private text" not in svg


def test_capture_never_sends_control_c_to_a_terminal():
    assert capture.copy_shortcut_for("kitty") == "CTRL SHIFT, C"
    assert capture.copy_shortcut_for("org.kde.okular") == "CTRL, C"
    modern = capture.copy_commands("org.kde.okular", "0xabc")[0]
    assert 'key = "code:54"' in modern[-1]
    assert 'address:0xabc' in modern[-1]


def test_terminal_uses_primary_before_copy_and_still_reports_dev(monkeypatch):
    monkeypatch.setattr(capture, "active_window", lambda: {"class":"kitty", "address":"0xabc"})
    monkeypatch.setattr(capture, "cursor_pos", lambda: None)
    monkeypatch.setattr(capture, "_try_primary", lambda _: "выделено в Kitty")
    monkeypatch.setattr(capture, "_try_copy", lambda *args: pytest.fail("primary succeeded"))
    result = capture.capture_selection()
    assert (result.text, result.origin, result.context) == ("выделено в Kitty", "primary", "dev")
    assert capture.plan_for("okular") == ["copy", "primary"]


@pytest.mark.parametrize("before,after,old,new,expected", [
    (10,11,"same","same",True), (10,10,"old","new",True),
    (10,None,"old","new",True), (None,11,"same","same",True),
    (None,None,"same","same",False), (None,None,"old","new",True),
])
def test_copy_freshness_uses_watcher_even_when_text_is_identical(before,after,old,new,expected):
    assert capture.copy_changed(before,after,old,new,False) is expected


def test_dev_capture_gives_four_words_but_never_enters_study_buffer(desktop, monkeypatch, tmp_path):
    monkeypatch.setattr(desktop, "STATE_DIR", tmp_path)
    (tmp_path / "sand.enabled").touch()
    monkeypatch.setattr(capture,"capture_selection",lambda **kw: capture.CaptureResult(text="kitty selection", origin="primary", context="dev"))
    monkeypatch.setattr(desktop.context, "remember", lambda *a, **k: pytest.fail("terminal text is not study"))
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    shown = []
    cues = [{"simple": "a", "term": "b"}]
    app._cue_cache = {"key": cues}
    app.service = SimpleNamespace(cue_cache_key=lambda text: "key",
                                  add_fragment_with_cues=lambda text, c: SimpleNamespace(
                                      artifact_id=None, generating_operation_id=None, id="f", request_ids=[],
                                      source_hash="h"))
    app._commit_fragment = lambda fragment: None
    app.flash = SimpleNamespace(show_cues=lambda c, origin="": shown.append((c, origin)))
    app.seed()
    assert shown and shown[0][0] == cues
    assert "не в учебную историю" in shown[0][1]


def test_failed_capture_never_falls_back_to_stale_clipboard(desktop, monkeypatch, tmp_path):
    monkeypatch.setattr(desktop, "STATE_DIR", tmp_path)
    (tmp_path / "sand.enabled").touch()
    monkeypatch.setattr(capture,"capture_selection",lambda **kw: capture.CaptureResult())
    monkeypatch.setattr(desktop,"run_probe",lambda *a: pytest.fail("stale probes are not evidence of selection"))
    assert desktop.selected_text_source() == ("", "")


def test_v2_defaults_keep_rungs_read_and_verdict_bridge_hidden(desktop):
    assert not desktop.DesktopApp.SAND_KNOWN_MARKS
    assert not desktop.DesktopApp.SAND_LADDER_BRIDGE


def test_pre_step_rung_and_edge_move_into_the_new_study_session(tmp_path):
    store = SandStore(tmp_path / "records.sqlite3")
    edge = store.pin("buffer", "ребро")
    rung = store.add_rung("buffer", SimpleNamespace(start="A", step="B", text="шаг"), edge["id"])
    store.move_session("buffer", "study")
    assert store.edge("buffer") is None
    assert store.edge("study")["id"] == edge["id"]
    assert store.rungs("study")[0]["id"] == rung
    store.bind_step(rung, "block")
    assert store.rung_for_step("block")["status"] == "read"
    store.mark(rung, "known")
    assert store.rung_for_step("block")["status"] == "known"


def test_final_guess_is_saved_before_its_verdict_without_rewriting_words(tmp_path):
    store = SandStore(tmp_path / "records.sqlite3")
    edge = store.pin("s", "ребро")
    key = store.guess(edge["id"], "close", "  догадка\n ", "pending")
    store.verdict(key, "unavailable")
    assert store.guesses(edge["id"])[0]["text"] == "  догадка\n "
    assert store.guesses(edge["id"])[0]["verdict"] == "unavailable"


def test_many_rungs_all_ignite_before_the_scene_ends():
    scene = cutscene_model.build_scene(cutscene_model.DaySummary("x", [f"c{i}" for i in range(16)], [],
        far_edge="цель", ladder=[cutscene_model.LadderStep(str(i), "read") for i in range(30)]))
    assert max(s.ignite for s in scene.stars) < scene.timeline["title"] < scene.timeline["end"]


def test_sky_connects_only_equal_saved_labels():
    from cognitive_popups.sand_cli import sky
    a = {"stars": [{"label": "a", "x": .2, "y": .3}], "edges": []}
    b = {"stars": [{"label": "a", "x": .2, "y": .3}, {"label":"b", "x": .7,"y": .4}], "edges": []}
    scene = sky([a,b])
    assert scene["edges"] == [(0,1,True)]
    assert all(0 <= s["x"] <= 1 and 0 <= s["y"] <= 1 for s in scene["stars"])
    assert sky([{**a, "stars": [{"label":"Мысль 1", "x":.2,"y":.3}]}]*2)["edges"] == []


def test_close_loop_saves_the_guess_before_request_and_archives_actual_scene(desktop, monkeypatch, tmp_path):
    import time
    from cognitive_popups.records import RecordStore
    records = RecordStore(tmp_path / "records.sqlite3")
    monkeypatch.setattr(desktop, "RECORDS", records)
    records.add_step("b", "шаг", session_key="s", minutes=15, started_epoch=time.time()-100,
                     deadline_epoch=time.time()+800)
    records.finish_step("b", "done", "вынес различие")
    store = SandStore(records.path)
    edge = store.pin("s", "трудное ребро")
    store.guess(edge["id"], "open", "  первая\n ", "unchecked")
    rung = store.add_rung("s", SimpleNamespace(start="A", step="B", text="ступенька"), edge["id"])
    def check(text, material):
        assert store.guesses(edge["id"])[-1]["verdict"] == "pending"
        assert text == "  финальная\n "
        return SimpleNamespace(status="confirmed", text="Согласуется с источником")
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.service = SimpleNamespace(check_prediction=check)
    app._commit_prediction = lambda _: None
    app._universe = lambda: None
    app._choose = lambda *a, **k: "next"
    monkeypatch.setattr(desktop, "popup_input", lambda *a, **k: "  финальная\n ")
    monkeypatch.setattr(desktop.context, "pack", lambda *a, **k: None)
    shown = []
    monkeypatch.setattr(desktop, "run_popup", lambda payload, **k: shown.append(payload))
    app._sand_close("s", records.session_steps("s"), [])
    assert store.edge("s") is None
    assert store.guesses(edge["id"])[-1]["verdict"] == "confirmed"
    assert shown[-1]["mode"] == "cutscene"
    assert "не оценка освоения" in shown[-1]["scene"]["stars"][-1]["text"]
    with store.db() as db:
        assert db.execute("SELECT status FROM sand_rungs WHERE id=?", (rung,)).fetchone()[0] == "read"
        assert db.execute("SELECT count(*) FROM sand_scenes").fetchone()[0] == 1


def test_unknown_window_does_not_change_clipboard(monkeypatch):
    monkeypatch.setattr(capture, "active_window", lambda: {})
    monkeypatch.setattr(capture, "cursor_pos", lambda: None)
    monkeypatch.setattr(capture, "save_clipboard", lambda: pytest.fail("must not inspect or change clipboard without a target"))
    assert not capture.capture_selection().text


def test_capture_restores_clipboard_when_copy_raises(monkeypatch):
    monkeypatch.setattr(capture, "active_window", lambda: {"class":"okular","address":"0xabc"})
    monkeypatch.setattr(capture, "cursor_pos", lambda: None)
    saved = capture.SavedClipboard("text/plain", b"previous")
    monkeypatch.setattr(capture, "save_clipboard", lambda: saved)
    monkeypatch.setattr(capture, "read_clipboard_text", lambda: "previous")
    monkeypatch.setattr(capture, "_run", lambda *_: (_ for _ in ()).throw(RuntimeError("command failure")))
    restored = []
    monkeypatch.setattr(capture, "restore_clipboard", restored.append)
    with pytest.raises(RuntimeError):
        capture.capture_selection()
    assert restored == [saved]


def test_capture_never_clears_an_unknown_format(monkeypatch):
    monkeypatch.setattr(capture, "active_window", lambda: {"class":"okular","address":"0xabc"})
    monkeypatch.setattr(capture, "cursor_pos", lambda: None)
    monkeypatch.setattr(capture, "save_clipboard", lambda: capture.SavedClipboard())
    monkeypatch.setattr(capture, "_run", lambda *_: pytest.fail("do not mutate an unsupported format"))
    assert not capture.capture_selection().text


def test_particle_renderer_outputs_transparent_svg():
    sand = Sand(1)
    sand.lift([(20, 20), (40, 40)], mine=True)
    svg = svg_particles(sand)
    assert svg.count("<circle") == 2
    assert "#d6ad79" in svg and "<rect" not in svg


def test_ready_and_direct_dual_routes_use_sand_before_legacy_assets(monkeypatch, tmp_path):
    from cognitive_popups import popup_helper, sand_cues
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    (tmp_path / "sand.enabled").touch()
    seen = []
    monkeypatch.setattr(sand_cues,"show",lambda *args: seen.append(args) or 0)
    for n in (1,3,4,6):
        items = [{"simple":str(i),"term":"term"} for i in range(n)]
        assert popup_helper.show_orbital_popup(items,1,2,"title",True,[{"action":"ladder"}],"cache") == 0
        assert seen[-1] == (items,1,2,"title",True,[{"action":"ladder"}],"cache")
        assert popup_helper.show_dual_popup(items,1,2,"title") == 0
        assert seen[-1] == (items,1,2,"title")


@pytest.mark.parametrize("count",[1,3,4,6])
@pytest.mark.parametrize("anchor",[(999,999),(150,210),(300,590),(400,400)])
def test_sand_words_plan_fits_workarea(count, anchor):
    from cognitive_popups.sand_cues import plan, detail_box
    area = (100,200,900,500)
    p = plan(area,anchor,count)
    x,y = p.position
    assert 100 <= x and x+p.width <= 1000 and 200 <= y and y+p.height <= 700
    assert len(p.words) == len(p.sides) == count
    for i,(cx,cy,w) in enumerate(p.words):
        assert 0 <= cx-w/2 and cx+w/2 <= p.width and 0 <= cy <= p.height
        d = detail_box(p,i,500)
        assert 0 <= d.x and d.x+d.width <= p.width and 0 <= d.y and d.y+d.height <= p.height


def test_sand_words_sit_at_the_corners_of_the_selection():
    from cognitive_popups.sand_cues import plan, detail_box
    selection = (600,400,1100,430)
    p = plan((0,0,1920,1080),(1100,415),4,0,selection)
    ox,oy = p.position
    (tl,tr,bl,br) = [(cx+ox,cy+oy) for cx,cy,_ in p.words]
    assert p.sides == (-1,-1,1,1) and p.found
    assert tl[1] < 400 and tr[1] < 400 and bl[1] > 430 and br[1] > 430   # above / below
    assert tl[0] < 850 < tr[0] and bl[0] < 850 < br[0]                     # left / right
    assert abs(tl[0]-(600+115)) < 2 and abs(tr[0]-(1100-115)) < 2          # over the ends
    up, down = detail_box(p,0,120), detail_box(p,3,120)
    assert up.y+up.height+oy < tl[1] and down.y+oy > br[1]                 # explanations open outward
    src = p.source
    assert (src.x+ox, src.y+oy, src.x+src.width+ox, src.y+src.height+oy) == selection


def test_short_selection_pushes_words_apart():
    from cognitive_popups.sand_cues import plan, COL_W
    p = plan((0,0,1920,1080),(900,500),4,0,(880,490,930,510))
    (l,_,_),(r,_,_) = p.words[0], p.words[1]
    assert r-l >= COL_W


def test_magnet_pulls_toward_pointer_bounded_and_rests_after():
    from cognitive_popups.sand_cues import Magnet
    magnet = Magnet(1)
    for _ in range(120):
        magnet.step(1.0,(100,100),(160,130),(560,420),1.0)
    assert 3 < magnet.x < 25 and 0 < magnet.y < 25 and magnet.s > 1 and magnet.r > 0
    for _ in range(200):
        magnet.step(1.0,(100,100),None,(560,420),0.0)
    assert magnet.resting()
    x,y = magnet.place(100,100,120,100)
    assert abs(x-120) < .5 and abs(y-100) < .5


def test_word_grains_follow_their_magnet():
    p, sand = _formed()
    for frame in range(120):
        sand.land_due(); sand.step(1.0)
    cx,cy,_ = p.words[0]
    pointer = (cx+60,cy+20,0.0,0.0)
    before = sum(g.x for g in sand.grains if g.g == 0 and g.r == 0)
    for frame in range(90):
        sand.step(1.0,mouse=pointer,pull=1.0)
    after = sum(g.x for g in sand.grains if g.g == 0 and g.r == 0)
    assert sand.magnets[0].x > 2 and after > before
    assert not sand.still()


def _formed(count=4, seed=3):
    from cognitive_popups.sand_cues import WordSand, plan
    p = plan((0,0,1600,1000),(800,400),count)
    sand = WordSand(p,seed=seed,count=count)
    mains = [[(cx+dx,cy) for dx in range(-20,21)] for cx,cy,_ in p.words]
    sand.words(mains)
    return p, sand


def test_sand_words_lift_core_then_land_one_after_another():
    p, sand = _formed()
    modes, landings = [], []
    for frame in range(240):
        landings += sand.land_due()
        sand.step(1.0)
        modes.append(sand.mode)
    assert modes[0] == "lift" and "form" in modes
    assert modes.index("form") >= 46  # lift and a short core are never skipped
    assert landings == [0,1,2,3]
    word = [g for g in sand.grains if g.g == 0 and g.r == 0]
    cx,cy,_ = p.words[0]
    assert sum(abs(g.y-cy) < 6 and abs(g.x-cx) < 26 for g in word) > .9*len(word)


def test_sand_words_calm_down_to_stillness_and_stay_lit():
    _, sand = _formed()
    for frame in range(300):
        sand.land_due()
        sand.step(1.0, calm=1.0 if frame > 120 else 0.0)
    assert sand.still()
    buckets = sand.buckets(calm=1.0)
    assert sum(len(b) for b in buckets) == len(sand.grains)


def test_sand_words_disclosure_sends_reserve_into_text_and_stream_flows():
    from cognitive_popups.sand_cues import RESERVE, MAIN, PER_WORD
    from cognitive_popups.sand_cues import detail_box
    p, sand = _formed()
    for frame in range(150):
        sand.land_due(); sand.step(1.0)
    d = detail_box(p,1,120)
    sand.disclose(1,[(d.x+10+i%200,d.y+8) for i in range(500)])
    cx,cy,_ = p.words[1]
    streams = [None,((cx,cy+20),(cx,cy+40),(d.x+d.width/2,d.y-4)),None,None]
    for frame in range(150):
        sand.step(1.0,streams=streams)
    reserve = [g for g in sand.grains if g.g == 1 and g.r == 1]
    assert len(reserve) == round(len(sand.grains)//4*RESERVE/PER_WORD)
    assert sum(abs(g.y-(d.y+8)) < 6 for g in reserve) > .85*len(reserve)
    flow = sand.flow
    sand.step(1.0,streams=streams,calm=1.0)
    assert sand.flow == flow  # calm freezes the stream


def test_sand_words_return_to_the_selection_and_finish():
    _, sand = _formed()
    for frame in range(120):
        sand.land_due(); sand.step(1.0)
    sand.back()
    for frame in range(80):
        sand.step(1.0)
    assert sand.done
    near = sum(abs(g.x-g.sx)+abs(g.y-g.sy) < 4 for g in sand.grains)
    assert near > .8*len(sand.grains)


def test_sand_words_regroup_for_other_word_counts():
    _, sand = _formed(count=3)
    assert {g.g for g in sand.grains} == {0,1,2}
    assert sand.count == 3 and sand.landed == [False]*3


def test_waiting_sand_hands_the_words_to_the_same_window(tmp_path, monkeypatch):
    from threading import Event
    from cognitive_popups import sand_runtime
    release = Event()
    class Proc:
        stdin = SimpleNamespace(write=lambda *_: None, close=lambda: None)
        def poll(self): return None
        def wait(self, timeout=None):
            waits.append(timeout)
            assert release.wait(2)
    waits = []
    monkeypatch.setattr(sand_runtime.subprocess,"Popen",lambda *a,**k: Proc())
    handle = sand_runtime.SandHandle(["x"],{},tmp_path)
    assert handle.alive()
    path = handle.path
    assert handle.finish(["a"],"done",{"mode":"dual","items":[{"simple":"a"}]})
    message = json.loads(path.read_text())
    release.set()
    import time
    deadline = time.monotonic() + 2
    while not waits and time.monotonic() < deadline:
        time.sleep(.01)
    assert waits == [None]
    assert message["payload"]["items"][0]["simple"] == "a"


def test_cancelled_wait_never_adopts_words(tmp_path, monkeypatch):
    from cognitive_popups import sand_runtime
    class Proc:
        stdin = SimpleNamespace(write=lambda *_: None, close=lambda: None)
        def poll(self): return None
        def wait(self, timeout=None): pass
    monkeypatch.setattr(sand_runtime.subprocess,"Popen",lambda *a,**k: Proc())
    handle = sand_runtime.SandHandle(["x"],{},tmp_path)
    handle.path.write_text('{"status":"cancelled"}')
    assert not handle.alive()
    assert not handle.finish(["a"],"done",{"mode":"dual"})


def test_sand_hourglass_is_transparent_and_reallocates_grains():
    from cognitive_popups.focus_timer import FocusTimer
    early = FocusTimer._sand_svg(200,132,750,1000)
    late = FocusTimer._sand_svg(200,132,250,1000)
    assert '<rect' not in early and '<image' not in early
    assert early.count('data-chamber="upper"') == 75
    assert late.count('data-chamber="upper"') == 25
    assert early.count('data-chamber="lower"') == 25


def test_scene_has_no_opaque_screen_background():
    from dataclasses import asdict
    scene = asdict(cutscene_model.build_scene(cutscene_model.DaySummary("x",["a"],[])))
    assert '<rect' not in scene_svg(scene,0)




def test_palette_brightness_reads_the_covered_area_of_a_screenshot():
    from cognitive_popups.sand_surface import _ppm_brightness
    width, height = 4, 2
    # left half black, right half white
    row = bytes([0,0,0]*2 + [255,255,255]*2)
    data = b"P6\n# grim\n%d %d\n255\n" % (width, height) + row * height
    assert _ppm_brightness(data,(0,0,.5,1)) == 0
    assert _ppm_brightness(data,(.5,0,1,1)) > 250


def _mixer(tmp_path):
    import wave
    from cognitive_popups import sand_audio
    with wave.open(str(tmp_path / "tick.wav"), "wb") as handle:
        handle.setnchannels(1); handle.setsampwidth(2); handle.setframerate(sand_audio.RATE)
        handle.writeframes(b"\x00\x40" * 400)  # constant 0.5
    mixer = sand_audio.Mixer(command=["cat"], assets=tmp_path, volume=1.0)
    mixer.closed = True  # mix by hand, no stream thread output needed
    mixer.thread.join(1)
    return sand_audio, mixer


def test_mixer_pans_and_delays_voices(tmp_path):
    from array import array
    sand_audio, mixer = _mixer(tmp_path)
    mixer.closed = False
    mixer.play("tick", gain=1.0, pan=-1.0)
    mixer.play("tick", gain=1.0, pan=1.0, delay=100 / sand_audio.RATE)
    block = array("h", mixer.mix(256))
    left, right = block[0::2], block[1::2]
    assert left[0] > 16000 and right[0] < 100       # hard left first
    assert right[99] < 100 and right[150] > 16000   # the right voice starts after its delay
    mixer.close()


def test_mixer_drops_finished_voices_and_soft_clips(tmp_path):
    from array import array
    sand_audio, mixer = _mixer(tmp_path)
    mixer.closed = False
    for _ in range(6):
        mixer.play("tick", gain=1.0)
    block = array("h", mixer.mix(256))
    assert max(block) < 32767
    mixer.mix(256)
    assert not mixer.busy()
    mixer.close()


def test_sand_sound_respects_mute_and_global_off(tmp_path, monkeypatch):
    from cognitive_popups import sand_audio
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    (tmp_path / "sand.enabled").touch()
    monkeypatch.delenv("COGNITIVE_SOUND", raising=False)
    assert sand_audio.enabled()
    (tmp_path / "sand.silent").touch()
    assert not sand_audio.enabled()
    (tmp_path / "sand.silent").unlink()
    monkeypatch.setenv("COGNITIVE_SOUND", "off")
    assert not sand_audio.enabled()


def _alphas(buckets):
    from cognitive_popups.sand_cues import LEVELS
    return [(i + .5) / LEVELS for i, bucket in enumerate(buckets) for _ in bucket]


def test_lift_pollen_is_gone_by_half_a_second():
    from cognitive_popups.sand_cues import WordSand, plan
    sand = WordSand(plan((0,0,1600,1000),(800,400)), seed=1)
    for _ in range(31):  # 0.52 s
        sand.step(1.0)
    assert sand.mode in ("lift", "core")
    assert sum(len(b) for b in sand.buckets()) < .05 * len(sand.grains)


def test_landed_word_sand_settles_to_dust_and_shines_only_in_motion():
    p, sand = _formed()
    for _ in range(200):
        sand.land_due(); sand.step(1.0, calm=1.0)
    resting = _alphas(sand.buckets())
    assert resting and max(resting) < .35          # dust under the real text
    for g in sand.grains[:200]:
        g.vx = 3.0
    moving = _alphas(sand.buckets())
    assert max(moving) > .9                         # sand is bright while it moves


def test_page_colour_and_ink():
    from cognitive_popups.sand_screen import Shot, mean_colour, ink_for, DARK_INK, LIGHT_INK
    shot = Shot(bytes([250,250,250])*100 + bytes([20,22,26])*100, 10, 20)
    light = mean_colour(shot,(0,0,10,10)); dark = mean_colour(shot,(0,10,10,10))
    assert ink_for(light) == DARK_INK and ink_for(dark) == LIGHT_INK
    assert mean_colour(shot,(50,50,5,5)) is None


def test_sand_only_words_keep_their_grains_lit_at_rest():
    p, sand = _formed()
    for _ in range(200):
        sand.land_due(); sand.step(1.0, calm=1.0)
    assert min(_alphas(sand.buckets(dust=False))) > .9



def _page(width, height, rows):
    """White page with highlighted rectangles: rows = [(x0, y0, x1, y1, colour)]."""
    from cognitive_popups.sand_screen import Shot
    data = bytearray(b"\xff" * (width * height * 3))
    for x0, y0, x1, y1, colour in rows:
        for y in range(y0, y1):
            data[(y * width + x0) * 3:(y * width + x1) * 3] = bytes(colour) * (x1 - x0)
    return Shot(bytes(data), width, height)


def test_selection_spans_lines_with_a_thin_gap_but_not_paragraphs():
    from cognitive_popups.sand_screen import find_selection
    grey = (200, 200, 205)
    shot = _page(800, 400, [(100, 100, 700, 124, grey), (100, 127, 300, 151, grey),   # two lines, 3 px gap
                            (100, 180, 700, 204, grey)])                              # next paragraph
    box, colour = find_selection(shot, 295, 140)
    assert box[1] <= 100 and box[3] >= 148 and box[2] >= 696   # both lines, full width
    assert box[3] < 180                                         # not the next paragraph


def test_sand_only_words_are_sharp_within_a_third_of_a_second():
    from cognitive_popups.sand_cues import WordSand, plan
    p = plan((0,0,1600,1000),(800,400)); s = WordSand(p, seed=3)
    s.words([[(cx+dx,cy+dy) for dx in range(-40,41,2) for dy in (-6,0,6)] for cx,cy,_ in p.words])
    mouse = (p.words[0][0]+60, p.words[0][1]+20, .5, .2)
    while not (s.mode == "form" and all(s.landed) and s.ef - s.land_at[-1] > .3):
        s.land_due(); s.step(1.0, mouse=mouse, pull=1.0, crisp=True)
    good = total = 0
    for g in s.grains:
        if g.r == 0:
            cx, cy, _ = p.words[g.g]
            x, y = s.magnets[g.g].place(cx, cy, g.ax, g.ay)
            total += 1; good += abs(g.x-x)+abs(g.y-y) < 1.5
    assert good / total > .85


def test_page_colour_ignores_the_selection_highlight():
    from cognitive_popups.sand_screen import mean_colour
    shot = _page(60, 30, [(0, 0, 40, 30, (200, 190, 240))])   # highlight over most of a white page
    colour = mean_colour(shot, (0, 0, 60, 30), exclude=(200, 190, 240))
    assert min(colour) > 250
    assert mean_colour(shot, (0, 0, 30, 30), exclude=(200, 190, 240))[0] == 200   # only highlight: keep it
