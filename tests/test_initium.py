"""Explicit journeys use the actual step flow, with isolated ledgers and scripted windows."""
import json
from types import SimpleNamespace

import pytest
from cognitive_popups.records import RecordStore
from cognitive_popups.sand_store import SandStore
from cognitive_popups.focus import Focus
from cognitive_popups.initium import identity_for, Birth, at_end, progress
from cognitive_popups.session_scene import scene_for_session, session_thoughts
from test_steps import _app


@pytest.mark.parametrize('target',[1,3,5,None])
def test_length_identity_and_restart(tmp_path,target):
    records=RecordStore(tmp_path/'r.sqlite3')
    j=records.ensure_journey('a'); records.plan_journey('a',target)
    restored=RecordStore(records.path).open_journey()
    assert restored['identity']==j['identity']==identity_for('a')
    assert restored['target_steps']==target and restored['completed']==0
    assert progress(restored)==f"0/{target or '∞'}"


def test_spark_is_present_at_time_zero_and_physics_is_bounded():
    birth=Birth(identity_for('a'),(150,200),(800,100))
    assert birth.pose(0)==(150,200,.25)
    for i in range(215): birth.advance(i/60,1/60)
    x,y,scale=birth.pose(3.55)
    assert x==800 and y==pytest.approx(100) and scale==.62
    assert all(abs(p['x'])<2000 and abs(p['y'])<2000 for p in birth.particles)


def test_visual_precedes_capture_and_popup(desktop,monkeypatch,tmp_path):
    app,records,*_= _app(desktop,monkeypatch,tmp_path,[''],[])
    order=[]
    app._show_initium=lambda *a,**k: order.append('spark')
    original=desktop.popup_input
    monkeypatch.setattr(desktop,'popup_input',lambda *a,**k: (order.append('input'),original(*a,**k))[1])
    app.start_step()
    assert order==['spark','input']
    assert app.focus.snapshot()['study_step']


@pytest.mark.parametrize('target',[1,3,5])
def test_finale_only_after_last_confirmation_once(desktop,monkeypatch,tmp_path,target):
    answers=['read'] + ['']*target
    choices=['done','break']*(target-1)+['done']
    app,records,*_=_app(desktop,monkeypatch,tmp_path,answers,choices)
    app.start_step(); key=records.last_step()['session_key']; records.plan_journey(key,target)
    finales=[]
    def close(key,**kwargs):
        if records.claim_finale(key):
            finales.append(key)
            records.close_study_session(key)
            records.finale_state(key,'shown')
    app._close_study_session=close
    for i in range(target):
        if i:
            answers.insert(0,'read'); app.start_step()
        block=app.focus.snapshot()['id']
        assert not finales
        app._step_end(block)
        if i<target-1: assert not finales
    assert finales==[key]
    app._step_end(block)
    assert finales==[key]
    assert records.journey(key)['completed']==target


def test_free_never_ends_from_gap_or_timer(desktop,monkeypatch,tmp_path):
    app,records,*_=_app(desktop,monkeypatch,tmp_path,['read','','read'],['done','break'])
    app.start_step(); first=app.focus.snapshot()['id']; key=records.last_step()['session_key']
    app._step_end(first)
    monkeypatch.setattr(desktop.steps,'is_new_session',lambda *a,**k: True)
    app.start_step()
    assert records.last_step()['session_key']==key
    assert not at_end(records.journey(key)) and records.closure(key) is None


def test_overdue_study_step_resumes_but_legacy_focus_does_not(tmp_path):
    r=RecordStore(tmp_path/'r.sqlite3'); f=Focus(tmp_path/'f.json',r,None,clock=lambda:0)
    state=f.start('читаю',5,study_step=True)
    assert Focus(f.path,r,None,clock=lambda:10).snapshot()==state
    legacy=Focus(tmp_path/'old.json',r,None,clock=lambda:0); legacy.start('x',5)
    assert Focus(legacy.path,r,None,clock=lambda:10).snapshot()['status']=='interrupted'


def test_confirmation_idempotent_and_claim_atomic(tmp_path):
    r=RecordStore(tmp_path/'r.sqlite3'); r.ensure_journey('s')
    r.add_step('a','x',session_key='s',minutes=15,started_epoch=10,deadline_epoch=20)
    assert not at_end(r.journey('s'))
    assert r.finish_step('a','partly','real')
    assert not r.finish_step('a','done','fake duplicate')
    assert r.step('a')['takeaway']=='real'
    assert r.claim_finale('s') and not RecordStore(r.path).claim_finale('s')


