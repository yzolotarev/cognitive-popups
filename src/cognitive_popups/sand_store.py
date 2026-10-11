"""Session far edges, explicit rung marks, and verbatim guesses."""
import json
import sqlite3
import time
import uuid
from contextlib import contextmanager
from pathlib import Path

SCHEMA = """
CREATE TABLE IF NOT EXISTS sand_edges (
 id TEXT PRIMARY KEY, session_key TEXT NOT NULL, text TEXT NOT NULL,
 source TEXT NOT NULL, created_epoch REAL NOT NULL, closed_epoch REAL);
CREATE TABLE IF NOT EXISTS sand_rungs (
 id TEXT PRIMARY KEY, edge_id TEXT REFERENCES sand_edges(id), session_key TEXT NOT NULL,
 start TEXT NOT NULL, target TEXT NOT NULL, text TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('read','known')), created_epoch REAL NOT NULL,
 marked_epoch REAL, block_id TEXT);
CREATE TABLE IF NOT EXISTS sand_guesses (
 id TEXT PRIMARY KEY, edge_id TEXT NOT NULL REFERENCES sand_edges(id),
 kind TEXT NOT NULL CHECK(kind IN ('open','close')), text TEXT NOT NULL,
 verdict TEXT NOT NULL, created_epoch REAL NOT NULL);
CREATE TABLE IF NOT EXISTS sand_scenes (
 session_key TEXT PRIMARY KEY, payload TEXT NOT NULL, created_epoch REAL NOT NULL);
CREATE TABLE IF NOT EXISTS sand_versions (
 session_key TEXT NOT NULL, original TEXT NOT NULL, bolder TEXT NOT NULL,
 created_epoch REAL NOT NULL);
"""

class SandStore:
    def __init__(self, path):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.db() as db:
            db.executescript(SCHEMA)

    @contextmanager
    def db(self):
        db = sqlite3.connect(self.path, timeout=10)
        db.row_factory = sqlite3.Row
        db.execute("PRAGMA foreign_keys=ON")
        try:
            with db:
                yield db
        finally:
            db.close()

    def pin(self, session_key, text, source=""):
        if not text.strip():
            raise ValueError("empty far edge")
        with self.db() as db:
            db.execute("UPDATE sand_edges SET closed_epoch=? WHERE session_key=? AND closed_epoch IS NULL", (time.time(), session_key))
            db.execute("INSERT INTO sand_edges VALUES (?,?,?,?,?,NULL)", (uuid.uuid4().hex, session_key, text, source, time.time()))
        return self.edge(session_key)

    def edge(self, session_key):
        with self.db() as db:
            row = db.execute("SELECT * FROM sand_edges WHERE session_key=? AND closed_epoch IS NULL ORDER BY created_epoch DESC LIMIT 1", (session_key,)).fetchone()
        return dict(row) if row else None

    def add_rung(self, session_key, step, edge_id=None):
        rung_id = uuid.uuid4().hex
        with self.db() as db:
            db.execute("INSERT INTO sand_rungs VALUES (?,?,?,?,?,?,?,?,NULL,NULL)", (rung_id, edge_id, session_key, step.start, step.step, step.text, "read", time.time()))
        return rung_id

    def rungs(self, session_key):
        edge = self.edge(session_key)
        edge_id = edge["id"] if edge else None
        with self.db() as db:
            rows = db.execute("SELECT * FROM sand_rungs WHERE session_key=? AND (edge_id=? OR (edge_id IS NULL AND ? IS NULL)) ORDER BY created_epoch", (session_key, edge_id, edge_id)).fetchall()
        return [dict(row) for row in rows]

    def mark(self, rung_id, status):
        if status not in ("read", "known"):
            raise ValueError("explicit read/known mark required")
        with self.db() as db:
            db.execute("UPDATE sand_rungs SET status=?, marked_epoch=? WHERE id=?", (status, time.time(), rung_id))

    def bind_step(self, rung_id, block_id):
        with self.db() as db:
            db.execute("UPDATE sand_rungs SET block_id=? WHERE id=?", (block_id, rung_id))

    def rung_for_step(self, block_id):
        with self.db() as db:
            row = db.execute("SELECT * FROM sand_rungs WHERE block_id=? ORDER BY created_epoch DESC LIMIT 1", (block_id,)).fetchone()
        return dict(row) if row else None

    def move_session(self, old, new):
        with self.db() as db:
            db.execute("UPDATE sand_edges SET session_key=? WHERE session_key=? AND closed_epoch IS NULL", (new, old))
            db.execute("UPDATE sand_rungs SET session_key=? WHERE session_key=? AND block_id IS NULL", (new, old))

    def guess(self, edge_id, kind, text, verdict):
        if kind not in ("open", "close") or not text.strip():
            raise ValueError("guess needs a kind and text")
        with self.db() as db:
            guess_id = uuid.uuid4().hex
            db.execute("INSERT INTO sand_guesses VALUES (?,?,?,?,?,?)", (guess_id, edge_id, kind, text, verdict, time.time()))
        return guess_id

    def verdict(self, guess_id, verdict):
        with self.db() as db:
            db.execute("UPDATE sand_guesses SET verdict=? WHERE id=?", (verdict, guess_id))

    def guesses(self, edge_id):
        with self.db() as db:
            return [dict(r) for r in db.execute("SELECT * FROM sand_guesses WHERE edge_id=? ORDER BY created_epoch", (edge_id,))]

    def version(self, session_key, original, bolder):
        with self.db() as db:
            db.execute("INSERT INTO sand_versions VALUES (?,?,?,?)", (session_key, original, bolder, time.time()))

    def versions(self, start, end):
        with self.db() as db:
            return [dict(r) for r in db.execute("SELECT * FROM sand_versions WHERE created_epoch BETWEEN ? AND ? ORDER BY created_epoch", (start, end))]

    def save_scene(self, session_key, scene):
        with self.db() as db:
            db.execute("INSERT OR REPLACE INTO sand_scenes VALUES (?,?,?)", (session_key, json.dumps(scene, ensure_ascii=False), time.time()))

    def close(self, session_key):
        with self.db() as db:
            db.execute("UPDATE sand_edges SET closed_epoch=? WHERE session_key=? AND closed_epoch IS NULL", (time.time(), session_key))
