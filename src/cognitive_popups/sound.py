"""Optional private semantic cues; dismissal is always silent.

Playback launches a detached player without waiting for audio completion.
Missing assets, unavailable players and playback errors never affect the UI.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

#: Deliberately minimal: one stem, tried with each suffix, so dropping
#: `close-sound.wav` (or `.ogg`, `.mp3`, ...) into the state directory is enough.
DEFAULT_STEM = "close-sound"
DEFAULT_SUFFIXES = (".wav", ".ogg", ".oga", ".flac", ".mp3", ".m4a", ".opus")

#: Values of `COGNITIVE_CLOSE_SOUND` that mean "no sound", not a path.
DISABLED = {"0", "off", "none", "false", "no"}

#: WAV/OGG/FLAC decode directly through PipeWire/PulseAudio, so try those native
#: players first and keep latency low.
DIRECT_PLAYERS = ("pw-play", "paplay", "aplay")
#: Anything else (mp3, m4a, opus, ...) goes to an ffmpeg-based player, which
#: decodes what the native players cannot.
DECODING_PLAYERS = ("ffplay", "mpv")
LOSSLESS_SUFFIXES = {".wav", ".ogg", ".oga", ".flac", ".aiff", ".aif"}


def _state_root() -> Path:
    root = os.environ.get("COGNITIVE_STATE_DIR") or "~/.local/state/cognitive-popups"
    return Path(root).expanduser()


MATERIALIZE = "materialize"
REVEAL = "reveal"
CORRECTION = "correction"
RESOLVE = "resolve"
ROLES = (MATERIALIZE, REVEAL, CORRECTION, RESOLVE)

# Frozen 2026-10-07: one cue only. Folding an open Orbital cue or closing
# Orbital plays RESOLVE (the "cheers" asset). Every other event and every bare
# role is silent until the sound system is deliberately reopened.
EVENT_ROLES = {
    "orbital_close": RESOLVE,
    # The reader's own completion points (2026-10-07): a step marked done and a
    # study session closed. Never a model verdict, never an ordinary close.
    "step_done": RESOLVE,
    "session_close": RESOLVE,
    # A saved note answers with a quiet click, not with cheers (owner, 07.10).
    "note_saved": REVEAL,
}


def sound_path(role: str | None = None) -> Path | None:
    """Resolve a role cue, or the legacy close path for API compatibility.

    Legacy path lookup is retained, but play_close_sound never plays it.
    """
    if role is not None:
        if not isinstance(role, str):
            return None
        role = EVENT_ROLES.get(role.lower(), role.lower())
        if role not in ROLES:
            return None
        if os.environ.get("COGNITIVE_SOUND", "").strip().lower() in DISABLED:
            return None
        raw = os.environ.get(f"COGNITIVE_SOUND_{role.upper()}")
        if raw is not None and raw.strip().lower() in DISABLED:
            return None
        if raw and raw.strip():
            return Path(raw.strip()).expanduser()
        root = Path(os.environ.get("COGNITIVE_SOUND_DIR") or str(_state_root() / "sounds")).expanduser()
        for suffix in DEFAULT_SUFFIXES:
            candidate = root / f"{role}{suffix}"
            if candidate.is_file():
                return candidate
        return None
    raw = os.environ.get("COGNITIVE_CLOSE_SOUND")
    if raw is not None and raw.strip().lower() in DISABLED:
        return None
    if raw and raw.strip():
        return Path(raw).expanduser()
    for suffix in DEFAULT_SUFFIXES:
        candidate = _state_root() / f"{DEFAULT_STEM}{suffix}"
        if candidate.is_file():
            return candidate
    return None


def _player_command(path: Path) -> list[str] | None:
    if path.suffix.lower() in LOSSLESS_SUFFIXES:
        order = DIRECT_PLAYERS + DECODING_PLAYERS
    else:
        order = DECODING_PLAYERS + DIRECT_PLAYERS
    for name in order:
        executable = shutil.which(name)
        if not executable:
            continue
        if name == "ffplay":
            return [executable, "-nodisp", "-autoexit", "-loglevel", "quiet", str(path)]
        if name == "mpv":
            return [executable, "--no-video", "--really-quiet", str(path)]
        if name == "aplay":
            return [executable, "-q", str(path)]
        return [executable, str(path)]
    return None


def play_close_sound() -> None:
    """Compatibility shim: dismissing a window is intentionally silent."""


def play(role: str) -> None:
    """Play an enabled UI event's cue without waiting for completion or raising."""
    try:
        if not isinstance(role, str) or role.lower() not in EVENT_ROLES:
            return
        debug = os.environ.get("COGNITIVE_DEBUG") == "1"
        call_ns = time.monotonic_ns() if debug else None
        path = sound_path(role)
        if path is None or not path.is_file():
            return
        command = _player_command(path)
        if command is None:
            return
        spawn_ns = time.monotonic_ns() if debug else None
        subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        if debug:
            spawned_ns = time.monotonic_ns()
            canonical_role = EVENT_ROLES.get(role.lower(), role.lower())
            # Log only after spawning so journal I/O cannot delay the cue launch.
            print(
                f"[cognitive-popups] sound role={canonical_role} "
                f"player={Path(command[0]).name} call_ns={call_ns} "
                f"spawn_ns={spawn_ns} spawned_ns={spawned_ns} "
                f"call_to_spawn_ms={(spawn_ns - call_ns) / 1e6:.3f} "
                f"call_to_spawned_ms={(spawned_ns - call_ns) / 1e6:.3f}",
                file=sys.stderr,
                flush=True,
            )
    except Exception:
        # Audio is optional; it must never break the caller.
        pass
