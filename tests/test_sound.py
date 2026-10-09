import importlib.util
import math
import subprocess
import wave
from array import array
from pathlib import Path

import pytest

from cognitive_popups import sound


def test_sound_off_by_explicit_setting_even_when_file_exists(monkeypatch, tmp_path):
    (tmp_path / "close-sound.wav").write_bytes(b"RIFF")
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    monkeypatch.setenv("COGNITIVE_CLOSE_SOUND", "off")

    assert sound.sound_path() is None


def test_sound_uses_explicit_path(monkeypatch, tmp_path):
    chosen = tmp_path / "my-bell.ogg"
    monkeypatch.setenv("COGNITIVE_CLOSE_SOUND", str(chosen))

    assert sound.sound_path() == chosen


def test_sound_falls_back_to_first_default_in_state_dir(monkeypatch, tmp_path):
    monkeypatch.delenv("COGNITIVE_CLOSE_SOUND", raising=False)
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    (tmp_path / "close-sound.ogg").write_bytes(b"OggS")

    assert sound.sound_path() == tmp_path / "close-sound.ogg"


def test_sound_absent_is_none(monkeypatch, tmp_path):
    monkeypatch.delenv("COGNITIVE_CLOSE_SOUND", raising=False)
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))

    assert sound.sound_path() is None


def test_lossless_sound_prefers_native_player(monkeypatch):
    monkeypatch.setattr(sound.shutil, "which", lambda name: f"/usr/bin/{name}")

    command = sound._player_command(Path("/x/close-sound.wav"))

    assert command[0] == "/usr/bin/pw-play"
    assert command[-1] == "/x/close-sound.wav"


def test_lossy_sound_prefers_decoding_player(monkeypatch):
    monkeypatch.setattr(sound.shutil, "which", lambda name: f"/usr/bin/{name}")

    command = sound._player_command(Path("/x/close-sound.mp3"))

    assert command[:2] == ["/usr/bin/ffplay", "-nodisp"]
    assert command[-1] == "/x/close-sound.mp3"


def test_no_player_available_is_none(monkeypatch):
    monkeypatch.setattr(sound.shutil, "which", lambda name: None)

    assert sound._player_command(Path("/x/close-sound.wav")) is None


def test_play_is_fire_and_forget_and_detached(monkeypatch, tmp_path):
    audio = tmp_path / "close-sound.wav"
    audio.write_bytes(b"RIFF")
    calls = {}
    monkeypatch.setattr(sound, "sound_path", lambda role: audio)
    monkeypatch.setattr(sound, "_player_command", lambda path: ["pw-play", str(path)])

    def fake_popen(command, **kwargs):
        calls["command"] = command
        calls["kwargs"] = kwargs
        return None

    monkeypatch.setattr(subprocess, "Popen", fake_popen)

    sound.play("orbital_close")

    assert calls["command"] == ["pw-play", str(audio)]
    assert calls["kwargs"]["start_new_session"] is True
    for stream in ("stdin", "stdout", "stderr"):
        assert calls["kwargs"][stream] == subprocess.DEVNULL


def test_play_swallows_missing_file_and_player_errors(monkeypatch):
    monkeypatch.setattr(sound, "sound_path", lambda role: None)
    monkeypatch.setattr(
        subprocess, "Popen",
        lambda *a, **k: (_ for _ in ()).throw(AssertionError("must not spawn")),
    )

    sound.play(sound.REVEAL)

    monkeypatch.setattr(sound, "sound_path", lambda role: Path("/does/not/exist.wav"))
    sound.play(sound.REVEAL)


@pytest.fixture
def private_sounds(monkeypatch, tmp_path):
    for key in ("COGNITIVE_SOUND", "COGNITIVE_SOUND_DIR", *(f"COGNITIVE_SOUND_{r.upper()}" for r in sound.ROLES)):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    root = tmp_path / "sounds"
    root.mkdir()
    return root


@pytest.mark.parametrize("role", sound.ROLES)
def test_roles_resolve_but_bare_roles_are_frozen(monkeypatch, private_sounds, role):
    asset = private_sounds / f"{role}.wav"
    asset.write_bytes(b"RIFF")
    assert sound.sound_path(role.upper()) == asset
    monkeypatch.setattr(sound, "_player_command", lambda path: ["player", str(path)])
    monkeypatch.setattr(subprocess, "Popen", lambda *a, **k: pytest.fail("frozen role must not spawn"))
    sound.play(role)


@pytest.mark.parametrize("event", ["orbital_close"])
def test_only_orbital_fold_and_close_play_cheers(monkeypatch, private_sounds, event):
    asset = private_sounds / "resolve.wav"
    asset.write_bytes(b"RIFF")
    for role in ("materialize", "reveal", "correction"):
        (private_sounds / f"{role}.wav").write_bytes(b"RIFF")
    calls = []
    monkeypatch.setattr(sound, "_player_command", lambda path: ["player", str(path)])
    monkeypatch.setattr(subprocess, "Popen", lambda command, **kwargs: calls.append(command))
    for silent in ("orbital_open", "term_reveal", "meaning_reveal", "evidence_reveal", "cue_select",
                   "orbital_collapse"):  # folding a cue back is silent (owner, 07.10)
        sound.play(silent)
    assert calls == []
    sound.play(event)
    assert calls == [["player", str(asset)]]


