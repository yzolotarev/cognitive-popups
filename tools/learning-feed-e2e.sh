#!/usr/bin/env bash
# Isolated GTK/SQLite regression: no real key, no user DB, no desktop interaction.
set -euo pipefail
out=$(realpath -m "${1:-/tmp/cognitive-learning-e2e}")
mkdir -p "$out"
root=$(cd "$(dirname "$0")/.." && pwd)
bin="$root/apps/desktop/target/debug/cognitive-popups"
export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR=$(mktemp -d)
export WAYLAND_DISPLAY=wayland-1 GTK_A11Y=none
chmod 700 "$XDG_RUNTIME_DIR"
export COGNITIVE_DB="$out/journal.db" COGNITIVE_PROMPTS="$root/docs/v2/port/prompts" COGNITIVE_AI_FIXTURE="$root/apps/desktop/tools/fixture.json"
# Fresh path required: never overwrite a previous test or a user's database.
if [[ -e "$COGNITIVE_DB" ]]; then echo "Choose a fresh output directory" >&2; exit 1; fi
printf 'output * resolution 1280x900\n' > "$XDG_RUNTIME_DIR/sway.conf"
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman sway -c "$XDG_RUNTIME_DIR/sway.conf" >"$out/sway.log" 2>&1 &
sway_pid=$!
daemon=0
cleanup() { if (( daemon )); then kill "$daemon" 2>/dev/null || true; fi; kill "$sway_pid" 2>/dev/null || true; }
trap cleanup EXIT
for _ in $(seq 60); do [[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]] && break; sleep .1; done
printf 'Метод Ньютона сходится квадратично, если начальное приближение достаточно близко к корню.' | wl-copy --primary
"$root/target/debug/cognitive-exoskeleton" nudge consent --db "$COGNITIVE_DB" experiments on > "$out/consent.json"
dbus-run-session "$bin" daemon > "$out/daemon.log" 2>&1 &
daemon=$!
for _ in $(seq 400); do grep -q '^ready$' "$out/daemon.log" && break; sleep .1; done
grep -q '^ready$' "$out/daemon.log"
step() { "$bin" signal practice "feed $1"; sleep 2.3; }
step support
grim "$out/map.png"
"$bin" signal hide
sleep .2
"$bin" signal practice probes
sleep 1.2
python3 - "$COGNITIVE_DB" <<'PY'
import json,sqlite3,sys
c=sqlite3.connect(sys.argv[1]); s=json.loads(c.execute('select json from learning_states order by updated_at desc limit 1').fetchone()[0])
assert s['episode']['helped'] and s['episode']['position']==0
assert s['episode']['preparation'][0]['kind']=='action'
assert 'reference' not in s['episode']['preparation'][0]
PY
step next
step answer
step matched
step answer
step matched
grim "$out/return.png"
step answer
step matched
step end
grim "$out/stop.png"
python3 - "$COGNITIVE_DB" <<'PY'
import json,sqlite3,sys
c=sqlite3.connect(sys.argv[1]); s=json.loads(c.execute('select json from learning_states order by updated_at desc limit 1').fetchone()[0])
assert s['completed'],s
observations=s['completed'][0]['episode']['observations']
assert observations[0]['provenance']=='Navigation'
assert all(o['provenance']=='AfterHelp' for o in observations[1:]),observations
assert s['policy']['matches']==0
assert c.execute('select count(*) from memory_reviews').fetchone()[0]==0
assert c.execute('select count(*) from learning_feedback').fetchone()[0]==1
assert not [r for r in c.execute("select kind from learning_events") if r[0]=='mastered']
print('OK: map has no answer/grade; help survives restore; whole-chain return is AfterHelp; no FSRS review; optional feedback offered once.')
PY
if grep -En 'panicked|already (mutably )?borrowed|CRITICAL' "$out/daemon.log"; then exit 1; fi
printf 'Artifacts: %s\n' "$out"
