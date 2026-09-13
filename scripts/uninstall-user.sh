#!/usr/bin/env bash
set -euo pipefail
UNIT_NAME="cognitive-popups.service"
systemctl --user disable --now "$UNIT_NAME" 2>/dev/null || true
rm -f "$HOME/.config/systemd/user/$UNIT_NAME"
systemctl --user daemon-reload
echo "Removed $UNIT_NAME."
