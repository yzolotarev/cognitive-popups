"""Песочный движок: частицы и пять глаголов, без привязки к отрисовке.

Глаголы системы (одинаковые во всех окнах):
    lift      отслоить: пыльца поднимается с текста (смысл берётся, текст остаётся)
    wait      ждать: тихое дышащее ядро, пока думает модель
    collapse  собрать: ядро схлопывается в слова (цели = точки текста)
    settle    осесть: твоё сохраняется, песок ложится и уходит в точку (часики)
    back      вернуть: песок оседает обратно в источник

Движок не знает про GTK. Отрисовщик раз в кадр зовёт `step(dt, mouse)`,
затем рисует `particles` (x, y, size, alpha, kind). Когда `is_still()`
истинно, кадры надо остановить: правило покоя и экономия батареи.

Цели для слов берутся из маски текста: отрисовщик рендерит слово в
offscreen-поверхность и передаёт альфа-канал в `sample_mask`.

Только стандартная библиотека. Скорость: ~1000 частиц за 1–3 мс на шаг
на обычном ноутбуке (проверить на месте; потолок кадров 60 в секунду).
"""
from __future__ import annotations

import math
import random
from dataclasses import dataclass

# Токены движения: одинаковые везде, чтобы тело узнавало глаголы.
DUR = {"lift": 0.4, "collapse": 0.3, "settle": 0.6, "back": 0.9}
SPRING = {"wait": 0.02, "collapse": 0.032, "settle": 0.03, "back": 0.026}
DAMP = {"lift": 0.95, "wait": 0.86, "collapse": 0.80, "settle": 0.82, "back": 0.82}
MOUSE_PUSH_R, MOUSE_PULL_R = 45.0, 140.0


@dataclass
class P:
    x: float
    y: float
    vx: float = 0.0
    vy: float = 0.0
    tx: float = 0.0
    ty: float = 0.0
    ox: float = 0.0      # точка-источник (для back)
    oy: float = 0.0
    dx: float = 0.0      # смещение в диске ядра
    dy: float = 0.0
    ph: float = 0.0
    delay: float = 0.0
    born: float = 0.0
    size: float = 1.0
    alpha: float = 1.0
    group: int = 0       # к какому слову относится
    mine: bool = False   # твой песок (плотный) или чужой (пыльца)


def sample_mask(alpha: bytes | bytearray | memoryview, width: int, height: int, stride: int,
                n: int, rnd: random.Random, offset: tuple[float, float] = (0, 0),
                threshold: int = 110, channel: int = 3, bpp: int = 4) -> list[tuple[float, float]]:
    """Случайные точки внутри непрозрачных пикселей (например, ARGB32 из cairo).

    Перебор идёт в C (срезы байтов и поиск), а не попиксельно в Python: маска
    пояснения строится в момент клика и не должна задерживать кадр.
    """
    table = bytes(1 if v > threshold else 0 for v in range(256))
    data = bytes(alpha)
    pts = []
    for y in range(height):
        row = data[y * stride + channel:y * stride + width * bpp:bpp].translate(table)
        x = row.find(1)
        while x != -1:
            pts.append((x + offset[0], y + offset[1]))
            x = row.find(1, x + 1)
    if not pts:
        return [(offset[0] + width / 2, offset[1] + height / 2)] * n
    return [pts[rnd.randrange(len(pts))] for _ in range(n)]


def _flow(x: float, y: float, t: float) -> float:
    return (math.sin(x * 0.012 + t) * math.cos(y * 0.014 - t * 0.7) + math.sin((x + y) * 0.008 + t * 0.5)) * math.pi


