"""Модель катсцены «per aspera ad astra» для закрытия учебной сессии.

Катсцена НЕ случайная: она вычисляется из настоящих данных дня, поэтому
каждый день свой и что-то значит. Этот модуль чистый (без окон и звука):
на входе сводка дня, на выходе всё, что нужно отрисовщику и звуку.

Повторяет логику демо `reference/demos/cutscene_per_aspera.html`.
Только стандартная библиотека.
"""
from __future__ import annotations

import hashlib
import math
import random
from dataclasses import dataclass, field

# --- Входные данные ------------------------------------------------------

@dataclass
class Thought:
    concept: str                  # к какому понятию прилипает
    text: str                     # дословно
    kind: str = "own"             # own | err | vague | gap
    versions: list[str] = field(default_factory=list)  # для vague: [размытая, смелее]


@dataclass
class LadderStep:
    label: str
    status: str = "read"          # read (читал) | known (знаю)


@dataclass
class DaySummary:
    date: str                                  # '2026-10-10'
    concepts: list[str]                        # понятия дня (из вселенной)
    links: list[tuple[str, str]]               # связи между ними (из вселенной, не от модели)
    thoughts: list[Thought] = field(default_factory=list)
    steps_yes: int = 0                         # шаги «Да»
    steps_partial: int = 0                     # «Частично» и «Нет» → туманность
    ladder: list[LadderStep] = field(default_factory=list)
    far_edge: str | None = None                # далёкое ребро сессии
    far_edge_attached: bool = False            # финальная догадка о нём верна
    comet: bool = False                        # старая мысль связалась с новой


# --- Выход ---------------------------------------------------------------

FORMS = {"hub": "корона", "chain": "река", "twins": "двойная звезда",
         "ring": "кольцо", "scatter": "рассеянное скопление"}
MODES = {
    "dorian": [0, 2, 3, 5, 7, 9, 10], "aeolian": [0, 2, 3, 5, 7, 8, 10],
    "phrygian": [0, 1, 3, 5, 7, 8, 10], "mixolydian": [0, 2, 4, 5, 7, 9, 10],
    "ionian": [0, 2, 4, 5, 7, 9, 11], "lydian": [0, 2, 4, 6, 7, 9, 11],
    "acoustic": [0, 2, 4, 6, 7, 9, 10],
}
MODE_NAMES = {"dorian": "дорийский", "aeolian": "эолийский", "phrygian": "фригийский",
              "mixolydian": "миксолидийский", "ionian": "ионийский", "lydian": "лидийский",
              "acoustic": "акустический"}
PENT = [0, 1, 2, 4, 5]          # консонантные ступени лада для обычных звёзд
TINTS = ["#F5C4B3", "#B5D4F4", "#FAC775", "#CECBF6", "#9FE1CB"]

# тайминг (секунды от начала)
T_LIFT, T_FORM, STAR_GAP, T_TITLE, T_END = 4.2, 8.3, 0.42, 13.9, 17.5


@dataclass
class Star:
    label: str
    kind: str          # own | err | gap | vague | step | far
    x: float           # 0..1
    y: float           # 0..1 (0 сверху)
    ignite: float = 0.0
    rank: int = 0
    text: str = ""
    versions: list[str] = field(default_factory=list)
    notes: list[tuple[float, int, int]] = field(default_factory=list)  # (задержка, ступень, октава)


@dataclass
class Scene:
    form: str
    form_name: str
    name: str
    tint: str
    modes: tuple[str, str, str]
    root_midi: int
    stars: list[Star]
    edges: list[tuple[int, int, bool]]   # (a, b, пунктир)
    nebula: bool
    comet: bool
    stats: dict
    timeline: dict


def _seed(day: DaySummary) -> int:
    h = hashlib.sha256((day.date + "|" + "|".join(sorted(day.concepts))).encode()).hexdigest()
    return int(h[:12], 16)


def classify_form(n: int, links: list[tuple[int, int]]) -> str:
    """Форма созвездия по форме графа мыслей дня."""
    if n <= 2 or not links:
        return "scatter"
    adj = {i: set() for i in range(n)}
    for a, b in links:
        adj[a].add(b); adj[b].add(a)
    deg = [len(adj[i]) for i in range(n)]
    comps = _components(n, adj)
    big = [c for c in comps if len(c) >= 2]
    if len(comps) >= 3 or len(links) < n - 2:
        return "scatter"
    if max(deg) >= max(3, math.ceil(0.7 * (n - 1))):
        return "hub"
    if len(comps) == 1 and len(links) >= n and all(d == 2 for d in deg):
        return "ring"
    if len(comps) == 1 and max(deg) <= 2:
        return "chain"
    for a, b in links:  # мостик между двумя темами
        adj[a].discard(b); adj[b].discard(a)
        parts = _components(n, adj)
        adj[a].add(b); adj[b].add(a)
        if len(parts) == 2 and all(len(p) >= 2 for p in parts):
            return "twins"
    if len(big) == 1 and max(deg) <= 2:
        return "chain"
    return "scatter" if len(comps) > 1 else "chain"


