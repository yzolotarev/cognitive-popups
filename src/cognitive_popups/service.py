from __future__ import annotations

import hashlib
import time
from dataclasses import asdict, dataclass, replace
from contextvars import ContextVar
from functools import wraps
from inspect import signature
from typing import Any

from .client import GeminiWeb2API, Web2APIError, parse_json_object
from .models import CognitiveSession, FeynmanCheck, PredictionCheck, now_iso
from .notes import normalise_anchor
from . import observation
from .operation_context import OperationContext, current_operation, operation_scope


# Captured along with the originating UI callback, never shared across workers.
output_artifact: ContextVar[str | None] = ContextVar("output_artifact", default=None)


class CueList(list):
    """List-compatible cues carrying the identity of their generating request."""

    def __init__(self, items, *, prompt_hash="", model="", cache_key=None):
        super().__init__(items)
        self.prompt_hash = prompt_hash
        self.model = model
        self.cache_key = cache_key
        self.artifact_id = None
        self.operation_id = None
        self.request_ids = []


class GeneratedQuestion(str):
    def __new__(cls, text, prompt_hash=""):
        value = super().__new__(cls, text)
        value.prompt_hash = prompt_hash
        return value


def observed_operation(kind):
    """Observe submitted arguments before validation/network IO, including failures."""
    def decorate(method):
        @wraps(method)
        def wrapped(self, *args, **kwargs):
            ctx = current_operation() or OperationContext()
            ctx = replace(ctx, buffer_session_id=self.session.id)
            with operation_scope(ctx):
                arguments = dict(signature(method).bind(self, *args, **kwargs).arguments)
                arguments.pop("self")
                source = observation.record_artifact(kind + "_input", {
                    "arguments": arguments, "buffer": self.session.to_dict(),
                    "buffer_context": self.session.buffer_context(),
                })
                store = getattr(self.client, "observation_store", None)
                requests = store.request_ids_for_operation if store is not None else observation.request_ids_for_operation
                before = set(requests(ctx.operation_id))
                output_artifact.set(None)
                try:
                    result = method(self, *args, **kwargs)
                except BaseException as exc:
                    observation.record_annotation("operation_failed", {
                        "kind": kind, "input_artifact_id": source,
                        "error_type": type(exc).__name__,
                        "request_ids": [r for r in requests(ctx.operation_id) if r not in before],
                    })
                    raise
                request_ids = [r for r in requests(ctx.operation_id) if r not in before]
                payload = result.to_dict() if hasattr(result, "to_dict") else (
                    asdict(result) if hasattr(result, "__dataclass_fields__") else result)
                if isinstance(result, CueList):
                    payload = {"cues": list(result), "prompt_hash": result.prompt_hash,
                               "model": result.model, "temperature": 0.0, "max_tokens": 500}
                artifact = observation.record_artifact(kind + "_generated", payload,
                    request_ids=request_ids, source_artifact_id=source)
                output_artifact.set(artifact)
                if isinstance(result, CueList):
                    result.artifact_id = artifact
                    result.operation_id = ctx.operation_id
                    result.request_ids = request_ids
                if hasattr(result, "id"):
                    observation.record_annotation("ledger_link", {
                        "kind": kind, "check_id": result.id, "artifact_id": artifact,
                        "buffer_session_id": self.session.id,
                        "fragment_ids": result.buffer_fragment_ids,
                    })
                return result
        return wrapped
    return decorate
from .prompt_settings import PromptSettings
from .prompts import (
    clarify_prompt,
    clarify_question_prompt,
    example_prompt,
    feynman_check_prompt,
    feynman_question_prompt,
    goal_prompt,
    prediction_check_prompt,
    reframe_prompt,
    seed_prompt,
    summary_prompt,
)

#: A pause longer than this between two reader actions ends the reading session.
#: 45 minutes is the boundary v1 used for its ring of drops. The unit of activity
#: is the reader's own hotkey, not the selection watcher: the watcher only fills
#: the cue cache, while the press makes a cycle.
IDLE_SESSION_SECONDS = 45 * 60

#: Terms explained in one window. Beyond four the answer stops being a
#: clarification and becomes a lecture, so the rest are named, not answered.
MAX_CLARIFY_TERMS = 4

