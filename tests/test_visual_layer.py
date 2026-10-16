"""The visual layer as one system: tokens, reduced motion, the two cutscenes."""
from __future__ import annotations

import importlib.util
import os
import re
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from xml.etree import ElementTree

import pytest

from cognitive_popups import cutscene_model, motion, paper_scene, session_card, theme

PACKAGE = Path(theme.__file__).resolve().parent
#: Modules of the visual layer; none of them may carry its own colour literal.
VISUAL_MODULES = (
    "sand_cues", "sand_fx", "sand_canvas", "sand_surface", "sand_screen", "sand_audio", "sand_core",
    "sand_runtime", "sand_store", "sand_cli", "focus_timer", "hud", "orbital", "orbital_assets",
    "orbital_motion", "initium_star", "initium", "cutscene_model", "session_scene", "sound",
    "render_sounds", "motion", "paper_scene", "session_card",
)
HEX = re.compile(r"#[0-9a-fA-F]{6}\b")


# --- tokens ---------------------------------------------------------------

@pytest.mark.parametrize("tone", ["dark", "light"])
def test_every_text_pair_reads_at_four_and_a_half_to_one(tone):
    colours = theme.palette(tone)
    for fg, bg in theme.TEXT_PAIRS:
        assert theme.contrast(colours[fg], colours[bg]) >= 4.5, (tone, fg, bg)


def test_contrast_and_luminance_match_wcag_endpoints():
    assert theme.contrast("#000000", "#ffffff") == pytest.approx(21.0)
    assert theme.contrast("#777777", "#777777") == pytest.approx(1.0)
    assert theme.contrast((255, 255, 255), "#000000") == pytest.approx(21.0)


def test_palette_follows_the_page_brightness():
    assert theme.pick("#ffffff") is theme.LIGHT and theme.pick((250, 248, 240)) is theme.LIGHT
    assert theme.pick("#101010") is theme.DARK and theme.pick(None) is theme.DARK
    assert theme.pick(200) is theme.LIGHT and theme.pick(40) is theme.DARK


def test_one_amber_one_family_and_a_scale_from_eleven_px():
    assert theme.DARK["accent"] == "#d6ad79"
    assert theme.DARK["own_words"] == "#9fc3e8"
    assert min(theme.SIZES) == theme.MIN_TEXT_PX == 11
    assert theme.pango(theme.SMALL).startswith("Noto Sans, Sans") and theme.pango(13).endswith("13px")
    assert theme.pango(18, "SemiBold", px=False) == "Noto Sans, Sans SemiBold 18"
    assert "font-size: 11px" in theme.css_font(9)  # never below the floor
    # The calm miss highlight is not red.
    for tone in ("dark", "light"):
        r, g, b = theme.rgb255(theme.palette(tone)["miss"])
        assert not (r > g + 60 and r > b + 60)


def test_no_colour_literal_outside_theme():
    offenders = []
    for name in VISUAL_MODULES:
        path = PACKAGE / f"{name}.py"
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if HEX.search(line):
                offenders.append(f"{path.name}:{number}: {line.strip()}")
    assert not offenders, "\n".join(offenders)


def test_no_font_family_literal_outside_theme():
    pattern = re.compile(r"""["'](?:Noto Sans|Sans(?: SemiBold)?)\b[^"']*["']""")
    offenders = [f"{name}.py:{number}"
                 for name in VISUAL_MODULES
                 for number, line in enumerate((PACKAGE / f"{name}.py").read_text(encoding="utf-8").splitlines(), 1)
                 if pattern.search(line)]
    assert not offenders, offenders


# --- reduced motion ---------------------------------------------------------

@pytest.fixture
def calm_env(monkeypatch, tmp_path):
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    monkeypatch.delenv("COGNITIVE_REDUCED_MOTION", raising=False)
    monkeypatch.delitem(sys.modules, "gi.repository.Gtk", raising=False)
    return tmp_path


def _gtk(animations: bool):
    settings = SimpleNamespace(get_property=lambda name: animations)
    return SimpleNamespace(Settings=SimpleNamespace(get_default=lambda: settings))


def test_motion_is_full_unless_something_asks_for_calm(calm_env):
    assert not motion.reduced()
    assert not motion.reduced(_gtk(True))
    assert motion.duration(.6) == .6


@pytest.mark.parametrize("value", ["1", "on", "yes", "TRUE"])
def test_environment_reduces_motion(calm_env, monkeypatch, value):
    monkeypatch.setenv("COGNITIVE_REDUCED_MOTION", value)
    assert motion.reduced()
    assert motion.duration(.6) == 0


