from __future__ import annotations

import argparse
import json
import os
import subprocess
import tempfile
import threading
from pathlib import Path
from typing import Mapping

from . import prompts


#: Prompts travel with the rest of the state, so `COGNITIVE_STATE_DIR` moves the
#: whole app at once; `COGNITIVE_PROMPTS_PATH` overrides this file alone.
STATE_ROOT = os.environ.get("COGNITIVE_STATE_DIR") or "~/.local/state/cognitive-popups"
DEFAULT_PATH = Path(
    os.environ.get("COGNITIVE_PROMPTS_PATH") or os.path.join(STATE_ROOT, "prompts.json")
).expanduser()

#: Which constant in `prompts` holds each editable prompt. The defaults are read
#: through these names rather than snapshotted at import, so editing `prompts.py`
#: while the daemon runs takes effect the same way editing `prompts.json` does.
PROMPT_ATTRS: Mapping[str, str] = {
    "four_words": "SEED_SYSTEM",
    "feynman_question": "QUESTION_SYSTEM",
    "feynman_check": "FEYNMAN_SYSTEM",
    "prediction": "PREDICTION_SYSTEM",
    "clarify": "CLARIFY_SYSTEM",
    "clarify_question": "CLARIFY_QUESTION_SYSTEM",
    "summary": "SUMMARY_SYSTEM",
    "example": "EXAMPLE_SYSTEM",
    "reframe": "REFRAME_SYSTEM",
    "goal": "GOAL_SYSTEM",
    # Background preparation of one practice task (see practice.py). Editable the
    # same way as the rest, so the analysis rules can be tuned without a release.
    "practice_analysis": "PRACTICE_ANALYSIS_SYSTEM",
}
PROMPT_KEYS = tuple(PROMPT_ATTRS)

_SOURCE = Path(getattr(prompts, "__file__", "") or "")
_CACHE: dict[str, object] = {"stamp": None, "values": {}}
_LOCK = threading.Lock()


def _module_defaults() -> dict[str, str]:
    """Defaults from the module this process already imported.

    The safety net: a prompt key must never be left without text, even if the
    source file cannot be read at construction time.
    """
    defaults: dict[str, str] = {}
    for key, attr in PROMPT_ATTRS.items():
        value = getattr(prompts, attr, None)
        if isinstance(value, str) and value.strip():
            defaults[key] = value
    return defaults


def built_in_defaults(source: Path | None = None) -> dict[str, str]:
    """Defaults read from the prompt source file, not from an import snapshot.

    A running daemon imports `prompts` once, so an edit to that file would stay
    invisible until a restart. Re-reading the source removes the trap. The file is
    compiled but never imported, so a half-written file cannot replace the module
    the process is using or break a prompt lookup: a compile error simply yields
    nothing, and the caller keeps the text it already had.
    """
    path = Path(source) if source is not None else _SOURCE
    try:
        stamp = path.stat().st_mtime_ns
    except OSError:
        return {}
    with _LOCK:
        if source is None and _CACHE["stamp"] == stamp:
            return dict(_CACHE["values"])  # type: ignore[arg-type]
        try:
            namespace: dict[str, object] = {"__name__": "cognitive_popups.prompts.snapshot"}
            # Our own prompt source, not third-party code: compiling it is exactly
            # what importing it would do, minus the import.
            exec(compile(path.read_text(encoding="utf-8"), str(path), "exec"), namespace)  # noqa: S102
        except (OSError, SyntaxError, ValueError):
            return {}
        values: dict[str, str] = {}
        for key, attr in PROMPT_ATTRS.items():
            value = namespace.get(attr)
            if isinstance(value, str) and value.strip():
                values[key] = value
        if source is None:
            _CACHE["stamp"] = stamp
            _CACHE["values"] = values
        return dict(values)


