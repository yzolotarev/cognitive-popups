import json
from pathlib import Path

from cognitive_popups.event_log import EventLog
from cognitive_popups.models import CognitiveSession, PredictionCheck
from cognitive_popups.nodes import TaskNode
from cognitive_popups.records import RecordStore
from cognitive_popups.tasks import (
    QUICK_KEYS,
    TYPES,
    TaskValidationError,
    active_session_id,
    build_attempt_check,
    build_solution,
    build_task,
    generate_task,
    parse_generated_task,
    read_task_text,
    reading_context,
    revisit_task,
    _main,
)

CUES = [
    {"simple": "память", "term": "memory", "meaning": "удержание"},
    {"simple": "группы", "term": "chunking", "meaning": "объединение"},
    {"simple": "предел", "term": "limit", "meaning": "ограничение"},
    {"simple": "заметка", "term": "note", "meaning": "внешняя опора"},
]


def seeded_store(path: Path, *, session_id: str, text: str, created_utc: str | None = None,
                 closed: bool = False) -> RecordStore:
    store = RecordStore(path)
    session = CognitiveSession(id=session_id)
    store.open_session(session_id, created_utc=created_utc)
    fragment = session.add_fragment(text, [cue["term"] for cue in CUES], cue_details=CUES)
    store.save_fragment(session, fragment)
    if closed:
        store.close_session(session_id, reason="shutdown")
    return store


def test_reading_context_excludes_task_generation_telemetry(tmp_path):
    event_path = tmp_path / "events.sqlite3"
    log = EventLog(event_path)
    log.log("click", session_id="task-session", window="menu", detail="4. Применить")
    log.log("hotkey", session_id="task-session", window="task", detail="Применить")
    log.log("result", session_id="reader-session", window="feynman", detail="reader action")
    context = reading_context(
        records_path=tmp_path / "records.sqlite3",
        events_path=event_path,
    )
    assert "reader action" not in context
    assert "Применить" not in context


def test_reading_context_injects_material_and_actions(tmp_path):
    store = seeded_store(
        tmp_path / "records.sqlite3",
        session_id="a" * 32,
        text="Working memory holds about four chunks.",
    )
    assert store  # the store wrote the session
    EventLog(tmp_path / "events.sqlite3").log(
        "result", session_id="chain-1", window="clarify", detail="2 terms, ground=selection"
    )

    context = reading_context(
        records_path=tmp_path / "records.sqlite3",
        events_path=tmp_path / "events.sqlite3",
    )

    assert "READING_SESSION" in context
    assert "Working memory holds about four chunks." in context
    assert "memory, chunking, limit, note" in context
    assert "ЧТО ДЕЛАЛ" not in context
    assert "clarify" not in context


def test_events_only_do_not_create_task_context(tmp_path):
    events = EventLog(tmp_path / "events.sqlite3")
    events.log("copy", session_id="s1", window="text", detail="Обосновать")
    events.log("copy", session_id="s1", window="text", detail="Задача")
    assert reading_context(
        records_path=tmp_path / "records.sqlite3",
        events_path=tmp_path / "events.sqlite3",
    ) == ""


def test_reading_context_is_empty_when_nothing_was_recorded(tmp_path):
    assert reading_context(
        records_path=tmp_path / "records.sqlite3",
        events_path=tmp_path / "events.sqlite3",
    ) == ""


def test_active_session_prefers_the_open_one(tmp_path):
    path = tmp_path / "records.sqlite3"
    seeded_store(path, session_id="a" * 32, text="first", closed=True)
    seeded_store(path, session_id="b" * 32, text="second")

    assert active_session_id(RecordStore(path)) == "b" * 32


def test_active_session_does_not_reuse_a_closed_session(tmp_path):
    # A closed session is not silently substituted for what the reader is doing now.
    path = tmp_path / "records.sqlite3"
    seeded_store(path, session_id="a" * 32, text="first", closed=True)
    RecordStore(path).open_session("c" * 32)  # open, but holds no fragments

    assert active_session_id(RecordStore(path)) is None