@pytest.mark.parametrize("value", sorted(sound.DISABLED))
def test_global_and_role_mute(monkeypatch, private_sounds, value):
    (private_sounds / "reveal.wav").write_bytes(b"RIFF")
    monkeypatch.setenv("COGNITIVE_SOUND_REVEAL", value)
    assert sound.sound_path(sound.REVEAL) is None
    monkeypatch.delenv("COGNITIVE_SOUND_REVEAL")
    monkeypatch.setenv("COGNITIVE_SOUND", value)
    assert sound.sound_path(sound.REVEAL) is None


def test_overrides_and_priority(monkeypatch, private_sounds, tmp_path):
    monkeypatch.setenv("COGNITIVE_SOUND_DIR", str(tmp_path))
    for suffix in (".ogg", ".wav"):
        (tmp_path / f"resolve{suffix}").write_bytes(b"audio")
    assert sound.sound_path(sound.RESOLVE) == tmp_path / "resolve.wav"
    monkeypatch.setenv("COGNITIVE_SOUND_RESOLVE", str(tmp_path / "missing.wav"))
    assert sound.sound_path(sound.RESOLVE) == tmp_path / "missing.wav"


def test_dismiss_always_silent(monkeypatch, private_sounds):
    monkeypatch.setenv("COGNITIVE_CLOSE_SOUND", str(private_sounds / "close.wav"))
    monkeypatch.setattr(sound, "sound_path", lambda *args: pytest.fail("dismiss must not resolve assets"))
    sound.play_close_sound()


@pytest.mark.parametrize("role", ["dismiss", "../reveal", "", None, 42])
def test_invalid_and_absent_roles_silent(monkeypatch, private_sounds, role):
    monkeypatch.setattr(subprocess, "Popen", lambda *args, **kwargs: pytest.fail("must not spawn"))
    sound.play(role)
    sound.play(sound.CORRECTION)


def test_spawn_error_and_no_player_are_silent(monkeypatch, private_sounds):
    asset = private_sounds / "materialize.wav"
    asset.write_bytes(b"RIFF")
    monkeypatch.setattr(sound, "_player_command", lambda path: None)
    sound.play(sound.MATERIALIZE)
    monkeypatch.setattr(sound, "_player_command", lambda path: ["missing-player"])
    def fail(*args, **kwargs):
        raise OSError("player disappeared")
    monkeypatch.setattr(subprocess, "Popen", fail)
    sound.play(sound.MATERIALIZE)


@pytest.mark.parametrize("setting", [None, "0", "true", "1"])
def test_debug_spawn_timestamps_only_when_enabled(monkeypatch, private_sounds, capsys, setting):
    asset = private_sounds / "resolve.wav"
    asset.write_bytes(b"RIFF")
    if setting is None:
        monkeypatch.delenv("COGNITIVE_DEBUG", raising=False)
    else:
        monkeypatch.setenv("COGNITIVE_DEBUG", setting)
    ticks = iter([1_000_000_000, 1_002_000_000, 1_005_000_000])
    def clock():
        assert setting == "1", "normal playback must not sample debug clocks"
        return next(ticks)
    monkeypatch.setattr(sound.time, "monotonic_ns", clock)
    monkeypatch.setattr(sound, "_player_command", lambda path: ["/usr/bin/pw-play", str(path)])
    calls = []
    def spawn(command, **kwargs):
        assert capsys.readouterr().err == "", "logging must happen after spawning"
        calls.append(command)
    monkeypatch.setattr(subprocess, "Popen", spawn)
    sound.play("orbital_close")
    output = capsys.readouterr()
    assert calls == [["/usr/bin/pw-play", str(asset)]]
    assert output.out == ""
    if setting == "1":
        assert output.err == (
            "[cognitive-popups] sound role=resolve player=pw-play "
            "call_ns=1000000000 spawn_ns=1002000000 spawned_ns=1005000000 "
            "call_to_spawn_ms=2.000 call_to_spawned_ms=5.000\n"
        )
        assert str(asset) not in output.err
    else:
        assert output.err == ""


def test_debug_dismiss_and_muted_cues_do_not_log_or_spawn(monkeypatch, private_sounds, capsys):
    monkeypatch.setenv("COGNITIVE_DEBUG", "1")
    monkeypatch.setenv("COGNITIVE_SOUND", "off")
    monkeypatch.setattr(subprocess, "Popen", lambda *a, **k: pytest.fail("must not spawn"))
    sound.play_close_sound()
    sound.play("cue_select")
    sound.play(sound.MATERIALIZE)
    assert capsys.readouterr().err == ""


