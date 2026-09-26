#!/usr/bin/env bash
set -euo pipefail
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNITS=("cognitive-popups.service" "cognitive-hud.service")
for unit in "${UNITS[@]}"; do
  systemctl --user disable --now "$unit" 2>/dev/null || true
  rm -f "$UNIT_DIR/$unit"
done
systemctl --user daemon-reload
echo "Removed: ${UNITS[*]}"
