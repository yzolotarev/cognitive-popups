#!/usr/bin/env bash
# Сгенерировать тренировочную задачу по материалу текущей сессии чтения.
#
# Отдельный процесс, а не сигнал сервису: меню и попап рисует общий хелпер,
# поэтому работающий демон трогать не нужно. По умолчанию открывается меню;
# явные режимы (например, --practice) проходят напрямую.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="${COGNITIVE_PROJECT:-$(cd -- "$SCRIPT_DIR/.." && pwd)}"

ARGS=("$@")
if [[ ${#ARGS[@]} -eq 0 ]]; then
  ARGS=(--menu)
fi

exec env \
  PYTHONPATH="$PROJECT/src${PYTHONPATH:+:$PYTHONPATH}" \
  COGNITIVE_PROJECT="$PROJECT" \
  COGNITIVE_STATE_DIR="${COGNITIVE_STATE_DIR:-$PROJECT/var}" \
  GDK_BACKEND=x11 \
  python3 -m cognitive_popups.tasks "${ARGS[@]}"
