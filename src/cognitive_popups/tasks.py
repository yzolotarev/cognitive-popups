"""Task generator: practice built from the material the reader is actually reading.

The advanced types are operations, not topics: practising a method, proving a result
does not depend on a representation, deriving a statement, refuting a plausible
intuition, solving a non-standard problem, and revisiting an earlier notion
through a more general structure.

Each type yields **two** prompts, not one. Bundling the solution into the task
turns every exercise into a worked example: a task you cannot attempt is not a
task, and there is nothing for the reader's own attempt to be checked against.
So the condition comes first and the solution is asked for separately.

The condition never names the idea of the solution. Leaked hints were the other
half of the same defect: a statement of the answer in the setup reads as a task
but is a lecture.

The subject and the topic are optional. Called from a hotkey there is no form to
fill in, and the material the reader is on *is* the subject: the prompts then say
"по материалу выше" and the injected session supplies the rest.

The hotkey flow calls the model itself and shows the result in a window, the way
the other four modes do. Handing the reader a prompt to paste into a model by hand
would be a step the rest of the loop does not have, and it hides the task behind a
second tool. The solution stays a separate, explicitly invoked act controlled by
the reader.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import uuid
from collections import Counter
from collections.abc import Sequence
from contextvars import ContextVar
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path

from . import observation, verify
from .client import GeminiWeb2API, Web2APIError, parse_json_object
from .event_log import EventChain, default_log
from .nodes import MODE_KINDS, TaskNode, parse_nodes, render_nodes, select_node, strip_wrapper
from .operation_context import OperationContext, current_operation, operation_scope
from .records import RecordStore
from .service import prompt_hash, output_artifact

_TASK_CHAIN: ContextVar[EventChain | None] = ContextVar("task_chain", default=None)
_TASK_SOURCE: ContextVar[str | None] = ContextVar("task_source", default=None)


class ReadingContext(str):
    """The exact assembled prompt context plus its recorded technical buffer."""

    def __new__(cls, text, *, session_id=None, buffer=(), event_ids=()):
        value = super().__new__(cls, text)
        value.session_id = session_id
        value.buffer = list(buffer)
        value.event_ids = list(event_ids)
        return value

#: How much of the reader's material is injected. Enough to fix the subject, small
#: enough not to crowd out the task itself.
MAX_CONTEXT_CHARS = 12000
FRAGMENT_CLIP = 2200
DEFAULT_FRAGMENTS = 3
DEFAULT_EVENTS = 10

LEVELS = ("базовый", "продвинутый", "исследовательский")
DEFAULT_LEVEL = "продвинутый"


#: The one rule that applies to every type: the condition is not the answer.
FORM_RULES = """

Требования к форме:
1. Поле condition содержит только условие. Решения, ответа, подсказки и «разбора» в нём быть не должно.
2. Не называй в условии идею решения, нужный метод, эффект или механизм — даже намёком.
3. Если выбранный тип не опирается на материал, верни needs_context вместо выдумывания метода или объекта.
4. grounding содержит 1–3 короткие дословные опоры из материала; каждая опора — не более 12 слов, без пересказа и новых слов.
5. Если задача вводит свои числа или данные, включи их в condition и не приписывай источнику; источник даёт понятия, а не эти числа.
6. confidence — твоя уверенность, что условие корректно и разрешимо: high, medium или low. При low задача будет показана с пометкой о неуверенности.
7. Если задача вычислительная и её ответ можно проверить машинно, продублируй данные в payload; иначе верни payload пустым.
8. Оформи condition так, чтобы его можно было читать с экрана: сначала данные отдельным абзацем, затем требование отдельной последней строкой, начинающейся со слова «Требуется:». Не сливай условие и требование в один абзац. Разбивай абзацем и перечисление данных, если их больше двух.
9. Условие должно быть самодостаточным для попытки без угадывания невидимого объекта. Если задача относится к таблице, Excel, коду, схеме, графику или диапазону, покажи в condition минимальный необходимый фрагмент: заголовки, строки/значения, обозначения или схему. Один адрес вроде D2:D50 — это только координата, а не содержимое.
10. Если порядок строк, взаимное расположение столбцов или другая структура влияет на ответ, покажи эту структуру и однозначно опиши, что именно требуется определить. Не смешивай два разных направления XLOOKUP: «вправо/влево» означает положение return_array относительно lookup_array, а поиск сверху вниз/снизу вверх — порядок совпадений и отдельный search_mode. Для Excel/XLOOKUP condition обязательно включай маленькую видимую таблицу с заголовками и минимум двумя строками, помеченную «Учебные данные», даже если формулу формально можно записать по одним адресам диапазонов. Для поиска вправо/влево расположи lookup_array и return_array в таблице соответственно и попроси применить формулу, а не просто назвать диапазоны. Если нужной структуры нет в материале, создай явно обозначенный учебный пример с новыми данными, сохранив тот же навык. Не выдавай учебные данные за источник и не ссылайся на невидимый объект. needs_context используй только если даже такой пример нельзя сформулировать.

Верни только JSON:
{"status":"ready|not_applicable|needs_context","condition":"условие или пустая строка","grounding":["дословная опора"],"required_operations":["действие ученика"],"difficulty":"quick|standard|advanced","confidence":"high|medium|low","payload":{}}"""

SOLUTION_RULES = """
Ученик уже попробовал решить сам — поэтому разбор должен отвечать на задачу, а не подменять её.
Доведи разбор до однозначного результата. Не переписывай теорию вокруг задачи."""


@dataclass(frozen=True)
class TaskType:
    key: str
    title: str
    tag: str
    task: str
    solution: str


@dataclass(frozen=True)
class MacroMode:
    key: str
    title: str
    instruction: str


@dataclass(frozen=True)
class GeneratedTask:
    status: str
    condition: str
    grounding: tuple[str, ...]
    required_operations: tuple[str, ...]
    difficulty: str
    raw_response: str
    validation_errors: tuple[str, ...]
    attempts: int
    latency_ms: int
    artifact_id: str | None = None
    domain: str = ""
    subtype: str = ""
    confidence: str = ""
    #: Machine checkable data for the verifier, e.g. {"type":"linear_system",
    #: "matrix":[[…]],"rhs":[…]}. Empty when the task cannot be checked in code.
    payload: dict = field(default_factory=dict)
    node_id: str = ""
    node_type: str = ""
    verification_status: str = ""
    verification_note: str = ""
    #: Non-blocking findings about a task that is shown anyway. A formal defect is
    #: a hint for the reader, not a reason to withhold the task.
    marks: tuple[str, ...] = ()


class TaskValidationError(Web2APIError):
    def __init__(self, result: GeneratedTask):
        self.result = result
        super().__init__("задача не прошла проверку: " + "; ".join(result.validation_errors))


TYPES: tuple[TaskType, ...] = (
    TaskType(
        key="7",
        title="Быстро: конкретный пример",
        tag="Один небольшой пример, который делает абстрактную связь видимой.",
        task="""Сформулируй одну короткую задачу {by_topic}, которую можно решить за несколько минут и которая превращает одну абстрактную связь из материала в конкретный пример.
Требования:
1. Дай все необходимые числа или объекты.
2. Задача проверяет только одну связь и имеет короткий проверяемый ответ.
3. Не называй идею решения в условии.{form}""",
        solution="""Реши задачу кратко, по шагам. В конце одной фразой свяжи результат с исходной абстрактной идеей.""",
    ),
    TaskType(
        key="8",
        title="Быстро: различить понятия",
        tag="Различение двух похожих объектов, ролей или отношений.",
        task="""Сформулируй короткую задачу {by_topic}, где нужно различить два похожих понятия или отношения из материала.