#: A clarification is allowed to be a short usable explanation. The previous
#: eleven-word cap produced dictionary fragments and made a reader's request for
#: an accessible explanation fail before the key relation could be stated.
CLARIFY_WORDS = 80


@dataclass
class SessionSplit:
    """A reading session that an idle pause ended, and when the work stopped."""

    session: CognitiveSession
    ended_utc: str


@dataclass
class Clarification:
    """Explanations the reader asked for, and the terms that stayed unanswered.

    `omitted` holds the reader's own words for everything the window did not
    cover — beyond the limit, or without a basis in the text. A term named and
    then silently dropped would read as if the reader had never asked.
    """

    lines: list[dict[str, str]]
    omitted: list[str]
    truncated: list[str]


@dataclass
class Answer:
    """The model's answer to the reader's own question about the material.

    `grounding` says how the answer stands to the text: `in_text` when it follows
    from the material, `partly_in_text` when it reads the material together with
    the model's own step, `not_in_text` when the text does not contain it at all.
    `basis` keeps a quotation only when it was found verbatim, so an empty field
    means "no verbatim support found", not "nothing was said". Like a
    clarification, an answer is a reading aid and never evidence of learning.
    """

    text: str
    question: str = ""
    grounding: str = ""
    basis: str = ""
    prompt_hash: str = ""
    model: str = ""


@dataclass
class Summary:
    """A plain-language gist of one passage, and nothing else."""

    text: str


@dataclass
class Reframe:
    """A model proposal, not the reader's view or a learning verdict.

    `status` is `proposal` only with a verbatim basis in the supplied snapshot;
    otherwise it is `insufficient` and `text` explicitly abstains.
    """

    text: str
    status: str
    basis: str = ""
    focus: str = ""
    current_frame: str = ""
    prompt_hash: str = ""
    model: str = ""


@dataclass
class Example:
    """One concrete illustration of a highlighted passage.

    `intent_snapshot` is kept because an intent is only passed when the reader
    explicitly chose it, and the window must be able to show which one was used.
    The text is reading material, never evidence of learning.
    """

    text: str
    intent_snapshot: str = ""
    query: str = ""
    prompt_hash: str = ""
    model: str = ""


@dataclass
class Goal:
    """One suggested wording of what the reader wants from a passage.

    A draft, not a decision: it exists so the reader can react to a concrete
    phrase instead of composing one from nothing. Nothing is stored until they
    accept it through the ordinary bookmark.
    """

    text: str
    direction: str = ""
    note: str = ""
    prompt_hash: str = ""
    model: str = ""


def prompt_hash(text: str) -> str:
    """Short stable id of a prompt text, so two versions stay distinguishable."""
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:12]


def parse_terms(raw: str) -> list[str]:
    """Terms the reader typed, in the order they wrote them.

    Commas and newlines both separate; duplicates are dropped by the same folded
    key note anchors use, because asking twice about one word is not two
    questions. Order is kept: what the reader lists first is what bothers them
    most, and that is the only relevance signal the system has — ranking the
    terms by importance would be the model judging the reader's questions.
    """
    terms: list[str] = []
    seen: set[str] = set()
    for chunk in raw.replace("\n", ",").split(","):
        term = " ".join(chunk.split()).strip()
        key = normalise_anchor(term) or ""
        if term and key and key not in seen:
            seen.add(key)
            terms.append(term)
    return terms


#: Words that mark a request rather than a name. A term list is short names; a
#: question names what the reader wants to know.
_QUESTION_WORDS = frozenset({
    "что", "кто", "как", "какой", "какая", "какое", "какие", "какую", "каким",
    "чем", "чему", "почему", "зачем", "где", "куда", "откуда", "когда", "сколько",
})

#: Multi-word markers that only read as a request, never as a term.
_QUESTION_PHRASES = ("если бы", "можно ли", "стоит ли", "правда ли", "что такое")

#: A glossary request is a short comma-separated list of short names. A piece
#: longer than this carries a predicate and reads as a question.
_TERM_CHUNK_WORDS = 4


