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
import time
from pathlib import Path

if __package__ in (None, ""):
    # Allow running the file directly, without the package on PYTHONPATH.
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    from cognitive_popups.event_log import EventChain, EventLog
else:
    from .event_log import EventChain, EventLog

_LOG = EventLog()
_CHAIN = EventChain.from_env(_LOG, origin="popup", pid=os.getpid())


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


def estimate_objects_size(items):
    count = len(items)
    longest = max((len(s) for s in items), default=0)
    width = min(max(160, longest * 7 + 28), 320)
    height = min(max(32, count * 22 + 10), 240)
    return width, height


def estimate_text_size(text):
    lines = [ln for ln in text.splitlines() if ln.strip()]
    longest = max((len(ln) for ln in lines), default=len(text))
    width = min(max(260, longest * 7 + 48), 560)
    height = min(max(90, max(3, len(lines)) * 22 + 32), 420)
    return width, height


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


def apply_popup_font(Gtk, Gdk):
    provider = Gtk.CssProvider()
    provider.load_from_data(b'* { font-family: "Noto Sans"; font-size: 14px; }')
    screen = Gdk.Screen.get_default()
    if screen is not None:
        Gtk.StyleContext.add_provider_for_screen(
            screen, provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )


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


def show_input_popup(prompt_text, px=None, py=None):
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
            self.connect('focus-out-event', self.on_cancel)
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

            label = Gtk.Label(label=prompt, xalign=0)
            label.set_line_wrap(True)
            label.set_max_width_chars(34)
            label.set_xalign(0)
            box.pack_start(label, False, False, 0)

            self.entry = Gtk.Entry()
            self.entry.set_activates_default(True)
            self.entry.connect('activate', self.on_submit)
            box.pack_start(self.entry, False, False, 0)
            self.set_default(self.entry)

            self.resize(340, 94)
            self.show_all()
            GLib.idle_add(self._finalize_open)

        def _finalize_open(self):
            self.move_to(self.px, self.py)
            self.present()
            self.present_with_time(int(time.time() * 1000))
            self.entry.grab_focus()
            self.entry.set_position(-1)
            _CHAIN.emit('window_open', window='input', detail=prompt_text)
            return False

        def move_to(self, px, py):
            if px is None or py is None:
                px, py = get_mouse_position()
            self.move(int(px) + 12, int(py) + 12)

        def on_submit(self, *args):
            text = self.entry.get_text().strip()
            _CHAIN.emit('submit', window='input', detail=f'{len(text)} chars')
            sys.stdout.write(text)
            sys.stdout.flush()
            Gtk.main_quit()
            return True

        def on_copy(self, *args):
            copy_text_to_clipboard(self.entry.get_text().strip())
            _CHAIN.emit('copy', window='input')
            return True

        def on_cancel(self, *args):
            _CHAIN.emit('window_close', window='input', detail='cancel')
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
                label.set_max_width_chars(max(14, min(34, width // 8)))
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
            _CHAIN.emit('copy', window='objects', detail=f'{len(self.items)} objects')
            return True

        def on_click(self, *args):
            return self.close_and_quit(reason='click')

        def on_key(self, widget, event):
            if event.keyval in (Gdk.KEY_Escape, Gdk.KEY_Return, Gdk.KEY_KP_Enter):
                return self.close_and_quit()
            return False

        def close_and_quit(self, reason='key'):
            _CHAIN.emit('window_close', window='objects', detail=reason)
            self.destroy()
            Gtk.main_quit()
            return True

    popup = Popup(items, px, py)
    _CHAIN.emit('window_open', window='objects', detail=f'{len(items)} objects')
    Gtk.main()
    return 0


def show_menu_popup(items, px=None, py=None, title='Menu'):
    """Render selectable action rows and return the selected action on stdout."""
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

    width = min(max(220, max(len(label) for label, _ in rows) * 7 + 48), 420)

    class MenuPopup(Gtk.Window):
        def __init__(self):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.set_decorated(False)
            self.set_resizable(False)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_keep_above(True)
            self.set_type_hint(Gdk.WindowTypeHint.POPUP_MENU)
            self.set_title(title)
            self.connect('key-press-event', self.on_key)
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
                text.set_max_width_chars(42)
                row.add(text)
                box.pack_start(row, False, False, 0)
            self.show_all()
            GLib.idle_add(self.place)

        def place(self):
            self.resize(width, max(40, len(rows) * 34 + 12))
            if px is None or py is None:
                mx, my = get_mouse_position()
            else:
                mx, my = int(px), int(py)
            self.move(mx + 12, my + 12)
            self.present()
            _CHAIN.emit('window_open', window='menu', detail=title)
            return False

        def choose(self, _widget, event, action):
            if getattr(event, 'button', 0) == 3:
                _CHAIN.emit('window_close', window='menu', detail='right_click')
                self.close()
                return True
            index = next((i for i, (_l, a) in enumerate(rows) if a == action), None)
            label = rows[index][0] if index is not None else action
            _CHAIN.emit('click', window='menu', layer=1, item_index=index, item_label=label, detail=action)
            sys.stdout.write(json.dumps({'action': action}, ensure_ascii=False))
            sys.stdout.flush()
            self.close()
            return True

        def on_key(self, _widget, event):
            if event.keyval == Gdk.KEY_Escape:
                _CHAIN.emit('window_close', window='menu', detail='escape')
                self.close()
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
            self.meaning = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
            self.columns.pack_start(self.left, False, False, 0)
            self.columns.pack_start(self.right, False, False, 0)
            self.columns.pack_start(self.meaning, False, False, 0)
            self.meaning.hide()
            for index, (simple, term, meaning) in enumerate(rows):
                row = Gtk.EventBox()
                row.set_visible_window(True)
                row.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
                row.connect('button-press-event', self.show_terms, index)
                row.set_size_request(-1, 28)
                cell = Gtk.Label(label=simple, xalign=0)
                cell.set_margin_top(5); cell.set_margin_bottom(5)
                cell.set_margin_start(9); cell.set_margin_end(9)
                cell.set_width_chars(12)
                cell.set_max_width_chars(16)
                row.add(cell)
                self.left.pack_start(row, False, False, 0)
            self.show_all()
            self.meaning.hide()
            GLib.idle_add(self.place)

        def place(self):
            self.resize(190, max(42, len(rows) * 32 + 8))
            mx, my = (get_mouse_position() if px is None or py is None else (int(px), int(py)))
            self.move(mx + 12, my + 12)
            self.present()
            _CHAIN.emit('window_open', window='dual', layer=1, detail=title or f'{len(rows)} cues')
            return False

        def show_terms(self, _widget, event, _index):
            if getattr(event, 'button', 0) == 3:
                return self.close(reason='right_click')
            if not isinstance(_index, int) or not 0 <= _index < len(rows):
                return True
            simple, term = rows[_index][0], rows[_index][1]
            _CHAIN.emit('click', window='dual', layer=1, item_index=_index, item_label=simple)
            # The second column is revealed one word at a time: a click opens the
            # term of that word only, so all four are never handed over at once.
            self.revealed.add(_index)
            self.render_terms()
            _CHAIN.emit(
                'layer_open', window='dual', layer=2, item_index=_index,
                item_label=term, detail='word revealed',
            )
            return True

        def render_terms(self):
            """Keep one row per word so a revealed term lines up with its word."""
            for child in self.right.get_children():
                self.right.remove(child)
            for index, (_simple, term, _meaning) in enumerate(rows):
                holder = Gtk.EventBox()
                holder.set_size_request(-1, 28)
                if index in self.revealed:
                    holder.set_visible_window(True)
                    holder.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
                    holder.connect('button-press-event', self.show_meaning, index)
                    cell = Gtk.Label(label=term, xalign=0)
                    cell.set_width_chars(22)
                    cell.set_max_width_chars(24)
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
            self.resize(420, max(42, len(rows) * 28 + 8))

        def show_meaning(self, _widget, event, index):
            if getattr(event, 'button', 0) == 3:
                return self.close(reason='right_click')
            term = rows[index][1] if isinstance(index, int) and 0 <= index < len(rows) else None
            _CHAIN.emit('click', window='dual', layer=2, item_index=index, item_label=term)
            if index in self.active:
                self.active.remove(index)
                shown = False
            else:
                self.active.add(index)
                shown = True
            for child in self.meaning.get_children():
                self.meaning.remove(child)
            _CHAIN.emit(
                'layer_toggle', window='dual', layer=3, item_index=index,
                item_label=term, detail='shown' if shown else 'hidden',
            )
            if not self.active:
                self.meaning.hide()
                self.resize(420, max(42, len(rows) * 28 + 8))
                return True
            for row_index, (_row_simple, row_term, row_meaning) in enumerate(rows):
                holder = Gtk.EventBox()
                holder.set_visible_window(False)
                holder.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
                holder.connect('button-press-event', self.close_on_right)
                if row_index in self.active:
                    detail = Gtk.Label(label=row_meaning or row_term, xalign=0)
                    detail.set_line_wrap(False)
                    detail.set_width_chars(40)
                    detail.set_max_width_chars(44)
                    detail.set_margin_top(5); detail.set_margin_bottom(5)
                    detail.set_margin_start(12); detail.set_margin_end(12)
                    detail.set_size_request(-1, 18)
                    holder.add(detail)
                else:
                    blank = Gtk.Label(label='', xalign=0)
                    blank.set_size_request(-1, 18)
                    holder.add(blank)
                holder.set_size_request(-1, 28)
                self.meaning.pack_start(holder, False, False, 0)
            self.meaning.show_all()
            self.resize(760, max(42, len(rows) * 28 + 8))
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
            _CHAIN.emit(
                'window_close', window='dual',
                detail=f'{reason} after {time.monotonic() - self._opened:.1f}s',
            )
            self.destroy(); Gtk.main_quit(); return True

    DualPopup()
    Gtk.main()
    return 0


def show_text_popup(text, px=None, py=None, title='Result'):
    try:
        import gi
        gi.require_version('Gtk', '3.0')
        gi.require_version('Gdk', '3.0')
        from gi.repository import Gtk, Gdk, GLib
        apply_popup_font(Gtk, Gdk)
    except Exception:
        return 1

    width, height = estimate_text_size(text)

    class TextPopup(Gtk.Window):
        def __init__(self, body, px=None, py=None):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.body = body
            self.px = px
            self.py = py
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

            view = Gtk.TextView()
            view.set_editable(False)
            view.set_cursor_visible(False)
            view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
            view.set_left_margin(4)
            view.set_right_margin(4)
            view.get_buffer().set_text(body)
            box.pack_start(view, True, True, 0)

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
            copy_text_to_clipboard(self.body)
            _CHAIN.emit('copy', window='text', detail=title)
            return True

        def on_key(self, widget, event):
            if event.keyval in (Gdk.KEY_Escape, Gdk.KEY_Return, Gdk.KEY_KP_Enter):
                return self.close_and_quit()
            return False

        def close_and_quit(self, reason='key'):
            _CHAIN.emit('window_close', window='text', detail=reason)
            self.destroy()
            Gtk.main_quit()
            return True

    TextPopup(text, px, py)
    _CHAIN.emit('window_open', window='text', detail=title)
    Gtk.main()
    return 0


def show_note_popup(anchor='', px=None, py=None):
    """Multi-line capture for an error note; prints the comment on stdout.

    The reader's own words need more room than the single-line input popup, and
    Enter has to insert a newline: saving is Ctrl+Enter or the button.
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
            self.set_title('Error note')
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

            label = Gtk.Label(label=anchor or 'нет выделения', xalign=0)
            label.set_line_wrap(True)
            label.set_max_width_chars(46)
            label.set_xalign(0)
            box.pack_start(label, False, False, 0)

            self.view = Gtk.TextView()
            self.view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
            self.view.set_left_margin(4)
            self.view.set_right_margin(4)
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

            self.resize(410, 230)
            self.show_all()
            GLib.idle_add(self.place)

        def place(self):
            if self.px is None or self.py is None:
                mx, my = get_mouse_position()
            else:
                mx, my = int(self.px), int(self.py)
            self.move(mx + 12, my + 12)
            self.present()
            self.present_with_time(int(time.time() * 1000))
            self.view.grab_focus()
            _CHAIN.emit('window_open', window='note', detail=self.anchor or 'no selection')
            return False

        def text(self):
            buffer = self.view.get_buffer()
            return buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True).strip()

        def on_submit(self, *_args):
            text = self.text()
            if not text:
                return True
            _CHAIN.emit('submit', window='note', detail=f'{len(text)} chars')
            sys.stdout.write(text)
            sys.stdout.flush()
            self.destroy()
            Gtk.main_quit()
            return True

        def on_cancel(self, *_args):
            _CHAIN.emit('window_close', window='note', detail='cancel')
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


def _dispatch(payload, mode):
    if mode == 'note':
        return show_note_popup(
            str(payload.get('anchor', '')), payload.get('x'), payload.get('y')
        )

    if mode == 'input':
        prompt_text = str(payload.get('prompt', 'Введите непонятное ядро или слова:')).strip()
        return show_input_popup(prompt_text, payload.get('x'), payload.get('y'))

    if mode == 'menu':
        items = payload.get('items', [])
        if isinstance(items, list):
            return show_menu_popup(items, payload.get('x'), payload.get('y'), str(payload.get('title', 'Menu')))
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
        return show_text_popup(text, px, py, title=title)

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
    payload = load_payload()
    mode = str(payload.get('mode', 'objects')).lower().strip()
    _CHAIN.emit('popup_start', window=mode)
    try:
        return _dispatch(payload, mode)
    except Exception as exc:
        # A popup that dies before Gtk.main() leaves no visible trace otherwise.
        _CHAIN.emit('error', window=mode, detail=f'{type(exc).__name__}: {exc}')
        raise


if __name__ == '__main__':
    raise SystemExit(main())
