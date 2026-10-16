"""One state directory for the service, the scripts and the CLI.

Everything the app writes (the four databases, pid files, prompts, flags) lives
under `state_dir()`. The order is:

    COGNITIVE_STATE_DIR                       explicit override (systemd units set it)
    $XDG_STATE_HOME/cognitive-popups          the XDG location
    ~/.local/state/cognitive-popups           the XDG default when the variable is unset

The value is read on every call, never cached at import: a module that froze it
at import time could not be pointed elsewhere by a test or a wrapper script.
"""
from __future__ import annotations

import os
from pathlib import Path

APP = "cognitive-popups"
ENV_STATE_DIR = "COGNITIVE_STATE_DIR"

#: Per-store overrides that still win over `state_dir()` for one file.
DB_ENV = {
    "records.sqlite3": "COGNITIVE_RECORD_DB",
    "events.sqlite3": "COGNITIVE_EVENT_DB",
    "notes.sqlite3": "COGNITIVE_NOTES_DB",
    "universe.sqlite3": "COGNITIVE_UNIVERSE_DB",
}


def state_dir() -> Path:
    """The state root, resolved now (see module docstring)."""
    explicit = os.environ.get(ENV_STATE_DIR)
    if explicit:
        return Path(explicit).expanduser()
    xdg = os.environ.get("XDG_STATE_HOME")
    if xdg:
        return Path(xdg).expanduser() / APP
    return Path("~/.local/state").expanduser() / APP


def db_path(name: str) -> Path:
    """One of the four databases, honouring its own override variable."""
    override = os.environ.get(DB_ENV.get(name, ""), "")
    return Path(override).expanduser() if override else state_dir() / name


def all_db_paths() -> dict[str, Path]:
    return {name.split(".")[0]: db_path(name) for name in DB_ENV}