def test_every_type_keeps_the_solution_out_of_the_task():
    # The defect this module exists to fix: the condition must not carry the answer.
    for task in TYPES:
        built = build_task(task.key, subject="химия", topic="замещение", level="продвинутый")
        assert "condition содержит только условие" in built
        assert task.solution.strip() not in built
        for name in ("form", "by_topic", "from_topic", "topic_words", "subject_words"):
            assert "{" + name + "}" not in built


def test_task_prompt_requires_visible_structure_for_table_like_tasks():
    built = build_task(
        "apply",
        subject="Excel",
        context="Текст описывает поиск цены по артикулу в диапазонах D2:D50 и G2:G50.",
    )

    assert "самодостаточным" in built
    assert "Один адрес вроде D2:D50" in built
    assert "направление поиска" in built
    assert "Не смешивай два разных направления XLOOKUP" in built
    assert "return_array относительно lookup_array" in built
    assert "невидимый объект" in built
    assert "явно обозначенный учебный пример" in built


def test_spreadsheet_ranges_are_not_blocked_by_validation():
    task, errors = parse_generated_task(
        task_json(
            condition=(
                "В ячейке B5 нужно найти значение в диапазоне D2:D20 "
                "и вернуть его из F2:F20.\n\n"
                "Требуется: составить формулу XLOOKUP."
            )
        ),
        "apply",
        "Материал про Excel и функцию XLOOKUP.",
    )

    assert task.status == "ready"
    assert not errors


def test_visible_spreadsheet_fixture_is_allowed():
    task, errors = parse_generated_task(
        task_json(
            condition=(
                "Учебная таблица:\n"
                "| D | F |\n| A101 | 2500 |\n| A102 | 1200 |\n\n"
                "Требуется: составить формулу XLOOKUP."
            )
        ),
        "apply",
        "Материал про Excel и функцию XLOOKUP.",
    )

    assert task.status == "ready"
    assert not any("не показывает структуру данных" in error for error in errors)


def test_a_quoted_backslash_in_the_material_does_not_break_generation():
    # Independently authored material includes a literal backslash before a comma.
    # Copying it unescaped into fenced JSON must not lose the quoted character.
    raw = (
        '```json\n{"status":"ready","domain":"Технология","subtype":"Маркировка деталей",'
        '"condition":"На столе лежат детали с метками q и z.'
        '\\n\\nТребуется: различить круглые и квадратные детали по правилу маркировки.",'
        '"grounding":["Метки q\\, z* обозначают круглую и квадратную детали соответственно."],'
        '"required_operations":["Различить круглые и квадратные детали"],'
        '"difficulty":"standard","confidence":"high","payload":{}}\n```'
    )
    material = r"Метки q\, z* обозначают круглую и квадратную детали соответственно."

    task, errors = parse_generated_task(raw, "distinguish", material)

    assert not errors
    assert task.status == "ready"
    assert task.grounding == (material,)


def test_quick_task_types_are_available_before_advanced_types():
    assert QUICK_KEYS == ("7", "8", "9", "10")
    assert [task.key for task in TYPES[:4]] == ["7", "8", "9", "10"]
    assert "короткую задачу" in build_task("7", context="ЧТО ЧИТАЛ: группа Галуа")
    assert "один промежуточный шаг" in build_task("10", context="ЧТО ЧИТАЛ: подстановка")


def test_macro_prompt_lets_the_model_choose_a_domain_subtype():
    built = build_task("distinguish", context="Текст о различии двух аргументов")
    assert "ВЫБРАННОЕ ДЕЙСТВИЕ: Различить" in built
    assert '"subtype"' in built
    assert "Не натягивай математический расчёт" in built


def test_macro_task_response_keeps_domain_and_subtype():
    raw = task_json(grounding=["перестановку корней"])
    data = __import__("json").loads(raw)
    data.update({"domain": "алгебра", "subtype": "example_non_example"})
    result, errors = parse_generated_task(__import__("json").dumps(data, ensure_ascii=False), "distinguish", "Текст объясняет перестановку корней.")
    assert not errors
    assert result.domain == "алгебра"
    assert result.subtype == "example_non_example"


