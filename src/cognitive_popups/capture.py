"""Универсальный захват выделенного текста поверх любого окна (Hyprland, Wayland).

Идея: программа сама нажимает «копировать» в активном окне, забирает текст
и возвращает буфер обмена в прежнее состояние. Работает одинаково в браузере,
Okular и почти любом приложении, без расширений и без помощи приложения.

Только стандартная библиотека Python. Внешние команды уже есть в проекте:
hyprctl, wl-copy, wl-paste.

Проверено в собственном GTK-окне на Hyprland 0.56.2 с русской раскладкой.

Встраивание: сделать первой пробой в `desktop.selected_text_source`,
существующие пробы primary/clipboard оставить запасными.
"""
from __future__ import annotations

import json
import os
import subprocess
import time
import uuid
from threading import RLock
from dataclasses import dataclass, field
from pathlib import Path

# --- Настройки -----------------------------------------------------------

# Окна терминалов: Ctrl+C там прерывает программу, копирование другое.
TERMINAL_CLASSES = {
    "kitty", "foot", "footclient", "alacritty", "org.wezfurlong.wezterm",
    "com.mitchellh.ghostty", "wezterm", "konsole", "gnome-terminal-server",
    "xterm", "urxvt", "st", "tilix",
}
# Окна, где текст почти всегда про разработку, а не про учёбу.
DEV_CLASSES = {
    "dev.zed.zed", "zed", "code", "code-oss", "codium", "vscodium",
    "jetbrains-pycharm", "jetbrains-idea", "neovide", "emacs",
} | TERMINAL_CLASSES
# Заголовки окон браузера, которые означают чат с ИИ, а не учебный текст.
CHAT_TITLE_MARKERS = ("Claude", "ChatGPT", "Gemini", "Perplexity", "DeepSeek", "Copilot", "Codex", "OpenCode")

