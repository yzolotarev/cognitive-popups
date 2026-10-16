"""Portable checkout paths, without touching the user's services or state."""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time

import pytest


ROOT = Path(__file__).resolve().parents[1]
UNITS = ("cognitive-popups.service", "cognitive-hud.service")
#: Timers and the oneshot services they start. Only maintenance is enabled by
#: default; the universe timer is installed but off.
UNIVERSE_UNITS = ("cognitive-universe.timer", "cognitive-universe.service")
MAINTAIN_UNITS = ("cognitive-maintain.timer", "cognitive-maintain.service")
ALL_UNITS = UNITS + UNIVERSE_UNITS + MAINTAIN_UNITS
ENABLED = ["cognitive-popups.service", "cognitive-maintain.timer"]
DEAD_BRIDGE = "http://127.0.0.1:9/v1/chat/completions"


@pytest.fixture
def sandbox(tmp_path):
    home = tmp_path / "test home"
    home.mkdir()
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    env = {key: value for key, value in os.environ.items()
           if not key.startswith("COGNITIVE_") and key not in
           ("XDG_CONFIG_HOME", "XDG_STATE_HOME", "PYTHONPATH", "BASH_ENV", "ENV")}
    env.update(HOME=str(home), PATH=f"{bin_dir}:{os.defpath}",
               CALL_LOG=str(tmp_path / "calls.jsonl"), COGNITIVE_API_URL=DEAD_BRIDGE)
    # No real network in tests: curl says the bridge is down unless a test
    # replaces it.
    curl = bin_dir / "curl"
    curl.write_text("#!/bin/sh\nexit 7\n")
    curl.chmod(0o755)
    recorder = (
        f"#!{sys.executable}\n"
        "import json, os, sys\n"
        "with open(os.environ['CALL_LOG'], 'a') as stream:\n"
        "    stream.write(json.dumps({'argv': sys.argv, 'env': dict(os.environ)}) + '\\n')\n"
    )
    for name in ("systemctl", "python3", "setsid"):
        executable = bin_dir / name
        executable.write_text(recorder)
        executable.chmod(0o755)
    return tmp_path, env


def checkout(base, name):
    project = base / name
    for directory in ("scripts", "systemd"):
        (project / directory).mkdir(parents=True)
    for name in ("install-user.sh", "uninstall-user.sh", "cognitive-hud.sh",
                 "cognitive-tasks.sh", "cognitive-service.sh", "cog"):
        shutil.copy2(ROOT / "scripts" / name, project / "scripts" / name)
    for name in ALL_UNITS:
        shutil.copy2(ROOT / "systemd" / name, project / "systemd" / name)
    return project


def run_script(project, name, env, *args):
    return subprocess.run(
        ["bash", str(project / "scripts" / name), *args], env=env,
        cwd=env["HOME"], capture_output=True, text=True, timeout=10,
    )


def calls(env):
    path = Path(env["CALL_LOG"])
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def state_home(env) -> Path:
    return Path(env["HOME"]) / ".local/state/cognitive-popups"


