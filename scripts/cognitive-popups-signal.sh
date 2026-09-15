#!/usr/bin/env bash
set -euo pipefail

STATE_DIR="${COGNITIVE_STATE_DIR:-$HOME/.local/state/cognitive-popups}"
PID_FILE="$STATE_DIR/desktop.pid"
ACTION="${1:-}"
REQUEST=""

case "$ACTION" in
  seed)    SIGNAL=USR1 ;;
  menu)    SIGNAL=WINCH ;;
  feynman)   SIGNAL=USR2 ;;
  prediction) SIGNAL=HUP ;;
  # GLib accepts only six signals and the four above already claim the useful
  # ones, so `note` queues its action and wakes the daemon with SIGWINCH.
  note)    SIGNAL=WINCH; REQUEST=note ;;

  stop)    SIGNAL=TERM ;;
  *) echo "usage: $0 {seed|menu|feynman|prediction|note|stop}" >&2; exit 2 ;;
esac

if [[ ! -r "$PID_FILE" ]]; then
  echo "cognitive-popups is not running" >&2
  exit 1
fi
PID="$(cat "$PID_FILE")"
if [[ ! "$PID" =~ ^[0-9]+$ ]] || ! kill -0 "$PID" 2>/dev/null; then
  echo "stale PID file: $PID_FILE" >&2
  exit 1
fi
if [[ -n "$REQUEST" ]]; then
  printf '%s\n' "$REQUEST" > "$STATE_DIR/request"
fi

kill "-$SIGNAL" "$PID"
