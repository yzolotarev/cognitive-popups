"""Native GTK input regions, using system Cairo rather than Python Cairo."""
import ctypes as C
from ctypes.util import find_library
from functools import lru_cache

class Rectangle(C.Structure):
    _fields_ = [(name, C.c_int) for name in ("x", "y", "width", "height")]

@lru_cache(maxsize=1)
def api():
    cairo = C.CDLL(find_library("cairo"))
    gdk = C.CDLL(find_library("gdk-3"))
    cairo.cairo_region_create.restype = C.c_void_p
    cairo.cairo_region_union_rectangle.argtypes = [C.c_void_p, C.POINTER(Rectangle)]
    cairo.cairo_region_destroy.argtypes = [C.c_void_p]
    gdk.gdk_window_input_shape_combine_region.argtypes = [C.c_void_p, C.c_void_p, C.c_int, C.c_int]
    return cairo, gdk

def apply(window, runs):
    if window is None:
        return
    cairo, gdk = api()
    region = cairo.cairo_region_create()
    try:
        for x, y, width, height in runs:
            rect = Rectangle(int(x), int(y), max(1,int(width)), max(1,int(height)))
            cairo.cairo_region_union_rectangle(region, C.byref(rect))
        gdk.gdk_window_input_shape_combine_region(hash(window), region, 0, 0)
    finally:
        cairo.cairo_region_destroy(region)