@pytest.mark.parametrize("name", ["cognitive-popups", "cognitive-exoskeleton", "custom clone"])
@pytest.mark.parametrize("xdg", [False, True])
def test_install_and_uninstall(sandbox, name, xdg):
    base, env = sandbox
    project = checkout(base, name)
    if xdg:
        env["XDG_CONFIG_HOME"] = str(base / "custom config")
    config = Path(env.get("XDG_CONFIG_HOME", str(Path(env["HOME"]) / ".config")))
    unit_dir = config / "systemd/user"
    unit_dir.mkdir(parents=True)
    unrelated = unit_dir / "unrelated.service"
    unrelated.write_text("keep me")
    templates = {unit: (project / "systemd" / unit).read_text() for unit in ALL_UNITS}
    result = run_script(project, "install-user.sh", env)
    assert result.returncode == 0, result.stderr
    for unit in ALL_UNITS:
        rendered = (unit_dir / unit).read_text()
        assert rendered == templates[unit].replace("@COGNITIVE_PROJECT@", str(project))
        assert (project / "systemd" / unit).read_text() == templates[unit]
        assert "/usr/bin/python" not in rendered
        assert "@COGNITIVE_PROJECT@/var" not in templates[unit]
        assert "Requires=" not in rendered
        if unit.endswith(".service"):
            assert f"WorkingDirectory={project}\n" in rendered
            assert 'Environment="COGNITIVE_STATE_DIR=%S/cognitive-popups"' in rendered
            assert f'ExecStart="{project}/scripts/cognitive-service.sh" -m cognitive_popups' in rendered
            assert "DISPLAY=:0" not in rendered and "WAYLAND_DISPLAY=" not in rendered
    popups = (unit_dir / "cognitive-popups.service").read_text()
    assert "Wants=gemini-web2api.service" in popups
    assert "PartOf=graphical-session.target" in popups
    assert "Nice=19" in (unit_dir / "cognitive-maintain.service").read_text()
    assert "OnCalendar=weekly" in (unit_dir / "cognitive-maintain.timer").read_text()
    assert [call["argv"][1:] for call in calls(env)] == [
        ["--user", "daemon-reload"],
        ["--user", "enable", "--now", *ENABLED],
    ]
    assert "systemctl --user enable --now cognitive-hud.service" in result.stdout
    assert "systemctl --user enable --now cognitive-universe.timer" in result.stdout
    assert "WARNING: the model bridge does not answer at http://127.0.0.1:9/health" in result.stderr
    result = run_script(project, "uninstall-user.sh", env)
    assert result.returncode == 0, result.stderr
    assert all(not (unit_dir / unit).exists() for unit in ALL_UNITS)
    assert unrelated.read_text() == "keep me"
    assert [call["argv"][1:] for call in calls(env)][2:] == [
        *(["--user", "disable", "--now", unit] for unit in
          ("cognitive-popups.service", "cognitive-hud.service", *UNIVERSE_UNITS, *MAINTAIN_UNITS)),
        ["--user", "daemon-reload"],
    ]


def test_install_links_old_checkout_state_without_moving_it(sandbox):
    base, env = sandbox
    project = checkout(base, "cognitive-popups")
    (project / "var").mkdir()
    (project / "var/notes.sqlite3").write_bytes(b"my notes")
    result = run_script(project, "install-user.sh", env)
    assert result.returncode == 0, result.stderr
    link = state_home(env)
    assert link.is_symlink() and link.resolve() == (project / "var").resolve()
    assert (project / "var/notes.sqlite3").read_bytes() == b"my notes"
    assert "nothing was moved" in result.stdout
    # A second install changes nothing and says nothing alarming.
    again = run_script(project, "install-user.sh", env)
    assert again.returncode == 0 and "two state directories" not in again.stderr
    result = run_script(project, "uninstall-user.sh", env)
    assert "NOT deleted" in result.stdout and str(link) in result.stdout
    assert (project / "var/notes.sqlite3").read_bytes() == b"my notes"


def test_install_never_merges_two_state_directories(sandbox):
    base, env = sandbox
    project = checkout(base, "cognitive-popups")
    (project / "var").mkdir()
    (project / "var/records.sqlite3").write_bytes(b"old")
    state_home(env).mkdir(parents=True)
    (state_home(env) / "records.sqlite3").write_bytes(b"new")
    result = run_script(project, "install-user.sh", env)
    assert result.returncode == 0
    assert "two state directories" in result.stderr
    assert not state_home(env).is_symlink()
    assert (project / "var/records.sqlite3").read_bytes() == b"old"


def test_install_reports_a_live_bridge(sandbox):
    base, env = sandbox
    project = checkout(base, "cognitive-popups")
    curl = Path(env["PATH"].split(":")[0]) / "curl"
    curl.write_text("#!/bin/sh\nexit 0\n")
    result = run_script(project, "install-user.sh", env)
    assert result.returncode == 0
    assert "Model bridge: OK (http://127.0.0.1:9/health)" in result.stdout
    assert "WARNING" not in result.stderr


