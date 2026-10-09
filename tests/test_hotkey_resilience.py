"""A failed hotkey action must not deafen the key for the rest of the run."""
import json


def test_failed_action_keeps_listening_and_is_logged(desktop, monkeypatch):
    logged = []
    monkeypatch.setattr(desktop, "emit", lambda event, **fields: logged.append((event, fields)))
    calls = []

    def action():
        calls.append(1)
        if len(calls) == 1:
            raise RuntimeError("boom")

    handler = desktop.keep_listening(action)
    # GLib keeps a signal source only while its callback returns True.
    assert handler() is True
    assert handler() is True
    assert len(calls) == 2
    [(event, fields)] = logged
    assert event == "error" and fields["window"] == "hotkey"
    assert fields["detail"] == "RuntimeError: boom"
    assert "RuntimeError: boom" in json.loads(fields["payload_json"])["traceback"]


def test_logging_failure_still_keeps_listening(desktop, monkeypatch):
    def broken(*_args, **_kwargs):
        raise OSError("log unavailable")
    monkeypatch.setattr(desktop, "emit", broken)
    def action():
        raise ValueError("x")
    assert desktop.keep_listening(action)() is True


def test_binary_clipboard_is_no_selection_not_a_crash(desktop):
    # A copied screenshot: PNG bytes are not UTF-8 and used to raise out of
    # the selection read, taking Alt+I (and every queued hotkey) down with it.
    png = "printf '\\211PNG\\r\\n\\032\\n'"
    assert desktop.run_probe(["sh", "-c", png]) == (
        None, "", "not text (binary clipboard content)")
    assert desktop.run_probe(["sh", "-c", "printf 'текст'"]) == (0, "текст", "")
