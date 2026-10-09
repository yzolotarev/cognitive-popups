import pytest

from cognitive_popups.popup_helper import (
    _focus_halo, _focus_halo_extent, _focus_halo_mode, _focus_halo_pixels,
    estimate_keys_size, estimate_text_size,
)


def test_focus_halo_is_disabled_by_default(monkeypatch):
    monkeypatch.delenv('COGNITIVE_POPUP_HALO', raising=False)
    assert _focus_halo_mode() == 'off'
    assert _focus_halo({}) is None


@pytest.mark.parametrize('monitor,extent', [(1920, 640), (3840, 640), (1280, 426)])
def test_focus_halo_extent_from_each_edge(monitor, extent):
    assert _focus_halo_extent(monitor) == extent


@pytest.mark.parametrize('mode', ['off', '', 'invalid'])
def test_focus_halo_disabled_modes(monkeypatch, mode):
    monkeypatch.setenv('COGNITIVE_POPUP_HALO', mode)
    assert _focus_halo({}) is None


@pytest.mark.parametrize('mode,peak', [('dark', 209), ('light', 98)])
@pytest.mark.parametrize('popup_width,popup_height', [(160, 120), (161, 121)])
def test_focus_halo_never_paints_over_popup(mode, peak, popup_width, popup_height):
    width = height = 600
    pixels = _focus_halo_pixels(width, height, popup_width, popup_height, mode)
    left = (width - popup_width) // 2
    top = (height - popup_height) // 2
    for y in range(top, top + popup_height):
        start = (y * width + left) * 4 + 3
        assert not any(pixels[start:start + popup_width * 4:4])
    extent = min(left, top)
    assert pixels[(top * width + left - 1) * 4 + 3] == int(peak * (1 - 1 / extent) ** 4)
    assert pixels[3] == 0


def test_shadow_falloff_is_radial_not_rectangular():
    extent = 640
    content = 20
    width = content + 2 * extent
    pixels = _focus_halo_pixels(width, width, content, content, 'dark')

    def alpha(x, y):
        return pixels[(y * width + x) * 4 + 3]

    center = extent + content // 2
    values = [alpha(extent - distance, center) for distance in (1, 160, 320, 480, 640)]
    assert values == sorted(values, reverse=True)
    assert values[-1] == 0
    assert values[2] == int(209 * 0.5 ** 4)
    assert alpha(extent - 160, extent - 160) < alpha(extent - 160, center)
    assert alpha(extent - 480, extent - 480) == 0
    assert not any(pixels[3:width * 4:4])
    assert not any(pixels[(width - 1) * width * 4 + 3::4])
    assert all(alpha(0, y) == alpha(width - 1, y) == 0 for y in range(width))


def test_prediction_result_uses_expanded_text_geometry():
    text = (
        "Гипотеза: цветная метка на коробке всегда позволяет найти нужную деталь, "
        "даже когда коробки переставили на другую полку\n\n"
        "Результат: частично подтверждено\n\n"
        "В тексте: Each box has a coloured label, but the inventory number is needed "
        "when two boxes share the same colour.\n\n"
        "Расхождение: цвет помогает сузить поиск, но при совпадении меток нужно "
        "дополнительно проверить номер коробки"
    )

    compact_width, _ = estimate_text_size(text)
    expanded_width, expanded_height = estimate_text_size(text, expanded=True)

    assert compact_width <= 560
    assert 420 <= expanded_width < 760
    assert expanded_height < 526


def test_prediction_size_shrinks_for_shorter_text():
    short_width, short_height = estimate_text_size(
        "Гипотеза: A\n\nРезультат: частично подтверждено\n\nВ тексте: B\n\nРасхождение: C",
        expanded=True,
    )
    long_width, long_height = estimate_text_size(
        "Гипотеза: " + "длинная формулировка " * 30 +
        "\n\nРезультат: частично подтверждено\n\nВ тексте: " +
        "source evidence " * 30 + "\n\nРасхождение: " + "объяснение " * 30,
        expanded=True,
    )

    assert short_width <= long_width
    assert short_height <= long_height


def test_keys_reference_grows_with_rows_but_stays_a_reading_column():
    rows = [
        ("Alt+W", "четыре слова из выделенного текста"),
        ("Alt+Shift+T", "новая задача по старому материалу"),
    ]
    width, height = estimate_keys_size(rows)

    assert 320 <= width <= 560
    assert height > 0
    # More rows means a taller window, up to the cap.
    assert estimate_keys_size(rows * 6)[1] > height
    # One very long action wraps instead of widening the window without bound.
    long_width, _ = estimate_keys_size([("Alt+K", "длинный текст " * 40)])
    assert long_width <= 560


