from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Iterable

from .models import CognitiveSession

#: Archived buffers kept. Each one is a whole reading buffer, and the file is
#: read in full on start; the ledger (records.sqlite3) keeps everything anyway.
KEEP = 50


class SessionHistory:
    """Small local JSON store for explicitly archived buffers.

    Writes are atomic (temporary file + `os.replace`), so a crash mid-write
    leaves the previous file. A file that still cannot be read is moved to
    `.broken` and the history starts empty instead of stopping the daemon.
    """

    def __init__(self, path: str | Path, *, keep: int = KEEP):
        self.path = Path(path)
        self.keep = keep

    def load(self) -> list[CognitiveSession]:
        if not self.path.exists():
            return []
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
            if not isinstance(data, list):
                raise ValueError("history file must contain a list")
            return [CognitiveSession.from_dict(item) for item in data]
        except (OSError, ValueError, TypeError, KeyError, AttributeError) as exc:
            broken = self.path.with_name(self.path.name + ".broken")
            try:
                os.replace(self.path, broken)
            except OSError:
                pass
            print(f"cognitive history: unreadable {self.path} ({type(exc).__name__}) moved to {broken}",
                  file=sys.stderr)
            return []

    def append(self, session: CognitiveSession) -> None:
        self.replace([*self.load(), session])

    def replace(self, sessions: Iterable[CognitiveSession]) -> None:
        kept = list(sessions)[-self.keep:] if self.keep else list(sessions)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temporary = self.path.with_name(self.path.name + ".tmp")
        with temporary.open("w", encoding="utf-8") as stream:
            stream.write(json.dumps([item.to_dict() for item in kept], ensure_ascii=False, indent=2) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, self.path)
