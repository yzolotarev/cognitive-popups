"""The reader's universe: verbatim thoughts, model-named concepts, graph recall."""
import json
import sqlite3

import pytest

from cognitive_popups import universe as U


class FakeClient:
    def __init__(self, *replies):
        self.replies = list(replies)
        self.calls = []

    def complete(self, messages, **kwargs):
        self.calls.append((messages, kwargs))
        # After naming, sync asks for synonyms; an exhausted script means "none".
        return self.replies.pop(0) if self.replies else '{"aliases": []}'


@pytest.fixture
def world(tmp_path, monkeypatch):
    for key in ("COGNITIVE_NOTES_DB", "COGNITIVE_RECORD_DB", "COGNITIVE_UNIVERSE_DB"):
        monkeypatch.delenv(key, raising=False)
    notes = sqlite3.connect(tmp_path / "notes.sqlite3")
    notes.execute("create table error_notes (id integer, created_utc text, comment text, anchor text)")
    notes.executemany("insert into error_notes values (?,?,?,?)", [
        (1, "2026-09-20T10:00:00+00:00", "по сути как выпадающий список", "VLOOKUP ищет код"),
        (2, "2026-09-20T10:01:00+00:00", "опять окно не там где мышка", "абзац"),
        (3, "2026-09-20T10:02:00+00:00", "freut mich - сцена из фильма", "Nice to meet you"),
    ])
    notes.commit()
    records = sqlite3.connect(tmp_path / "records.sqlite3")
    records.executescript("""
        create table predictions (id text, created_utc text, hypothesis text, status text,
                                  mismatch text, one_delta text);
        create table prediction_fragments (prediction_id text, fragment_id text);
        create table fragments (id text, source_text text);
        create table feynman_checks (id text, created_utc text, question text, explanation text);
        create table intents (id text, created_utc text, text text);
        insert into predictions values ('p1', '2026-09-20T10:03:00+00:00', 'vlookup ищет по строкам',
                                        'contradicted', '', 'ищет вниз по левому столбцу');
        insert into prediction_fragments values ('p1', 'f1');
        insert into fragments values ('f1', 'Функция VLOOKUP берёт код');
        insert into feynman_checks values ('c1', '2026-09-20T10:04:00+00:00', 'Q', 'хз');
        insert into intents values ('g1', '2026-09-20T09:00:00+00:00', 'подготовка к стажировке');
    """)
    records.commit()
    return U.Universe(root=tmp_path)


NAMING = json.dumps({"items": [
    {"id": "note:1", "topic": "excel", "about_system": False,
     "concepts": ["Выпадающий список", "VLOOKUP"], "relations": [["vlookup", "похоже на", "выпадающий список"]]},
    {"id": "note:2", "topic": "программа", "about_system": True, "concepts": ["vlookup", "окно"]},
    {"id": "note:3", "topic": "немецкий", "about_system": False, "concepts": ["freut mich", "фильм"]},
    {"id": "hyp:p1", "topic": "excel", "about_system": False, "concepts": ["vlookup", "строка"]},
    {"id": "goal:g1", "topic": "excel", "about_system": False, "concepts": ["стажировка"]},
    {"id": "intruder", "concepts": ["x"]},
]}, ensure_ascii=False)


def test_harvest_is_verbatim_idempotent_and_skips_empty_answers(world):
    assert world.harvest() == 5  # three notes, one hypothesis, one goal; "хз" is no thought
    assert world.harvest() == 0
    with world._db() as db:
        text, anchor, delta = db.execute(
            "select text, anchor, delta from thoughts where id = 'hyp:p1'").fetchone()
    assert (text, anchor, delta) == ("vlookup ищет по строкам", "Функция VLOOKUP берёт код",
                                     "ищет вниз по левому столбцу")