def test_keys_size_survives_an_empty_reference():
    width, height = estimate_keys_size([])

    assert width >= 320
    assert height > 0


def test_prediction_size_respects_small_monitor():
    width, height = estimate_text_size(
        "Гипотеза: " + "длинный текст " * 100,
        expanded=True,
        available_width=900,
        available_height=500,
    )

    assert width <= 680
    assert height + 34 <= 500 - 48


def test_dual_dispatch_passes_existing_payload_and_motion(monkeypatch):
    from cognitive_popups import popup_helper as helper
    payload = helper.orbital_demo_payload()
    payload.update(x=-100, y=200, reduced_motion=True)
    calls = []
    monkeypatch.setattr(helper, 'show_orbital_popup', lambda *a, **kw: calls.append((a, kw)) or 7)
    assert helper._dispatch(payload, 'dual') == 7
    assert calls == [((payload['items'], -100, 200, 'Orbital fixture'),
                      {'reduced_motion': True})]


def test_non_four_cues_keep_legacy_dual(monkeypatch):
    from cognitive_popups import popup_helper as helper
    calls = []
    monkeypatch.setattr(helper, 'show_dual_popup', lambda *a: calls.append(a) or 9)
    items = [{'simple': 'one', 'term': 'a', 'meaning': 'b'}]
    assert helper.show_orbital_popup(items, 1, 2, 'legacy') == 9
    assert calls == [(items, 1, 2, 'legacy')]


def test_prediction_delta_and_secondary_evidence_dispatch(monkeypatch):
    from cognitive_popups import popup_helper as helper
    calls = []
    monkeypatch.setattr(helper, 'show_text_popup', lambda *a, **kw: calls.append((a, kw)) or 0)
    helper._dispatch({'text': 'legacy body', 'one_delta': 'one discrepancy',
                      'evidence': 'source quotation', 'title': 'Моя гипотеза'}, 'text')
    args, kwargs = calls.pop()
    assert args[0] == 'one discrepancy'
    assert kwargs['evidence'] == 'source quotation'
    assert not kwargs['expanded']
    helper._dispatch({'text': 'legacy body'}, 'text')
    args, kwargs = calls.pop()
    assert args[0] == 'legacy body'
    assert kwargs['evidence'] == ''


def test_orbital_demo_has_deterministic_four_cues():
    from cognitive_popups import popup_helper as helper
    payload = helper.orbital_demo_payload()
    assert payload == helper.orbital_demo_payload()
    assert [r[0] for r in helper.orbital_rows(payload['items'])] == ['change', 'rule', 'step', 'error']


def test_semantic_sound_optional_and_failure_safe(monkeypatch):
    from cognitive_popups import popup_helper as helper
    calls = []
    monkeypatch.setattr(helper.sound, 'play', calls.append, raising=False)
    helper._semantic_sound('term_reveal')
    assert calls == ['term_reveal']
    def broken(role):
        raise RuntimeError('player unavailable')
    monkeypatch.setattr(helper.sound, 'play', broken)
    helper._semantic_sound('meaning_reveal')
    monkeypatch.delattr(helper.sound, 'play')
    helper._semantic_sound('term_reveal')


def test_orbital_observation_records_layers_and_silent_close(monkeypatch):
    from types import SimpleNamespace
    from cognitive_popups import popup_helper as helper
    sounds, events, artifacts, presentations = [], [], [], []
    class Store:
        def __init__(self, **kwargs):
            pass
        def record_artifact(self, kind, content, **kwargs):
            artifacts.append((kind, content))
            return str(len(artifacts))
        def record_presentation(self, artifact, **kwargs):
            presentations.append(kwargs)
    chain = SimpleNamespace(log=SimpleNamespace(enabled=False), context=None,
                            emit=lambda event, **kw: events.append((event, kw)))
    monkeypatch.setattr(helper.observation, 'ObservationStore', Store)
    monkeypatch.setattr(helper, 'play_close_sound', lambda: sounds.append('close'))
    obs = helper.PopupObservation(chain, {})
    obs.content = {'cues': [('change', '', '')], 'active': None}
    obs.emit('window_open', window='dual')
    obs.emit('orbital_open', window='dual')
    obs.content = {'cues': [('change', 'transition', '')], 'active': 0}
    obs.emit('cue_select', window='dual', item_index=0)
    obs.emit('term_reveal', window='dual', item_index=0)
    obs.emit('meaning_reveal', window='dual', item_index=0)
    obs.emit('evidence_reveal', window='text')
    obs.emit('window_close', window='dual', silent=True)
    obs.emit('window_close', window='dual', silent=True)
    assert not sounds
    assert len(events) == len(presentations) == 7
    assert all('window_instance_id' in fields for _, fields in events)
    assert all('silent' not in fields for _, fields in events)
    assert presentations[3]['payload']['rendered']['active'] == 0


