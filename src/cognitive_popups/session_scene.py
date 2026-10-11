"""Cutscene from this session's real words and existing graph, without model IO."""
from dataclasses import asdict
from datetime import datetime

from .cutscene_model import DaySummary, Thought, LadderStep, build_scene


def scene_for_session(session_key, rows, notes, sand, universe=None):
    start = min(r["started_epoch"] for r in rows)
    end = max(r.get("finished_epoch") or r["deadline_epoch"] for r in rows)
    own = [Thought(f"Мысль {i + 1}", n.comment) for i, n in enumerate(notes)]
    with sand.db() as db:
        guesses = db.execute("SELECT id,hypothesis,status FROM predictions WHERE created_epoch BETWEEN ? AND ? ORDER BY created_epoch", (start, end)).fetchall()
    own += [Thought(f"Догадка {i + 1}", r["hypothesis"],
                    "err" if r["status"] in ("contradicted", "not_supported") else
                    "vague" if r["status"] == "vague" else "own") for i, r in enumerate(guesses)]
    own += [Thought(f"Шаг {i + 1}", r["takeaway"]) for i, r in enumerate(rows) if r.get("takeaway")]
    for pair in sand.versions(start, end):
        match = next((t for t in own if t.text == pair["original"]), None)
        if match:
            match.kind = "vague"
            match.versions = [pair["original"], pair["bolder"]]
    concepts, links, comet = [], [], False
    if universe is not None:
        from .universe import _ro
        since = datetime.fromtimestamp(start).astimezone().isoformat()
        until = datetime.fromtimestamp(end).astimezone().isoformat()
        # UTC lexical comparisons would be wrong across offsets: use julianday.
        named = _ro(universe.path, "SELECT tc.concept,t.text,t.kind,t.verdict FROM thought_concepts tc JOIN thoughts t ON t.id=tc.thought_id WHERE julianday(t.created_utc) BETWEEN julianday(?) AND julianday(?) AND t.hidden=0 AND coalesce(t.about_system,0)=0 ORDER BY t.created_utc", (since, until))
        for concept, text, kind, verdict in named:
            if concept not in concepts and len(concepts) < 16:
                concepts.append(concept)
                match = next((t for t in own if t.text == text), None)
                if match:
                    match.concept = concept
                else:
                    own.append(Thought(concept, text, "err" if verdict == "contradicted" else "own"))
        links = [(a, b) for a, b in _ro(universe.path, "SELECT DISTINCT l.a,l.b FROM links l JOIN thoughts t ON t.id=l.thought_id WHERE julianday(t.created_utc) BETWEEN julianday(?) AND julianday(?) AND t.hidden=0 AND coalesce(t.about_system,0)=0", (since, until)) if a in concepts and b in concepts]
        older = {r[0] for r in _ro(universe.path, "SELECT DISTINCT tc.concept FROM thought_concepts tc JOIN thoughts t ON t.id=tc.thought_id WHERE julianday(t.created_utc)<julianday(?) AND t.hidden=0 AND coalesce(t.about_system,0)=0", (since,))}
        comet = any((a in older) != (b in older) for a,b in links)
    # Unnamed words remain words; don't invent semantic graph edges between them.
    for thought in own[:16]:
        if thought.concept not in concepts and len(concepts) < 16:
            concepts.append(thought.concept)
    edge = sand.edge(session_key)
    edge_guesses = sand.guesses(edge["id"]) if edge else []
    last_close = next((g for g in reversed(edge_guesses) if g["kind"] == "close"), None)
    rungs = sand.rungs(session_key)
    day = DaySummary(date=f"{datetime.fromtimestamp(start).date()}:{session_key}",
        concepts=concepts, links=links, thoughts=own,
        steps_yes=sum(r.get("outcome") == "done" for r in rows),
        steps_partial=sum(r.get("outcome") in ("partly", "not") for r in rows),
        ladder=[LadderStep(r["target"], r["status"]) for r in rungs],
        far_edge=edge["text"] if edge else None,
        # Solid line means concordance with the source, not mastery.
        far_edge_attached=bool(last_close and last_close["verdict"] == "confirmed"), comet=comet)
    scene = asdict(build_scene(day))
    scene["session_key"] = session_key
    scene["graph_links"] = links
    scene["guesses"] = edge_guesses
    if edge and scene["stars"]:
        scene["stars"][-1]["text"] = ("Догадка согласуется с источником. Это не оценка освоения.\n" if day.far_edge_attached else "Далёкое ребро\n") + "\n\n".join(g["text"] for g in edge_guesses)
    return scene
