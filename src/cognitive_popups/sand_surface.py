"""Transparent owned GTK surfaces and source-aware sand colours, no bitmap assets."""
import os
from pathlib import Path


def enabled():
    root = Path(os.environ.get("COGNITIVE_STATE_DIR") or "~/.local/state/cognitive-popups").expanduser()
    return (root / "sand.enabled").exists()


CARDS = {"light": (247, 243, 235), "dark": (26, 27, 31)}


def _pointer(Gdk):
    """Pointer in window pixels. Hyprland is asked first: GTK's idea of the
    pointer goes stale while it is over a Wayland window."""
    import json
    import subprocess
    try:
        x, y = (float(v) for v in subprocess.run(["hyprctl", "cursorpos"], capture_output=True, text=True,
                                                 timeout=1).stdout.replace(" ", "").split(",", 1))
        monitors = json.loads(subprocess.run(["hyprctl", "monitors", "-j"], capture_output=True, text=True,
                                             timeout=1).stdout)
        for m in monitors:
            if m["x"] <= x < m["x"] + m["width"] / m["scale"] and m["y"] <= y < m["y"] + m["height"] / m["scale"]:
                return m["x"] + (x - m["x"]) * m["scale"], m["y"] + (y - m["y"]) * m["scale"]
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        pass
    display = Gdk.Display.get_default()
    seat = display.get_default_seat() if display else None
    pointer = seat.get_pointer() if seat else None
    if pointer:
        _, x, y = pointer.get_position()
        return x, y
    return None


def configure(window, Gtk, Gdk, *, force=False, card=False):
    """Sand look for an owned window.

    ``card`` (owner, 10.10.2026): text windows get a nearly opaque rounded card
    in the page's tone, dark over a dark page and light over a light one, so
    long text always reads. Without it the window stays fully transparent
    (the sand windows draw their own legibility).
    """
    if not force and not enabled():
        return
    visual = window.get_screen().get_rgba_visual()
    if visual and not window.get_realized():
        window.set_visual(visual)
    window.set_app_paintable(not card)
    window.set_decorated(False)
    colours = DARK
    pointer = _pointer(Gdk)
    if pointer:
        colours = palette(Gdk, *pointer)
    provider = Gtk.CssProvider()
    provider.load_from_data(_css(colours, card).encode())
    def apply(widget):
        widget.get_style_context().add_provider(provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 100)
        if isinstance(widget, Gtk.Container):
            for child in widget.get_children():
                apply(child)

    placed = {"at": None}

    def retone(x, y, width, height):
        # The page under the card, not under the pointer, decides its tone
        # (the screenshot is cached from before the window appeared).
        if card and width > 1 and height > 1 and placed["at"] != (x, y):
            placed["at"] = (x, y)
            provider.load_from_data(_css(palette(Gdk, x, y, width, height), card).encode())

    def realized(*_):
        x, y = window.get_position()
        retone(x, y, *window.get_size())
        apply(window)

    def moved(_window, event):
        # Many popups are shown first and moved to their place right after.
        retone(event.x, event.y, event.width, event.height)
        return False

    apply(window)
    # Children are built after this call; apply to the completed tree before map.
    window.connect("realize", realized)
    if card:
        window.connect("configure-event", moved)


def _css(colours, card):
    if card:
        r, g, b = CARDS["light" if colours is LIGHT else "dark"]
        return ('* { background-color: transparent; background-image: none; box-shadow: none; } '
                f'window {{ background-color: rgb({r},{g},{b}); border-radius: 12px; '
                'border: 1px solid rgba(218,189,140,.28); } '
                f'label, textview, textview text, entry, button {{ color: {colours["text"]}; }} '
                'button { border-color: rgba(218,189,140,.45); border-radius: 6px; } '
                'entry, textview { border-color: rgba(218,189,140,.3); } '
                'selection { background-color: rgba(218,189,140,.35); }')
    return ('* { background-color: transparent; background-image: none; box-shadow: none; } '
            f'label, textview, entry, button {{ color: {colours["text"]}; text-shadow: 0 1px 2px {colours["shadow"]}; }} '
            'button { border-color: rgba(218,189,140,.45); border-radius: 2px; } '
            'entry, textview { border-color: rgba(218,189,140,.3); } '
            'selection { background-color: rgba(218,189,140,.35); }')


LIGHT = {"text":"#3e3020", "grain":"#78603e", "shadow":"#ffffff"}
DARK = {"text":"#eee1c7", "grain":"#d6ad79", "shadow":"#151515"}


def _ppm_brightness(data, box):
    """Mean luminance of a binary PPM inside ``box`` given as fractions (x0, y0, x1, y1)."""
    parts, index = [], 0
    while len(parts) < 4:
        end = data.index(b"\n", index) if data[index:index + 1] != b"#" else None
        if end is None:
            index = data.index(b"\n", index) + 1
            continue
        parts += data[index:end].split()
        index = end + 1
    width, height = int(parts[1]), int(parts[2])
    pixels = data[index:]
    x0, y0 = int(box[0] * width), int(box[1] * height)
    x1, y1 = max(x0 + 1, int(box[2] * width)), max(y0 + 1, int(box[3] * height))
    total = count = 0
    for y in range(max(0, y0), min(height, y1)):
        for x in range(max(0, x0), min(width, x1)):
            i = (y * width + x) * 3
            total += .2126 * pixels[i] + .7152 * pixels[i + 1] + .0722 * pixels[i + 2]
            count += 1
    return total / count if count else None


_SHOT = {}


def _wayland_brightness(Gdk, x, y, width, height):
    """XWayland cannot see Wayland windows; ask the compositor for a tiny screenshot."""
    import shutil
    import subprocess
    if not shutil.which("grim"):
        return None
    screen = Gdk.Screen.get_default()
    sw, sh = screen.get_width(), screen.get_height()
    import time
    cached = _SHOT.get("data")
    if cached and time.monotonic() - _SHOT["at"] < 2:
        data = cached
    else:
        data = subprocess.run(["grim", "-s", "0.06", "-t", "ppm", "-"], capture_output=True, timeout=1).stdout
        _SHOT.update(data=data, at=time.monotonic())
    if not data:
        return None
    return _ppm_brightness(data, (x / sw, y / sh, (x + width) / sw, (y + height) / sh))


def palette(Gdk, x, y, width=8, height=8):
    """Only local pixels, never uploaded or stored; fall back to the dark desktop.

    With a size, (x, y) is the top-left of the area the sand will cover: the
    whole area decides, not the few pixels under the pointer.
    """
    if width == height == 8:
        x, y = x - 4, y - 4
    try:
        brightness = _wayland_brightness(Gdk, x, y, width, height)
        if brightness is not None:
            return LIGHT if brightness > 150 else DARK
    except Exception:  # noqa: BLE001 - colour is a nicety, never a failure
        pass
    try:
        root = Gdk.get_default_root_window()
        image = Gdk.pixbuf_get_from_window(root, max(0,int(x)), max(0,int(y)), max(1,int(width)), max(1,int(height)))
        if width > 32 or height > 32:
            from gi.repository import GdkPixbuf
            image = image.scale_simple(32, 24, GdkPixbuf.InterpType.BILINEAR)
        pixels, channels = image.get_pixels(), image.get_n_channels()
        brightness = sum(.2126*pixels[i]+.7152*pixels[i+1]+.0722*pixels[i+2]
                         for i in range(0,len(pixels),channels)) / (len(pixels)//channels)
        if brightness > 150:
            return LIGHT
    except (AttributeError, TypeError, ValueError):
        pass
    return DARK
