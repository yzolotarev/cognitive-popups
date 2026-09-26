from __future__ import annotations

from datetime import datetime, timedelta

from cognitive_popups import hud_state, records


def _store(tmp_path) -> records.RecordStore:
    return records.RecordStore(tmp_path / "records.sqlite3")


def _save_task(store, task_id: str, *, status: str = "generated",
               condition: str = "Различить два случая применения.", session_id: str = "s1") -> None:
    store.save_task(
        task_id=task_id,
        session_id=session_id,
        task_type="apply",
        level="базовый",
        context="материал сессии",
        condition=condition,
        status=status,
    )


def _save_preparation(store, preparation_id: str, *, context_key: str = "ctx-1",
                      status: str = "ready", task_id: str | None = None,
                      completed_stage: int | None = None) -> None:
    default_stage = {"ready": 4, "insufficient": 1, "failed": 0}.get(status, 2)
    store.save_preparation(
        preparation_id=preparation_id,
        context_key=context_key,
        status=status,
        completed_stage=default_stage if completed_stage is None else completed_stage,
        stage_label="Есть что попробовать" if status == "ready" else "Готовлю задачу",
        task_id=task_id,
    )


def _iso(days_ago: int = 0) -> str:
    moment = datetime.now().astimezone().replace(hour=12, minute=0, second=0, microsecond=0)
    return (moment - timedelta(days=days_ago)).isoformat()


def test_the_newest_unattempted_task_is_offered(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "old")
    _save_task(store, "new")

    offered = hud_state.latest_practice(store)

    assert offered is not None
    assert offered.task_id == "new"


def test_an_attempted_task_stops_being_offered(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "done", status="attempt_checked")

    assert hud_state.latest_practice(store) is None


def test_a_task_without_a_condition_is_not_offered(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "empty", condition="   ")

    assert hud_state.latest_practice(store) is None


def test_a_dismissed_task_is_not_replaced_by_the_one_before_it(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "older")
    _save_task(store, "dismissed")

    # «Убрать подсветку» must mean the panel goes quiet, not "show the next one".
    assert hud_state.latest_practice(store, dismissed=["dismissed"]) is None


def test_a_newer_task_lights_up_after_a_dismissal(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "dismissed")
    _save_task(store, "brand-new")

    offered = hud_state.latest_practice(store, dismissed=["dismissed"])

    assert offered is not None
    assert offered.task_id == "brand-new"


def test_the_offered_task_says_which_session_it_came_from(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "mine", session_id="s1")
    _save_task(store, "other", session_id="s2")

    now = hud_state.latest_practice(store, current_session_id="s2")
    then = hud_state.latest_practice(store, current_session_id="s1")

    assert now.from_current_session is True
    assert then.from_current_session is False


def test_a_dismissal_survives_a_reload(tmp_path):
    path = tmp_path / "hud.json"
    hud_state.dismiss("task-1", path)
    hud_state.dismiss("task-1", path)  # idempotent
    hud_state.dismiss("task-2", path)

    assert hud_state.load_dismissed(path) == ["task-1", "task-2"]


def test_a_missing_or_corrupt_state_file_reads_as_empty(tmp_path):
    assert hud_state.load_dismissed(tmp_path / "absent.json") == []

    broken = tmp_path / "hud.json"
    broken.write_text("{not json", encoding="utf-8")
    assert hud_state.load_dismissed(broken) == []

    broken.write_text('{"dismissed": "not a list"}', encoding="utf-8")
    assert hud_state.load_dismissed(broken) == []


def test_dismissals_are_bounded(tmp_path):
    path = tmp_path / "hud.json"
    for index in range(hud_state.DISMISSED_LIMIT + 5):
        hud_state.dismiss(f"task-{index}", path)

    kept = hud_state.load_dismissed(path)

    assert len(kept) == hud_state.DISMISSED_LIMIT
    assert kept[-1] == f"task-{hud_state.DISMISSED_LIMIT + 4}"
    assert "task-0" not in kept


def test_summary_is_the_first_non_empty_line_folded_to_one_line():
    condition = "\n\n  Различить   два случая\nи назвать признак различия."

    assert hud_state.summary_of(condition) == "Различить два случая"


def test_summary_is_cut_to_fit():
    summary = hud_state.summary_of("а" * 400, limit=40)

    assert len(summary) == 40
    assert summary.endswith("…")


def test_summary_of_an_empty_condition_is_empty():
    assert hud_state.summary_of("") == ""
    assert hud_state.summary_of("\n \n") == ""