def test_legacy_observation_close_still_plays_once(monkeypatch):
    from types import SimpleNamespace
    from cognitive_popups import popup_helper as helper
    sounds = []
    chain = SimpleNamespace(log=SimpleNamespace(enabled=False), context=None,
                            emit=lambda *a, **kw: None)
    monkeypatch.setattr(helper, 'play_close_sound', lambda: sounds.append('close'))
    obs = helper.PopupObservation(chain, {})
    obs.emit('window_close', window='text')
    obs.emit('window_open', window='text')
    obs.emit('window_close', window='text')
    obs.emit('window_close', window='text')
    assert sounds == ['close']


def test_monitor_workarea_uses_logical_monitor_api():
    from types import SimpleNamespace
    from cognitive_popups import popup_helper as helper
    area = SimpleNamespace(x=-1600, y=100, width=1600, height=850)
    monitor = SimpleNamespace(get_workarea=lambda: area)
    display = SimpleNamespace(get_monitor_at_point=lambda x, y: monitor)
    gdk = SimpleNamespace(Display=SimpleNamespace(get_default=lambda: display))
    assert helper._monitor_workarea(gdk, -500, 200) == (-1600, 100, 1600, 850)


def test_demo_main_uses_dispatch_without_reading_stdin_or_halo(monkeypatch):
    from types import SimpleNamespace
    from cognitive_popups import popup_helper as helper
    payloads = []
    monkeypatch.setattr(helper.sys, 'argv', ['popup_helper', '--demo-orbital'])
    monkeypatch.setattr(helper, 'load_payload', lambda: pytest.fail('read stdin'))
    monkeypatch.setattr(helper, 'PopupObservation', lambda *a: SimpleNamespace(opened=False))
    monkeypatch.setattr(helper, '_emit', lambda *a, **kw: None)
    monkeypatch.setattr(helper, '_focus_halo', lambda *a: pytest.fail('outside halo'))
    monkeypatch.setattr(helper, '_dispatch', lambda p, m: payloads.append((p, m)) or 0)
    monkeypatch.setattr(helper, '_PRESENTATION', None)
    assert helper.main() == 0
    assert payloads == [(helper.orbital_demo_payload(), 'dual')]


def test_fill_readable_reuses_existing_tag():
    from types import SimpleNamespace
    from cognitive_popups import popup_helper as helper
    tag = object()
    applied = []
    buffer = SimpleNamespace(set_text=lambda text: None,
        get_tag_table=lambda: SimpleNamespace(lookup=lambda name: tag),
        create_tag=lambda *a, **kw: pytest.fail('duplicate tag'),
        get_iter_at_offset=lambda offset: offset,
        apply_tag=lambda *args: applied.append(args))
    helper.fill_readable(SimpleNamespace(get_buffer=lambda: buffer), 'Delta: one difference')
    assert applied == [(tag, 0, 6)]


@pytest.mark.parametrize('value,expected', [(None, 'subtle'), ('off', 'off'),
    ('normal', 'normal'), ('strong', 'strong'), (' SUBTLE ', 'subtle'), ('typo', 'subtle')])
def test_orbital_focus_environment(monkeypatch, value, expected):
    from cognitive_popups import popup_helper as helper
    if value is None:
        monkeypatch.delenv('COGNITIVE_ORBITAL_FOCUS', raising=False)
    else:
        monkeypatch.setenv('COGNITIVE_ORBITAL_FOCUS', value)
    assert helper._orbital_focus_mode() == expected


def test_orbital_typography_is_not_resized_on_disclosure():
    from cognitive_popups import popup_helper as helper
    assert helper.ORBITAL_CUE_PX == 18
    assert helper.ORBITAL_BODY_PX == 15


