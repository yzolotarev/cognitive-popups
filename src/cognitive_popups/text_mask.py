"""Render the actual Pango layout to an alpha mask using native system libraries."""
import ctypes as C
from ctypes.util import find_library
from functools import lru_cache

@lru_cache(maxsize=1)
def api():
    cairo = C.CDLL(find_library("cairo"))
    pango = C.CDLL(find_library("pangocairo-1.0"))
    for name,args,result in (
        ("cairo_image_surface_create",[C.c_int,C.c_int,C.c_int],C.c_void_p),
        ("cairo_create",[C.c_void_p],C.c_void_p),
        ("cairo_set_source_rgba",[C.c_void_p]+[C.c_double]*4,None),
        ("cairo_move_to",[C.c_void_p,C.c_double,C.c_double],None),
        ("cairo_surface_flush",[C.c_void_p],None),
        ("cairo_image_surface_get_stride",[C.c_void_p],C.c_int),
        ("cairo_image_surface_get_data",[C.c_void_p],C.c_void_p),
        ("cairo_destroy",[C.c_void_p],None),
        ("cairo_surface_destroy",[C.c_void_p],None),
    ):
        function = getattr(cairo,name)
        function.argtypes, function.restype = args,result
    pango.pango_cairo_show_layout.argtypes = [C.c_void_p,C.c_void_p]
    pango.pango_cairo_show_layout.restype = None
    return cairo,pango

def render(layout, width, height, offsets=(0,0)):
    cairo,pango = api()
    width,height = max(1,int(width)),max(1,int(height))
    surface = cairo.cairo_image_surface_create(0,width,height)
    context = cairo.cairo_create(surface)
    try:
        cairo.cairo_set_source_rgba(context,1,1,1,1)
        cairo.cairo_move_to(context,*offsets)
        pango.pango_cairo_show_layout(context,hash(layout))
        cairo.cairo_surface_flush(surface)
        stride = cairo.cairo_image_surface_get_stride(surface)
        # White native BGRA has identical RGB channels; alpha remains the fourth.
        data = C.string_at(cairo.cairo_image_surface_get_data(surface),stride*height)
        return data,width,height,stride
    finally:
        cairo.cairo_destroy(context)
        cairo.cairo_surface_destroy(surface)
