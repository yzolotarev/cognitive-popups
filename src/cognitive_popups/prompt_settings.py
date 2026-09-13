from __future__ import annotations

import json
import argparse
import os
import subprocess
import tempfile
from pathlib import Path
from typing import Mapping

from . import prompts


DEFAULT_PATH = Path("~/.config/cognitive-popups/prompts.json").expanduser()

DEFAULT_PROMPTS: Mapping[str, str] = {
    "four_words": prompts.SEED_SYSTEM,
    "feynman_question": prompts.QUESTION_SYSTEM,
    "feynman_check": prompts.FEYNMAN_SYSTEM,
}


class PromptSettings:
    """Persistent full replacements for the built-in prompts."""

    def __init__(self, path: Path = DEFAULT_PATH):
        self.path = Path(path).expanduser()
        self._overrides: dict[str, str] = {}
        self.reload()

    def reload(self) -> None:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except (FileNotFoundError, OSError, json.JSONDecodeError):
            data = {}
        self._overrides = {
            key: value.strip()
            for key, value in data.items()
            if key in DEFAULT_PROMPTS and isinstance(value, str) and value.strip()
        } if isinstance(data, dict) else {}

    def get(self, key: str) -> str:
        return self._overrides.get(key, DEFAULT_PROMPTS[key])

    def set(self, key: str, value: str) -> None:
        if key not in DEFAULT_PROMPTS:
            raise KeyError(key)
        value = value.strip()
        if not value:
            raise ValueError("prompt must not be empty")
        self._overrides[key] = value
        self._save()

    def reset(self, key: str) -> None:
        if key not in DEFAULT_PROMPTS:
            raise KeyError(key)
        self._overrides.pop(key, None)
        self._save()

    def reset_all(self) -> None:
        self._overrides.clear()
        self._save()

    def is_custom(self, key: str) -> bool:
        return key in self._overrides

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
    parser.add_argument("key", nargs="?", choices=sorted(DEFAULT_PROMPTS), help="prompt to edit")
    parser.add_argument("--list", action="store_true", help="show prompt keys and custom status")
    parser.add_argument("--reset", choices=sorted(DEFAULT_PROMPTS), help="reset one prompt")
    parser.add_argument("--reset-all", action="store_true", help="reset all prompts")
    args = parser.parse_args()
    settings = PromptSettings()
    if args.list:
        for key in DEFAULT_PROMPTS:
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
