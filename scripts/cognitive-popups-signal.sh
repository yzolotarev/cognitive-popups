#!/usr/bin/env bash
set -euo pipefail

STATE_DIR="${COGNITIVE_STATE_DIR:-$HOME/.local/state/cognitive-popups}"
PID_FILE="$STATE_DIR/desktop.pid"
ACTION="${1:-}"

case "$ACTION" in
  seed)    SIGNAL=USR1 ;;
  menu)    SIGNAL=WINCH ;;
  feynman) SIGNAL=USR2 ;;

  stop)    SIGNAL=TERM ;;
  *) echo "usage: $0 {seed|menu|feynman|stop}" >&2; exit 2 ;;
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
kill "-$SIGNAL" "$PID"
