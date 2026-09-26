import json
import sqlite3
from concurrent.futures import ThreadPoolExecutor

import pytest

from cognitive_popups import observation as obs
from cognitive_popups.operation_context import (
    OperationContext, bind_operation, current_operation, operation_scope,
)


@pytest.fixture(autouse=True)
def isolated_storage(tmp_path, monkeypatch):
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path))
    monkeypatch.setenv("COGNITIVE_RECORD_DB", str(tmp_path / "records.sqlite3"))
    monkeypatch.delenv("COGNITIVE_EVENT_DISABLE", raising=False)
    monkeypatch.delenv("COGNITIVE_OBSERVATION_DISABLE", raising=False)


def test_records_coexistence_backup_and_idempotence(tmp_path):
    store = obs.ObservationStore()
    with sqlite3.connect(store.path) as conn:
        conn.execute("CREATE TABLE records (id INTEGER PRIMARY KEY, data TEXT)")
        conn.execute("INSERT INTO records VALUES (1, 'untouched')")
        conn.execute("PRAGMA user_version=2")
    assert store.record_annotation("input", {"source": "full"})
    assert store.record_annotation("input", {"source": "second"})
    with store.connect() as conn:
        assert conn.execute("PRAGMA user_version").fetchone()[0] == 2
        assert conn.execute("SELECT data FROM records").fetchone()[0] == "untouched"
        assert conn.execute("SELECT version FROM obs_schema").fetchone()[0] == obs.SCHEMA_VERSION
        with pytest.raises(sqlite3.OperationalError):
            conn.execute("DELETE FROM records")
    backups = list(tmp_path.glob("*.backup-*.sqlite3"))
    assert len(backups) == 1
    with sqlite3.connect(backups[0]) as conn:
        assert conn.execute("SELECT data FROM records").fetchone()[0] == "untouched"
        assert not conn.execute("SELECT name FROM sqlite_master WHERE name='obs_schema'").fetchall()


def test_start_finish_config_hashes_and_immutability(monkeypatch):
    store = obs.ObservationStore()
    context = OperationContext(operation_id="op", interaction_id="interaction")
    params = {"model": "test", "temperature": 0, "max_tokens": 12}
    for rid, text in [("a", "first"), ("b", "second")]:
        monkeypatch.setenv("ARBITRARY_SECRET", text)
        assert store.start_request(rid, context, [{"role": "system", "content": "rules"},
            {"role": "user", "content": text}], params)
    store.start_request("c", context, [{"role": "system", "content": "changed"}], params)
    store.start_request("d", context, [], {**params, "temperature": 1})
    assert store.finish_request("a", "ok", response_text="result") == 1
    assert store.finish_request("a", "timeout") == 0
    assert store.start_request("a", context, [], {}) is None
    with store.connect() as conn:
        rows = {r["request_id"]: dict(r) for r in conn.execute("SELECT * FROM obs_requests")}
        configs = {r["id"]: dict(r) for r in conn.execute("SELECT * FROM obs_configs")}
    assert rows["a"]["status"] == "ok"
    assert rows["b"]["status"] == "started"
    assert rows["b"]["finished_epoch"] is None
    assert rows["a"]["config_id"] == rows["b"]["config_id"]
    assert rows["a"]["config_id"] != rows["c"]["config_id"]
    assert configs[rows["a"]["config_id"]]["parameters_hash"] == configs[rows["c"]["config_id"]]["parameters_hash"]
    assert configs[rows["a"]["config_id"]]["parameters_hash"] != configs[rows["d"]["config_id"]]["parameters_hash"]
    assert "ARBITRARY_SECRET" not in str(configs)
    assert json.loads(rows["a"]["messages_json"])[1]["content"] == "first"


def test_public_artifact_annotation_presentation_contract():
    ctx = OperationContext(operation_id="op", interaction_id="i", buffer_session_id="buffer")
    with operation_scope(ctx):
        annotation = obs.record_annotation("source", {"text": "x" * 20000})
        artifact = obs.record_artifact("answer", {"answer": [1, 2]}, request_ids=["r"], source_artifact_id="source")
        obs.record_presentation(artifact, window_instance_id="window", event="opened", payload={"full": True})
        obs.record_presentation(None, window_instance_id="window", event="closed")
    store = obs.ObservationStore()
    with store.connect() as conn:
        row = conn.execute("SELECT * FROM obs_artifacts").fetchone()
        assert row["id"] == artifact
        assert row["operation_id"] == "op"
        assert row["interaction_id"] == "i"
        assert row["buffer_session_id"] == "buffer"
        assert row["source_artifact_id"] == "source"
        assert json.loads(row["request_ids_json"]) == ["r"]
        row = conn.execute("SELECT * FROM obs_annotations").fetchone()
        assert row["id"] == annotation
        assert len(json.loads(row["payload_json"])["text"]) == 20000
        assert conn.execute("SELECT count(*) FROM obs_presentations").fetchone()[0] == 2