def test_build_task_without_a_topic_leans_on_the_material():
    # A hotkey has no form to fill in: the material the reader is on is the topic.
    built = build_task(
        "5",
        subject="",
        topic="",
        level="продвинутый",
        context="ЧТО ЧИТАЛ: нуклеофильное замещение",
    )

    assert "по материалу выше" in built
    assert "Материал выше и есть тема задачи" in built
    assert "«»" not in built
    assert "разделов предмета" in built


def test_build_task_names_the_topic_only_when_it_is_given():
    built = build_task("6", subject="химия", topic="замещение", level="базовый")

    assert "по теме «замещение»" in built
    assert "внутри предмета «химия»" in built


def test_build_solution_renders_the_topic_placeholders_too():
    built = build_solution(
        "6",
        "Условие.",
        subject="",
        topic="",
        level="продвинутый",
    )

    assert "из материала выше" in built
    assert "{from_topic}" not in built


def test_build_task_injects_the_context_and_lets_topic_refine_it():
    built = build_task(
        "4",
        subject="химия",
        topic="замещение",
        level="продвинутый",
        context="ЧТО ЧИТАЛ: нуклеофильное замещение у насыщенного углерода",
    )

    assert "ЧТО ЧИТАЛ: нуклеофильное замещение" in built
    assert "уточнение, а не замена" in built
    assert "«замещение»" in built


def test_build_task_without_context_says_nothing_about_reading():
    built = build_task("4", subject="химия", topic="замещение", level="базовый")

    assert "ученик читает сейчас" not in built
    assert "базовый" in built


def test_build_attempt_check_keeps_condition_and_attempt_together():
    built = build_attempt_check(
        {"condition": "Решите x + 2 = 5"},
        "x = 3",
        context="Материал про линейные уравнения",
    )
    assert "Решите x + 2 = 5" in built
    assert "x = 3" in built
    assert "СТАТУС" in built
    assert "полное решение" in built


def test_build_solution_carries_the_condition_and_the_rules():
    built = build_solution(
        "3",
        "Докажите, что предел не зависит от единиц измерения.",
        subject="физика",
        topic="измерения",
        level="продвинутый",
    )

    assert "Докажите, что предел не зависит" in built
    assert "Ответь на каждый шаг по порядку" in built
    assert "уже попробовал решить сам" in built


def test_read_task_text_from_a_file(tmp_path):
    path = tmp_path / "task.md"
    path.write_text("Условие из файла.\n", encoding="utf-8")

    assert read_task_text(str(path)) == "Условие из файла."


class TaskClient:
    def __init__(self, *replies):
        self.replies = iter(replies)
        self.calls = []

    def complete(self, messages, **kwargs):
        self.calls.append(messages)
        return next(self.replies)


def task_json(condition="Сравните корень и перестановку корней.", grounding=None,
              status="ready", difficulty="quick"):
    return __import__("json").dumps({
        "status": status,
        "condition": condition,
        "grounding": grounding or ["перестановку корней"],
        "required_operations": ["сравнить"],
        "difficulty": difficulty,
    }, ensure_ascii=False)


def test_macro_task_may_decline_and_the_refusal_is_only_a_mark():
    task, errors = parse_generated_task(
        task_json(condition="", status="needs_context"),
        "distinguish",
        "Текст о причинах распада группы и публичном мифе.",
    )
    assert not errors
    assert any("отказался" in mark for mark in task.marks)


def test_grounding_from_nowhere_is_a_mark_not_a_refusal():
    task, errors = parse_generated_task(
        task_json(grounding=["выдуманное поле"]),
        "8",
        "Текст объясняет перестановку корней.",
    )
    assert not errors
    assert any("не найдена" in mark for mark in task.marks)


def test_labelled_solution_leak_is_a_mark_not_a_refusal():
    task, errors = parse_generated_task(
        task_json(condition="Решение: поменяйте a и b местами."),
        "8",
        "Текст объясняет перестановку корней.",
    )
    assert not errors
    assert any("решение" in mark for mark in task.marks)


