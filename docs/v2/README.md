# Документы Cognitive Popups v2

Пакет подготовлен 28.10.2026 в репозитории `yzolotarev/methodology` (ветка `claude/modest-bardeen-ji82ir`). Где этот пакет расходится со спецификацией v0.2 (`docs/spec/`), действует пакет. Сверка — [`reconcile-rust-spec.md`](reconcile-rust-spec.md).

## Для разработки (начинать отсюда)
| Файл | Что это |
|---|---|
| [`SPEC.md`](SPEC.md) | Спецификация: продукт, стек, инварианты, системы S1–S7, окна, фоновые механизмы, данные |
| [`DESIGN.md`](DESIGN.md) | Визуальный дизайн по Apple HIG: шрифт, цвет, материалы, сетка, движение, фокус — единый источник для окон P7 |
| [`ROADMAP.md`](ROADMAP.md) | Этапы P0–P8 с критериями готовности; P0 — проба Rust + GTK 4 |
| [`DATA_MODEL.md`](DATA_MODEL.md) | Сущности новой базы |
| [`DAY_SCENARIO.md`](DAY_SCENARIO.md) | Сквозной день — приёмочный сценарий |
| [`AGENT_GUIDE.md`](AGENT_GUIDE.md) | Инструкция для агента-разработчика (`CLAUDE.md`) |
| [`port/prompts/`](port/prompts/README.md) | Промпты v1 для переноса (37 файлов) и новый промпт извлечения единиц |

## Решения и обоснования
| Файл | Что это |
|---|---|
| [`decisions.md`](decisions.md) | Журнал решений A1–A18, B1–B35; главный источник при противоречиях |
| [`function-map.md`](function-map.md) | Карта 101 функции v1 по шаблону: триггер, логика, выход, пример, крайние случаи |
| [`reconcile-rust-spec.md`](reconcile-rust-spec.md) | Сверка с Rust-репозиторием (OD-01…13, инварианты) |
| [`audit/young-methodology-audit.md`](audit/young-methodology-audit.md) | Методологический аудит по книгам Скотта Янга |

## Источники
| Файл | Что это |
|---|---|
| Apple HIG | Не копируется в публичный репозиторий: https://developer.apple.com/design/human-interface-guidelines/ (полный дамп — в репозитории `methodology`) |
| `sources/py-fsrs-README.md` | Документация FSRS (сроки, параметры, оптимизатор) |
| `sources/gemini-v1-inventory.md` | Инвентаризация v1 от Gemini; проверена частично, см. B33 |
| `prompts/gemini-v1-inventory.md` | Промпт, по которому делалась инвентаризация |
