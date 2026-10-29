#!/usr/bin/env bash
# Every window in light and dark, on a headless sway: what CI uploads for review.
# The model is replaced by tools/fixture.json. Needs sway, grim, wl-clipboard,
# dbus. Usage: tools/screenshots.sh <out-dir>
set -euo pipefail
out=$(realpath -m "${1:-screenshots}")
here=$(cd "$(dirname "$0")/.." && pwd)
bin="$here/target/debug/cognitive-popups"
mkdir -p "$out"
export XDG_RUNTIME_DIR=$(mktemp -d) WAYLAND_DISPLAY=wayland-1 GTK_A11Y=none
chmod 700 "$XDG_RUNTIME_DIR"
printf 'output * bg #8aa4c8 solid_color\n' >"$XDG_RUNTIME_DIR/sway.conf"
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman \
  sway -c "$XDG_RUNTIME_DIR/sway.conf" >"$out/sway.log" 2>&1 &
sway_pid=$!
trap 'kill $sway_pid 2>/dev/null || true' EXIT
for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/wayland-1" ] && break; sleep 0.1; done

printf 'Метод Ньютона сходится квадратично, если начальное приближение достаточно близко к корню.' |
  wl-copy --primary

# name|window|argument
shots=(
  "note|note"
  "understand-words|understand|words"
  "understand-own|understand|own"
  "understand-compare|understand|own ранг, ступени, корень, метод"
  "understand-explain|understand|explain"
  "understand-example|understand|example"
  "understand-ladder|understand|ladder"
  "understand-character|understand|character"
  "hypothesis|hypothesis|"
  "practice|practice|probes"
  "practice-methods|practice|methods"
  "practice-feynman|practice|feynman"
  "hypothesis-answer|hypothesis|чем ближе старт к корню, тем быстрее сходится"
)

for theme in light dark; do
  COGNITIVE_THEME=$theme COGNITIVE_DB="$out/journal-$theme.db" COGNITIVE_CURSOR=300,200 \
    COGNITIVE_AI_FIXTURE="$here/tools/fixture.json" \
    dbus-run-session "$bin" daemon >"$out/daemon-$theme.log" 2>&1 &
  daemon=$!
  for _ in $(seq 50); do grep -q ready "$out/daemon-$theme.log" && break; sleep 0.1; done
  sleep 2 # the first window of a fresh process waits for fonts
  for shot in "${shots[@]}"; do
    IFS='|' read -r name window argument <<<"$shot"
    # shellcheck disable=SC2086
    "$bin" signal $window ${argument:+"$argument"}
    sleep 1.2
    grim -g "250,0 680x800" "$out/$name-$theme.png"
    "$bin" signal hide
    sleep 0.3
  done
  pkill -P $daemon || true
  kill $daemon 2>/dev/null || true
  wait $daemon 2>/dev/null || true
done
echo "screenshots in $out"
