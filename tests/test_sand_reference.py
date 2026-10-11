"""Тесты референс-модулей. Запуск: python3 -m pytest reference/tests  или  python3 reference/tests/test_reference.py"""
import random
import sys
from pathlib import Path



from cognitive_popups import capture
from cognitive_popups import cutscene_model as cm
from cognitive_popups import instant_cues as ic
from cognitive_popups.sand_core import Sand, sample_mask

GALOIS = ("Уравнение разрешимо в радикалах тогда и только тогда, когда его группа Галуа разрешима. "
          "Группа Галуа состоит из перестановок корней, которые сохраняют все алгебраические соотношения "
          "между корнями. Перестановки корней, не сохраняющие соотношения, в группу Галуа не входят.")


def test_cues_are_four_and_distinct():
    r = ic.draft_cues(GALOIS)
    terms = [c["term"] for c in r["cues"]]
    assert 1 <= len(terms) <= 4
    stems = [ic.stem(t.split()[0]) for t in terms]
    assert len(set(stems)) == len(stems), terms


def test_background_lowers_common_words():
    bg = ic.build_background(["Система системе системы время временем со временем"] * 50)
    text = "Энтропия в системе со временем растёт. Энтропия замкнутой системы не убывает."
    terms = [c["term"].lower() for c in ic.draft_cues(text, background=bg)["cues"]]
    assert terms[0].startswith("энтроп"), terms


def test_symbol_definitions():
    t = "Здесь ψ " + ic.DASH + " волновая функция, а ħ " + ic.DASH + " постоянная Планка."
    syms = {s["symbol"]: s["meaning"] for s in ic.extract_symbols(t)}
    assert syms.get("ψ", "").startswith("волновая"), syms
    assert syms.get("ħ", "").startswith("постоянная"), syms


def test_edges_from_shared_sentences():
    r = ic.draft_cues(GALOIS)
    assert all(e["weight"] >= 1 for e in r["edges"])


def test_forms():
    assert cm.classify_form(5, [(0, 1), (0, 2), (0, 3), (0, 4)]) == "hub"
    assert cm.classify_form(5, [(0, 1), (1, 2), (2, 3), (3, 4)]) == "chain"
    assert cm.classify_form(5, [(0, 1), (1, 2), (2, 3), (3, 4), (4, 0)]) == "ring"
    assert cm.classify_form(6, [(0, 1), (1, 2), (3, 4), (4, 5), (2, 3)]) in ("twins", "chain")
    assert cm.classify_form(6, [(0, 1), (0, 2), (3, 4), (3, 5), (2, 3)]) == "twins"
    assert cm.classify_form(6, [(0, 1)]) == "scatter"


def _day(**kw):
    base = dict(date="2026-10-10", concepts=["a", "b", "c", "d", "e"],
                links=[("a", "b"), ("a", "c"), ("a", "d"), ("a", "e")])
    base.update(kw)
    return cm.DaySummary(**base)


def test_scene_deterministic_and_separated():
    s1, s2 = cm.build_scene(_day()), cm.build_scene(_day())
    assert [(s.x, s.y) for s in s1.stars] == [(s.x, s.y) for s in s2.stars]
    for i, a in enumerate(s1.stars):
        for b in s1.stars[i + 1:]:
            assert ((a.x - b.x) ** 2 + (a.y - b.y) ** 2) ** 0.5 > 0.05


def test_modes_follow_day():
    calm = cm.build_scene(_day())
    assert calm.modes[0] == "dorian" and calm.modes[2] == "ionian"
    hard = cm.build_scene(_day(thoughts=[cm.Thought("b", "x", "err"), cm.Thought("c", "y", "gap")], comet=True))
    assert hard.modes[0] == "phrygian" and hard.modes[2] == "acoustic"
    err = next(s for s in hard.stars if s.kind == "err")
    assert err.notes[0][1] == 3 and err.notes[1][1] == 4  # напряжение разрешается


def test_ladder_and_far_edge():
    sc = cm.build_scene(_day(ladder=[cm.LadderStep("s1"), cm.LadderStep("s2", "known")], far_edge="F"))
    kinds = [s.kind for s in sc.stars]
    assert kinds.count("far") == 1 and kinds.count("step") == 1
    assert sc.stars[-1].label == "F" and sc.stars[-1].ignite <= cm.T_TITLE


def test_capture_pure_helpers():
    assert capture.parse_cursorpos('{"x": 10, "y": 20}') == (10, 20)
    assert capture.parse_cursorpos("100, 200") == (100, 200)
    assert capture.parse_cursorpos("") is None
    assert capture.copy_shortcut_for("kitty") == "CTRL SHIFT, C"
    assert capture.copy_shortcut_for("firefox") == "CTRL, C"
    assert capture.classify_context("dev.zed.Zed", "x") == "dev"
    assert capture.classify_context("firefox", "Claude - чат") == "dev"
    assert capture.classify_context("org.kde.okular", "Кострикин.pdf") == "study"


def test_sand_becomes_still():
    s = Sand(1); r = random.Random(2)
    s.lift([(100 + r.random() * 200, 100 + r.random() * 20) for _ in range(400)])
    for _ in range(20):
        s.step(1)
    s.wait((200, 200))
    for _ in range(40):
        s.step(1)
    s.collapse([[(150 + r.random() * 60, 260 + r.random() * 10) for _ in range(30)] for _ in range(4)])
    for _ in range(400):
        s.step(1)
        if s.is_still():
            break
    assert s.is_still()


def test_sample_mask():
    w, h = 10, 4
    buf = bytearray(w * h * 4)
    buf[(2 * w + 5) * 4 + 3] = 255
    pts = sample_mask(buf, w, h, w * 4, 5, random.Random(0))
    assert set(pts) == {(5, 2)}


if __name__ == "__main__":
    fns = [v for k, v in dict(globals()).items() if k.startswith("test_")]
    for f in fns:
        f()
        print("ok", f.__name__)

