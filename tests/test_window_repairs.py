from types import SimpleNamespace

import pytest

from cognitive_popups import capture, input_shapes
from cognitive_popups.models import PredictionCheck
from cognitive_popups.sand_cues import SandWords, plan
from cognitive_popups.popup_helper import connect_popup_keys


def test_escape_repeat_is_consumed_until_target_receives_release():
    signals, closed = {}, []
    target = SimpleNamespace(connect=lambda name, callback: signals.update({name: callback}))
    gdk = SimpleNamespace(KEY_Escape=27)
    connect_popup_keys(target, gdk, lambda widget, event: closed.append(widget) or True)
    event = SimpleNamespace(keyval=27)
    for _ in range(20):
        assert signals['key-press-event'](target, event)
    assert not closed
    assert signals['key-release-event'](target, event)
    assert closed == [target]
    signals['key-release-event'](target, event)
    assert closed == [target]


@pytest.mark.parametrize('selection', [(0, 200, 1100, 400), (0, 0, 1200, 800),
                                      (400, 720, 800, 800), (1100, 0, 1200, 80)])
def test_waiting_core_and_caption_fit_window_and_screen(selection):
    p = plan((0, 0, 1200, 800), (1199, 799), selection=selection)
    x, y = p.core
    assert 60 <= x <= p.width - 60
    assert 30 <= y <= p.height - 54
    assert 0 <= p.position[0] <= 1200 - p.width
    assert 0 <= p.position[1] <= 800 - p.height


def test_selection_and_waiting_do_not_capture_pointer(monkeypatch):
    ui = SandWords({})
    ui.plan = plan((0, 0, 1200, 800), (600, 400), selection=(400, 350, 800, 450))
    ui.window = SimpleNamespace(get_window=lambda: 'native')
    ui.detail_box, ui.buttons, ui.opened = None, [], False
    ui.extents = [(cx-115, cy-16, cx-30, 60, 32) for cx, cy, _ in ui.plan.words]
    ui.sand = SimpleNamespace(magnets=[SimpleNamespace(x=0, y=0) for _ in range(4)])
    shapes = []
    monkeypatch.setattr(input_shapes, 'apply', lambda _, runs: shapes.append(runs))
    ui.shape()
    assert not shapes[-1]
    ui.opened = True
    ui.shape()
    x = 600 - ui.plan.position[0]
    y = 400 - ui.plan.position[1]
    assert not any(rx <= x < rx+w and ry <= y < ry+h for rx, ry, w, h in shapes[-1])
    assert len(shapes[-1]) == 4
    # Empty padding beside the word is not an invisible scroll blocker either.
    cx, cy, _ = ui.plan.words[0]
    assert not any(rx <= cx-100 < rx+w and ry <= cy < ry+h for rx, ry, w, h in shapes[-1])


def test_primary_from_another_document_is_not_a_copy_fallback(monkeypatch):
    monkeypatch.setattr(capture, 'active_window', lambda: {'class': 'browser', 'address': '0xabc', 'title': 'new'})
    monkeypatch.setattr(capture, 'cursor_pos', lambda: None)
    monkeypatch.setattr(capture, '_try_copy', lambda *args: '')
    monkeypatch.setattr(capture, '_try_primary', lambda *args: pytest.fail('unverified old selection'))
    result = capture.capture_selection()
    assert not result.text
    assert 'primary owner unverified; ignored' in result.notes


def test_empty_clipboard_is_a_valid_snapshot(monkeypatch):
    monkeypatch.setattr(capture, '_run', lambda *a, **kw: (1, b'Nothing is copied\n'))
    assert capture.save_clipboard().empty
    monkeypatch.setattr(capture, '_run', lambda *a, **kw: (1, b'Failed to connect to a Wayland server'))
    assert not capture.save_clipboard().empty
    monkeypatch.setattr(capture, '_run', lambda *a, **kw: (0, b'application/x-unknown\n'))
    assert not capture.save_clipboard().empty


def test_autocopy_runs_with_empty_clipboard_and_restores_it(monkeypatch):
    empty = capture.SavedClipboard(empty=True)
    monkeypatch.setattr(capture, 'save_clipboard', lambda: empty)
    values = iter(['', 'выделенный текст', 'выделенный текст'])
    monkeypatch.setattr(capture, 'read_clipboard_text', lambda: next(values))
    monkeypatch.setattr(capture, 'stamp_mtime', lambda p: 123)
    sent, restored = [], []
    monkeypatch.setattr(capture, '_run', lambda cmd, **kw: (sent.append(cmd) or 0, b''))
    monkeypatch.setattr(capture, 'restore_clipboard', restored.append)
    assert capture._try_copy(capture.CaptureResult(window_class='firefox'), '0xabc', 300, False) == 'выделенный текст'
    assert sent and restored == [empty]