@pytest.mark.parametrize('display_name', ['wayland-1', ':0'])
def test_orbital_placement_only_moves_own_mapped_window(monkeypatch, display_name):
    import json
    from types import SimpleNamespace
    from cognitive_popups import popup_helper as helper
    calls = []
    window = SimpleNamespace(move=lambda x, y: calls.append(('gtk', x, y)),
        get_display=lambda: SimpleNamespace(get_name=lambda: display_name),
        get_title=lambda: 'Orbital fixture')
    clients = [{'pid': 99999, 'title': 'Orbital fixture', 'mapped': True, 'address': 'other'},
               {'pid': helper.os.getpid(), 'title': 'Orbital fixture', 'mapped': True,
                'address': 'own'}]
    monkeypatch.setenv('HYPRLAND_INSTANCE_SIGNATURE', 'test')
    monkeypatch.setattr(helper.subprocess, 'check_output', lambda *a, **kw: json.dumps(clients))
    monkeypatch.setattr(helper.subprocess, 'run', lambda args, **kw: calls.append(args))
    helper._place_orbital(window, helper.orbital.Rect(-600, 100, 600, 420))
    assert calls[0] == ('gtk', -600, 100)
    assert len(calls) == 2  # One bounded compositor call, never a polling loop.
    assert calls[1][:2] == ['hyprctl', 'eval']
    code = calls[1][2]
    for prop, value in (('border_size', '0'), ('no_blur', 'true'),
                        ('no_shadow', 'true'), ('opaque', 'false')):
        assert ('hl.dispatch(hl.dsp.window.set_prop({window="address:own",prop="'
                + prop + '",value="' + value + '"}))') in code
    assert 'hl.dispatch(hl.dsp.window.move({window="address:own",x=-600,y=100,relative=false}))' in code
    assert 'hl.dispatch(hl.dsp.window.tag({window="address:own",tag="+hyprglass_disabled"}))' in code
    assert 'other' not in code
    clients.pop()
    calls.clear()
    helper._place_orbital(window, helper.orbital.Rect(100, 200, 600, 420))
    assert calls == [('gtk', 100, 200)]


def test_orbital_placement_does_not_require_compositor_cli(monkeypatch):
    from types import SimpleNamespace
    from cognitive_popups import popup_helper as helper
    calls = []
    window = SimpleNamespace(move=lambda *a: calls.append(a))
    monkeypatch.delenv('HYPRLAND_INSTANCE_SIGNATURE', raising=False)
    monkeypatch.setattr(helper.subprocess, 'check_output', lambda *a, **kw: pytest.fail('CLI'))
    helper._place_orbital(window, helper.orbital.Rect(5, 6, 600, 420))
    assert calls == [(5, 6)]


def test_demo_matches_approved_algebra_terms():
    from cognitive_popups.popup_helper import orbital_demo_payload
    assert [r['term'] for r in orbital_demo_payload()['items']] == [
        'automorphism', 'structure-preserving', 'compose', 'not bijective']