def test_a_long_grounding_is_accepted_and_only_marked():
    long_grounding = " ".join(["слово"] * 13)
    task, errors = parse_generated_task(
        task_json(grounding=[long_grounding]),
        "8",
        "Текст объясняет слово и его значение.",
    )
    assert not errors
    assert any("длинная опора" in mark for mark in task.marks)


def test_macro_task_can_paraphrase_its_grounding():
    raw = task_json(
        condition="Сравните объяснения разрыва музыкальной группы.",
        grounding=["распад The Beatles"],
    )
    _, errors = parse_generated_task(
        raw,
        "distinguish",
        "Текст о распаде The Beatles и нескольких причинах разрыва группы.",
    )
    assert not any("condition" in error for error in errors)


def test_a_condition_unrelated_to_the_material_is_a_mark():
    task, errors = parse_generated_task(
        task_json(condition="Посчитайте скорость автомобиля."),
        "8",
        "Текст объясняет перестановку корней.",
    )
    assert not errors
    assert any("не опирается" in mark for mark in task.marks)


def test_generate_task_repairs_one_invalid_response():
    client = TaskClient(
        "не json",
        task_json(),
    )
    result = generate_task(client, "8", "prompt", "Материал про перестановку корней.")
    assert result.status == "ready"
    assert result.attempts == 2
    assert len(client.calls) == 2


def test_reading_context_preserves_formula_layout(tmp_path):
    path = tmp_path / "records.sqlite3"
    seeded_store(path, session_id="d" * 32, text="x³ + ax²\n  + bx + c = 0")
    context = reading_context(records_path=path, events_path=tmp_path / "events.sqlite3")
    assert "x³ + ax²\n  + bx + c = 0" in context


MACRO_CONTEXT = "Текст объясняет однородную систему и её свойства."


def macro_json(condition="Различите однородную и неоднородную систему.", **extra):
    data = {
        "status": "ready",
        "condition": condition,
        "grounding": ["однородную систему"],
        "required_operations": ["различить"],
        "difficulty": "quick",
        "domain": "алгебра",
        "subtype": "example_non_example",
        "confidence": "high",
    }
    data.update(extra)
    return json.dumps(data, ensure_ascii=False)


def definition_node() -> TaskNode:
    return TaskNode(
        id="n1", kind="definition", title="Терминология",
        text="Система называется однородной, если все свободные члены равны нулю.",
        start=0, end=10, deps=(), ops=("distinguish",),
    )


def test_macro_prompt_pins_the_selected_node():
    node = TaskNode(
        id="n1", kind="statement", title="Теорема 1",
        text="Теорема 1. Две системы эквивалентны.",
        start=0, end=10, deps=(), ops=("prove",),
    )
    built = build_task("justify", context=MACRO_CONTEXT, node=node)
    assert "УЗЕЛ МАТЕРИАЛА" in built
    assert "Теорема 1" in built
    assert "Не объединяй в одной задаче разные узлы" in built
    assert "пересказать готовое доказательство" in built
    assert '"payload"' in built


def test_generate_task_records_the_verified_node():
    node = definition_node()
    client = TaskClient(macro_json())
    result = generate_task(
        client, "distinguish", "prompt", MACRO_CONTEXT, node=node, nodes=[node]
    )
    assert result.node_id == "n1"
    assert result.node_type == "definition"
    assert result.verification_status == "passed"


def test_generate_task_verifies_a_checkable_payload():
    payload = {"type": "linear_system", "matrix": [[1, 2], [3, 4]], "rhs": [5, 6]}
    client = TaskClient(macro_json(payload=payload))
    result = generate_task(client, "distinguish", "prompt", MACRO_CONTEXT)
    assert result.payload == payload
    assert result.verification_status == "passed"
    assert "rank(A)=2" in result.verification_note