def test_cog_runs_modules_with_the_shared_state_dir(sandbox):
    base, env = sandbox
    project = checkout(base, "cognitive-popups")
    result = subprocess.run(["bash", str(project / "scripts/cog")], env=env, capture_output=True,
                            text=True, timeout=10)
    assert result.returncode == 2 and "usage: cog" in result.stderr
    for args, expected in ((["doctor", "--json"], ["-m", "cognitive_popups.doctor", "--json"]),
                           (["maintain", "--dry-run"], ["-m", "cognitive_popups.maintain", "--dry-run"]),
                           (["stats"], ["-m", "cognitive_popups.overview"]),
                           (["notes", "--repeats"], ["-m", "cognitive_popups.notes", "--repeats"]),
                           (["tail", "5"], ["-m", "cognitive_popups.event_log", "--tail", "5"])):
        Path(env["CALL_LOG"]).unlink(missing_ok=True)
        result = subprocess.run(["bash", str(project / "scripts/cog"), *args], env=env,
                                capture_output=True, text=True, timeout=10)
        assert result.returncode == 0, result.stderr
        call = calls(env)[0]
        assert call["argv"][1:] == expected
        assert call["env"]["COGNITIVE_STATE_DIR"] == str(state_home(env))
        assert call["env"]["PYTHONPATH"].startswith(f"{project}/src")


def test_scripts_never_call_bare_python_or_default_to_the_checkout():
    for script in (ROOT / "scripts").iterdir():
        if script.suffix not in {".sh", ""} or not script.is_file():
            continue
        text = script.read_text()
        assert "/usr/bin/python " not in text and "/usr/bin/python\n" not in text, script
        assert "/var}" not in text, script
    assert "var/" in (ROOT / ".gitignore").read_text().split()


@pytest.mark.parametrize("name", ["bad%h", "bad&name", "bad|name", 'bad"name',
                                  "bad\\name", "bad\nname", "bad$name", "bad'name", "trailing space "])
def test_install_rejects_unsupported_paths_before_changes(sandbox, name):
    base, env = sandbox
    project = checkout(base, name)
    result = run_script(project, "install-user.sh", env)
    assert result.returncode != 0
    assert "Unsupported checkout path" in result.stderr
    assert not (Path(env["HOME"]) / ".config").exists()
    assert not calls(env)


@pytest.mark.parametrize("name", ["cognitive-popups", "cognitive-exoskeleton", "custom clone"])
@pytest.mark.parametrize("override", [False, True])
@pytest.mark.parametrize("launcher", ["cognitive-tasks.sh", "cognitive-hud.sh"])
def test_launchers_resolve_checkout(sandbox, name, override, launcher):
    base, env = sandbox
    project = checkout(base, name)
    selected = base / "explicit project" if override else project
    selected.mkdir(exist_ok=True)
    if override:
        env["COGNITIVE_PROJECT"] = str(selected)
    env["PYTHONPATH"] = "/existing/python/path"
    result = run_script(project, launcher, env)
    assert result.returncode == 0, result.stderr
    # The HUD detaches; wait only for our recording stub, never a real GUI.
    deadline = time.monotonic() + 3
    while not calls(env) and time.monotonic() < deadline:
        time.sleep(0.01)
    recorded = calls(env)
    assert len(recorded) == 1
    call = recorded[0]
    assert call["env"]["COGNITIVE_PROJECT"] == str(selected)
    assert call["env"]["COGNITIVE_STATE_DIR"] == str(state_home(env))
    assert call["env"]["PYTHONPATH"] == f"{selected}/src:/existing/python/path"
    assert call["env"]["GDK_BACKEND"] == "x11"
    if launcher == "cognitive-tasks.sh":
        assert call["argv"][1:] == ["-m", "cognitive_popups.tasks", "--menu"]
    else:
        assert call["argv"][1:] == ["python3", "-m", "cognitive_popups.hud"]
        assert (state_home(env) / "hud.log").exists()


def test_rendered_units_pass_systemd_parser(sandbox):
    analyzer = shutil.which("systemd-analyze")
    if not analyzer:
        pytest.skip("systemd-analyze unavailable")
    base, env = sandbox
    project = checkout(base, "clone with spaces")
    result = run_script(project, "install-user.sh", env)
    assert result.returncode == 0, result.stderr
    unit_dir = Path(env["HOME"]) / ".config/systemd/user"
    # Satisfy the dependency locally; verify does not start any unit.
    (unit_dir / "gemini-web2api.service").write_text("[Service]\nExecStart=/usr/bin/true\n")
    result = subprocess.run(
        [analyzer, "--user", "verify", *(str(unit_dir / unit) for unit in ALL_UNITS)],
        env=env, capture_output=True, text=True, timeout=15,
    )
    output = result.stdout + result.stderr
    if result.returncode and any(reason in output for reason in (
            "Failed to lookup RuntimeDirectory", "No such device or address",
            "Failed to connect to bus", "XDG_RUNTIME_DIR")):
        pytest.skip("no systemd user manager here")
    assert result.returncode == 0, output


