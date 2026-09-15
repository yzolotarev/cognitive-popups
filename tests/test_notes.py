
import sqlite3
import tempfile
from pathlib import Path

from cognitive_popups.notes import (
    ANCHOR_LIMIT,
    SCHEMA_VERSION,
    NoteError,
    NoteStore,
    normalise_anchor,
    render,
)


def temp_store() -> NoteStore:
    return NoteStore(Path(tempfile.mkdtemp()) / "notes.sqlite3")


def test_add_and_get_roundtrip():
    store = temp_store()

    note = store.add("assumed it was just a description", anchor="working memory")

    assert note.id == 1
    assert note.status == "open"
    assert note.anchor == "working memory"
    assert note.closed_utc is None
    assert store.get(1).comment.startswith("assumed")


def test_empty_comment_is_rejected():
    store = temp_store()

    try:
        store.add("   ")
    except NoteError as exc:
        assert "must not be empty" in str(exc)
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected NoteError")


def test_anchor_key_ignores_case_spacing_and_edge_punctuation():
    store = temp_store()

    note = store.add("...", anchor="  Working   Memory.  ")

    assert normalise_anchor("Working   Memory.") == "working memory"
    assert [item.id for item in store.list(anchor_key="working memory")] == [note.id]


def test_long_anchor_is_clipped():
    store = temp_store()

    note = store.add("...", anchor="x" * 5000)

    assert len(note.anchor) == ANCHOR_LIMIT
    assert note.anchor.endswith("…")


def test_close_marks_the_gap_handled():
    store = temp_store()
    note = store.add("...", anchor="memory")

    closed = store.close(note.id, "worked through the definition")

    assert closed.status == "closed"
    assert closed.resolution == "worked through the definition"
    assert closed.closed_utc is not None
    assert store.list(status="open") == []
    assert [item.id for item in store.list(status="closed")] == [note.id]


def test_closing_an_unknown_note_raises():
    store = temp_store()

    try:
        store.close(42)
    except NoteError as exc:
        assert "42" in str(exc)
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected NoteError")


def test_delete_removes_the_note_for_good():
    store = temp_store()
    note = store.add("...", anchor="memory")

    removed = store.delete(note.id)

    assert removed.id == note.id
    assert store.list() == []


def test_deleting_an_unknown_note_raises():
    store = temp_store()

    try:
        store.delete(7)
    except NoteError as exc:
        assert "7" in str(exc)
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected NoteError")


def test_repeats_find_the_anchor_that_tripped_twice():
    store = temp_store()
    store.add("раз", anchor="working memory")
    store.add("два", anchor="Working Memory!")
    store.add("мимо", anchor="theorem")

    assert store.repeats() == [("working memory", 2)]


def test_by_kind_groups_and_labels_the_missing_kind():
    store = temp_store()
    store.add("a", kind="названия из матанализа")
    store.add("b", kind="названия из матанализа")
    store.add("c")

    assert store.by_kind() == [("названия из матанализа", 2), ("(без категории)", 1)]


def test_unwritable_store_raises_instead_of_losing_the_note():
    # Telemetry may fail silently; the reader's own words may not.
    broken = Path(tempfile.mkdtemp()) / "notes.sqlite3"
    broken.mkdir()

    try:
        NoteStore(broken).add("важные слова")
    except NoteError:
        pass
    else:  # pragma: no cover - the call must fail
        raise AssertionError("expected NoteError")


def test_render_shows_anchor_and_resolution():
    store = temp_store()
    note = store.add("assumed it was a description", anchor="working memory")
    store.close(note.id, "worked through the definition")

    text = render(store.list())

    assert "working memory" in text
    assert "[closed]" in text
    assert "worked through the definition" in text


def test_schema_is_versioned():
    store = temp_store()
    store.add("...")

    conn = sqlite3.connect(store.path)
    try:
        version = conn.execute("PRAGMA user_version").fetchone()[0]
    finally:
        conn.close()

    assert version == SCHEMA_VERSION
