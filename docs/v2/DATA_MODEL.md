# Модель данных Cognitive Popups v2

Новая SQLite-база (A17). Здесь сущности и связи, а не окончательный DDL: точные типы и индексы задаются при реализации. Каждая таблица отвечает за одно, ссылки — по id. Для каждой сущности указано, откуда она взята: таблица v1 (справочник — `docs/v2/sources/gemini-v1-inventory.md`, раздел 4.1, сверять с кодом) или решение 28.10.

## 1. Журнал (S1)

| Таблица | Поля | Откуда |
|---|---|---|
| `events` | `event_id`, `ts_utc`, `ts_local`, `tz`, `operation_id`, `kind`, `window`, `source` (`user`/`ai`/`other`), `origin` (`primary`/`clipboard`/`copy`/`dev`/`chat`), `payload_json`, `idempotency_key` UNIQUE, `lesson_id` NULL | v1 `events` + Q-30 + INV-04 |
| `operations` | `operation_id`, `started_at`, `kind`, `parent_id` | F-100 |
| `model_requests` | `request_id`, `operation_id`, `provider`, `model`, `prompt_key`, `prompt_hash`, `latency_ms`, `tokens_in`, `tokens_out`, `status`, `error` | v1 `obs_requests` |

## 2. Материал и собственные записи

| Таблица | Поля | Откуда |
|---|---|---|
| `fragments` | `fragment_id`, `text`, `source_hash`, `origin`, `window_title`, `doc_ref`, `created_at` | v1 `fragments` |
| `cues` | `fragment_id`, `n`, `simple`, `term`, `meaning`, `by` (`ai`/`user`) | F-03, F-04 |
| `notes` | `note_id`, `comment`, `anchor`, `fragment_id`, `status` (`open`/`closed`), `created_at`, `closed_at` | v1 `notes.sqlite3` |
| `hypotheses` | `hypothesis_id`, `text`, `fragment_id`, `verdict` (`confirmed`/`partially_confirmed`/`contradicted`/`not_supported`/`vague`/`pending`), `quote`, `correction`, `created_at` | v1 `predictions`; B21 |
| `feynman_checks` | `id`, `question`, `explanation`, `status`, `gaps_json` | v1 |
| `recent_passages` | файл или таблица: до 12 записей за 30 мин | F-95 |
| `materials` | `material_id` (16 hex FNV-1a 64 от чистого текста), `lesson_id`, `subject_id`, `text` (без невидимых меток), `registered_at`; полный текст, если зарегистрирован (CLI или наблюдатель буфера по согласию `study_clipboard`); `lesson_id` — первый урок, связи со всеми — `lesson_materials`; тот же текст — та же строка, новый — новая, старая остаётся | B78 |
| `lesson_materials` | `lesson_id`, `material_id`, `at`; ключ (`lesson_id`, `material_id`) — текст привязан к каждому уроку, где зарегистрирован; урок берёт последний привязанный | B78 |
| `material_blocks` | `material_id`, `block` (`b2`), `position`, `role` (6 ролей или `другое`), `text`; ключ (`material_id`, `block`) | B78 |
| `material_fragments` | `lesson_id`, `block`, `text` (чистый выделенный текст части выделения), `at`, `operation_id`, `key` UNIQUE (`<операция>:<блок>:<n>`) — только для частей с известным блоком; из них восстанавливается материал урока | B78 |
| `material_interactions` | `material_id` NULL (для старых записей неизвестен), `lesson_id`, `block` NULL (блок не найден — пробел виден), `window` (`alt_w`/`alt_t`/`ctrl_alt_p`/`alt_e`/…), `operation_id`, `sequence` NULL (номер события-источника в журнале), `at`, `key` UNIQUE (`<операция>:<блок или ->`) — только наблюдаемое; порядок — `at`, затем `sequence`. Статус блока выводится: `observed_interaction` или `unknown` (`not_read` зарезервирован); полнота текста блока — `known`/`partial`/`unknown` | B78 |

