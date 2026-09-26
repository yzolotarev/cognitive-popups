"""Background preparation of one practice task from the material at hand.

The panel's highlight only means something if a task behind it really exists, so
this module is what produces that task: it decides whether the material supports
one small verifiable action, generates it through the existing task pipeline, and
publishes it only after that pipeline's own verification passed.

Three rules shape it:

* **The question is about the material, never about the reader.** The analysis
  asks "what can be done with this text", not "is this person ready to
  practise". There is no readiness score here, and nothing in this module is a
  measurement of the session.
* **The work is bounded.** Two analysis calls at most (the second only to repair
  invalid grounding) and the generator's existing one repair. No loop that keeps
  asking until something usable appears.
* **Nothing is published unverified.** `ready` is written only after a task row
  was saved, and only for a context that is still current (see `desktop.py`).

The stages are the panel's scale, and they are stages of the instrument, not of
the reader:

    material → application → task → check

Nothing here scores a person, and nothing here is allowed to fill that scale by
itself: it advances only when a real step of the work completed.
"""
from __future__ import annotations

import hashlib
import json
import threading
import uuid
from dataclasses import dataclass, replace

from . import prompts
from .client import PRIORITY_BACKGROUND, Web2APIError, parse_json_object
from .nodes import MODE_KINDS, parse_nodes, select_node, strip_wrapper
from .operation_context import OperationContext, operation_scope
from .records import RecordStore
from .service import prompt_hash
from .tasks import (
    DEFAULT_LEVEL,
    MACRO_BY_KEY,
    TaskValidationError,
    active_session_id,
    build_task,
    generate_task,
    grounding_in_source,
)

#: The panel's scale, in order. `completed_stage` is how many of these finished.
STAGES = ("material", "application", "task", "check")

#: What each status means for the scale and for the reader. A status advances the
#: scale only when the corresponding real step is done — see `PROGRESS`.
STATUS_LABELS = {
    "queued": "Ждёт очереди",
    "analyzing": "Ищу применение по материалу",
    "generating": "Готовлю задачу",
    "validating": "Проверяю условие",
    "repairing": "Уточняю условие",
    "ready": "Есть что попробовать",
    "insufficient": "Пока не найдено подходящее действие",
    "failed": "Подготовка не удалась",
    "superseded": "Материал сменился",
    "interrupted": "Подготовка прервана",
    "dismissed": "Подготовка убрана",
}

#: How much of the scale a status proves. `generating` means the application was
#: chosen; `repairing` means a condition existed and is being fixed; only `ready`
#: claims the verification passed.
PROGRESS = {
    "queued": 1,
    "analyzing": 1,
    "generating": 2,
    "validating": 3,
    "repairing": 3,
    "ready": 4,
    "insufficient": 1,
    "failed": 0,
    "superseded": 0,
    "interrupted": 0,
    "dismissed": 0,
}

#: Analysis calls per preparation. The second is a repair of invalid grounding,
#: not a second opinion: an answer that is wrong for another reason is refused.
MAX_ANALYSIS_ATTEMPTS = 2

ANALYSIS_STATUSES = ("candidate", "insufficient")


@dataclass(frozen=True)
class PreparationContext:
    """The exact material one preparation is about, frozen at submit time.

    A snapshot rather than a live reference: the reader keeps reading while the
    model works, and a task must not be attributed to material that arrived
    after the question was asked. `key` is what makes "the same material" mean
    the same thing on the next save.
    """

    source_text: str
    source_hash: str
    fragment_ids: tuple[str, ...] = ()
    goal_id: str = ""
    goal_revision: str = ""
    goal_text: str = ""
    linked_hypothesis: str = ""
    model: str = ""
    prompt_revision: str = ""

    @property
    def key(self) -> str:
        material = json.dumps(
            {
                "source_hash": self.source_hash,
                "goal_id": self.goal_id,
                "goal_revision": self.goal_revision,
                "goal_text": self.goal_text,
                "linked_hypothesis": self.linked_hypothesis,
                "model": self.model,
                "prompt_revision": self.prompt_revision,
            },
            ensure_ascii=False,
            sort_keys=True,
        )
        return hashlib.sha256(material.encode("utf-8")).hexdigest()[:32]

    def payload(self) -> dict:
        return {
            "source_hash": self.source_hash,
            "fragment_ids": list(self.fragment_ids),
            "goal_id": self.goal_id,
            "goal_revision": self.goal_revision,
            "goal_text": self.goal_text,
            "linked_hypothesis": self.linked_hypothesis,
            "model": self.model,
            "prompt_revision": self.prompt_revision,
            "source_chars": len(self.source_text),
        }


