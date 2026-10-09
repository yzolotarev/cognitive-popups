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
#: The universe runs on a timer: the timer is enabled, its oneshot service is
#: rendered next to it and started only by the timer.
UNIVERSE_UNITS = ("cognitive-universe.timer", "cognitive-universe.service")


@pytest.fixture
def sandbox(tmp_path):
    home = tmp_path / "test home"
    home.mkdir()
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    env = {key: value for key, value in os.environ.items()
           if not key.startswith("COGNITIVE_") and key not in
           ("XDG_CONFIG_HOME", "PYTHONPATH", "BASH_ENV", "ENV")}
    env.update(HOME=str(home), PATH=f"{bin_dir}:{os.defpath}",
               CALL_LOG=str(tmp_path / "calls.jsonl"))
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
                 "cognitive-tasks.sh"):
        shutil.copy2(ROOT / "scripts" / name, project / "scripts" / name)
    for name in UNITS + UNIVERSE_UNITS:
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
    templates = {unit: (project / "systemd" / unit).read_text() for unit in UNITS}
    result = run_script(project, "install-user.sh", env)
    assert result.returncode == 0, result.stderr
    for unit in UNITS:
        rendered = (unit_dir / unit).read_text()
        assert rendered == templates[unit].replace("@COGNITIVE_PROJECT@", str(project))
        assert f"WorkingDirectory={project}\n" in rendered
        for key, suffix in (("PYTHONPATH", "/src"), ("COGNITIVE_PROJECT", ""),
                            ("COGNITIVE_STATE_DIR", "/var")):
            assert f'Environment="{key}={project}{suffix}"' in rendered
        assert "Environment=DISPLAY=:0\n" in rendered
        assert "Environment=WAYLAND_DISPLAY=wayland-1\n" in rendered
        assert "Environment=XDG_RUNTIME_DIR=%t\n" in rendered
        module = "cognitive_popups.hud" if unit == "cognitive-hud.service" else "cognitive_popups"
        assert f"ExecStart=/usr/bin/python3 -m {module}\n" in rendered
        assert (project / "systemd" / unit).read_text() == templates[unit]
    universe = (unit_dir / "cognitive-universe.service").read_text()
    assert f"WorkingDirectory={project}\n" in universe
    assert f'Environment="COGNITIVE_STATE_DIR={project}/var"' in universe
    assert "ExecStart=/usr/bin/python3 -m cognitive_popups.universe sync" in universe
    assert (unit_dir / "cognitive-universe.timer").exists()
    assert [call["argv"][1:] for call in calls(env)] == [
        ["--user", "daemon-reload"],
        ["--user", "enable", "--now", *UNITS, "cognitive-universe.timer"],
    ]
    result = run_script(project, "uninstall-user.sh", env)
    assert result.returncode == 0, result.stderr
    assert all(not (unit_dir / unit).exists() for unit in UNITS + UNIVERSE_UNITS)
    assert unrelated.read_text() == "keep me"
    assert [call["argv"][1:] for call in calls(env)][2:] == [
        *(["--user", "disable", "--now", unit] for unit in UNITS + UNIVERSE_UNITS),
        ["--user", "daemon-reload"],
    ]


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
    assert call["env"]["COGNITIVE_STATE_DIR"] == str(selected / "var")
    assert call["env"]["PYTHONPATH"] == f"{selected}/src:/existing/python/path"
    assert call["env"]["GDK_BACKEND"] == "x11"
    if launcher == "cognitive-tasks.sh":
        assert call["argv"][1:] == ["-m", "cognitive_popups.tasks", "--menu"]
    else:
        assert call["argv"][1:] == ["python3", "-m", "cognitive_popups.hud"]
        assert (selected / "var/hud.log").exists()


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
        [analyzer, "--user", "verify", *(str(unit_dir / unit) for unit in UNITS + UNIVERSE_UNITS)],
        env=env, capture_output=True, text=True, timeout=15,
    )
    assert result.returncode == 0, result.stdout + result.stderr


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
    # Ctrl+Alt+G, Alt+R and Alt+Shift+R were retired or frozen on 2026-10-07.
    assert len(bindings) == 14
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
