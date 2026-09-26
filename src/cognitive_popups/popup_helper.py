#!/usr/bin/env python3
"""GTK popup helper: renders one popup per invocation and returns its result.

This is the only place that sees clicks, so it is also where click logging
happens.  Every window records `window_open`, each click inside it, and
`window_close` with the time spent open; rows are chained through `parent_id`
by `EventChain`, so the log reconstructs "clicked this word -> this layer
opened".  When the desktop layer spawned this process, the chain continues the
interaction it started (see `cognitive_popups.event_log`).

Invoked as `python -m cognitive_popups.popup_helper` with a JSON payload on
stdin; the selected action or typed text goes back on stdout.
"""
import json
import os
import subprocess
import sys
import textwrap
import time
import uuid
from pathlib import Path

if __package__ in (None, ""):
    # Allow running the file directly, without the package on PYTHONPATH.
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    from cognitive_popups.event_log import EventChain, EventLog
    from cognitive_popups import observation
    from cognitive_popups.operation_context import operation_scope
else:
    from .event_log import EventChain, EventLog
    from . import observation
    from .operation_context import operation_scope

_LOG = EventLog()
_CHAIN = EventChain.from_env(_LOG, origin="popup", pid=os.getpid())


#: One font size for every window. The geometry below is derived from it so a
#: window is measured for the text that will actually be drawn. 14px in a fixed
#: 90-pixel-tall window was measured to be unreadable on a real task: the reader
#: could not get past the formatting to the condition itself.
FONT_SIZE_PX = 16

#: Average advance of one Cyrillic glyph and the distance between two baselines
#: at FONT_SIZE_PX, spacing set below included. Used only to *estimate* a window.
CHAR_PX = 9.4
LINE_PX = 26

#: TextView side margins plus the window's own padding: the text never starts at
#: pixel zero, so a width estimate must not count those pixels as characters.
TEXT_PADDING_PX = 52

#: A clickable row: taller than one line so the label is not clipped by its own
#: padding. Every list-like window (cues, menu, layers) uses this.
ROW_PX = 32

#: Longest run before a colon that still reads as a field label rather than a
#: sentence that happens to contain a colon. See `label_split`.
LABEL_MAX_CHARS = 28


class PopupObservation:
    """One helper invocation, with no inference from process start to display."""

    def __init__(self, chain, payload):
        self.chain = chain
        refs = payload.get("observation", {})
        self.instance = refs.get("window_instance_id") or uuid.uuid4().hex
        self.artifact = refs.get("artifact_id")
        self.opened = False
        self.closed = False
        self.content = {}
        self.rendered_artifact = None
        self.store = observation.ObservationStore(enabled=chain.log.enabled)
        if not self.artifact:
            with operation_scope(chain.context):
                self.artifact = self.store.record_artifact("rendered_payload", payload)

    def emit(self, event, **fields):
        with operation_scope(self.chain.context):
            if event == "window_close" and (self.closed or not self.opened):
                return None
            if event == "window_open":
                self.opened = True
            if event == "window_close":
                self.closed = True
            if event in {"window_open", "layer_open", "layer_toggle"}:
                self.rendered_artifact = self.store.record_artifact("rendered_view", self.content,
                    source_artifact_id=self.artifact)
            if event in {"window_open", "window_close", "layer_open", "layer_toggle"}:
                self.store.record_presentation(self.artifact,
                    window_instance_id=self.instance, event=event,
                    payload={**fields, "rendered": self.content,
                             "rendered_artifact_id": self.rendered_artifact})
            return self.chain.emit(event, **fields, artifact_id=self.artifact,
                                   window_instance_id=self.instance)

    def submitted(self, text, window):
        with operation_scope(self.chain.context):
            artifact = self.store.record_artifact("submitted_input", {
                "text": text, "window": window, "window_instance_id": self.instance,
            }, source_artifact_id=self.artifact)
            self.store.record_annotation("input_submission", {
                "artifact_id": artifact, "window_instance_id": self.instance,
            })


_PRESENTATION = None


def _emit(event, **fields):
    if _PRESENTATION is not None:
        return _PRESENTATION.emit(event, **fields)
    return _CHAIN.emit(event, **fields)


def _rendered(**content):
    if _PRESENTATION is not None:
        _PRESENTATION.content = content


def _submitted(text, window):
    if _PRESENTATION is not None:
        _PRESENTATION.submitted(text, window)


def load_payload():
    try:
        data = sys.stdin.read().strip()
        if not data:
            return {"mode": "objects", "objects": []}
        payload = json.loads(data)
        if isinstance(payload, dict):
            payload.setdefault("mode", "objects")
            return payload
        if isinstance(payload, list):
            return {"mode": "objects", "objects": payload}
    except Exception:
        pass
    return {"mode": "objects", "objects": []}


def chars_for(width_px, padding=24):
    """How many glyphs fit in `width_px` at FONT_SIZE_PX, for a wrapping label."""
    return max(8, min(60, int((width_px - padding) / CHAR_PX)))


def estimate_objects_size(items):
    count = len(items)
    longest = max((len(s) for s in items), default=0)
    width = min(max(180, int(longest * CHAR_PX) + 28), 320)
    height = min(max(ROW_PX + 10, count * ROW_PX + 10), 240)
    return width, height


def label_split(line):
    """Where a leading `Название:` field label ends, or 0 when there is none.

    Rendering the label itself in bold is what makes a result window scannable:
    the reader finds the discrepancy or the requirement without reading the whole
    block. The rule is deliberately formal — a short run, a colon, few words —
    because it has to work for a task, a verdict and a clarification alike, and
    must not fire on a sentence that merely contains a colon.
    """
    prefix, sep, _rest = line.partition(':')
    prefix = prefix.strip()
    if not sep or not prefix or len(prefix) > LABEL_MAX_CHARS:
        return 0
    if len(prefix.split()) > 3:
        return 0
    return line.index(':') + 1


def _escape_markup(text):
    """The three characters Pango markup cannot take literally.

    Kept local instead of importing GLib, so the emphasis rule stays testable
    without a display and without a GTK import at module load.
    """
    return text.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')


def label_markup(text, escape=_escape_markup):
    """Escape `text` and bold the field labels in it.

    Only the label is emphasised; the rest keeps the plain font, so a long
    condition does not turn into a wall of bold.
    """
    out = []
    for line in text.split('\n'):
        index = label_split(line)
        if index:
            out.append('<b>' + escape(line[:index]) + '</b>' + escape(line[index:]))
        else:
            out.append(escape(line))
    return '\n'.join(out)


def _wrapped_line_count(text, width_px):
    # Reserve the horizontal padding, then fit whole glyphs into what is left.
    chars_per_line = max(24, int((width_px - TEXT_PADDING_PX) / CHAR_PX))
    count = 0
    for line in text.splitlines() or [text]:
        if not line:
            count += 1
        else:
            count += len(textwrap.wrap(
                line,
                width=chars_per_line,
                break_long_words=True,
                break_on_hyphens=False,
            ))
    return max(1, count)


def estimate_text_size(text, expanded=False, available_width=1280, available_height=720):
    if expanded:
        max_width = min(680, max(240, available_width - 48))
        max_window_height = min(EXPANDED_TEXT_HEIGHT, max(194, available_height - 48))
        max_height = max(160, max_window_height - 34)
        candidates = list(range(420, 681, 40))
        candidates = [width for width in candidates if width <= max_width]
        if not candidates or candidates[-1] != max_width:
            candidates.append(max_width)
        chosen_width = candidates[-1]
        chosen_height = max_height
        for candidate in candidates:
            visual_lines = _wrapped_line_count(text, candidate)
            needed_height = max(110, visual_lines * LINE_PX + 32)
            if needed_height <= max_height:
                chosen_width = candidate
                chosen_height = needed_height
                break
        return chosen_width, chosen_height
    # A model writes a task as one paragraph, so counting the source lines gives
    # one line and a window barely two lines tall — the defect this fixes. The
    # height has to come from the text *after* wrapping at the chosen width.
    lines = [ln for ln in text.splitlines() if ln.strip()] or [text]
    longest = max((len(ln) for ln in lines), default=len(text))
    width = min(max(300, int(longest * CHAR_PX) + 48), 560)
    visual_lines = _wrapped_line_count(text, width)
    height = min(max(120, visual_lines * LINE_PX + 32), MAX_TEXT_HEIGHT)
    return width, height


MAX_TEXT_HEIGHT = 460
EXPANDED_TEXT_HEIGHT = 560


def get_mouse_position():
    try:
        import gi
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gdk
        display = Gdk.Display.get_default()
        seat = display.get_default_seat()
        pointer = seat.get_pointer()
        _, px, py = pointer.get_position()
        return int(px), int(py)
    except Exception:
        return 100, 100