def test_generate_task_refuses_a_system_whose_required_consistency_fails():
    payload = {
        "type": "linear_system", "matrix": [[1, 1], [2, 2]], "rhs": [1, 3],
        "require": "consistent",
    }
    client = TaskClient(macro_json(payload=payload), macro_json(payload=payload))
    try:
        generate_task(client, "distinguish", "prompt", MACRO_CONTEXT)
    except TaskValidationError as exc:
        assert any("проверка до показа" in error for error in exc.result.validation_errors)
    else:
        raise AssertionError("an inconsistent system was shown")


def test_low_confidence_is_a_mark_not_a_refusal():
    client = TaskClient(macro_json(confidence="low"))
    result = generate_task(client, "distinguish", "prompt", MACRO_CONTEXT)
    assert result.status == "ready"
    assert len(client.calls) == 1
    assert any("не уверена" in mark for mark in result.marks)


def test_parsed_task_keeps_payload_and_confidence():
    raw = macro_json(confidence="medium", payload={"type": "poem"})
    result, errors = parse_generated_task(raw, "distinguish", MACRO_CONTEXT)
    assert not errors
    assert result.confidence == "medium"
    assert result.payload == {"type": "poem"}


def test_saved_task_round_trips_the_new_columns(tmp_path):
    store = RecordStore(tmp_path / "records.sqlite3")
    store.save_task(
        task_id="t1", session_id=None, task_type="justify", level="продвинутый",
        context=MACRO_CONTEXT, condition="Докажите теорему.", status="generated",
        domain="алгебра", node_id="n1", node_type="statement",
        verification_status="passed", verification_note="доказательство в материале",
        payload_json=json.dumps({"type": "linear_system"}),
    )
    row = store.load_tasks()[-1]
    assert row["node_id"] == "n1"
    assert row["node_type"] == "statement"
    assert row["domain"] == "алгебра"
    assert row["verification_status"] == "passed"
    assert json.loads(row["payload_json"])["type"] == "linear_system"


def test_revisit_lists_only_closed_source_and_linked_hypotheses(tmp_path, capsys):
    store = seeded_store(tmp_path / "records.sqlite3", session_id="old", text="Old source explains groups.",
                         created_utc="2026-08-01T10:00:00+00:00", closed=True)
    seeded_store(tmp_path / "records.sqlite3", session_id="live", text="Current source")
    fragment_id = store.load_fragments("old")[0].id
    session = CognitiveSession(id="old")
    store.save_prediction(session, PredictionCheck(
        buffer_fragment_ids=[fragment_id], hypothesis="Groups are sets", status="incorrect", id="linked"))
    store.save_prediction(session, PredictionCheck(
        buffer_fragment_ids=[], hypothesis="Orphan guess", status="incorrect", id="orphan"))
    rows = store.revisit_sources()
    assert {row["id"] for row in rows} == {f"f:{fragment_id}", f"h:linked:{fragment_id}"}
    assert all("status" not in row and "evidence" not in row for row in rows)
    assert revisit_task("", store=store) == 0
    listing = capsys.readouterr().out
    assert "Old source" in listing and "Groups are sets" in listing
    assert "incorrect" not in listing and "Current source" not in listing
    assert rows[0]["created_utc"] in listing
    assert "Untitled session" in listing


def test_revisit_generates_one_saved_task_from_exact_source(tmp_path, capsys):
    store = seeded_store(tmp_path / "records.sqlite3", session_id="old", text="Old source explains groups.", closed=True)
    fragment_id = store.load_fragments("old")[0].id

    class Client(TaskClient):
        model = "fake"

        def health(self):
            return {}

    client = Client(macro_json(condition="Даны две группы.\n\nТребуется: применить правило.",
                               grounding=["Old source explains groups."]))
    assert revisit_task(f"f:{fragment_id}", store=store, client=client) == 0
    row = store.load_tasks()[0]
    assert row["task_type"] == "apply"
    assert row["status"] == "generated"
    assert "Old source explains groups." in row["context"]
    assert "Требуется" in capsys.readouterr().out
    assert len(client.calls) == 1
    assert "Old source explains groups." in client.calls[0][0]["content"]
    assert "Current source" not in client.calls[0][0]["content"]