@dataclass(frozen=True)
class Analysis:
    status: str
    practice_kind: str = ""
    target: str = ""
    grounding: tuple[str, ...] = ()
    missing_context: tuple[str, ...] = ()
    confidence: str = ""
    #: What the model actually said, kept so a refused analysis can be read back
    #: later. Without it a mismatched answer is invisible: only "status was
    #: invalid" would survive, which is not enough to judge the prompt.
    raw: str = ""

    def payload(self) -> dict:
        return {
            "status": self.status,
            "practice_kind": self.practice_kind,
            "target": self.target,
            "grounding": list(self.grounding),
            "missing_context": list(self.missing_context),
            "confidence": self.confidence,
            "raw": self.raw,
        }


@dataclass(frozen=True)
class Outcome:
    """What happened to one preparation, for the caller's log and the panel."""

    status: str
    preparation_id: str
    context_key: str
    task_id: str = ""
    condition: str = ""
    practice_kind: str = ""
    target: str = ""
    reason: str = ""
    grounding: tuple[str, ...] = ()

    @property
    def ready(self) -> bool:
        return self.status == "ready"


def analysis_system_text() -> str:
    """The editable analysis prompt, falling back to the module default.

    Read per preparation rather than cached: a preparation happens a few times a
    session, and an edit to the prompt should take effect without a restart, the
    way every other prompt in this app does.
    """
    try:
        from .prompt_settings import PromptSettings

        text = PromptSettings().get("practice_analysis")
    except Exception:  # noqa: BLE001 - a missing settings file must not stop preparation
        text = ""
    return text or prompts.PRACTICE_ANALYSIS_SYSTEM


def analysis_prompt(context: PreparationContext, *, repair_errors: tuple[str, ...] = ()) -> str:
    parts = [
        "<MATERIAL>",
        context.source_text.strip(),
        "</MATERIAL>",
    ]
    if context.goal_text.strip():
        parts += ["", "<READER_GOAL>", context.goal_text.strip(), "</READER_GOAL>"]
    else:
        parts += ["", "<READER_GOAL>", "Цель не указана: считай её обзорной.", "</READER_GOAL>"]
    if context.linked_hypothesis.strip():
        parts += ["", "<READER_NOTE>", context.linked_hypothesis.strip(), "</READER_NOTE>"]
    if repair_errors:
        parts += [
            "",
            "Предыдущий ответ не прошёл проверку:",
            *[f"- {error}" for error in repair_errors],
            "Верни исправленный JSON полностью. Не добавляй текст вне JSON.",
        ]
    return "\n".join(parts)


def parse_analysis(raw: str) -> tuple[Analysis, list[str]]:
    """Parse one analysis answer; the second element is what was wrong with it."""
    kept = str(raw or "").strip()[:1200]
    try:
        data = parse_json_object(raw)
    except Web2APIError as exc:
        return Analysis("", raw=kept), [str(exc)]
    if not isinstance(data, dict):
        return Analysis("", raw=kept), ["ответ не является объектом JSON"]
    grounding = data.get("grounding")
    if isinstance(grounding, str):
        grounding = [grounding]
    missing = data.get("missing_context")
    if isinstance(missing, str):
        missing = [missing]
    analysis = Analysis(
        # Only case and padding are normalised: a status is a small fixed set of
        # words, and guessing at anything else would be inventing the answer.
        status=str(data.get("status", "")).strip().lower(),
        practice_kind=str(data.get("practice_kind", "")).strip().lower(),
        target=" ".join(str(data.get("target", "")).split()),
        grounding=tuple(str(item).strip() for item in grounding if str(item).strip())
        if isinstance(grounding, list) else (),
        missing_context=tuple(str(item).strip() for item in missing if str(item).strip())
        if isinstance(missing, list) else (),
        confidence=str(data.get("confidence", "")).strip().lower(),
        raw=kept,
    )
    return analysis, validate_analysis(analysis)