def _monitor_workarea(Gdk, px=None, py=None):
    """Return the GTK logical work area for the monitor containing the popup."""
    try:
        screen = Gdk.Screen.get_default()
        display = Gdk.Display.get_default()
        monitor = display.get_monitor_at_point(int(px or 0), int(py or 0))
        if monitor is None:
            monitor = display.get_primary_monitor()
        index = display.get_monitor_number(monitor)
        area = screen.get_monitor_workarea(index)
        return area.x, area.y, area.width, area.height
    except Exception:
        return 0, 0, 1280, 720


def apply_popup_font(Gtk, Gdk):
    provider = Gtk.CssProvider()
    provider.load_from_data(
        '* { font-family: "Noto Sans"; font-size: %dpx; }' % FONT_SIZE_PX
    )
    screen = Gdk.Screen.get_default()
    if screen is not None:
        Gtk.StyleContext.add_provider_for_screen(
            screen, provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )


def apply_zoom(Gtk, Gdk, scale):
    """Resize the text of this popup process without restarting it.

    Every helper invocation is its own process and its windows are undecorated:
    there is no title bar and no edge to drag, so the text size has to be
    reachable from the keyboard. Attached at USER priority so it overrides the
    process-wide font set by `apply_popup_font`.
    """
    provider = Gtk.CssProvider()
    size = max(11, int(round(FONT_SIZE_PX * scale)))
    provider.load_from_data(
        ('* { font-family: "Noto Sans"; font-size: %dpx; }' % size).encode('utf-8')
    )
    screen = Gdk.Screen.get_default()
    if screen is not None:
        Gtk.StyleContext.add_provider_for_screen(
            screen, provider, Gtk.STYLE_PROVIDER_PRIORITY_USER
        )
    return provider


def readable_view(Gtk, editable=False):
    """A TextView configured for reading rather than for dumping text.

    The spacing is set here, not in CSS, because GTK3 CSS has no line-height:
    without it a result of several blocks reads as one dense wall.
    """
    view = Gtk.TextView()
    view.set_editable(editable)
    view.set_cursor_visible(editable)
    view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
    view.set_left_margin(6)
    view.set_right_margin(6)
    view.set_top_margin(4)
    view.set_bottom_margin(4)
    view.set_pixels_above_lines(4)
    view.set_pixels_below_lines(5)
    view.set_pixels_inside_wrap(2)
    return view


def fill_readable(view, text):
    """Show `text` with its field labels emphasised (see `label_split`)."""
    buffer = view.get_buffer()
    buffer.set_text(text)
    tag = buffer.create_tag('field-label', weight=700)
    offset = 0
    for line in text.split('\n'):
        index = label_split(line)
        if index:
            buffer.apply_tag(
                tag,
                buffer.get_iter_at_offset(offset),
                buffer.get_iter_at_offset(offset + index),
            )
        offset += len(line) + 1
    return buffer


def copy_text_to_clipboard(text):
    value = text or ''
    for command in (
        ['wl-copy'],
        ['xclip', '-selection', 'clipboard'],
    ):
        try:
            result = subprocess.run(
                command,
                input=value.encode('utf-8'),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=3,
            )
            if result.returncode == 0:
                return True
        except (FileNotFoundError, subprocess.SubprocessError, OSError):
            continue

    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk
        clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
        clipboard.set_text(value, -1)
        clipboard.store()
        return True
    except Exception:
        return False


def show_input_popup(prompt_text, px=None, py=None, initial=''):
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    class Prompt(Gtk.Window):
        def __init__(self, prompt, px=None, py=None):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.px = px
            self.py = py
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
            self.set_title('Enter nucleus')
            self.set_border_width(0)
            self.connect('key-press-event', self.on_key)

            outer = Gtk.EventBox()
            outer.set_visible_window(True)
            self.add(outer)

            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
            box.set_margin_top(8)
            box.set_margin_bottom(8)
            box.set_margin_start(10)
            box.set_margin_end(10)
            outer.add(box)

            label = Gtk.Label(xalign=0)
            self.prompt_label = label
            self.render_task(label, prompt)
            label.set_line_wrap(True)
            label.set_max_width_chars(48)
            box.pack_start(label, False, False, 0)

            self.entry = Gtk.Entry()
            self.entry.set_activates_default(True)
            self.entry.connect('activate', self.on_submit)
            if initial:
                self.entry.set_text(initial)
            box.pack_start(self.entry, False, False, 0)

            self.set_size_request(480, -1)
            self.show_all()
            GLib.idle_add(self._finalize_open)

        @staticmethod
        def render_task(label, prompt):
            """Show the task as a card: emphasised subject, then the four words as written.

            Escape everything the model wrote before re-inserting markup, so a stray
            angle bracket in the prompt cannot break the label. Field labels in the
            prompt get the same emphasis every result window uses.
            """
            first, _sep, rest = prompt.partition('\n')
            if first.startswith('Объяснить:'):
                topic = first[len('Объяснить:'):].strip()
                markup = f"Объяснить: <b>{_escape_markup(topic)}</b>"
                if rest.strip():
                    markup += '\n' + label_markup(rest.strip())
                label.set_markup(markup)
            else:
                label.set_markup(label_markup(prompt))

        def _finalize_open(self):
            # The window must take the keyboard here: an input popup that opens
            # unfocused is one Escape away from losing what the reader typed, and
            # the window_open event is the only proof the popup ever appeared.
            self.move_to(self.px, self.py)
            self.present()
            self.entry.grab_focus()
            self.entry.set_position(-1)
            _rendered(prompt=self.prompt_label.get_text(), title='Enter nucleus')
            _emit('window_open', window='input', detail=prompt_text)
            return False

        def move_to(self, px, py):
            if px is None or py is None:
                px, py = get_mouse_position()
            self.move(int(px) + 12, int(py) + 12)

        def on_submit(self, *args):
            raw = self.entry.get_text()
            _submitted(raw, 'input')
            text = raw.strip()
            _emit('submit', window='input', detail=f'{len(text)} chars')
            sys.stdout.write(text)
            sys.stdout.flush()
            Gtk.main_quit()
            return True

        def on_copy(self, *args):
            copy_text_to_clipboard(self.entry.get_text().strip())
            _emit('copy', window='input')
            return True

        def on_cancel(self, *args):
            _emit('window_close', window='input', detail='cancel')
            Gtk.main_quit()
            return True

        def on_key(self, widget, event):
            if event.keyval in (Gdk.KEY_Escape,):
                return self.on_cancel()
            return False

    Prompt(prompt_text, px, py)
    Gtk.main()
    return 0


def show_objects_popup(items, px=None, py=None):
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    width, height = estimate_objects_size(items)

    class Popup(Gtk.Window):
        def __init__(self, items, px=None, py=None):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.items = items
            self.px = px
            self.py = py
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
            self.set_title('Objects Tooltip')
            self.set_border_width(0)
            self.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
            self.connect('button-press-event', self.on_click)
            self.connect('key-press-event', self.on_key)
            self.connect('focus-out-event', lambda *a: False)

            outer = Gtk.EventBox()
            outer.set_visible_window(True)
            outer.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
            outer.connect('button-press-event', self.on_click)
            self.add(outer)

            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            box.set_margin_top(6)
            box.set_margin_bottom(6)
            box.set_margin_start(6)
            box.set_margin_end(6)
            outer.add(box)

            for item in items:
                row = Gtk.EventBox()
                row.set_visible_window(True)
                row.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
                row.connect('button-press-event', self.on_click)
                label = Gtk.Label(label=item, xalign=0)
                label.set_line_wrap(True)
                label.set_justify(Gtk.Justification.LEFT)
                label.set_xalign(0)
                label.set_margin_top(4)
                label.set_margin_bottom(4)
                label.set_margin_start(8)
                label.set_margin_end(8)
                label.set_max_width_chars(chars_for(width))
                row.add(label)
                box.pack_start(row, False, False, 0)

            button_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            copy_btn = Gtk.Button(label='Copy')
            copy_btn.connect('clicked', self.on_copy)
            button_row.pack_start(copy_btn, False, False, 0)
            box.pack_start(button_row, False, False, 0)

            self.set_size_request(width, height + 34)
            self.show_all()
            GLib.idle_add(self.apply_geometry)
            GLib.idle_add(self.present)

        def apply_geometry(self):
            self.resize(width, height + 34)
            self.move_to(self.px, self.py)
            return False

        def move_to(self, px, py):
            if px is None or py is None:
                px, py = get_mouse_position()
            self.move(int(px) + 12, int(py) + 12)

        def on_copy(self, *args):
            copy_text_to_clipboard('\n'.join(self.items))
            _emit('copy', window='objects', detail=f'{len(self.items)} objects')
            return True

        def on_click(self, *args):
            return self.close_and_quit(reason='click')

        def on_key(self, widget, event):
            if event.keyval in (Gdk.KEY_Escape, Gdk.KEY_Return, Gdk.KEY_KP_Enter):
                return self.close_and_quit()
            return False

        def close_and_quit(self, reason='key'):
            _emit('window_close', window='objects', detail=reason)
            self.destroy()
            Gtk.main_quit()
            return True

    popup = Popup(items, px, py)
    _rendered(objects=items)
    _emit('window_open', window='objects', detail=f'{len(items)} objects')
    Gtk.main()
    return 0


