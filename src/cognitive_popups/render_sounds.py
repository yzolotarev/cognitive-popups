"""Офлайн-генератор мягкой звуковой библиотеки (WAV) без новых библиотек.

Зачем: в проекте звук играется готовыми файлами через `sound.py`
(внешний плеер, без синтеза в реальном времени). Значит, мягкие щипки,
подложки, шорох и ветер проще один раз отрендерить в файлы, а потом
только проигрывать. Лады и ноты выбирает `cutscene_model.note_midi`.

Что внутри уже «сведено» под комфорт (см. docs/06_SOUND_SYSTEM.md):
  * срез низов 110 Гц (2 фильтра), −4 дБ на 320 Гц (мутность),
    −3.5 дБ на 3 кГц (утомляющая зона), −5 дБ полка выше 6 кГц;
  * реверберация с паузой 40 мс, её возврат только 280 Гц – 2.6 кГц;
  * подложки из чистых синусов, расстройка ±3 цента (без «биений»);
  * одна громкость для всех нот (без нормализации каждого файла).

Запуск (один раз, в фоне, займёт несколько минут на чистом Python):
    python3 render_sounds.py --out ~/.local/state/cognitive-popups/sounds/sand
    python3 render_sounds.py --out /tmp/s --only pluck --lo 62 --hi 64   # быстрая проба

Только стандартная библиотека: math, random, wave, struct, array.
"""
from __future__ import annotations

import argparse
import math
import random
import struct
import wave
from array import array
from pathlib import Path

SR = 32000


def hz(midi: float) -> float:
    return 440.0 * 2 ** ((midi - 69) / 12)


# --- Биквадратные фильтры (формулы RBJ) ------------------------------------

def biquad(kind: str, f0: float, q: float = 0.707, gain_db: float = 0.0):
    A = 10 ** (gain_db / 40)
    w = 2 * math.pi * f0 / SR
    cw, sw = math.cos(w), math.sin(w)
    alpha = sw / (2 * q)
    if kind == "lp":
        b0, b1, b2 = (1 - cw) / 2, 1 - cw, (1 - cw) / 2; a0, a1, a2 = 1 + alpha, -2 * cw, 1 - alpha
    elif kind == "hp":
        b0, b1, b2 = (1 + cw) / 2, -(1 + cw), (1 + cw) / 2; a0, a1, a2 = 1 + alpha, -2 * cw, 1 - alpha
    elif kind == "bp":
        b0, b1, b2 = alpha, 0.0, -alpha; a0, a1, a2 = 1 + alpha, -2 * cw, 1 - alpha
    elif kind == "peak":
        b0, b1, b2 = 1 + alpha * A, -2 * cw, 1 - alpha * A; a0, a1, a2 = 1 + alpha / A, -2 * cw, 1 - alpha / A
    elif kind == "hshelf":
        s = 2 * math.sqrt(A) * alpha
        b0 = A * ((A + 1) + (A - 1) * cw + s); b1 = -2 * A * ((A - 1) + (A + 1) * cw); b2 = A * ((A + 1) + (A - 1) * cw - s)
        a0 = (A + 1) - (A - 1) * cw + s; a1 = 2 * ((A - 1) - (A + 1) * cw); a2 = (A + 1) - (A - 1) * cw - s
    else:
        raise ValueError(kind)
    return (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)


def run_filter(x: list[float], c) -> list[float]:
    b0, b1, b2, a1, a2 = c
    x1 = x2 = y1 = y2 = 0.0
    out = [0.0] * len(x)
    for i, v in enumerate(x):
        y = b0 * v + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2
        x2, x1, y2, y1 = x1, v, y1, y
        out[i] = y
    return out


MASTER_EQ = [biquad("hp", 110, 0.6), biquad("hp", 110, 0.6), biquad("peak", 320, 0.9, -4.0),
             biquad("peak", 3000, 1.1, -3.5), biquad("hshelf", 6000, 0.707, -5.0)]
REV_EQ = [biquad("hp", 280, 0.6), biquad("lp", 2600, 0.5)]


# --- Реверберация (Шрёдер: 4 гребёнки + 2 фазовращателя) -------------------

