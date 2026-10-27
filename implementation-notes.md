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
# Full-v1 implementation pass (2026-10-27)

- Continued on `implementation/full-v1`, based on the integrated snapshot and
  PR #6. No main-branch merge or modification of historical app data.
- Added `crates/services`: protected OS credentials, bounded/redacted HTTP
  provider, durable versioned subject/session/unit objects, optimistic concurrency,
  draft recovery, frozen exact-answer evidence and atomic FSRS 6.6.2 batches.
- Added the Slint desktop hub: settings, subjects/goals/preferences, context/own
  notes/AI separation, editable plans under a 15-minute guide timer, pause/resume,
  own results, memory/self-assessments, optional manual scheduling, snooze,
  composite groups/checks, history and opt-in experiments.
- Author explicitly approved MRT protocol v1 in this session (OD-06). Eligibility
  comes from stored independent code-check errors; both arms and assignment
  probabilities are logged before delivery. Frequency: 900 seconds, 3/UTC day;
  outcome window: 24h. Delivery attempts, failure, revocation and control are distinct.
- Capture is bounded (2-second subprocess deadlines, 1 MiB text limit), moved off
  the GUI thread, and persisted before display. Edited material is marked manual;
  clipboard age/window ownership are never claimed. Windows has a native clipboard
  fallback; primary-selection parity remains unverified.
- Note Save now waits for durable commit; typed drafts are local. Ctrl+Enter and
  Escape are wired, with input focus forwarded to the editor. OS instance locks
  prevent duplicate editors from repeated invocations. No legacy bindings changed.
- Separate new runtime directory `cognitive-exoskeleton-v1`; previous development
  and historical databases are untouched. Immutable SQLite archive inspection
  refuses live WAL snapshots and has a byte-preservation/no-sidecar test.
- Linux/Windows user-local installation scripts and package CI added; autostart
  is opt-in. This is not a claim of Windows desktop acceptance or signing.
- Local checks at this checkpoint: 46 tests, strict workspace Clippy, formatting,
  release build; cargo audit has no vulnerability failures but 8 unmaintained
  transitive-dependency warnings (Slint/zbus/keyring stack), not suppressed.
- Real Hyprland probe: diagnostic window auto-exits in 5.10 s; hub is mapped,
  submits buffers/commits and does not panic. Protected credential store synthetic
  write/read/delete passed; no existing secret was read or enumerated.

## Deviations / remaining gates

- Exact-answer code checks are intentionally limited: a frozen user-confirmed
  mapping does not establish arbitrary semantic competence. No LLM-only grade.
- Automatic admission/first-check policy and true FSRS merges remain unapproved;
  explicit manual dates/retention and non-destructive groups are available.
- No historical W/T executors, graph recommender or screen/audio recorder were
  silently promoted into mandatory scope. Preference selection/legacy KEEP
  policy still requires its own decision.
- Real external browser/PDF/editor capture, focus/hotkeys, installation on both
  desktops, live-provider plan quality and release metrics still require evidence.
  Tests of UI controllers use Slint's testing backend, not desktop acceptance.
- Prowl structural checks are unavailable in this clone (no `.prowl` workspace);
  this does not replace compiler, integration or desktop checks.

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
