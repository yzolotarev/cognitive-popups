#!/usr/bin/env bash
# P0 stack probe on Hyprland. Run from this directory:  ./run.sh
# It prints a report to paste back. Takes about two minutes.
set -u
cd "$(dirname "$0")"
cargo build --release -q || { echo "build failed"; exit 1; }
P=./target/release/gtk4-probe
LOG=$(mktemp)
pkill -x gtk4-probe 2>/dev/null; sleep 0.3
$P daemon >"$LOG" 2>&1 &
for _ in $(seq 1 50); do grep -q ready=1 "$LOG" && break; sleep 0.2; done

echo "== 2. Latency: 20 shows of the pre-created window"
for _ in $(seq 1 20); do $P show; sleep 0.4; $P hide; sleep 0.2; done
sleep 0.5
grep -o 'latency_ms=[0-9.]*' "$LOG" | cut -d= -f2 | sort -n | awk '
  {v[NR]=$1} END {printf "latency_ms median=%.1f p95=%.1f max=%.1f n=%d\n", v[int(NR/2)+1], v[int(NR*0.95)], v[NR], NR}'

step() { echo; echo "== $1"; read -r -p "   $2  → Enter, когда сделал(а). Результат одним словом (ok/нет): " ans; echo "manual: $1 = $ans" >>"$LOG"; }

$P show
step "1a. Без захвата клавиатуры" "Окно у курсора. НЕ кликая в него, напечатай что-нибудь в терминале. Печатается в терминал?"
step "1b. Клик включает ввод" "Кликни в поле окна и напечатай по-русски «Привет, ранг ⅓·6», Enter."
step "3. Вставка" "Скопируй любой русский текст, вставь в поле (Ctrl+V), Enter."
step "5a. Esc" "Нажми Esc в окне. Окно закрылось?"
$P show; sleep 0.3; $P show
step "5b. Второй вызов" "Окно одно (не два)? Закрой его Esc."
$P show --focus on_demand
step "6a. Горячая клавиша, on_demand" "Окно открылось. НЕ кликая, сразу печатай. Текст попадает в поле? Закрой Esc."
$P show --focus exclusive
step "6b. Горячая клавиша, exclusive" "То же: НЕ кликая, печатай. Попадает в поле? Esc закрывает?"

echo; echo "===== ОТЧЁТ (скопируй всё ниже) ====="
echo "hyprland: $(hyprctl version 2>/dev/null | head -1)"
echo "gtk4: $(pkg-config --modversion gtk4 2>/dev/null)  layer-shell: $(pkg-config --modversion gtk4-layer-shell-0 2>/dev/null)  typst: $(typst --version 2>/dev/null || echo none)"
grep -vE 'EGL|MESA|ZINK|^$' "$LOG" | grep -v '^latency_ms='
grep -o 'latency_ms=[0-9.]*' "$LOG" | cut -d= -f2 | sort -n | awk '
  {v[NR]=$1} END {printf "latency_ms median=%.1f p95=%.1f max=%.1f n=%d\n", v[int(NR/2)+1], v[int(NR*0.95)], v[NR], NR}'
pkill -x gtk4-probe