Требования:
1. Дай конкретный пример и один похожий контрпример.
2. Попроси назвать различие и объяснить его одной-двумя фразами.
3. Не сообщай правильное различие в условии.{form}""",
        solution="""Разбери пример и контрпример, назови точное различие и укажи, какая ошибка возникает при их смешении.""",
    ),
    TaskType(
        key="9",
        title="Быстро: сравнить аналогию",
        tag="Проверка, где знакомая аналогия работает и где перестаёт работать.",
        task="""Сформулируй задачу {by_topic}, которая сравнивает новый объект с одной знакомой аналогией из материала или контекста.
Попроси назвать одно сходство и одно различие. Дай достаточно данных, чтобы ответить без изучения всей темы. Не подсказывай ответ.{form}""",
        solution="""Назови сходство и различие, затем отдельно укажи границу, за которой аналогия вводит в заблуждение.""",
    ),
    TaskType(
        key="10",
        title="Быстро: один шаг метода",
        tag="Выполнение одного промежуточного шага с явной опорой.",
        task="""Сформулируй задачу {by_topic}, в которой нужно выполнить только один промежуточный шаг метода из материала.
Разрешается дать формулу или образец, но не результат этого шага. Не требуй полного решения большой задачи.{form}""",
        solution="""Покажи только нужный шаг и коротко объясни, зачем он нужен в полном методе. Не разворачивай соседнюю теорию.""",
    ),
    TaskType(
        key="1",
        title="Отработка метода",
        tag="Пошаговая задача с одним проверяемым ответом — тренирует технику.",
        task="""Сформулируй одну тренировочную задачу {by_topic}, для решения которой нужно
применить конкретный пошаговый метод или процедуру, а не общее рассуждение.

Требования:
1. Условие содержит конкретные данные: числа, факты, объекты со значениями, а не абстрактные обозначения.
2. У задачи один однозначно проверяемый ответ.{form}""",
        solution="""Дай образцовое решение по шагам, явно называя каждый шаг метода.
В конце отдельной строкой назови типичную ошибку, которую здесь делают.""",
    ),
    TaskType(
        key="2",
        title="Докажи независимость",
        tag="Доказательство свойства вне конкретного представления или частного случая.",
        task="""Сформулируй задачу {by_topic}, где нужно доказать, что некоторое свойство объекта
НЕ зависит от выбора его конкретного представления: системы координат, точки отсчёта,
единиц измерения, реализации, обозначений или частного случая. Выбери форму независимости,
естественную для этого материала.

Требования:
1. Назови свойство и то, относительно чего заявлена независимость.
2. Задача требует общего доказательства, а не проверки на одном примере.
3. Не намекай, почему свойство инвариантно.{form}""",
        solution="""Дай полное доказательство. В конце назови, какое преобразование или замена
проверялись и почему инвариантность вообще возможна.""",
    ),
    TaskType(
        key="3",
        title="Выведи закон сам",
        tag="Цепочка шагов, ведущая к самостоятельному открытию утверждения.",
        task="""Возьми одно нетривиальное утверждение, закон или теорему {from_topic} и разбей его
вывод на цепочку из 3–6 последовательных под-задач, которые вместе составляют полный вывод —
так, будто ученик сам, шаг за шагом, приходит к этому утверждению, отталкиваясь только от
базовых определений и уже известных фактов.

Требования:
1. Каждый шаг — отдельный прямой вопрос, отвечаемый из предыдущих шагов.
2. Итоговое утверждение в условии НЕ формулируй: его должен получить ученик.
3. Вопрос шага не должен называть вывод или его механизм раньше, чем ученик к нему придёт.{form}""",
        solution="""Ответь на каждый шаг по порядку, а в конце сформулируй утверждение,
к которому приводит вся цепочка, и назови, на каком шаге вывод становится неизбежным.""",
    ),
    TaskType(
        key="4",
        title="Найди контрпример",
        tag="Опровержение правдоподобной, но ложной интуиции.",
        task="""Сформулируй вопрос вида «Верно ли, что…» про утверждение {from_topic}, которое звучит
правдоподобно — по аналогии с более простым или более ранним случаем, — но на самом деле либо
неверно в общем случае, либо верно только при дополнительных условиях.

Требования:
1. Вопрос не выдаёт ответ: ни «на самом деле нет», ни намёков на контрпример.
2. Утверждение должно быть достаточно конкретным, чтобы его можно было опровергнуть или подтвердить.{form}""",
        solution="""Сначала ответь: верно или нет. Затем дай контрпример либо условия, при которых
утверждение всё же верно. Обязательно назови, из-за какого именно ошибочного обобщения
возникает ложная интуиция.""",
    ),
    TaskType(
        key="5",
        title="Нестандартная задача",
        tag="Уровень олимпиады или научного семинара, требует нетривиальной идеи.",
        task="""Придумай нестандартную задачу {by_topic} уровня студенческой олимпиады или научного
семинара. Задача должна:
1. требовать комбинации минимум двух разных идей или разделов {subject_words};
2. не решаться прямым применением одного известного метода — нужна нетривиальная идея;
3. иметь точную формулировку и единственный проверяемый результат.

Требования: не называй ни нужный раздел, ни ключевой эффект, ни идею решения.{form}""",
        solution="""Дай полное решение. Отдельно перечисли, какие две идеи нужно было соединить
и в каком месте задача перестаёт решаться прямым методом.""",
    ),
    TaskType(
        key="6",
        title="Спираль на новом уровне",
        tag="Возврат к более ранней теме через более общую структуру.",
        task="""Выбери более простое или более раннее понятие внутри {subject_words}, на котором
строится понимание {topic_words}. Сформулируй одну задачу {by_topic}, которая заставляет
посмотреть на это понятие под новым, более общим углом.

Требования:
1. Раннее понятие укажи только в grounding, не добавляй его отдельной строкой к condition.
2. НЕ называй в condition, в какую более общую структуру это понятие входит и чем именно обобщается:
   это должен увидеть ученик, иначе связка понятий уже содержит ответ.{form}""",
        solution="""Реши задачу и явно покажи, в какую более общую структуру {from_topic}
