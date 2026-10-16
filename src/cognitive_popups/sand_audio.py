"""Sound of the four-words sand: one low-latency stream, mixed in this process.

Owner's decision of 10.10.2026: the whole demo set in the words window, quiet,
with no hum while waiting. Pre-rendered assets (lift, land, back, hover and
plucked notes with their reverb) are mixed with grains synthesised on the fly,
so the rustle can follow the pointer's speed. Only the standard library: the
mix is a few thousand multiply-adds per block, written to ``pw-cat``.

Opt-in (note 122, «шубуршания начинают надоедать»): silent unless the flag
``sand.sound`` exists (``cognitive-sand.sh unmute``). ``sand.silent``,
``cognitive-sand.sh mute`` and ``COGNITIVE_SOUND=off`` still silence it.
"""
from __future__ import annotations

import math
import os
import random
import shutil
import subprocess
import threading
import time
import wave
from array import array
from pathlib import Path

RATE = 32000
BLOCK = 256           # 8 ms
LEAD_S = .045         # how far ahead of the clock the stream is kept
VOLUME = .5
#: Am7, one note per word, as in the demo: four landings make a chord.
NOTES = (69, 72, 76, 79)
DISABLED = {"0", "off", "none", "false", "no"}


def state_root() -> Path:
    from .motion import state_dir
    return state_dir()


def opted_in(root: Path | None = None) -> bool:
    """The reader asked for sand sound (``sand.sound``) and did not mute it."""
    root = state_root() if root is None else root
    return (root / "sand.sound").exists() and not (root / "sand.silent").exists()


def enabled() -> bool:
    if os.environ.get("COGNITIVE_SOUND", "").strip().lower() in DISABLED:
        return False
    root = state_root()
    return (root / "sand.enabled").exists() and opted_in(root)


def asset_dir() -> Path:
    return Path(os.environ.get("COGNITIVE_SAND_SOUND_DIR") or state_root() / "sounds-sand")


def pan_gains(pan: float) -> tuple[float, float]:
    pan = max(-1.0, min(1.0, pan))
    angle = (pan + 1) * math.pi / 4
    return math.cos(angle), math.sin(angle)


def make_grains(rnd: random.Random) -> list[tuple[float, array]]:
    """Short band-limited noise bursts (5-20 ms) at a few centre frequencies."""
    grains = []
    for centre in (2200, 3200, 4500, 6000, 7500, 9000):
        for _ in range(3):
            n = int(RATE * (.005 + rnd.random() * .015))
            # two one-pole filters: a crude band-pass around the centre
            a = math.exp(-2 * math.pi * centre * 1.4 / RATE)
            b = math.exp(-2 * math.pi * centre * .6 / RATE)
            low = high = 0.0
            out = array("f")
            for i in range(n):
                x = rnd.random() * 2 - 1
                low = (1 - a) * x + a * low
                high = (1 - b) * x + b * high
                env = math.sin(math.pi * i / n) ** 2
                out.append((low - high) * env * 3.2)
            grains.append((centre, out))
    return grains


