#!/usr/bin/env bash
# Remove the systemd user units. Never touches the data.
set -euo pipefail
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
STATE_DIR="${COGNITIVE_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/cognitive-popups}"
UNITS=("cognitive-popups.service" "cognitive-hud.service" "cognitive-universe.timer" "cognitive-universe.service"
       "cognitive-maintain.timer" "cognitive-maintain.service")
for unit in "${UNITS[@]}"; do
  systemctl --user disable --now "$unit" 2>/dev/null || true
  rm -f "$UNIT_DIR/$unit"
done
systemctl --user daemon-reload
echo "Removed: ${UNITS[*]}"
if [[ -e "$STATE_DIR" ]]; then
  target="$(readlink -f -- "$STATE_DIR" 2>/dev/null || echo "$STATE_DIR")"
  echo "Your notes, hypotheses and ledger were NOT deleted. They are in:"
  echo "  $STATE_DIR${target:+ (-> $target)}"
  echo "Delete them yourself only if you really mean to."
fi