def _components(n, adj):
    seen, comps = set(), []
    for s in range(n):
        if s in seen:
            continue
        stack, comp = [s], []
        seen.add(s)
        while stack:
            v = stack.pop(); comp.append(v)
            for w in adj[v]:
                if w not in seen:
                    seen.add(w); stack.append(w)
        comps.append(comp)
    return comps


def _layout(form: str, n: int, links, R: random.Random) -> list[tuple[float, float]]:
    cx, cy, sc = 0.36 + R.random() * 0.2, 0.36 + R.random() * 0.06, 0.16
    pts = [(0.5, 0.4)] * n
    if form == "hub":
        deg = [sum(1 for e in links if i in e) for i in range(n)]
        c = max(range(n), key=lambda i: deg[i]); others = [i for i in range(n) if i != c]
        pts = list(pts); pts[c] = (cx, cy)
        for k, i in enumerate(others):
            a = k / max(1, len(others)) * 2 * math.pi + R.random() * 0.4
            r = sc * (0.6 + R.random() * 0.4)
            pts[i] = (cx + math.cos(a) * r * 1.4, cy + math.sin(a) * r * 1.3)
    elif form == "chain":
        ph = R.random() * 6
        for i in range(n):
            u = i / max(1, n - 1)
            pts[i] = (0.1 + u * 0.62, cy + math.sin(u * 4.5 + ph) * sc * 0.9)
    elif form == "ring":
        for i in range(n):
            a = i / n * 2 * math.pi + R.random() * 0.25
            pts[i] = (cx + math.cos(a) * sc * 1.25, cy + math.sin(a) * sc * 1.1)
    else:  # twins и scatter
        for i in range(n):
            pts[i] = (0.1 + R.random() * 0.6, 0.16 + R.random() * 0.42)
    return pts


def _relax(pts: list[list[float]], fixed: set[int], min_d: float = 0.075, iters: int = 90):
    """Разводит звёзды, чтобы не слипались и наводились по отдельности."""
    for _ in range(iters):
        for i in range(len(pts)):
            for j in range(i + 1, len(pts)):
                dx, dy = pts[j][0] - pts[i][0], pts[j][1] - pts[i][1]
                d = math.hypot(dx, dy) or 1e-3
                if d < min_d:
                    f = (min_d - d) / 2; dx /= d; dy /= d
                    if i not in fixed:
                        pts[i][0] -= dx * f; pts[i][1] -= dy * f
                    if j not in fixed:
                        pts[j][0] += dx * f; pts[j][1] += dy * f
        for p in pts:
            p[0] = min(0.96, max(0.04, p[0])); p[1] = min(0.68, max(0.1, p[1]))


