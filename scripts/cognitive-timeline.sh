#!/usr/bin/env bash
# Лента учёбы за день из всех хранилищ программы. Только чтение.
#   cognitive-timeline.sh               сегодня
#   cognitive-timeline.sh 2026-09-20    один день
#   cognitive-timeline.sh --days        сводка по дням
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="${COGNITIVE_PROJECT:-$(cd -- "$SCRIPT_DIR/.." && pwd)}"

exec env \
  PYTHONPATH="$PROJECT/src${PYTHONPATH:+:$PYTHONPATH}" \
  COGNITIVE_STATE_DIR="${COGNITIVE_STATE_DIR:-$PROJECT/var}" \
  python3 -m cognitive_popups.timeline "$@"