def test_naming_in_background_normalises_and_ignores_unknown_ids(world):
    client = FakeClient(NAMING)
    result = world.sync(client)
    assert result == {"added": 5, "named": 5, "failed": 0, "merged": 0, "prepared": 0,
                      "requests": 2, "pending": 0}
    assert client.calls[0][1]["priority"] == U.PRIORITY_BACKGROUND
    with world._db() as db:
        assert db.execute("select count(*) from thought_concepts where thought_id='intruder'").fetchone()[0] == 0
        assert ("выпадающий список",) in db.execute(
            "select concept from thought_concepts where thought_id='note:1'").fetchall()
        assert db.execute("select a, relation, b from links").fetchall() == [
            ("vlookup", "похоже на", "выпадающий список")]


def test_recall_reaches_own_images_and_never_the_app_itself(world):
    world.sync(FakeClient(NAMING))
    hits = world.recall(["выпадающий список"])
    assert [h.thought_id for h in hits][0] == "note:1"
    vlookup = [h.thought_id for h in world.recall(["vlookup"])]
    assert "note:2" not in vlookup  # about the app: kept, never recalled into study
    # "vlookup" sits on most thoughts here, so it is a hub: alone it wakes nothing
    # (hyp:p1); with the graph's own link to the reader's image it does (note:1).
    assert vlookup == ["note:1"]


def test_weak_or_unknown_links_stay_silent(world):
    world.sync(FakeClient(NAMING))
    assert world.recall(["омлет"]) == []
    assert world.recall([]) == []


def test_hidden_thoughts_are_neither_named_nor_recalled(world):
    world.harvest()
    assert world.set_hidden("note:3", True)
    assert "note:3" not in [tid for tid, *_ in world.pending(100)]
    assert not world.set_hidden("note:404", True)


def test_passage_concepts_are_cached(world):
    world.sync(FakeClient(NAMING))
    client = FakeClient(json.dumps({"concepts": ["Freut Mich", "приветствие"]}))
    concepts, hits = world.query(client, "Hallo! Freut mich.")
    assert concepts == ["freut mich", "приветствие"]  # no bridges named
    assert [h.thought_id for h in hits] == ["note:3"]
    again, _ = world.query(client, "Hallo!   Freut mich.")
    assert again == concepts and len(client.calls) == 1


def test_bridges_reach_the_readers_world_only_through_known_concepts(world):
    world.sync(FakeClient(NAMING))
    client = FakeClient(json.dumps({"concepts": ["справочник"],
                                    "bridges": ["выпадающий список", "выдумка"]}))
    concepts, hits = world.query(client, "Справочник товаров")
    assert concepts == ["справочник", "мост: выпадающий список"]  # unknown bridge dropped
    assert [h.thought_id for h in hits] == ["note:1"]


def test_alias_merge_folds_short_names_and_refuses_related_pairs(world):
    naming = json.loads(NAMING)
    naming["items"][3]["concepts"] = ["галуа", "строка"]
    naming["items"][4]["concepts"] = ["теория галуа"]
    aliases = json.dumps({"aliases": [["галуа", "теория галуа"], ["vlookup", "строка"]]})
    result = world.sync(FakeClient(json.dumps(naming, ensure_ascii=False), aliases))
    assert result["merged"] == 1
    with world._db() as db:
        assert db.execute("select alias, canonical from aliases").fetchall() == [("галуа", "теория галуа")]
        assert db.execute("select count(*) from thought_concepts where concept='галуа'").fetchone()[0] == 0
    assert {h.thought_id for h in world.recall(["галуа"])} == {"hyp:p1", "goal:g1"}


def test_failed_naming_stops_without_losing_the_queue(world):
    class Down:
        def complete(self, *a, **k):
            raise U.Web2APIError("bridge down")
    result = world.sync(Down())
    assert result["named"] == 0 and result["pending"] == 5


def test_slack_gate_spends_nothing_when_busy_and_one_piece_on_trickle(world):
    busy = FakeClient()
    assert world.sync(busy, gate=lambda: "busy")["requests"] == 0 and busy.calls == []
    trickle = FakeClient(NAMING)
    result = world.sync(trickle, gate=lambda: "trickle")
    assert result["requests"] == 1 and len(trickle.calls) == 1  # naming only, no merge
