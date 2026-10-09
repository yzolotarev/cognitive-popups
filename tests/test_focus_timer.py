"""Timer rendering and scheduling contracts without a desktop or Cairo binding."""
import sys
from types import SimpleNamespace
from xml.etree import ElementTree

from cognitive_popups.focus_timer import FocusTimer


def timer_fixture():
    timer = object.__new__(FocusTimer)
    timer.state = {"id": "block", "goal": "goal", "started_at": 0, "deadline": 900}
    timer.goal = "goal"
    timer.width, timer.height = timer.WIDTH, timer.HEIGHT
    timer.clock = lambda: 850
    timer.closed = timer.expired = False
    timer.source = timer.expiration_source = None
    events = []
    timer.GLib = SimpleNamespace(
        timeout_add=lambda delay, callback: events.append((delay, callback)) or 11,
        timeout_add_seconds=lambda delay, callback: events.append((delay, callback)) or 12,
        source_remove=lambda source: events.append(("remove", source)),
    )
    timer.area = SimpleNamespace()
    timer._refresh = lambda: events.append("refresh")
    timer._layout_labels = lambda: None
    timer.window = SimpleNamespace(get_mapped=lambda: False, destroy=lambda: events.append("destroy"))
    timer.on_expire = lambda block: events.append(("expire", block))
    timer._place = lambda: None
    return timer, events


def test_constructor_preserves_goal_and_passive_transparent_window(monkeypatch):
    timer, _ = timer_fixture()
    calls = []

    class Widget:
        def __getattr__(self, name):
            return lambda *args: calls.append((name, args))

    window, area = Widget(), Widget()
    window.get_screen = lambda: SimpleNamespace(get_rgba_visual=lambda: "rgba")
    window.get_style_context = lambda: SimpleNamespace(add_provider=lambda *args: None)
    created_labels = []

    def label():
        widget = Widget()
        widget.get_style_context = window.get_style_context
        created_labels.append(widget)
        return widget

    gtk = SimpleNamespace(
        Window=lambda **kwargs: window, Overlay=lambda: area,
        Image=Widget, Fixed=Widget, Label=label,
        CssProvider=Widget, STYLE_PROVIDER_PRIORITY_APPLICATION=600,
    )
    monkeypatch.setattr(FocusTimer, "_refresh", lambda self: None)
    gdk = SimpleNamespace(WindowTypeHint=SimpleNamespace(NOTIFICATION="notification"),
                          Display=SimpleNamespace(get_default=lambda: None),
                          EventMask=SimpleNamespace(ENTER_NOTIFY_MASK=1, LEAVE_NOTIFY_MASK=2))
    repository = SimpleNamespace(Gtk=gtk, GLib=timer.GLib, Gdk=gdk,
                                 Pango=SimpleNamespace(EllipsizeMode=SimpleNamespace(END="end")),
                                 GdkPixbuf=None)
    monkeypatch.setitem(sys.modules, "gi", SimpleNamespace(
        require_version=lambda *args: None,
        require_foreign=lambda name: calls.append(("foreign", (name,))),
    ))
    monkeypatch.setitem(sys.modules, "gi.repository", repository)

    goal = "  Понять  λ <b>точно</b>\nбез подмены  "
    actual = FocusTimer({**timer.state, "goal": goal}, lambda _: None, lambda _: None,
                        clock=lambda: 850)
    assert actual.goal == goal
    assert not any(name == "foreign" for name, _ in calls)
    assert not any(name == "connect" and args[0] == "draw" for name, args in calls)
    assert len(created_labels) == 2
    assert actual.goal_label is created_labels[1]
    assert ("set_text", (goal,)) in calls
    assert calls.count(("set_single_line_mode", (True,))) == 2
    assert calls.count(("set_use_markup", (False,))) == 2
    assert calls.count(("set_ellipsize", ("end",))) == 2
    assert calls.count(("set_max_width_chars", (1,))) == 2
    assert ("set_accept_focus", (False,)) in calls
    assert ("set_focus_on_map", (False,)) in calls
    assert ("set_keep_above", (True,)) in calls
    assert ("set_resizable", (False,)) in calls
    assert ("set_app_paintable", (True,)) in calls
    assert ("set_visual", ("rgba",)) in calls


