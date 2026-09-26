"""Passive, append-only observations in records.sqlite3.

Public reporting API: ``with store.connect() as conn`` is read-only and returns
sqlite3.Row objects. Tables are obs_schema, obs_configs, obs_requests,
obs_artifacts, obs_presentations, obs_annotations. user_version belongs to the
records store and is never touched. A started row without a finish is unresolved
(e.g. SIGKILL), not proof of failure. No automatic recovery guesses are made.
"""
from __future__ import annotations

from contextlib import closing, contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import sys
import threading
import time
import uuid

from .operation_context import RUN_ID, OperationContext, current_operation

SCHEMA_VERSION = 1
DB_TIMEOUT = 0.25
_TRUTHY = {"1", "true", "yes", "on"}
_failure_lock = threading.Lock()
_failures: dict[str, int] = {}
_warned: dict[str, float] = {}


def disabled():
    return any(os.environ.get(key, "").lower() in _TRUTHY for key in
               ("COGNITIVE_EVENT_DISABLE", "COGNITIVE_OBSERVATION_DISABLE"))


def report_failure(component, exc):
    # Never include exception text: it may contain source text or credentials.
    with _failure_lock:
        _failures[component] = _failures.get(component, 0) + 1
        now = time.monotonic()
        if component not in _warned or now - _warned[component] >= 60:
            _warned[component] = now
            try:
                print(f"cognitive observability: {component} failed ({type(exc).__name__}); "
                      f"failure count={_failures[component]}", file=sys.stderr)
            except Exception:
                pass


def failure_counts():
    with _failure_lock:
        return dict(_failures)


def timestamp():
    epoch = time.time()
    return (datetime.fromtimestamp(epoch, timezone.utc).isoformat(timespec="microseconds"),
            epoch, time.monotonic())


