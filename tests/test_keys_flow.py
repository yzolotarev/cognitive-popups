"""Alt+K: the shortcut reference is local, complete and never calls the model.

The reference exists because the hotkeys are the primary way in. Two things
therefore have to hold: it must describe exactly the keys the configuration
binds, and opening it must cost nothing — no buffer, no model, no ledger write.
"""
import json
import re
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parent.parent


def _bound_keys() -> set[str]:
    """Key names the Hyprland configuration actually binds."""
    lua = (ROOT / "config" / "hypr-v2.lua").read_text(encoding="utf-8")
    bound = set()
    for line in lua.splitlines():
        if line.lstrip().startswith("--"):
            continue
        for key in re.findall(r'hl\.bind\("([^"]+)"', line):
            bound.add("+".join(part.strip().capitalize() for part in key.split("+")))
    return bound


def test_keys_reference_covers_every_binding_exactly(desktop):
    """The reference cannot drift from the keys it describes.

    A key the configuration binds must be documented, and the reference must not
    promise a combination that is never bound. Entries without a key of their own
    say where the action lives instead.
    """
    documented = {key for key, _action in desktop.KEYS_REFERENCE}

    assert documented - {"—"} == _bound_keys()


def test_keys_rows_are_complete(desktop):
    for key, action in desktop.KEYS_REFERENCE:
        assert key.strip()
        assert action.strip()
        assert "\n" not in key and "\n" not in action


def test_show_keys_needs_no_buffer_and_no_model(desktop):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app._busy = False
    # A session with nothing in it: the reference must not require reading work.
    app.service = SimpleNamespace(session=SimpleNamespace(fragments=[]))
    shown = []
    app.flash = SimpleNamespace(show_keys=lambda rows: shown.append(rows))
    desktop.begin_interaction("hotkey", window="keys")

    app.show_keys()

    assert shown == [desktop.KEYS_REFERENCE]


def test_the_keys_window_payload_carries_the_rows(desktop, monkeypatch):
    written = []

    class _Stdin:
        def write(self, text):
            written.append(text)

        def close(self):
            pass

    class _Proc:
        stdin = _Stdin()

    monkeypatch.setattr(desktop, "popup_helper_command", lambda: ["helper"])
    monkeypatch.setattr(desktop, "cursor_position", lambda: None)
    monkeypatch.setattr(desktop, "prepare_popup", lambda payload, window: payload)
    monkeypatch.setattr(desktop.subprocess, "Popen", lambda *a, **k: _Proc())
    desktop.begin_interaction("hotkey", window="keys")

    desktop.FlashWindow().show_keys([("Alt+W", "четыре слова"), ("Alt+K", "справка")])

    payload = json.loads(written[0])
    assert payload["mode"] == "keys"
    assert payload["rows"] == [["Alt+W", "четыре слова"], ["Alt+K", "справка"]]
