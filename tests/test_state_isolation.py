"""A test run cannot write the reader's real state, whatever the shell exported."""
import os
import subprocess
import sys
from pathlib import Path

import conftest
from cognitive_popups import event_log, notes, observation, paths, records, request_gate

ROOT = Path(__file__).resolve().parent.parent


def _under(path, roots) -> bool:
    resolved = str(Path(path).expanduser().resolve())
    return any(resolved == root or resolved.startswith(root + os.sep) for root in roots)


def test_every_store_resolves_inside_the_test_directory(tmp_path):
    assert paths.state_dir() == tmp_path
    for resolved in (records.resolve_db_path(), notes.resolve_db_path(),
                     event_log.resolve_db_path(), observation.ObservationStore().path,
                     request_gate.resolve_root()):
        assert _under(resolved, (str(tmp_path.resolve()),)), resolved
        assert not _under(resolved, conftest.REAL_STATE_ROOTS), resolved


def test_frozen_module_constants_are_redirected():
    for module in (records, notes, event_log, request_gate):
        for name in ("STATE_ROOT", "DEFAULT_DB"):
            value = getattr(module, name, None)
            if value is not None:
                assert not _under(value, conftest.REAL_STATE_ROOTS), f"{module.__name__}.{name}={value}"


def test_redirect_moves_paths_under_a_real_root(tmp_path):
    real = conftest.REAL_STATE_ROOTS[0]
    moved = conftest._redirect(Path(real) / "records.sqlite3", (real,), tmp_path)
    assert moved == tmp_path / "records.sqlite3"
    assert conftest._redirect("/elsewhere/x", (real,), tmp_path) is None


def test_run_tests_script_ignores_an_exported_state_dir(tmp_path):
    """The pytest-free runner with a "real" state dir exported writes nothing there."""
    real = tmp_path / "real-state"
    real.mkdir()
    env = dict(os.environ, COGNITIVE_STATE_DIR=str(real), HOME=str(tmp_path / "home"),
               COGNITIVE_RECORD_DB=str(real / "records.sqlite3"),
               COGNITIVE_NOTES_DB=str(real / "notes.sqlite3"),
               COGNITIVE_EVENT_DB=str(real / "events.sqlite3"))
    result = subprocess.run([sys.executable, str(ROOT / "scripts" / "run-tests.py"), "test_notes"],
                            env=env, capture_output=True, text=True, timeout=120)
    assert " 0 failed" in result.stdout or result.returncode == 0, result.stdout[-2000:]
    assert list(real.iterdir()) == []