class PromptSettings:
    """Persistent full replacements for the built-in prompts."""

    def __init__(self, path: Path = DEFAULT_PATH):
        self.path = Path(path).expanduser()
        self._overrides: dict[str, str] = {}
        # Start from the imported module, then refresh from the file on reload.
        self._defaults: dict[str, str] = _module_defaults()
        self.reload()

    def reload(self) -> None:
        """Re-read both halves: defaults from `prompts.py`, overrides from JSON."""
        self._defaults.update(built_in_defaults())
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (FileNotFoundError, OSError, json.JSONDecodeError):
            data = {}
        self._overrides = {
            key: value.strip()
            for key, value in data.items()
            if key in PROMPT_ATTRS and isinstance(value, str) and value.strip()
        } if isinstance(data, dict) else {}

    def default(self, key: str) -> str:
        """The built-in text for `key` as `prompts.py` has it right now."""
        return self._defaults.get(key, "")

    def get(self, key: str) -> str:
        value = self._overrides.get(key)
        # An override that only repeats the built-in text is not a customisation:
        # let the file's version win, so a redundant saved prompt cannot freeze a
        # key against later edits to `prompts.py`.
        if value and value.strip() == self.default(key).strip():
            return self.default(key)
        # Migrate only the known legacy rule in memory; retain custom instructions
        # and leave the user's saved prompt untouched.
        if key == "four_words" and value:
            return value.replace(
                "- term — точное исходное понятие не более трёх слов;",
                "- term — точное исходное понятие без ограничения числа слов; не обрезай название и не теряй смысловые различия;",
            )
        # Older local overrides encoded the 11-word clarification contract.
        # Do not let that stale contract reintroduce the defect fixed here.
        if key == "clarify" and value and "одиннадцати слов" in value:
            return self.default(key)
        # Older local overrides also kept the pre-2026-09-19 rules, which let a
        # general dictionary sense replace the definition the text itself gives.
        # Both markers must be present, so a genuinely custom prompt is left alone.
        if key == "clarify" and value:
            folded = value.casefold()
            if "начни с доступного смысла, затем добавь" in folded and "не ограничивайся им" in folded:
                return self.default(key)
        # A saved copy of the pre-2026-09-20 clarify default would shadow the rule
        # that a designation is explained by its role in the concrete place. Both
        # markers must be present, so a genuinely custom prompt is left alone.
        if key == "clarify" and value:
            folded = value.casefold()
            if (
                "роль в конкретном месте текста" not in folded
                and "носит название" in folded
                and "не подменяй объяснение общим значением слова" in folded
            ):
                return self.default(key)
        return value or self.default(key)

    def set(self, key: str, value: str) -> None:
        if key not in PROMPT_ATTRS:
            raise KeyError(key)
        value = value.strip()
        if not value:
            raise ValueError("prompt must not be empty")
        self._overrides[key] = value
        self._save()

    def reset(self, key: str) -> None:
        if key not in PROMPT_ATTRS:
            raise KeyError(key)
        self._overrides.pop(key, None)
        self._save()

    def reset_all(self) -> None:
        self._overrides.clear()
        self._save()

    def is_custom(self, key: str) -> bool:
        """Whether the saved text actually differs from the built-in one."""
        value = self._overrides.get(key)
        return bool(value) and value.strip() != self.default(key).strip()

    def _save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(
            json.dumps(self._overrides, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )


def edit_prompt(settings: PromptSettings, key: str) -> None:
    editor = os.environ.get("EDITOR") or os.environ.get("VISUAL") or "vi"
    with tempfile.NamedTemporaryFile("w+", encoding="utf-8", suffix=".prompt", delete=False) as handle:
        path = Path(handle.name)
        handle.write(settings.get(key))
        handle.write("\n")
    try:
        subprocess.run([editor, str(path)], check=True)
        value = path.read_text(encoding="utf-8").strip()
        settings.set(key, value)
    finally:
        path.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="Edit cognitive-popups prompts")
    parser.add_argument("key", nargs="?", choices=sorted(PROMPT_KEYS), help="prompt to edit")
    parser.add_argument("--list", action="store_true", help="show prompt keys and custom status")
    parser.add_argument("--reset", choices=sorted(PROMPT_KEYS), help="reset one prompt")
    parser.add_argument("--reset-all", action="store_true", help="reset all prompts")
    args = parser.parse_args()
    settings = PromptSettings()
    if args.list:
        for key in PROMPT_KEYS:
            print(f"{key}: {'custom' if settings.is_custom(key) else 'default'}")
        return 0
    if args.reset_all:
        settings.reset_all()
        return 0
    if args.reset:
        settings.reset(args.reset)
        return 0
    if args.key:
        edit_prompt(settings, args.key)
        return 0
    parser.error("specify a prompt key, --list, --reset, or --reset-all")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
