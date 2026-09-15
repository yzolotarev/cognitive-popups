
import io
import tempfile
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

from cognitive_popups import event_log, notes


def temp_db(name: str) -> Path:
    return Path(tempfile.mkdtemp()) / name


def run(argv: list[str], module):
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        code = module.main(argv)
    return code, out.getvalue(), err.getvalue()


def test_notes_cli_says_so_when_empty():
    code, _out, err = run(["--db", str(temp_db("notes.sqlite3")), "--open"], notes)

    assert code == 1
    assert "no notes" in err


def test_notes_cli_lists_then_closes():
    db = temp_db("notes.sqlite3")
    notes.NoteStore(db).add("assumed it was a description", anchor="working memory")

    code, out, _err = run(["--db", str(db), "--open"], notes)
    assert code == 0
    assert "working memory" in out
    assert "[open]" in out

    code, out, _err = run(["--db", str(db), "--close", "1", "-m", "worked through the definition"], notes)
    assert code == 0
    assert "closed" in out

    code, out, _err = run(["--db", str(db), "--all"], notes)
    assert code == 0
    assert "[closed]" in out


def test_notes_cli_repeats_and_by_kind():
    db = temp_db("notes.sqlite3")
    store = notes.NoteStore(db)
    store.add("раз", anchor="working memory")
    store.add("два", anchor="Working Memory!")
    store.add("три", anchor="theorem", kind="названия")

    code, out, _err = run(["--db", str(db), "--repeats"], notes)
    assert code == 0
    assert "working memory" in out

    code, out, _err = run(["--db", str(db), "--by-kind"], notes)
    assert code == 0
    assert "названия" in out


def test_notes_cli_deletes():
    db = temp_db("notes.sqlite3")
    notes.NoteStore(db).add("assumed it was a description", anchor="working memory")

    code, out, _err = run(["--db", str(db), "--delete", "1"], notes)

    assert code == 0
    assert "deleted" in out
    assert notes.NoteStore(db).list() == []


def test_notes_cli_does_not_traceback_on_a_broken_database():
    broken = temp_db("notes.sqlite3")
    broken.mkdir()

    code, _out, err = run(["--db", str(broken), "--open"], notes)

    assert code == 1
    assert "cannot open" in err


def test_event_log_cli_says_so_when_empty():
    code, _out, err = run(["--db", str(temp_db("events.sqlite3")), "--tail", "5"], event_log)

    assert code == 1
    assert "no events" in err


def test_event_log_cli_prints_timeline_then_prunes():
    db = temp_db("events.sqlite3")
    event_log.EventLog(db).log("hotkey", session_id="s1", window="seed", detail="4 слова")

    code, out, _err = run(["--db", str(db), "--tail", "5"], event_log)
    assert code == 0
    assert "hotkey" in out

    code, out, _err = run(["--db", str(db), "--prune-days", "0"], event_log)
    assert code == 0
    assert "removed 1 events" in out
    assert event_log.fetch(db) == []
