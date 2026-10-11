"""Explicit controls and replay of saved scenes; never starts model requests."""
import argparse
import json
import os
import re
import subprocess
from pathlib import Path

from .sand_store import SandStore


def sky(scenes):
    stars, edges, by_label = [], [], {}
    columns = min(3, max(1, len(scenes)))
    rows = max(1, (len(scenes) + columns - 1) // columns)
    for i, scene in enumerate(scenes):
        offset = len(stars)
        for star in scene.get("stars", []):
            star = dict(star, x=((i % columns) + star["x"])/columns,
                        y=((i // columns) + star["y"])/rows, ignite=0, notes=[])
            semantic = not re.fullmatch(r"(?:Мысль|Догадка|Шаг) \d+", star["label"])
            if semantic and star["label"] in by_label:
                edges.append((by_label[star["label"]], len(stars), True))
            if semantic:
                by_label[star["label"]] = len(stars)
            stars.append(star)
        edges.extend((a+offset, b+offset, dashed) for a,b,dashed in scene.get("edges", []))
    return {"name": "Небо сохранённых сессий", "form_name": f"{len(scenes)} созвездий",
            "stars": stars, "edges": edges, "timeline": {"lift": 0, "form": 0, "title": 0, "end": 0},
            "modes": ["ionian"]*3, "root_midi": 50, "tint": "#e8d9bd"}


def main(argv=None):
    parser = argparse.ArgumentParser(description="Песочный интерфейс: управление без запросов к модели")
    parser.add_argument("command", choices=("on", "off", "status", "reduced", "motion", "mute", "unmute", "grains", "text", "history", "replay", "sky"))
    parser.add_argument("session", nargs="?")
    args = parser.parse_args(argv)
    root = Path(os.environ.get("COGNITIVE_STATE_DIR") or Path(__file__).resolve().parents[2] / "var")
    root.mkdir(parents=True, exist_ok=True)
    if args.command in ("on", "off", "reduced", "motion"):
        marker = root / ("sand.enabled" if args.command in ("on", "off") else "sand.reduced")
        if args.command in ("on", "reduced"):
            marker.touch()
        else:
            marker.unlink(missing_ok=True)
        print("Включено" if args.command in ("on", "reduced") else "Выключено")
        return 0
    if args.command in ("mute", "unmute"):
        marker = root / "sand.silent"
        marker.touch() if args.command == "mute" else marker.unlink(missing_ok=True)
        print("Звук песка выключен" if args.command == "mute" else "Звук песка включён")
        return 0
    if args.command in ("grains", "text"):
        marker = root / "sand.grains_only"
        marker.touch() if args.command == "grains" else marker.unlink(missing_ok=True)
        print("Слова только из песка" if args.command == "grains" else "Слова проявляются текстом")
        return 0
    if args.command == "status":
        print(json.dumps({"enabled": (root/"sand.enabled").exists(), "reduced_motion": (root/"sand.reduced").exists(),
                          "sound": not (root/"sand.silent").exists(),
                          "words": "sand" if (root/"sand.grains_only").exists() else "text"}, ensure_ascii=False))
        return 0
    store = SandStore(root / "records.sqlite3")
    with store.db() as db:
        records = db.execute("SELECT session_key,payload FROM sand_scenes ORDER BY created_epoch DESC LIMIT 12").fetchall()
    if args.command == "history":
        for record in records:
            scene = json.loads(record["payload"])
            print(f'{record["session_key"]}  {scene.get("name", "")}  {len(scene.get("stars", []))} звёзд')
        return 0
    selected = next((r for r in records if not args.session or r["session_key"] == args.session), None)
    if selected is None:
        print("Сохранённых сцен пока нет.")
        return 1
    scene = sky([json.loads(r["payload"]) for r in reversed(records)]) if args.command == "sky" else json.loads(selected["payload"])
    env = dict(os.environ, COGNITIVE_STATE_DIR=str(root), GDK_BACKEND="x11")
    return subprocess.run(["/usr/bin/python", "-m", "cognitive_popups.popup_helper"],
                          input=json.dumps({"mode": "cutscene", "scene": scene}, ensure_ascii=False), text=True, env=env).returncode


if __name__ == "__main__":
    raise SystemExit(main())
