#!/usr/bin/env bash
# Install the systemd user units for this checkout.
#
# Enabled: the desktop service and the weekly maintenance timer. The side
# panel (HUD) and the universe timer are installed but left off; the lines
# printed at the end turn them on. Nothing here moves or deletes data.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/cognitive-popups"
API_URL="${COGNITIVE_API_URL:-http://127.0.0.1:8081/v1/chat/completions}"
# Environment values are quoted; WorkingDirectory accepts internal spaces but
# strips trailing whitespace. Reject unsafe sed/systemd syntax before writing.
if [[ "$ROOT" == *[!a-zA-Z0-9_./\ -]* || "$ROOT" == *" " ]]; then
  echo "Unsupported checkout path: use ASCII letters, digits, spaces, _, ., / or -; no trailing space." >&2
  exit 1
fi
ENABLED=("cognitive-popups.service" "cognitive-maintain.timer")
# Rendered but not enabled: off by default, or started only by their timer.
OPTIONAL=("cognitive-hud.service" "cognitive-universe.timer")
HELPERS=("cognitive-universe.service" "cognitive-maintain.service")

# Older installs kept the state in <checkout>/var. If that has data and the XDG
# directory does not exist yet, point the XDG directory at it: one link, no copy,
# nothing moved or deleted.
if [[ -d "$ROOT/var" && ! -e "$STATE_DIR" && ! -L "$STATE_DIR" ]] \
    && compgen -G "$ROOT/var/*.sqlite3" >/dev/null; then
  mkdir -p "$(dirname -- "$STATE_DIR")"
  ln -s "$ROOT/var" "$STATE_DIR"
  echo "State: $STATE_DIR -> $ROOT/var (link to your existing data; nothing was moved)"
elif [[ -d "$ROOT/var" && -e "$STATE_DIR" && ! "$STATE_DIR" -ef "$ROOT/var" ]] \
    && compgen -G "$ROOT/var/*.sqlite3" >/dev/null; then
  echo "WARNING: two state directories: $STATE_DIR (used) and $ROOT/var (not used)." >&2
  echo "         Nothing was changed; merge or remove one of them by hand." >&2
else
  echo "State: $STATE_DIR"
fi

mkdir -p "$UNIT_DIR"
for unit in "${ENABLED[@]}" "${OPTIONAL[@]}" "${HELPERS[@]}"; do
  sed "s|@COGNITIVE_PROJECT@|$ROOT|g" "$ROOT/systemd/$unit" > "$UNIT_DIR/$unit"
done
systemctl --user daemon-reload
systemctl --user enable --now "${ENABLED[@]}"

echo "Installed and started: ${ENABLED[*]}"
echo "Installed, off by default:"
echo "  side panel:       systemctl --user enable --now cognitive-hud.service"
echo "  universe timer:   systemctl --user enable --now cognitive-universe.timer"

# The model bridge is optional for the service (Wants=), but without it there
# are no four words, no hypothesis checks and no tasks.
HEALTH="${API_URL%%/v1/*}/health"
if command -v curl >/dev/null 2>&1; then
  if curl -fsS -m 3 "$HEALTH" >/dev/null 2>&1; then
    echo "Model bridge: OK ($HEALTH)"
  else
    echo "WARNING: the model bridge does not answer at $HEALTH." >&2
    echo "         Notes and steps work; words and checks need the bridge (or set COGNITIVE_API_URL)." >&2
  fi
else
  echo "Model bridge: not checked (no curl); run 'scripts/cog doctor' later."
fi

echo "Hotkeys: merge config/hypr-v2.lua into your Hyprland config."
echo "Check everything with: $ROOT/scripts/cog doctor"