def validate_analysis(analysis: Analysis, source_text: str = "") -> list[str]:
    """Blocking checks: what the rest of the pipeline depends on.

    Only what would break the next step is blocking. A weak target or a low
    confidence is not a refusal — the generator itself reports those and the
    reader never sees this stage at all.
    """
    if analysis.status not in ANALYSIS_STATUSES:
        return [f"недопустимый status: {analysis.status!r}"]
    if analysis.status == "insufficient":
        return []
    errors: list[str] = []
    if analysis.practice_kind not in MACRO_BY_KEY:
        errors.append(f"недопустимый practice_kind: {analysis.practice_kind!r}")
    if not analysis.target:
        errors.append("target пуст")
    if not analysis.grounding:
        errors.append("grounding пуст")
    elif source_text and not grounding_in_source(analysis.grounding, source_text):
        errors.append("grounding не найден в материале дословно")
    return errors


def analyze(client, context: PreparationContext, *, system_text: str = "") -> tuple[Analysis, list[str]]:
    """Ask what can be done with this material, with one bounded repair attempt."""
    system = system_text or analysis_system_text()
    errors: tuple[str, ...] = ()
    analysis = Analysis("")
    for _ in range(MAX_ANALYSIS_ATTEMPTS):
        raw = client.complete(
            [
                {"role": "system", "content": system},
                {"role": "user", "content": analysis_prompt(context, repair_errors=errors)},
            ],
            max_tokens=700,
            priority=PRIORITY_BACKGROUND,
        )
        analysis, errors = parse_analysis(raw)
        if analysis.status == "insufficient":
            # A refusal is a complete answer, not a malformed one: asking again
            # would only repeat the same question.
            return analysis, []
        # Grounding is checked against the material it claims to come from, which
        # is exactly what the next step would otherwise spend a call on.
        errors = tuple(validate_analysis(analysis, context.source_text))
        if not errors:
            return analysis, []
    return analysis, list(errors)


def prepare(
    client,
    store: RecordStore,
    context: PreparationContext,
    *,
    preparation_id: str,
    on_stage=None,
    level: str = DEFAULT_LEVEL,
    system_text: str = "",
) -> Outcome:
    """Run one preparation end to end, writing every transition to the ledger.

    The row is advanced only when a step really finished, so the panel's scale
    can never be filled by the passage of time or by a request being sent.
    """
    notify = on_stage or (lambda _status: None)
    state = {"status": ""}

    def advance(status: str, **fields) -> None:
        if state["status"] == status:
            return
        state["status"] = status
        store.update_preparation(
            preparation_id,
            status=status,
            completed_stage=PROGRESS.get(status, 0),
            stage_label=STATUS_LABELS.get(status, ""),
            **fields,
        )
        notify(status)

    def finish(status: str, *, reason: str = "", **fields) -> Outcome:
        advance(status, error_code=reason[:200], **fields)
        return Outcome(
            status=status,
            preparation_id=preparation_id,
            context_key=context.key,
            reason=reason,
        )

    source = context.source_text.strip()
    if not source:
        return finish("insufficient", reason="no_material")

    advance("analyzing")
    try:
        analysis, errors = analyze(client, context, system_text=system_text)
    except Web2APIError as exc:
        return finish("failed", reason=f"analysis: {exc}")
    analysis_json = json.dumps(analysis.payload(), ensure_ascii=False)
    if analysis.status != "candidate" or errors:
        reason = "analysis_errors: " + "; ".join(errors) if errors else "insufficient_material"
        return finish("insufficient", reason=reason, analysis_json=analysis_json)

    nodes = parse_nodes(strip_wrapper(source)) if source else []
    node = select_node(nodes, analysis.practice_kind) if analysis.practice_kind in MODE_KINDS else None
    prompt = build_task(
        analysis.practice_kind,
        level=level,
        context=source,
        node=node,
    )
    advance(
        "generating",
        practice_kind=analysis.practice_kind,
        analysis_json=analysis_json,
    )
    try:
        generated = generate_task(
            client,
            analysis.practice_kind,
            prompt,
            source,
            node=node,
            nodes=nodes,
            on_stage=advance,
        )
    except TaskValidationError as exc:
        _save_rejected(store, context, analysis, exc.result, prompt, client)
        return finish("failed", reason="verification: " + "; ".join(exc.result.validation_errors),
                      practice_kind=analysis.practice_kind, analysis_json=analysis_json)
    except Web2APIError as exc:
        return finish("failed", reason=f"generation: {exc}",
                      practice_kind=analysis.practice_kind, analysis_json=analysis_json)

    condition = generated.condition.strip()
    if generated.status != "ready" or not condition:
        # The generator itself refused: there is nothing to offer, and saying so
        # is the honest end of this preparation.
        _save_rejected(store, context, analysis, generated, prompt, client)
        return finish("insufficient", reason=f"generator_{generated.status or 'unknown'}",
                      practice_kind=analysis.practice_kind, analysis_json=analysis_json)

    task_id = uuid.uuid4().hex
    store.save_task(
        task_id=task_id,
        session_id=active_session_id(store),
        task_type=analysis.practice_kind,
        task_subtype=generated.subtype,
        level=level,
        context=source,
        condition=condition,
        status="generated",
        prompt_hash=prompt_hash(prompt),
        model=client.model,
        raw_response=generated.raw_response,
        grounding_json=json.dumps(generated.grounding, ensure_ascii=False),
        operations_json=json.dumps(generated.required_operations, ensure_ascii=False),
        attempts=generated.attempts,
        latency_ms=generated.latency_ms,
        domain=generated.domain,
        node_id=generated.node_id,
        node_type=generated.node_type,
        verification_status=generated.verification_status,
        verification_note=generated.verification_note,
        payload_json=json.dumps(generated.payload, ensure_ascii=False),
    )
    advance("ready", task_id=task_id)
    return Outcome(
        status="ready",
        preparation_id=preparation_id,
        context_key=context.key,
        task_id=task_id,
        condition=condition,
        practice_kind=analysis.practice_kind,
        target=analysis.target,
        grounding=generated.grounding or analysis.grounding,
    )