@pytest.mark.parametrize("flag", ["motion.reduced", "sand.reduced"])
def test_flag_files_reduce_motion(calm_env, flag):
    (calm_env / flag).touch()
    assert motion.reduced()


def test_gtk_setting_reduces_motion_and_odd_stubs_do_not(calm_env):
    assert motion.reduced(_gtk(False))
    assert not motion.reduced(SimpleNamespace())  # no Settings at all


def test_flag_lives_in_the_shared_state_dir_not_the_cwd(calm_env, monkeypatch):
    elsewhere = calm_env / "cwd"
    (elsewhere / "var").mkdir(parents=True)
    (elsewhere / "var" / "sand.reduced").touch()
    from cognitive_popups.sand_fx import reduced_motion
    before = os.getcwd()
    os.chdir(elsewhere)
    try:
        assert not reduced_motion()  # sand_fx no longer resolves "var" from the cwd
        motion.set_reduced(True)
        assert reduced_motion() and (calm_env / "motion.reduced").exists()
        motion.set_reduced(False)
        assert not reduced_motion()
    finally:
        os.chdir(before)


def test_orbital_tween_lands_at_once_when_reduced(calm_env, monkeypatch):
    from cognitive_popups.orbital_motion import Tween
    moving = Tween((0., 0.), (10., 20.), 5., .22)
    assert moving.value(5.) == (0., 0.) and not moving.done(5.)
    monkeypatch.setenv("COGNITIVE_REDUCED_MOTION", "1")
    still = Tween((0., 0.), (10., 20.), 5., .22)
    assert still.value(5.) == (10., 20.) and still.done(5.)
    still.retarget((1., 2.), 6., .3)
    assert still.value(6.) == (1., 2.)


def test_initium_birth_is_skipped_when_reduced():
    from cognitive_popups.initium import birth_plays
    assert birth_plays(True, False)
    assert not birth_plays(True, True) and not birth_plays(False, False)


def test_sand_settle_jumps_to_rest_when_reduced():
    from cognitive_popups.sand_core import Sand
    sand = Sand(3)
    sand.reduced_motion = True
    sand.lift([(40 + i, 95) for i in range(20)], mine=True)
    sand.settle((315, 110))
    sand.step(1.0)
    assert sand.particles
    assert all(abs(p.x - 315) <= 1.2 and abs(p.y - 110) <= 1.2 for p in sand.particles)


# --- per aspera, capped ------------------------------------------------------

def _day(concepts=8):
    names = [f"понятие {i}" for i in range(concepts)]
    thoughts = [cutscene_model.Thought(names[-1], "мысль", "err")] if names else []
    return cutscene_model.DaySummary("2026-10-16", names, [(names[0], n) for n in names[1:]],
                                     thoughts=thoughts, far_edge=None if names else "дальше")


@pytest.mark.parametrize("count", [0, 1, 5, 16, 40])
def test_session_cutscene_lasts_at_most_four_seconds(count):
    scene = cutscene_model.build_scene(_day(count))
    assert scene.timeline["end"] <= 4.0
    assert all(star.ignite < scene.timeline["title"] < scene.timeline["end"] for star in scene.stars)


def test_cutscene_playhead_skips_and_respects_reduced_motion():
    end = cutscene_model.T_END
    assert cutscene_model.playhead(1.0) == 1.0
    assert cutscene_model.playhead(99.0) == end
    assert cutscene_model.playhead(0.0, reduced=True) == end
    assert cutscene_model.playhead(0.2, skipped=True) == end


def test_legacy_saved_scene_is_squeezed_with_every_star_lit():
    legacy = {"timeline": {"lift": 4.2, "form": 8.3, "title": 13.9, "end": 17.5},
              "stars": [{"label": "a", "ignite": 8.3}, {"label": "b", "ignite": 12.0}]}
    fitted = cutscene_model.fit_timeline(legacy)
    assert fitted["timeline"]["end"] == pytest.approx(4.0)
    assert [s["ignite"] for s in fitted["stars"]] == sorted(s["ignite"] for s in fitted["stars"])
    assert all(s["ignite"] < fitted["timeline"]["end"] for s in fitted["stars"])
    short = {"timeline": {"end": 3.0}, "stars": []}
    assert cutscene_model.fit_timeline(short) is short


def test_cutscene_svg_uses_theme_tokens_only():
    from cognitive_popups.sand_fx import scene_svg
    from dataclasses import asdict
    scene = asdict(cutscene_model.build_scene(_day(4)))
    svg = scene_svg(scene, scene["timeline"]["end"])
    colours = set(HEX.findall(svg))
    allowed = set(theme.DARK.values()) | {t.lower() for t in theme.STAR_TINTS}
    assert colours <= allowed
    assert theme.DARK["miss"] in colours  # the saved mistake stands out, calmly


