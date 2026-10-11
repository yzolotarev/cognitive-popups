"""Direct cairo drawing for GTK 3 without Python cairo: the system libraries via ctypes.

PyGObject cannot hand a ``cairo_t`` to Python without pycairo, so the ``draw``
signal is connected natively and the callback draws through libcairo. The
frame clock drives redraws at the compositor's rate; nothing is rasterised
to an intermediate image (the SVG path was the source of the stutter).
"""
from __future__ import annotations

import ctypes as C
import math
from ctypes.util import find_library
from functools import lru_cache

DRAW = C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p, C.c_void_p)
OP_CLEAR, OP_SOURCE, OP_OVER = 0, 1, 2
D = C.c_double


@lru_cache(maxsize=1)
def api():
    cairo = C.CDLL(find_library("cairo"))
    pango = C.CDLL(find_library("pangocairo-1.0"))
    gobject = C.CDLL(find_library("gobject-2.0"))
    for name, args in (
        ("cairo_set_source_rgba", [C.c_void_p, D, D, D, D]),
        ("cairo_rectangle", [C.c_void_p, D, D, D, D]),
        ("cairo_arc", [C.c_void_p, D, D, D, D, D]),
        ("cairo_move_to", [C.c_void_p, D, D]),
        ("cairo_line_to", [C.c_void_p, D, D]),
        ("cairo_curve_to", [C.c_void_p, D, D, D, D, D, D]),
        ("cairo_translate", [C.c_void_p, D, D]),
        ("cairo_scale", [C.c_void_p, D, D]),
        ("cairo_rotate", [C.c_void_p, D]),
        ("cairo_set_line_width", [C.c_void_p, D]),
        ("cairo_set_operator", [C.c_void_p, C.c_int]),
        ("cairo_fill", [C.c_void_p]),
        ("cairo_stroke", [C.c_void_p]),
        ("cairo_paint", [C.c_void_p]),
        ("cairo_paint_with_alpha", [C.c_void_p, D]),
        ("cairo_new_path", [C.c_void_p]),
        ("cairo_clip", [C.c_void_p]),
        ("cairo_save", [C.c_void_p]),
        ("cairo_restore", [C.c_void_p]),
        ("cairo_push_group", [C.c_void_p]),
        ("cairo_pop_group_to_source", [C.c_void_p]),
    ):
        function = getattr(cairo, name)
        function.argtypes, function.restype = args, None
    cairo.cairo_image_surface_create.argtypes = [C.c_int, C.c_int, C.c_int]
    cairo.cairo_image_surface_create.restype = C.c_void_p
    cairo.cairo_create.argtypes = [C.c_void_p]
    cairo.cairo_create.restype = C.c_void_p
    cairo.cairo_get_source.argtypes = [C.c_void_p]
    cairo.cairo_get_source.restype = C.c_void_p
    cairo.cairo_pattern_set_filter.argtypes = [C.c_void_p, C.c_int]
    cairo.cairo_pattern_set_filter.restype = None
    cairo.cairo_mask_surface.argtypes = [C.c_void_p, C.c_void_p, D, D]
    cairo.cairo_mask_surface.restype = None
    cairo.cairo_destroy.argtypes = [C.c_void_p]
    cairo.cairo_destroy.restype = None
    cairo.cairo_image_surface_create_for_data.argtypes = [C.c_void_p, C.c_int, C.c_int, C.c_int, C.c_int]
    cairo.cairo_image_surface_create_for_data.restype = C.c_void_p
    cairo.cairo_set_source_surface.argtypes = [C.c_void_p, C.c_void_p, D, D]
    cairo.cairo_set_source_surface.restype = None
    cairo.cairo_surface_destroy.argtypes = [C.c_void_p]
    cairo.cairo_surface_destroy.restype = None
    pango.pango_cairo_show_layout.argtypes = [C.c_void_p, C.c_void_p]
    pango.pango_cairo_show_layout.restype = None
    gobject.g_signal_connect_data.argtypes = [C.c_void_p, C.c_char_p, C.c_void_p, C.c_void_p, C.c_void_p, C.c_int]
    gobject.g_signal_connect_data.restype = C.c_ulong
    return cairo, pango, gobject