def _save_rejected(store: RecordStore, context: PreparationContext, analysis: Analysis,
                  generated, prompt: str, client) -> None:
    """Keep a refused task in the ledger: a silence would hide why nothing came.

    Written with the same shape the interactive path uses for a task it cannot
    show, so both are read back the same way.
    """
    try:
        store.save_task(
            task_id=uuid.uuid4().hex,
            session_id=active_session_id(store),
            task_type=analysis.practice_kind or "unknown",
            task_subtype=getattr(generated, "subtype", ""),
            level=DEFAULT_LEVEL,
            context=context.source_text.strip(),
            condition="",
            status="invalid",
            prompt_hash=prompt_hash(prompt),
            model=getattr(client, "model", ""),
            raw_response=getattr(generated, "raw_response", ""),
            validation_error="; ".join(getattr(generated, "validation_errors", ()) or ()),
            attempts=getattr(generated, "attempts", 1),
            latency_ms=getattr(generated, "latency_ms", 0),
            domain=getattr(generated, "domain", ""),
            node_id=getattr(generated, "node_id", ""),
            node_type=getattr(generated, "node_type", ""),
            verification_status=getattr(generated, "verification_status", ""),
            verification_note=getattr(generated, "verification_note", ""),
            payload_json=json.dumps(getattr(generated, "payload", {}) or {}, ensure_ascii=False),
        )
    except Exception:  # noqa: BLE001 - the refusal itself must still be reported
        pass


def context_from_fragment(
    fragment,
    *,
    model: str = "",
    goal=None,
    prompt_revision: str = "",
    display_text: str = "",
) -> PreparationContext:
    """Build the snapshot for one saved fragment, plus the goal in force.

    `display_text` lets a caller pass the passage as the reader saw it; the
    fragment's own `source_text` is used otherwise.
    """
    text = (display_text or getattr(fragment, "source_text", "") or "").strip()
    goal_id = getattr(goal, "id", "") or ""
    return PreparationContext(
        source_text=text,
        source_hash=str(getattr(fragment, "source_hash", "") or ""),
        fragment_ids=(str(getattr(fragment, "id", "") or ""),),
        goal_id=goal_id,
        goal_revision=str(getattr(goal, "updated_at", "") or getattr(goal, "created_at", "") or ""),
        goal_text=str(getattr(goal, "text", "") or ""),
        model=model,
        prompt_revision=prompt_revision,
    )


def new_preparation_id() -> str:
    return uuid.uuid4().hex