## 3. Практика (S5)

| Таблица | Поля | Откуда |
|---|---|---|
| `probe_sets` | `set_id`, `fragment_id`, `contract_version`, `created_at` | v1 |
| `probe_categories` | `category_id`, `set_id`, `title`, `invariant`, `rank`, `kind` (`text`/`recall`) | F-19, B31 |
| `probe_cards` | `card_id`, `category_id`, `action`, `example_before`, `example_after`, `prompt`, `answer`, `card_kind`, `source_span`, `substeps_json`, `rejected_reason` NULL | F-19, F-96 |
| `tasks` | `task_id`, `type` (1–10), `unit_id`, `condition`, `grounding_json`, `status`, `payload_json` | v1 `tasks`; B24 |
| `attempts` | `attempt_id`, `idempotency_key` UNIQUE, `card_id`/`task_id`/`unit_id`, `answer_text`, `attempt_before_reveal`, `help_used`, `verification_method` (`code_check`/`criterion_check`/`self_graded`/`model`/`none`), `evidence_kind` (8 видов, S4 правило 11), `created_at` | v1 `skill_attempts`, F-97, B33 |
| `probe_guidance` | `set_id`, `state_json` | v1 `ProbeGuidance` |

## 4. Предметы, цели, уроки

| Таблица | Поля | Откуда |
|---|---|---|
| `subjects` | `subject_id`, `name`, `goal_text` (долгосрочная), `why`, `final_task`, `conditions`, `success_criterion`, `deadline`, `status` (`active`/`paused`/`achieved`/`maintaining`/`removed`/`draft`), `fsrs_params_json` NULL, `params_optimized_at` | v1 `profiles`; B20, B32 |
| `competencies` | `competency_id`, `subject_id`, `text`, `kind` (8 видов), `confirmed`, `removed` | v1; B28 |
| `mechanism_switches` | `subject_id`, `mechanism`, `enabled` | Q-49 |
| `journeys` | `journey_id`, `target_steps` (NULL = ∞), `completed`, `finale_state` | v1 `step_journeys` |
| `lessons` | `lesson_id`, `subject_id`, `goal_15min`, `why`, `criteria_json`, `own_thoughts`, `plan_version`, `status` (`open`/`paused`/`closed`), `created_at`, `topic_id` NULL (тема программы, которая была следующей при составлении плана) | v1 `learning_lessons`; B20, B25, B71 |
| `subject_topics` | `topic_id`, `subject_id`, `position`, `title`, `added_at`, `done_at` NULL (отметка пользователя), `removed` (скрыта, не удалена) | B71 |
| `plan_items` | `item_id`, `lesson_id`, `plan_version`, `n`, `text`, `status` (`pending`/`current`/`worked`/`skipped`), `memory_unit_id` NULL, `why_included` | A6, B20 |
| `steps` | `step_id`, `lesson_id`, `journey_id`, `text`, `minutes`, `started_at`, `deadline`, `outcome` (`done`/`partly`/`not`/NULL), `takeaway` | v1 `steps` |
| `journey_feynman` | `journey_id`, `status` (`unverified`/`checked`), `question`, `answer`, `updated_at` | F-30 integrative question for a fixed path; `checked` means the formative response completed, never a learning grade |
| `closures` | `journey_id`, `remaining`, `closed_at`, `reason` | F-30 |

## 5. Учебные единицы и память (S3, S4)