входит раннее понятие и как именно оно оказывается её частным случаем.""",
    ),
)

BY_KEY = {task.key: task for task in TYPES}

MACRO_MODES: tuple[MacroMode, ...] = (
    MacroMode(
        "apply", "Применить",
        "Примени способ, правило, модель или процедуру из материала к новому конкретному случаю. "
        "Для математики это может быть вычисление, решение или один шаг метода; для гуманитарной "
        "темы — реконструкция аргумента или разбор кейса по критериям.",
    ),
    MacroMode(
        "distinguish", "Различить",
        "Дай два похожих случая или объекта и попроси различить их по критерию из материала. "
        "Выбирай классификацию, сравнение, пример/не-пример или проверку границы понятия.",
    ),
    MacroMode(
        "justify", "Обосновать",
        "Попроси вывести или обосновать нетривиальную связь из материала. "
        "Для математики это доказательство; для истории — причинная связь; для философии — "
        "реконструкция аргумента; для психологии — проверка объяснения по наблюдаемым признакам.",
    ),
    MacroMode(
        "boundary", "Проверить границу",
        "Попроси построить объект или ситуацию с заданными свойствами, изменить условие, "
        "провести мысленный эксперимент или найти контрпример к чрезмерному обобщению.",
    ),
)
MACRO_BY_KEY = {mode.key: mode for mode in MACRO_MODES}
# Kept as a compatibility export for callers that used the former quick menu.
QUICK_KEYS = ("7", "8", "9", "10")


def active_session_id(store: RecordStore) -> str | None:
    """The reading session to take material from.

    Only an open session with fragments is eligible. Falling back to a closed
    session silently changes the subject of the task; the reader can still name
    an older subject explicitly with ``--topic``.
    """
    rows = store.sessions()
    if not rows:
        return None
    counts = Counter(store.fragment_session_pairs().values())
    for row in reversed(rows):
        if not row.get("closed_epoch") and counts.get(row["id"]):
            return row["id"]
    return None


def _clip(text: str, limit: int) -> str:
    # Preserve newlines, operators and formula layout. Flattening whitespace made
    # OCR-heavy mathematics harder to distinguish and could join unrelated tokens.
    return text if len(text) <= limit else text[: limit - 1].rstrip() + "…"


def reading_context(
    *,
    records_path: str | Path | None = None,
    events_path: str | Path | None = None,
    fragments: int = DEFAULT_FRAGMENTS,
    events: int = DEFAULT_EVENTS,
    limit: int = MAX_CONTEXT_CHARS,
) -> str:
    """Return source fragments only; telemetry is not study material.

    ``events_path`` and ``events`` remain accepted for CLI/API compatibility, but
    event-log rows are deliberately excluded. Copying a popup, opening a menu, or
    invoking a task must never become the subject of the next task.
    """
    store = RecordStore(records_path)
    lines: list[str] = []

    session_id = active_session_id(store)
    buffer = []
    if session_id:
        buffer = store.load_fragments(session_id)
        read = buffer[-fragments:] if fragments else []
        if read:
            lines.append(f"<READING_SESSION id=\"{session_id[:8]}\">")
            for fragment in read:
                terms = ", ".join(cue["term"] for cue in fragment.cue_details) or "—"
                lines.append(f"<SOURCE_FRAGMENT id=\"{fragment.id}\" sha256=\"{fragment.source_hash}\">")
                lines.append(_clip(fragment.source_text, FRAGMENT_CLIP))
                lines.append("</SOURCE_FRAGMENT>")
                lines.append(f"<CUES>{terms}</CUES>")
            lines.append("</READING_SESSION>")
        else:
            lines.append(f"ЧТО ЧИТАЛ: сессия {session_id[:8]} ещё не содержит фрагментов.")

    block = "\n".join(lines)
    text = block if len(block) <= limit else block[: limit - 1].rstrip() + "…"
    return ReadingContext(text, session_id=session_id,
        buffer=[f.to_dict() for f in buffer], event_ids=[])


_LEAKED_ANSWER = re.compile(r"(?im)^\s*(?:решение|ответ|подсказка|разбор)\s*:")

_ALLOWED_TASK_STATUSES = {"ready", "not_applicable", "needs_context"}
_ALLOWED_DIFFICULTIES = {"quick", "standard", "advanced"}
_GROUNDING_STOPWORDS = {
    "это", "как", "для", "или", "при", "что", "где", "когда", "между", "через",
    "the", "and", "for", "with", "from", "that", "this", "into", "when", "where",
}


def _search_text(value: str) -> str:
    return " ".join(value.casefold().split())


def grounding_in_source(quotes: Sequence[str], source: str) -> bool:
    """Whether each quoted support really comes from the material.

    Ordinary inflection is tolerated (see `_grounding_tokens_in_source`);
    invented wording is not. This is a blocking check only where it is cheap and
    safe — deciding whether to spend a generation call on a background
    candidate — never on a task that is already being shown to a reader.
    """
    folded = _search_text(source)
    known = _content_tokens(source)
    for quote in quotes:
        tokens = _content_tokens(str(quote))
        if not tokens:
            return False
        if _search_text(str(quote)) in folded:
            continue
        if not _grounding_tokens_in_source(tokens, known):
            return False
    return True


def _content_tokens(value: str) -> set[str]:
    return {
        token
        for token in re.findall(r"[^\W_]+", value.casefold(), flags=re.UNICODE)
        if len(token) >= 3 and token not in _GROUNDING_STOPWORDS
    }


def _grounding_tokens_in_source(tokens: set[str], source: set[str]) -> bool:
    """Allow ordinary inflection differences while keeping grounding source-bound."""
    for token in tokens:
        if token in source:
            continue
        if len(token) < 5 or not any(
            len(other) >= 5 and (token.startswith(other[:5]) or other.startswith(token[:5]))
            for other in source
        ):
            return False
    return True


def validate_generated_task(key: str, data: dict[str, object], context: str) -> list[str]:
    """Blocking checks only: the shape the rest of the pipeline depends on.

    A defect of *content* — a weak anchor, a missing domain, a condition that
    looks like a leaked solution — is no longer a refusal. It is reported by
    ``task_marks`` and shown with the task, so the reader decides. The former
    refusals were measured to be unreliable (an anchor with a repaired formula
    and a task about a different subject both passed) and costly (a second model
    call, and good tasks discarded over a word count). What stays blocking is
    only what would break rendering or the ledger.
    """
    errors: list[str] = []
    status = str(data.get("status", "")).strip()
    condition = str(data.get("condition", "")).strip()
    operations = data.get("required_operations")
    difficulty = str(data.get("difficulty", "")).strip()
    confidence = str(data.get("confidence", "")).strip().casefold()
    payload = data.get("payload")

    if confidence and confidence not in ("high", "medium", "low"):
        errors.append("confidence должен быть high, medium или low")
    if payload is not None and not isinstance(payload, dict):
        errors.append("payload должен быть объектом")
    if status not in _ALLOWED_TASK_STATUSES:
        errors.append("status должен быть ready, not_applicable или needs_context")
    if status != "ready":
        if condition:
            errors.append("при неготовом status поле condition должно быть пустым")
        return errors
    if not condition:
        errors.append("condition пуст")
    if not isinstance(operations, list) or not any(str(item).strip() for item in operations):
        errors.append("required_operations должен называть действие ученика")
    if difficulty not in _ALLOWED_DIFFICULTIES:
        errors.append("difficulty должен быть quick, standard или advanced")
    return errors


def task_marks(key: str, data: dict[str, object], context: str) -> list[str]:
    """Findings shown with a task instead of suppressing it.

    These were refusals. They are kept because they carry information — "the
    anchor is not in the material", "this looks like a leaked hint", "the model
    is unsure" — and demoted to marks because none is reliable enough to decide
    for the reader: the anchor check is blind to formulas, and a single shared
    word is enough to look grounded.
    """
    marks: list[str] = []

    def add(mark: str) -> None:
        if mark not in marks:
            marks.append(mark)

    status = str(data.get("status", "")).strip()
    condition = str(data.get("condition", "")).strip()
    grounding = data.get("grounding")
    domain = str(data.get("domain", "")).strip()
    subtype = str(data.get("subtype", "")).strip()
    confidence = str(data.get("confidence", "")).strip().casefold()
    payload = data.get("payload")
    source_tokens = _content_tokens(context)

    if status in {"needs_context", "not_applicable"} and key in MACRO_BY_KEY and source_tokens:
        add("режим отказался при непустом материале")
    if status != "ready":
        return marks

    if _LEAKED_ANSWER.search(condition):
        add("условие помечено как решение или ответ")
    if not isinstance(grounding, list) or not grounding:
        add("опора не указана")
    else:
        searchable = _search_text(context)
        for anchor in grounding[:3]:
            text = str(anchor).strip()
            if len(text.split()) > 12:
                add("длинная опора: " + (text if len(text) <= 40 else text[:39] + "…"))
            elif len(text) < 3 or (
                _search_text(text) not in searchable
                and not _grounding_tokens_in_source(_content_tokens(text), source_tokens)
            ):
                add("опора не найдена в материале")
    if condition and source_tokens and not (_content_tokens(condition) & source_tokens):
        add("условие не опирается ни на одно понятие материала")
    if key in MACRO_BY_KEY and not (domain and subtype):
        add("не указаны domain и subtype")
    machine_checkable = isinstance(payload, dict) and bool(payload.get("type"))
    if confidence == "low" and not machine_checkable:
        add("модель не уверена в задаче, машинной проверки нет")
    return marks


def parse_generated_task(raw: str, key: str, context: str) -> tuple[GeneratedTask, list[str]]:
    try:
        data = parse_json_object(raw)
    except Web2APIError as exc:
        empty = GeneratedTask("", "", (), (), "", raw, (str(exc),), 1, 0)
        return empty, [str(exc)]
    errors = validate_generated_task(key, data, context)
    marks = task_marks(key, data, context)
    payload = data.get("payload")
    task = GeneratedTask(
        status=str(data.get("status", "")).strip(),
        condition=str(data.get("condition", "")).strip(),
        grounding=tuple(str(item).strip() for item in data.get("grounding", []) if str(item).strip())
        if isinstance(data.get("grounding"), list) else (),
        required_operations=tuple(
            str(item).strip() for item in data.get("required_operations", []) if str(item).strip()
        ) if isinstance(data.get("required_operations"), list) else (),
        difficulty=str(data.get("difficulty", "")).strip(),
        raw_response=raw,
        validation_errors=tuple(errors),
        attempts=1,
        latency_ms=0,
        domain=str(data.get("domain", "")).strip(),
        subtype=str(data.get("subtype", "")).strip(),
        confidence=str(data.get("confidence", "")).strip(),
        payload=payload if isinstance(payload, dict) else {},
        marks=tuple(marks),
    )
    return task, errors


def generate_task(
    client,
    key: str,
    prompt: str,
    context: str,
    *,
    node: TaskNode | None = None,
    nodes: Sequence[TaskNode] = (),
    on_stage=None,
) -> GeneratedTask:
    """Generate one task, reporting progress through `on_stage` if given.

    `on_stage(name)` is how a caller that cannot show a window — the background
    preparation — reports which part of the work is running. The hotkey path
    passes nothing and behaves exactly as before.
    """
    notify = on_stage or (lambda _stage: None)
    ctx = current_operation() or OperationContext()
    ctx = replace(ctx, buffer_session_id=getattr(context, "session_id", None) or ctx.buffer_session_id)
    with operation_scope(ctx):
        source = observation.record_artifact("task_input", {
            "task_type": key, "prompt": prompt, "context": context,
            "node_id": getattr(node, "id", None), "node_type": getattr(node, "kind", None),
        }, source_artifact_id=_TASK_SOURCE.get())
        store = getattr(client, "observation_store", None)
        requests = store.request_ids_for_operation if store is not None else observation.request_ids_for_operation
        before = set(requests(ctx.operation_id))
        output_artifact.set(None)
        notify("generating")
        try:
            result = _generate_task(client, key, prompt, context, node=node, nodes=nodes,
                                    on_stage=notify)
        except BaseException as exc:
            artifact = observation.record_artifact("task_failed", {
                "error_type": type(exc).__name__,
                "result": asdict(exc.result) if isinstance(exc, TaskValidationError) else None,
            }, request_ids=[r for r in requests(ctx.operation_id) if r not in before],
                source_artifact_id=source)
            if isinstance(exc, TaskValidationError):
                exc.result = replace(exc.result, artifact_id=artifact)
            raise
        artifact = observation.record_artifact("task_generated", asdict(result),
            request_ids=[r for r in requests(ctx.operation_id) if r not in before],
            source_artifact_id=source)
        output_artifact.set(artifact)
        return replace(result, artifact_id=artifact)


def _generate_task(
    client,
    key: str,
    prompt: str,
    context: str,
    *,
    node: TaskNode | None = None,
    nodes: Sequence[TaskNode] = (),
    on_stage=None,
) -> GeneratedTask:
    """Generate, validate, verify and repair once. An unverified task is never shown."""
    notify = on_stage or (lambda _stage: None)
    started = time.monotonic()
    last: GeneratedTask | None = None
    errors: list[str] = []
    for attempt in range(1, 3):
        request = prompt
        if attempt == 2:
            notify("repairing")
            request += (
                "\n\nПредыдущий ответ не прошёл проверку:\n- "
                + "\n- ".join(errors)
                + "\nВерни исправленный JSON полностью. Не добавляй текст вне JSON."
                + f"\n<PREVIOUS_RESPONSE>\n{last.raw_response if last else ''}\n</PREVIOUS_RESPONSE>"
            )
        raw = client.complete([{"role": "user", "content": request}], max_tokens=1400)
        parsed, errors = parse_generated_task(raw, key, context)
        verification = verify.VerificationResult("", "")
        if not errors:
            verification = verify.verify_task(parsed, node=node, nodes=nodes, mode=key)
            if verification.status != "passed":
                errors = [f"проверка до показа: {verification.note}"]
        last = replace(
            parsed,
            attempts=attempt,
            latency_ms=int((time.monotonic() - started) * 1000),
            validation_errors=tuple(errors),
            node_id=getattr(node, "id", "") or "",
            node_type=getattr(node, "kind", "") or "",
            verification_status=verification.status,
            verification_note=verification.note,
        )
        if not errors:
            return last
    assert last is not None
    raise TaskValidationError(last)


def _render(template: str, *, subject: str, topic: str, form: bool = False) -> str:
    """Fill the shared phrasing.

    With no topic the prompts lean on the injected material instead of naming a
    theme, because the caller who has no form to fill in — a hotkey — has no way
    to name one.
    """
    if topic:
        topic_fills = {
            "by_topic": f"по теме «{topic}»",
            "from_topic": f"из темы «{topic}»",
            "topic_words": f"темы «{topic}»",
        }
    else:
        topic_fills = {
            "by_topic": "по материалу выше",
            "from_topic": "из материала выше",
            "topic_words": "темы из материала выше",
        }
    fills = {
        **topic_fills,
        "subject_words": f"предмета «{subject}»" if subject else "предмета",
        "form": FORM_RULES if form else "",
    }
    for name, value in fills.items():
        template = template.replace("{" + name + "}", value)
    return template


def _header(subject: str, level: str) -> str:
    subject_clause = f" по предмету «{subject}»" if subject else ""
    return f"Ты — опытный преподаватель{subject_clause}. Уровень: {level}.\n"


def _context_block(context: str, topic: str) -> str:
    if not context:
        return ""
    precedence = (
        f"Тема «{topic}» ниже — уточнение, а не замена материала.\n"
        if topic
        else "Материал выше и есть тема задачи.\n"
    )
    heading = (
        "Материал из архивной сессии:"
        if context.startswith("<ARCHIVED_SOURCE ")
        else "Материал, который ученик читает сейчас:"
    )
    return (
        f"\n{heading}\n\n"
        f"{context}\n\n"
        "Задача должна опираться на этот материал: бери его понятия, обозначения и случаи. "
        + precedence
    )


def _node_block(node: TaskNode | None) -> str:
    """Pin the task to one parsed node instead of the whole fragment."""
    if node is None:
        return ""
    text = node.text
    if len(text) > 1200:
        text = text[:1199].rstrip() + "…"
    kind = node.kind
    statement_note = (
        "Тип узла — утверждение: ученик должен сам вывести или доказать связь, "
        "а не пересказать готовое доказательство из материала.\n"
        if kind == "statement"
        else ""
    )
    return (
        "\nУЗЕЛ МАТЕРИАЛА, ПРО КОТОРЫЙ ЗАДАЧА (ровно один):\n"
        f"тип: {kind}\nзаголовок: {node.title}\n"
        f"текст:\n{text}\n"
        "Задача должна относиться ровно к этому узлу; материал выше — только фон. "
        "Не объединяй в одной задаче разные узлы.\n"
        + statement_note
    )


def _macro_task(
    mode: MacroMode, *, subject: str, topic: str, context: str, node: TaskNode | None = None
) -> str:
    return (
        "Ты — академический тьютор и составитель задач.\n"
        "Определи предметную область материала и выбери подходящий предметный subtype сам; "
        "пользователь не должен выбирать узкую классификацию. Не натягивай математический "
        "расчёт на гуманитарный текст и не добавляй искусственные числа.\n\n"
        f"ВЫБРАННОЕ ДЕЙСТВИЕ: {mode.title}\n{mode.instruction}\n\n"
        "Адаптируй задачу к эпистемике области: используй формулы и доказательства в математике, "
        "аргументы в философии, источники и причинные связи в истории, наблюдаемые признаки и "
        "модели в психологии. Не ставь клинический диагноз.\n"
        "Создай одну микро-задачу: короткое условие и ясное требование самостоятельного действия. "
        "Условие оформи в два абзаца: сначала данные, затем требование отдельной строкой, "
        "начинающейся со слова «Требуется:». "
        "grounding выбирай как короткую дословную фразу, видимую в переданном материале; "
        "не превращай grounding в объяснение или пересказ. "
        "Обычно это 1–2 коротких абзаца, но не ломай формулу или необходимое ограничение ради "
        "ровно двух физических строк. Не показывай решение, метод решения или мета-комментарии.\n"
        "Условие должно быть самодостаточным для попытки без угадывания невидимого объекта. "
        "Для таблицы, Excel, кода, схемы, графика или диапазона покажи минимальный необходимый "
        "фрагмент: заголовки, строки/значения, обозначения или схему. Один адрес вроде D2:D50 "
        "— это только координата, а не содержимое. Если порядок строк, направление поиска или "
        "взаимное расположение столбцов влияет на ответ, покажи эту структуру и объясни, что "
        "именно требуется определить. Не смешивай два разных направления XLOOKUP: «вправо/влево» "
        "означает положение return_array относительно lookup_array, а поиск сверху вниз/снизу вверх — "
        "порядок совпадений и отдельный search_mode. Для Excel/XLOOKUP обязательно включи в condition "
        "маленькую видимую таблицу с заголовками и минимум двумя строками, помеченную «Учебные данные», "
        "даже если формулу можно записать по одним адресам диапазонов. Для поиска вправо/влево покажи "
        "lookup_array и return_array по разные стороны или попроси переставить их роли. Если структуры нет в материале, "
        "создай явно обозначенный учебный пример с новыми данными, сохранив тот же навык; "
        "не выдавай учебные данные "
        "за источник и не ссылайся на невидимый объект. needs_context используй только если "
        "даже такой пример нельзя сформулировать.\n"
        "Если во фрагменте нет готовой формулы или алгоритма, НЕ возвращай needs_context только "
        "по этой причине: выбери честный fallback внутри выбранного действия. Для apply восстанови "
        "способ анализа или примени различие из текста к новому нейтральному кейсу; для distinguish "
        "сопоставь два тезиса/кейса из текста; для justify восстанови аргумент или причинную цепь; "
        "для boundary проверь оговорку, построй контрпример или мысленный эксперимент.\n"
        "Если задача требует конкретную таблицу, диапазон, код, схему, график или порядок данных, "
        "которых нет в переданном материале, сначала создай самодостаточный учебный пример, явно "
        "помеченный как новый; needs_context оставь для случаев, где такой пример невозможен.\n"
        + _node_block(node)
        + _context_block(context, topic)
        + "\nВерни только JSON:\n"
        '{"status":"ready|not_applicable|needs_context",'
        '"domain":"область", "subtype":"узкий тип",'
        '"condition":"условие и требование",'
        '"grounding":["1–3 дословные опоры из материала"],'
        '"required_operations":["действия ученика"],'
        '"difficulty":"quick|standard|advanced",'
        '"confidence":"high|medium|low",'
        '"payload":{}}\n'
        "Если задача вводит свои числа, включи их в condition и не приписывай источнику. "
        "Если узел про линейные системы и задача требует вердикта (совместна ли, определена ли, "
        "сколько решений), продублируй систему машинно: "
        '"payload":{"type":"linear_system","matrix":[[…]],"rhs":[…]} и заполни "require":'
        '"consistent" или "unique", если задача требует именно этого. '
        'Иначе верни "payload":{}.\n'
        "confidence — твоя уверенность, что условие корректно и разрешимо; при low задача не будет показана.\n"
    )


def build_task(
    key: str,
    *,
    subject: str = "",
    topic: str = "",
    level: str = DEFAULT_LEVEL,
    context: str = "",
    node: TaskNode | None = None,
) -> str:
    """Build either a legacy internal subtype or a user-facing macro prompt."""
    if key in MACRO_BY_KEY:
        return _header(subject, level) + _macro_task(
            MACRO_BY_KEY[key], subject=subject, topic=topic, context=context, node=node
        )
    task = BY_KEY[key]
    return (
        _header(subject, level)
        + _context_block(context, topic)
        + "\n"
        + _render(task.task, subject=subject, topic=topic, form=True).strip()
        + "\n"
    )


def build_attempt_check(
    task: dict[str, object],
    attempt: str,
    *,
    context: str = "",
) -> str:
    """Check one reader attempt without turning the first response into a lecture."""
    return (
        "Ты — аккуратный преподаватель математики. Проверь попытку ученика по условию.\n"
        "Сначала назови конкретно, что уже верно. Затем укажи первый существенный сбой "
        "или следующий шаг. Не переписывай полное решение, если это не нужно для "
        "исправления; не приписывай ошибку ученику, если условие или OCR неоднозначны.\n"
        "Верни обычный текст с разделами: СТАТУС, ЧТО ВЕРНО, СЛЕДУЮЩИЙ ШАГ, "
        "ЧТО ПРОВЕРИТЬ. Статус: correct, needs_retry или insufficient_attempt.\n\n"
        f"УСЛОВИЕ:\n{str(task.get('condition', '')).strip()}\n\n"
        f"ПОПЫТКА УЧЕНИКА:\n{attempt.strip()}\n\n"
        + (f"ИСТОЧНИК, ПО КОТОРОМУ СОЗДАНА ЗАДАЧА:\n{context}\n" if context else "")
    )


def build_solution(
    key: str,
    task_text: str,
    *,
    subject: str = "",
    topic: str = "",
    level: str = DEFAULT_LEVEL,
) -> str:
    """The solution prompt, built from the condition the reader already has."""
    if key in MACRO_BY_KEY:
        solution_instruction = (
            "Проверь попытку ученика по условию: назови, что верно, укажи первый "
            "существенный сбой и следующий шаг. Не выдавай полный разбор без необходимости."
        )
    else:
        task = BY_KEY[key]
        solution_instruction = _render(task.solution, subject=subject, topic=topic).strip()
    return (
        _header(subject, level)
        + "\nЗАДАЧА:\n"
        + task_text.strip()
        + "\n\n"
        + solution_instruction
        + "\n"
        + SOLUTION_RULES.replace(
            "Ученик уже попробовал решить сам — поэтому разбор должен отвечать на задачу, а не подменять её.",
            "Ученик уже попробовал решить сам или сразу запросил разбор; в обоих случаях разбор должен отвечать на задачу, а не подменять её.",
        )
        + "\n"
    )


def current_passage(*, primary_only: bool = False, min_clipboard_chars: int = 0) -> ReadingContext:
    """Snapshot a source without mistaking a short popup copy for study material."""
    try:
        from .desktop import run_probe, selection_probes
    except Exception:
        return ReadingContext("")
    for name, command in selection_probes(primary_only=primary_only):
        code, stdout, _stderr = run_probe(command)
        if code == 0 and stdout.strip():
            text = stdout.strip()
            if name.startswith("clipboard-") and len(text) < min_clipboard_chars:
                continue
            return ReadingContext(
                f'<CURRENT_PASSAGE source="{name}">\n{text}\n</CURRENT_PASSAGE>',
                buffer=[], event_ids=[]
            )
    return ReadingContext("")


def _submitted_condition(raw: str, source: str) -> str:
    observation.record_artifact("submitted_condition", {"text": raw, "source": source})
    return raw.strip()


def read_task_text(source: str | None) -> str:
    """Condition text from a file, from stdin, or from the clipboard."""
    if not source or source == "-":
        if not source:
            try:
                result = subprocess.run(
                    ["wl-paste", "--no-newline"],
                    capture_output=True,
                    text=True,
                    timeout=5,
                    check=False,
                )
                return _submitted_condition(result.stdout, "clipboard")
            except (OSError, subprocess.SubprocessError):
                return ""
        return _submitted_condition(sys.stdin.read(), "stdin")
    return _submitted_condition(Path(source).expanduser().read_text(encoding="utf-8"), source)


def copy_to_clipboard(text: str) -> bool:
    try:
        subprocess.run(["wl-copy"], input=text, text=True, timeout=5, check=True)
        return True
    except (OSError, subprocess.SubprocessError):
        return False


def _chain() -> EventChain:
    """One interaction — the hotkey press and everything it causes.

    The task flow lands in the same journal as the rest of the reading loop, so a
    generated task is traceable the way a seeded fragment or a Feynman check is.
    """
    return _TASK_CHAIN.get() or EventChain.from_env(default_log(), origin="task", pid=os.getpid())


def _popup(payload: dict[str, object], timeout: int = 3600) -> str:
    """Render one popup through the shared helper and return its stdout."""
    # The GTK layer is imported only here: the text paths work without a display.
    from .desktop import cursor_position, popup_helper_command

    command = popup_helper_command()
    if command is None:
        return ""
    position = cursor_position()
    if position:
        payload.setdefault("x", position[0])
        payload.setdefault("y", position[1])
    chain = _chain()
    instance = uuid.uuid4().hex
    with operation_scope(chain.context):
        artifact = observation.record_artifact("rendered_payload", payload,
            source_artifact_id=output_artifact.get())
        observation.record_presentation(artifact, window_instance_id=instance,
            event="spawn", payload={"window": payload.get("mode")})
    payload = dict(payload, observation={"artifact_id": artifact, "window_instance_id": instance})
    chain.emit("window_spawn", window=str(payload.get("mode", "task")),
        artifact_id=artifact, window_instance_id=instance)
    env = {**os.environ, "GDK_BACKEND": "x11", **chain.env(),
           "COGNITIVE_EVENT_DB": str(chain.log.path)}
    if not chain.log.enabled:
        env["COGNITIVE_EVENT_DISABLE"] = "1"
    root = str(Path(__file__).resolve().parent.parent)
    env["PYTHONPATH"] = os.pathsep.join([root, env.get("PYTHONPATH", "")])
    try:
        result = subprocess.run(
            command,
            input=json.dumps(payload, ensure_ascii=False),
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    return result.stdout.strip()


def _choose_type_menu(tasks: tuple[TaskType | MacroMode, ...], title: str) -> str | None:
    """Render a macro menu while keeping internal subtype ids out of the UI."""
    items = [
        {"label": f"{index}. {task.title}", "action": task.key}
        for index, task in enumerate(tasks, start=1)
    ]
    raw = _popup({
        "mode": "menu",
        "title": title,
        "focusable": True,
        "items": items,
    })
    if not raw:
        return None
    try:
        action = str(json.loads(raw).get("action", ""))
    except ValueError:
        return None
    return action if action in BY_KEY or action in MACRO_BY_KEY else None


def choose_type() -> str | None:
    """Let the reader choose a cognitive action; the model chooses the subtype."""
    return _choose_type_menu(MACRO_MODES, "Как поработать с материалом?")


def show_text(text: str, title: str) -> None:
    """One result window through the same renderer every other mode uses.

    The reader gets the task itself, not a prompt describing it: making them paste
    a prompt into a model by hand is a step the other four modes do not have.
    Expanded geometry, because a task was measured to arrive as one dense block in
    a window too small to read it — the condition is the whole point of the call.
    """
    _popup({"mode": "text", "text": text, "title": title, "expanded": True})


def _latest_task(store: RecordStore, task_id: str = "") -> dict[str, object] | None:
    rows = store.load_tasks()
    if task_id:
        return next((row for row in rows if row.get("id") == task_id), None)
    return next((row for row in reversed(rows) if row.get("status") == "generated" and row.get("condition")), None)


def practice_task(task_id: str, context: str, client) -> int:
    """Explicit second step: attempt the saved condition, then request feedback."""
    store = RecordStore()
    task = _latest_task(store, task_id)
    if not task:
        show_text("Нет сохранённой задачи для практики. Сначала вызови генерацию.", "Практика")
        return 2
    condition = str(task.get("condition") or "").strip()
    attempt = _popup({
        "mode": "task_attempt",
        "prompt": "Задача:\n" + condition + "\n\nВведи свою попытку (Ctrl+Enter — отправить):",
    })
    if not attempt.strip():
        _chain().emit("action", window="task", detail="practice cancelled")
        return 0
    task_id_text = str(task.get("id") or "")
    programmatic = verify.check_attempt(task, attempt)
    if programmatic is not None:
        # The verdict is arithmetic, so no model call is needed, and the check
        # works even when the local bridge is down.
        _chain().emit("action", window="task", detail=f"practice {task_id_text[:8]} checked in code")
        feedback = programmatic
    else:
        prompt = build_attempt_check(task, attempt, context=str(task.get("context") or context))
        _chain().emit("prompt", window="task", detail=f"practice {task_id_text[:8]}")
        try:
            client.health()
            feedback = client.complete([{"role": "user", "content": prompt}], max_tokens=900)
        except Web2APIError as exc:
            _chain().emit("error", window="task", detail=str(exc))
            show_text(f"Ошибка проверки попытки:\n\n{exc}", "Практика")
            return 1
    # Keep the original condition and context; only the learner's attempt and the
    # feedback change. This makes the practice episode auditable in the ledger.
    updated = dict(task)
    updated["attempt"] = attempt
    updated["solution"] = feedback
    updated["status"] = "attempt_checked"
    try:
        store.save_task(
            task_id=str(updated["id"]),
            session_id=str(updated["session_id"]) if updated.get("session_id") else None,
            task_type=str(updated.get("task_type") or ""),
            task_subtype=str(updated.get("task_subtype") or ""),
            level=str(updated.get("level") or ""),
            context=str(updated.get("context") or ""), condition=condition,
            solution=feedback, attempt=attempt, status="attempt_checked",
            prompt_hash=str(updated.get("prompt_hash") or ""),
            model=str(updated.get("model") or getattr(client, "model", "")),
            raw_response=str(updated.get("raw_response") or ""),
            grounding_json=str(updated.get("grounding_json") or "[]"),
            operations_json=str(updated.get("operations_json") or "[]"),
            validation_error=str(updated.get("validation_error") or ""),
            attempts=int(str(updated.get("attempts") or 1)),
            latency_ms=int(str(updated.get("latency_ms") or 0)),
            domain=str(updated.get("domain") or ""),
            node_id=str(updated.get("node_id") or ""),
            node_type=str(updated.get("node_type") or ""),
            verification_status=str(updated.get("verification_status") or ""),
            verification_note=str(updated.get("verification_note") or ""),
            payload_json=str(updated.get("payload_json") or "{}"),
            created_utc=str(updated.get("created_utc") or ""),
        )
    except Exception as exc:  # feedback remains visible even if the ledger fails
        _chain().emit("error", window="task", detail=f"ledger: {exc}")
    observation.record_annotation("task_practice", {
        "task_id": task.get("id"), "attempt_chars": len(attempt), "feedback_chars": len(feedback),
        "checked_in_code": programmatic is not None,
    })
    _chain().emit("result", window="task", detail=f"practice checked {task_id_text[:8]}")
    show_text(feedback, "Практика: обратная связь")
    return 0


def revisit_task(source_id: str, *, store: RecordStore, client=None, show_popup: bool = False) -> int:
    """One explicit archived source -> one new application condition."""
    sources = store.revisit_sources()
    if not source_id:
        for row in sources:
            label = row["hypothesis"] if row["kind"] == "hypothesis" else row["source_text"]
            preview = " ".join(str(label).split())[:100]
            print(f"{row['id']}  {row['created_utc']}  {row['title']}  {preview}")
        if not sources:
            print("Нет архивных фрагментов с источником.")
        return 0
    row = next((item for item in sources if item["id"] == source_id), None)
    if row is None:
        print("Источник не найден среди закрытых сессий.", file=sys.stderr)
        return 2
    source = str(row["source_text"])
    context = f"<ARCHIVED_SOURCE fragment_id={row['fragment_id']!r}>\n{source}\n</ARCHIVED_SOURCE>"
    if row["kind"] == "hypothesis":
        context += (
            "\n<READER_HYPOTHESIS (не доказательство, не оценка понимания)>\n"
            + str(row["hypothesis"]) + "\n</READER_HYPOTHESIS>"
        )
    if len(context) > MAX_CONTEXT_CHARS:
        print("Архивный источник слишком длинный для одной задачи; снимок не обрезан.", file=sys.stderr)
        return 2
    client = client or GeminiWeb2API()
    prompt = (
        "Это добровольное возвращение к старому материалу. Создай ОДНУ новую задачу "
        "на применение; гипотеза — вопрос читателя, не факт и не опора. "
        "Цитаты grounding бери только из ARCHIVED_SOURCE, не из гипотезы.\n"
        + build_task("apply", context=context)
    )
    _TASK_SOURCE.set(observation.record_artifact("task_source", {
        "revisit_id": source_id, "assembled_context": context,
        "fragment_ids": [row["fragment_id"]],
    }))
    try:
        client.health()
        generated = generate_task(client, "apply", prompt, context)
    except (Web2APIError, TaskValidationError) as exc:
        print(f"Ошибка генерации: {exc}", file=sys.stderr)
        return 1
    if generated.status != "ready":
        print(f"Задача не создана: {generated.status}", file=sys.stderr)
        return 1
    if not generated.grounding or not all(
        _search_text(quote) in _search_text(source) for quote in generated.grounding
    ):
        print("Задача не сохранена: дословная опора не найдена в источнике.", file=sys.stderr)
        return 1
    task_id = uuid.uuid4().hex
    try:
        store.save_task(
            task_id=task_id, session_id=None, task_type="apply", level=DEFAULT_LEVEL,
            context=context, condition=generated.condition, status="generated",
            prompt_hash=prompt_hash(prompt), model=client.model,
            raw_response=generated.raw_response,
            grounding_json=json.dumps(generated.grounding, ensure_ascii=False),
            operations_json=json.dumps(generated.required_operations, ensure_ascii=False),
            attempts=generated.attempts, latency_ms=generated.latency_ms,
            domain=generated.domain, task_subtype=generated.subtype,
            node_id=generated.node_id, node_type=generated.node_type,
            verification_status=generated.verification_status,
            verification_note=generated.verification_note,
            payload_json=json.dumps(generated.payload, ensure_ascii=False),
        )
    except Exception as exc:
        print(f"Задача не сохранена: {exc}", file=sys.stderr)
        return 1
    print(f"Задача {task_id}:\n{generated.condition}")
    for mark in generated.marks:
        print(f"⚑ {mark}")
    if show_popup:
        show_text(generated.condition + "".join(f"\n\n⚑ {mark}" for mark in generated.marks),
                  "Задача: возвращение к архиву")
    return 0


def choose_revisit_source(sources: list[dict[str, object]]) -> str | None:
    """Select an exact archived ID using the helper's nine keyboard shortcuts."""
    start = 0
    previous: list[int] = []
    while True:
        page_size = 7 if previous else 8
        page = sources[start:start + page_size]
        items = []
        if previous:
            items.append({"label": "1. ← Назад", "action": "__previous__"})
        for row in page:
            preview = row["hypothesis"] if row["kind"] == "hypothesis" else row["source_text"]
            preview = " ".join(str(preview).split())[:90]
            label = f"{row['id']}  {str(row['created_utc'])[:10]}  {row['title']}  {preview}"
            items.append({"label": f"{len(items) + 1}. {label}", "action": row["id"]})
        next_start = start + len(page)
        has_next = next_start < len(sources)
        if has_next:
            items.append({"label": f"{len(items) + 1}. Далее →", "action": "__next__"})
        raw = _popup({"mode": "menu", "title": "Вернуться к архивной теме",
                      "focusable": True, "items": items})
        try:
            action = json.loads(raw).get("action") if raw else None
        except (ValueError, TypeError, AttributeError):
            return None
        if action == "__next__" and has_next:
            previous.append(start)
            start = next_start
        elif action == "__previous__" and previous:
            start = previous.pop()
        elif action in {row["id"] for row in page}:
            return action
        else:
            return None