def rgb(colour: str) -> tuple[float, float, float]:
    colour = colour.lstrip("#")
    return tuple(int(colour[i:i + 2], 16) / 255 for i in (0, 2, 4))


class Image:
    """An ARGB32 cairo surface over Python-owned bytes (premultiplied BGRA rows)."""

    def __init__(self, data: bytearray, width: int, height: int):
        self.buffer = (C.c_ubyte * len(data)).from_buffer(data)
        self.data = data
        self.surface = api()[0].cairo_image_surface_create_for_data(
            C.addressof(self.buffer), 0, width, height, width * 4)

    def close(self):
        if self.surface:
            api()[0].cairo_surface_destroy(self.surface)
            self.surface = None


class Halo:
    """A soft glow around a layout's glyphs, rendered once into its own surface.

    Map labels and subtitles read on any background this way: the glow takes
    the page's own colour, dense at the letters and fading outward (owner,
    10.10.2026). Drawn every frame as one image, never re-rendered.
    """

    def __init__(self, layout, width, height, colour, radius=5):
        cairo, pango, _ = api()
        self.pad = pad = radius + 2
        self.width, self.height = int(width) + 2 * pad, int(height) + 2 * pad
        self.surface = cairo.cairo_image_surface_create(0, self.width, self.height)
        # Glyphs are shaped once, then spread horizontally and vertically (a
        # separable soft dilation): ~4r stamps instead of a ring of hundreds.
        glyphs = cairo.cairo_image_surface_create(0, self.width, self.height)
        wide = cairo.cairo_image_surface_create(0, self.width, self.height)
        contexts = [cairo.cairo_create(glyphs), cairo.cairo_create(wide), cairo.cairo_create(self.surface)]
        gcr, wcr, cr = contexts
        try:
            cairo.cairo_set_source_rgba(gcr, 1, 1, 1, 1)
            cairo.cairo_move_to(gcr, pad, pad)
            pango.pango_cairo_show_layout(gcr, hash(layout))
            # solid up to radius-3 (a real dilation), then a soft three-pixel edge
            edge = {radius - 2: .6, radius - 1: .35, radius: .15}
            weights = [(k, edge.get(abs(k), 1.0)) for k in range(-radius, radius + 1)]
            for k, weight in weights:
                cairo.cairo_set_source_rgba(wcr, 1, 1, 1, weight)
                cairo.cairo_mask_surface(wcr, glyphs, k, 0)
            for k, weight in weights:
                cairo.cairo_set_source_rgba(cr, *colour, weight)
                cairo.cairo_mask_surface(cr, wide, 0, k)
        finally:
            for context in contexts:
                cairo.cairo_destroy(context)
            cairo.cairo_surface_destroy(glyphs)
            cairo.cairo_surface_destroy(wide)

    def close(self):
        if self.surface:
            api()[0].cairo_surface_destroy(self.surface)
            self.surface = None