class Sand:
    def __init__(self, seed: int = 0):
        self.rnd = random.Random(seed)
        self.particles: list[P] = []
        self.verb = "idle"
        self.t = 0.0
        self.t_verb = 0.0
        self.core = (0.0, 0.0)
        self.core_r = 36.0
        self.still_frames = 0
        self.reduced_motion = False

    # --- глаголы ---------------------------------------------------------

    def lift(self, source_points: list[tuple[float, float]], groups: int = 4, mine: bool = False):
        r = self.rnd
        self.particles = []
        x0 = min(p[0] for p in source_points); x1 = max(p[0] for p in source_points)
        for i, (x, y) in enumerate(source_points):
            a, rr = r.random() * 2 * math.pi, math.sqrt(r.random())
            self.particles.append(P(x=x, y=y, ox=x, oy=y, vx=(r.random() - 0.5) * 0.8, vy=-0.3 - r.random() * 1.1,
                                    dx=math.cos(a) * rr, dy=math.sin(a) * rr, ph=r.random() * 6.28,
                                    delay=r.random() * 0.3 + (x - x0) / (x1 - x0 + 1) * 0.3,
                                    born=r.random() * 0.35, size=0.75 + r.random() * 1.1,
                                    group=i % groups, mine=mine))
        self._set("lift")

    def wait(self, core_xy: tuple[float, float], radius: float = 36.0):
        self.core, self.core_r = core_xy, radius
        self._set("wait")

    def collapse(self, targets_by_group: list[list[tuple[float, float]]]):
        counters = [0] * len(targets_by_group)
        for p in self.particles:
            g = p.group % len(targets_by_group)
            pts = targets_by_group[g]
            p.tx, p.ty = pts[counters[g] % len(pts)]
            counters[g] += 1
        self._set("collapse")

    def settle(self, point: tuple[float, float]):
        for p in self.particles:
            p.tx, p.ty = point
        self._set("settle")

    def back(self):
        for p in self.particles:
            p.tx, p.ty = p.ox, p.oy
        self._set("back")

    def _set(self, verb: str):
        self.verb, self.t_verb, self.still_frames = verb, self.t, 0

    # --- шаг ---------------------------------------------------------------

    def step(self, dt: float, mouse: tuple[float, float, float, float] | None = None):
        """dt в кадрах по 1/60 с. mouse = (x, y, vx, vy) или None."""
        self.t += dt / 60
        el = self.t - self.t_verb
        t = self.t
        moving = 0
        br = 1 + 0.05 * math.sin(t * 1.7)
        for p in self.particles:
            a = _flow(p.x, p.y, t * 0.4)
            k, K, dm = 0.0, SPRING.get(self.verb, 0.02), DAMP.get(self.verb, 0.9)
            tx, ty = p.x, p.y
            if self.verb == "lift":
                p.vy -= 0.012 * dt
                p.alpha = min(1.0, max(0.0, (el - p.born) * 4))
            elif self.verb == "wait":
                k = min(1.0, max(0.0, (el - p.delay) * 2))
                tx = self.core[0] + p.dx * self.core_r * br
                ty = self.core[1] + p.dy * self.core_r * br
            elif self.verb in ("collapse", "settle"):
                k = min(1.0, max(0.0, (el - p.group * 0.08) * 2.4))
                tx = p.tx + math.cos(a + p.ph) * 1.1
                ty = p.ty + math.sin(a + p.ph) * 1.1
                if self.verb == "settle":
                    p.alpha = min(1.0, math.hypot(p.x - p.tx, p.y - p.ty) / 30)
            elif self.verb == "back":
                k = min(1.0, el * 1.7)
                tx, ty = p.ox, p.oy
                p.alpha = min(1.0, math.hypot(p.x - p.ox, p.y - p.oy) / 30)
            if self.reduced_motion and self.verb != "lift":
                p.x, p.y, p.vx, p.vy = tx, ty, 0.0, 0.0
                continue
            p.vx += ((tx - p.x) * K * k + math.cos(a) * 0.06 * (1 - k)) * dt
            p.vy += ((ty - p.y) * K * k + math.sin(a) * 0.06 * (1 - k)) * dt
            if mouse and self.verb != "back":
                mx, my, mvx, mvy = mouse
                ddx, ddy = p.x - mx, p.y - my
                d2 = ddx * ddx + ddy * ddy
                if 0.01 < d2 < MOUSE_PULL_R ** 2:
                    d = math.sqrt(d2)
                    if d < MOUSE_PUSH_R:
                        f = 1 - d / MOUSE_PUSH_R
                        p.vx += (ddx / d * f * 1.8 + mvx * f * 0.28) * dt
                        p.vy += (ddy / d * f * 1.8 + mvy * f * 0.28) * dt
                    elif self.verb == "collapse":
                        f = (1 - (d - MOUSE_PUSH_R) / (MOUSE_PULL_R - MOUSE_PUSH_R)) * 0.035
                        p.vx -= ddx / d * f * dt
                        p.vy -= ddy / d * f * dt
            damp = dm ** dt
            p.vx *= damp; p.vy *= damp
            p.x += p.vx * dt; p.y += p.vy * dt
            if abs(p.vx) + abs(p.vy) > 0.05:
                moving += 1
        self.still_frames = self.still_frames + 1 if moving < max(3, len(self.particles) // 200) else 0

    def is_still(self) -> bool:
        """Правило покоя: если почти ничего не движется 20 кадров подряд, остановить кадры."""
        return self.verb in ("collapse", "settle", "back", "idle") and self.still_frames > 20

