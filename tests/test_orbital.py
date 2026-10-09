import pytest

from cognitive_popups.orbital import Disclosure, background_svg, blob_points, layout


@pytest.mark.parametrize('area,anchor', [
    ((0, 0, 1920, 1080), (1900, 1060)),
    ((-1920, -200, 1920, 1080), (-1910, -190)),
    ((100, 200, 320, 240), (9000, -9000)),
    ((0, 0, 1, 1), (0, 0)),
])
def test_footprint_and_cues_stay_in_workarea(area, anchor):
    orbit = layout(area, anchor)
    box = orbit.footprint
    x, y, w, h = area
    assert x <= box.x <= x + w - box.width
    assert y <= box.y <= y + h - box.height
    assert 0 < box.width <= min(800, w)
    assert 0 < box.height <= min(720, h)
    assert len(orbit.cues) == 4
    for cue in (*orbit.cues, *orbit.details):
        assert 0 <= cue.x <= box.width - cue.width
        assert 0 <= cue.y <= box.height - cue.height
        for px, py in blob_points(cue):
            assert cue.x <= px <= cue.x + cue.width
            assert cue.y <= py <= cue.y + cue.height


def test_geometry_deterministic_and_nonoverlapping():
    a = layout((0, 0, 1920, 1080), (100, 100))
    assert a == layout((0, 0, 1920, 1080), (100, 100))
    for i, rect in enumerate(a.cues):
        assert blob_points(rect, i) == blob_points(rect, i)
        for other in a.cues[i + 1:]:
            assert (rect.x + rect.width <= other.x or other.x + other.width <= rect.x
                    or rect.y + rect.height <= other.y or other.y + other.height <= rect.y)


def test_disclosure_order_and_only_one_active():
    rows = [(str(i), 'term ' + str(i), 'meaning ' + str(i)) for i in range(4)]
    state = Disclosure(rows)
    assert state.visible() == tuple((r[0], '', '') for r in rows)
    assert state.advance(2) == ('cue_select', 'term_reveal')
    assert state.visible()[2] == ('2', 'term 2', '')
    assert state.advance(2) == ('meaning_reveal',)
    assert state.visible()[2] == tuple(rows[2])
    assert state.advance(2) == ('cue_collapse',)
    assert state.active is None and state.layer == 1
    assert state.visible() == tuple((r[0], '', '') for r in rows)
    assert state.advance(2) == ('cue_select', 'term_reveal')
    assert state.advance(0) == ('cue_select', 'term_reveal')
    assert state.visible()[2] == ('2', '', '')
    assert sum(bool(term) for _, term, _ in state.visible()) == 1
    assert state.advance(-1) == state.advance(4) == ()
    assert state.active == 0


def test_disclosure_without_meaning_collapses_after_term():
    state = Disclosure([('a', 'term', ''), ('b', 't', 'm'), ('c', 't', 'm'), ('d', 't', 'm')])
    assert state.advance(0) == ('cue_select', 'term_reveal')
    assert state.advance(0) == ('cue_collapse',)
    assert state.active is None


def test_background_has_four_packaged_blobs_and_tiny_nucleus():
    from xml.etree import ElementTree as ET
    orbit = layout((0, 0, 1920, 1080), (960, 540))
    ns = {'s': 'http://www.w3.org/2000/svg'}
    initial = ET.fromstring(background_svg(orbit))
    selected = ET.fromstring(background_svg(orbit, 2))
    before = [p for p in initial.findall('s:image', ns) if p.get('data-cue') is not None]
    after = [p for p in selected.findall('s:image', ns) if p.get('data-focus') is None]
    assert len(before) == 4 and len(after) == 5
    assert [p.attrib for p in before] == [dict(p.attrib, opacity='1') for p in after[1:]]
    assert all(p.get('href').startswith('data:image/png;base64,') for p in before)
    assert initial.find('s:circle', ns).get('r') == '2'


def test_approved_radial_geometry():
    orbit = layout((0, 0, 1920, 1080), (960, 540))
    cx, cy = orbit.nucleus
    for r, (dx, dy) in zip(orbit.cues, ((0,-128),(128,0),(0,128),(-128,0))):
        assert abs(r.x+r.width/2-cx-dx) <= .5
        assert abs(r.y+r.height/2-cy-dy) <= .5
    assert all((r.width,r.height)==(115,105) for r in orbit.cues)
    assert all(r.width<=280 and r.height<=230 for r in orbit.details)


def test_nucleus_is_cursor_origin_except_for_edge_shift():
    orbit = layout((-1920, 0, 1920, 1080), (-960, 540))
    assert (orbit.footprint.x + orbit.nucleus[0],
            orbit.footprint.y + orbit.nucleus[1]) == (-960, 540)
    edge = layout((-1920, 0, 1920, 1080), (-1910, 5))
    assert edge.footprint.x == -1920
    assert edge.footprint.y == 0
    assert edge.nucleus == orbit.nucleus


