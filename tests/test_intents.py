from __future__ import annotations

import sqlite3

from cognitive_popups import records


def _store(tmp_path) -> records.RecordStore:
    return records.RecordStore(tmp_path / "records.sqlite3")


def test_intention_is_saved_and_recalled(tmp_path):
    store = _store(tmp_path)
    assert store.current_intention() is None

    saved = store.save_intention(
        "  понять, зачем из системы делают матрицу  ",
        criterion="покажу замену на одном примере",
        stopped_at="коэффициенты понял",
    )

    current = store.current_intention()
    assert current is not None
    assert current.id == saved.id
    assert current.text == "понять, зачем из системы делают матрицу"
    assert current.criterion == "покажу замену на одном примере"
    assert current.stopped_at == "коэффициенты понял"
    assert current.status == "current"
    assert store.intention_history() == []


def test_new_intention_replaces_without_judging_the_old_one(tmp_path):
    store = _store(tmp_path)
    first = store.save_intention("первая цель")
    second = store.save_intention("вторая цель")

    assert store.current_intention().id == second.id
    assert [item.id for item in store.intention_history()] == [first.id]

    # Replaced, not reached and not failed: it can be made current again.
    restored = store.promote_intention(first.id)
    assert restored.id == first.id
    assert store.current_intention().id == first.id
    assert [item.id for item in store.intention_history()] == [second.id]


def test_edit_keeps_the_same_bookmark(tmp_path):
    store = _store(tmp_path)
    saved = store.save_intention("черновая цель")

    updated = store.update_intention(
        saved.id, "уточнённая цель", criterion="когда назову два примера"
    )

    assert updated.id == saved.id
    assert store.current_intention().text == "уточнённая цель"
    assert updated.updated_at
    assert store.intention_history() == []


def test_only_the_optional_lines_can_be_left_empty(tmp_path):
    store = _store(tmp_path)
    store.save_intention("цель без критерия")

    current = store.current_intention()
    assert current.text == "цель без критерия"
    assert current.criterion == ""
    assert current.stopped_at == ""


def test_empty_intention_is_rejected(tmp_path):
    store = _store(tmp_path)
    for text in ("", "   "):
        try:
            store.save_intention(text)
        except ValueError:
            pass
        else:  # pragma: no cover - the call must fail
            raise AssertionError("an empty intention was accepted")
    assert store.current_intention() is None


def test_intentions_survive_restart_and_buffer_clears(tmp_path):
    path = tmp_path / "records.sqlite3"
    records.RecordStore(path).save_intention("пережить перерыв")

    # The bookmark is not owned by a reading session: closing and reopening the
    # store (a restart) and closing stale sessions must not touch it.
    store = records.RecordStore(path)
    store.close_stale()
    store.close_session("unknown", "clear")

    assert records.RecordStore(path).current_intention().text == "пережить перерыв"


def test_material_and_direction_are_stored_with_the_goal(tmp_path):
    store = _store(tmp_path)
    saved = store.save_intention(
        "понять, почему работает",
        material="абзац про матрицы",
        material_origin="буфер обмена",
        direction="Понять, почему работает",
    )

    current = store.current_intention()
    assert current.id == saved.id
    assert current.material == "абзац про матрицы"
    assert current.material_origin == "буфер обмена"
    assert current.direction == "Понять, почему работает"

    # The passage keeps its own shape: it is not folded into a single line.
    store.save_intention("другая", material="строка один\nстрока два")
    assert store.current_intention().material == "строка один\nстрока два"


def test_editing_a_goal_keeps_what_it_was_grounded_in(tmp_path):
    store = _store(tmp_path)
    saved = store.save_intention(
        "черновик", material="абзац", material_origin="выделение",
        direction="Понять общую идею")

    updated = store.update_intention(saved.id, "уточнённая цель")

    assert updated.material == "абзац"
    assert updated.material_origin == "выделение"
    assert updated.direction == "Понять общую идею"

    # An explicit empty value still clears it: only None means "leave alone".
    cleared = store.update_intention(saved.id, "третья", material="")
    assert cleared.material == ""


def test_a_goal_needs_no_material_to_be_stored(tmp_path):
    store = _store(tmp_path)
    store.save_intention("цель без материала")

    current = store.current_intention()
    assert current.text == "цель без материала"
    assert current.material == ""
    assert current.material_origin == ""
    assert current.direction == ""


def test_ledger_written_before_the_goal_columns_still_opens(tmp_path):
    path = tmp_path / "records.sqlite3"
    conn = sqlite3.connect(path)
    conn.executescript(
        "CREATE TABLE intents (id TEXT PRIMARY KEY, text TEXT NOT NULL, criterion TEXT,"
        " stopped_at TEXT, source TEXT, created_utc TEXT NOT NULL,"
        " created_epoch REAL NOT NULL, updated_utc TEXT, updated_epoch REAL,"
        " status TEXT NOT NULL DEFAULT 'current');"
    )
    conn.execute(
        "INSERT INTO intents (id, text, created_utc, created_epoch, status)"
        " VALUES ('old-goal', 'старая цель', '2026-09-01T00:00:00+00:00', 1.0, 'current')"
    )
    conn.commit()
    conn.close()

    store = _store(tmp_path)
    current = store.current_intention()

    assert current is not None
    assert current.text == "старая цель"
    assert current.material == ""
    assert current.material_origin == ""
    assert current.direction == ""

    # The added columns are writable on the migrated file.
    store.update_intention(current.id, "уточнённая", material="абзац")
    assert store.current_intention().material == "абзац"


def test_existing_ledger_is_copied_once_before_migration(tmp_path):
    path = tmp_path / "records.sqlite3"
    conn = sqlite3.connect(path)
    conn.executescript(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, title TEXT,"
        " created_utc TEXT NOT NULL, created_epoch REAL NOT NULL,"
        " closed_utc TEXT, closed_epoch REAL, close_reason TEXT);"
    )
    conn.execute(
        "INSERT INTO sessions (id, title, created_utc, created_epoch)"
        " VALUES ('old', 'Old', '2026-09-01T00:00:00+00:00', 1.0)"
    )
    conn.execute("PRAGMA user_version=3")
    conn.commit()
    conn.close()

    store = _store(tmp_path)
    store.open_session("s1")

    assert records.SCHEMA_VERSION > 3
    backups = [p for p in tmp_path.iterdir() if ".backup-" in p.name]
    assert len(backups) == 1
    # Existing rows survive the migration.
    assert {s["id"] for s in store.sessions()} == {"old", "s1"}
    assert store.save_intention("после миграции").status == "current"

    # The ledger is already current, so reopening makes no second copy.
    records.RecordStore(path).open_session("s2")
    assert len([p for p in tmp_path.iterdir() if ".backup-" in p.name]) == 1
