from __future__ import annotations

import json
from pathlib import Path
from typing import Iterable

from .models import CognitiveSession


class SessionHistory:
    """Small local JSON store for explicitly archived buffers."""

    def __init__(self, path: str | Path):
        self.path = Path(path)

    def load(self) -> list[CognitiveSession]:
        if not self.path.exists():
            return []
        data = json.loads(self.path.read_text(encoding="utf-8"))
        if not isinstance(data, list):
            raise ValueError("history file must contain a list")
        return [CognitiveSession.from_dict(item) for item in data]

    def append(self, session: CognitiveSession) -> None:
        sessions = self.load()
        sessions.append(session)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(
            json.dumps([item.to_dict() for item in sessions], ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )

    def replace(self, sessions: Iterable[CognitiveSession]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(
            json.dumps([item.to_dict() for item in sessions], ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
