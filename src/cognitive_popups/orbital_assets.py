"""Packaged procedural membranes; no imaging dependency at runtime."""
import base64
import json
import math
import struct
import zlib
from functools import lru_cache
from pathlib import Path

ROOT = Path(__file__).with_name('assets') / 'orbital'
RENDER_STATS = {'svg_parses': 0}

@lru_cache(maxsize=4)
def image_uri(index):
    return 'data:image/png;base64,' + base64.b64encode((ROOT / f'blob_{index + 1}.png').read_bytes()).decode('ascii')

@lru_cache(maxsize=2)
def safe_insets(cue=False):
    name = 'cue_safe.json' if cue else 'safe.json'
    return tuple(tuple(row) for row in json.loads((ROOT / name).read_text()))

@lru_cache(maxsize=8)
def focus_pixels(width, height, peak):
    """Pure-black local dim with a smooth, monotonic elliptical alpha field.

    The squared taper reaches zero with zero slope at the boundary. Keeping
    RGB zero also avoids unpremultiplication bands at very low alpha in SVG
    loaders and window readback. Nothing samples or blurs desktop content.
    """
    width, height = max(1, width), max(1, height)
    cx, cy = width / 2, height / 2
    rx, ry = max(.5, width * .4), max(.5, height * .4)
    pixels = bytearray(width * height * 4)
    xs = [((x + .5 - cx) / rx) ** 2 for x in range(width)]
    for y in range(height):
        dy = ((y + .5 - cy) / ry) ** 2
        for x, dx in enumerate(xs):
            radius2 = dx + dy
            if radius2 < 1:
                alpha = peak * math.exp(-3 * radius2) * (1 - radius2) ** 2
                pixels[(y * width + x) * 4 + 3] = round(255 * alpha)
    return bytes(pixels)


@lru_cache(maxsize=8)
def focus_image_uri(width, height, peak):
    """Encode the cached raster using standard-library PNG, no Pillow."""
    pixels = focus_pixels(width, height, peak)
    def chunk(kind, data):
        return (struct.pack('!I', len(data)) + kind + data
                + struct.pack('!I', zlib.crc32(kind + data)))
    scanlines = b''.join(b'\x00' + pixels[y * width * 4:(y + 1) * width * 4]
                         for y in range(height))
    png = (b'\x89PNG\r\n\x1a\n'
           + chunk(b'IHDR', struct.pack('!2I5B', width, height, 8, 6, 0, 0, 0))
           + chunk(b'IDAT', zlib.compress(scanlines)) + chunk(b'IEND', b''))
    return 'data:image/png;base64,' + base64.b64encode(png).decode('ascii')


@lru_cache(maxsize=40)
def rendered_pixbuf(orbit, active, focus):
    from gi.repository import GdkPixbuf
    from .orbital import background_svg
    import os
    if os.environ.get('COGNITIVE_DEBUG') == '1':
        RENDER_STATS['svg_parses'] += 1
    loader = GdkPixbuf.PixbufLoader.new_with_type('svg')
    loader.write(background_svg(orbit, active, focus).encode())
    loader.close()
    return loader.get_pixbuf()


@lru_cache(maxsize=4)
def source_pixbuf(index):
    from gi.repository import GdkPixbuf
    return GdkPixbuf.Pixbuf.new_from_file(str(ROOT / f'blob_{index + 1}.png'))


@lru_cache(maxsize=8)
def static_pixbuf(orbit, focus):
    from dataclasses import replace
    return rendered_pixbuf(replace(orbit, cues=()), None, focus)


@lru_cache(maxsize=32)
def cue_pixbuf(index, width, height):
    from gi.repository import GdkPixbuf
    return source_pixbuf(index).scale_simple(width, height, GdkPixbuf.InterpType.BILINEAR)


def composite_pixbuf(orbit, focus, active=None, membrane=None, dim=None):
    """Only raster scaling/compositing in frames; never parse SVG here."""
    from gi.repository import GdkPixbuf
    result = static_pixbuf(orbit, focus).copy()
    def paste(index, rect, alpha, cue=False):
        source = cue_pixbuf(index, rect.width, rect.height) if cue else source_pixbuf(index)
        source.composite(result, rect.x, rect.y, rect.width, rect.height,
                         rect.x, rect.y, rect.width / source.get_width(),
                         rect.height / source.get_height(), GdkPixbuf.InterpType.BILINEAR,
                         round(255 * alpha))
    if active is not None:
        paste(active, membrane or orbit.details[active], 1.)
    for index, rect in enumerate(orbit.cues):
        paste(index, rect, dim[index] if dim is not None else
              (1. if active in (None, index) else .62), cue=True)
    return result


@lru_cache(maxsize=32)
def alpha_runs(pixbuf):
    import re
    pixels, stride, channels = pixbuf.get_pixels(), pixbuf.get_rowstride(), pixbuf.get_n_channels()
    return tuple((run.start(), y, run.end()-run.start(), 1)
                 for y in range(pixbuf.get_height())
                 for run in re.finditer(b'[\\x64-\\xff]+',
                     pixels[y*stride+3:y*stride+pixbuf.get_width()*channels:channels]))


@lru_cache(maxsize=1)
def _shape_api():
    import ctypes as C
    from ctypes.util import find_library
    cairo = C.CDLL(find_library('cairo'))
    gdk = C.CDLL(find_library('gdk-3'))
    class Rectangle(C.Structure):
        _fields_ = [('x', C.c_int), ('y', C.c_int), ('width', C.c_int), ('height', C.c_int)]
    cairo.cairo_region_create.restype = C.c_void_p
    cairo.cairo_region_union_rectangle.argtypes = [C.c_void_p, C.POINTER(Rectangle)]
    cairo.cairo_region_destroy.argtypes = [C.c_void_p]
    gdk.gdk_window_input_shape_combine_region.argtypes = [C.c_void_p, C.c_void_p, C.c_int, C.c_int]
    cairo.cairo_region_subtract_rectangle.argtypes = [C.c_void_p, C.POINTER(Rectangle)]
    return C, cairo, gdk, Rectangle


@lru_cache(maxsize=64)
def input_runs(orbit, active=None, membrane=None):
    """Cached local masks: scan only each small membrane, not the footprint."""
    regions = list(enumerate(orbit.cues))
    if active is not None:
        regions.append((active, membrane or orbit.details[active]))
    return tuple((x + rect.x, y + rect.y, w, h)
                 for index, rect in regions
                 for x, y, w, h in alpha_runs(cue_pixbuf(index, rect.width, rect.height)))


def apply_input_regions(window, runs, width, height):
    C, cairo, gdk, Rectangle = _shape_api()
    region = cairo.cairo_region_create()
    try:
        for run in runs:
            cairo.cairo_region_union_rectangle(region, C.byref(Rectangle(*run)))
        cairo.cairo_region_subtract_rectangle(region, C.byref(Rectangle(
            width//2-3, height//2-3, 6, 6)))
        gdk.gdk_window_input_shape_combine_region(hash(window), region, 0, 0)
    finally:
        cairo.cairo_region_destroy(region)


def apply_input_shape(window, pixbuf):
    """Tight cached alpha masks, unioned only at transition boundaries.

    System Cairo/GDK C APIs avoid a Python Cairo dependency. GDK handles
    device scaling; the decorative focus field never captures input.
    """
    pixbufs = pixbuf if isinstance(pixbuf, tuple) else (pixbuf,)
    runs = tuple(run for raster in pixbufs for run in alpha_runs(raster))
    apply_input_regions(window, runs, pixbufs[0].get_width(), pixbufs[0].get_height())