def main(argv: list[str] | None = None) -> int:
    chain = EventChain.from_env(default_log(), origin="task", pid=os.getpid())
    token = _TASK_CHAIN.set(chain)
    source_token = _TASK_SOURCE.set(None)
    artifact_token = output_artifact.set(None)
    try:
        with operation_scope(chain.context):
            return _main(argv)
    finally:
        output_artifact.reset(artifact_token)
        _TASK_SOURCE.reset(source_token)
        _TASK_CHAIN.reset(token)


def _main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Generate practice tasks from the reading session")
    parser.add_argument(
        "--type", choices=sorted({*BY_KEY, *MACRO_BY_KEY}),
        help="macro mode or legacy internal subtype",
    )
    parser.add_argument(
        "--menu",
        action="store_true",
        help="pick the type in a popup, then generate the task and show it",
    )
    parser.add_argument("--list", action="store_true", help="show the types and exit")
    parser.add_argument("--revisit", nargs="?", const="", metavar="SOURCE_ID",
                        help="list archived sources; pass an exact ID to generate one application task")
    parser.add_argument("--revisit-menu", action="store_true",
                        help="choose an archived source by keyboard and show its new task")
    parser.add_argument("--subject", default="", help="optional; the material supplies it otherwise")
    parser.add_argument("--topic", default="", help="optional; the material supplies it otherwise")
    parser.add_argument("--level", choices=LEVELS, default=DEFAULT_LEVEL)
    parser.add_argument("--no-context", action="store_true", help="do not inject the reading session")
    parser.add_argument("--show-context", action="store_true", help="print the context and exit")
    parser.add_argument(
        "--nodes",
        action="store_true",
        help="print the parsed study nodes of the material and exit",
    )
    parser.add_argument("--last-fragments", type=int, default=DEFAULT_FRAGMENTS)
    parser.add_argument("--last-events", type=int, default=DEFAULT_EVENTS)
    parser.add_argument(
        "--solution",
        nargs="?",
        const="",
        metavar="FILE",
        help="build the solution prompt; condition from FILE, '-' for stdin, or the clipboard",
    )
    parser.add_argument(
        "--practice",
        nargs="?",
        const="",
        metavar="TASK_ID",
        help="show the latest generated task (or TASK_ID), collect an attempt and check it",
    )
    parser.add_argument("--copy", action="store_true", help="copy the result to the clipboard")
    args = parser.parse_args(argv)

    if args.list:
        print("Пользовательские режимы:")
        for mode in MACRO_MODES:
            print(f"{mode.key}. {mode.title} — {mode.instruction}")
        print("\nLegacy subtypes (для совместимости):")
        for task in TYPES:
            print(f"{task.key}. {task.title} — {task.tag}")
        return 0

    if args.revisit is not None or args.revisit_menu:
        if (args.revisit is not None and args.revisit_menu or args.menu
                or args.practice is not None or args.solution is not None
                or args.no_context or args.topic or args.subject or args.type not in (None, "apply")):
            parser.error("--revisit/--revisit-menu cannot be combined with other task modes")
        store = RecordStore()
        if args.revisit_menu:
            sources = store.revisit_sources()
            if not sources:
                show_text("Нет архивных фрагментов с источником.", "Архив")
                return 0
            source_id = choose_revisit_source(sources)
            if source_id is None:
                return 0
            return revisit_task(source_id, store=store, show_popup=True)
        return revisit_task(args.revisit if args.revisit is not None else "", store=store)

    context = ""
    # For the interactive menu, snapshot the source before opening any popup.
    # A current passage has priority over older saved fragments. Other CLI modes
    # intentionally do not read the clipboard implicitly.
    if args.menu and not args.no_context and not args.topic:
        # Primary selection is the strongest current source. A sufficiently long
        # clipboard is the next explicit source for "copy, then Alt+T"; short
        # labels such as "Задача" copied from a result popup are ignored.
        context = current_passage(primary_only=True)
        if not context:
            context = current_passage(min_clipboard_chars=80)
    if not context and not args.no_context:
        context = reading_context(
            fragments=args.last_fragments,
            events=args.last_events,
        )
    if args.show_context:
        print(context or "(нет данных для контекста)")
        return 0
    if args.nodes:
        parsed = parse_nodes(strip_wrapper(str(context)))
        print(render_nodes(parsed) if parsed else "(нет узлов: нет материала)")
        return 0

    _TASK_SOURCE.set(observation.record_artifact("task_source", {
        "assembled_context": str(context), "arguments": vars(args),
        "buffer_session_id": getattr(context, "session_id", None),
        "technical_buffer": getattr(context, "buffer", []),
        "event_ids": getattr(context, "event_ids", []),
        "fragment_ids": [f["id"] for f in getattr(context, "buffer", [])],
    }))
    if args.practice is not None:
        # The bridge is checked inside practice_task only if a model call is
        # actually needed: a machine checkable attempt is judged in code.
        return practice_task(args.practice, context, GeminiWeb2API())

    if args.menu:
        chain = _chain()
        key = choose_type()
        observation.record_annotation("task_choice", {"task_type": key,
            "outcome": "cancelled" if key is None else "submitted",
            "source_artifact_id": _TASK_SOURCE.get()})
        if key is None:
            return 0
        title = MACRO_BY_KEY[key].title if key in MACRO_BY_KEY else BY_KEY[key].title
        chain.emit("hotkey", window="task", detail=title)
        if not context and not args.topic:
            chain.emit("action", window="task", detail="no material")
            show_text(
                "Нет материала для задачи. Выдели или скопируй фрагмент и повтори Alt+T, "
                "либо сначала сохрани его через Alt+W.",
                "Задача",
            )
            return 0
        client = GeminiWeb2API()
        try:
            health = client.health()
            chain.emit(
                "health",
                window="task",
                detail=f"ok {health.get('latency_ms', '?')}ms version={health.get('version', '?')}",
            )
        except Web2APIError as exc:
            chain.emit("error", window="task", detail=str(exc))
            show_text(f"Локальный bridge недоступен:\n\n{exc}", "Задача")
            return 1
        nodes = parse_nodes(strip_wrapper(str(context))) if context else []
        node = select_node(nodes, key) if key in MODE_KINDS else None
        chain.emit(
            "action",
            window="task",
            detail=(f"node={node.kind}:{node.id}" if node else f"nodes={len(nodes)} no-node"),
        )
        prompt = build_task(
            key, subject=args.subject, topic=args.topic, level=args.level, context=context, node=node
        )
        chain.emit("prompt", window="task", detail=f"{key} {prompt_hash(prompt)} model={client.model}")
        generated: GeneratedTask | None = None
        try:
            generated = generate_task(client, key, prompt, context or args.topic, node=node, nodes=nodes)
            condition = generated.condition
        except Exception as exc:  # noqa: BLE001 - the reader needs the reason, not a window
            chain.emit("error", window="task", detail=str(exc))
            if isinstance(exc, TaskValidationError):
                generated = exc.result
                task_store = RecordStore()
                task_id = uuid.uuid4().hex
                observation.record_annotation("ledger_link", {"kind": "task", "task_id": task_id,
                    "artifact_id": generated.artifact_id})
                try:
                    task_store.save_task(
                        task_id=task_id,
                        session_id=active_session_id(task_store),
                        task_type=key,
                        task_subtype=generated.subtype,
                        level=args.level,
                        context=context,
                        condition="",
                        status="invalid",
                        prompt_hash=prompt_hash(prompt),
                        model=client.model,
                        raw_response=generated.raw_response,
                        validation_error="; ".join(generated.validation_errors),
                        attempts=generated.attempts,
                        latency_ms=generated.latency_ms,
                        domain=generated.domain,
                        node_id=generated.node_id,
                        node_type=generated.node_type,
                        verification_status=generated.verification_status,
                        verification_note=generated.verification_note,
                        payload_json=json.dumps(generated.payload, ensure_ascii=False),
                    )
                except Exception as ledger_exc:  # noqa: BLE001
                    chain.emit("error", window="task", detail=f"ledger: {ledger_exc}")
            show_text(f"Ошибка генерации:\n\n{exc}", "Задача")
            return 1
        assert generated is not None
        if generated.status != "ready":
            message = (
                "Для этого типа задачи в выбранном материале недостаточно опоры."
                if generated.status == "needs_context"
                else "Этот тип задачи не подходит к выбранному материалу."
            )
            condition = ""
        else:
            message = condition
        # A formal defect of the task is a hint, not a reason to withhold it: the
        # marks travel with the condition and the reader decides what to do.
        if generated.marks:
            message = "\n".join("⚑ " + mark for mark in generated.marks) + "\n\n" + message
        chain.emit(
            "result",
            window="task",
            detail=f"{generated.status}, {generated.attempts} attempt(s), {generated.latency_ms}ms"
                   + (f", marks: {'; '.join(generated.marks)}" if generated.marks else ""),
        )
        task_id = uuid.uuid4().hex
        observation.record_annotation("ledger_link", {"kind": "task", "task_id": task_id,
            "artifact_id": generated.artifact_id})
        task_store = RecordStore()
        try:
            task_store.save_task(
                task_id=task_id,
                session_id=active_session_id(task_store),
                task_type=key,
                task_subtype=generated.subtype,
                level=args.level,
                context=context,
                condition=condition,
                status="generated" if generated.status == "ready" else generated.status,
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
            chain.emit("result", window="task", detail=f"saved {task_id[:8]}")
        except Exception as exc:  # noqa: BLE001 - generation remains visible if ledger fails
            chain.emit("error", window="task", detail=f"ledger: {exc}")
        show_text(message, title)
        # The condition is the end of this invocation. The reader decides when
        # to request a solution through the explicit --solution path, so task
        # generation never turns into an unsolicited next step.
        return 0

    if not args.type:
        parser.error("укажите --type 1..10, либо --list, --menu, --show-context")

    if args.solution is not None:
        task_text = read_task_text(args.solution)
        observation.record_artifact("solution_prompt_input", {"task_text": task_text},
            source_artifact_id=_TASK_SOURCE.get())
        if not task_text:
            print("не удалось прочитать условие (файл, stdin или клипборд пусты)", file=sys.stderr)
            return 2
        prompt = build_solution(
            args.type, task_text, subject=args.subject, topic=args.topic, level=args.level
        )
    else:
        if not context and not args.topic:
            parser.error("нет ни материала сессии, ни --topic: нечего тренировать")
        prompt = build_task(
            args.type,
            subject=args.subject,
            topic=args.topic,
            level=args.level,
            context=context,
        )

    artifact = observation.record_artifact("prompt_export", {
        "prompt": prompt, "kind": "solution" if args.solution is not None else "task",
    }, source_artifact_id=_TASK_SOURCE.get())
    if args.copy and copy_to_clipboard(prompt):
        observation.record_annotation("prompt_exported", {"artifact_id": artifact, "destination": "clipboard"})
        print(f"скопировано в буфер ({len(prompt)} символов)", file=sys.stderr)
    else:
        print(prompt)
        observation.record_annotation("prompt_exported", {"artifact_id": artifact, "destination": "stdout"})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