def looks_like_question(raw: str) -> bool:
    """Whether the reader typed a question rather than a list of terms.

    Alt+C answers both, and the two cannot be told apart by one marker: a reader
    may write "что такое группа Галуа", "почему критерий не работает", or a long
    hypothetical with no question mark at all. A question mark, an interrogative
    word, or a long comma-separated piece each decide it; a short list of short
    names stays a glossary request. The choice is local and testable rather than
    delegated to the model, and it only picks which prompt runs — it never
    changes what the reader's words mean.
    """
    text = " ".join(raw.split()).strip()
    if not text:
        return False
    if "?" in text:
        return True
    folded = text.casefold()
    words = [word.strip(".,;:!?()\"'«»") for word in folded.split()]
    if any(word in _QUESTION_WORDS for word in words):
        return True
    # Phrases are matched word by word, not as substrings, so "если быстрее" is
    # not read as the opening "если бы".
    for phrase in _QUESTION_PHRASES:
        parts = phrase.split()
        span = len(parts)
        if any(words[index:index + span] == parts for index in range(len(words) - span + 1)):
            return True
    chunks = [chunk for chunk in folded.split(",") if chunk.strip()]
    # Several pieces: a term list unless one of them reads as a phrase. A single
    # piece with more than a few words is a question, not a name.
    return any(len(chunk.split()) > _TERM_CHUNK_WORDS for chunk in chunks)