def test_stale_watcher_cannot_hide_a_changed_text_or_accept_unchanged_text():
    assert capture.copy_changed(10, 10, 'старое', 'новое')
    assert not capture.copy_changed(10, 10, 'старое', 'старое')
    assert not capture.copy_changed(10, 11, 'старое', '')


@pytest.mark.parametrize('notes,message', [
    (['secret clipboard'], 'Захват пропущен: защищённый буфер обмена'),
    (['clipboard not restorable'], 'Не удалось сохранить прежний формат буфера обмена'),
    (['sendshortcut failed: refused'], 'Не удалось отправить команду копирования'),
    (['copy produced no fresh text'], 'Копирование не передало выделенный текст'),
])
def test_capture_failure_names_the_actual_operation(notes, message):
    assert capture.failure_message(notes) == message


def test_context_switch_during_copy_cannot_attach_text_to_old_title(monkeypatch):
    windows = iter([{'class': 'browser', 'address': '0xabc', 'title': 'HR'},
                    {'class': 'browser', 'address': '0xabc', 'title': 'posture'}])
    monkeypatch.setattr(capture, 'active_window', lambda: next(windows))
    monkeypatch.setattr(capture, 'cursor_pos', lambda: None)
    monkeypatch.setattr(capture, '_try_copy', lambda *args: 'текст HR')
    assert not capture.capture_selection().text


def test_reopen_only_displays_saved_answer_without_changing_state(desktop):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    shown = []
    app.flash = SimpleNamespace(show_text=lambda *a, **kw: shown.append((a, kw)))
    app._busy = True
    check = PredictionCheck([], 'моя мысль', 'confirmed', display='мой ответ', ask='дальше?',
                            evidence='источник', nodes=['архив'])
    for _ in range(3):
        app.reopen_prediction(check)
    assert len(shown) == 3 and app._busy
    assert all(args[0] == check.display for args, _ in shown)
    assert shown[0][1]['note'] == 'моя мысль'
    assert shown[0][1]['evidence'] == 'источник'


def test_reopen_nodes_preserves_display_without_duplicating_mark(desktop):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    shown = []
    app.flash = SimpleNamespace(show_text=lambda text, *a, **kw: shown.append(text))
    display = desktop.nodes.describe('partly', ['архив', 'процесс'])
    check = PredictionCheck([], 'мысль', 'partially_confirmed', display=display,
                            nodes=['архив', 'процесс'], mark='partly')
    app.reopen_prediction(check)
    assert shown == [display]
    check.display = ''  # Older JSON without the rendered field.
    app.reopen_prediction(check)
    assert shown == [display, display]


@pytest.mark.parametrize('error,status', [(None, 'done'), ('ошибка', 'error')])
def test_alt_c_wait_finishes_once_and_keeps_other_wait(desktop, error, status):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    seen = []
    app._clarify_wait = SimpleNamespace(finish=lambda **kw: seen.append(kw))
    other = app._sand_wait = object()
    app._finish_clarify_wait(error)
    app._finish_clarify_wait(error)
    assert seen == [{'status': status}] and app._sand_wait is other


def test_saved_answers_can_be_recovered_after_daemon_restart(desktop, monkeypatch, tmp_path):
    from cognitive_popups.models import CognitiveSession
    from cognitive_popups.records import RecordStore
    from cognitive_popups.history import SessionHistory
    store = RecordStore(tmp_path / 'records.sqlite3')
    session = CognitiveSession()
    check = PredictionCheck([], 'гипотеза', 'confirmed', display='сохранённый ответ', nodes=['архив'])
    store.save_prediction(session, check)
    monkeypatch.setattr(desktop, 'RECORDS', RecordStore(store.path))
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.service = SimpleNamespace(session=CognitiveSession())
    app.history = SessionHistory(tmp_path / 'history.json')
    loaded = app.saved_predictions()
    assert len(loaded) == 1
    assert loaded[0].id == check.id and loaded[0].display == check.display
    assert loaded[0].nodes == ['архив']
