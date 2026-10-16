"""Alt+W on the same text twice, one four-words window, a draft when the model fails."""
import subprocess
import sys


def test_same_text_within_two_minutes_is_a_repeat(desktop):
    assert desktop.repeat_seed(("k", 100.0), "k", 150.0)
    assert not desktop.repeat_seed(("k", 100.0), "k", 100.0 + desktop.SEED_REPEAT_S)
    assert not desktop.repeat_seed(("k", 100.0), "other", 110.0)
    assert not desktop.repeat_seed(None, "k", 110.0)


def test_offline_cues_take_words_from_the_text(desktop):
    text = ("Энтропия растёт в замкнутой системе. Энтропия связана с числом микросостояний. "
            "Микросостояния считает статистическая механика. Температура задаёт распределение.")
    draft = desktop.offline_cues(text)
    assert draft
    for word in draft.split(" · "):
        assert word.lower()[:5] in text.lower()
    assert desktop.offline_cues("   ") == ""


def test_new_cue_window_replaces_the_old_one(desktop, monkeypatch):
    spawned = []
    real_popen = subprocess.Popen

    def popen(command, **kwargs):
        proc = real_popen([sys.executable, "-c", "import time; time.sleep(30)"], stdin=subprocess.PIPE, text=True)
        spawned.append(proc)
        return proc

    monkeypatch.setattr(desktop, "popup_helper_command", lambda: ["helper"])
    monkeypatch.setattr(desktop, "cursor_position", lambda: None)
    monkeypatch.setattr(desktop, "prepare_popup", lambda payload, mode: payload)
    monkeypatch.setattr(desktop, "popup_env", lambda: {})
    monkeypatch.setattr(desktop.subprocess, "Popen", popen)
    flash = desktop.FlashWindow.__new__(desktop.FlashWindow)
    cue = {"simple": "рост", "term": "энтропия", "meaning": "мера беспорядка"}
    try:
        flash.show_cues([cue])
        flash.show_cues([cue])
        assert len(spawned) == 2
        assert spawned[0].wait(timeout=5) is not None
        assert spawned[1].poll() is None
    finally:
        for proc in spawned:
            proc.kill()
            proc.wait()