@pytest.mark.parametrize('reduced', [True, False])
@pytest.mark.parametrize('index', [None, 0, 1, 2, 3, 'long'])
def test_live_orbital_bounded_disclosure(monkeypatch, index, reduced):
    import os
    if os.environ.get('COGNITIVE_GTK_SMOKE') != '1':
        pytest.skip('opt-in live GTK smoke: COGNITIVE_GTK_SMOKE=1')
    import gi
    gi.require_version('Gtk', '3.0')
    from gi.repository import Gtk, Gdk, GLib
    from types import SimpleNamespace
    from cognitive_popups import popup_helper as helper
    from cognitive_popups.orbital import safe_rect
    sounds, failures = [], []
    monkeypatch.setattr(helper, '_semantic_sound', sounds.append)
    monkeypatch.setattr(helper, '_emit', lambda *a, **kw: None)
    payload = helper.orbital_demo_payload()
    if index == 'long':
        payload['items'][2]['term'] = 'explicitly typed long term ' * 100
        payload['items'][2]['meaning'] = 'complete meaning without semantic trimming ' * 100
    selected = 2 if index == 'long' else index
    def inspect():
        window = next(w for w in Gtk.Window.list_toplevels() if w.get_title() == payload['title'])
        try:
            assert tuple(window.get_size()) == (800, 720)
            assert GLib.get_prgname() == 'cognitive-popup'
            assert window.get_visual().get_depth() == 32
            if os.environ.get('HYPRLAND_INSTANCE_SIGNATURE'):
                import json
                clients = json.loads(helper.subprocess.check_output(
                    ['hyprctl', 'clients', '-j'], timeout=.5))
                client = next(c for c in clients if c.get('pid') == os.getpid()
                              and c.get('title') == payload['title'] and c.get('mapped'))
                assert client['class'] == 'cognitive-popup'
                assert client['xwayland'] == (os.environ.get('GDK_BACKEND') == 'x11')
                assert 'hyprglass_disabled' in client['tags']
                address = 'address:' + client['address']
                for prop, value in (('border_size', '0'), ('no_blur', 'true'),
                                    ('no_shadow', 'true'), ('opaque', 'false')):
                    assert helper.subprocess.check_output(
                        ['hyprctl', 'getprop', address, prop], timeout=.5,
                        text=True).strip() == value
            def inspect_background(widget):
                context = widget.get_style_context()
                for flags in (Gtk.StateFlags.NORMAL, Gtk.StateFlags.BACKDROP):
                    assert context.get_background_color(flags).alpha == 0
                assert widget.get_app_paintable()
                if isinstance(widget, Gtk.Container):
                    for child in widget.get_children():
                        inspect_background(child)
            inspect_background(window)
            canvas = window.canvas.get_pixbuf()
            assert canvas.get_has_alpha()
            pixels, stride = canvas.get_pixels(), canvas.get_rowstride()
            for x, y in ((0, 0), (799, 0), (0, 719), (799, 719)):
                assert pixels[y * stride + x * 4 + 3] == 0
            from cognitive_popups.orbital_assets import focus_pixels
            field = focus_pixels(800, 720, .15)
            assert field[(360 * 800 + 400) * 4 + 3] > 0
            assert field[3] == 0
            if selected is not None:
                safe = safe_rect(window.visual_geometry.details[selected], selected, padding=8)
                a = window.detail_scroll.get_allocation()
                assert (a.width, a.height) == (safe.width, safe.height)
                text = window.detail_label.get_text()
                assert payload['items'][selected]['term'].strip() in text
                assert payload['items'][selected]['meaning'].strip() in text
                if index == 'long':
                    adjustment = window.detail_scroll.get_vadjustment()
                    assert adjustment.get_upper() > adjustment.get_page_size()
                    adjustment.set_value(adjustment.get_upper()-adjustment.get_page_size())
            pb = Gdk.pixbuf_get_from_window(window.get_window(), 0, 0, 800, 720)
            assert pb is not None
            pb.savev('var/design-review/gtk-orbital-' + str(index) + '.png', 'png', [], [])
            if selected is not None:
                # One more press folds the open cue back; the window stays.
                window.key(None, SimpleNamespace(keyval=Gdk.KEY_1+selected))
                assert not window.closed
                assert not window.detail_holder.get_visible()
                if reduced:
                    assert window.membrane is None and window.collapsing is None
                    assert all(label.get_opacity() == 1. for label in window.labels)
                else:
                    assert window.collapsing == selected and window.rect_tween is not None
        except Exception as exc:
            failures.append(exc)
        finally:
            window.dismiss('smoke')
        return False
    def advance():
        window = next(w for w in Gtk.Window.list_toplevels() if w.get_title() == payload['title'])
        if selected is not None:
            event = SimpleNamespace(keyval=Gdk.KEY_1+selected)
            window.key(None,event)
            window.key(None,event)
        GLib.timeout_add(450, inspect)
        return False
    def deadline():
        failures.append(AssertionError('GTK smoke exceeded its four-second deadline'))
        for window in Gtk.Window.list_toplevels():
            if window.get_title() == payload['title']:
                window.dismiss('smoke_timeout')
        return False
    watchdog = GLib.timeout_add(4000, deadline)
    def when_ready():
        window = next(w for w in Gtk.Window.list_toplevels() if w.get_title() == payload['title'])
        if not window.revealed or window.reveal_tween:
            return True
        advance()
        return False
    GLib.timeout_add(30, when_ready)
    assert helper.show_orbital_popup(payload['items'], 900, 550, payload['title'], reduced_motion=reduced) == 0
    if not failures or 'deadline' not in str(failures[-1]):
        GLib.source_remove(watchdog)
    assert not failures, failures
    assert sounds == (['orbital_open', 'orbital_close'] if selected is None else
                      ['orbital_open', 'term_reveal', 'meaning_reveal', 'orbital_collapse',
                       'orbital_close'])