def _normalise_grounding(text: str) -> str:
    """Fold whitespace and case while keeping formulas and punctuation meaningful."""
    return " ".join(text.casefold().split())


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
        self._last_activity: float | None = None
        self._last_activity_utc: str | None = None


    def prompt_id(self, key: str) -> dict[str, str]:
        """Identity of the prompt a call is about to use.

        Prompt files are edited while the daemon runs, so everything that reaches
        the log carries the hash of the text that produced it; otherwise two prompt
        versions become indistinguishable afterwards.
        """
        text = self._prompt(key)
        return {
            "prompt_key": key,
            "prompt_hash": prompt_hash(text),
            "prompt_source": "custom" if self.prompt_settings.is_custom(key) else "default",
            "model": self._model_name(),
        }

    def _model_name(self) -> str:
        return str(getattr(self.client, "model", "") or "")

    def _prompt(self, key: str) -> str:
        """Read a prompt fresh from disk, so an edit applies without a restart.

        Prompts are the part most often edited while the daemon runs, and a restart
        would throw away the live buffer (it lives in RAM only). Both halves are
        re-read: the built-in defaults from `prompts.py` and the overrides from
        `prompts.json`.
        """
        self.prompt_settings.reload()
        return self.prompt_settings.get(key)

    def cue_cache_key(self, source_text: str) -> tuple[str, str, str, str]:
        return (source_text, prompt_hash(self._prompt("four_words")),
                self._model_name(), str(getattr(self.client, "url", "")))

    def example_cache_key(self, material: str, query: str = "", intent: str = "") -> tuple:
        """Everything that changes the answer, so a cached one can never reappear
        under a new context: material, request, explicitly passed intent, model,
        bridge and prompt version."""
        return (material, query, intent, prompt_hash(self._prompt("example")),
                self._model_name(), str(getattr(self.client, "url", "")))

    @observed_operation("cues")
    def extract_cues(self, source_text: str) -> list[dict[str, str]]:
        text = source_text.strip()
        if not text:
            raise ValueError("source text must not be empty")
        system_prompt = self._prompt("four_words")
        model = self._model_name()
        cache_key = (source_text, prompt_hash(system_prompt), model, str(getattr(self.client, "url", "")))
        problems: list[str] = []
        for attempt in range(2):
            extra = ""
            if attempt:
                extra = (
                    f"\nВАЖНО: предыдущий ответ не прошёл проверку ({problems[-1]}). "
                                        "Повтори ответ полностью: ровно четыре объекта. "
                                        "term сохраняй полностью, без ограничения числа слов. "
                    "У каждого объекта должны быть только поля simple, term и meaning; simple — ровно одно слово. meaning — не более двенадцати слов и должен передавать определяющий смысл: что понятие считает, измеряет, задаёт или характеризует, а не повторять его название. simple должен быть коротким UI-ориентиром категории или роли, а не фамилией, именем или отличительным фрагментом term; для измеряемой или оцениваемой величины допустим ярлык «метрика»; повтор категории допустим, если он точнее."
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

                if len(meaning.split()) > 12:
                    invalid = f"meaning длиннее двенадцати слов: {meaning!r}"
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
            return CueList(normalized, prompt_hash=prompt_hash(system_prompt), model=model,
                           cache_key=cache_key)
        raise Web2APIError("не удалось получить четыре объекта: " + "; ".join(problems))

    def add_fragment_with_cues(self, source_text: str, cues: list[dict[str, str]]):
        text = source_text.strip()
        if not text:
            raise ValueError("source text must not be empty")
        details = [dict(cue) for cue in cues]
        fragment = self.session.add_fragment(
            text,
            [cue["term"] for cue in details],
            cue_details=details,
        )
        # A plain list has unknown provenance; never stamp today's prompt on it.
        fragment.prompt_hash = getattr(cues, "prompt_hash", "")
        fragment.model = getattr(cues, "model", "")
        fragment.artifact_id = getattr(cues, "artifact_id", None)
        fragment.generating_operation_id = getattr(cues, "operation_id", None)
        fragment.request_ids = list(getattr(cues, "request_ids", []))
        output_artifact.set(fragment.artifact_id)
        observation.record_annotation("ledger_link", {
            "kind": "fragment", "fragment_id": fragment.id,
            "buffer_session_id": self.session.id, "artifact_id": fragment.artifact_id,
            "generating_operation_id": fragment.generating_operation_id,
            "request_ids": fragment.request_ids,
        })
        return fragment

    def add_fragment(self, source_text: str):
        return self.add_fragment_with_cues(source_text, self.extract_cues(source_text))

    @observed_operation("feynman_question")
    def create_feynman_question(self) -> str:
        if not self.session.fragments:
            raise ValueError("the common buffer is empty")
        system_prompt = self._prompt("feynman_question")
        question_prompt_hash = prompt_hash(system_prompt)
        problems: list[str] = []
        for attempt in range(2):
            repair = ""
            if attempt:
                repair = (
                    "\nПредыдущий вопрос не прошёл проверку: " + "; ".join(problems)
                    + ". Верни новый JSON; дословно назови два term из CUES одного фрагмента."
                )
            data = parse_json_object(self.client.complete(
                feynman_question_prompt(
                    self.session.buffer_context(),
                    system_prompt + repair,
                ),
                max_tokens=250,
            ))
            question = data.get("question")
            if not isinstance(question, str) or not question.strip():
                problems.append("вопрос пуст")
                continue
            folded = _normalise_grounding(question)
            grounded = any(
                sum(
                    1
                    for cue in fragment.cue_details
                    if _normalise_grounding(cue.get("term", "")) in folded
                ) >= 2
                for fragment in self.session.fragments
            )
            if grounded:
                return GeneratedQuestion(question.strip(), question_prompt_hash)
            problems.append("вопрос не называет два term из одного фрагмента")
        raise Web2APIError("не удалось привязать вопрос Фейнмана к материалу: " + "; ".join(problems))

    @observed_operation("feynman_check")
    def check_feynman(self, question: str, explanation: str) -> FeynmanCheck:
        if not self.session.fragments:
            raise ValueError("the common buffer is empty")
        if not explanation.strip():
            raise ValueError("explanation must not be empty")
        system_prompt = self._prompt("feynman_check")
        data = parse_json_object(self.client.complete(
            feynman_check_prompt(
                self.session.buffer_context(),
                question,
                explanation,
                system_prompt,
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
            question_prompt_hash=getattr(question, "prompt_hash", ""),
            check_prompt_hash=prompt_hash(system_prompt),
            model=self._model_name(),
        )
        self.session.add_feynman_check(check)
        return check

    def _reader_material(self, fallback_text: str) -> str:
        """The ground both Alt+C modes read: the buffer, plus the live selection.

        The buffer holds what the reader chose to work with, so it comes first;
        the passage on screen is added when it differs, because the sentence that
        defines the term or answers the question may be highlighted and not yet
        committed. Raising on an empty ground keeps either mode from answering
        out of nothing.
        """
        material = self.session.buffer_context()
        selection = fallback_text.strip()
        if selection and _normalise_grounding(selection) not in _normalise_grounding(material):
            material = f"{material}\n\n=== CURRENT_SELECTION ===\n{selection}"
        material = material.strip()
        if not material:
            raise ValueError("the common buffer is empty")
        return material

    @observed_operation("clarify")
    def clarify_terms(self, terms: list[str], fallback_text: str = "") -> Clarification:
        """Explain terms the reader did not understand.

        The ground is the common buffer plus the current selection (see
        `_reader_material`), because the sentence that defines the term may be on
        screen and not yet committed, and asking about that page must not require
        seeding four cues first. A clarification is material for reading on and
        never evidence, so it is the one mode that may lean on the selection — the
        checks stay bound to the buffer.

        The limit on length is enforced by truncation rather than trusted to the
        prompt, the way `extract_cues` checks the shape of every cue. A line about
        a term the reader never named is not an answer and is dropped; the term it
        pretended to answer stays in `omitted`.
        """
        material = self._reader_material(fallback_text)
        asked = parse_terms("\n".join(terms))
        if not asked:
            raise ValueError("no terms to explain")

        window = asked[:MAX_CLARIFY_TERMS]
        # The reader's own spelling is what the window shows back, so a term is
        # displayed the way it was asked for, not the way the model echoed it.
        by_key = {normalise_anchor(term): term for term in window}
        data = parse_json_object(self.client.complete(
            clarify_prompt(
                material,
                window,
                self._prompt("clarify"),
            ),
            max_tokens=400,
        ))
        raw_lines = data.get("lines")
        lines: list[dict[str, str]] = []
        answered: set[str] = set()
        truncated: list[str] = []
        if isinstance(raw_lines, list):
            for item in raw_lines:
                if not isinstance(item, dict):
                    continue
                key = normalise_anchor(str(item.get("term", ""))) or ""
                if key not in by_key or key in answered:
                    continue
                explanation = " ".join(str(item.get("explanation", "")).split()).strip()
                if not explanation:
                    continue
                words = explanation.split()
                if len(words) > CLARIFY_WORDS:
                    explanation = " ".join(words[:CLARIFY_WORDS]).rstrip(",;:")
                    truncated.append(by_key[key])
                lines.append({"term": by_key[key], "explanation": explanation})
                answered.add(key)
        return Clarification(
            lines=lines,
            omitted=[term for term in asked if (normalise_anchor(term) or "") not in answered],
            truncated=truncated,
        )

    @observed_operation("question")
    def answer_question(self, question: str, fallback_text: str = "") -> Answer:
        """Answer the reader's own question with the material as the ground (Alt+C).

        The reader is not naming a word to explain but asking something the text
        may only partly answer, so verbatim support cannot be required: demanding
        it is exactly the recorded failure this mode fixes, where a real question
        was refused with "no basis in the text". The answer must instead state how
        it stands to the material — `grounding` — and a quotation is kept only
        when it was found verbatim, the same rule `check_prediction` applies to
        evidence. Like a clarification, the answer is a reading aid and never
        evidence of learning.
        """
        material = self._reader_material(fallback_text)
        asked = " ".join(question.split()).strip()
        if not asked:
            raise ValueError("question must not be empty")
        system_prompt = self._prompt("clarify_question")
        data = parse_json_object(self.client.complete(
            clarify_question_prompt(material, asked, system_prompt),
            max_tokens=600,
        ))
        text = " ".join(str(data.get("answer", "")).split()).strip()
        if not text:
            raise Web2APIError("empty model response")
        grounding = str(data.get("grounding", "")).strip()
        if grounding not in {"in_text", "partly_in_text", "not_in_text"}:
            grounding = ""
        # An unverified quotation is never shown as if it stood in the text: the
        # model's own words remain in the observation log, and the field is left
        # empty, which reads as "no verbatim support" rather than "nothing said".
        basis = str(data.get("basis", "")).strip()
        if basis and _normalise_grounding(basis) not in _normalise_grounding(material):
            basis = ""
        return Answer(
            text=text,
            question=asked,
            grounding=grounding,
            basis=basis,
            prompt_hash=prompt_hash(system_prompt),
            model=self._model_name(),
        )

    @observed_operation("summary")
    def summarize(self, source_text: str) -> Summary:
        """Compress a passage to its plain-language gist (Ctrl+Q).

        The result is material for reading on, never evidence: it is logged
        together with its input so a summary can be audited later, but it does
        not reach the records ledger or a note's resolution.
        """
        text = source_text.strip()
        if not text:
            raise ValueError("source text must not be empty")
        raw = self.client.complete(
            summary_prompt(text, self._prompt("summary")),
            max_tokens=700,
        )
        result = raw.strip()
        if not result:
            raise Web2APIError("empty model response")
        return Summary(result)

    @observed_operation("example")
    def show_example(self, material: str, query: str = "", intent: str = "") -> Example:
        """One concrete case for the passage the reader highlighted (Alt+G).

        Unlike the checks, this is not bound to the common buffer: the ground is
        the material passed in, meaning the current selection or a fragment the
        reader explicitly chose. The intent enters only when the reader asked
        for it, so by default it cannot silently steer the answer. One model
        call, no planner/critic chain: the reader is waiting to read on.
        """
        text = material.strip()
        if not text:
            raise ValueError("material must not be empty")
        system_prompt = self._prompt("example")
        raw = self.client.complete(
            example_prompt(text, query, intent, system_prompt),
            max_tokens=800,
        )
        result = raw.strip()
        if not result:
            raise Web2APIError("empty model response")
        return Example(
            text=result,
            intent_snapshot=intent.strip(),
            query=query.strip(),
            prompt_hash=prompt_hash(system_prompt),
            model=self._model_name(),
        )

    @observed_operation("reframe")
    def reframe(self, material: str, focus: str, current_frame: str = "") -> Reframe:
        """Offer one source-grounded alternative for an explicit text snapshot.

        The caller supplies the saved buffer or highlight; this method never
        fetches a selection or generates anything without being called.
        """
        source = material.strip()
        chosen_focus = focus.strip()
        if not source:
            raise ValueError("material must not be empty")
        if not chosen_focus:
            raise ValueError("focus must not be empty")
        system_prompt = self._prompt("reframe")
        data = parse_json_object(self.client.complete(
            reframe_prompt(source, chosen_focus, current_frame.strip(), system_prompt),
            max_tokens=400,
        ))
        basis = data.get("basis")
        frame = data.get("frame")
        implication = data.get("implication")
        valid = (
            data.get("status") == "proposal"
            and isinstance(basis, str) and bool(basis.strip()) and basis.strip() in source
            and isinstance(frame, str) and bool(frame.strip())
            and isinstance(implication, str) and bool(implication.strip())
        )
        return Reframe(
            text=(f"Возможный ракурс (предложение модели): {frame.strip()} "
                  f"Опора в тексте: «{basis.strip()}». "
                  f"Следствие: {implication.strip()}" if valid else
                  "По переданному тексту не могу обоснованно предложить другой ракурс для этого фокуса."),
            status="proposal" if valid else "insufficient",
            basis=basis.strip() if valid else "",
            focus=chosen_focus,
            current_frame=current_frame.strip(),
            prompt_hash=prompt_hash(system_prompt),
            model=self._model_name(),
        )

    @observed_operation("goal")
    def suggest_goal(self, material: str, direction: str = "", note: str = "") -> Goal:
        """Word the chosen direction as one concrete goal (inside the bookmark).

        The material is the passage the reader is in; the direction is what they
        picked from a short list; the note is anything they typed themselves. One
        call, no planner chain, and no attempt to diagnose what they know: the
        result is a draft the reader accepts, edits, or discards.
        """
        text = material.strip()
        if not text:
            raise ValueError("material must not be empty")
        system_prompt = self._prompt("goal")
        raw = self.client.complete(
            goal_prompt(text, direction, note, system_prompt),
            max_tokens=200,
        )
        phrase = " ".join(raw.strip().split())
        if not phrase:
            raise Web2APIError("empty model response")
        return Goal(
            text=phrase,
            direction=direction.strip(),
            note=note.strip(),
            prompt_hash=prompt_hash(system_prompt),
            model=self._model_name(),
        )

    @observed_operation("prediction")
    def check_prediction(self, hypothesis: str) -> PredictionCheck:
        """Judge one hypothesis against the buffer.

        A missing quotation is not a failure. The verdict answers "does the
        material support this claim", and some verdicts — a contradicted
        hypothesis, an unclear one — have no supporting sentence to quote. The
        local check therefore distinguishes a quotation that was *invented*
        (verbatim text not in the material) from no quotation at all, instead of
        refusing the whole answer and losing the reader's interaction with it.

        An unverified quotation is never displayed as if it stood in the text;
        it is dropped, the model's own words remain in the observation log, and
        the window says plainly that no verbatim support was found.

        `subject_note` carries knowledge the model added from outside the
        material, kept apart so that "true in the subject but absent here" is
        never read as "confirmed by the text".
        """
        if not self.session.fragments:
            raise ValueError("the common buffer is empty")
        statement = hypothesis.strip()
        if not statement:
            raise ValueError("hypothesis must not be empty")
        system_prompt = self._prompt("prediction")
        allowed = {
            "confirmed",
            "partially_confirmed",
            "not_supported",
            "contradicted",
            "unclear",
        }
        source = _normalise_grounding("\n".join(f.source_text for f in self.session.fragments))
        problems: list[str] = []
        data: dict[str, Any] = {}
        status = ""
        evidence_text = ""
        for attempt in range(2):
            repair = ""
            if attempt:
                repair = (
                    "\nПредыдущий ответ не прошёл локальную проверку: "
                    + "; ".join(problems)
                    + ". Повтори весь JSON. Поле evidence скопируй дословно из SOURCE TEXT; "
                    "если дословной опоры в тексте нет — оставь evidence пустой строкой, "
                    "не подбирай похожую цитату и не пересказывай."
                )
            data = parse_json_object(self.client.complete(
                prediction_check_prompt(
                    self.session.buffer_context(),
                    statement,
                    system_prompt + repair,
                ),
                max_tokens=400,
            ))
            status = str(data.get("status", ""))
            evidence = data.get("evidence", "")
            evidence_text = evidence.strip() if isinstance(evidence, str) else ""
            problems = []
            if status not in allowed:
                problems.append("неизвестный status")
            if evidence_text and _normalise_grounding(evidence_text) not in source:
                problems.append("evidence не является дословной цитатой исходного текста")
            if not problems:
                break
        if problems:
            # Only a verdict that never arrived is fatal: there is then nothing
            # to show. The reader's hypothesis is kept by the caller.
            raise Web2APIError("не удалось проверить гипотезу: " + "; ".join(problems))
        mismatch = data.get("mismatch", "")
        subject_note = data.get("subject_note", "")
        check = PredictionCheck(
            buffer_fragment_ids=[fragment.id for fragment in self.session.fragments],
            hypothesis=statement,
            status=status,
            mismatch=mismatch.strip() if isinstance(mismatch, str) else "",
            # Kept only when it was found in the material, so an empty field
            # means "no verbatim support was found", not "the model said nothing".
            evidence=evidence_text if _normalise_grounding(evidence_text) in source else "",
            subject_note=subject_note.strip() if isinstance(subject_note, str) else "",
            prompt_hash=prompt_hash(system_prompt),
            model=self._model_name(),
        )
        self.session.add_prediction_check(check)
        return check

    def mark_activity(
        self,
        now: float | None = None,
        at_utc: str | None = None,
    ) -> SessionSplit | None:
        """Record one reader action; end the session when a long pause precedes it.

        Returns the session the pause ended, stamped with the moment work actually
        stopped rather than with the returning press: a session closed only when
        the reader comes back would look hours longer than it was.

        `now` (monotonic seconds) and `at_utc` are injectable for tests, the same
        way the ledger lets a caller pass `created_utc`.
        """
        moment = time.monotonic() if now is None else now
        stamp = at_utc or now_iso()
        retired: SessionSplit | None = None
        if self._last_activity is not None and moment - self._last_activity > IDLE_SESSION_SECONDS:
            retired = SessionSplit(
                session=self.session,
                ended_utc=self._last_activity_utc or stamp,
            )
            self.session = self.session.clear()
        self._last_activity = moment
        self._last_activity_utc = stamp
        return retired

    def clear_buffer(self) -> CognitiveSession:
        """Return the old session for history, then start a fresh active buffer."""
        archived = self.session
        self.session = self.session.clear()
        # An explicit clear already made the boundary, so the next action must not
        # report a second split for the same pause.
        self._last_activity = None
        self._last_activity_utc = None
        return archived

    def summary(self) -> dict[str, Any]:
        return {
            "session_id": self.session.id,
            "fragments": len(self.session.fragments),
            "cues": len(self.session.fragments) * 4,
            "feynman_checks": len(self.session.feynman_checks),
        }