# --- cutscene A: на бумагу ---------------------------------------------------

WORDS = ["регламент", "программа", "должность", "юрисдикция"]


def test_paper_frame_starts_in_the_cluster_and_ends_in_the_corners():
    box = (560, 360)
    first, last = paper_scene.frame(0, WORDS, box), paper_scene.frame(1, WORDS, box)
    words0 = [s for s in first if s.kind == "word"]
    words1 = [s for s in last if s.kind == "word"]
    assert [s.text for s in words1] == WORDS
    cx, cy = box[0] / 2, box[1] / 2
    for s in words0:  # each word touches the centre gap
        assert abs(s.x + s.w / 2 - cx) < s.w and abs(s.y + s.h / 2 - cy) < s.h
    m = paper_scene.MARGIN
    tl, tr, bl, br = words1
    assert (tl.x, tl.y) == (m, m)
    assert tr.x + tr.w == pytest.approx(box[0] - m) and tr.y == m
    assert bl.x == m and bl.y + bl.h == pytest.approx(box[1] - m)
    assert br.x + br.w == pytest.approx(box[0] - m) and br.y + br.h == pytest.approx(box[1] - m)
    assert all(s.alpha == 0 for s in last)  # then everything has faded


@pytest.mark.parametrize("box", [(560, 360), (420, 260), (900, 500)])
@pytest.mark.parametrize("words", [WORDS, ["a", "b", "c", "d"], ["очень длинное слово " * 4] * 4])
def test_paper_frame_never_overlaps_once_settled(box, words):
    for t in (paper_scene.SETTLED_T, 1.0):
        shapes = paper_scene.frame(t, words, box)
        for i, a in enumerate(shapes):
            assert 0 <= a.x and a.x + a.w <= box[0] + .01 and 0 <= a.y and a.y + a.h <= box[1] + .01
            for b in shapes[i + 1:]:
                assert not paper_scene.overlaps(a, b), (a, b)


def test_paper_frame_is_deterministic_and_moves_only_forward():
    assert paper_scene.frame(.37, WORDS) == paper_scene.frame(.37, WORDS)
    xs = [paper_scene.frame(t / 20, WORDS)[0].x for t in range(21)]
    assert xs == sorted(xs, reverse=True)  # top-left word heads left, never back


def test_paper_centre_line_fades_in_then_everything_fades_out():
    line = lambda t: paper_scene.frame(t, WORDS)[-1]
    assert line(0).alpha == 0 and line(0).text == paper_scene.CENTRE_LINE
    assert line(paper_scene.SETTLED_T).alpha == 1
    assert 0 < line((paper_scene.SETTLED_T + 1) / 2).alpha < 1
    assert paper_scene.DURATION_S <= 1.0


def test_paper_scene_reduced_and_skipped_go_to_final_positions():
    assert paper_scene.playhead(0.0, skipped=True) == 1.0
    settled = paper_scene.playhead(0.0, reduced=True)
    assert settled == paper_scene.SETTLED_T
    assert paper_scene.playhead(paper_scene.REDUCED_HOLD_S, reduced=True) == 1.0
    held = [s.rect() for s in paper_scene.frame(settled, WORDS)]
    final = [s.rect() for s in paper_scene.frame(1.0, WORDS)]
    assert held == final  # nothing moves under reduced motion
    assert paper_scene.playhead(paper_scene.DURATION_S / 2) == pytest.approx(.5)


def test_paper_scene_handles_fewer_words_and_stays_on_screen():
    assert [s.kind for s in paper_scene.frame(1, ["одно"])] == ["word", "line"]
    assert paper_scene.place_box((5, 5), (0, 0, 1920, 1080)) == (0, 0, 560, 360)
    assert paper_scene.place_box((1900, 1070), (0, 0, 1920, 1080)) == (1360, 720, 560, 360)
    assert paper_scene.place_box((50, 50), (0, 0, 300, 200)) == (0, 0, 300, 200)


def test_paper_scene_play_without_gtk_finishes_at_once(monkeypatch):
    monkeypatch.setitem(sys.modules, "gi", None)
    done = []
    assert paper_scene.play(WORDS, (100, 100), on_done=lambda: done.append(True)) is None
    assert done == [True]


# --- cutscene B: итог сессии -------------------------------------------------

HYPOTHESES = [
    {"hypothesis": "регламент задаёт программу должности", "status": "contradicted", "term": "регламент"},
    {"hypothesis": "юрисдикция ограничивает программу", "status": "confirmed", "term": "юрисдикция"},
    {"hypothesis": "должность — это роль в процессе", "status": "vague", "terms": ["регламент"]},
]
NOTES = [{"comment": "не понял, кто утверждает регламент"}]