# Безопасные типы для возврата буфера: текст и картинка.
RESTORABLE_TYPES = ("text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "image/png")
# Подсказка менеджеров паролей (KeePassXC и др.): такой буфер не возвращаем.
SECRET_HINT = "x-kde-passwordManagerHint"

SENTINEL_PREFIX = "cognitive-popups-sentinel-"
STATE_DIR = Path(os.environ.get("COGNITIVE_STATE_DIR", "~/.local/state/cognitive-popups")).expanduser()
PRIMARY_STAMP = STATE_DIR / "primary.stamp"
CLIP_STAMP = STATE_DIR / "clipboard.stamp"
CAPTURE_LOCK = RLock()


@dataclass
class CaptureResult:
    text: str = ""
    origin: str = ""            # copy | primary | clipboard | ""
    window_class: str = ""
    window_title: str = ""
    context: str = "unknown"    # study | dev | unknown
    cursor: tuple[int, int] | None = None
    elapsed_ms: float = 0.0
    notes: list[str] = field(default_factory=list)


# --- Внешние команды -----------------------------------------------------

def _run(cmd: list[str], data: bytes | None = None, timeout: float = 1.0) -> tuple[int | None, bytes]:
    try:
        r = subprocess.run(cmd, input=data, capture_output=True, timeout=timeout, check=False)
        return r.returncode, r.stdout or b""
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
        return None, b""


def active_window() -> dict:
    code, out = _run(["hyprctl", "activewindow", "-j"])
    if code != 0:
        return {}
    try:
        return json.loads(out.decode("utf-8", "replace") or "{}")
    except json.JSONDecodeError:
        return {}


def parse_cursorpos(raw: str) -> tuple[int, int] | None:
    """Принимает и `{"x":..,"y":..}`, и строку `x, y`."""
    raw = raw.strip()
    if not raw:
        return None
    try:
        obj = json.loads(raw)
        return int(obj["x"]), int(obj["y"])
    except (json.JSONDecodeError, KeyError, TypeError, ValueError):
        pass
    parts = [p.strip() for p in raw.split(",")]
    if len(parts) == 2:
        try:
            return int(float(parts[0])), int(float(parts[1]))
        except ValueError:
            return None
    return None


def cursor_pos() -> tuple[int, int] | None:
    code, out = _run(["hyprctl", "cursorpos", "-j"])
    if code != 0:
        code, out = _run(["hyprctl", "cursorpos"])
    return parse_cursorpos(out.decode("utf-8", "replace")) if code == 0 else None


# --- Классификация окна --------------------------------------------------

def classify_context(window_class: str, title: str) -> str:
    cls = (window_class or "").lower()
    if cls in DEV_CLASSES or cls.startswith("jetbrains-"):
        return "dev"
    if any(m.lower() in (title or "").lower() for m in CHAT_TITLE_MARKERS):
        return "dev"
    if cls:
        return "study"
    return "unknown"


def copy_shortcut_for(window_class: str) -> str:
    cls = (window_class or "").lower()
    if cls in TERMINAL_CLASSES:
        return "CTRL SHIFT, C"
    return "CTRL, C"


def copy_commands(window_class, address):
    shortcut = copy_shortcut_for(window_class)
    mods = "CTRL + SHIFT" if window_class.lower() in TERMINAL_CLASSES else "CTRL"
    # XKB physical C (evdev 46 + 8), independent of the active RU/EN layout.
    dispatch = 'hl.dsp.send_shortcut({ mods = ' + json.dumps(mods) + ', key = "code:54", window = ' + json.dumps("address:" + address) + ' })'
    return (["hyprctl", "dispatch", dispatch],
            ["hyprctl", "dispatch", "sendshortcut", f"{shortcut}, address:{address}"])


# --- Буфер обмена --------------------------------------------------------

@dataclass
class SavedClipboard:
    mime: str | None = None
    data: bytes = b""
    secret: bool = False


def save_clipboard() -> SavedClipboard:
    code, out = _run(["wl-paste", "--list-types"])
    if code != 0:
        return SavedClipboard()
    types = [t.strip() for t in out.decode("utf-8", "replace").splitlines() if t.strip()]
    if SECRET_HINT in types:
        return SavedClipboard(secret=True)
    for mime in RESTORABLE_TYPES:
        if mime in types:
            c, data = _run(["wl-paste", "--no-newline", "--type", mime], timeout=2.0)
            if c == 0:
                return SavedClipboard(mime=mime, data=data)
    return SavedClipboard()


def restore_clipboard(saved: SavedClipboard) -> None:
    """Возвращает прежний буфер. Пароль не возвращаем: пусть исчезнет, как и должен."""
    if saved.secret or saved.mime is None:
        _run(["wl-copy", "--clear"])
        return
    # wl-copy уходит в фон и сам обслуживает буфер, поэтому не ждём его.
    try:
        subprocess.Popen(["wl-copy", "--type", saved.mime], stdin=subprocess.PIPE,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).communicate(saved.data, timeout=1.0)
        # wl-copy's parent can exit before its background owner is ready.
        deadline = time.monotonic() + .3
        while time.monotonic() < deadline:
            code, data = _run(["wl-paste", "--no-newline", "--type", saved.mime])
            if code == 0 and data == saved.data:
                break
            time.sleep(.01)
    except (OSError, subprocess.TimeoutExpired):
        pass


def read_clipboard_text() -> str:
    code, out = _run(["wl-paste", "--no-newline"])
    if code != 0:
        return ""
    try:
        return out.decode("utf-8")
    except UnicodeDecodeError:
        return ""


def primary_age_seconds() -> float | None:
    """Возраст последнего изменения primary. Нужен наблюдатель (см. ниже)."""
    try:
        return time.time() - PRIMARY_STAMP.stat().st_mtime
    except OSError:
        return None


def stamp_mtime(path: Path) -> float | None:
    try:
        return path.stat().st_mtime
    except OSError:
        return None


def plan_for(window_class: str) -> list[str]:
    return ["primary", "copy"] if (window_class or "").lower() in TERMINAL_CLASSES else ["copy", "primary"]


def copy_changed(stamp_before, stamp_after, text_before, text_after, sentinel=False):
    if stamp_before is not None or stamp_after is not None:
        return stamp_after is not None and (stamp_before is None or stamp_after > stamp_before)
    return bool(text_after) and text_after != text_before


def _try_primary(last_capture_time):
    age = primary_age_seconds()
    fresh_limit = None if last_capture_time is None else max(0.0, time.time() - last_capture_time)
    if age is None or (fresh_limit is not None and age >= fresh_limit):
        return ""
    code, out = _run(["wl-paste", "--primary", "--no-newline"])
    if code != 0:
        return ""
    try:
        return out.decode("utf-8").strip()
    except UnicodeDecodeError:
        return ""


# Наблюдатель свежести primary (запускать один раз, например exec-once в Hyprland):
#   wl-paste --primary --watch sh -c 'touch ~/.local/state/cognitive-popups/primary.stamp'


# --- Главная функция -----------------------------------------------------

def capture_selection(timeout_ms: int = 300, use_sentinel: bool = False,
                      last_capture_time: float | None = None) -> CaptureResult:
    with CAPTURE_LOCK:
        return _capture_selection(timeout_ms, use_sentinel, last_capture_time)


def _capture_selection(timeout_ms, use_sentinel, last_capture_time):
    """Захватить то, что выделено прямо сейчас.

    В терминале сначала свежий primary, затем автокопирование; в остальных
    окнах наоборот. Контекст возвращается отдельно. Старый буфер не считается
    выделением; повторное копирование распознаётся по отметке наблюдателя.
    """
    t_start = time.monotonic()
    res = CaptureResult()
    win = active_window()
    res.window_class = str(win.get("class", ""))
    res.window_title = str(win.get("title", ""))
    res.context = classify_context(res.window_class, res.window_title)
    res.cursor = cursor_pos()
    address = str(win.get("address", ""))
    if not address or not address.startswith("0x") or any(c not in "0123456789abcdefABCDEF" for c in address[2:]):
        return res
    for way in plan_for(res.window_class):
        text = _try_primary(last_capture_time) if way == "primary" else _try_copy(res, address, timeout_ms, use_sentinel)
        if text:
            res.text, res.origin = text, way
            break
    res.elapsed_ms = (time.monotonic() - t_start) * 1000
    return res


def _try_copy(res, address, timeout_ms, use_sentinel):
    saved = save_clipboard()
    # Unknown formats cannot be restored losslessly; keep the existing primary
    # probes as the fallback rather than clearing rich data or a password hint.
    if saved.mime is None or saved.secret:
        res.notes.append("secret clipboard" if saved.secret else "clipboard not restorable")
        return ""
    sentinel = SENTINEL_PREFIX + uuid.uuid4().hex[:8]
    before = ""
    if use_sentinel:
        # Метка в буфере позволяет отличить «ничего не выделено» от «выделено то же самое».
        # Цена: метка может попасть в историю буфера (например, Ryoku). См. 03_UNIVERSAL_CAPTURE.md.
        _run(["wl-copy", sentinel])
        before = sentinel
    else:
        before = read_clipboard_text()

    stamp_before = stamp_mtime(CLIP_STAMP)

    text = ""
    try:
        code, output = 1, b""
        for command in copy_commands(res.window_class, address):
            code, output = _run(command)
            if code == 0:
                break
            res.notes.append("copy command: " + output.decode("utf-8", "replace")[:180])
        if code != 0:
            res.notes.append("sendshortcut failed: " + output.decode("utf-8", "replace")[:180])
        else:
            deadline = time.monotonic() + timeout_ms / 1000
            while time.monotonic() < deadline:
                cur = read_clipboard_text()
                if copy_changed(stamp_before, stamp_mtime(CLIP_STAMP), before, cur, use_sentinel):
                    text = cur
                    break
                time.sleep(0.015)
    finally:
        # Don't overwrite a later copy made by the reader while we waited.
        if read_clipboard_text() == (text or before):
            restore_clipboard(saved)

    return text.strip()


if __name__ == "__main__":  # ручная проверка: выделить текст и запустить
    r = capture_selection()
    print(json.dumps({k: v for k, v in r.__dict__.items() if k != "text"}, ensure_ascii=False, indent=2))
    print("TEXT:", r.text[:200])