def show_menu_popup(items, px=None, py=None, title='Menu', focusable=False):
    """Render selectable action rows and return the selected action on stdout.

    `focusable` decides whether the menu takes the keyboard. A menu driven by
    number keys needs the focus; the click-driven one must not take it, or a
    stray click elsewhere would close it while the reader is still choosing.
    """
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    rows = []
    for item in items:
        if isinstance(item, dict):
            label = str(item.get('label', '')).strip()
            action = str(item.get('action', label)).strip()
        else:
            label = action = str(item).strip()
        if label:
            rows.append((label, action))
    if not rows:
        return 0

    width = min(max(240, int(max(len(label) for label, _ in rows) * CHAR_PX) + 48), 460)

    class MenuPopup(Gtk.Window):
        def __init__(self):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            # UTILITY takes the keyboard, POPUP_MENU does not: that is the whole
            # difference between the input popup and the click-driven menu.
            self.set_type_hint(
                Gdk.WindowTypeHint.UTILITY if focusable else Gdk.WindowTypeHint.POPUP_MENU
            )
            if focusable:
                self.set_can_focus(True)
                self.sink = None
            self.set_title(title)
            self.connect('key-press-event', self.on_key)
            # Closing on focus-out suits a click-driven menu: the reader is about to
            # dismiss it anyway. On the keyboard-driven one it is a defect — the menu
            # vanishes the instant focus moves anywhere else, which reads as nothing
            # having happened at all.
            if not focusable:
                self.connect('focus-out-event', lambda *args: self.close())

            outer = Gtk.EventBox()
            outer.set_visible_window(True)
            self.add(outer)
            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            box.set_margin_top(6)
            box.set_margin_bottom(6)
            box.set_margin_start(6)
            box.set_margin_end(6)
            outer.add(box)
            if focusable:
                # An invisible focus sink. The input popup gets the keyboard because
                # its entry grabs focus; a menu has no widget of its own, and a
                # toplevel asking for focus on its own does not reach the compositor.
                sink = Gtk.EventBox()
                sink.set_can_focus(True)
                box.pack_start(sink, False, False, 0)
                self.sink = sink
            for label, action in rows:
                row = Gtk.EventBox()
                row.set_visible_window(True)
                row.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
                row.connect('button-press-event', self.choose, action)
                text = Gtk.Label(label=label, xalign=0)
                text.set_margin_top(6)
                text.set_margin_bottom(6)
                text.set_margin_start(8)
                text.set_margin_end(8)
                text.set_max_width_chars(chars_for(width, padding=32))
                row.add(text)
                box.pack_start(row, False, False, 0)
            self.show_all()
            GLib.idle_add(self.place)

        def place(self):
            self.resize(width, max(44, len(rows) * (ROW_PX + 6) + 12))
            if px is None or py is None:
                mx, my = get_mouse_position()
            else:
                mx, my = int(px), int(py)
            self.move(mx + 12, my + 12)
            self.present()
            if focusable:
                self.sink.grab_focus()
            _rendered(title=title, items=rows)
            _emit('window_open', window='menu', detail=title)
            return False

        def pick(self, action):
            """Report the chosen row and close, whoever asked — mouse or key."""
            index = next((i for i, (_l, a) in enumerate(rows) if a == action), None)
            label = rows[index][0] if index is not None else action
            _emit('click', window='menu', layer=1, item_index=index, item_label=label, detail=action)
            try:
                sys.stdout.write(json.dumps({'action': action}, ensure_ascii=False))
                sys.stdout.flush()
            except OSError:
                # The parent is gone. The window still has to close: a popup that
                # outlives its caller stays on screen with nobody to dismiss it.
                pass
            self.close()

        def choose(self, _widget, event, action):
            if getattr(event, 'button', 0) == 3:
                _emit('window_close', window='menu', detail='right_click')
                self.close()
                return True
            self.pick(action)
            return True

        def on_key(self, _widget, event):
            if event.keyval == Gdk.KEY_Escape:
                _emit('window_close', window='menu', detail='escape')
                self.close()
                return True
            # Number keys choose a row, so a menu can be driven without the mouse.
            if Gdk.KEY_1 <= event.keyval <= Gdk.KEY_9:
                digit = event.keyval - Gdk.KEY_1
            elif Gdk.KEY_KP_1 <= event.keyval <= Gdk.KEY_KP_9:
                digit = event.keyval - Gdk.KEY_KP_1
            else:
                return False
            if digit < len(rows):
                self.pick(rows[digit][1])
                return True
            return False

        def close(self, *_args):
            self.destroy()
            Gtk.main_quit()
            return True

    MenuPopup()
    Gtk.main()
    return 0


