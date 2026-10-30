#!/usr/bin/env python3
"""CLI acceptance over an isolated DB; a local HTTP model fixture, no network/key.
Usage: python3 tools/b78-e2e.py [core binary] [database]
Only creates a new database; refuses to overwrite an existing one.
"""
import http.server
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import threading

root = Path(__file__).resolve().parent.parent
binary = Path(sys.argv[1] if len(sys.argv) > 1 else root / 'target/debug/cognitive-exoskeleton').resolve()
db = Path(sys.argv[2] if len(sys.argv) > 2 else '/tmp/b78-test.db')
if db.exists():
    raise SystemExit(f'Refusing to overwrite {db}')
plan = {'goal': 'Вычислить ожидаемый убыток', 'why': 'Проверить решение', 'criteria': [{'text': 'Посчитаю сам'}], 'plan': [{'text': 'Понять механизм'}, {'text': 'Решить пример'}]}
card = {'question': 'Как вычислить ожидаемую потерю?', 'reference': 'Умножаем вероятность на убыток.'}

class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        system = body['messages'][0]['content']
        value = card if 'По полному тексту одного' in system else plan
        response = json.dumps({'choices': [{'message': {'content': json.dumps(value, ensure_ascii=False)}}]}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(response)
    def log_message(self, *args):
        pass

server = http.server.HTTPServer(('127.0.0.1', 0), Model)
threading.Thread(target=server.serve_forever, daemon=True).start()
key = db.with_suffix('.fixture-key')
key.write_text('local-fixture')
key.chmod(0o600)
env = dict(os.environ, COGNITIVE_DB=str(db), COGNITIVE_AI_PROVIDER='openai_compatible', COGNITIVE_AI_MODEL='local-e2e-fixture', COGNITIVE_AI_BASE_URL=f'http://127.0.0.1:{server.server_port}/v1', COGNITIVE_AI_KEY_FILE=str(key), COGNITIVE_AI_FALLBACK_MODELS='')

def run(*args, at=1000, parsed=False, fail=False):
    command = [str(binary), *args[:2], '--db', str(db), *args[2:], '--now', str(at)]
    p = subprocess.run(command, cwd=root, env=env, text=True, capture_output=True)
    if fail:
        assert p.returncode != 0, command
        return
    assert p.returncode == 0, f'{command}: {p.stderr}'
    return json.loads(p.stdout) if parsed else p.stdout.strip()

try:
    subject = run('subject', 'add', 'Вероятности', 'Рассчитывать потери')
    run('nudge', 'consent', 'experiments', 'on')
    text = db.with_suffix('.material.txt')
    text.write_text('⟦b1 · механизм⟧\nУмножаем вероятность на убыток.\n⟦b2 · пример⟧\nУмножаем вероятность на убыток.\n')
    seen = set()
    for i in range(40):
        lesson = run('lesson', 'plan', '--subject', subject, '--prompts', str(root/'docs/v2/port/prompts'), parsed=True)['lesson']['id']
        run('step', 'start', '20 читаю')
        registered = run('material', 'register', '--lesson', lesson, '--file', str(text), parsed=True)
        rows = [a for a in registered['assignments'] if a['lesson'] == lesson]
        assert len(rows) == 2 and all(a['card_status'] == 'created' for a in rows)
        assert not run('material', 'register', '--lesson', lesson, '--file', str(text), parsed=True)['new']
        hint = run('experiment', 'hint', '--lesson', lesson, at=1001, parsed=True)
        if hint:
            run('experiment', 'skip', '--card', hint['id'], at=1002)
        for a in rows:
            seen.add(a['arm'])
            if a['arm'].startswith('suggest_'):
                run('experiment', 'interact', '--lesson', lesson, '--block', a['block'], '--function', a['arm'].removeprefix('suggest_'), '--operation', 'interaction-'+a['id'], at=1003)
        run('step', 'finish', 'done', at=1004)
        run('lesson', 'close', at=1005)
        if {'suggest_alt_t', 'suggest_alt_w', 'silence'} <= seen:
            break
    else:
        raise AssertionError(f'Random draw did not cover the three arms: {seen}')
    exported = run('experiment', 'export', '--json', parsed=True)
    rows = [row['assignment'] for row in exported['assignments']]
    with sqlite3.connect(db) as connection:
        due = connection.execute('SELECT c.due-a.assigned_at FROM memory_cards c JOIN block_assignments a ON a.id=c.unit_id').fetchall()
        assert len(due)==len(rows) and all(d==(86400,) for d in due)
    before = run('practice', 'tree', '--prompts', str(root/'docs/v2/port/prompts'), '--from-pill', at=87399, parsed=True)
    assert before['categories'] == []
    after = run('practice', 'tree', '--prompts', str(root/'docs/v2/port/prompts'), '--from-pill', at=87400, parsed=True)
    recall = next(c for c in after['categories'] if c['category']=='recall')
    assert len(recall['cards']) == len(rows)
    selected = []
    for arm in sorted(seen):
        a = next(a for a in rows if a['arm'] == arm)
        selected.append(a)
        run('experiment', 'grade', '--card', a['id'], '--operation', a['id'], '--grade', 'good', at=87721, fail=True)
        run('experiment', 'answer', '--card', a['id'], '--operation', a['id'], '--attempt', 'Вероятность умножить на убыток', at=87399, fail=True)
        answer = run('experiment', 'answer', '--card', a['id'], '--operation', a['id'], '--attempt', 'Вероятность умножить на убыток', at=87721, parsed=True)
        assert answer['reference'] == card['reference']
        grade = {'suggest_alt_t':'again','suggest_alt_w':'hard'}.get(arm,'good')
        for _ in range(2):  # duplicate delivery creates one review
            run('experiment', 'grade', '--card', a['id'], '--operation', a['id'], '--grade', grade, at=87999)
    a=selected[0]
    run('experiment','answer','--card',a['id'],'--operation','later','--attempt','Умножить',at=173800)
    run('experiment','grade','--card',a['id'],'--operation','later','--grade','good',at=173800)
    exported = run('experiment','export','--json',parsed=True)
    by_id={row['assignment']['id']:row for row in exported['assignments']}
    for a in selected:
        first=by_id[a['id']]['first_check']
        assert first['interval_seconds']==86721 and first['graded_interval_seconds']==86999 and first['source']=='self_graded'
    assert len(by_id[selected[0]['id']]['subsequent_reviews'])==1
    export_file=db.with_suffix('.export.json')
    export_file.write_text(json.dumps(exported,ensure_ascii=False,indent=2))
    print('HTTP model fixture; isolated CLI acceptance')
    print(f'DB={db}; lessons={i+1}; assignments={len(rows)}; cards={len(rows)}; due_delta=86400s for EVERY card')
    print('arms=' + ', '.join(sorted(seen)))
    print('own attempt → reference → self_graded; early answer rejected; duplicate grade idempotent')
    print('Alt+T recall: empty at +24h-1s; every stored card present at +24h')
    print('first_checks=5; actual_interval=86721s; grading_interval=86999s; subsequent_reviews=1')
    print(json.dumps(exported['summary'], ensure_ascii=False))
    print(f'full_export={export_file}')
finally:
    server.shutdown()
    key.unlink(missing_ok=True)