def test_day_label_is_relative_then_dated():
    assert hud_state.day_label(_iso(0)) == "сегодня"
    assert hud_state.day_label(_iso(1)) == "вчера"
    # Older than yesterday: the date, so a stored task cannot read as current.
    assert hud_state.day_label(_iso(40)) not in ("сегодня", "вчера")
    assert hud_state.day_label("") == ""
    assert hud_state.day_label("not a timestamp") == ""


def test_goal_view_is_absent_until_a_goal_is_accepted(tmp_path):
    store = _store(tmp_path)
    assert hud_state.goal_view(store) is None

    store.save_intention("различать случаи", criterion="назову два примера")
    goal = hud_state.goal_view(store)

    assert goal is not None
    assert goal.text == "различать случаи"
    assert goal.criterion == "назову два примера"


# ── background preparation reaches the panel ─────────────────────────────────

def test_a_task_from_a_ready_preparation_is_offered(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "t1")
    _save_preparation(store, "p1", status="ready", task_id="t1")

    offered = hud_state.latest_practice(store)

    assert offered is not None
    assert offered.task_id == "t1"


def test_a_task_from_a_superseded_preparation_is_not_offered(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "t1")
    _save_preparation(store, "p1", status="superseded", task_id="t1")

    # The work was real and stays in the ledger, but the reader has moved on: it
    # must not be presented as the current opportunity.
    assert hud_state.latest_practice(store) is None


def test_a_task_made_by_hand_has_no_preparation_and_is_offered(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "manual")

    offered = hud_state.latest_practice(store)

    assert offered is not None
    assert offered.task_id == "manual"


def test_preparation_view_reports_the_finished_stage(tmp_path):
    store = _store(tmp_path)
    _save_preparation(store, "p1", status="generating")

    view = hud_state.preparation_view(store)

    assert view is not None
    assert view.status == "generating"
    assert view.completed_stage == 2
    assert view.stage_count == 4
    assert view.payload()["working"] is True


def test_a_ready_preparation_whose_task_is_gone_shows_no_scale(tmp_path):
    store = _store(tmp_path)
    _save_task(store, "t1")
    _save_preparation(store, "p1", status="ready", task_id="t1")

    # Attempted or dismissed: the opportunity is gone, so a full ring would lie.
    assert hud_state.preparation_view(store, offered_task_id="") is None
    assert hud_state.preparation_view(store, offered_task_id="t1") is not None


def test_no_preparation_means_no_scale(tmp_path):
    assert hud_state.preparation_view(_store(tmp_path)) is None


def test_a_failed_preparation_shows_no_scale(tmp_path):
    store = _store(tmp_path)
    _save_preparation(store, "p1", status="failed")

    # A bridge failure concludes nothing about the material, so an empty bar would
    # be a verdict about text nobody managed to read.
    assert hud_state.preparation_view(store) is None


def test_a_refusal_about_the_material_is_shown(tmp_path):
    store = _store(tmp_path)
    _save_preparation(store, "p1", status="insufficient")

    view = hud_state.preparation_view(store)

    assert view is not None
    assert view.completed_stage == 1
    assert view.payload()["working"] is False


def test_a_replaced_preparation_shows_no_scale(tmp_path):
    store = _store(tmp_path)
    _save_preparation(store, "p1", status="superseded")

    assert hud_state.preparation_view(store) is None


# ── panel settings ───────────────────────────────────────────────────────────

def test_background_preparation_is_off_until_it_is_turned_on(tmp_path):
    path = tmp_path / "hud.json"

    assert hud_state.load_settings(path)["prepare_in_background"] is False

    hud_state.set_setting("prepare_in_background", True, path)

    assert hud_state.load_settings(path)["prepare_in_background"] is True


def test_an_unknown_setting_is_refused(tmp_path):
    path = tmp_path / "hud.json"

    try:
        hud_state.set_setting("do_my_reading", True, path)
    except KeyError:
        pass
    else:  # pragma: no cover - the call must fail
        raise AssertionError("an unknown setting was stored")


def test_the_two_halves_of_the_panel_file_do_not_clobber_each_other(tmp_path):
    path = tmp_path / "hud.json"
    hud_state.dismiss("task-1", path)

    hud_state.set_setting("prepare_in_background", True, path)

    assert hud_state.load_dismissed(path) == ["task-1"]
    hud_state.dismiss("task-2", path)
    assert hud_state.load_settings(path)["prepare_in_background"] is True
    assert hud_state.load_dismissed(path) == ["task-1", "task-2"]


def test_a_file_from_the_first_release_still_reads(tmp_path):
    path = tmp_path / "hud.json"
    path.write_text('["old-task"]', encoding="utf-8")

    assert hud_state.load_dismissed(path) == ["old-task"]
    assert hud_state.load_settings(path)["prepare_in_background"] is False
