#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
# Environment values are quoted; WorkingDirectory accepts internal spaces but
# strips trailing whitespace. Reject unsafe sed/systemd syntax before writing.
if [[ "$ROOT" == *[!a-zA-Z0-9_./\ -]* || "$ROOT" == *" " ]]; then
  echo "Unsupported checkout path: use ASCII letters, digits, spaces, _, ., / or -; no trailing space." >&2
  exit 1
fi
UNITS=("cognitive-popups.service" "cognitive-hud.service" "cognitive-universe.timer")
# Rendered but not enabled on its own: the timer starts it.
HELPERS=("cognitive-universe.service")

mkdir -p "$UNIT_DIR"
for unit in "${UNITS[@]}" "${HELPERS[@]}"; do
  sed "s|@COGNITIVE_PROJECT@|$ROOT|g" "$ROOT/systemd/$unit" > "$UNIT_DIR/$unit"
done
systemctl --user daemon-reload
systemctl --user enable --now "${UNITS[@]}"

echo "Installed and started: ${UNITS[*]}"
echo "Merge config/hypr-v2.lua into your Hyprland user config to enable hotkeys and the panel."
echo "Verify with: systemctl --user status cognitive-popups.service"