def test_revisit_hypothesis_is_a_question_not_a_source(tmp_path):
    store = seeded_store(tmp_path / "records.sqlite3", session_id="old", text="Old source explains groups.", closed=True)
    fragment_id = store.load_fragments("old")[0].id
    store.save_prediction(CognitiveSession(id="old"), PredictionCheck(
        buffer_fragment_ids=[fragment_id], hypothesis="Are groups always finite?",
        status="needs_retry", id="guess"))

    class Client(TaskClient):
        model = "fake"

        def health(self):
            return {}

    client = Client(macro_json(grounding=["Old source explains groups."]))
    assert revisit_task(f"h:guess:{fragment_id}", store=store, client=client) == 0
    prompt = client.calls[0][0]["content"]
    assert "Are groups always finite?" in prompt
    assert "гипотеза — вопрос читателя, не факт" in prompt
    assert "needs_retry" not in prompt
    assert "Are groups always finite?" in store.load_tasks()[0]["context"]


def test_revisit_refuses_unknown_oversized_or_ungrounded_source(tmp_path, capsys):
    store = seeded_store(tmp_path / "records.sqlite3", session_id="old", text="Old source explains groups.", closed=True)
    assert revisit_task("f:unknown", store=store) == 2
    fragment_id = store.load_fragments("old")[0].id

    class Client(TaskClient):
        model = "fake"

        def health(self):
            return {}

    client = Client(macro_json(grounding=["hallucinated quote"]))
    assert revisit_task(f"f:{fragment_id}", store=store, client=client) == 1
    assert store.load_tasks() == []
    long_store = seeded_store(tmp_path / "long.sqlite3", session_id="older", text="x" * 12001, closed=True)
    long_id = long_store.load_fragments("older")[0].id
    assert revisit_task(f"f:{long_id}", store=long_store) == 2
    assert "не обрезан" in capsys.readouterr().err


def test_revisit_cli_listing_does_not_read_current_context_or_create_client(tmp_path, monkeypatch, capsys):
    import cognitive_popups.tasks as tasks

    store = seeded_store(tmp_path / "records.sqlite3", session_id="old", text="Archived study source", closed=True)
    monkeypatch.setattr(tasks, "RecordStore", lambda: store)
    monkeypatch.setattr(tasks, "reading_context", lambda **kwargs: (_ for _ in ()).throw(AssertionError("current context")))
    monkeypatch.setattr(tasks, "GeminiWeb2API", lambda: (_ for _ in ()).throw(AssertionError("model")))
    assert _main(["--revisit"]) == 0
    assert "Archived study source" in capsys.readouterr().out


def test_revisit_menu_selects_exact_source_and_shows_saved_condition(tmp_path, monkeypatch, capsys):
    import cognitive_popups.tasks as tasks

    store = seeded_store(tmp_path / "records.sqlite3", session_id="old",
                         text="Old source explains groups.",
                         created_utc="2026-08-01T10:00:00+00:00", closed=True)
    source_id = store.revisit_sources()[0]["id"]
    monkeypatch.setattr(tasks, "RecordStore", lambda: store)
    monkeypatch.setattr(tasks, "reading_context", lambda **kw: (_ for _ in ()).throw(AssertionError("current context")))
    rendered = []
    def popup(payload):
        rendered.append(payload)
        return json.dumps({"action": source_id})
    monkeypatch.setattr(tasks, "_popup", popup)
    shown = []
    monkeypatch.setattr(tasks, "show_text", lambda text, title: shown.append((text, title)))

    class Client(TaskClient):
        model = "fake"
        def health(self):
            return {}

    client = Client(macro_json(condition="Даны группы.\n\nТребуется: применить правило.",
                               grounding=["Old source explains groups."]))
    monkeypatch.setattr(tasks, "GeminiWeb2API", lambda: client)
    assert _main(["--revisit-menu"]) == 0
    assert rendered[0]["mode"] == "menu" and rendered[0]["focusable"] is True
    assert rendered[0]["items"][0]["action"] == source_id
    assert source_id in rendered[0]["items"][0]["label"]
    assert store.revisit_sources()[0]["created_utc"][:10] in rendered[0]["items"][0]["label"]
    assert "Untitled session" in rendered[0]["items"][0]["label"]
    assert "Old source explains groups." in rendered[0]["items"][0]["label"]
    assert shown[0][0].startswith(store.load_tasks()[0]["condition"])
    assert "⚑" in shown[0][0]
    assert len(client.calls) == 1
    assert "Требуется" in capsys.readouterr().out


