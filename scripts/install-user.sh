#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT_DIR="$HOME/.config/systemd/user"
UNIT_NAME="cognitive-popups.service"

mkdir -p "$UNIT_DIR"
cp "$ROOT/systemd/$UNIT_NAME" "$UNIT_DIR/$UNIT_NAME"
systemctl --user daemon-reload
systemctl --user enable --now "$UNIT_NAME"

echo "Installed and started $UNIT_NAME."
echo "Merge config/hypr-v2.lua into your Hyprland user config to enable hotkeys."
echo "Verify with: systemctl --user status $UNIT_NAME"