def test_hidden_timer_keeps_only_expiration_and_resumes_ticks():
    timer, events = timer_fixture()
    timer._schedule_expiration()
    assert events[0][0] == 50000
    timer._mapped()
    assert timer.source == 12
    timer._unmapped()
    assert timer.source is None
    assert timer.expiration_source == 11
    assert timer._tick() is False
    timer.clock = lambda: 900
    timer._expire()
    timer._expire()
    assert events.count(("expire", "block")) == 1
    assert events.count("refresh") == 1  # Mapping only; hidden expiry does not redraw.
    timer._mapped()
    assert timer.source is None


def test_early_expiration_reschedules_and_close_removes_both_sources():
    timer, events = timer_fixture()
    assert timer._expire() is False
    assert not timer.expired
    assert timer.expiration_source == 11
    timer._mapped()
    timer.close()
    timer.close()
    assert events.count(("remove", 11)) == 1
    assert events.count(("remove", 12)) == 1
    assert events.count("destroy") == 1
    assert timer._expire() is False
    assert not any(isinstance(event, tuple) and event[0] == "expire" for event in events)


def test_position_uses_pointer_monitor_workarea_and_bounds():
    timer, _ = timer_fixture()
    moves, sizes, points = [], [], []
    workarea = SimpleNamespace(x=-1600, y=40, width=1600, height=860)
    monitor = SimpleNamespace(get_workarea=lambda: workarea)
    pointer = SimpleNamespace(get_position=lambda: (None, -600, 300))
    display = SimpleNamespace(
        get_default_seat=lambda: SimpleNamespace(get_pointer=lambda: pointer),
        get_monitor_at_point=lambda x, y: points.append((x, y)) or monitor,
    )
    timer.Gdk = SimpleNamespace(Display=SimpleNamespace(get_default=lambda: display))
    timer.area.set_size_request = lambda *size: sizes.append(size)
    timer.window.resize = lambda *size: sizes.append(size)
    timer.window.move = lambda *point: moves.append(point)
    FocusTimer._place(timer)
    assert points == [(-600, 300)]
    assert moves == [(-216, 56)]
    workarea.width, workarea.height = 80, 60
    FocusTimer._place(timer)
    assert sizes[-1] == (80, 60)
    assert moves[-1] == (-1600, 40)


def test_position_falls_back_to_primary_then_first_monitor():
    for has_pointer, has_primary in ((False, True), (True, True), (False, False)):
        timer, _ = timer_fixture()
        moves, first_monitor = [], []
        monitor = SimpleNamespace(get_workarea=lambda: SimpleNamespace(
            x=100, y=80, width=1920, height=1080))
        pointer = SimpleNamespace(get_position=lambda: (None, 0, 0))
        display = SimpleNamespace(
            get_default_seat=lambda: SimpleNamespace(get_pointer=lambda: pointer if has_pointer else None),
            get_monitor_at_point=lambda x, y: None,
            get_primary_monitor=lambda: monitor if has_primary else None,
            get_n_monitors=lambda: 1,
            get_monitor=lambda index: first_monitor.append(index) or monitor,
        )
        timer.Gdk = SimpleNamespace(Display=SimpleNamespace(get_default=lambda: display))
        timer.area.set_size_request = lambda *size: None
        timer.window.resize = lambda *size: None
        timer.window.move = lambda *point: moves.append(point)
        FocusTimer._place(timer)
        assert moves == [(1804, 96)]
        assert first_monitor == ([] if has_primary else [0])