def encode(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def digest(value):
    return hashlib.sha256(encode(value).encode()).hexdigest()


def backup_database(path):
    """Called under a reserved write lock, before DDL; backup reads committed state."""
    target = Path(str(path) + f".backup-{uuid.uuid4().hex}.sqlite3")
    deadline = time.monotonic() + 2

    def progress(status, remaining, total):
        if time.monotonic() > deadline:
            raise TimeoutError("database backup deadline")

    try:
        with closing(sqlite3.connect(Path(path).resolve().as_uri() + "?mode=ro", uri=True,
                                     timeout=DB_TIMEOUT)) as source:
            with closing(sqlite3.connect(target, timeout=DB_TIMEOUT)) as dest:
                source.backup(dest, pages=256, progress=progress, sleep=0.01)
    except BaseException:
        target.unlink(missing_ok=True)
        raise


_SCHEMA = """
CREATE TABLE IF NOT EXISTS obs_schema (version INTEGER PRIMARY KEY);
CREATE TABLE IF NOT EXISTS obs_configs (
 id TEXT PRIMARY KEY, source_hash TEXT NOT NULL, parameters_hash TEXT NOT NULL,
 snapshot_json TEXT NOT NULL, created_utc TEXT, created_epoch REAL, created_mono REAL);
CREATE TABLE IF NOT EXISTS obs_requests (
 request_id TEXT PRIMARY KEY, operation_id TEXT NOT NULL, interaction_id TEXT,
 buffer_session_id TEXT, operation_kind TEXT, run_id TEXT NOT NULL, pid INTEGER,
 config_id TEXT NOT NULL REFERENCES obs_configs(id), messages_json TEXT NOT NULL,
 parameters_json TEXT NOT NULL, started_utc TEXT NOT NULL, started_epoch REAL NOT NULL,
 started_mono REAL NOT NULL, finished_utc TEXT, finished_epoch REAL, finished_mono REAL,
 duration_seconds REAL, status TEXT NOT NULL, raw_response BLOB, response_text TEXT,
 error_type TEXT, error_text TEXT, http_status INTEGER);
CREATE INDEX IF NOT EXISTS obs_requests_operation ON obs_requests(operation_id, started_epoch);
CREATE TABLE IF NOT EXISTS obs_artifacts (
 id TEXT PRIMARY KEY, operation_id TEXT, interaction_id TEXT, buffer_session_id TEXT,
 run_id TEXT, kind TEXT NOT NULL, payload_json TEXT NOT NULL, source_artifact_id TEXT,
 request_ids_json TEXT NOT NULL, created_utc TEXT, created_epoch REAL, created_mono REAL);
CREATE INDEX IF NOT EXISTS obs_artifacts_operation ON obs_artifacts(operation_id);
CREATE TABLE IF NOT EXISTS obs_annotations (
 id TEXT PRIMARY KEY, operation_id TEXT, interaction_id TEXT, buffer_session_id TEXT,
 run_id TEXT, kind TEXT NOT NULL, payload_json TEXT NOT NULL,
 created_utc TEXT, created_epoch REAL, created_mono REAL);
CREATE TABLE IF NOT EXISTS obs_presentations (
 id TEXT PRIMARY KEY, operation_id TEXT, interaction_id TEXT, buffer_session_id TEXT,
 run_id TEXT, artifact_id TEXT, window_instance_id TEXT NOT NULL, event TEXT NOT NULL,
 payload_json TEXT NOT NULL, created_utc TEXT, created_epoch REAL, created_mono REAL);
"""


class ObservationStore:
    def __init__(self, path=None, *, enabled=True):
        self._path = Path(path).expanduser() if path is not None else None
        self.enabled = enabled

    @property
    def path(self):
        return self._path or Path(os.environ.get("COGNITIVE_RECORD_DB") or
            str(Path(os.environ.get("COGNITIVE_STATE_DIR") or
                     "~/.local/state/cognitive-popups") / "records.sqlite3")).expanduser()

    @contextmanager
    def connect(self):
        """Read-only connection; does not create files or migrate schemas."""
        conn = sqlite3.connect(self.path.resolve().as_uri() + "?mode=ro", uri=True,
                               timeout=DB_TIMEOUT)
        conn.row_factory = sqlite3.Row
        try:
            yield conn
        finally:
            conn.close()

    def _write(self, action):
        if not self.enabled or disabled():
            return None
        conn = None
        try:
            path = self.path
            path.parent.mkdir(parents=True, exist_ok=True)
            conn = sqlite3.connect(path, timeout=DB_TIMEOUT)
            conn.execute("PRAGMA foreign_keys=ON")
            conn.execute("BEGIN IMMEDIATE")
            tables = {r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            version = conn.execute("SELECT max(version) FROM obs_schema").fetchone()[0] if "obs_schema" in tables else 0
            if not version:
                if tables:
                    backup_database(path)
                for statement in _SCHEMA.split(";"):
                    if statement.strip():
                        conn.execute(statement)
                conn.execute("INSERT INTO obs_schema VALUES (?)", (SCHEMA_VERSION,))
            result = action(conn)
            conn.commit()
            return result
        except Exception as exc:
            report_failure("observation.write", exc)
            return None
        finally:
            if conn is not None:
                conn.close()

    def start_request(self, request_id, context, messages, parameters, *, started=None):
        if not self.enabled or disabled():
            return None
        try:
            stamp = started or timestamp()
            # Hash actual installed Python sources; never scan environment/secrets.
            sources = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                       for p in sorted(Path(__file__).parent.glob("*.py"))}
            config = {"sources": sources, "parameters": parameters,
                      "system_messages": [m for m in messages if m.get("role") in ("system", "developer")]}
            config_id = digest(config)
            messages_json, parameters_json = encode(messages), encode(parameters)

            def action(conn):
                conn.execute("INSERT OR IGNORE INTO obs_configs VALUES (?,?,?,?,?,?,?)",
                             (config_id, digest(sources), digest(parameters), encode(config), *stamp))
                conn.execute("INSERT INTO obs_requests (request_id,operation_id,interaction_id,"
                             "buffer_session_id,operation_kind,run_id,pid,config_id,messages_json,"
                             "parameters_json,started_utc,started_epoch,started_mono,status) "
                             "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                             (request_id, context.operation_id, context.interaction_id,
                              context.buffer_session_id, context.kind, RUN_ID, os.getpid(),
                              config_id, messages_json, parameters_json, *stamp, "started"))
                return request_id
            return self._write(action)
        except Exception as exc:
            report_failure("observation.start", exc)
            return None

    def finish_request(self, request_id, status, *, raw_response=None, response_text=None,
                       error=None, http_status=None, finished=None):
        try:
            stamp = finished or timestamp()
            return self._write(lambda conn: conn.execute(
                "UPDATE obs_requests SET finished_utc=?,finished_epoch=?,finished_mono=?,"
                "duration_seconds=?-started_mono,status=?,raw_response=?,response_text=?,"
                "error_type=?,error_text=?,http_status=? WHERE request_id=? AND status='started'",
                (*stamp, stamp[2], status, raw_response, response_text,
                 type(error).__name__ if error else None, str(error) if error else None,
                 http_status, request_id)).rowcount)
        except Exception as exc:
            report_failure("observation.finish", exc)
            return None

    def _record(self, table, fields):
        if not self.enabled or disabled():
            return None
        try:
            ctx = current_operation()
            stamp = timestamp()
            row = dict(id=uuid.uuid4().hex, operation_id=ctx.operation_id if ctx else None,
                       interaction_id=ctx.interaction_id if ctx else None,
                       buffer_session_id=ctx.buffer_session_id if ctx else None, run_id=RUN_ID,
                       created_utc=stamp[0], created_epoch=stamp[1], created_mono=stamp[2], **fields)
            return self._write(lambda conn: (conn.execute(
                f"INSERT INTO {table} ({','.join(row)}) VALUES ({','.join('?' for _ in row)})",
                tuple(row.values())), row["id"])[1])
        except Exception as exc:
            report_failure("observation.record", exc)
            return None

    def record_artifact(self, kind, payload, *, request_ids=None, source_artifact_id=None):
        if not self.enabled or disabled():
            return None
        try:
            return self._payload_record("obs_artifacts", payload, kind=kind,
                request_ids_json=encode(request_ids or []), source_artifact_id=source_artifact_id)
        except Exception as exc:
            report_failure("observation.artifact", exc)
            return None

    def record_annotation(self, kind, payload):
        return self._payload_record("obs_annotations", payload, kind=kind)

    def record_presentation(self, artifact_id, *, window_instance_id, event, payload=None):
        self._payload_record("obs_presentations", payload, artifact_id=artifact_id,
                             window_instance_id=window_instance_id, event=event)

    def _payload_record(self, table, payload, **fields):
        if not self.enabled or disabled():
            return None
        try:
            return self._record(table, dict(payload_json=encode(payload), **fields))
        except Exception as exc:
            report_failure("observation.payload", exc)
            return None

    def request_ids_for_operation(self, operation_id):
        if not self.path.exists():
            return []
        try:
            with self.connect() as conn:
                return [r[0] for r in conn.execute("SELECT request_id FROM obs_requests "
                    "WHERE operation_id=? ORDER BY started_epoch,request_id", (operation_id,))]
        except Exception as exc:
            report_failure("observation.read", exc)
            return []


def record_artifact(kind: str, payload: object, *, request_ids: list[str] | None = None,
                    source_artifact_id: str | None = None) -> str | None:
    return ObservationStore().record_artifact(kind, payload, request_ids=request_ids,
                                              source_artifact_id=source_artifact_id)


def record_presentation(artifact_id: str | None, *, window_instance_id: str,
                        event: str, payload: object = None) -> None:
    ObservationStore().record_presentation(artifact_id, window_instance_id=window_instance_id,
                                           event=event, payload=payload)


def record_annotation(kind: str, payload: object) -> str | None:
    return ObservationStore().record_annotation(kind, payload)


def request_ids_for_operation(operation_id: str) -> list[str]:
    return ObservationStore().request_ids_for_operation(operation_id)