def test_actual_scene_reuses_identity_without_fiction(tmp_path):
    r=RecordStore(tmp_path/'r.sqlite3'); j=r.ensure_journey('s')
    r.add_step('a','read',session_key='s',minutes=15,started_epoch=100,deadline_epoch=200)
    r.finish_step('a','done','real takeaway',finished_utc='1970-01-01T00:02:30+00:00')
    sand=SandStore(r.path); rows=r.session_steps('s')
    notes=[SimpleNamespace(id=1,created_utc='1970-01-01T00:02:00+00:00',comment='real note'),
           SimpleNamespace(id=2,created_utc='1970-01-01T00:09:00+00:00',comment='outside')]
    thoughts=session_thoughts(rows,notes,sand)
    assert [t['text'] for t in thoughts]==['real note','real takeaway']
    scene=scene_for_session('s',rows,notes,sand)
    assert scene['initium']==j['identity'] and scene['tint']==j['identity']['tint']
    texts=[s['text'] for s in scene['stars']]
    assert 'outside' not in texts
    assert sum(s['kind']=='initium' for s in scene['stars'])==1


def test_automatic_close_has_no_additional_forms(desktop,monkeypatch,tmp_path):
    app,r,*_=_app(desktop,monkeypatch,tmp_path,['read',''],['done'])
    app._universe=lambda:None
    app.start_step(); key=r.last_step()['session_key']; r.plan_journey(key,1)
    shown=[]
    monkeypatch.setattr(desktop,'run_popup',lambda payload,**kwargs: (shown.append(payload), json.dumps({'action':'done'}))[1])
    app._step_end(app.focus.snapshot()['id'])
    assert shown[-1]['mode']=='cutscene' and len([p for p in shown if p['mode']=='cutscene'])==1
    assert r.closure(key) and r.journey(key)['finale']=='shown'
    app._sand_close(key,r.session_steps(key),[],ask_edge=False)
    assert len([p for p in shown if p['mode']=='cutscene'])==1


def test_pending_finale_survives_a_playback_failure(desktop,monkeypatch,tmp_path):
    app,r,*_=_app(desktop,monkeypatch,tmp_path,['read'],[])
    app._universe=lambda:None
    app.start_step(); key=r.last_step()['session_key']
    r.finish_step(r.last_step()['id'],'done')
    monkeypatch.setattr(desktop,'run_popup',lambda *a,**k: (_ for _ in ()).throw(OSError('no desktop')))
    with pytest.raises(OSError): app._sand_close(key,r.session_steps(key),[],ask_edge=False)
    assert r.journey(key)['finale']=='pending'
    assert r.open_journey()['session_key']==key


def test_legacy_study_timer_adopted_from_real_step(tmp_path):
    r=RecordStore(tmp_path/'r.sqlite3')
    r.add_step('old','read',session_key='legacy',minutes=15,started_epoch=0,deadline_epoch=5)
    p=tmp_path/'f.json'
    p.write_text(json.dumps(dict(id='old',goal='read',status='running',started_at=0,deadline=5)))
    restored=Focus(p,r,None,clock=lambda:20).snapshot()
    assert restored['study_step'] and restored['status']=='running'


def test_manual_early_close_plays_existing_finale(desktop,monkeypatch,tmp_path):
    app,r,asked,_,_=_app(desktop,monkeypatch,tmp_path,['read','','remaining'],['done','close','partly'])
    app._universe=lambda:None
    original=desktop.run_popup; finales=[]
    def popup(payload,**kwargs):
        if payload['mode']=='cutscene': finales.append(payload); return ''
        return original(payload,**kwargs)
    monkeypatch.setattr(desktop,'run_popup',popup)
    app.start_step(); key=r.last_step()['session_key']; r.plan_journey(key,5)
    app._step_end(app.focus.snapshot()['id'],early=True)
    assert r.journey(key)['completed']==1 and r.closure(key)['remaining']=='remaining'
    assert len(finales)==1 and r.journey(key)['finale']=='shown'


def test_optional_goal_and_length_use_existing_popup_response(desktop,monkeypatch,tmp_path):
    monkeypatch.setattr(desktop,'run_popup',lambda *a,**k: json.dumps({'text':'25 read','target_steps':3,'goal':'own aim'}))
    journey={'session_key':'s','target_steps':None}
    assert desktop.popup_input('step','step',journey=journey)=='25 read'
    assert journey['target_steps']==3 and journey['goal']=='own aim'