class PreparationQueue:
    """One background preparation at a time; only the newest context waits.

    The queue never grows. A second submission while one is running replaces the
    pending slot, so a reader who moves through ten passages does not leave ten
    preparations to grind through afterwards — and does not have yesterday's
    material presented as today's.

    Everything here runs off the UI thread. The callbacks it is given are called
    from that thread too, so the daemon marshals them back onto its own loop
    rather than touching GTK or the event chain from here.
    """

    def __init__(
        self,
        store: RecordStore,
        client,
        *,
        session_id: str = "",
        system_text: str = "",
        on_stage=None,
        on_result=None,
        poll_seconds: float = 0.5,
    ):
        self.store = store
        self.client = client
        self._session_id = session_id
        self._system_text = system_text
        self._on_stage = on_stage or (lambda *_args: None)
        self._on_result = on_result or (lambda *_args: None)
        self._poll_seconds = poll_seconds
        self._condition = threading.Condition()
        self._pending: tuple[PreparationContext, str] | None = None
        self._running_id = ""
        self._latest_key = ""
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    # ── lifecycle ────────────────────────────────────────────────────────────

    def start(self) -> None:
        if self._thread is not None:
            return
        self._stop.clear()
        self._thread = threading.Thread(target=self._serve, name="practice-preparation", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        with self._condition:
            self._condition.notify_all()
        thread, self._thread = self._thread, None
        if thread is not None and thread.is_alive():
            thread.join(timeout=5.0)

    # ── submission ───────────────────────────────────────────────────────────

    def submit(self, context: PreparationContext, *, enabled: bool = True) -> str | None:
        """Queue this context. Returns the preparation id, or None when skipped.

        The same material is never sent twice: one row per `context_key`, for
        good. A skipped submission is a normal outcome, not an error — it is what
        stops a re-saved fragment from starting the whole analysis again.
        """
        if not enabled or not context.source_text.strip():
            return None
        with self._condition:
            self._latest_key = context.key
        if self.store.preparation_by_context(context.key) is not None:
            return None
        preparation_id = new_preparation_id()
        self.store.save_preparation(
            preparation_id=preparation_id,
            context_key=context.key,
            status="queued",
            completed_stage=PROGRESS["queued"],
            stage_label=STATUS_LABELS["queued"],
            context_json=json.dumps(context.payload(), ensure_ascii=False),
        )
        with self._condition:
            self._pending = (context, preparation_id)
            self._condition.notify_all()
        return preparation_id

    def latest_context_key(self) -> str:
        with self._condition:
            return self._latest_key

    # ── worker ───────────────────────────────────────────────────────────────

    def _serve(self) -> None:
        while not self._stop.is_set():
            with self._condition:
                while self._pending is None and not self._stop.is_set():
                    self._condition.wait(timeout=self._poll_seconds)
                if self._stop.is_set():
                    break
                item = self._pending
                self._pending = None
            if item is None:
                continue
            context, preparation_id = item
            self._running_id = preparation_id
            try:
                self._run_one(context, preparation_id)
            finally:
                self._running_id = ""

    def _run_one(self, context: PreparationContext, preparation_id: str) -> None:
        background = OperationContext(kind="background", buffer_session_id=self._session_id or None)
        try:
            with operation_scope(background):
                outcome = prepare(
                    self.client,
                    self.store,
                    context,
                    preparation_id=preparation_id,
                    on_stage=lambda status: self._on_stage(status, preparation_id),
                    system_text=self._system_text,
                )
        except Exception as exc:  # noqa: BLE001 - background work must never crash the daemon
            self._finish_row(preparation_id, "failed", error_code=str(exc))
            self._on_result(None, str(exc))
            return
        self._on_result(self._retire_if_stale(context, outcome), "")

    def _finish_row(self, preparation_id: str, status: str, *, error_code: str = "") -> None:
        try:
            self.store.update_preparation(
                preparation_id,
                status=status,
                completed_stage=PROGRESS.get(status, 0),
                stage_label=STATUS_LABELS.get(status, ""),
                error_code=error_code[:200],
            )
        except Exception:  # noqa: BLE001 - a failed bookkeeping write is not a crash
            pass

    def _retire_if_stale(self, context: PreparationContext, outcome: Outcome) -> Outcome:
        """Never present work for material the reader has already left.

        The check happens once, after the run: writing `superseded` earlier would
        be overwritten by the run's own next transition. The task stays in the
        ledger — the work was real — but it is not offered as current.
        """
        if context.key == self.latest_context_key():
            return outcome
        self._finish_row(outcome.preparation_id, "superseded", error_code="superseded_by_newer_material")
        return replace(outcome, status="superseded", reason="superseded_by_newer_material")