| Таблица | Поля | Откуда |
|---|---|---|
| `units` | `unit_id`, `subject_id`, `title`, `action`, `kind` (8 видов), `mastered_at` NULL, `merged_into` NULL, `paused`, `created_at` | B27, B28, B32 |
| `unit_events` | `unit_id`, `event_id` | Q-30, INV-03 |
| `unit_links` | `unit_id`, `linked_from` (`same_as`), `undo_state_json` | Q-31: [отделить] |
| `merge_candidates` | `candidate_id`, `unit_ids_json`, `why`, `status` (`proposed`/`merged`/`dismissed`) | B32 |
| `fsrs_cards` | `unit_id`, `state`, `step`, `stability`, `difficulty`, `due`, `last_review`, `preset` (= `subject_id` или `default`) | crate `fsrs`; B32 |
| `review_log` | `review_id`, `unit_id`, `attempt_id` UNIQUE, `rating` (1–3), `source` (`verified`/`self_graded`), `reviewed_at` | INV-04, B27 |
| `references` | `unit_id`, `quote`, `event_id` — эталон для «Достань из памяти» | B27, B31 |

## 6. Фоновые механизмы и MRT

| Таблица | Поля | Откуда |
|---|---|---|
| `decisions` | `decision_id`, `mechanism` (`opportunity`/`daily`/`lightbulb`/`microgoal`/`mrt`), `trigger`, `eligible`, `ineligible_reason`, `randomized`, `probability`, `arm` (`show`/`silence`), `features_json`, `created_at` | v1 `director_decisions`, `opportunity_*`, `lightbulb_decisions`; F-57 |
| `decision_outcomes` | `decision_id`, `outcome_kind` (`engagement`/`verified_result`/`missing`), `value`, `observed_at` | B13, OD-06 |
| `reactions` | `decision_id`, `reaction` (`accepted`/`dismissed`/`snoozed`/`suppressed`/`never_again`) | v1 |
| `consents` | `scope` (`daily`/`mrt`/`episodic`), `granted`, `at` | INV-09 |

## 7. Настройки и служебное

| Что | Где |
|---|---|
| Ключ ИИ | Secret Service; запасной вариант — файл с правами 600 (Q-69) |
| Провайдер, модель, URL | `settings.json` |
| Промпты | `prompts/*.md` — данные, правятся без пересборки |
| Положение пилюли, масштаб текста, halo, звук, «уменьшить движение» | `settings.json` |
| Резервные копии | 3 последние перед миграцией (Q-70) |
| Архив v1 | отдельный каталог, только чтение (A17) |

## Эксперимент блоков (B80–B81)

| Таблица | Поля и контракт |
|---|---|
| `block_assignments` | `id` PK, `experiment`, `policy`, `lesson`, `subject` NULL, `material`, `block`, `role`, `allowed` JSON, `probabilities` JSON, `arm`, `probability`, `assigned_at`, `shown_at` NULL, `skipped_at` NULL, `completed_at` NULL, `completed_operation` NULL, `card_status` (`pending`/`created`/`card_failed`), `card_reason` NULL, `question` NULL, `reference` NULL; UNIQUE (`lesson`,`material`,`block`) |
| `block_assignment_checks` | (`lesson`,`material`) PK, `reason` (`assigned`/`no_eligible_roles`/`lesson_limit`), `at`; нет согласия — нет записи |
| `block_attempts` | `operation` PK, `assignment`, `attempt`, `revealed_at`; CLI раскрывает эталон после непустой попытки, self-grade требует сохранённую попытку |

Выполнение сохраняет триггер после вставки `material_interactions`: совпадают материал, урок, блок, назначенная функция; время строго после назначения. Старые взаимодействия с NULL material_id не подтверждают выполнение. Эксперимент не меняет таблицы прежнего MRT.

Карточка: `memory_cards.unit_id = block_assignments.id`, первое due = `assigned_at + 86400`. Оценки — существующие `memory_reviews`; первая запись даёт исход (rating/source/at и интервал `at-assigned_at`), остальные — дальнейшее удержание. При сбое создания карточки назначение остаётся в экспорте с `card_failed`, проверка missing. Молчаливые ветки тоже имеют карточки; показ/пропуск/выполнение не являются оценками и не двигают FSRS.
