# B78 шаги 2–3: локальная приёмка 30.10.2026

Рабочее дерево `/home/y/projects/cognitive-popups-b78`, ветка `feat/b78-local`. Основной каталог не редактировался.

## Проверки

- `cargo fmt --all -- --check` — exit 0.
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` — exit 0.
- `cargo test --locked --workspace` — exit 0, 311 тестов; лог `/tmp/b78-root-tests.log`.
- `apps/desktop`: `cargo fmt -- --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked` — exit 0, 12 тестов; лог `/tmp/b78-desktop-tests.log`.

## CLI

`python3 tools/b78-e2e.py /home/y/.local/bin/cognitive-popups /tmp/b78-test.db` — exit 0 (окончательная установленная сборка). CLI запущен с `COGNITIVE_DB=/tmp/b78-test.db`; ответы модели даёт локальная HTTP-фикстура, запросы и проверки идут через настоящий AI-клиент. Жребий — производственный, охват всех веток проверяется кодом прогона. Полный экспорт `/tmp/b78-test.export.json`.

```text
HTTP model fixture; isolated CLI acceptance
DB=/tmp/b78-test.db; lessons=8; assignments=16; cards=16; due_delta=86400s for EVERY card
arms=aa_first, aa_second, silence, suggest_alt_t, suggest_alt_w
own attempt → reference → self_graded; early answer rejected; duplicate grade idempotent
Alt+T recall: empty at +24h-1s; every stored card present at +24h
first_checks=5; actual_interval=86721s; grading_interval=86999s; subsequent_reviews=1
{"aa_first": {"assigned": 6, "checked": 1, "completed": 0, "matched": 0, "missed": 1, "missing": 5, "partial": 0, "shown": 0}, "aa_second": {"assigned": 6, "checked": 1, "completed": 0, "matched": 0, "missed": 0, "missing": 5, "partial": 1, "shown": 0}, "silence": {"assigned": 2, "checked": 1, "completed": 0, "matched": 1, "missed": 0, "missing": 1, "partial": 0, "shown": 0}, "suggest_alt_t": {"assigned": 1, "checked": 1, "completed": 1, "matched": 1, "missed": 0, "missing": 0, "partial": 0, "shown": 1}, "suggest_alt_w": {"assigned": 1, "checked": 1, "completed": 1, "matched": 1, "missed": 0, "missing": 0, "partial": 0, "shown": 1}}
full_export=/tmp/b78-test.export.json
```

В экспорт входят исходные назначения с вероятностями, показ/пропуск/выполнение, card_status, оценки/источники, фактические интервалы, последующие повторы и ITT missing. Экспорт через установленный `cognitive-popups experiment export --json` совпадает с CLI ядра: `/tmp/b78-installed-export.json`.

## Установка

`make install` — exit 0 (`/tmp/b78-install.log`). Установлены `~/.local/bin/cognitive-popups`, `cognitive-popups-core` и промпт `NEW_block_card.md`. Демон перезапущен, в `~/.cache/cognitive-popups.log` — `ready`. `cognitive-popups doctor` — exit 0, все проверки «есть», лог `/tmp/b78-doctor.log`.

Скриншоты проверяются из артефакта `desktop-screenshots` CI «Desktop (Linux)»: `lesson-clock-hint-light.png` и `lesson-clock-hint-dark.png`. Локально sway не устанавливался.
