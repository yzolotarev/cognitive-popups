from __future__ import annotations

from typing import Any

from .client import GeminiWeb2API, Web2APIError, parse_json_object
from .models import CognitiveSession, FeynmanCheck, PredictionCheck
from .prompt_settings import PromptSettings
from .prompts import (
    feynman_check_prompt,
    feynman_question_prompt,
    prediction_check_prompt,
    seed_prompt,
)


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
        problems: list[str] = []
        for attempt in range(2):
            extra = ""
            if attempt:
                extra = (
                    "\nВАЖНО: предыдущий ответ нарушил формат. Повтори ответ полностью. "
                    "У каждого объекта должны быть только поля simple, term и meaning; simple — ровно одно слово. meaning — не более четырёх слов и должен передавать определяющий смысл: что понятие считает, измеряет, задаёт или характеризует, а не повторять его название. simple должен быть коротким UI-ориентиром категории или роли, а не фамилией, именем или отличительным фрагментом term; для измеряемой или оцениваемой величины допустим ярлык «метрика»; четыре simple должны различаться."
                )
            # Transport failures (timeout, bridge unreachable) propagate as-is:
            # retrying them here would only double the time the reader waits.
            raw = self.client.complete(
                seed_prompt(text, system_prompt + extra),
                max_tokens=500,
            )
            try:
                data = parse_json_object(raw)
            except Web2APIError as exc:
                problems.append(f"попытка {attempt + 1}: {exc}")
                continue
            cues = data.get("cues")
            if not isinstance(cues, list) or len(cues) != 4:
                found = len(cues) if isinstance(cues, list) else 0
                problems.append(f"попытка {attempt + 1}: ожидалось 4 объекта, получено {found}")
                continue
            normalized = []
            invalid = ""
            for cue in cues:
                if not isinstance(cue, dict):
                    invalid = "элемент списка не объект"
                    break
                simple = str(cue.get("simple", "")).strip()
                term = str(cue.get("term", "")).strip()
                meaning = str(cue.get("meaning", "")).strip()
                if not simple or not term or not meaning:
                    invalid = f"пустое поле у объекта {term or simple or '?'!r}"
                    break
                if len(simple.split()) != 1:
                    invalid = f"simple не одно слово: {simple!r}"
                    break
                if len(term.split()) > 3:
                    invalid = f"term длиннее трёх слов: {term!r}"
                    break
                if len(meaning.split()) > 4:
                    invalid = f"meaning длиннее четырёх слов: {meaning!r}"
                    break
                if simple.casefold() == term.casefold():
                    invalid = f"simple совпадает с term: {simple!r}"
                    break
                normalized.append({
                    "simple": simple,
                    "term": term,
                    "meaning": meaning,
                })
            if invalid:
                problems.append(f"попытка {attempt + 1}: {invalid}")
                continue
            if len({item["simple"].casefold() for item in normalized}) != 4:
                duplicates = ", ".join(sorted({item["simple"] for item in normalized}))
                problems.append(f"попытка {attempt + 1}: simple повторяются ({duplicates})")
                continue
            return normalized
        raise Web2APIError("не удалось получить четыре объекта: " + "; ".join(problems))

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

    def check_prediction(self, hypothesis: str) -> PredictionCheck:
        if not self.session.fragments:
            raise ValueError("the common buffer is empty")
        statement = hypothesis.strip()
        if not statement:
            raise ValueError("hypothesis must not be empty")
        data = parse_json_object(self.client.complete(
            prediction_check_prompt(
                self.session.buffer_context(),
                statement,
                self.prompt_settings.get("prediction"),
            ),
            max_tokens=400,
        ))
        status = data.get("status")
        allowed = {
            "confirmed",
            "partially_confirmed",
            "not_supported",
            "contradicted",
            "unclear",
        }
        if status not in allowed:
            raise Web2APIError("invalid prediction status")
        mismatch = data.get("mismatch", "")
        evidence = data.get("evidence", "")
        check = PredictionCheck(
            buffer_fragment_ids=[fragment.id for fragment in self.session.fragments],
            hypothesis=statement,
            status=status,
            mismatch=mismatch.strip() if isinstance(mismatch, str) else "",
            evidence=evidence.strip() if isinstance(evidence, str) else "",
        )
        self.session.add_prediction_check(check)
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