def show_dual_popup(items, px=None, py=None, title=''):
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    rows = []
    for item in items:
        if not isinstance(item, dict):
            continue
        simple_value = item.get('simple')
        simple = '' if simple_value is None else str(simple_value).strip()
        term_value = item.get('term', simple)
        term = simple if term_value is None else str(term_value).strip()
        meaning_value = item.get('meaning')
        meaning = '' if meaning_value is None else str(meaning_value).strip()
        if simple:
            rows.append((simple, term, meaning))
    if not rows:
        return 0
    class DualPopup(Gtk.Window):
        def __init__(self):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.active: set[int] = set()
            self.revealed: set[int] = set()
            self._opened = time.monotonic()
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
            self.connect('key-press-event', self.on_key)
            self.outer = Gtk.EventBox()
            self.outer.set_visible_window(True)
            self.add(self.outer)
            self.columns = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=0)
            self.outer.add(self.columns)
            self.left = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            self.right = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            # The third column carries the exact term: the second column now opens
            # the plain meaning, so the term is revealed last (layers swapped).
            self.term = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            self.columns.pack_start(self.left, False, False, 0)
            self.columns.pack_start(self.right, False, False, 0)
            self.columns.pack_start(self.term, False, False, 0)
            self.term.hide()
            for index, (simple, term, meaning) in enumerate(rows):
                row = Gtk.EventBox()
                row.set_visible_window(True)
                row.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
                row.connect('button-press-event', self.reveal_meanings, index)
                row.set_size_request(-1, ROW_PX)
                cell = Gtk.Label(label=simple, xalign=0)
                cell.set_margin_top(5); cell.set_margin_bottom(5)
                cell.set_margin_start(9); cell.set_margin_end(9)
                cell.set_width_chars(12)
                cell.set_max_width_chars(16)
                row.add(cell)
                self.left.pack_start(row, False, False, 0)
            self.show_all()
            self.term.hide()
            GLib.idle_add(self.place)

        def place(self):
            self.resize(200, max(44, len(rows) * ROW_PX + 8))
            mx, my = (get_mouse_position() if px is None or py is None else (int(px), int(py)))
            self.move(mx + 12, my + 12)
            self.present()
            _rendered(simple=[row[0] for row in rows], terms=[], meanings=[])
            _emit('window_open', window='dual', layer=1, detail=title or f'{len(rows)} cues')
            return False

        def reveal_meanings(self, _widget, event, _index):
            if getattr(event, 'button', 0) == 3:
                return self.close(reason='right_click')
            if not isinstance(_index, int) or not 0 <= _index < len(rows):
                return True
            simple, term, meaning = rows[_index]
            _emit('click', window='dual', layer=1, item_index=_index, item_label=simple)
            # The second column is revealed one word at a time: a click opens the
            # meaning of that word only, so all four are never handed over at once.
            self.revealed.add(_index)
            self.render_meanings()
            self.observe_layers()
            _emit(
                'layer_open', window='dual', layer=2, item_index=_index,
                item_label=meaning or term, detail='meaning revealed',
            )
            return True

        def render_meanings(self):
            """Keep one row per word so a revealed meaning lines up with its word."""
            for child in self.right.get_children():
                self.right.remove(child)
            for index, (_simple, term, meaning) in enumerate(rows):
                holder = Gtk.EventBox()
                holder.set_size_request(-1, ROW_PX)
                if index in self.revealed:
                    holder.set_visible_window(True)
                    holder.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
                    holder.connect('button-press-event', self.toggle_terms, index)
                    cell = Gtk.Label(label=meaning or term, xalign=0)
                    cell.set_width_chars(22)
                    cell.set_max_width_chars(26)
                else:
                    holder.set_visible_window(False)
                    holder.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
                    holder.connect('button-press-event', self.close_on_right)
                    cell = Gtk.Label(label='', xalign=0)
                cell.set_margin_top(5); cell.set_margin_bottom(5)
                cell.set_margin_start(12); cell.set_margin_end(12)
                holder.add(cell)
                self.right.pack_start(holder, False, False, 0)
            self.right.show_all()
            self.resize(470, max(44, len(rows) * ROW_PX + 8))

        def observe_layers(self):
            _rendered(simple=[row[0] for row in rows],
                meanings=[(row[2] or row[1]) if i in self.revealed else '' for i, row in enumerate(rows)],
                terms=[(row[1] or row[2]) if i in self.active else '' for i, row in enumerate(rows)])

        def toggle_terms(self, _widget, event, index):
            if getattr(event, 'button', 0) == 3:
                return self.close(reason='right_click')
            if not isinstance(index, int) or not 0 <= index < len(rows):
                return True
            term = rows[index][1] or rows[index][2]
            meaning = rows[index][2] or rows[index][1]
            _emit('click', window='dual', layer=2, item_index=index, item_label=meaning)
            if index in self.active:
                self.active.remove(index)
                shown = False
            else:
                self.active.add(index)
                shown = True
            for child in self.term.get_children():
                self.term.remove(child)
            if not self.active:
                self.term.hide()
                self.resize(470, max(44, len(rows) * ROW_PX + 8))
                self.observe_layers()
                _emit('layer_toggle', window='dual', layer=3, item_index=index,
                      item_label=term, detail='hidden')
                return True
            for row_index, (_row_simple, row_term, row_meaning) in enumerate(rows):
                holder = Gtk.EventBox()
                holder.set_visible_window(False)
                holder.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
                holder.connect('button-press-event', self.close_on_right)
                if row_index in self.active:
                    detail = Gtk.Label(label=row_term or row_meaning, xalign=0)
                    detail.set_line_wrap(False)
                    detail.set_width_chars(40)
                    detail.set_max_width_chars(44)
                    detail.set_margin_top(5); detail.set_margin_bottom(5)
                    detail.set_margin_start(12); detail.set_margin_end(12)
                    detail.set_size_request(-1, 22)
                    holder.add(detail)
                else:
                    blank = Gtk.Label(label='', xalign=0)
                    blank.set_size_request(-1, 22)
                    holder.add(blank)
                holder.set_size_request(-1, ROW_PX)
                self.term.pack_start(holder, False, False, 0)
            self.term.show_all()
            self.resize(760, max(44, len(rows) * ROW_PX + 8))
            self.observe_layers()
            _emit('layer_toggle', window='dual', layer=3, item_index=index,
                  item_label=term, detail='shown' if shown else 'hidden')
            return True

        def close_on_right(self, _widget, event):
            if getattr(event, 'button', 0) == 3:
                return self.close(reason='right_click')
            return False

        def on_key(self, _widget, event):
            if event.keyval == Gdk.KEY_Escape:
                return self.close(reason='escape')
            return False

        def close(self, *_args, **kwargs):
            reason = kwargs.get('reason', 'key')
            _emit(
                'window_close', window='dual',
                detail=f'{reason} after {time.monotonic() - self._opened:.1f}s',
            )
            self.destroy(); Gtk.main_quit(); return True

    DualPopup()
    Gtk.main()
    return 0


def show_text_popup(text, px=None, py=None, title='Result', expanded=False, note='', actions=None):
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    if expanded:
        area_x, area_y, area_width, area_height = _monitor_workarea(Gdk, px, py)
        width, height = estimate_text_size(
            text,
            expanded=True,
            available_width=area_width,
            available_height=area_height,
        )
    else:
        area_x = area_y = 0
        area_width, area_height = 1280, 720
        width, height = estimate_text_size(text)
    height_cap = (
        min(EXPANDED_TEXT_HEIGHT, max(194, area_height - 48)) - 34
        if expanded else MAX_TEXT_HEIGHT
    )

    class TextPopup(Gtk.Window):
        def __init__(self, body, px=None, py=None):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.body = body
            self.px = px
            self.py = py
            self._scale = 1.0
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
            self.set_title(title)
            self.set_border_width(0)
            self.connect('focus-out-event', lambda *a: False)
            self.connect('key-press-event', self.on_key)

            outer = Gtk.EventBox()
            outer.set_visible_window(True)
            outer.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
            outer.connect('button-press-event', lambda *a: self.close_and_quit(reason='click'))
            self.add(outer)

            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            box.set_margin_top(8)
            box.set_margin_bottom(8)
            box.set_margin_start(10)
            box.set_margin_end(10)
            outer.add(box)

            if note:
                # Short provenance line, e.g. which intent was taken into
                # account. Kept as a label so it is not copied with the answer.
                note_label = Gtk.Label(label=note, xalign=0)
                note_label.set_line_wrap(True)
                note_label.set_max_width_chars(64)
                box.pack_start(note_label, False, False, 0)

            view = readable_view(Gtk)
            fill_readable(view, body)
            scrolled = Gtk.ScrolledWindow()
            scrolled.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
            scrolled.set_shadow_type(Gtk.ShadowType.IN)
            scrolled.set_size_request(-1, min(height, height_cap))
            scrolled.add(view)
            box.pack_start(scrolled, True, True, 0)

            button_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            copy_btn = Gtk.Button(label='Copy')
            copy_btn.connect('clicked', self.on_copy)
            button_row.pack_start(copy_btn, False, False, 0)
            for action in actions or []:
                if isinstance(action, dict) and action.get('action') == 'reframe':
                    choice = Gtk.Button(label=str(action.get('label') or 'Другой ракурс') + ' (Ctrl+R)')
                    choice.connect('clicked', self.on_action, 'reframe')
                    button_row.pack_start(choice, False, False, 0)
            hint = Gtk.Label(label='Ctrl +/− — размер')
            hint.set_halign(Gtk.Align.END)
            button_row.pack_end(hint, False, False, 0)
            box.pack_start(button_row, False, False, 0)

            self.set_size_request(width, min(height, height_cap) + 34)
            self.show_all()
            GLib.idle_add(self.apply_geometry)
            GLib.idle_add(self.present)

        def apply_geometry(self):
            self.resize(width, min(height, height_cap) + 34)
            self.move_to(self.px, self.py)
            return False

        def move_to(self, px, py):
            if px is None or py is None:
                px, py = get_mouse_position()
            x = int(px) + 12
            y = int(py) + 12
            if expanded:
                x = min(max(area_x, x), area_x + area_width - width)
                y = min(max(area_y, y), area_y + area_height - min(height, height_cap) - 34)
            self.move(x, y)

        def zoom(self, delta):
            """Make the text bigger or smaller, and give it the room to match.

            An undecorated window has no edge to drag, so this is the reader's
            only way to size the text; the zoom is logged, because a window the
            reader had to enlarge is evidence about the default, not a detail.
            """
            self._scale = max(0.85, min(2.0, round(self._scale + delta, 2)))
            apply_zoom(Gtk, Gdk, self._scale)
            room = max(120, (area_height if expanded else 900) - 90)
            body_height = int(min(height * self._scale, height_cap * self._scale, room))
            self.resize(int(width * self._scale), body_height + 34)
            _emit('layer_open', window='text', detail=f'zoom {self._scale:.2f}')
            return True

        def on_copy(self, *args):
            copy_text_to_clipboard(self.body)
            _emit('copy', window='text', detail=title)
            return True

        def on_action(self, _button, action):
            _emit('click', window='text', detail=action)
            sys.stdout.write(json.dumps({'action': action}, ensure_ascii=False))
            sys.stdout.flush()
            return self.close_and_quit(reason=action)

        def on_key(self, widget, event):
            if event.state & Gdk.ModifierType.CONTROL_MASK:
                if event.keyval == Gdk.KEY_r and any(
                    isinstance(item, dict) and item.get('action') == 'reframe'
                    for item in actions or []
                ):
                    return self.on_action(None, 'reframe')
                if event.keyval in (Gdk.KEY_plus, Gdk.KEY_equal, Gdk.KEY_KP_Add):
                    return self.zoom(0.15)
                if event.keyval in (Gdk.KEY_minus, Gdk.KEY_underscore, Gdk.KEY_KP_Subtract):
                    return self.zoom(-0.15)
                if event.keyval in (Gdk.KEY_0, Gdk.KEY_KP_0):
                    return self.zoom(1.0 - self._scale)
            if event.keyval in (Gdk.KEY_Escape, Gdk.KEY_Return, Gdk.KEY_KP_Enter):
                return self.close_and_quit()
            return False

        def close_and_quit(self, reason='key'):
            _emit('window_close', window='text', detail=reason)
            self.destroy()
            Gtk.main_quit()
            return True

    TextPopup(text, px, py)
    _rendered(text=text, title=title, expanded=expanded, note=note)
    _emit('window_open', window='text', detail=title)
    Gtk.main()
    return 0


