# Промпты v1 для переноса

Выгружены дословно из кода v1 (28.10.2026). Промпты переносятся как **данные** (файлы), а не как код. Так их можно править без пересборки (B35).

- **PORT** — переносится, при переносе сверить с решениями в `docs/v2/decisions.md`.
- **DROP** — функция убрана.

| Имя | Ключ prompts.json | Модуль v1 | Статус |
|---|---|---|---|
| `PRACTICE_ANALYSIS_SYSTEM` | `practice_analysis` | `prompts.py` | PORT |
| `SEED_SYSTEM` | `four_words` | `prompts.py` | PORT |
| `FEYNMAN_SYSTEM` | `feynman_check` | `prompts.py` | PORT |
| `PREDICTION_SYSTEM` | `prediction` | `prompts.py` | PORT |
| `SUMMARY_SYSTEM` | `summary` | `prompts.py` | DROP — F-11 убрана (B23) |
| `CLARIFY_SYSTEM` | `clarify` | `prompts.py` | PORT |
| `CLARIFY_QUESTION_SYSTEM` | `clarify_question` | `prompts.py` | PORT |
| `CLARIFY_UNIVERSE_SYSTEM` | `clarify_universe` | `prompts.py` | PORT |
| `NODES_SYSTEM` | `prediction_nodes` | `prompts.py` | PORT |
| `LADDER_SYSTEM` | `ladder` | `prompts.py` | PORT |
| `QUESTION_SYSTEM` | `feynman_question` | `prompts.py` | PORT |
| `INTEGRATIVE_EXTRACT_SYSTEM` | `integrative_extract` | `prompts.py` | PORT |
| `INTEGRATIVE_MERGE_SYSTEM` | `integrative_merge` | `prompts.py` | PORT |
| `INTEGRATIVE_COMPOSE_SYSTEM` | `integrative_compose` | `prompts.py` | PORT |
| `EXAMPLE_SYSTEM` | `example` | `prompts.py` | PORT |
| `REFRAME_SYSTEM` | `reframe` | `prompts.py` | PORT |
| `GOAL_SYSTEM` | `goal` | `prompts.py` | PORT |
| `CHARACTER_SYSTEM` | `character` | `prompts.py` | PORT |
| `STEP_GOAL_SYSTEM` | `step_goal` | `prompts.py` | PORT |
| `SHAPE` | `—` | `probe_generator.py` | PORT |
| `DRILL_CONTRACT` | `—` | `probe_generator.py` | PORT |
| `BRIEF` | `—` | `probe_generator.py` | PORT |
| `FORMAT` | `—` | `probe_generator.py` | PORT |
| `SCHEMA` | `—` | `universe.py` | не промпт — схема таблицы |
| `NAMING_SYSTEM` | `—` | `universe.py` | PORT |
| `ALIAS_SYSTEM` | `—` | `universe.py` | PORT |
| `PASSAGE_SYSTEM` | `—` | `universe.py` | PORT |
| `SYSTEM_PROMPT` | `—` | `recycling.py` | DROP — F-15 убрана (B22) |
| `GENERATOR` | `—` | `focus.py` | DROP — F-87 наследие (B32) |
| `CHECKER` | `—` | `focus.py` | DROP — F-87 наследие (B32) |
| `FORM_RULES` | `—` | `tasks.py` | PORT |
| `_SCHEMA` | `—` | `micro_clarify.py` | не промпт — схема таблицы |
| `LANGUAGE_RULES` | `—` | `language.py` | PORT |
| `CUE_LANGUAGE_RULES` | `—` | `language.py` | PORT |
| `build_probe_prompt()` | `—` | `probes.py` | PORT |
| `generation_messages()` | `—` | `lessons.py` | PORT |
| `TYPES` (10 типов) | — | `tasks.py` | PORT |