class GlowLayer:
    """The grains' halo at 1/``factor`` resolution, scaled up soft on paint.

    One small square per grain in a quarter-size surface; bilinear scaling
    blurs it into a glow. Cheap enough for every frame (the full-size
    version cost ~10 ms a frame).
    """

    def __init__(self, width, height, factor=4):
        cairo = api()[0]
        self.factor = factor
        self.width, self.height = max(1, int(width) // factor + 2), max(1, int(height) // factor + 2)
        self.surface = cairo.cairo_image_surface_create(0, self.width, self.height)
        self.cr = cairo.cairo_create(self.surface)

    def begin(self):
        c = api()[0]
        c.cairo_set_operator(self.cr, OP_CLEAR)
        c.cairo_paint(self.cr)
        c.cairo_set_operator(self.cr, OP_OVER)

    def add(self, points, colour, alpha=.8, size=3.2):
        c, cr, f = api()[0], self.cr, self.factor
        rect, half = c.cairo_rectangle, size / 2
        c.cairo_set_source_rgba(cr, *colour, alpha)
        for x, y in points:
            rect(cr, x / f - half, y / f - half, size, size)
        c.cairo_fill(cr)

    def close(self):
        c = api()[0]
        if self.surface:
            c.cairo_destroy(self.cr)
            c.cairo_surface_destroy(self.surface)
            self.surface = None


class Painter:
    """Thin drawing vocabulary over one ``cairo_t`` for the duration of a frame."""

    def __init__(self, cr):
        self.cr = cr
        self.c, self.p, _ = api()

    def clear(self):
        self.c.cairo_set_operator(self.cr, OP_CLEAR)
        self.c.cairo_paint(self.cr)
        self.c.cairo_set_operator(self.cr, OP_OVER)

    def grains(self, buckets, colour, levels):
        """``buckets[i]`` holds (x, y, size) drawn at alpha ``(i + 0.5) / levels``."""
        c, cr = self.c, self.cr
        rect, r, g, b = c.cairo_rectangle, *colour
        for i, bucket in enumerate(buckets):
            if not bucket:
                continue
            c.cairo_set_source_rgba(cr, r, g, b, (i + .5) / levels)
            for x, y, s in bucket:
                rect(cr, x, y, s, s)
            c.cairo_fill(cr)

    def glow_layer(self, layer, bbox, alpha=1.0):
        """Scale the low-resolution glow up, only over ``bbox`` (x0, y0, x1, y1)."""
        c, cr = self.c, self.cr
        x0, y0, x1, y1 = bbox
        c.cairo_save(cr)
        c.cairo_rectangle(cr, x0, y0, x1 - x0, y1 - y0)
        c.cairo_clip(cr)
        c.cairo_scale(cr, layer.factor, layer.factor)
        c.cairo_set_source_surface(cr, layer.surface, 0, 0)
        c.cairo_pattern_set_filter(c.cairo_get_source(cr), 4)  # bilinear: soft and cheap
        c.cairo_paint_with_alpha(cr, alpha)
        c.cairo_restore(cr)

    def glow(self, points, colour, layers=((9.0, .16), (5.0, .3))):
        """A soft page-coloured patch under each point: a halo that moves with the sand."""
        if not points:
            return
        c, cr = self.c, self.cr
        rect = c.cairo_rectangle
        for size, alpha in layers:
            half = size / 2
            c.cairo_set_source_rgba(cr, *colour, alpha)
            for x, y in points:
                rect(cr, x - half, y - half, size, size)
            c.cairo_fill(cr)

    def text(self, layout, x, y, colour, alpha, scale=1.0, angle=0.0, pivot=(0.0, 0.0), image=None, bbox=None):
        """Draw a layout; with ``image`` its glyphs are painted with that picture.

        ``bbox`` (x, y, w, h) bounds the work for the picture variant.
        """
        if alpha <= .003:
            return
        c, cr = self.c, self.cr
        if image is not None and image.surface:
            c.cairo_save(cr)
            if bbox:
                c.cairo_rectangle(cr, *bbox)
                c.cairo_clip(cr)
            c.cairo_push_group(cr)
            # The picture is locked to window space before the glyph transform.
            c.cairo_set_source_surface(cr, image.surface, 0, 0)
            self._place(x, y, scale, angle, pivot)
            c.cairo_move_to(cr, 0, 0)
            self.p.pango_cairo_show_layout(cr, hash(layout))
            c.cairo_pop_group_to_source(cr)
            c.cairo_paint_with_alpha(cr, alpha)
            c.cairo_restore(cr)
            return
        c.cairo_save(cr)
        if scale != 1.0 or angle:
            c.cairo_translate(cr, x + pivot[0], y + pivot[1])
            c.cairo_rotate(cr, angle)
            c.cairo_scale(cr, scale, scale)
            c.cairo_translate(cr, -pivot[0], -pivot[1])
        else:
            c.cairo_translate(cr, x, y)
        c.cairo_set_source_rgba(cr, *colour, alpha)
        c.cairo_move_to(cr, 0, 0)
        self.p.pango_cairo_show_layout(cr, hash(layout))
        c.cairo_restore(cr)

    def halo(self, halo, x, y, alpha, scale=1.0, angle=0.0, pivot=(0.0, 0.0)):
        """Paint a prepared halo whose layout would be drawn at (x, y)."""
        if alpha <= .003 or not halo.surface:
            return
        c, cr = self.c, self.cr
        c.cairo_save(cr)
        self._place(x, y, scale, angle, pivot)
        c.cairo_set_source_surface(cr, halo.surface, -halo.pad, -halo.pad)
        c.cairo_paint_with_alpha(cr, alpha)
        c.cairo_restore(cr)

    def _place(self, x, y, scale, angle, pivot):
        c, cr = self.c, self.cr
        if scale != 1.0 or angle:
            c.cairo_translate(cr, x + pivot[0], y + pivot[1])
            c.cairo_rotate(cr, angle)
            c.cairo_scale(cr, scale, scale)
            c.cairo_translate(cr, -pivot[0], -pivot[1])
        else:
            c.cairo_translate(cr, x, y)

    def ring(self, x, y, radius, colour, alpha, width=.75):
        c, cr = self.c, self.cr
        c.cairo_new_path(cr)
        c.cairo_set_source_rgba(cr, *colour, alpha)
        c.cairo_set_line_width(cr, width)
        c.cairo_arc(cr, x, y, radius, 0, 6.2832)
        c.cairo_stroke(cr)

    def plate(self, x, y, width, height, radius, colour, alpha):
        """Rounded backing in the page's colour (frosted fallback, off by default)."""
        if alpha <= .003:
            return
        c, cr = self.c, self.cr
        r = min(radius, width / 2, height / 2)
        c.cairo_new_path(cr)
        c.cairo_arc(cr, x + width - r, y + r, r, -1.5708, 0)
        c.cairo_arc(cr, x + width - r, y + height - r, r, 0, 1.5708)
        c.cairo_arc(cr, x + r, y + height - r, r, 1.5708, 3.1416)
        c.cairo_arc(cr, x + r, y + r, r, 3.1416, 4.7124)
        c.cairo_set_source_rgba(cr, *colour, alpha)
        c.cairo_fill(cr)

    def band(self, x, y, width, height, colour, alpha):
        c, cr = self.c, self.cr
        c.cairo_set_source_rgba(cr, *colour, alpha)
        c.cairo_rectangle(cr, x, y, width, height)
        c.cairo_fill(cr)

    def clip(self, x, y, width, height):
        self.c.cairo_save(self.cr)
        self.c.cairo_rectangle(self.cr, x, y, width, height)
        self.c.cairo_clip(self.cr)

    def unclip(self):
        self.c.cairo_restore(self.cr)

    def translate(self, dx, dy):
        self.c.cairo_translate(self.cr, dx, dy)


def connect_draw(widget, paint):
    """Call ``paint(Painter)`` on every ``draw``; keep the returned handle alive."""
    _, _, gobject = api()

    def on_draw(_widget, cr, _data):
        try:
            paint(Painter(cr))
        except Exception as exc:  # a ctypes callback must never raise into C
            import sys
            print(f"sand draw: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 1

    callback = DRAW(on_draw)
    gobject.g_signal_connect_data(hash(widget), b"draw", C.cast(callback, C.c_void_p), None, None, 0)
    return callback