def test_revisit_menu_escape_or_unknown_action_never_generates(tmp_path, monkeypatch):
    import cognitive_popups.tasks as tasks

    store = seeded_store(tmp_path / "records.sqlite3", session_id="old",
                         text="Archived material", closed=True)
    monkeypatch.setattr(tasks, "RecordStore", lambda: store)
    monkeypatch.setattr(tasks, "GeminiWeb2API", lambda: (_ for _ in ()).throw(AssertionError("model")))
    monkeypatch.setattr(tasks, "reading_context", lambda **kw: (_ for _ in ()).throw(AssertionError("context")))
    for answer in ("", json.dumps({"action": "f:not-a-source"}), "not json"):
        monkeypatch.setattr(tasks, "_popup", lambda payload: answer)
        assert _main(["--revisit-menu"]) == 0
        assert store.load_tasks() == []


def test_revisit_menu_pages_remain_keyboard_selectable(tmp_path, monkeypatch):
    import cognitive_popups.tasks as tasks

    store = RecordStore(tmp_path / "records.sqlite3")
    for index in range(18):
        seeded_store(tmp_path / "records.sqlite3", session_id=f"old-{index}",
                     text=f"Archived material {index}", closed=True)
    sources = store.revisit_sources()
    menus = []
    actions = iter(("__next__", "__previous__", "__next__", "__next__", sources[-1]["id"]))
    def popup(payload):
        menus.append(payload)
        return json.dumps({"action": next(actions)})
    monkeypatch.setattr(tasks, "_popup", popup)
    assert tasks.choose_revisit_source(sources) == sources[-1]["id"]
    assert [len(menu["items"]) for menu in menus] == [9, 9, 9, 9, 4]
    assert all(menu["focusable"] and len(menu["items"]) <= 9 for menu in menus)
    assert menus[-1]["items"][-1]["action"] == sources[-1]["id"]


def test_revisit_menu_empty_archive_does_not_generate(tmp_path, monkeypatch):
    import cognitive_popups.tasks as tasks

    store = RecordStore(tmp_path / "empty.sqlite3")
    monkeypatch.setattr(tasks, "RecordStore", lambda: store)
    monkeypatch.setattr(tasks, "GeminiWeb2API", lambda: (_ for _ in ()).throw(AssertionError("model")))
    shown = []
    monkeypatch.setattr(tasks, "show_text", lambda text, title: shown.append(text))
    assert _main(["--revisit-menu"]) == 0
    assert "Нет архивных" in shown[0]


def test_an_existing_database_gains_the_new_task_columns(tmp_path):
    import sqlite3

    path = tmp_path / "records.sqlite3"
    connection = sqlite3.connect(path)
    connection.execute(
        "CREATE TABLE tasks ("
        " id TEXT PRIMARY KEY, session_id TEXT, task_type TEXT NOT NULL, level TEXT, context TEXT,"
        " condition TEXT NOT NULL, solution TEXT, attempt TEXT,"
        " status TEXT NOT NULL DEFAULT 'generated', created_utc TEXT NOT NULL, created_epoch REAL NOT NULL,"
        " prompt_hash TEXT, model TEXT)"
    )
    connection.commit()
    connection.close()

    store = RecordStore(path)
    store.save_task(
        task_id="t1", session_id=None, task_type="justify", level="базовый",
        context="c", condition="Условие.", node_id="n9", verification_status="passed",
    )
    row = store.load_tasks()[-1]
    assert row["node_id"] == "n9"
    assert row["verification_status"] == "passed"