class Mixer:
    """Voices are (samples, position, left gain, right gain); the thread mixes them."""

    def __init__(self, command=None, assets=None, volume=VOLUME, seed=0):
        self.rnd = random.Random(seed)
        self.assets = Path(assets) if assets else asset_dir()
        self.volume = volume
        self.voices = []
        self.cache = {}
        self.lock = threading.Lock()
        self.closed = False
        self.grains = make_grains(self.rnd)
        self.process = subprocess.Popen(
            command or ["pw-cat", "--playback", "--raw", "--rate", str(RATE), "--channels", "2",
                        "--format", "s16", "--latency", "20ms", "--media-role", "Game", "-"],
            stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    @classmethod
    def create(cls):
        """A mixer when sound is on and the player exists, otherwise None."""
        if not enabled() or not shutil.which("pw-cat") or not asset_dir().is_dir():
            return None
        try:
            mixer = cls()
        except OSError:
            return None
        if mixer.process.poll() is not None:  # the player refused the stream
            mixer.closed = True
            return None
        return mixer

    # --- sources -------------------------------------------------------------

    def load(self, stem, max_s=2.6):
        """Samples of an asset as floats in sample units (gain carries 1/32768)."""
        if stem not in self.cache:
            samples = array("f")
            try:
                with wave.open(str(self.assets / f"{stem}.wav")) as handle:
                    frames = handle.readframes(min(handle.getnframes(), int(handle.getframerate() * max_s)))
                samples = array("f", array("h", frames))  # converted in C
                n = len(samples)
                fade = max(1, min(int(RATE * .06), n // 4))
                for i in range(n - fade, n):
                    samples[i] *= (n - i) / fade
            except (OSError, wave.Error, EOFError):
                pass
            self.cache[stem] = samples
        return self.cache[stem]

    def preload(self):
        """Everything the window may play, loaded before the first click needs it."""
        for stem in ("lift", "land", "back"):
            self.load(stem)
        for midi in NOTES:
            self.load(f"pluck_{midi}")
            self.load(f"pluck_{midi + 12}")
            self.load(f"pluck_soft_{midi}")

    def add(self, samples, gain=1.0, pan=0.0, delay=0.0):
        if not samples or self.closed:
            return
        left, right = pan_gains(pan)
        gain *= self.volume
        with self.lock:
            self.voices.append([samples, -int(delay * RATE), left * gain, right * gain])

    def play(self, stem, gain=1.0, pan=0.0, delay=0.0):
        self.add(self.load(stem), gain / 32768, pan, delay)

    def note(self, midi, gain=.3, pan=0.0, delay=0.0, soft=False):
        midi = max(48, min(86, int(midi)))
        self.play(f"pluck_{'soft_' if soft else ''}{midi}", gain, pan, delay)

    def grain(self, freq, gain, pan=0.0, delay=0.0):
        best = min(self.grains, key=lambda g: abs(g[0] - freq) + self.rnd.random() * 900)
        self.add(best[1], gain, pan + (self.rnd.random() - .5) * .3, delay)

    def pour(self, dur, count, pan0, pan1, reverse=False, low=4500, high=9000, gain=.06):
        """A trickle of grains across the stereo field (lift, disclosure, return)."""
        for _ in range(count):
            u = self.rnd.random()
            fade = 1 - (1 - u if reverse else u) * .6
            self.grain(low + self.rnd.random() * (high - low), gain * fade * self.rnd.random(),
                       pan0 + (pan1 - pan0) * u, u * dur)

    def arpeggio(self, word, up=True, pan=0.0):
        """Disclosure climbs an octave above the word's note; folding walks back down."""
        steps = (0, 1, 2) if up else (2, 1, 0)
        for i, k in enumerate(steps):
            midi = NOTES[(word + k) % 4] + (12 if up else 0)
            self.note(midi, .16 if up else .13, pan, (.12 if up else 0) + i * .07, soft=not up)

    # --- the stream ----------------------------------------------------------

    def mix(self, n):
        left = [0.0] * n
        right = [0.0] * n
        with self.lock:
            voices = list(self.voices)
        finished = []
        for voice in voices:
            samples, pos, gl, gr = voice
            if pos < 0:
                skip = min(n, -pos)
                voice[1] = pos + skip
                if skip == n:
                    continue
                start, pos = skip, 0
            else:
                start = 0
            seg = samples[pos:pos + n - start]
            if start:
                left[start:start + len(seg)] = [a + b * gl for a, b in zip(left[start:], seg)]
                right[start:start + len(seg)] = [a + b * gr for a, b in zip(right[start:], seg)]
            else:
                left[:len(seg)] = [a + b * gl for a, b in zip(left, seg)]
                right[:len(seg)] = [a + b * gr for a, b in zip(right, seg)]
            voice[1] = pos + len(seg)
            if voice[1] >= len(samples):
                finished.append(voice)
        if finished:
            with self.lock:
                self.voices = [v for v in self.voices if v not in finished]
        out = array("h", bytes(4 * n))
        for i in range(n):
            # soft clip keeps chords from cracking
            l, r = left[i], right[i]
            out[2 * i] = int(32767 * (l if -.6 < l < .6 else math.tanh(l)))
            out[2 * i + 1] = int(32767 * (r if -.6 < r < .6 else math.tanh(r)))
        return out.tobytes()

    def run(self):
        self.preload()
        started = time.monotonic()
        written = 0
        stream = self.process.stdin
        try:
            while not self.closed:
                ahead = written / RATE - (time.monotonic() - started)
                if ahead > LEAD_S:
                    time.sleep(min(.008, ahead - LEAD_S))
                    continue
                stream.write(self.mix(BLOCK))
                stream.flush()
                written += BLOCK
        except (BrokenPipeError, OSError, ValueError):
            pass

    def busy(self):
        with self.lock:
            return bool(self.voices)

    def close(self, linger=0.0):
        """Let ringing voices finish (up to ``linger`` s), then end the stream."""
        end = time.monotonic() + linger
        while linger and self.busy() and time.monotonic() < end:
            time.sleep(.02)
        self.closed = True
        try:
            self.process.stdin.close()
        except OSError:
            pass
        try:
            self.process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            self.process.terminate()