def test_live_orbital_compositor_transparent_corners(monkeypatch, tmp_path):
    import json
    import os
    import shutil
    if (os.environ.get('COGNITIVE_GTK_SMOKE') != '1'
            or not os.environ.get('HYPRLAND_INSTANCE_SIGNATURE')):
        pytest.skip('opt-in live Hyprland compositor smoke')
    if shutil.which('grim') is None:
        pytest.skip('grim is required for compositor readback')
    import gi
    gi.require_version('Gtk', '3.0')
    from gi.repository import Gtk, GLib, GdkPixbuf
    from cognitive_popups import popup_helper as helper
    monkeypatch.setattr(helper, '_emit', lambda *a, **kw: None)
    monkeypatch.setattr(helper, '_semantic_sound', lambda *a: None)
    monkeypatch.setenv('COGNITIVE_ORBITAL_FOCUS', 'subtle')
    GLib.set_prgname('cognitive-popup')
    background = Gtk.Window()
    background.set_wmclass('cognitive-popup', 'cognitive-popup')
    background.set_title('Owned compositor alpha background')
    background.set_default_size(1000, 850)
    css = Gtk.CssProvider()
    css.load_from_data(b'window { background-color: #e0b060; background-image: none; }')
    background.get_style_context().add_provider(css, Gtk.STYLE_PROVIDER_PRIORITY_USER)
    background.show_all()
    payload = helper.orbital_demo_payload()
    failures = []
    screenshot = tmp_path / ('orbital-' + os.environ.get('GDK_BACKEND', 'default') + '.png')

    def own_client(title):
        clients = json.loads(helper.subprocess.check_output(
            ['hyprctl', 'clients', '-j'], timeout=.5))
        return next(c for c in clients if c.get('pid') == os.getpid()
                    and c.get('title') == title and c.get('mapped'))

    def window():
        return next(w for w in Gtk.Window.list_toplevels() if w.get_title() == payload['title'])

    def close():
        window().dismiss('compositor_smoke')
        background.destroy()

    def inspect():
        try:
            client = own_client(payload['title'])
            assert client['xwayland'] == (os.environ.get('GDK_BACKEND') == 'x11')
            assert 'hyprglass_disabled' in client['tags']
            x, y = client['at']
            width, height = client['size']
            # XWayland's compositor size differs from GTK pixels at fractional scale.
            helper.subprocess.run(['grim', '-g', f'{x-20},{y-20} {width+40}x{height+40}',
                                   str(screenshot)], check=True, timeout=1)
            pb = GdkPixbuf.Pixbuf.new_from_file(str(screenshot))
            pixels, stride, channels = pb.get_pixels(), pb.get_rowstride(), pb.get_n_channels()
            sx, sy = pb.get_width() / (width+40), pb.get_height() / (height+40)

            def rgb(x, y):
                offset = round(y*sy)*stride + round(x*sx)*channels
                return tuple(pixels[offset:offset+3])

            corners = [(rgb(10, 40), rgb(40, 40)),
                       (rgb(width+30, 40), rgb(width, 40)),
                       (rgb(10, height), rgb(40, height)),
                       (rgb(width+30, height), rgb(width, height))]
            assert all(max(abs(a-b) for a, b in zip(outside, (224, 176, 96))) <= 2
                       for outside, inside in corners), corners
            assert all(max(abs(a-b) for a, b in zip(outside, inside)) <= 2
                       for outside, inside in corners), corners
            center = rgb(20+width/2, 20+height*400/720)
            assert center[0] < rgb(40, 40)[0] - 5, (center, rgb(40, 40))
        except Exception as exc:
            failures.append(exc)
        finally:
            close()
        return False

    def prepare():
        try:
            popup = own_client(payload['title'])
            bg = own_client(background.get_title())
            x, y = popup['at']
            address = json.dumps('address:' + bg['address'])
            commands = ['hl.dispatch(hl.dsp.window.tag({window=' + address
                        + ',tag="+hyprglass_disabled"}))']
            commands.extend('hl.dispatch(hl.dsp.window.set_prop({window=' + address
                            + ',prop=' + json.dumps(prop) + ',value=' + json.dumps(value) + '}))'
                            for prop, value in (('no_dim', 'true'), ('no_shadow', 'true'),
                                                ('border_size', '0'), ('opacity', '1'),
                                                ('opacity_inactive', '1'),
                                                ('opacity_inactive_override', '1')))
            commands.append('hl.dispatch(hl.dsp.window.move({window=' + address
                            + f',x={x-60},y={y-40},relative=false' + '}))')
            helper.subprocess.run(['hyprctl', 'eval', '(function() '
                                   + '; '.join(commands) + ' end)()'], check=True, timeout=.5)
            window().present()
            GLib.timeout_add(500, inspect)
        except Exception as exc:
            failures.append(exc)
            close()
        return False

    def deadline():
        failures.append(AssertionError('compositor smoke exceeded four-second deadline'))
        close()
        return False

    watchdog = GLib.timeout_add(4000, deadline)
    GLib.timeout_add(750, prepare)
    try:
        assert helper.show_orbital_popup(payload['items'], 900, 550, payload['title'],
                                         reduced_motion=True) == 0
    finally:
        background.destroy()
        if not failures or 'deadline' not in str(failures[-1]):
            GLib.source_remove(watchdog)
    assert not failures, failures


