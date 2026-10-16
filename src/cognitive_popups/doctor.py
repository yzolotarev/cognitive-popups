"""`cog doctor`: one line per check, OK / WARN / FAIL, and how to fix it.

Read-only: nothing is created, migrated or started. Databases are opened with
`mode=ro`, the model is asked only for `/health` (3 s), the service only for
its state. Exit status 1 when any check FAILs; `--json` for scripts.

    python3 -m cognitive_popups.doctor [--json]
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from .paths import all_db_paths, state_dir

OK, WARN, FAIL = "OK", "WARN", "FAIL"
SIGNAL_SCRIPT = "cognitive-popups-signal.sh"
SERVICE = "cognitive-popups.service"
DATABASES = ("records", "events", "notes", "universe")


@dataclass
class Check:
    status: str
    name: str
    detail: str
    fix: str = ""


class Probe:
    """Everything the checks touch outside this process; tests replace it."""

    def which(self, name: str) -> str | None:
        return shutil.which(name)

    def run(self, argv: list[str], timeout: float = 3.0) -> tuple[int, str]:
        try:
            done = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        except (OSError, subprocess.SubprocessError) as exc:
            return 127, str(exc)
        return done.returncode, (done.stdout or "") + (done.stderr or "")

    def import_gtk(self) -> str | None:
        """None when Gtk 3.0 imports, otherwise the reason."""
        code = "import gi; gi.require_version('Gtk', '3.0'); from gi.repository import Gtk"
        status, output = self.run([sys.executable, "-c", code], timeout=10)
        return None if status == 0 else (output.strip().splitlines() or ["import failed"])[-1]

    def health(self) -> dict[str, Any]:
        from .client import client_from_env
        client = client_from_env()
        data = client.health(timeout=3.0)
        return {"url": client.url, "model": client.model, **data}

    def model_config(self) -> tuple[str, str]:
        from .client import client_from_env
        client = client_from_env()
        return client.url, client.model

    def pid_alive(self, pid: int) -> bool:
        try:
            os.kill(pid, 0)
        except PermissionError:
            return True
        except OSError:
            return False
        return True

    def stale_prompts(self, root: Path) -> tuple[list[str], list[str]]:
        """(stale override keys, custom keys) from prompts.json; read-only."""
        from . import prompt_settings
        path = Path(os.environ.get("COGNITIVE_PROMPTS_PATH") or root / "prompts.json")
        if not path.exists():
            return [], []
        settings = prompt_settings.PromptSettings(path)
        stale = list(getattr(settings, "stale_keys", lambda: [])())
        custom = [key for key in prompt_settings.PROMPT_KEYS
                  if key not in stale and settings.is_custom(key)]
        return stale, custom


def _size(path: Path) -> int:
    total = 0
    for entry in path.rglob("*"):
        try:
            if entry.is_file() and not entry.is_symlink():
                total += entry.stat().st_size
        except OSError:
            pass
    return total


def _human_size(size: int) -> str:
    for unit in ("Б", "КБ", "МБ", "ГБ"):
        if size < 1024 or unit == "ГБ":
            return f"{size:.0f} {unit}" if unit == "Б" else f"{size:.1f} {unit}"
        size /= 1024
    return str(size)


def _git_root(path: Path) -> Path | None:
    for parent in (path, *path.parents):
        if (parent / ".git").exists():
            return parent
    return None


def check_python() -> Check:
    version = ".".join(map(str, sys.version_info[:3]))
    if sys.version_info >= (3, 10):
        return Check(OK, "python", version)
    return Check(FAIL, "python", version, "нужен Python 3.10 или новее")


def check_gtk(probe: Probe) -> Check:
    error = probe.import_gtk()
    if error is None:
        return Check(OK, "gtk", "gi + Gtk 3.0 импортируются")
    return Check(FAIL, "gtk", error, "поставь PyGObject и GTK 3 (python3-gi, gir1.2-gtk-3.0)")


def check_tools(probe: Probe) -> list[Check]:
    checks = []
    clip = [name for name in ("wl-paste", "xclip") if probe.which(name)]
    checks.append(Check(OK, "буфер обмена", ", ".join(clip)) if clip else
                  Check(FAIL, "буфер обмена", "нет ни wl-paste, ни xclip", "поставь wl-clipboard"))
    for name, why in (("hyprctl", "бинды не проверить"), ("pw-play", "звука не будет")):
        checks.append(Check(OK, name, probe.which(name) or "") if probe.which(name) else
                      Check(WARN, name, f"не найден — {why}", ""))
    return checks


def check_state(probe: Probe, root: Path) -> list[Check]:
    from .observation import list_backups
    if not root.exists():
        return [Check(WARN, "каталог состояния", f"{root} ещё не создан",
                      "появится при первом запуске сервиса")]
    resolved = root.resolve()
    shown = f"{root}" + (f" -> {resolved}" if resolved != root else "")
    writable = os.access(resolved, os.W_OK)
    backups = sum(len(list_backups(path)) for path in all_db_paths().values())
    detail = f"{shown}, {_human_size(_size(resolved))}, резервных копий: {backups}"
    checks = [Check(OK if writable else FAIL, "каталог состояния", detail,
                    "" if writable else f"нет права записи: chmod u+w {resolved}")]
    if backups > 3 * len(DATABASES):
        checks.append(Check(WARN, "резервные копии", f"{backups} копий", "cog maintain оставит по три"))
    repo = _git_root(resolved)
    if repo is not None:
        status, _ = probe.run(["git", "-C", str(repo), "check-ignore", "-q", str(resolved)])
        if status != 0:
            checks.append(Check(WARN, "состояние в git", f"{resolved} внутри {repo} и не в .gitignore",
                                "добавь его в .gitignore: git clean -fdx удалит заметки"))
    return checks


def check_service(probe: Probe, root: Path) -> list[Check]:
    checks = []
    if probe.which("systemctl"):
        status, output = probe.run(["systemctl", "--user", "is-active", SERVICE])
        state = output.strip().splitlines()[0] if output.strip() else "unknown"
        checks.append(Check(OK, "сервис", state) if status == 0 else
                      Check(FAIL, "сервис", f"{SERVICE}: {state}",
                            f"systemctl --user start {SERVICE}; журнал: journalctl --user -u {SERVICE}"))
    else:
        checks.append(Check(WARN, "сервис", "systemctl не найден", "запусти сервис вручную"))
    for name, required in (("desktop.pid", True), ("hud.pid", False)):
        path = root / name
        if not path.exists():
            if required:
                checks.append(Check(WARN, name, "нет файла", "сервис не запущен или пишет в другой каталог"))
            continue
        try:
            pid = int(path.read_text().strip() or 0)
        except (OSError, ValueError):
            pid = 0
        if pid and probe.pid_alive(pid):
            checks.append(Check(OK, name, f"процесс {pid} жив"))
        else:
            checks.append(Check(WARN, name, f"процесс {pid or '?'} не найден (устаревший файл)",
                                "перезапусти сервис"))
    return checks


def check_binds(probe: Probe) -> Check | None:
    if not probe.which("hyprctl"):
        return None
    status, output = probe.run(["hyprctl", "binds", "-j"])
    if status != 0:
        return Check(WARN, "бинды", "hyprctl не ответил (Hyprland не запущен?)")
    if SIGNAL_SCRIPT in output:
        return Check(OK, "бинды", f"{output.count(SIGNAL_SCRIPT)} клавиш ведут в {SIGNAL_SCRIPT}")
    return Check(WARN, "бинды", "конфиг не подключён", "подключи config/hypr-v2.lua в конфиг Hyprland")


def check_model(probe: Probe) -> Check:
    url, model = probe.model_config()
    try:
        data = probe.health()
    except Exception as exc:  # noqa: BLE001 - every failure is one line here
        from .errors import human_error
        return Check(WARN, "модель", f"{model} @ {url}: {human_error(exc, url=url)}",
                     "запусти мост или задай COGNITIVE_API_URL / COGNITIVE_MODEL; заметки работают и без него")
    latency = data.get("latency_ms")
    return Check(OK, "модель", f"{model} @ {url}" + (f", {latency} мс" if latency is not None else ""))


def check_flags(probe: Probe, root: Path) -> Check:
    on: list[str] = []
    off: list[str] = []
    (on if (root / "sand.enabled").exists() else off).append("песок")
    sound = ((root / "sand.sound").exists() and not (root / "sand.silent").exists()
             and os.environ.get("COGNITIVE_SOUND", "").lower() not in {"0", "off", "false", "no"})
    (on if sound else off).append("звук песка")
    reduced = (os.environ.get("COGNITIVE_REDUCED_MOTION", "").strip().lower() in {"1", "on", "yes", "true"}
               or (root / "motion.reduced").exists() or (root / "sand.reduced").exists())
    (on if reduced else off).append("меньше движения")
    if probe.which("systemctl"):
        for label, unit in (("universe", "cognitive-universe.timer"), ("HUD", "cognitive-hud.service")):
            status, _ = probe.run(["systemctl", "--user", "is-enabled", unit])
            (on if status == 0 else off).append(label)
    if (root / "hud.frozen").exists() and "HUD" in on:
        on[on.index("HUD")] = "HUD (заморожен)"
    return Check(OK, "флаги", "вкл: " + (", ".join(on) or "—") + " · выкл: " + (", ".join(off) or "—"))


def check_prompts(probe: Probe, root: Path) -> Check:
    try:
        stale, custom = probe.stale_prompts(root)
    except Exception as exc:  # noqa: BLE001
        return Check(WARN, "промпты", f"prompts.json не читается: {type(exc).__name__}",
                     "python3 -m cognitive_popups.prompt_settings")
    if stale:
        return Check(WARN, "промпты", "устаревшие замены (не применяются): " + ", ".join(stale),
                     "открой их через scripts/cognitive-prompts.py или сбрось")
    return Check(OK, "промпты", ("свои: " + ", ".join(custom)) if custom else "встроенные")


def check_databases() -> list[Check]:
    checks = []
    for name, path in all_db_paths().items():
        if not path.exists():
            checks.append(Check(OK, f"{name}.sqlite3", "ещё нет (создастся при первой записи)"))
            continue
        try:
            conn = sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True, timeout=2)
            try:
                result = conn.execute("PRAGMA quick_check").fetchone()[0]
                version = conn.execute("PRAGMA user_version").fetchone()[0]
            finally:
                conn.close()
        except sqlite3.Error as exc:
            checks.append(Check(FAIL, f"{name}.sqlite3", str(exc), "проверь права и место на диске"))
            continue
        if result == "ok":
            checks.append(Check(OK, f"{name}.sqlite3", f"цела, схема {version}"))
        else:
            checks.append(Check(FAIL, f"{name}.sqlite3", f"quick_check: {result}",
                                "останови сервис и восстанови из резервной копии рядом с файлом"))
    return checks


def run_checks(probe: Probe | None = None, root: Path | None = None) -> list[Check]:
    probe = probe or Probe()
    root = root or state_dir()
    checks: list[Check] = [check_python(), check_gtk(probe), *check_tools(probe),
                           *check_state(probe, root), *check_service(probe, root)]
    binds = check_binds(probe)
    if binds is not None:
        checks.append(binds)
    checks.extend([check_model(probe), check_flags(probe, root), check_prompts(probe, root)])
    checks.extend(check_databases())
    return checks


def render(checks: list[Check]) -> str:
    lines = []
    for check in checks:
        lines.append(f"{check.status:<4}  {check.name}: {check.detail}")
        if check.fix and check.status != OK:
            lines.append(f"      → {check.fix}")
    failed = sum(check.status == FAIL for check in checks)
    warned = sum(check.status == WARN for check in checks)
    lines.append("")
    lines.append("всё в порядке" if not failed and not warned else
                 f"проблем: {failed}, предупреждений: {warned}")
    return "\n".join(lines)


def main(argv: list[str] | None = None, *, probe: Probe | None = None) -> int:
    parser = argparse.ArgumentParser(prog="cog doctor", description="Самопроверка: только чтение.")
    parser.add_argument("--json", action="store_true", help="результат в JSON")
    args = parser.parse_args(argv)
    checks = run_checks(probe)
    if args.json:
        print(json.dumps([asdict(check) for check in checks], ensure_ascii=False, indent=2))
    else:
        print(render(checks))
    return 1 if any(check.status == FAIL for check in checks) else 0


if __name__ == "__main__":  # pragma: no cover - CLI entry point
    sys.exit(main())