def build_scene(day: DaySummary) -> Scene:
    if not day.concepts and day.far_edge:
        from dataclasses import replace
        day = replace(day, concepts=["Начало пути"])
    R = random.Random(_seed(day))
    idx = {c: i for i, c in enumerate(day.concepts)}
    links = sorted({tuple(sorted((idx[a], idx[b]))) for a, b in day.links if a in idx and b in idx and a != b})
    n = len(day.concepts)
    form = classify_form(n, links)
    pts = [list(p) for p in _layout(form, n, links, R)]

    by_concept = {}
    for t in day.thoughts:
        by_concept.setdefault(t.concept, t)
    stars: list[Star] = []
    for i, c in enumerate(day.concepts):
        t = by_concept.get(c)
        stars.append(Star(label=c, kind=(t.kind if t else "own"), x=pts[i][0], y=pts[i][1],
                          text=(t.text if t else ""), versions=(t.versions if t else [])))
    edges = [(a, b, False) for a, b in links]
    deg = [sum(1 for e in links if i in e) for i in range(n)]
    central = max(range(n), key=lambda i: deg[i]) if n else 0
    order = [central] + [i for i in range(n) if i != central]

    if day.far_edge:
        fx, fy = 0.9, 0.1 + R.random() * 0.12
        prev = central
        for k, st in enumerate(day.ladder, 1):
            u = k / (len(day.ladder) + 1)
            ax, ay = stars[central].x, stars[central].y
            stars.append(Star(label=st.label, kind="step" if st.status == "read" else "own",
                              x=ax + (fx - ax) * u, y=ay + (fy - ay) * u + math.sin(u * math.pi) * 0.05,
                              text="читал, ещё не знаю" if st.status == "read" else "знаю"))
            edges.append((prev, len(stars) - 1, True)); order.append(len(stars) - 1); prev = len(stars) - 1
        stars.append(Star(label=day.far_edge, kind="own" if day.far_edge_attached else "far", x=fx, y=fy,
                          text="прицепилось" if day.far_edge_attached else "далёкое ребро: пока не цепляется"))
        edges.append((prev, len(stars) - 1, not day.far_edge_attached)); order.append(len(stars) - 1)

    P = [[s.x, s.y] for s in stars]
    _relax(P, fixed={i for i, s in enumerate(stars) if s.kind == "far"})
    for s, p in zip(stars, P):
        s.x, s.y = round(p[0], 4), round(p[1], 4)

    n_hard = sum(1 for s in stars if s.kind in ("err", "gap"))
    errs = sum(1 for s in stars if s.kind == "err")
    m1 = "dorian" if n_hard == 0 else "aeolian" if n_hard == 1 else "phrygian"
    m2 = "ionian" if day.steps_yes >= 3 else "mixolydian"
    m3 = "acoustic" if (errs and day.comet) else "lydian" if (day.ladder or day.comet or day.far_edge) else "ionian"

    for r, i in enumerate(order):
        s = stars[i]; s.ignite = round(T_FORM + r * STAR_GAP, 2); s.rank = r
        if s.kind == "err":     # напряжение, которое разрешается
            s.notes = [(0.0, 3, 1), (0.5, 2 if m3 == "ionian" else 4, 1)]
        elif s.kind == "gap":
            s.notes = [(0.0, 1, 1), (0.4, 2, 1)]
        elif s.kind == "vague":  # приглушённо, потом ясно (смелее)
            s.notes = [(0.0, PENT[r % 5], 1), (1.3, PENT[r % 5], 1)]
        elif s.kind == "step":
            s.notes = [(0.0, [4, 5, 6][min(2, r % 3)], 1)]   # в миксолидийском
        elif s.kind == "far":
            s.notes = [(0.0, 1, 2)]
        else:
            s.notes = [(0.0, PENT[r % 5], 1)]

    return Scene(
        form=form, form_name=FORMS[form], name=stars[central].label if stars else "",
        tint=TINTS[R.randrange(len(TINTS))], modes=(m1, m2, m3),
        root_midi=R.choice([50, 52, 53]), stars=stars, edges=edges,
        nebula=day.steps_partial > 0, comet=day.comet,
        stats={"мысли": len(day.thoughts), "сохранённые ошибки": errs, "ступеньки": len(day.ladder),
               "трудные места": n_hard},
        timeline={"lift": T_LIFT, "form": T_FORM,
                  "title": max(T_TITLE, T_FORM + max(0, len(stars)-1)*STAR_GAP + 1.4),
                  "end": max(T_END, T_FORM + max(0, len(stars)-1)*STAR_GAP + 3.6)},
    )


def note_midi(root_midi: int, mode: str, degree: int, octave: int) -> int:
    sc = MODES[mode]
    return root_midi + 12 * (octave + degree // 7) + sc[degree % 7]


if __name__ == "__main__":
    d = DaySummary(
        date="2026-10-10",
        concepts=["корни", "перестановки", "группа подстановок", "уравнение", "симметрия"],
        links=[("перестановки", "корни"), ("перестановки", "группа подстановок"),
               ("перестановки", "симметрия"), ("перестановки", "уравнение")],
        thoughts=[Thought("корни", "корни можно переставлять, a↔b"),
                  Thought("группа подстановок", "все перестановки корней входят в группу", "err"),
                  Thought("симметрия", "", "vague", ["что-то про симметрию", "перестановка не меняет коэффициенты"])],
        steps_yes=1, steps_partial=1,
        ladder=[LadderStep("перестановки, сохраняющие связи"), LadderStep("группа Галуа как подгруппа")],
        far_edge="разрешимая группа", comet=True)
    s = build_scene(d)
    print(s.form_name, s.name, [MODE_NAMES[m] for m in s.modes])
    for st in s.stars:
        print(f"{st.kind:6s} {st.label:32s} ({st.x:.2f},{st.y:.2f}) t={st.ignite} notes={st.notes}")