def test_each_detail_is_outward_and_does_not_cover_other_cues():
    orbit = layout((0, 0, 1920, 1080), (960, 540))
    for i, detail in enumerate(orbit.details):
        cue = orbit.cues[i]
        cx,cy = orbit.nucleus
        if i == 0:
            assert detail.y+detail.height < cy
        elif i == 1:
            assert detail.x > cx
        elif i == 2:
            assert detail.y > cy
        else:
            assert detail.x+detail.width < cx
        for j,other in enumerate(orbit.cues):
            if j == i:
                continue
            assert (detail.y+detail.height <= other.y or other.y+other.height <= detail.y
                    or detail.x+detail.width <= other.x or other.x+other.width <= detail.x)


@pytest.mark.parametrize('mode,peak', [('off', None), ('subtle', '0.14'),
                                     ('normal', '0.28'), ('strong', '0.46')])
def test_focus_levels_local_grayscale(mode, peak):
    from xml.etree import ElementTree as ET
    ns = {'s': 'http://www.w3.org/2000/svg'}
    root = ET.fromstring(background_svg(layout((0, 0, 1920, 1080), (800, 500)), focus=mode))
    fields = [p for p in root.findall('s:image', ns) if p.get('data-focus') is not None]
    assert len(fields) == (0 if peak is None else 1)
    assert root.find('s:ellipse', ns) is None
    assert root.find('s:defs/s:radialGradient', ns) is None
    if peak is not None:
        from cognitive_popups.orbital_assets import focus_pixels
        assert fields[0].get('data-focus') == mode
        pixels = focus_pixels(800, 720, float(peak))
        assert max(pixels[3::4]) <= round(255 * float(peak))
        assert not any(pixels[0::4])
        assert not any(pixels[1::4])
        assert not any(pixels[2::4])
    for node in root.iter():
        for value in node.attrib.values():
            if value.startswith('#'):
                assert value[1:3] == value[3:5] == value[5:7]


def test_focus_fades_outward_without_hole_ring_or_hard_edge():
    from cognitive_popups.orbital_assets import focus_pixels
    w, h = 800, 720
    pixels = focus_pixels(w, h, .14)
    assert pixels is focus_pixels(w, h, .14)
    def alpha(x, y):
        return pixels[(y * w + x) * 4 + 3]
    for dx, dy in ((1, 0), (0, 1), (1, 1)):
        values = [alpha(w//2 + dx*d, h//2 + dy*d) for d in range(280)]
        assert values[0] == round(255 * .14)
        assert values == sorted(values, reverse=True)
        assert max(a-b for a,b in zip(values, values[1:])) <= 1
        assert values[-1] == 0
    assert not any(pixels[3:w*4:4])
    assert not any(pixels[(h-1)*w*4+3::4])
    assert all(alpha(0,y) == alpha(w-1,y) == 0 for y in range(h))


def test_focus_png_encodes_the_alpha_field_without_nested_gradients():
    import base64
    import struct
    import zlib
    from cognitive_popups.orbital_assets import focus_image_uri, focus_pixels
    w, h = 80, 72
    png = base64.b64decode(focus_image_uri(w,h,.14).split(',',1)[1])
    assert png[:8] == b'\x89PNG\r\n\x1a\n'
    offset, compressed = 8, b''
    while offset < len(png):
        size = struct.unpack('!I',png[offset:offset+4])[0]
        kind = png[offset+4:offset+8]
        data = png[offset+8:offset+8+size]
        assert struct.unpack('!I',png[offset+8+size:offset+12+size])[0] == zlib.crc32(kind+data)
        if kind == b'IDAT':
            compressed += data
        offset += size+12
    scanlines = zlib.decompress(compressed)
    assert all(scanlines[y*(w*4+1)] == 0 for y in range(h))
    assert b''.join(scanlines[y*(w*4+1)+1:(y+1)*(w*4+1)] for y in range(h)) == focus_pixels(w,h,.14)


def test_alpha_masks_are_cached_tight_runs_not_footprint_rectangles():
    from cognitive_popups.orbital_assets import alpha_runs
    class Pixels:
        reads = 0
        def get_pixels(self):
            self.reads += 1
            return bytes(channel for alpha in (0, 99, 100, 255, 0, 180, 0)
                         for channel in (0, 0, 0, alpha))
        def get_rowstride(self):
            return 28
        def get_n_channels(self):
            return 4
        def get_width(self):
            return 7
        def get_height(self):
            return 1
    raster = Pixels()
    runs = alpha_runs(raster)
    assert runs == ((2, 0, 2, 1), (5, 0, 1, 1))
    assert alpha_runs(raster) is runs
    assert raster.reads == 1


def test_safe_text_rectangles_are_inside_each_membrane_allocation():
    from cognitive_popups.orbital import safe_rect
    orbit = layout((0,0,1920,1080),(960,540))
    for cue_mode, rectangles in ((True,orbit.cues),(False,orbit.details)):
        for i,r in enumerate(rectangles):
            safe = safe_rect(r,i,cue=cue_mode)
            assert r.x < safe.x < safe.x+safe.width < r.x+r.width
            assert r.y < safe.y < safe.y+safe.height < r.y+r.height
