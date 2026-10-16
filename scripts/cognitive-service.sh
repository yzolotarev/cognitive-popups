#!/usr/bin/env bash
# Start one Python entry point for a systemd user unit (or by hand):
#   cognitive-service.sh -m cognitive_popups
#
# The graphical session should export DISPLAY / WAYLAND_DISPLAY to the user
# manager (`systemctl --user import-environment DISPLAY WAYLAND_DISPLAY` in the
# compositor's autostart). The fallbacks below only cover a session that did
# not: the first Wayland socket in XDG_RUNTIME_DIR, and :0 for XWayland.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="${COGNITIVE_PROJECT:-$(cd -- "$SCRIPT_DIR/.." && pwd)}"
export COGNITIVE_PROJECT="$PROJECT"
export PYTHONPATH="$PROJECT/src${PYTHONPATH:+:$PYTHONPATH}"
export COGNITIVE_STATE_DIR="${COGNITIVE_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/cognitive-popups}"

if [[ -z "${WAYLAND_DISPLAY:-}" && -n "${XDG_RUNTIME_DIR:-}" ]]; then
  for socket in "$XDG_RUNTIME_DIR"/wayland-[0-9]*; do
    if [[ -S "$socket" ]]; then
      export WAYLAND_DISPLAY="${socket##*/}"
      break
    fi
  done
fi
export DISPLAY="${DISPLAY:-:0}"

exec python3 "$@"