def test_position_without_display_or_monitors_is_safe():
    timer, _ = timer_fixture()
    for display in (None, SimpleNamespace(
        get_default_seat=lambda: None, get_primary_monitor=lambda: None,
        get_n_monitors=lambda: 0,
    )):
        timer.Gdk = SimpleNamespace(Display=SimpleNamespace(get_default=lambda: display))
        FocusTimer._place(timer)


def test_svg_grayscale_rounded_surface_track_and_gentle_salience():
    early = ElementTree.fromstring(FocusTimer._background_svg(200, 132, 800, 900))
    late = ElementTree.fromstring(FocusTimer._background_svg(200, 132, 10, 900))
    assert early.attrib["viewBox"] == "0 0 200 132"
    assert early[0].attrib["rx"] == "24"
    assert early[0].attrib["fill-opacity"] == "0.94"
    assert early[1].attrib["stroke"] == "#3b3b3b"
    assert float(early[2].attrib["stroke-width"]) < float(late[2].attrib["stroke-width"]) <= 2.5
    assert int(early[2].attrib["stroke"][1:3], 16) < int(late[2].attrib["stroke"][1:3], 16) < 230
    for svg in (early, late):
        for node in svg:
            color = node.attrib.get("stroke", node.attrib.get("fill"))
            assert color[1:3] == color[3:5] == color[5:7]
        assert not any("animate" in node.tag or "text" in node.tag for node in svg)
    expired = ElementTree.fromstring(FocusTimer._background_svg(200, 132, 0, 900, True))
    assert len(expired) == 2  # Only the surface and track remain.
    full = ElementTree.fromstring(FocusTimer._background_svg(80, 60, 900, 900))
    dash, circumference = map(float, full[2].attrib["stroke-dasharray"].split())
    assert dash == circumference
    assert full.attrib["width"] == "80"


def test_refresh_uses_svg_loader_and_keeps_labels_stable():
    timer, events = timer_fixture()
    text = [""]
    timer.countdown = SimpleNamespace(
        get_text=lambda: text[0],
        set_text=lambda value: text.__setitem__(0, value) or events.append(("text", value)),
    )
    timer.goal_label = object()
    countdown, goal = timer.countdown, timer.goal_label
    timer.canvas = SimpleNamespace(set_from_pixbuf=lambda pixbuf: events.append(("pixbuf", pixbuf)))
    loader = SimpleNamespace(
        write=lambda data: events.append(("svg", ElementTree.fromstring(data))),
        close=lambda: events.append("loader closed"), get_pixbuf=lambda: "image",
    )
    timer.GLib.Bytes = SimpleNamespace(new=lambda data: SimpleNamespace(get_data=lambda: data))
    timer.GdkPixbuf = SimpleNamespace(PixbufLoader=SimpleNamespace(
        new_with_type=lambda kind: events.append(("loader", kind)) or loader))
    FocusTimer._refresh(timer)
    assert text[0] == "00:50"
    FocusTimer._refresh(timer)
    assert events.count(("text", "00:50")) == 1
    timer.expired = True
    FocusTimer._refresh(timer)
    assert text[0] == "Время"
    assert timer.countdown is countdown and timer.goal_label is goal
    assert events.count(("loader", "svg")) == 3
    assert events.count("loader closed") == 3
    assert events.count(("pixbuf", "image")) == 3


def test_visible_expiration_refreshes_once_and_cleans_up():
    timer, events = timer_fixture()
    timer.window.get_mapped = lambda: True
    timer.source = 42
    timer.clock = lambda: 900
    assert timer._tick() is True
    assert timer._expire() is False
    assert timer._expire() is False
    assert events == ["refresh", ("remove", 42), "refresh", ("expire", "block")]
    timer.close()
    timer.close()
    assert events[-1] == "destroy"
    assert events.count("destroy") == 1
    assert timer._tick() is False


def test_tick_refreshes_only_while_mapped():
    timer, events = timer_fixture()
    timer.window.get_mapped = lambda: True
    assert timer._tick() is True
    assert events == ["refresh"]
    timer.expired = True
    assert timer._tick() is False
    assert events == ["refresh"]