def show_note_popup(anchor='', px=None, py=None, *, title='Error note', prompt=''):
    """Multi-line capture for an error note; prints the comment on stdout.

    Only the input field is shown. The anchor is still captured and stored, but
    never rendered: echoing the page back at the reader turns the window into a
    copy of what they are already reading. Enter has to insert a newline, so
    saving is Ctrl+Enter or the button.
    """
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    class NotePopup(Gtk.Window):
        def __init__(self, anchor, px=None, py=None):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.anchor = anchor
            self.px = px
            self.py = py
            self._opened = time.monotonic()
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
            self.set_title(title)
            self.set_border_width(0)
            self.connect('key-press-event', self.on_key)

            outer = Gtk.EventBox()
            outer.set_visible_window(True)
            self.add(outer)
            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
            box.set_margin_top(8)
            box.set_margin_bottom(8)
            box.set_margin_start(10)
            box.set_margin_end(10)
            outer.add(box)

            if prompt:
                # The condition is what the reader has to work from, so it gets
                # the same field-label emphasis and spacing as any result window.
                label = Gtk.Label(xalign=0)
                label.set_markup(label_markup(prompt))
                label.set_line_wrap(True)
                label.set_max_width_chars(56)
                box.pack_start(label, False, False, 0)
            self.view = readable_view(Gtk, editable=True)
            self.view.set_size_request(380, 120)
            box.pack_start(self.view, True, True, 0)

            buttons = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            save = Gtk.Button(label='Записать')
            save.connect('clicked', self.on_submit)
            buttons.pack_start(save, False, False, 0)
            cancel = Gtk.Button(label='Отмена')
            cancel.connect('clicked', self.on_cancel)
            buttons.pack_end(cancel, False, False, 0)
            box.pack_start(buttons, False, False, 0)

            window_width, window_height = 410, 190
            if prompt:
                # A condition has to fit above the input field; the old fixed
                # 190-pixel window showed a task through a letterbox.
                prompt_width, prompt_height = estimate_text_size(prompt)
                window_width = max(window_width, prompt_width)
                window_height = max(window_height, 150 + prompt_height)
            self.resize(window_width, window_height)
            self.show_all()
            GLib.idle_add(self.place)

        def place(self):
            if self.px is None or self.py is None:
                mx, my = get_mouse_position()
            else:
                mx, my = int(self.px), int(self.py)
            self.move(mx + 12, my + 12)
            self.present()
            self.view.grab_focus()
            _rendered(buttons=['Записать', 'Отмена'], prompt=prompt)
            _emit('window_open', window='note' if not prompt else 'task_attempt', detail=self.anchor or 'no selection')
            return False

        def text(self):
            buffer = self.view.get_buffer()
            return buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True).strip()

        def on_submit(self, *_args):
            text = self.text()
            if not text:
                return True
            buffer = self.view.get_buffer()
            _submitted(buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True), 'note' if not prompt else 'task_attempt')
            _emit('submit', window='note' if not prompt else 'task_attempt', detail=f'{len(text)} chars')
            sys.stdout.write(text)
            sys.stdout.flush()
            self.destroy()
            Gtk.main_quit()
            return True

        def on_cancel(self, *_args):
            _emit('window_close', window='note' if not prompt else 'task_attempt', detail='cancel')
            self.destroy()
            Gtk.main_quit()
            return True

        def on_key(self, _widget, event):
            if event.keyval == Gdk.KEY_Escape:
                return self.on_cancel()
            if event.keyval in (Gdk.KEY_Return, Gdk.KEY_KP_Enter) \
                    and (event.state & Gdk.ModifierType.CONTROL_MASK):
                return self.on_submit()
            return False

    NotePopup(anchor, px, py)
    Gtk.main()
    return 0


def show_intent_popup(intent=None, history=None, error='', px=None, py=None):
    """The accepted goal, with continuing to read as the default action.

    The phrase is what the window is about; «Продолжить чтение» closes it and
    nothing is written. Editing, the optional criterion and stopping place, and
    the replaced goals stay behind their own actions, so returning to the
    bookmark never turns into a form to fill in.
    """
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    intent = intent if isinstance(intent, dict) else {}
    previous = [row for row in (history or []) if isinstance(row, dict)]
    goal_text = str(intent.get('text') or '')
    material = str(intent.get('material') or '')
    material_origin = str(intent.get('material_origin') or '')

    class IntentPopup(Gtk.Window):
        def __init__(self):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
            self.set_title('Сейчас хочу')
            self.set_border_width(0)
            self.connect('key-press-event', self.on_key)

            self._history_open = False
            self.outer = Gtk.EventBox()
            self.outer.set_visible_window(True)
            self.add(self.outer)
            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
            box.set_margin_top(8)
            box.set_margin_bottom(8)
            box.set_margin_start(10)
            box.set_margin_end(10)
            self.outer.add(box)

            if error:
                banner = Gtk.Label(label='Не удалось сохранить: ' + error, xalign=0)
                banner.set_line_wrap(True)
                banner.set_max_width_chars(56)
                box.pack_start(banner, False, False, 0)

            self.goal = Gtk.Label(label=goal_text or 'Сейчас хочу…', xalign=0)
            self.goal.set_line_wrap(True)
            self.goal.set_max_width_chars(54)
            box.pack_start(self.goal, False, False, 0)

            if material:
                # The goal stays answerable to the passage it was worded from.
                where = material_origin or 'материал'
                line = Gtk.Label(label=f'{where}: «{_preview(material)}»', xalign=0)
                line.set_line_wrap(True)
                line.set_max_width_chars(54)
                box.pack_start(line, False, False, 0)

            buttons = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            self.keep = Gtk.Button(label='Продолжить чтение')
            self.keep.connect('clicked', self.on_keep)
            buttons.pack_start(self.keep, False, False, 0)
            new_goal = Gtk.Button(label='Новая цель')
            new_goal.set_tooltip_text('Подобрать другую цель по тому, что читаешь сейчас')
            new_goal.connect('clicked', self.on_assist)
            buttons.pack_start(new_goal, False, False, 0)
            edit = Gtk.Button(label='Изменить')
            edit.connect('clicked', self.on_edit)
            buttons.pack_start(edit, False, False, 0)
            if previous:
                previous_button = Gtk.Button(label='Прежние')
                previous_button.connect('clicked', self.on_toggle_history)
                buttons.pack_start(previous_button, False, False, 0)
            cancel = Gtk.Button(label='Отмена')
            cancel.connect('clicked', self.on_cancel)
            buttons.pack_end(cancel, False, False, 0)
            box.pack_start(buttons, False, False, 0)

            # Everything that edits the phrase stays folded away: this window is
            # a return to the bookmark, not a form to fill in.
            self.editor = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
            self.entry = Gtk.Entry()
            self.entry.set_placeholder_text('Сейчас хочу…')
            self.entry.set_text(goal_text)
            self.entry.set_activates_default(True)
            self.entry.connect('activate', self.on_save)
            self.editor.pack_start(self.entry, False, False, 0)

            self.optional = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
            criterion_label = Gtk.Label(label='Достаточно на этот заход, когда:', xalign=0)
            self.optional.pack_start(criterion_label, False, False, 0)
            self.criterion = Gtk.Entry()
            self.criterion.set_text(str(intent.get('criterion') or ''))
            self.optional.pack_start(self.criterion, False, False, 0)
            stopped_label = Gtk.Label(label='Остановился здесь:', xalign=0)
            self.optional.pack_start(stopped_label, False, False, 0)
            self.stopped = Gtk.Entry()
            self.stopped.set_text(str(intent.get('stopped_at') or ''))
            self.optional.pack_start(self.stopped, False, False, 0)

            self.toggle = Gtk.Button(label='Ещё: критерий и место остановки')
            self.toggle.connect('clicked', self.on_toggle_optional)
            self.editor.pack_start(self.toggle, False, False, 0)
            self.editor.pack_start(self.optional, False, False, 0)

            save = Gtk.Button(label='Сохранить')
            save.connect('clicked', self.on_save)
            self.editor.pack_start(save, False, False, 0)
            box.pack_start(self.editor, False, False, 0)

            self.history_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            box.pack_start(self.history_box, False, False, 0)

            self.set_size_request(440, -1)
            self.show_all()
            if intent.get('criterion') or intent.get('stopped_at'):
                self.toggle.set_label('Свернуть дополнительные строки')
            else:
                self.optional.hide()
            self.editor.hide()
            self.history_box.hide()
            GLib.idle_add(self.place)

        def place(self):
            mx, my = (get_mouse_position() if px is None or py is None else (int(px), int(py)))
            self.move(mx + 12, my + 12)
            self.present()
            # Continuing to read is the default, so Enter must not land in an
            # editor the reader did not ask for.
            self.keep.grab_focus()
            _rendered(title='Сейчас хочу', intent=intent, has_history=bool(previous),
                      has_material=bool(material))
            _emit('window_open', window='intent', detail='намерение')
            return False

        def on_keep(self, *_args):
            self.finish({'action': 'keep'})
            return True

        def on_edit(self, *_args):
            # show() and not show_all(): the optional rows keep their own state.
            self.editor.show()
            self.entry.grab_focus()
            self.entry.set_position(-1)
            _emit('layer_open', window='intent', detail='edit')
            self.resize(440, -1)
            return True

        def on_toggle_optional(self, *_args):
            if self.optional.get_visible():
                self.optional.hide()
                self.toggle.set_label('Ещё: критерий и место остановки')
            else:
                self.optional.show_all()
                self.toggle.set_label('Свернуть дополнительные строки')
            # Height 1 is a positive value GTK accepts; a non-resizable window
            # grows to the content's requisition, so the extra rows still fit.
            self.resize(440, 1)
            return True

        def on_toggle_history(self, *_args):
            if self._history_open:
                self.history_box.hide()
                self._history_open = False
                return True
            if not self.history_box.get_children():
                for row in previous:
                    item = Gtk.Button(label=str(row.get('text') or ''))
                    item.set_relief(Gtk.ReliefStyle.NONE)
                    item.connect('clicked', self.on_restore, str(row.get('id') or ''))
                    self.history_box.pack_start(item, False, False, 0)
            self.history_box.show_all()
            self._history_open = True
            return True

        def on_restore(self, _widget, intention_id):
            if intention_id:
                self.finish({'action': 'restore', 'id': intention_id})
            return True

        def on_save(self, *_args):
            text = self.entry.get_text().strip()
            if not text:
                # An empty window is not an error and writes nothing.
                self.entry.grab_focus()
                return True
            # No material or source is sent: an edit of the phrase must not
            # disturb what the stored goal says about where it came from.
            self.finish({
                'action': 'save',
                'text': text,
                'criterion': self.criterion.get_text().strip(),
                'stopped_at': self.stopped.get_text().strip(),
            })
            return True

        def on_assist(self, *_args):
            # One explicit action, no model work yet: the direction window
            # decides whether there is anything to ground the wording in.
            self.finish({'action': 'assist'})
            return True

        def on_cancel(self, *_args):
            _emit('window_close', window='intent', detail='cancel')
            self.destroy()
            Gtk.main_quit()
            return True

        def on_key(self, _widget, event):
            if event.keyval == Gdk.KEY_Escape:
                return self.on_cancel()
            return False

        def finish(self, payload):
            _emit('submit', window='intent', detail=str(payload.get('action', '')))
            try:
                sys.stdout.write(json.dumps(payload, ensure_ascii=False))
                sys.stdout.flush()
            except OSError:
                pass
            self.destroy()
            Gtk.main_quit()
            return True

    IntentPopup()
    Gtk.main()
    return 0