def test_task_arguments_and_state_override(sandbox):
    base, env = sandbox
    project = checkout(base, "cognitive-popups")
    env["COGNITIVE_STATE_DIR"] = str(base / "separate state")
    result = run_script(project, "cognitive-tasks.sh", env, "--practice", "two words")
    assert result.returncode == 0, result.stderr
    call = calls(env)[0]
    assert call["argv"][1:] == ["-m", "cognitive_popups.tasks", "--practice", "two words"]
    assert call["env"]["COGNITIVE_STATE_DIR"] == env["COGNITIVE_STATE_DIR"]


def test_hud_stop_uses_override_state_without_launching(sandbox):
    base, env = sandbox
    project = checkout(base, "cognitive-popups")
    env["COGNITIVE_STATE_DIR"] = str(base / "separate state")
    result = run_script(project, "cognitive-hud.sh", env, "--stop")
    assert result.returncode == 0, result.stderr
    assert not calls(env)
    assert not (project / "var").exists()


@pytest.mark.parametrize("mode", ["public", "legacy", "both", "override", "empty"])
def test_hyprland_checkout_selection_and_quoting(sandbox, mode):
    lua = shutil.which("lua")
    if not lua:
        pytest.skip("Lua interpreter unavailable")
    base, env = sandbox
    projects = Path(env["HOME"]) / "projects"
    public = projects / "cognitive-popups"
    legacy = projects / "cognitive-exoskeleton"
    for project in (public, legacy) if mode == "both" else (legacy,) if mode == "legacy" else (public,):
        (project / "scripts").mkdir(parents=True)
        (project / "scripts/cognitive-popups-signal.sh").touch()
    selected = legacy if mode == "legacy" else public
    if mode == "override":
        selected = base / "explicit ' checkout"
        env["COGNITIVE_PROJECT"] = str(selected)
    elif mode == "empty":
        env["COGNITIVE_PROJECT"] = ""
    harness = base / "bindings.lua"
    harness.write_text(
        'hl = {dsp = {exec_cmd = function(cmd) return cmd end},\n'
        'bind = function(key, cmd) print(key .. "\\t" .. cmd) end,\n'
        'window_rule = function(rule) end}\n'
        'dofile(arg[1])\n'
    )
    result = subprocess.run([lua, str(harness), str(ROOT / "config/hypr-v2.lua")],
                            env=env, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    bindings = dict(line.split("\t", 1) for line in result.stdout.splitlines())
    # Ctrl+Alt+G, Alt+R and Alt+Shift+R were retired or frozen on 2026-10-07; Alt+L added 2026-10-09.
    assert len(bindings) == 15
    for key, script, arguments in (
        ("ALT + W", "cognitive-popups-signal.sh", ["seed"]),
        ("CTRL + ALT + W", "cognitive-popups-signal.sh", ["seed-batch"]),
        ("ALT + F", "cognitive-popups-signal.sh", ["feynman"]),
        ("ALT + E", "cognitive-popups-signal.sh", ["note"]),
        ("ALT + C", "cognitive-popups-signal.sh", ["clarify"]),
        ("CTRL + Q", "cognitive-popups-signal.sh", ["summary"]),
        ("ALT + I", "cognitive-popups-signal.sh", ["intent"]),
        ("ALT + G", "cognitive-popups-signal.sh", ["example"]),
        ("ALT + SHIFT + G", "cognitive-popups-signal.sh", ["example-ask"]),
        ("ALT + T", "cognitive-tasks.sh", []),
        ("ALT + SHIFT + T", "cognitive-tasks.sh", ["--revisit-menu"]),
        ("ALT + Y", "cognitive-tasks.sh", ["--practice"]),
        ("ALT + K", "cognitive-popups-signal.sh", ["keys"]),
        ("ALT + H", "cognitive-hud.sh", []),
    ):
        assert shlex.split(bindings[key]) == [str(selected / "scripts" / script), *arguments]
