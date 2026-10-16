"""One reduced-motion rule for every animation: Orbital, sand, timer, star, cutscenes.

Reduced motion means: show the final frame at once. It is on when any of
these says so:

- ``COGNITIVE_REDUCED_MOTION=1`` (also on/yes/true) in the environment;
- the flag file ``motion.reduced`` in the state directory;
- the legacy flag ``sand.reduced`` (``cognitive-sand.sh reduced``);
- GTK's ``gtk-enable-animations`` is off.

No GTK import here: the GTK setting is read only when a caller passes its
``Gtk`` module or GTK is already loaded in this process.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

from .paths import state_dir  # noqa: F401 - re-exported for the visual layer

FLAG = "motion.reduced"
LEGACY_FLAG = "sand.reduced"
TRUE = {"1", "on", "yes", "true"}


def _gtk_animations_off(Gtk=None) -> bool:
    if Gtk is None:
        Gtk = sys.modules.get("gi.repository.Gtk")
    if Gtk is None:
        return False
    try:
        settings = Gtk.Settings.get_default()
        return bool(settings is not None and not settings.get_property("gtk-enable-animations"))
    except Exception:  # noqa: BLE001 - a stub or odd theme never forces a mode
        return False


def reduced(Gtk=None) -> bool:
    """True when every animation should jump to its final frame."""
    if os.environ.get("COGNITIVE_REDUCED_MOTION", "").strip().lower() in TRUE:
        return True
    try:
        root = state_dir()
        if (root / FLAG).exists() or (root / LEGACY_FLAG).exists():
            return True
    except OSError:
        pass
    return _gtk_animations_off(Gtk)


def duration(seconds: float, Gtk=None) -> float:
    """``seconds``, or 0 under reduced motion (a tween then lands at once)."""
    return 0.0 if reduced(Gtk) else seconds


def set_reduced(on: bool) -> Path:
    """Write or remove the ``motion.reduced`` flag; returns its path."""
    path = state_dir() / FLAG
    if on:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()
    else:
        path.unlink(missing_ok=True)
    return path