@pytest.mark.parametrize("flag", ["COGNITIVE_EVENT_DISABLE", "COGNITIVE_OBSERVATION_DISABLE"])
def test_disabled_no_writes(flag, tmp_path, monkeypatch):
    store = obs.ObservationStore(tmp_path / "absent" / "db")
    monkeypatch.setenv(flag, "1")
    assert store.start_request("r", OperationContext(), [], {}) is None
    assert store.finish_request("r", "ok") is None
    assert store.record_annotation("input", {}) is None
    assert store.record_artifact("answer", {}) is None
    store.record_presentation(None, window_instance_id="w", event="open")
    assert not store.path.parent.exists()
    assert obs.record_annotation("input", {}) is None
    assert not obs.ObservationStore().path.exists()


def test_context_binding_restores_even_on_exception():
    outer, inner = OperationContext(), OperationContext()
    with operation_scope(outer):
        bound = bind_operation(current_operation)
        explicit = bind_operation(current_operation, inner)
    unbound = bind_operation(current_operation)
    with operation_scope(inner):
        assert bound() == outer
        assert explicit() == inner
        assert unbound() is None
        assert current_operation() == inner
        with pytest.raises(RuntimeError):
            with operation_scope(outer):
                raise RuntimeError()
        assert current_operation() == inner
    with ThreadPoolExecutor(1) as pool:
        assert pool.submit(bound).result() == outer
        assert pool.submit(current_operation).result() is None
    assert current_operation() is None


def test_failures_warn_rate_limit_and_count(tmp_path, capsys, monkeypatch):
    path = tmp_path / "directory"
    path.mkdir()
    store = obs.ObservationStore(path)
    monkeypatch.setattr(obs, "_warned", {})
    before = obs.failure_counts().get("observation.write", 0)
    assert store.record_annotation("input", {}) is None
    assert store.record_annotation("input", {}) is None
    assert obs.failure_counts()["observation.write"] == before + 2
    assert capsys.readouterr().err.count("observation.write failed") == 1


def test_locked_database_is_bounded_and_nonbreaking():
    import time
    store = obs.ObservationStore()
    store.record_annotation("first", {})
    with sqlite3.connect(store.path) as conn:
        conn.execute("BEGIN IMMEDIATE")
        started = time.monotonic()
        assert store.record_annotation("blocked", {}) is None
        assert time.monotonic() - started < 2


def test_dynamic_path_and_read_without_creation(tmp_path, monkeypatch):
    monkeypatch.delenv("COGNITIVE_RECORD_DB")
    store = obs.ObservationStore()
    assert store.request_ids_for_operation("missing") == []
    assert not store.path.exists()
    monkeypatch.setenv("COGNITIVE_STATE_DIR", str(tmp_path / "moved"))
    assert store.path == tmp_path / "moved" / "records.sqlite3"
    with pytest.raises(sqlite3.OperationalError):
        with store.connect():
            pass
    assert not store.path.parent.exists()


def test_async_context_isolation():
    import asyncio
    a, b = OperationContext(), OperationContext()

    async def current_after_yield():
        await asyncio.sleep(0)
        return current_operation()

    async def run():
        with operation_scope(a):
            callback_a = bind_operation(current_after_yield)
        with operation_scope(b):
            callback_b = bind_operation(current_after_yield)
        assert await asyncio.gather(callback_a(), callback_b()) == [a, b]
        assert current_operation() is None
    asyncio.run(run())


def test_abrupt_exit_leaves_durable_started_request(tmp_path):
    import os
    import subprocess
    import sys
    from pathlib import Path
    store = obs.ObservationStore()
    script = (
        "import os\n"
        "from cognitive_popups.observation import ObservationStore\n"
        "from cognitive_popups.operation_context import OperationContext\n"
        "assert ObservationStore().start_request('abrupt', OperationContext(operation_id='op'), [], {})\n"
        "os._exit(17)\n"
    )
    result = subprocess.run([sys.executable, "-c", script], timeout=10,
        env={**os.environ, "PYTHONPATH": str(Path(obs.__file__).resolve().parent.parent)})
    assert result.returncode == 17
    with store.connect() as conn:
        row = conn.execute("SELECT * FROM obs_requests").fetchone()
    assert row["status"] == "started"
    assert row["finished_epoch"] is None
    assert store.request_ids_for_operation("op") == ["abrupt"]


def test_failed_ddl_is_transactional(monkeypatch):
    store = obs.ObservationStore()
    with sqlite3.connect(store.path) as conn:
        conn.execute("CREATE TABLE original (id INTEGER)")
    monkeypatch.setattr(obs, "_SCHEMA", obs._SCHEMA + ";INVALID SQL;")
    assert store.record_annotation("input", {}) is None
    with store.connect() as conn:
        assert [r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")] == ["original"]


def test_migration_rollback_when_backup_fails(monkeypatch):
    store = obs.ObservationStore()
    with sqlite3.connect(store.path) as conn:
        conn.execute("CREATE TABLE original (id INTEGER)")
        conn.execute("PRAGMA user_version=2")
    def fail(path):
        raise OSError("backup unavailable")
    monkeypatch.setattr(obs, "backup_database", fail)
    assert store.record_annotation("input", {}) is None
    with store.connect() as conn:
        assert [r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")] == ["original"]
        assert conn.execute("PRAGMA user_version").fetchone()[0] == 2
