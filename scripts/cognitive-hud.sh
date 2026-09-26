#!/usr/bin/env bash
# Боковая панель: показать/скрыть, запустить, если её ещё нет.
#
#   cognitive-hud.sh          показать/скрыть; если панель не запущена — запустить
#   cognitive-hud.sh --stop   остановить
#
# Панель ничего не решает сама: кнопки спрашивают работающий демон через
# сокет в каталоге состояния. Если демон не запущен, панель это покажет.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="${COGNITIVE_PROJECT:-$(cd -- "$SCRIPT_DIR/.." && pwd)}"
STATE_DIR="${COGNITIVE_STATE_DIR:-$PROJECT/var}"
PID_FILE="$STATE_DIR/hud.pid"

COMMAND="${1:-toggle}"

running() {
  [[ -r "$PID_FILE" ]] || return 1
  local pid
  pid="$(cat "$PID_FILE" 2>/dev/null || true)"
  [[ "$pid" =~ ^[0-9]+$ ]] || return 1
  kill -0 "$pid" 2>/dev/null
}

case "$COMMAND" in
  toggle|--toggle|start|--start|show|hide) ;;
  stop|--stop|quit|--quit) ;;
  *) echo "usage: $0 [toggle|--stop]" >&2; exit 2 ;;
esac

if running; then
  PID="$(cat "$PID_FILE")"
  case "$COMMAND" in
    stop|--stop|quit|--quit) kill -TERM "$PID" ;;
    *) kill -USR1 "$PID" ;;
  esac
  exit 0
fi

# Нечего останавливать — это не ошибка.
case "$COMMAND" in
  stop|--stop|quit|--quit) exit 0 ;;
esac

mkdir -p "$STATE_DIR"
export PYTHONPATH="$PROJECT/src${PYTHONPATH:+:$PYTHONPATH}"
export COGNITIVE_PROJECT="$PROJECT"
export COGNITIVE_STATE_DIR="$STATE_DIR"
# Панель ставит себя сама, а Wayland этого не разрешает; попапы и демон уже
# работают так же.
export GDK_BACKEND=x11

# setsid: панель переживает закрытие терминала, из которого её позвали.
setsid python3 -m cognitive_popups.hud >"$STATE_DIR/hud.log" 2>&1 &
