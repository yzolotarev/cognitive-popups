import threading
from types import SimpleNamespace

import pytest
from cognitive_popups.records import RecordStore
from cognitive_popups.study_goals import StudyGoals
from cognitive_popups.manual_queue import selection


def test_boundary_restart_reuse_and_old_step(tmp_path):
    records = RecordStore(tmp_path/'records.sqlite3')
    goals = StudyGoals(records)
    assert goals.needs_goal(now=100)
    first = goals.accept('Хочу понять X', now=100, step_session_key='three-hour-session')
    records.add_step('old', 'read', session_key='three-hour-session', minutes=15,
                     goal_id=first['goal_id'], started_epoch=100, deadline_epoch=1000)
    assert not StudyGoals(RecordStore(records.path)).needs_goal(now=3699.999)
    assert goals.needs_goal(now=3700)
    assert not goals.needs_goal(now=9000, active_step=True)
    renewed = goals.accept(records.intention(first['goal_id']).text, now=3700)
    goals.bind_step_session('three-hour-session')
    assert renewed['id'] != first['id']
    assert renewed['goal_id'] != first['goal_id']
    assert records.step('old')['goal_id'] == first['goal_id']
    assert goals.current()['step_session_key'] == 'three-hour-session'
    goals.touch(now=4000)
    assert not goals.needs_goal(now=7599)
    assert goals.needs_goal(now=7600)


def make_app(desktop, tmp_path, monkeypatch):
    app = desktop.DesktopApp.__new__(desktop.DesktopApp)
    records = RecordStore(tmp_path/'records.sqlite3')
    monkeypatch.setattr(desktop, 'RECORDS', records)
    app.study_goals = StudyGoals(records)
    app._study_goal_lock = threading.Lock()
    app.focus = SimpleNamespace(snapshot=lambda:None)
    app.flash = SimpleNamespace(show_notice=lambda *_:None, show_text=lambda *_:None)
    app._split_idle_session = lambda:None
    class ImmediateThread:
        def __init__(self, target, **kw): self.target=target
        def start(self): self.target()
    monkeypatch.setattr(desktop, 'threading', SimpleNamespace(Thread=ImmediateThread))
    app._study_key = lambda:'old-step-session'
    monkeypatch.setattr(desktop, 'idle_add', lambda fn,*args:fn(*args))
    monkeypatch.setattr(desktop, 'emit', lambda *a,**k:None)
    return app


def test_cancel_and_retry(desktop, tmp_path, monkeypatch):
    app=make_app(desktop,tmp_path,monkeypatch)
    calls=[]
    monkeypatch.setattr(desktop,'run_popup',lambda payload,**kw:calls.append(payload) or '')
    app._seed_material=lambda *a:pytest.fail('cancel must not continue')
    monkeypatch.setattr(desktop,'selected_text_source',lambda:('original','copy'))
    selection.context='study'
    app.seed(); app.seed()
    assert len(calls)==2
    assert app.study_goals.current() is None
    assert not app._seed_request_running


def test_snapshot_duplicate_busy_and_resume(desktop, tmp_path, monkeypatch):
    app=make_app(desktop,tmp_path,monkeypatch)
    reads=[]; forms=[]; result=[]; timers=[]
    def capture():
        reads.append(1); selection.context='study'
        return 'original passage','copy'
    monkeypatch.setattr(desktop,'selected_text_source',capture)
    def form(payload,**kw):
        forms.append(payload)
        app.seed()  # Concurrent Alt+W while form is open.
        monkeypatch.setattr(desktop,'selected_text_source',lambda:pytest.fail('recapture'))
        return 'Хочу понять'
    monkeypatch.setattr(desktop,'run_popup',form)
    monkeypatch.setattr(desktop.GLib,'timeout_add',lambda ms, fn:timers.append(fn),raising=False)
    monkeypatch.setattr(desktop,'bind_chain',lambda fn:fn)
    app._seed_material=lambda *a:result.append(a)
    app._busy=True
    app.seed()
    assert len(reads)==len(forms)==1 and result==[]
    app.seed()
    app._busy=False
    timers.pop()()
    assert result==[('original passage','copy',False)]
    assert app.study_goals.records.current_intention().material=='original passage'


@pytest.mark.parametrize('dev,text', [(True,'dev text'),(False,'')])
def test_dev_and_failed_capture_no_form(desktop,tmp_path,monkeypatch,dev,text):
    app=make_app(desktop,tmp_path,monkeypatch)
    monkeypatch.setattr(desktop,'run_popup',lambda *a,**kw:pytest.fail('unexpected goal form'))
    monkeypatch.setattr(desktop,'selected_text_source',lambda:(text,'copy'))
    selection.context='dev' if dev else 'study'
    app._seed_material=lambda *a:None
    app.seed()
    assert app.study_goals.current() is None


def test_previous_goal_explicit_reuse(desktop,tmp_path,monkeypatch):
    app=make_app(desktop,tmp_path,monkeypatch)
    previous=app.study_goals.accept('Прежняя цель',now=0)
    def form(payload,**kw):
        assert payload['reuse']=='Прежняя цель'
        assert payload['required']
        return payload['reuse']
    monkeypatch.setattr(desktop,'run_popup',form)
    assert app._ensure_study_goal('snapshot','copy')
    assert app.study_goals.current()['goal_id'] != previous['goal_id']
    assert app.study_goals.records.current_intention().text=='Прежняя цель'


def test_expired_goal_stays_due_during_step(tmp_path):
    goals=StudyGoals(RecordStore(tmp_path/'records.sqlite3'))
    goals.accept('Старая цель',now=10)
    assert not goals.needs_goal(now=3610,active_step=True)
    goals.touch(now=3610)
    assert goals.needs_goal(now=3611,active_step=False)


def test_firefox_copy_recovers_alt_menu(monkeypatch):
    from cognitive_popups import capture
    clipboard=['before']; commands=[]
    monkeypatch.setattr(capture,'save_clipboard',lambda:capture.SavedClipboard('text/plain',b'before'))
    monkeypatch.setattr(capture,'read_clipboard_text',lambda:clipboard[0])
    monkeypatch.setattr(capture,'stamp_mtime',lambda _:None)
    monkeypatch.setattr(capture,'active_window',lambda:{'address':'0x123','title':'Article'})
    monkeypatch.setattr(capture,'restore_clipboard',lambda saved:clipboard.__setitem__(0,saved.data.decode()))
    def run(command):
        commands.append(command)
        if len(commands)==3: clipboard[0]='original paragraph'
        return 0,b''
    monkeypatch.setattr(capture,'_run',run)
    result=capture.CaptureResult(window_class='firefox',window_title='Article')
    assert capture._try_copy(result,'0x123',1,False)=='original paragraph'
    assert 'Escape' in commands[1][2]
    assert clipboard[0]=='before'