@pytest.mark.parametrize('reduced', [False, True])
def test_live_orbital_motion_lifecycle_and_rapid_retarget(monkeypatch, reduced):
    import os
    if os.environ.get('COGNITIVE_GTK_SMOKE') != '1':
        pytest.skip('opt-in normal/reduced GTK lifecycle smoke')
    import gi
    gi.require_version('Gtk', '3.0')
    from gi.repository import Gtk, GLib, Gdk
    from cognitive_popups import popup_helper as helper
    from cognitive_popups.orbital_motion import visual_rect
    from types import SimpleNamespace
    failures, sounds, events, placements, maps = [], [], [], [], []
    monkeypatch.setenv('COGNITIVE_DEBUG', '1')
    sound_boundaries, masks = [], []
    def semantic_sound(role):
        sounds.append(role)
        if role != 'orbital_open':
            w = own()
            sound_boundaries.append((helper.time.monotonic(), w.rect_tween))
    def emit(event, **kw):
        events.append(event)
        if event in ('term_reveal', 'meaning_reveal'):
            helper.time.sleep(.025)  # Regression: blocking semantic bookkeeping.
    monkeypatch.setattr(helper, '_semantic_sound', semantic_sound)
    monkeypatch.setattr(helper, '_emit', emit)
    from cognitive_popups import orbital_assets as assets
    original_shape = assets.apply_input_regions
    def shape(window, runs, width, height):
        def contains(x, y):
            return any(ry == y and rx <= x < rx+rw for rx, ry, rw, _ in runs)
        orbit = helper.orbital.layout((0, 0, width, height), (width//2, height//2))
        assert all(contains(c.x+c.width//2, c.y+c.height//2) for c in orbit.cues)
        assert not any(contains(x,y) for x,y in ((0,0),(width-1,0),(0,height-1),(width-1,height-1)))
        rows = {}
        for x,y,w,h in runs:
            assert h == 1
            rows.setdefault(y, []).append((x,x+w))
        area = 0
        for intervals in rows.values():
            right = -1
            for left, end in sorted(intervals):
                area += max(0, end-max(left,right))
                right = max(right,end)
        assert area < width*height*.5
        masks.append(area)
        own().debug('mask_validation', area=area, footprint_fraction=round(area/(width*height), 4))
        return original_shape(window, runs, width, height)
    monkeypatch.setattr(assets, 'apply_input_regions', shape)
    original_place = helper._place_orbital
    def place(window, bounds, **kw):
        placements.append((window.get_opacity(), window.revealed))
        assert window.get_opacity() == 0 and not window.revealed
        return original_place(window, bounds, **kw)
    monkeypatch.setattr(helper, '_place_orbital', place)
    original_show = Gtk.Window.show_all
    def show(window):
        if window.get_title() == 'Orbital lifecycle test':
            maps.append(window.get_opacity())
        return original_show(window)
    monkeypatch.setattr(Gtk.Window, 'show_all', show)
    payload = helper.orbital_demo_payload()
    payload['items'][2]['meaning'] = 'Long scrolling meaning ' * 200
    def own():
        return next(w for w in Gtk.Window.list_toplevels() if w.get_title() == 'Orbital lifecycle test')
    placement_count = []
    initial_snapshot = []
    def click(index):
        w = own()
        old_tween = w.rect_tween
        count = len(sound_boundaries)
        w.key(None, SimpleNamespace(keyval=Gdk.KEY_1+index))
        if len(sound_boundaries) > count:
            played, tween_at_play = sound_boundaries[-1]
            assert tween_at_play is old_tween
            if not reduced:
                assert w.rect_tween is not old_tween
                assert w.rect_tween.started >= played
                assert w.rect_tween.started == w.dim_tween.started
        return False
    def interrupt():
        window = own()
        if not reduced:
            sampled = visual_rect(window.rect_tween.value(helper.time.monotonic()))
            tick_source = window.motion_source
            click(0)
            assert window.motion_source == tick_source
            assert window.rect_tween.start[0] == pytest.approx(sampled.x, abs=2)
            assert not window.detail_holder.get_visible()
        else:
            click(0)
        click(1)
        click(2)
        click(2)
        return False
    def finish():
        window = own()
        try:
            assert len(placements) == placement_count[0]
            if initial_snapshot[0] is not None:
                assert helper._orbital_placement_snapshot(window) == initial_snapshot[0]
            assert window.get_opacity() == 1
            assert window.rect_tween is None
            assert window.motion_source is None
            assert window.detail_holder.get_visible()
            assert window.detail_holder.get_opacity() == 1
            assert 'Long scrolling meaning' in window.detail_label.get_text()
            scroll = window.detail_scroll.get_vadjustment()
            assert scroll.get_upper() > scroll.get_page_size()
            assert not hasattr(window, 'fade_source')
            assert masks
            log = window.motion_log
            accepted = [r for r in log if r['event'] == 'click_accepted']
            played = [r for r in log if r['event'] == 'sound' and r.get('transition') is not None]
            visual = [r for r in log if r['event'] == 'first_disclosure_frame']
            assert len(accepted) == len(played) == 5
            assert all(r['call_ms'] >= a['accepted_ms'] + 25 for a,r in zip(accepted,played))
            assert visual and all(r['play_to_visual_ms'] >= 0 for r in visual)
            assert next(r for r in log if r['event'] == 'map')['opacity'] == 0
            assert all(r['opacity'] == 0 for r in log if r['event'] == 'placement')
            if reduced:
                assert not any(r['event'] == 'first_visible_tick' for r in log)
            else:
                reveal = next(i for i,r in enumerate(log) if r['event'] == 'reveal_start')
                sound = next(i for i,r in enumerate(log) if r['event'] == 'sound')
                first = next(i for i,r in enumerate(log) if r['event'] == 'first_visible_tick')
                assert reveal < sound < first
                opacity = [r['opacity'] for r in log if r['event'] == 'reveal_tick']
                assert opacity == sorted(opacity) and opacity[-1] == 1.
        except Exception as exc:
            failures.append(exc)
        finally:
            window.dismiss('motion_test')
            assert window.open_source is window.place_source is window.motion_source is None
        return False
    def ready():
        window = own()
        if not window.revealed or window.reveal_tween:
            return True
        placement_count.append(len(placements))
        initial_snapshot.append(helper._orbital_placement_snapshot(window) if os.environ.get('HYPRLAND_INSTANCE_SIGNATURE') else None)
        click(0)
        GLib.timeout_add(55, interrupt)
        GLib.timeout_add(700, finish)
        return False
    def deadline():
        failures.append(AssertionError('motion lifecycle exceeded five seconds'))
        own().dismiss('timeout')
        return False
    watchdog = GLib.timeout_add(5000, deadline)
    poll = GLib.timeout_add(25, ready)
    helper.show_orbital_popup(payload['items'], 900, 550, 'Orbital lifecycle test', reduced_motion=reduced)
    if not failures or 'five seconds' not in str(failures[-1]):
        GLib.source_remove(watchdog)
    assert not failures, failures
    assert maps == [0.]
    assert sounds == ['orbital_open', 'term_reveal', 'meaning_reveal', 'term_reveal', 'term_reveal',
                      'meaning_reveal', 'orbital_close']
    assert events.count('orbital_open') == 1
    assert events.count('term_reveal') == 3
    assert events.count('meaning_reveal') == 2


@pytest.mark.parametrize('phase', ['opened', 'placement', 'reveal', 'disclosure', 'text'])
def test_live_orbital_dismiss_cancels_every_owned_callback(monkeypatch, phase):
    import os
    if os.environ.get('COGNITIVE_GTK_SMOKE') != '1':
        pytest.skip('opt-in GTK cancellation smoke')
    import gi
    gi.require_version('Gtk', '3.0')
    from gi.repository import Gtk, GLib, Gdk
    from cognitive_popups import popup_helper as helper
    from types import SimpleNamespace
    monkeypatch.setattr(helper, '_semantic_sound', lambda *_: None)
    monkeypatch.setattr(helper, '_emit', lambda *a, **kw: None)
    held, failures = [], []
    selected = False
    def inspect():
        nonlocal selected
        window = next(w for w in Gtk.Window.list_toplevels() if w.get_title() == 'Orbital cancellation')
        reached = {'opened': True,
                   'placement': window.place_source is not None,
                   'reveal': window.reveal_tween is not None,
                   'disclosure': window.rect_tween is not None,
                   'text': window.text_tween is not None}[phase]
        if phase in ('disclosure', 'text') and window.revealed and not selected:
            window.key(None, SimpleNamespace(keyval=Gdk.KEY_1))
            selected = True
            return True
        if not reached:
            return True
        held.append(window)
        window.dismiss('cancel_' + phase)
        return False
    def deadline():
        failures.append('cancellation deadline')
        for w in Gtk.Window.list_toplevels():
            if w.get_title() == 'Orbital cancellation':
                w.dismiss('timeout')
        return False
    watchdog = GLib.timeout_add(4000, deadline)
    poll = GLib.timeout_add(1, inspect)
    helper.show_orbital_popup(helper.orbital_demo_payload()['items'], 900, 550,
                             'Orbital cancellation', reduced_motion=False)
    if not failures:
        GLib.source_remove(watchdog)
    assert not failures
    window = held[0]
    assert window.closed
    assert window.open_source is window.place_source is window.motion_source is None
    assert window.reveal_tween is window.rect_tween is window.text_tween is None
    # Drain the main context after destruction: none of the owned callbacks
    # may touch the destroyed widget or restart motion.
    context = GLib.MainContext.default()
    while context.pending():
        context.iteration(False)
    assert window.motion_source is None
