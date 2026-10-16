"""A poisoned thought is skipped after three tries; unbacked hypotheses make no links."""
import json
import sqlite3

import pytest

from cognitive_popups import universe as U


class Client:
    def __init__(self, reply):
        self.reply = reply
        self.calls = []

    def complete(self, messages, **kwargs):
        self.calls.append(json.loads(messages[1]["content"].split("<THOUGHTS>\n", 1)[1].split("\n</THOUGHTS>")[0])
                          if "<THOUGHTS>" in messages[1]["content"] else None)
        if self.calls[-1] is None:  # the alias merge after naming
            return '{"aliases": []}'
        return self.reply(self.calls[-1]) if callable(self.reply) else self.reply


@pytest.fixture
def world(tmp_path):
    world = U.Universe(root=tmp_path)
    with world._db() as db:
        db.executemany("insert into thoughts (id, kind, text, created_utc, verdict) values (?,?,?,?,?)", [
            ("poison", "note", "яд", "2026-01-01", None),
            ("ok1", "note", "мысль один", "2026-01-02", None),
            ("hyp", "hypothesis", "vlookup ищет по строкам", "2026-01-03", "contradicted"),
        ])
    return world


def _named(world):
    with world._db() as db:
        return {row[0] for row in db.execute("select id from thoughts where extracted_utc is not null")}


def test_a_thought_that_breaks_naming_is_isolated_then_skipped(world):
    def reply(items):
        if any(item["id"] == "poison" for item in items):
            return "not json at all"
        return json.dumps({"items": [{"id": item["id"], "concepts": ["понятие"]} for item in items]})

    client = Client(reply)
    for _ in range(6):
        world.sync(client, limit=50)
    assert _named(world) == {"ok1", "hyp"}
    assert world.attempts("poison") == U.MAX_ATTEMPTS
    assert world.pending(100) == []
    with world._db() as db:
        assert "JSON" in db.execute("select last_error from thoughts where id='poison'").fetchone()[0]
    before = len(client.calls)
    world.sync(client)
    assert len(client.calls) == before, "a skipped thought costs no more requests"


def test_bridge_down_does_not_count_against_thoughts(world):
    class Down:
        def complete(self, *a, **k):
            raise U.Web2APIError("cannot reach the bridge", kind="transport")
    for _ in range(5):
        world.sync(Down())
    assert world.attempts("poison") == 0 and len(world.pending(100)) == 3


def test_contradicted_hypothesis_gives_concepts_but_no_links(world):
    def reply(items):
        assert {"id": "hyp", "verdict": "contradicted"}.items() <= next(i for i in items if i["id"] == "hyp").items()
        return json.dumps({"items": [{"id": item["id"], "concepts": ["vlookup", "строка"],
                                      "relations": [["vlookup", "ищет по", "строка"]]} for item in items]},
                          ensure_ascii=False)
    world.sync(Client(reply))
    with world._db() as db:
        links = db.execute("select thought_id from links").fetchall()
        concepts = db.execute("select concept from thought_concepts where thought_id='hyp'").fetchall()
    assert ("hyp",) not in links and ("ok1",) in links
    assert sorted(concepts) == [("vlookup",), ("строка",)]


def test_old_universe_files_gain_the_retry_columns(tmp_path):
    path = tmp_path / "universe.sqlite3"
    with sqlite3.connect(path) as db:
        db.execute("create table thoughts (id text primary key, kind text not null, text text not null,"
                   " anchor text, verdict text, delta text, created_utc text not null,"
                   " hidden integer not null default 0, topic text, about_system integer, extracted_utc text)")
        db.execute("insert into thoughts (id, kind, text, created_utc) values ('a','note','x','t')")
    world = U.Universe(path, root=tmp_path)
    assert world.attempts("a") == 0 and len(world.pending(10)) == 1
