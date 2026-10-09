"""Semantic feedback is owned by the displayed popup, never its launcher."""
import json
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace

import pytest

from cognitive_popups import popup_helper as helper
from cognitive_popups.models import CognitiveSession, PredictionCheck


@pytest.mark.parametrize('status', ['contradicted', 'partially_confirmed', 'confirmed',
                                   'not_supported', 'unsupported', 'unclear', 'clarification'])
@pytest.mark.parametrize('delta,mismatch', [('correction', ''), ('', 'legacy correction'), ('  ', '  ')])
def test_prediction_correction_mapping(desktop, monkeypatch, status, delta, mismatch):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.service = SimpleNamespace(session=CognitiveSession())
    calls = []
    monkeypatch.setattr(desktop.threading, 'Thread', lambda **kw: SimpleNamespace(start=kw['target']))
    monkeypatch.setattr(desktop, 'run_popup', lambda payload, **kw: calls.append(payload) or '')
    monkeypatch.setattr(desktop.sound, 'play', lambda *a: pytest.fail('parent must be silent'))
    app._prediction_done(PredictionCheck([], 'claim', status, mismatch=mismatch, one_delta=delta), None)
    expected = status in {'contradicted', 'partially_confirmed'} and bool(delta.strip() or mismatch.strip())
    assert calls[0].get('semantic_role') == ('correction' if expected else None)


@pytest.mark.parametrize('role', ['correction', 'resolve', 'materialize', 'reveal', 'dismiss',
                                 'arbitrary', None, [], {}])
def test_dispatch_filters_text_roles(monkeypatch, role):
    calls = []
    monkeypatch.setattr(helper, 'show_text_popup', lambda *a, **kw: calls.append(kw) or 0)
    helper._dispatch({'text': 'result', 'semantic_role': role}, 'text')
    assert calls[0]['semantic_role'] == (role if role in ('correction', 'resolve', 'materialize') else None)


def test_prediction_compact_and_legacy_expanded(monkeypatch):
    calls = []
    monkeypatch.setattr(helper, 'show_text_popup', lambda *a, **kw: calls.append(kw) or 0)
    for extra in ({'one_delta': 'delta'}, {'one_delta': ''}, {}, {'one_delta': 'delta', 'expanded': True}):
        helper._dispatch({'text': 'result', 'title': 'Моя гипотеза', **extra}, 'text')
    assert [call['expanded'] for call in calls] == [False, False, True, True]


@pytest.mark.parametrize('status,role', [('passed', 'resolve'), ('needs_retry', 'correction'), ('unclear', None)])
def test_focus_routes_feedback(desktop, monkeypatch, status, role):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    app.focus = SimpleNamespace(snapshot=lambda: {'id': 'block', 'status': 'completed'})
    calls = []
    app.flash = SimpleNamespace(show_text=lambda *a, **kw: calls.append(kw))
    monkeypatch.setattr(desktop.sound, 'play', lambda *a: pytest.fail('parent must be silent'))
    app._focus_result('block', {'status': status, 'text': 'result', 'gaps': []})
    assert calls == [{'semantic_role': role}]


@pytest.mark.parametrize('status,role', [('passed', 'resolve'), ('needs_retry', 'correction')])
def test_feynman_routes_feedback(desktop, monkeypatch, status, role):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    calls = []
    app.flash = SimpleNamespace(show_text=lambda *a, **kw: calls.append((a, kw)))
    monkeypatch.setattr(desktop.sound, 'play', lambda *a: pytest.fail('parent must be silent'))
    app._check_done(status, [], None)
    assert calls[0][1]['semantic_role'] == role
    if status == 'passed':
        assert calls[0] == (('Достаточно.',), {'title': 'Фейнман', 'semantic_role': 'resolve'})


def test_transport_failure_is_silent(desktop, monkeypatch):
    monkeypatch.setattr(desktop, 'popup_helper_command', lambda: ['helper'])
    monkeypatch.setattr(desktop.subprocess, 'Popen', lambda *a, **kw: (_ for _ in ()).throw(OSError('transport')))
    monkeypatch.setattr(desktop.sound, 'play', lambda *a: pytest.fail('failed transport must be silent'))
    desktop.FlashWindow.show_text(object(), 'correction', semantic_role='correction')


def test_actual_gtk_correction_callback_once(tmp_path):
    """Real GTK mapping/remapping; audio replaced with a callback log."""
    if not (os.environ.get('DISPLAY') or os.environ.get('WAYLAND_DISPLAY')):
        pytest.skip('real GTK sample requires a display')
    script = '''
import json, sys
sys.path.insert(0, sys.argv[1])
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib
from cognitive_popups import popup_helper as h
if not Gtk.init_check()[0]:
    sys.exit(77)
log = []
def play(role):
    window = Gtk.Window.list_toplevels()[0]
    log.append(['sound', role, window.get_mapped()])
h.sound.play = play
h._emit = lambda event, **kw: log.append(['event', event])
h._rendered = lambda **kw: None
def remap():
    window = Gtk.Window.list_toplevels()[0]
    window.hide()
    window.show_all()
    return False
def close():
    window = Gtk.Window.list_toplevels()[0]
    window.close_and_quit()
    return False
GLib.timeout_add(150, remap)
GLib.timeout_add(300, close)
h.show_text_popup('One meaningful correction.', semantic_role='correction')
print(json.dumps(log))
'''
    source = str(Path(helper.__file__).resolve().parents[1])
    result = subprocess.run(['/usr/bin/python3', '-c', script, source], capture_output=True,
                            text=True, timeout=10)
    if result.returncode == 77:
        pytest.skip('GTK display unavailable')
    assert result.returncode == 0, result.stderr
    log = json.loads(result.stdout.splitlines()[-1])
    assert [row for row in log if row[0] == 'sound'] == [['sound', 'correction', True]]
    assert log.count(['event', 'correction_shown']) == 1
    assert log.index(['event', 'correction_shown']) < log.index(['sound', 'correction', True])
