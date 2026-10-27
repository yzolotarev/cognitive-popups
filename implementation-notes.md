# Implementation notes

Рабочий журнал отличий от плана. Продуктовые решения сюда не переносятся — они
живут в `docs/decisions/`.

## P0 — реестр требований

- Спецификация v0.2 скопирована в `docs/spec/` без изменений; реестр генерируется
  из её таблиц, поэтому ID и статусы совпадают механически.
- Добавлен `tests/traceability.rs`: проверяет полноту, уникальность, совпадение
  статусов и допустимые состояния строк.
- Защита `main` не позволяет прямой push; работа идёт через PR. Это ожидаемое
  поведение, не сбой.

## P2 — домен и журнал (в работе)

**Найдено.** Таблица происхождения и конструкторы должны запрещать неверное
авторство на уровне типа, иначе разделение «своё / модель / источник» держится
только на дисциплине вызывающего кода.

**Сделано.** `crates/domain` — автор, операция, снимок источника, полезная
нагрузка с проверкой автора; `crates/journal` — SQLite-журнал schema 1,
транзакционная запись, уникальный идемпотентный ключ, чтение в порядке sequence,
WAL + `synchronous=FULL`, отказ открывать БД новее сборки.

**Отклонение.** Внутренний тег serde `kind` конфликтовал с полем `SourceSnapshot.kind`;
тег переименован в `payload`. Формат БД не затронут.

**Отклонение.** Workspace-крейты не собирались корневой командой `cargo test`;
CI переведён на `--workspace`. Без этого зелёный CI не проверял домен.

**Не сделано и почему.** Провайдеры LLM, ключи, архив v1, TTL и экспорт ждут
решений OD-11 и фактической проверки хранилища; сейчас нет кода, который бы
молча выбирал политику за автора.

## Проверки

- `cargo fmt --all -- --check`
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- `cargo test --locked --workspace --all-targets --all-features`

Desktop-проверки не выполнялись: P1 ещё не начат, GUI-стек не выбран.

## P2 — безопасное повторное применение событий (PR)

- Совпадающий idempotency key с полностью тем же событием остаётся harmless retry.
- Повторное использование ключа с изменённым текстом, автором, временем, операцией либо типом события возвращает `IdempotencyConflict`, не стирая исходное событие.
- Конфликт внутри batch откатывает все записи пакета.
- Добавлены тесты для идемпотентного повтора, конфликта после commit, конфликта внутри batch и изменения времени.

## P1 — решения автора

- OD-09: выбран Slint 1.18; для пассивных подсказок используются системные уведомления.
- OD-10: Linux-first, Windows desktop acceptance remains blocked (CI builds do not count).
- Статусы требований в исходной спецификации/registry не повышены самовольно; решения вынесены в `docs/decisions/open-decisions.md`.
# Integration checkpoint: local P2/P3 + PR #6 (2026-10-27)

- Preserved the local capture/UI/app slice in `wip/p2-p3-local-snapshot`
  (`2ef8e6f`) before integrating PR #6 on a separate branch.
- Merged PR #6 without conflicts; corrected its Rust formatting.
- Restored the root traceability integration test via the app package: a
  virtual workspace does not discover root-level `tests/` automatically.
- Local validation: 23 tests passed, including 3 traceability and 9 journal
  tests; workspace Clippy with warnings denied, formatting, release build,
  and CLI `--help` smoke check passed.
- CI installs Linux desktop build dependencies and allows 45 minutes for
  cold Slint builds. Windows runtime capture and desktop acceptance remain
  unverified; compilation is not desktop acceptance. The UI smoke command is
  diagnostic only; its auto-close/focus behavior remains unresolved.
- This is an integration checkpoint, not completion of v1 or P3 acceptance.
