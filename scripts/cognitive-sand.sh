#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
export PYTHONPATH="$SCRIPT_DIR/../src${PYTHONPATH:+:$PYTHONPATH}"
export COGNITIVE_STATE_DIR="${COGNITIVE_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/cognitive-popups}"
exec python3 -m cognitive_popups.sand_cli "$@"
