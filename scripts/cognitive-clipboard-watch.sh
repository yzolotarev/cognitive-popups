#!/usr/bin/env bash
# No clipboard content is logged or saved: only ownership-change timestamps.
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TASK_STATE="${COGNITIVE_STATE_DIR:-$SCRIPT_DIR/../var}"
mkdir -p "$TASK_STATE"
exec 9>"$TASK_STATE/clipboard-watch.lock"
flock -n 9 || exit 0
wl-paste --primary --watch touch "$TASK_STATE/primary.stamp" &
PRIMARY_PID=$!
trap 'kill "$PRIMARY_PID" 2>/dev/null || true' EXIT
wl-paste --watch touch "$TASK_STATE/clipboard.stamp" &
CLIPBOARD_PID=$!
trap 'kill "$PRIMARY_PID" "$CLIPBOARD_PID" 2>/dev/null || true' EXIT
wait -n "$PRIMARY_PID" "$CLIPBOARD_PID"