def _preview(text, limit=70):
    """One-line beginning of a passage, for a heading that must stay short."""
    flat = ' '.join(str(text or '').split())
    if len(flat) <= limit:
        return flat
    return flat[: limit - 1].rstrip() + '…'


def paste_clipboard_text():
    """Read the clipboard with the same backends used for copying."""
    for command in (['wl-paste', '--no-newline'], ['xclip', '-selection', 'clipboard', '-o']):
        try:
            result = subprocess.run(command, capture_output=True, text=True,
                                    timeout=2, check=False)
        except (OSError, subprocess.SubprocessError):
            continue
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
    return ''


def show_goal_popup(material='', origin='', directions=None, notice='', error='',
                    last_fragment='', px=None, py=None):
    """Pick what to get from a passage; one model call then words it.

    The window opens on the decision, not on a form: the captured passage is a
    one-line preview with a single action that replaces it, and the directions
    are the first thing to click. A fragment saved earlier under «4 слова» is
    offered as an explicit choice and never substituted on its own.

    Choosing a direction finishes the window; the wording comes back in the
    result window, where it is accepted, edited, or dropped.
    """
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    material = str(material or '')
    origin = str(origin or '')
    notice = str(notice or '')
    last_fragment = str(last_fragment or '')
    labels = [str(x) for x in (directions or []) if str(x).strip()]

    class GoalPopup(Gtk.Window):
        def __init__(self):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
            self.set_title('Подобрать цель')
            self.set_border_width(0)
            self.connect('key-press-event', self.on_key)
            self.editing = False

            self.outer = Gtk.EventBox()
            self.outer.set_visible_window(True)
            self.add(self.outer)
            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
            box.set_margin_top(8)
            box.set_margin_bottom(8)
            box.set_margin_start(10)
            box.set_margin_end(10)
            self.outer.add(box)

            if error:
                banner = Gtk.Label(label=error, xalign=0)
                banner.set_line_wrap(True)
                banner.set_max_width_chars(56)
                box.pack_start(banner, False, False, 0)

            heading = Gtk.Label(label='Что хочешь получить от чтения?', xalign=0)
            box.pack_start(heading, False, False, 0)

            # The passage is a heading, not a field: it is already captured, and
            # reviewing it is only needed when the capture was wrong.
            source_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            self.source_label = Gtk.Label(xalign=0)
            self.source_label.set_line_wrap(True)
            self.source_label.set_max_width_chars(42)
            source_row.pack_start(self.source_label, True, True, 0)
            self.change_button = Gtk.Button(label='Изменить')
            self.change_button.connect('clicked', self.on_toggle_editor)
            source_row.pack_end(self.change_button, False, False, 0)
            box.pack_start(source_row, False, False, 0)

            if notice:
                hint = Gtk.Label(label=notice, xalign=0)
                hint.set_line_wrap(True)
                hint.set_max_width_chars(56)
                box.pack_start(hint, False, False, 0)

            self.editor = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
            self.view = Gtk.TextView()
            self.view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
            self.view.set_left_margin(4)
            self.view.set_right_margin(4)
            self.view.get_buffer().set_text(material)
            scrolled = Gtk.ScrolledWindow()
            scrolled.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
            scrolled.set_shadow_type(Gtk.ShadowType.IN)
            scrolled.set_size_request(400, 90)
            scrolled.add(self.view)
            self.editor.pack_start(scrolled, True, True, 0)

            editor_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            paste = Gtk.Button(label='Из буфера обмена')
            paste.connect('clicked', self.on_paste)
            editor_row.pack_start(paste, False, False, 0)
            if last_fragment:
                last = Gtk.Button(label='Фрагмент «4 слова»')
                last.set_tooltip_text(_preview(last_fragment, 140))
                last.connect('clicked', self.on_last_fragment)
                editor_row.pack_start(last, False, False, 0)
            ready = Gtk.Button(label='Готово')
            ready.connect('clicked', self.on_toggle_editor)
            editor_row.pack_end(ready, False, False, 0)
            self.editor.pack_start(editor_row, False, False, 0)
            box.pack_start(self.editor, False, False, 0)

            self.buttons = []
            for label in labels:
                button = Gtk.Button(label=label)
                button.connect('clicked', self.on_direction, label)
                self.buttons.append(button)
                box.pack_start(button, False, False, 0)

            bottom = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            own_button = Gtk.Button(label='Своя цель…')
            own_button.connect('clicked', self.on_show_own)
            bottom.pack_start(own_button, False, False, 0)
            cancel = Gtk.Button(label='Отмена')
            cancel.connect('clicked', self.on_cancel)
            bottom.pack_end(cancel, False, False, 0)
            box.pack_start(bottom, False, False, 0)

            self.own = Gtk.Entry()
            self.own.set_placeholder_text('например: понять, откуда берётся шаг исключения')
            self.own.set_activates_default(True)
            self.own.connect('activate', self.on_own)
            box.pack_start(self.own, False, False, 0)

            self.set_size_request(440, 1)
            self.show_all()
            self.own.hide()
            # With no passage the reader has to put one in, so the editor is the
            # only useful thing on screen. Otherwise it stays folded away.
            self.editing = not material.strip()
            self.editor.set_visible(self.editing)
            self.change_button.set_visible(bool(material.strip()))
            self.refresh()
            GLib.idle_add(self.place)

        def current_material(self):
            buffer = self.view.get_buffer()
            return buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True).strip()

        def refresh(self):
            text = self.current_material()
            for button in self.buttons:
                # Nothing to word a goal from: do not pretend otherwise.
                button.set_sensitive(bool(text))
            if self.editing:
                self.source_label.set_text('Материал для цели:')
            elif text:
                self.source_label.set_text(f'{origin or "материал"}: «{_preview(text)}»')
            else:
                self.source_label.set_text('Материала нет: вставьте текст.')

        def place(self):
            mx, my = (get_mouse_position() if px is None or py is None else (int(px), int(py)))
            self.move(mx + 12, my + 12)
            self.present()
            # With a passage the decision is the first thing the keyboard meets;
            # without one there is nothing to decide until a passage arrives.
            if self.current_material() and self.buttons:
                self.buttons[0].grab_focus()
            else:
                self.view.grab_focus()
            _rendered(title='Что хочешь получить от чтения?',
                      has_material=bool(material.strip()),
                      has_last_fragment=bool(last_fragment),
                      direction_count=len(labels))
            _emit('window_open', window='goal', detail='подбор цели')
            return False

        def on_toggle_editor(self, *_args):
            self.editing = not self.editing
            self.editor.set_visible(self.editing)
            self.change_button.set_label('Свернуть' if self.editing else 'Изменить')
            if self.editing:
                _emit('layer_open', window='goal', detail='material')
                self.view.grab_focus()
            self.refresh()
            self.resize(440, 1)
            return True

        def on_paste(self, *_args):
            text = paste_clipboard_text()
            if not text:
                _emit('action', window='goal', detail='clipboard is empty')
                return True
            self.view.get_buffer().set_text(text)
            _emit('action', window='goal', detail='material from the clipboard')
            self.refresh()
            return True

        def on_last_fragment(self, *_args):
            self.view.get_buffer().set_text(last_fragment)
            _emit('action', window='goal', detail='material from the last «4 слова»')
            self.refresh()
            return True

        def on_show_own(self, *_args):
            self.own.show()
            self.own.grab_focus()
            _emit('layer_open', window='goal', detail='own wording')
            return True

        def on_direction(self, _widget, label):
            self.finish({
                'action': 'direction',
                'direction': label,
                'material': self.current_material(),
            })
            return True

        def on_own(self, *_args):
            text = self.own.get_text().strip()
            if not text:
                self.own.grab_focus()
                return True
            # With a passage, the model words this into the material; without
            # one it is kept as the reader wrote it, and no model is called.
            self.finish({
                'action': 'direction',
                'direction': '',
                'note': text,
                'material': self.current_material(),
            })
            return True

        def on_cancel(self, *_args):
            _emit('window_close', window='goal', detail='cancel')
            self.destroy()
            Gtk.main_quit()
            return True

        def on_key(self, _widget, event):
            if event.keyval == Gdk.KEY_Escape:
                return self.on_cancel()
            return False

        def finish(self, payload):
            _emit('submit', window='goal', detail=str(payload.get('action', '')))
            try:
                sys.stdout.write(json.dumps(payload, ensure_ascii=False))
                sys.stdout.flush()
            except OSError:
                pass
            self.destroy()
            Gtk.main_quit()
            return True

    GoalPopup()
    Gtk.main()
    return 0


