"""GTK sand rendered with the existing system SVG loader, no Python cairo."""
from __future__ import annotations

import html
import json
import math
import os
import time
from pathlib import Path

from .sand_core import Sand, sample_mask

FRAME_MS = 34  # below 30 fps, independent of the monitor's refresh rate


def reduced_motion(Gtk):
    settings = Gtk.Settings.get_default()
    root = Path(os.environ.get("COGNITIVE_STATE_DIR", "var"))
    return (root / "sand.reduced").exists() or os.environ.get("COGNITIVE_REDUCED_MOTION", "").lower() in {"1", "on", "yes", "true"} or bool(settings and not settings.get_property("gtk-enable-animations"))


def svg_particles(sand, width=360, height=160):
    dots = ''.join(f'<circle cx="{p.x:.1f}" cy="{p.y:.1f}" r="{p.size:.1f}" fill="{"#d6ad79" if p.mine else "#e8d9bd"}" opacity="{max(0,min(1,p.alpha)):.2f}"/>' for p in sand.particles)
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">{dots}</svg>'


def raster(GdkPixbuf, svg):
    loader = GdkPixbuf.PixbufLoader.new_with_type("svg")
    loader.write(svg.encode())
    loader.close()
    return loader.get_pixbuf()


def show_effect(payload):
    """Short settle after a saved note: grains fall into a point and fade."""
    import gi
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk, Gdk
    from .popup_helper import get_mouse_position, _emit
    from .sand_canvas import connect_draw, rgb
    sand = Sand(int(payload.get("seed", 0)))
    reduced = reduced_motion(Gtk)
    sand.reduced_motion = reduced
    sand.lift([(40 + (i % 50) * 5.3, 95 + (i // 50) * 6) for i in range(180)], mine=True)
    sand.settle((315, 110))
    window = Gtk.Window(title="Песок")
    from .sand_surface import configure
    configure(window, Gtk, Gdk, force=True)
    window.set_wmclass("cognitive-sand", "cognitive-sand")
    window.set_decorated(False)
    window.set_resizable(False)
    window.set_keep_above(True)
    window.set_skip_taskbar_hint(True)
    window.set_skip_pager_hint(True)
    window.set_focus_on_map(False)
    window.set_accept_focus(False)
    area = Gtk.DrawingArea()
    area.set_size_request(360, 160)
    window.add(area)
    colours = {"mine": rgb("#d6ad79"), "dust": rgb("#e8d9bd")}

    def paint(painter):
        painter.clear()
        buckets = [[] for _ in range(10)]
        for p in sand.particles:
            alpha = max(0.0, min(1.0, p.alpha))
            if alpha > .02:
                buckets[min(9, int(alpha * 10))].append((p.x, p.y, p.size))
        painter.grains(buckets, colours["mine"], 10)

    handle = connect_draw(area, paint)
    x, y = (payload.get("x"), payload.get("y"))
    if x is None or y is None:
        x, y = get_mouse_position()
    window.move(max(0, int(x) - 180), max(0, int(y) - 80))
    state = {"last": None, "started": None, "closed": False}

    def close(*_):
        if state["closed"]:
            return False
        state["closed"] = True
        _emit("window_close", window="sand", detail="settle")
        window.destroy()
        Gtk.main_quit()
        return False

    def tick(_widget, clock):
        now = clock.get_frame_time() / 1e6
        state["started"] = state["started"] or now
        dt = 1.0 if state["last"] is None else min(2.5, max(.05, (now - state["last"]) * 60))
        state["last"] = now
        sand.step(dt)
        area.queue_draw()
        if now - state["started"] > .75 or reduced:
            close()
            return False
        return True

    window.connect("delete-event", close)
    window.show_all()
    _emit("window_open", window="sand", detail="settle")
    area.add_tick_callback(tick)
    Gtk.main()
    del handle
    return 0


def scene_svg(scene, elapsed, width=900, height=520):
    timeline = scene["timeline"]
    stage = 0 if elapsed < timeline["lift"] else 1 if elapsed < timeline["form"] else 2
    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">']
    stars = scene.get("stars", [])
    if scene.get("nebula"):
        parts.append('<ellipse cx="450" cy="360" rx="180" ry="70" fill="#a88468" opacity=".07"/>')
    if scene.get("comet") and stage == 1:
        parts.append('<path d="M80 120 Q260 30 540 150" fill="none" stroke="#e8bd83" stroke-width="2" opacity=".5"/>')
    for a, b, dashed in scene.get("edges", []):
        if stage == 2 and elapsed >= max(stars[a]["ignite"], stars[b]["ignite"]):
            p, q = stars[a], stars[b]
            dash = ' stroke-dasharray="4 6"' if dashed else ''
            parts.append(f'<line x1="{p["x"]*width}" y1="{p["y"]*height}" x2="{q["x"]*width}" y2="{q["y"]*height}" stroke="#85775e" opacity=".6"{dash}/>')
    for i, star in enumerate(stars):
        if stage == 2 and elapsed < star["ignite"]:
            continue
        x, y = star["x"] * width, star["y"] * height
        if stage < 2:
            t = 0 if stage == 0 else min(1, (elapsed-timeline["lift"])/(timeline["form"]-timeline["lift"]))
            y = (1-t)*(height*.82 + math.sin(i*2)*15) + t*y
            x += math.sin(i+elapsed)*12*t*(1-t)
        kind = star["kind"]
        color = "#e8bd83" if kind in ("err", "gap") else scene.get("tint", "#e8d9bd")
        radius = 5 if stage == 2 else 3
        if kind in ("err", "gap", "far") and stage == 2:
            parts.append(f'<circle cx="{x}" cy="{y}" r="12" fill="none" stroke="{color}" opacity=".6"/>')
        if kind == 'initium' and stage == 2:
            identity = scene.get('initium', {})
            # The same tint and ray rotation as the compact live star.
            for k in range(identity.get('rays',4)):
                angle = identity.get('rotation',.37)+k*math.tau/identity.get('rays',4)
                parts.append(f'<path d="M{x} {y} L{x+math.cos(angle)*19} {y+math.sin(angle)*19}" stroke="{color}" stroke-width=".7" opacity=".5"/>')
            for r,a in ((24,.025),(16,.045),(10,.08)):
                parts.append(f'<circle cx="{x}" cy="{y}" r="{r}" fill="{color}" opacity="{a}"/>')
        parts.append(f'<circle cx="{x}" cy="{y}" r="{radius}" fill="{color}"/>')
    title = "per aspera" if stage == 0 else "ветер" if stage == 1 else "ad astra"
    parts.append(f'<text x="32" y="42" fill="#e8d9bd" font-size="24">{title}</text>')
    if elapsed >= timeline["title"]:
        parts.append(f'<text x="32" y="{height-38}" fill="#e8d9bd" font-size="20">{html.escape(scene.get("name", ""))} · {html.escape(scene.get("form_name", ""))}</text>')
    parts.append('</svg>')
    return ''.join(parts)


def show_scene(payload):
    import gi
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk, Gdk, GLib, GdkPixbuf
    from .popup_helper import _emit
    from . import sound
    scene = payload.get("scene") or {}
    if not scene.get("timeline"):
        return 0
    window = Gtk.Window(title="per aspera ad astra")
    from .sand_surface import configure
    configure(window, Gtk, Gdk, force=True)
    window.set_wmclass("cognitive-popup", "cognitive-popup")
    window.set_decorated(False)
    window.set_keep_above(True)
    width, height = 900, 520
    monitor = window.get_screen().get_primary_monitor()
    area = window.get_screen().get_monitor_workarea(max(0, monitor))
    width, height = min(width, area.width-40), min(height, area.height-40)
    window.move(area.x+(area.width-width)//2, area.y+(area.height-height)//2)
    overlay = Gtk.Overlay()
    image = Gtk.Image()
    overlay.add(image)
    tip = Gtk.Label(label="Любая клавиша: к финалу. Ещё одна: закрыть.")
    tip.set_halign(Gtk.Align.CENTER)
    tip.set_valign(Gtk.Align.END)
    overlay.add_overlay(tip)
    window.add(overlay)
    state = {"started": time.monotonic(), "final": reduced_motion(Gtk), "source": None, "notes": set(), "closed": False, "audio": [], "acts": set()}
    duration = scene["timeline"]["end"]

    def stop_audio():
        for process in state["audio"]:
            if process and process.poll() is None:
                process.terminate()
        state["audio"].clear()

    def play(process):
        if process:
            state["audio"].append(process)

    def close(*_):
        stop_audio()
        state["closed"] = True
        if state["source"]:
            GLib.source_remove(state["source"])
            state["source"] = None
        window.destroy()
        Gtk.main_quit()
        return True

    def tick():
        elapsed = duration if state["final"] else time.monotonic()-state["started"]
        image.set_from_pixbuf(raster(GdkPixbuf, scene_svg(scene, elapsed, width, height)))
        if not state["final"]:
            from .cutscene_model import note_midi
            for act, when in ((0, 0), (1, scene["timeline"]["lift"]), (2, scene["timeline"]["form"]), (3, scene["timeline"]["title"])):
                if act not in state["acts"] and elapsed >= when:
                    state["acts"].add(act)
                    if act == 1:
                        play(sound.play_sand_asset("wind"))
                    elif act in (0, 3):
                        mode = scene["modes"][0 if act == 0 else 2]
                        for degree in (0, 2, 4):
                            play(sound.play_sand_asset(f"pad_{note_midi(scene['root_midi'], mode, degree, 0)}"))
            for i, star in enumerate(scene.get("stars", [])):
                for j, (delay, degree, octave) in enumerate(star.get("notes", [])):
                    if (i,j) not in state["notes"] and elapsed >= star["ignite"]+delay:
                        state["notes"].add((i,j))
                        play(sound.play_sand_note(note_midi(scene["root_midi"], scene["modes"][2], degree, octave), soft=star["kind"] == "vague" and j == 0))
        if elapsed >= duration:
            state["final"] = True
            state["source"] = None
            return False
        return True

    def skip(*_):
        stop_audio()
        if state["final"]:
            return close()
        state["final"] = True
        if state["source"]:
            GLib.source_remove(state["source"])
            state["source"] = None
        tick()
        return True

    def hover(_, event):
        if not state["final"]:
            return False
        stars = scene.get("stars", [])
        nearest = min(stars, key=lambda s: math.hypot(s["x"]*width-event.x, s["y"]*height-event.y), default=None)
        if nearest and math.hypot(nearest["x"]*width-event.x, nearest["y"]*height-event.y) < 24:
            tip.set_text(nearest["label"] + "\n" + nearest.get("text", "") + "\n" + "\n".join(nearest.get("versions", [])))
        else:
            tip.set_text("Любая клавиша: закрыть")
        return False

    window.add_events(Gdk.EventMask.POINTER_MOTION_MASK)
    window.connect("motion-notify-event", hover)
    window.connect("key-press-event", skip)
    window.connect("delete-event", close)
    window.show_all()
    window.present()
    _emit("window_open", window="cutscene", detail=scene.get("session_key", ""))
    if state["final"]:
        tick()
    else:
        state["source"] = GLib.timeout_add(FRAME_MS, tick)
    Gtk.main()
    _emit("window_close", window="cutscene")
    return 0
