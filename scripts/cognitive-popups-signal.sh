#!/usr/bin/env bash
set -euo pipefail

# The state directory follows the checkout, so renaming or moving the project keeps
# the hotkeys working; COGNITIVE_STATE_DIR still overrides it.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="${COGNITIVE_STATE_DIR:-$SCRIPT_DIR/../var}"
PID_FILE="$STATE_DIR/desktop.pid"
ACTION="${1:-}"
REQUEST=""

case "$ACTION" in
  seed)    SIGNAL=USR1 ;;
  menu)    SIGNAL=WINCH ;;
  feynman)   SIGNAL=USR2 ;;
  prediction) SIGNAL=HUP ;;
  # GLib accepts only six signals and the four above already claim the useful
  # ones, so `note`, `clarify` and `summary` queue their action and wake the
  # daemon with SIGWINCH.
  note)    SIGNAL=WINCH; REQUEST=note ;;
  clarify) SIGNAL=WINCH; REQUEST=clarify ;;
  summary) SIGNAL=WINCH; REQUEST=summary ;;
  reframe) SIGNAL=WINCH; REQUEST=reframe ;;
  intent)  SIGNAL=WINCH; REQUEST=intent ;;
  example) SIGNAL=WINCH; REQUEST=example ;;
  # Прямой вызов остаётся прямым; отдельная команда открывает окно для
  # своего запроса или для вставки материала, когда выделения нет.
  example-ask) SIGNAL=WINCH; REQUEST=example-ask ;;

  stop)    SIGNAL=TERM ;;
  *) echo "usage: $0 {seed|menu|feynman|prediction|note|clarify|summary|reframe|intent|example|example-ask|stop}" >&2; exit 2 ;;
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
