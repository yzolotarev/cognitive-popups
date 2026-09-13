from __future__ import annotations

from typing import Any

from .client import GeminiWeb2API, Web2APIError, parse_json_object
from .models import CognitiveSession, FeynmanCheck
from .prompt_settings import PromptSettings
from .prompts import feynman_check_prompt, feynman_question_prompt, seed_prompt


class CognitiveService:
    """Application flow independent of GUI, hotkeys, and persistence."""

    def __init__(
        self,
        client: GeminiWeb2API,
        session: CognitiveSession | None = None,
        prompt_settings: PromptSettings | None = None,
    ):
        self.client = client
        self.session = session or CognitiveSession()
        self.prompt_settings = prompt_settings or PromptSettings()

    def extract_cues(self, source_text: str) -> list[dict[str, str]]:
        text = source_text.strip()
        if not text:
            raise ValueError("source text must not be empty")
        system_prompt = self.prompt_settings.get("four_words")
        for attempt in range(2):
            extra = ""
            if attempt:
                extra = (
                    "\nВАЖНО: предыдущий ответ нарушил формат. Повтори ответ полностью. "
                    "У каждого объекта должны быть только поля simple, term и meaning; simple — ровно одно слово. meaning — не более четырёх слов и должен передавать определяющий смысл: что понятие считает, измеряет, задаёт или характеризует, а не повторять его название. simple должен быть коротким UI-ориентиром категории или роли, а не фамилией, именем или отличительным фрагментом term; для измеряемой или оцениваемой величины допустим ярлык «метрика»; четыре simple должны различаться."
                )
            data = parse_json_object(self.client.complete(
                seed_prompt(text, system_prompt + extra),
                max_tokens=500,
            ))
            cues = data.get("cues")
            if not isinstance(cues, list) or len(cues) != 4:
                continue
            normalized = []
            valid = True
            for cue in cues:
                if not isinstance(cue, dict):
                    valid = False
                    break
                simple = str(cue.get("simple", "")).strip()
                term = str(cue.get("term", "")).strip()
                meaning = str(cue.get("meaning", "")).strip()
                if (
                    not simple or not term or not meaning
                    or len(simple.split()) != 1
                    or len(term.split()) > 3
                    or len(meaning.split()) > 4
                    or simple.casefold() == term.casefold()
                ):
                    valid = False
                    break
                normalized.append({
                    "simple": simple,
                    "term": term,
                    "meaning": meaning,
                })
            if valid and len({item["simple"].casefold() for item in normalized}) == 4:
                return normalized
        raise Web2APIError("seed response must contain four complete cue objects")

    def add_fragment_with_cues(self, source_text: str, cues: list[dict[str, str]]):
        text = source_text.strip()
        if not text:
            raise ValueError("source text must not be empty")
        details = [dict(cue) for cue in cues]
        return self.session.add_fragment(
            text,
            [cue["term"] for cue in details],
            cue_details=details,
        )

    def add_fragment(self, source_text: str):
        return self.add_fragment_with_cues(source_text, self.extract_cues(source_text))

    def create_feynman_question(self) -> str:
        if not self.session.fragments:
            raise ValueError("the common buffer is empty")
        data = parse_json_object(self.client.complete(
            feynman_question_prompt(
                self.session.buffer_context(),
                self.prompt_settings.get("feynman_question"),
            ),
            max_tokens=250,
        ))
        question = data.get("question")
        if not isinstance(question, str) or not question.strip():
            raise Web2APIError("Feynman question is empty")
        return question.strip()

    def check_feynman(self, question: str, explanation: str) -> FeynmanCheck:
        if not self.session.fragments:
            raise ValueError("the common buffer is empty")
        if not explanation.strip():
            raise ValueError("explanation must not be empty")
        data = parse_json_object(self.client.complete(
            feynman_check_prompt(
                self.session.buffer_context(),
                question,
                explanation,
                self.prompt_settings.get("feynman_check"),
            ),
            max_tokens=700,
        ))
        status = data.get("status")
        if status not in {"passed", "needs_retry"}:
            raise Web2APIError("invalid Feynman status")
        raw_gaps = data.get("gaps", [])
        gaps: list[dict[str, str]] = []
        if isinstance(raw_gaps, list):
            for gap in raw_gaps[:2]:
                if isinstance(gap, dict):
                    gaps.append({
                        "location": str(gap.get("location", "")),
                        "type": str(gap.get("type", "")),
                        "description": str(gap.get("description", "")),
                    })
        check = FeynmanCheck(
            buffer_fragment_ids=[fragment.id for fragment in self.session.fragments],
            question=question,
            explanation=explanation.strip(),
            status=status,
            gaps=gaps,
            follow_up=data.get("follow_up") if isinstance(data.get("follow_up"), str) else None,
        )
        self.session.add_feynman_check(check)
        return check

    def clear_buffer(self) -> CognitiveSession:
        """Return the old session for history, then start a fresh active buffer."""
        archived = self.session
        self.session = self.session.clear()
        return archived

    def summary(self) -> dict[str, Any]:
        return {
            "session_id": self.session.id,
            "fragments": len(self.session.fragments),
            "cues": len(self.session.fragments) * 4,
            "feynman_checks": len(self.session.feynman_checks),
        }