def test_card_is_deterministic_valid_svg_without_percent():
    svg = session_card.build_card(HYPOTHESES, NOTES, "session-42", "регламент — рамка, программа — путь")
    assert svg == session_card.build_card(HYPOTHESES, NOTES, "session-42", "регламент — рамка, программа — путь")
    assert svg != session_card.build_card(HYPOTHESES, NOTES, "session-43", "регламент — рамка, программа — путь")
    root = ElementTree.fromstring(svg)
    assert root.tag.endswith("svg")
    assert "%" not in svg
    assert "регламент" in root.find("{http://www.w3.org/2000/svg}title").text
    assert set(HEX.findall(svg)) <= set(theme.DARK.values())


def test_card_marks_misses_bright_and_links_shared_words():
    svg = session_card.build_card(HYPOTHESES, NOTES, "k", None)
    root = ElementTree.fromstring(svg)
    ns = "{http://www.w3.org/2000/svg}"
    kinds = [g.get("data-kind") for g in root.iter(f"{ns}g")]
    assert kinds == ["miss", "confirmed", "open"]
    miss = next(g for g in root.iter(f"{ns}g") if g.get("data-kind") == "miss")
    assert len(miss.findall(f"{ns}line")) == 4  # the ray
    assert session_card.links(HYPOTHESES) == [(0, 1), (0, 2)]  # «программу», «должность»


def test_card_name_is_the_most_frequent_term():
    assert session_card.constellation_name(HYPOTHESES, NOTES) == "регламент"
    assert session_card.constellation_name([{"text": "x y"}], []) == session_card.FALLBACK_TITLE


def test_empty_session_gets_a_calm_card():
    svg = session_card.build_card([], NOTES, "quiet", None)
    root = ElementTree.fromstring(svg)
    assert session_card.EMPTY_TITLE in svg
    assert "%" not in svg and not root.findall("{http://www.w3.org/2000/svg}g")
    for word in ("ни одной", "нет гипотез", "0"):
        assert word not in "".join(root.itertext())


def test_card_escapes_reader_text_and_percent():
    svg = session_card.build_card([{"text": "<b>50% & больше</b>", "status": "contradicted"}], [], "k", "100%")
    ElementTree.fromstring(svg)
    assert "%" not in svg and "<b>" not in svg


def test_save_card_files_by_date_and_key(tmp_path):
    svg = session_card.build_card(HYPOTHESES, [], "session-42", None)
    path = session_card.save_card(svg, tmp_path, day="2026-10-16")
    assert path.parent == tmp_path / "sky"
    assert re.fullmatch(r"2026-10-16-[0-9a-f]{8}\.svg", path.name)
    assert path.read_text(encoding="utf-8") == svg
    assert session_card.save_card(svg, tmp_path, day="2026-10-16") == path  # no duplicates
    assert len(list((tmp_path / "sky").iterdir())) == 1


def test_card_viewer_fades_within_a_second_and_a_half():
    assert session_card.fade_alpha(0) == 0 and session_card.fade_alpha(1.5) == 1
    assert session_card.fade_alpha(0, reduced=True) == 1
    assert session_card.show_card.__doc__


# --- hud compatibility ----------------------------------------------------------

@pytest.fixture
def hud(desktop, monkeypatch):
    monkeypatch.setattr(sys.modules["gi.repository"].Gtk, "Box", object, raising=False)
    path = PACKAGE / "hud.py"
    spec = importlib.util.spec_from_file_location("cognitive_popups._visual_hud", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_hud_signals_prefer_glibunix_and_fall_back(hud, monkeypatch):
    calls = []
    glib = SimpleNamespace(PRIORITY_DEFAULT=0,
                           unix_signal_add=lambda *args: calls.append(("old", args[1])) or 1)
    repo = sys.modules["gi.repository"]
    monkeypatch.setattr(repo, "GLibUnix", SimpleNamespace(signal_add=lambda *args: calls.append(("new", args[1])) or 2),
                        raising=False)
    assert hud.signal_add(10, print, glib=glib) == 2
    monkeypatch.delattr(repo, "GLibUnix")
    assert hud.signal_add(15, print, glib=glib) == 1
    assert calls == [("new", 10), ("old", 15)]


def test_hud_css_uses_theme_family_and_no_tiny_text(hud):
    css = hud.CSS.decode()
    assert "Noto Sans" in css
    sizes = [int(px) for px in re.findall(r"font-size:\s*(\d+)px", css)]
    assert sizes and min(sizes) >= theme.MIN_TEXT_PX