def test_debug_logging_failure_does_not_retry_or_raise(monkeypatch, private_sounds):
    (private_sounds / "resolve.wav").write_bytes(b"RIFF")
    monkeypatch.setenv("COGNITIVE_DEBUG", "1")
    monkeypatch.setattr(sound, "_player_command", lambda path: ["pw-play", str(path)])
    calls = []
    monkeypatch.setattr(subprocess, "Popen", lambda command, **kwargs: calls.append(command))
    class BrokenStream:
        def write(self, text):
            raise OSError("journal unavailable")
    monkeypatch.setattr(sound.sys, "stderr", BrokenStream())
    sound.play("orbital_close")
    assert len(calls) == 1


def importer():
    path = Path(__file__).resolve().parents[1] / "scripts" / "import-cognitive-sounds.py"
    if not path.exists():
        pytest.skip("the sound importer is private and not shipped in the public release")
    spec = importlib.util.spec_from_file_location("import_sounds", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_import_preserves_preroll_tail_and_normalizes(tmp_path):
    source, dest = tmp_path / "in.wav", tmp_path / "out.wav"
    samples = array("h", [500] * 1000)
    for i in range(300, 500):
        samples[i] += round(8000 * math.sin(2 * math.pi * (i - 300) / 20))
    with wave.open(str(source), "wb") as writer:
        writer.setparams((1, 2, 1000, 0, "NONE", "not compressed"))
        writer.writeframes(samples.tobytes())
    stats = importer().prepare(source, dest)
    with wave.open(str(dest), "rb") as reader:
        assert reader.getnchannels() == 1
        assert reader.getsampwidth() == 2
        output = array("h", reader.readframes(reader.getnframes()))
    assert stats["trim_start"] == pytest.approx(0.298)
    assert stats["duration"] == pytest.approx(0.452)
    assert max(abs(v) for v in output) == pytest.approx(32767 * 10 ** (-3 / 20), abs=1)
    assert all(v == 0 for v in output[:3])
    assert all(v == 0 for v in output[-250:])


def test_import_rejects_dc_only(tmp_path):
    source = tmp_path / "dc.wav"
    with wave.open(str(source), "wb") as writer:
        writer.setparams((1, 2, 1000, 0, "NONE", "not compressed"))
        writer.writeframes(array("h", [100] * 1000).tobytes())
    with pytest.raises(ValueError, match="no audible signal"):
        importer().prepare(source, tmp_path / "out.wav")


def test_import_stereo_preserves_balance_without_clipping(tmp_path):
    source, dest = tmp_path / "stereo.wav", tmp_path / "ready.wav"
    samples = array("h")
    for i in range(1000):
        value = round(4000 * math.sin(2 * math.pi * i / 20))
        samples.extend((value + 200, value // 2 - 100))
    with wave.open(str(source), "wb") as writer:
        writer.setparams((2, 2, 1000, 0, "NONE", "not compressed"))
        writer.writeframes(samples.tobytes())
    importer().prepare(source, dest)
    with wave.open(str(dest), "rb") as reader:
        assert reader.getnchannels() == 2
        output = array("h", reader.readframes(reader.getnframes()))
    assert max(abs(v) for v in output) < 32767
    assert max(output[1::2]) / max(output[0::2]) == pytest.approx(0.5, abs=0.001)


def test_failed_import_preserves_existing_assets_and_continues(monkeypatch, tmp_path):
    module = importer()
    existing = tmp_path / "reveal.wav"
    existing.write_bytes(b"original")
    calls = []
    def failed_run(command, timeout):
        calls.append(command)
        raise subprocess.TimeoutExpired(command, timeout)
    monkeypatch.setattr(module, "run", failed_run)
    monkeypatch.setattr(module.shutil, "which", lambda name: name)
    monkeypatch.setattr(module.sys, "argv", ["import", "--output-dir", str(tmp_path), "--force", "--role", "reveal", "--role", "resolve"])
    assert module.main() == 1
    assert len(calls) == 2
    assert existing.read_bytes() == b"original"
    assert sorted(p.name for p in tmp_path.iterdir()) == ["reveal.wav"]


@pytest.mark.parametrize("event,role", sound.EVENT_ROLES.items())
def test_ui_event_aliases(private_sounds, event, role):
    asset = private_sounds / f"{role}.wav"
    asset.write_bytes(b"RIFF")
    assert sound.sound_path(event) == asset
    assert sound.sound_path("cue_select") is None


def test_source_mapping_and_gain_hierarchy():
    module = importer()
    assert [url.rsplit("/", 1)[-1] for url in module.SOURCES.values()] == [
        "vAisWjP49xA", "nUsHboYC5zc", "Ka3DUKCbL-k", "85KE84f3_6g",
    ]
    assert list(module.ROLE_GAIN_DB.values()) == [-6, -3, -1, 0]


def test_import_skips_existing_assets(monkeypatch, tmp_path):
    module = importer()
    (tmp_path / "reveal.wav").write_bytes(b"original")
    monkeypatch.setattr(module.shutil, "which", lambda name: name)
    monkeypatch.setattr(module, "run", lambda *args: pytest.fail("must not download"))
    monkeypatch.setattr(module.sys, "argv", ["import", "--output-dir", str(tmp_path), "--role", "reveal"])
    assert module.main() == 0
