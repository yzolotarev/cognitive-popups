#!/usr/bin/env bash
# Measures one GUI probe on a live Hyprland session.
# Usage: probe.sh <label> <timeout-seconds|wait> <command...>
set -uo pipefail

label="$1"; shift
life="$1"; shift

active_class() {
  hyprctl activewindow -j 2>/dev/null |
    python3 -c 'import json,sys
try: d=json.load(sys.stdin)
except Exception: print("none|"); raise SystemExit
print((d.get("class") or "none") + "|" + (d.get("title") or ""))' 2>/dev/null || echo "none|"
}

window_geometry() {
  hyprctl clients -j 2>/dev/null |
    python3 -c 'import json,sys
label = sys.argv[1].lower()
for c in json.load(sys.stdin):
    name = ((c.get("class") or "") + " " + (c.get("title") or "")).lower()
    if label in name:
        x, y = c["at"]; w, h = c["size"]
        print(f"{x},{y} {w}x{h}")
        break' "$1" 2>/dev/null
}

before="$(active_class)"
start_ns="$(date +%s%N)"

"$@" &
pid=$!

sample_rss() {
  local goal_ns="$1"
  while [ "$(date +%s%N)" -lt "$goal_ns" ]; do sleep 0.05; done
  awk '/VmRSS/ {print $2}' "/proc/$pid/status" 2>/dev/null || echo missing
}

rss_1s="$(sample_rss $((start_ns + 1000000000)))"
active_1s="$(active_class)"
geom="$(window_geometry "$label")"

if [ -n "${geom:-}" ]; then
  grim -g "$geom" "/tmp/opencode/${label}-window.png" 2>/dev/null || true
fi

if [ "$life" = "wait" ]; then
  wait "$pid"
else
  sleep "$life"
  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
fi
end_ns="$(date +%s%N)"

echo "label=$label"
echo "active_before=$before"
echo "active_after_1s=$active_1s"
echo "rss_1s_kb=$rss_1s"
echo "wall_ms=$(( (end_ns - start_ns) / 1000000 ))"
echo "geometry=${geom:-none}"
