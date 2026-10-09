#!/usr/bin/env bash
# Вселенная читателя: его мысли дословно на графе понятий (скрипты + web2api).
#   cognitive-universe.sh sync            собрать новые мысли и назвать их понятия
#   cognitive-universe.sh query "текст"   какие твои мысли будит этот текст
#   cognitive-universe.sh stats | concepts | hide ID | show ID
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="${COGNITIVE_PROJECT:-$(cd -- "$SCRIPT_DIR/.." && pwd)}"

exec env \
  PYTHONPATH="$PROJECT/src${PYTHONPATH:+:$PYTHONPATH}" \
  COGNITIVE_STATE_DIR="${COGNITIVE_STATE_DIR:-$PROJECT/var}" \
  python3 -m cognitive_popups.universe "$@"