def reverb(x: list[float], seconds_tail: float = 3.0) -> list[float]:
    pre = int(0.04 * SR)
    n = len(x) + int(seconds_tail * SR)
    src = [0.0] * pre + x + [0.0] * (n - len(x) - pre)
    src = src[:n]
    combs = [int(SR * d) for d in (0.0297, 0.0371, 0.0411, 0.0437)]
    out = [0.0] * n
    for D in combs:
        buf = [0.0] * D; idx = 0; lp = 0.0
        fb, damp = 0.86, 0.35
        for i in range(n):
            y = buf[idx]
            lp = y * (1 - damp) + lp * damp
            buf[idx] = src[i] + lp * fb
            idx += 1
            if idx == D:
                idx = 0
            out[i] += y * 0.25
    for D, g in ((int(SR * 0.005), 0.7), (int(SR * 0.0017), 0.7)):
        buf = [0.0] * D; idx = 0
        for i in range(n):
            b = buf[idx]; v = out[i]
            y = -v * g + b
            buf[idx] = v + b * g
            idx += 1
            if idx == D:
                idx = 0
            out[i] = y
    for c in REV_EQ:
        out = run_filter(out, c)
    return out


def mix_master(dry: list[float], wet: float, tail: float = 3.0, gain: float = 3.6) -> list[float]:
    rv = reverb(dry, tail)
    n = len(rv)
    y = [0.0] * n
    dg = 1 - wet * 0.55
    for i in range(n):
        d = dry[i] if i < len(dry) else 0.0
        y[i] = d * dg + rv[i] * wet * 0.5
    for c in MASTER_EQ:
        y = run_filter(y, c)
    fade = int(0.05 * SR)
    for i in range(fade):
        y[n - 1 - i] *= i / fade
    return [v * gain for v in y]


# --- Голоса ----------------------------------------------------------------

def tri(ph: float) -> float:
    p = ph % 1.0
    return 4 * p - 1 if p < 0.5 else 3 - 4 * p


def pluck(midi: float, dur: float = 2.8, level: float = 0.33, cutoff: float = 1600.0) -> list[float]:
    f = hz(midi); n = int(dur * SR)
    level *= (hz(62) / f) ** 0.45 if midi > 62 else 1.0   # верхние ноты чуть тише: ухо к ним чувствительнее
    out = [0.0] * n
    p1 = p2 = p3 = 0.0
    d1, d2, d3 = f / SR, f * 2 ** (2 / 1200) / SR, 2 * f * 2 ** (-2 / 1200) / SR
    lp = 0.0
    att = int(0.03 * SR)
    for i in range(n):
        t = i / SR
        env = (i / att) if i < att else math.exp(-(t - 0.03) * 2.6)
        if i % 64 == 0:
            c = max(300.0, cutoff * (0.4 + 0.6 * math.exp(-t * 1.2)))
            a = 1 - math.exp(-2 * math.pi * c / SR)
        s = math.sin(2 * math.pi * p1) * 0.75 + tri(p2) * 0.22 + math.sin(2 * math.pi * p3) * 0.05
        p1 += d1; p2 += d2; p3 += d3
        lp += a * (s - lp)
        out[i] = lp * env * level
    return out


def pad(midi: float, dur: float = 5.5, level: float = 0.16, cutoff: float = 950.0) -> list[float]:
    f = hz(midi); n = int(dur * SR)
    out = [0.0] * n
    ds = [f * 2 ** (-3 / 1200) / SR, f * 2 ** (3 / 1200) / SR, 2 * f / SR]
    vs = [0.5, 0.5, 0.1]
    ph = [0.0, 0.0, 0.0]
    a = 1 - math.exp(-2 * math.pi * cutoff / SR)
    lp = 0.0
    att = min(1.1, dur * 0.3); rel_start = dur * 0.62; tau = dur * 0.11
    for i in range(n):
        t = i / SR
        env = min(1.0, t / att) if t < rel_start else math.exp(-(t - rel_start) / tau)
        s = 0.0
        for k in range(3):
            s += math.sin(2 * math.pi * ph[k]) * vs[k]; ph[k] += ds[k]
        lp += a * (s - lp)
        out[i] = lp * env * level
    return out


