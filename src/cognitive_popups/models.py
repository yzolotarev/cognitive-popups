from __future__ import annotations

from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
import hashlib
import uuid
from typing import Any


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def text_hash(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


@dataclass
class Fragment:
    source_text: str
    cues: list[str]
    cue_details: list[dict[str, str]] = field(default_factory=list)
    created_at: str = field(default_factory=now_iso)
    id: str = field(default_factory=lambda: uuid.uuid4().hex)
    source_hash: str = ""
    actions: list[dict[str, Any]] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.source_text = self.source_text.strip()
        self.source_hash = self.source_hash or text_hash(self.source_text)
        if len(self.cues) != 4:
            raise ValueError("a fragment must contain exactly four cues")
        self.cues = [cue.strip() for cue in self.cues]
        if any(not cue for cue in self.cues):
            raise ValueError("cues must not be empty")
        if not self.cue_details:
            self.cue_details = [
                {"simple": cue, "term": cue, "meaning": cue}
                for cue in self.cues
            ]
        if len(self.cue_details) != 4:
            raise ValueError("a fragment must contain exactly four cue details")
        normalized_details = []
        for item in self.cue_details:
            def clean(value: object) -> str:
                return "" if value is None else str(value).strip()

            if "simple" in item:
                normalized_details.append({
                    "simple": clean(item.get("simple", "")),
                    "term": clean(item.get("term", "")),
                    "meaning": clean(item.get("meaning", "")),
                })
            elif "term" in item:
                normalized_details.append({
                    "simple": clean(item.get("label", "")),
                    "term": clean(item.get("term", "")),
                    "meaning": clean(item.get("essence", "")),
                })
            else:
                normalized_details.append({
                    "simple": clean(item.get("gloss", item.get("label", ""))),
                    "term": clean(item.get("label", "")),
                    "meaning": clean(item.get("gloss", "")),
                })
        self.cue_details = normalized_details

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass
class FeynmanCheck:
    buffer_fragment_ids: list[str]
    question: str
    explanation: str
    status: str
    gaps: list[dict[str, str]] = field(default_factory=list)
    follow_up: str | None = None
    created_at: str = field(default_factory=now_iso)
    id: str = field(default_factory=lambda: uuid.uuid4().hex)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass
class PredictionCheck:
    buffer_fragment_ids: list[str]
    hypothesis: str
    status: str
    mismatch: str = ""
    evidence: str = ""
    created_at: str = field(default_factory=now_iso)
    id: str = field(default_factory=lambda: uuid.uuid4().hex)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass
class CognitiveSession:
    id: str = field(default_factory=lambda: uuid.uuid4().hex)
    title: str = "Untitled session"
    created_at: str = field(default_factory=now_iso)
    fragments: list[Fragment] = field(default_factory=list)
    feynman_checks: list[FeynmanCheck] = field(default_factory=list)
    prediction_checks: list[PredictionCheck] = field(default_factory=list)

    def add_fragment(
        self,
        source_text: str,
        cues: list[str],
        cue_details: list[dict[str, str]] | None = None,
    ) -> Fragment:
        fragment = Fragment(source_text=source_text, cues=cues, cue_details=cue_details or [])
        self.fragments.append(fragment)
        return fragment

    def add_feynman_check(self, check: FeynmanCheck) -> None:
        unknown = set(check.buffer_fragment_ids) - {f.id for f in self.fragments}
        if unknown:
            raise ValueError("Feynman check references unknown fragments")
        self.feynman_checks.append(check)

    def add_prediction_check(self, check: PredictionCheck) -> None:
        unknown = set(check.buffer_fragment_ids) - {f.id for f in self.fragments}
        if unknown:
            raise ValueError("Prediction check references unknown fragments")
        self.prediction_checks.append(check)

    def clear(self) -> "CognitiveSession":
        """Return a fresh active session; the old one can be stored in history."""
        return CognitiveSession()

    def buffer_context(self) -> str:
        parts: list[str] = []
        for index, fragment in enumerate(self.fragments, 1):
            parts.append(
                f"=== FRAGMENT {index} ({fragment.id}) ===\n"
                f"SOURCE TEXT:\n{fragment.source_text}\n\n"
                f"CUES:\n{', '.join(fragment.cues)}"
            )
        return "\n\n".join(parts)

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "title": self.title,
            "created_at": self.created_at,
            "fragments": [fragment.to_dict() for fragment in self.fragments],
            "feynman_checks": [check.to_dict() for check in self.feynman_checks],
            "prediction_checks": [check.to_dict() for check in self.prediction_checks],
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "CognitiveSession":
        session = cls(
            id=str(data.get("id") or uuid.uuid4().hex),
            title=str(data.get("title") or "Untitled session"),
            created_at=str(data.get("created_at") or now_iso()),
        )
        session.fragments = [Fragment(**item) for item in data.get("fragments", [])]
        session.feynman_checks = [FeynmanCheck(**item) for item in data.get("feynman_checks", [])]
        session.prediction_checks = [
            PredictionCheck(**item) for item in data.get("prediction_checks", [])
        ]
        return session
