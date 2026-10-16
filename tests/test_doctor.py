"""cog doctor with every outside dependency faked."""
import json
import os
import sqlite3

import pytest

from cognitive_popups import doctor
from cognitive_popups.client import Web2APIError
from cognitive_popups.records import RecordStore


class FakeProbe(doctor.Probe):
    def __init__(self, *, tools=("wl-paste", "hyprctl", "systemctl", "pw-play", "git"), gtk=None,
                 active=True, binds='[{"arg": "/x/scripts/cognitive-popups-signal.sh seed"}]',
                 health=True, enabled=(), ignored=True, alive=True, stale=(), custom=()):
        self.tools, self.gtk, self.active, self.binds = set(tools), gtk, active, binds
        self._health, self.enabled, self.ignored, self.alive = health, set(enabled), ignored, alive
        self.stale, self.custom = list(stale), list(custom)
        self.commands = []

    def which(self, name):
        return f"/usr/bin/{name}" if name in self.tools else None

    def run(self, argv, timeout=3.0):
        self.commands.append(argv)
        if argv[:3] == ["systemctl", "--user", "is-active"]:
            return (0, "active\n") if self.active else (3, "inactive\n")
        if argv[:3] == ["systemctl", "--user", "is-enabled"]:
            return (0, "enabled") if argv[3] in self.enabled else (1, "disabled")
        if argv[:2] == ["hyprctl", "binds"]:
            return 0, self.binds
        if argv[0] == "git":
            return (0, "") if self.ignored else (1, "")
        raise AssertionError(argv)

    def import_gtk(self):
        return self.gtk

    def model_config(self):
        return "http://127.0.0.1:8081/v1/chat/completions", "test-model"

    def health(self):
        if not self._health:
            raise Web2APIError("refused", kind="transport", url="http://127.0.0.1:8081/v1/chat/completions")
        return {"status": "ok", "latency_ms": 12}

    def pid_alive(self, pid):
        return self.alive

    def stale_prompts(self, root):
        return self.stale, self.custom


def _by_name(checks):
    return {check.name: check for check in checks}


@pytest.fixture
def healthy_state(tmp_path):
    RecordStore().sessions()
    (tmp_path / "desktop.pid").write_text(str(os.getpid()))
    return tmp_path


def test_healthy_machine_is_all_ok(healthy_state, capsys):
    checks = doctor.run_checks(FakeProbe(), healthy_state)
    bad = [c for c in checks if c.status != doctor.OK]
    assert bad == []
    names = _by_name(checks)
    assert "test-model @ http://127.0.0.1:8081" in names["модель"].detail
    assert names["records.sqlite3"].detail.startswith("цела, схема")
    assert names["events.sqlite3"].detail.startswith("ещё нет")
    assert "1 клавиш" in names["бинды"].detail
    assert str(healthy_state) in names["каталог состояния"].detail
    assert doctor.main([], probe=FakeProbe()) == 0
    assert "всё в порядке" in capsys.readouterr().out


def test_failures_and_warnings_each_say_how_to_fix(healthy_state):
    probe = FakeProbe(tools=("systemctl", "git"), gtk="No module named 'gi'", active=False,
                      health=False, alive=False, ignored=False, stale=["four_words"])
    names = _by_name(doctor.run_checks(probe, healthy_state))
    assert names["gtk"].status == doctor.FAIL and "PyGObject" in names["gtk"].fix
    assert names["буфер обмена"].status == doctor.FAIL
    assert names["hyprctl"].status == doctor.WARN
    assert "бинды" not in names
    assert names["сервис"].status == doctor.FAIL and "journalctl" in names["сервис"].fix
    assert names["desktop.pid"].status == doctor.WARN
    model = names["модель"]
    assert model.status == doctor.WARN and "мост 127.0.0.1:8081 недоступен" in model.detail
    assert names["промпты"].status == doctor.WARN and "four_words" in names["промпты"].detail


def test_unhooked_binds_and_state_inside_an_unignored_checkout(tmp_path):
    root = tmp_path / "repo" / "var"
    root.mkdir(parents=True)
    (tmp_path / "repo" / ".git").mkdir()
    names = _by_name(doctor.run_checks(FakeProbe(binds="[]", ignored=False), root))
    assert names["бинды"].status == doctor.WARN and names["бинды"].detail == "конфиг не подключён"
    assert names["состояние в git"].status == doctor.WARN
    names = _by_name(doctor.run_checks(FakeProbe(ignored=True), root))
    assert "состояние в git" not in names


def test_flags_list_what_is_on(healthy_state, monkeypatch):
    (healthy_state / "sand.enabled").touch()
    monkeypatch.setenv("COGNITIVE_REDUCED_MOTION", "1")
    flags = _by_name(doctor.run_checks(FakeProbe(enabled={"cognitive-universe.timer"}), healthy_state))["флаги"]
    on, off = flags.detail.split(" · ")
    assert "песок" in on and "меньше движения" in on and "universe" in on
    assert "звук песка" in off and "HUD" in off


def test_corrupt_database_fails(healthy_state):
    path = healthy_state / "notes.sqlite3"
    path.write_bytes(b"not a database at all" * 100)
    names = _by_name(doctor.run_checks(FakeProbe(), healthy_state))
    assert names["notes.sqlite3"].status == doctor.FAIL
    assert doctor.main([], probe=FakeProbe()) == 1


def test_missing_state_dir_is_a_warning_and_nothing_is_created(tmp_path):
    root = tmp_path / "nowhere"
    names = _by_name(doctor.run_checks(FakeProbe(), root))
    assert names["каталог состояния"].status == doctor.WARN
    assert not root.exists()


def test_doctor_is_read_only(healthy_state):
    """SQLite may add its WAL side files to read a WAL database; nothing else appears."""
    def files():
        return sorted(p.name for p in healthy_state.iterdir() if not p.name.endswith(("-wal", "-shm")))
    path = healthy_state / "records.sqlite3"
    before = path.stat().st_mtime_ns, files()
    doctor.run_checks(FakeProbe(), healthy_state)
    assert (path.stat().st_mtime_ns, files()) == before


def test_json_output(healthy_state, capsys):
    assert doctor.main(["--json"], probe=FakeProbe()) == 0
    data = json.loads(capsys.readouterr().out)
    assert {"status", "name", "detail", "fix"} <= set(data[0])


def test_real_probe_prompts_and_pid(tmp_path):
    probe = doctor.Probe()
    assert probe.stale_prompts(tmp_path) == ([], [])
    assert probe.pid_alive(os.getpid())
    assert probe.run(["definitely-not-a-command-xyz"])[0] == 127