def show_goal_result_popup(text='', direction='', notice='', error='', retryable=False,
                           has_material=False, px=None, py=None):
    """One wording to accept, with editing and another direction available.

    Accepting is the only action that stores anything: the phrase is the content
    of the window, «Принять и читать» closes it and the reader is back in the
    text. The phrasing can be corrected in place, but nothing has to be typed,
    and neither the material nor the direction has to be chosen again.
    """
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    text = str(text or '')
    direction = str(direction or '')
    notice = str(notice or '')
    error = str(error or '')

    class GoalResultPopup(Gtk.Window):
        def __init__(self):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
            self.set_title('Цель чтения')
            self.set_border_width(0)
            self.connect('key-press-event', self.on_key)

            self.outer = Gtk.EventBox()
            self.outer.set_visible_window(True)
            self.add(self.outer)
            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
            box.set_margin_top(8)
            box.set_margin_bottom(8)
            box.set_margin_start(10)
            box.set_margin_end(10)
            self.outer.add(box)

            if error:
                banner = Gtk.Label(label=error, xalign=0)
                banner.set_line_wrap(True)
                banner.set_max_width_chars(56)
                box.pack_start(banner, False, False, 0)

            heading = Gtk.Label(
                label='Текущая цель' if text.strip() else 'Цель чтения', xalign=0)
            box.pack_start(heading, False, False, 0)

            if notice:
                info = Gtk.Label(label=notice, xalign=0)
                info.set_line_wrap(True)
                info.set_max_width_chars(56)
                box.pack_start(info, False, False, 0)

            # The phrase is the content, not a form field: it can be corrected
            # here, and accepting it needs no typing at all.
            self.view = Gtk.TextView()
            self.view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
            self.view.set_left_margin(4)
            self.view.set_right_margin(4)
            self.view.get_buffer().set_text(text)
            scrolled = Gtk.ScrolledWindow()
            scrolled.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
            scrolled.set_shadow_type(Gtk.ShadowType.IN)
            scrolled.set_size_request(400, 74)
            scrolled.add(self.view)
            box.pack_start(scrolled, True, True, 0)

            hint = Gtk.Label(label='Можно поправить прямо здесь.', xalign=0)
            box.pack_start(hint, False, False, 0)

            buttons = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            self.accept = Gtk.Button(label='Принять и читать')
            self.accept.connect('clicked', self.on_accept)
            buttons.pack_start(self.accept, False, False, 0)
            if retryable:
                retry = Gtk.Button(label='Повторить')
                retry.connect('clicked', self.on_retry)
                buttons.pack_start(retry, False, False, 0)
            if has_material:
                another = Gtk.Button(label='Другое направление')
                another.connect('clicked', self.on_another)
                buttons.pack_start(another, False, False, 0)
            cancel = Gtk.Button(label='Отмена')
            cancel.connect('clicked', self.on_cancel)
            buttons.pack_end(cancel, False, False, 0)
            box.pack_start(buttons, False, False, 0)

            self.resize(440, 1)
            self.show_all()
            GLib.idle_add(self.place)

        def current_text(self):
            buffer = self.view.get_buffer()
            return buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True).strip()

        def place(self):
            mx, my = (get_mouse_position() if px is None or py is None else (int(px), int(py)))
            self.move(mx + 12, my + 12)
            self.present()
            # The default action is to keep reading, so Enter accepts and no
            # stray keystroke lands in the phrase.
            self.accept.grab_focus()
            _rendered(title='Цель чтения', direction=direction,
                      has_material=has_material, retryable=retryable)
            _emit('window_open', window='goal_result', detail='цель')
            return False

        def on_accept(self, *_args):
            text = self.current_text()
            if not text:
                self.view.grab_focus()
                return True
            self.finish({'action': 'accept', 'text': text})
            return True

        def on_retry(self, *_args):
            self.finish({'action': 'retry'})
            return True

        def on_another(self, *_args):
            self.finish({'action': 'another'})
            return True

        def on_cancel(self, *_args):
            _emit('window_close', window='goal_result', detail='cancel')
            self.destroy()
            Gtk.main_quit()
            return True

        def on_key(self, _widget, event):
            if event.keyval == Gdk.KEY_Escape:
                return self.on_cancel()
            if event.keyval in (Gdk.KEY_Return, Gdk.KEY_KP_Enter) \
                    and (event.state & Gdk.ModifierType.CONTROL_MASK):
                return self.on_accept()
            return False

        def finish(self, payload):
            _emit('submit', window='goal_result', detail=str(payload.get('action', '')))
            try:
                sys.stdout.write(json.dumps(payload, ensure_ascii=False))
                sys.stdout.flush()
            except OSError:
                pass
            self.destroy()
            Gtk.main_quit()
            return True

    GoalResultPopup()
    Gtk.main()
    return 0