def grains(dur: float, count: int, f_lo: float, f_hi: float, level: float, seed: int = 1,
           falling: bool = False) -> list[float]:
    r = random.Random(seed)
    n = int(dur * SR)
    out = [0.0] * n
    for _ in range(count):
        u = r.random()
        start = int(u * (n - int(0.06 * SR)))
        L = int((0.025 + r.random() * 0.03) * SR)
        c = biquad("bp", f_lo + r.random() * (f_hi - f_lo), 2.5)
        burst = [(r.random() * 2 - 1) for _ in range(L)]
        burst = run_filter(burst, c)
        g = level * r.random() * ((1 - 0.6 * u) if not falling else (0.4 + 0.6 * u))
        att = max(1, int(0.005 * SR))
        for j in range(L):
            env = (j / att) if j < att else math.exp(-(j - att) / (L * 0.25))
            out[start + j] += burst[j] * env * g
    return out


def wind(dur: float = 4.8, level: float = 0.25, seed: int = 3) -> list[float]:
    r = random.Random(seed)
    n = int(dur * SR)
    out = [0.0] * n
    x1 = x2 = y1 = y2 = 0.0
    for i in range(n):
        t = i / SR
        if i % 128 == 0:
            f0 = 380 * (820 / 380) ** min(1.0, t / 3.0) if t < 3 else 820 * (450 / 820) ** ((t - 3) / 1.6)
            b0, b1, b2, a1, a2 = biquad("bp", f0, 0.7)
        v = r.random() * 2 - 1
        y = b0 * v + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2
        x2, x1, y2, y1 = x1, v, y1, y
        env = min(1.0, t / 1.8) if t < 1.8 else max(0.0, 1 - (t - 1.8) / 3.0)
        out[i] = y * env * level
    return out


def pat(level: float = 0.5) -> list[float]:
    """Мягкое «приземление»: приглушённый шлепок песка, без низкого гула."""
    n = int(0.35 * SR)
    r = random.Random(7)
    noise = [r.random() * 2 - 1 for _ in range(n)]
    noise = run_filter(noise, biquad("lp", 420, 0.8))
    out = [0.0] * n
    ph = 0.0
    for i in range(n):
        t = i / SR
        f = 150 * (75 / 150) ** min(1.0, t / 0.16)
        ph += f / SR
        env = min(1.0, t / 0.004) * math.exp(-t / 0.07)
        out[i] = (noise[i] * 0.8 + math.sin(2 * math.pi * ph) * 0.35) * env * level
    return out


# --- Запись ------------------------------------------------------------------

def write_wav(path: Path, x: list[float]) -> None:
    peak = max(1e-9, max(abs(v) for v in x))
    scale = 0.89 / peak if peak > 0.89 else 1.0   # только защита от клиппинга
    data = array("h", (int(max(-1.0, min(1.0, v * scale)) * 32767) for v in x))
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR)
        w.writeframes(data.tobytes())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--only", choices=["pluck", "pad", "fx"], default=None)
    ap.add_argument("--lo", type=int, default=50, help="нижняя нота MIDI")
    ap.add_argument("--hi", type=int, default=86, help="верхняя нота MIDI")
    a = ap.parse_args()
    out = Path(a.out).expanduser()
    if a.only in (None, "fx"):
        write_wav(out / "lift.wav", mix_master(grains(0.9, 70, 1400, 3600, 0.25, 1), 0.45, 1.5))
        write_wav(out / "settle.wav", mix_master(grains(0.7, 45, 1200, 3000, 0.22, 2, falling=True), 0.45, 1.5))
        write_wav(out / "back.wav", mix_master(grains(1.0, 60, 1400, 3200, 0.2, 4, falling=True), 0.45, 1.5))
        write_wav(out / "land.wav", mix_master(pat(), 0.15, 1.0))
        write_wav(out / "wind.wav", mix_master(wind(), 0.6, 2.0))
        write_wav(out / "hover.wav", mix_master(grains(0.3, 18, 2500, 3600, 0.08, 5), 0.4, 1.0))
    if a.only in (None, "pluck"):
        for m in range(a.lo, a.hi + 1):
            write_wav(out / f"pluck_{m}.wav", mix_master(pluck(m), 0.8, 2.5))
            write_wav(out / f"pluck_soft_{m}.wav", mix_master(pluck(m, cutoff=450), 0.8, 2.5))
    if a.only in (None, "pad"):
        for m in range(max(a.lo, 48), min(a.hi, 72) + 1):
            write_wav(out / f"pad_{m}.wav", mix_master(pad(m), 0.85, 3.0))


if __name__ == "__main__":
    main()