def show_example_popup(material='', query='', intent_text='', has_intent=False,
                       has_last=False, error='', px=None, py=None):
    """Material and an optional request for one concrete example (Alt+Shift+G).

    Used when there is nothing highlighted to illustrate, or when the reader
    explicitly asks with their own request. The captured material is prefilled
    so that typing a request cannot lose what was selected. The intent is an
    explicit checkbox, never mixed in silently, and is shown so the result can
    say which goal was taken into account.
    """
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    class ExamplePopup(Gtk.Window):
        def __init__(self):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
            self.set_title('Покажи на примере')
            self.set_border_width(0)
            self.connect('key-press-event', self.on_key)

            self.outer = Gtk.EventBox()
            self.outer.set_visible_window(True)
            self.add(self.outer)
            box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
            box.set_margin_top(8)
            box.set_margin_bottom(8)
            box.set_margin_start(10)
            box.set_margin_end(10)
            self.outer.add(box)

            self.banner = Gtk.Label(xalign=0)
            self.banner.set_line_wrap(True)
            self.banner.set_max_width_chars(56)
            self.banner.set_text(error)
            box.pack_start(self.banner, False, False, 0)

            material_label = Gtk.Label(label='Материал:', xalign=0)
            box.pack_start(material_label, False, False, 0)
            self.view = Gtk.TextView()
            self.view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
            self.view.set_left_margin(4)
            self.view.set_right_margin(4)
            self.view.get_buffer().set_text(str(material or ''))
            scrolled = Gtk.ScrolledWindow()
            scrolled.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
            scrolled.set_shadow_type(Gtk.ShadowType.IN)
            scrolled.set_size_request(400, 110)
            scrolled.add(self.view)
            box.pack_start(scrolled, True, True, 0)

            request_label = Gtk.Label(label='Со своим запросом (по желанию):', xalign=0)
            box.pack_start(request_label, False, False, 0)
            self.request = Gtk.Entry()
            self.request.set_text(str(query or ''))
            self.request.set_activates_default(True)
            self.request.connect('activate', self.on_show)
            box.pack_start(self.request, False, False, 0)

            self.use_intent = None
            if has_intent:
                label = 'Учесть цель: ' + str(intent_text or '').strip()
                self.use_intent = Gtk.CheckButton(label=label)
                self.use_intent.set_active(False)
                box.pack_start(self.use_intent, False, False, 0)

            buttons = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            show = Gtk.Button(label='Показать пример')
            show.connect('clicked', self.on_show)
            buttons.pack_start(show, False, False, 0)
            if has_last:
                last = Gtk.Button(label='Последний пример')
                last.connect('clicked', self.on_last)
                buttons.pack_start(last, False, False, 0)
            cancel = Gtk.Button(label='Отмена')
            cancel.connect('clicked', self.on_cancel)
            buttons.pack_end(cancel, False, False, 0)
            box.pack_start(buttons, False, False, 0)

            # A non-resizable window takes the content height; only the width is
            # pinned. resize(..., -1) is rejected by GTK as a height.
            self.resize(440, 1)
            self.show_all()
            if not error:
                self.banner.hide()
            GLib.idle_add(self.place)

        def material(self):
            buffer = self.view.get_buffer()
            return buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True).strip()

        def place(self):
            mx, my = (get_mouse_position() if px is None or py is None else (int(px), int(py)))
            self.move(mx + 12, my + 12)
            self.present()
            self.request.grab_focus()
            _rendered(title='Покажи на примере', has_intent=has_intent, has_last=has_last)
            _emit('window_open', window='example', detail='пример')
            return False

        def on_show(self, *_args):
            material = self.material()
            if not material:
                # Do not fall back to anything not in front of the reader.
                self.banner.set_text('Нужен материал: вставьте текст или выберите фрагмент.')
                self.banner.show()
                return True
            self.finish({
                'action': 'show',
                'material': material,
                'query': self.request.get_text().strip(),
                'use_intent': bool(self.use_intent is not None and self.use_intent.get_active()),
            })
            return True

        def on_last(self, *_args):
            self.finish({'action': 'last'})
            return True

        def on_cancel(self, *_args):
            _emit('window_close', window='example', detail='cancel')
            self.destroy()
            Gtk.main_quit()
            return True

        def on_key(self, _widget, event):
            if event.keyval == Gdk.KEY_Escape:
                return self.on_cancel()
            if event.keyval in (Gdk.KEY_Return, Gdk.KEY_KP_Enter) \
                    and (event.state & Gdk.ModifierType.CONTROL_MASK):
                return self.on_show()
            return False

        def finish(self, payload):
            try:
                sys.stdout.write(json.dumps(payload, ensure_ascii=False))
                sys.stdout.flush()
            except OSError:
                pass
            self.destroy()
            Gtk.main_quit()
            return True

    ExamplePopup()
    Gtk.main()
    return 0


def _dispatch(payload, mode):
    if mode == 'intent':
        intent = payload.get('intent')
        history = payload.get('history', [])
        return show_intent_popup(
            intent if isinstance(intent, dict) else None,
            history if isinstance(history, list) else [],
            str(payload.get('error', '')),
            payload.get('x'),
            payload.get('y'),
        )

    if mode == 'goal':
        directions = payload.get('directions')
        return show_goal_popup(
            str(payload.get('material', '')),
            str(payload.get('origin', '')),
            directions if isinstance(directions, list) else [],
            str(payload.get('notice', '')),
            str(payload.get('error', '')),
            str(payload.get('last_fragment', '')),
            payload.get('x'),
            payload.get('y'),
        )

    if mode == 'goal_result':
        return show_goal_result_popup(
            str(payload.get('text', '')),
            str(payload.get('direction', '')),
            str(payload.get('notice', '')),
            str(payload.get('error', '')),
            bool(payload.get('retryable', False)),
            bool(payload.get('has_material', False)),
            payload.get('x'),
            payload.get('y'),
        )

    if mode == 'example':
        return show_example_popup(
            str(payload.get('material', '')),
            str(payload.get('query', '')),
            str(payload.get('intent_text', '')),
            bool(payload.get('has_intent', False)),
            bool(payload.get('has_last', False)),
            str(payload.get('error', '')),
            payload.get('x'),
            payload.get('y'),
        )

    if mode == 'note':
        return show_note_popup(
            str(payload.get('anchor', '')), payload.get('x'), payload.get('y')
        )

    if mode == 'task_attempt':
        return show_note_popup(
            '', payload.get('x'), payload.get('y'), title='Попытка решения',
            prompt=str(payload.get('prompt', 'Введите свою попытку:'))
        )

    if mode == 'input':
        prompt_text = str(payload.get('prompt', 'Введите непонятное ядро или слова:')).strip()
        return show_input_popup(
            prompt_text, payload.get('x'), payload.get('y'),
            str(payload.get('initial', '') or ''),
        )

    if mode == 'menu':
        items = payload.get('items', [])
        if isinstance(items, list):
            return show_menu_popup(
                items,
                payload.get('x'),
                payload.get('y'),
                str(payload.get('title', 'Menu')),
                focusable=bool(payload.get('focusable', False)),
            )
        return 0

    if mode == 'dual':
        items = payload.get('items', [])
        if isinstance(items, list):
            return show_dual_popup(items, payload.get('x'), payload.get('y'), str(payload.get('title', '')))
        return 0

    if mode == 'text':
        text = str(payload.get('text', '')).strip()
        if not text:
            return 0
        px = payload.get('x')
        py = payload.get('y')
        title = str(payload.get('title', 'Result'))
        # Existing desktop daemons may predate the explicit flag. The title
        # keeps prediction results expanded immediately without restarting the
        # daemon and discarding its in-memory reading buffer.
        expanded = bool(payload.get('expanded', False)) or title == 'Моя гипотеза'
        return show_text_popup(
            text, px, py, title=title, expanded=expanded,
            note=str(payload.get('note', '')),
            actions=payload.get('actions', []),
        )

    objects = payload.get('objects', [])
    if isinstance(objects, list):
        items = [str(o) for o in objects if str(o).strip()]
    else:
        items = [str(objects).strip()] if str(objects).strip() else []
    if not items:
        return 0
    px = payload.get('x')
    py = payload.get('y')
    return show_objects_popup(items, px, py)


def main():
    global _PRESENTATION
    payload = load_payload()
    _PRESENTATION = PopupObservation(_CHAIN, payload)
    mode = str(payload.get('mode', 'objects')).lower().strip()
    _emit('popup_start', window=mode)
    try:
        return _dispatch(payload, mode)
    except Exception as exc:
        # A popup that dies before Gtk.main() leaves no visible trace otherwise.
        _emit('error', window=mode, detail=f'{type(exc).__name__}: {exc}')
        raise
    finally:
        if _PRESENTATION.opened and not _PRESENTATION.closed:
            _emit('window_close', window=mode, detail='helper loop ended')


if __name__ == '__main__':
    raise SystemExit(main())
