# Cognitive Exoskeleton

Cross-platform desktop application for supporting your own thinking and learning while
you work in a browser, a PDF, an editor or an external trainer. The product contract is
the [versioned specification](docs/spec/cognitive_exoskeleton_v0_2_full_spec.md);
decisions in it are not replaced by this README.

Current state: **P2 in progress**. P0 delivered the versioned specification and the
traceability baseline; the first P2 slice provides the typed event journal with
provenance and idempotent writes. There is still no working desktop application and no
GUI framework has been selected. Legacy Python code remains in Git history.

## Пакет v2 (28.10.2026)

Актуальный пакет для разработки — [`docs/v2/`](docs/v2/README.md): спецификация `SPEC.md`, план `ROADMAP.md` (этапы P0–P8, P0 — проба Rust + GTK 4), модель данных, приёмочный день, журнал решений B1–B35, карта 101 функции v1 и промпты v1 для переноса. Где пакет расходится со спецификацией v0.2 ниже, действует пакет; сверка — [`docs/v2/reconcile-rust-spec.md`](docs/v2/reconcile-rust-spec.md). Инструкция для агента — [`CLAUDE.md`](CLAUDE.md).
## Journal from the command line

```sh
cargo run -- log add --db /tmp/j.sqlite3 --json '{"operation":"op-1","key":"k-1","lesson":"L1","event":{"payload":"user_text","text":"моя гипотеза"}}'
cargo run -- log lesson --db /tmp/j.sqlite3 L1
```

`log lesson` prints the lesson transcript that the end-of-lesson units prompt reads:
one line per event, `[e<n>]` id, then `->user:`, `->ai generated for this task:` or
`sth else:`. Text from development tools and AI chats is stored but never printed.

## AI client from the command line

```sh
chmod 600 ~/.config/cognitive-popups/ai.key
export COGNITIVE_AI_KEY_FILE=~/.config/cognitive-popups/ai.key   # Gemini by default
cargo run -- ai check
cargo run -- ai ask --prompts docs/v2/port/prompts --prompt seed_system --input-file paragraph.txt --json
```

Bring your own key: Gemini (default model `gemini-flash-lite-latest`) or any
OpenAI-compatible service (`COGNITIVE_AI_PROVIDER=openai_compatible`,
`COGNITIVE_AI_BASE_URL`, `COGNITIVE_AI_MODEL`). Without a key nothing is sent. A key file
readable by other users is refused. When the main model is overloaded, the client waits out a short
rate limit once and then tries the fallback models of the same provider
(`COGNITIVE_AI_FALLBACK_MODELS`; Gemini has defaults); the answer names the model
that gave it. Prompts are read from Markdown files (first
```` ```text ```` block), so they change without a rebuild.

## Learning units from the command line

```sh
cargo run -- units extract --db journal.db --prompts docs/v2/port/prompts --subject linal --lesson L1
cargo run -- units list --db journal.db --subject linal
cargo run -- units separate --db journal.db <link-id>
```

At the end of a lesson the model proposes units from the transcript; the code
keeps only what it can check: cited events exist, at least one of them was written by
the reader, `same_as` names a known unit, the kind is one of eight, merge candidates
are made only of units the reader marked as mastered. A lesson is extracted once.
Units live in the journal's database file, in their own tables.

## Memory (FSRS) from the command line

```sh
cargo run -- memory due --db journal.db                       # FSRS! N and the list
cargo run -- memory review --db journal.db <unit> good --source self_graded --key attempt-17
cargo run -- memory note --db journal.db <unit> self_report --key attempt-18   # «не помню»
cargo run -- memory goal --db journal.db --subject linal --deadline 2026-12-20T00:00:00Z
cargo run -- memory optimize --db journal.db --subject linal
cargo run -- units merge --db journal.db --title … --action … --kind transfer <unit> <unit>
cargo run -- units unmerge --db journal.db <merged-unit>
```

One schedule for everything: FSRS-6 (`fsrs` crate) with desired retention 0.9,
learning steps 1 and 10 minutes, relearning 10 minutes. A grade comes only from
code (`verified`) or from the reader comparing with the reference (`self_graded`);
there is no Easy and no way for a model to grade. «Не помню», skips and model
verdicts are kept as evidence without touching the schedule. Each attempt carries a
key, so a repeated submission is one review. A goal due within 7 days takes the
queue; a removed goal pauses its units. A subject trains its own 21 weights after
400 reviews, at most monthly. A merged unit starts from its weakest part; its
parts wait unchanged until it is undone.

## Practice probes (Alt+T) from the command line

```sh
cargo run -- practice probes --text crates/practice/fixtures/01-gauss.txt --prompts docs/v2/port/prompts
```

Three operations found in the text, each with a worked example, three cards
and a reserve for [Ещё 3] and the diagnostic case after a miss. The model
proposes, the code decides what is shown: the citation must be verbatim in the
text; numeric cards are computed by the code from the model's template (exact
fractions), and a template whose check fails on any case is rejected;
arithmetic in drill pairs is computed exactly; the meaning of the rest is checked
by a verifier model that can only remove cards. Rejected branches are asked for
again once, with the reason.

```sh
cargo run -- practice tree --db journal.db --prompts docs/v2/port/prompts --text page.txt
cargo run -- practice tree --db journal.db --prompts docs/v2/port/prompts --from-pill
cargo run -- memory review --db journal.db <unit> совпало --source self_graded --key attempt-21
```

`practice tree` adds «Достань из памяти» when units are due: one card per unit,
a question to answer from memory and a reference quoted verbatim from the unit's
own journal events, hidden until the reader answers. It replaces the third probe
category, comes first when opened from the «FSRS! N» pill, and is the only one
without a text. The reader grades by comparing: промахнулся / частично / совпало.

```sh
cargo run -- practice whole --prompts docs/v2/port/prompts --text page.txt --ops "Вычесть кратное; Найти множитель"
cargo run -- practice whole --prompts docs/v2/port/prompts --db journal.db --unit <unit> --type 7
```

[Собери вместе]: one whole task of the ten v1 types, with a worked solution
hidden until asked for. A newcomer to the unit gets worked-example types only;
anchors must be quoted verbatim from the material; plain text, no LaTeX.

```sh
cargo run -- practice methods --prompts docs/v2/port/prompts --text crates/practice/fixtures/06-methods.txt
```

Trails: «Выбор метода» and «Различи методы». The reader names the method to start
with and the feature of the condition that shows it. The criterion is fixed
before the answer; a named acceptable method is a code-checked success, anything
else the reader compares with the reference.

```sh
cargo run -- units family --db journal.db <unit> integer_roots
cargo run -- practice check --family integer_roots --seed <seed> --answer "−1; 3; 6"
```

A unit linked to a code-checked family (`integer_roots`, `work_proportion`) gets a
fresh task instance in «Достань из памяти» without a model request, and the code
grades the answer (`verified`).

## Background nudges and the MRT log

```sh
cargo run -- nudge consent --db journal.db opportunity on
cargo run -- nudge check --db journal.db --kind opportunity --topic hypothesis-17
cargo run -- mrt decide --db journal.db --key <error-event> --unit <unit>
cargo run -- mrt check --db journal.db --unit <unit> --correct
python3 tools/mrt_analysis.py journal.db
```

Nothing speaks first without consent. Opportunity and Daily share one budget
(not while busy or watching a video, ≤ 3 a day, 2 h apart, 24 h after «Не сейчас»),
and every silence says why. The micro-randomized trial randomizes only meaningful
moments, keeps engagement apart from the verified result, and the analysis refuses
a conclusion before an A/A check and enough data.
## Text capture

```sh
cargo run -- capture --db journal.db            # the selection, or Ctrl+C with the clipboard put back
cargo run -- capture --from-clipboard
```

The selection comes first; if it is empty, an emulated Ctrl+C reads the window
and the old clipboard (text or image) is put back. Password managers' clipboards
are never read. Text from terminals, code editors and AI chats is marked and kept
out of study history. Needs `wl-clipboard` and Hyprland (`hyprctl`) or `wtype`.

## Windows (P7, Linux)

`apps/desktop` is the GTK 4 + layer-shell app; a separate cargo workspace, since
the core also builds on Windows. One daemon keeps every window ready; a hotkey
only sends it a signal.

On Arch Linux install Rust, `gtk4`, `libadwaita`, `gtk4-layer-shell`,
`wl-clipboard`, `wtype`, and `playerctl`, then from the repository root run `make install`.
This installs the app to `~/.local/bin` and prompts and a desktop entry under
`~/.local/share`. Ensure `~/.local/bin` is on your PATH. Run
`cognitive-popups doctor` to check your session and dependencies.

Print `cognitive-popups hyprland-config --lua` for Hyprland 0.56 Lua, or
`cognitive-popups hyprland-config --classic` for the classic configuration.
Review the shortcuts for conflicts and add the appropriate output to your
Hyprland configuration, then restart the session. The application menu entry
opens the hub while the daemon is running; enter your AI key on its AI tab.
The installed example blocks are also in `~/.local/share/cognitive-popups/`.

For development, run `apps/desktop/tools/screenshots.sh shots/` from the
repository root to capture every window in both themes.

Alt+E is a note at the selected text: the selection becomes the quote, Enter
saves, Shift+Enter is a new line, Esc closes and keeps the draft.

Alt+W helps to understand the selection: four words (70 %), an explanation or an
answer to a question (10 %), an example (10 %), a step from what you know (5 %)
or a story by one fixed character (5 %); the row at the bottom switches by hand.
[Сначала сам] hides the model's four words until you write yours; matches are
highlighted by code, never scored. Without windows:
`cargo run -- understand words|explain|example|ladder|character --text <file> --prompts docs/v2/port/prompts`.
Ctrl+Alt+P records your guess about the selection first, then checks it: a
short corrective answer with a verbatim quote; [Подробнее] shows where it parts
from the text, [Другой ракурс] one other way to see it. A vague guess is asked to
be bolder once; a refuted one returns in FSRS reviews as «how it really is».
Without windows: `cargo run -- hypothesis check --db journal.db --prompts docs/v2/port/prompts --text <file> <guess…>`.

Alt+T is practice. Usually probes: up to three categories of one small
operation each, a worked example always visible, three cards with ✓/✗ before
[Ответ] (your own attempt) and «Совпало / Промахнулся» after it; the highlighted
next step follows rules in code. When FSRS reviews are due, «Достань из памяти»
replaces the third category: you answer first, then compare with a quote from
your own materials. The bottom row switches to method choice, «своими словами»
or a hypothesis.
Three more routes come from the stores and are drawn only when they have
something real: «продолжить задачу» reopens a whole task you set aside, with
your draft and the solution still hidden; «старый материал» offers an older
study fragment as probes again; «умение» asks one unit you have not answered
correctly for a week, if its goal is live. What cannot be offered says why, and
the hub can take any route out of the draw. Without windows:
`cargo run -- practice routes|route|task|old|skill --db journal.db`.

Alt+I is a ~15-minute lesson. The first time it asks for a subject and your
long-term goal (Alt+S). Then it builds a goal for the next ~15 minutes and a
plan of 2–5 items, at once or from your own thoughts; items can be made current
or skipped, never edited, and [Другой план] replaces the whole plan. A step runs
under a small clock in the corner; at its end a quiet window asks for one mark
and one line «что вынес». An unfinished lesson comes back on the next Alt+I.
When the lesson ends, its events become learning units in the background and a
notice in the corner says how many. Without windows:
`cargo run -- subject add|list`, `lesson plan|show|item|close`, `step start|finish|bridge`.

To set up all subjects at once, write them in one text file and run
`cognitive-popups subjects import plan.md` (check first with `--dry-run`;
`cognitive-popups subjects export` prints the current plan in the same format):

```text
# Теория вероятностей
Цель: решать задачи на Байеса и ЦПТ без подсказок
Срок: 2026-12-14
- Комбинаторика
- Формула Байеса
```

Importing again keeps your marks and removes nothing.

The hub opens with Alt+S, from the tray icon (or `cognitive-popups signal hub`): AI key,
provider and model with [Проверить]; subjects and their long-term goals; what
may appear by itself (all off by default); units with [Освоил], [Объединить]
and [Отделить]; the history of guesses, notes, lessons and the trial. The key
goes to the Secret Service keyring (`secret-tool`), else to a file only you can
read. With consent, a quiet «FSRS! N» pill in the corner opens due reviews.

Alt+B returns you to where you stopped: the last document you selected text in
(Firefox, Okular with its page, Obsidian) and your own last word about it — a
refuted guess first. Alt+K lists the keys. Manual end-to-end check:
[`docs/v2/E2E_P7.md`](docs/v2/E2E_P7.md).

Formulas: the model writes LaTeX between `\( … \)` or `\[ … \]`; a strict parser
(`crates/formula`) accepts a known subset and Typst, built into the app with its
fonts, draws it. What does not parse is shown as readable Unicode, never as
LaTeX source. Check one: `cargo run -- formula '\frac{1}{2x}'`.

`COGNITIVE_AI_FIXTURE=apps/desktop/tools/fixture.json` replaces the model with
canned answers (screenshots, demos without a key). Notes also
work without windows: `cargo run -- note add|list|close`. Needs `gtk4-layer-shell`.

## Requirements baseline (P0)

| Artefact | Purpose |
|---|---|
| [`docs/spec/…v0_2_full_spec.md`](docs/spec/cognitive_exoskeleton_v0_2_full_spec.md) | Product specification, version 0.2 (27.10.2026) |
| [`docs/requirements/registry.md`](docs/requirements/registry.md) | Every requirement ID, status, stage, implementation and evidence |
| [`docs/decisions/open-decisions.md`](docs/decisions/open-decisions.md) | OD-01…OD-13 cards: options, consequences, blocked requirements |
| [`docs/requirements/legacy-inventory.md`](docs/requirements/legacy-inventory.md) | Evidence-based KEEP/CHANGE/DROP input for historical features |
| [`docs/quality/acceptance-scenarios.md`](docs/quality/acceptance-scenarios.md) | End-to-end scenarios E2E-01…12 and evidence format |
| [`docs/spikes/platform-matrix.md`](docs/spikes/platform-matrix.md) | Proposed supported environments, platform probes S1–S8 |

`tests/traceability.rs` fails CI when a requirement ID from the specification is missing
from the registry, appears twice, or changes status without the specification changing.

## Development

Install [rustup](https://rustup.rs/), then run:

```sh
cargo run --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked --release
```

The toolchain and lint components are pinned in `rust-toolchain.toml`.
Commit `Cargo.lock` when changing dependencies.

## GitHub Actions

Every pull request and push to `main` runs formatting, dependency auditing,
Clippy, tests, release builds and executable smoke tests. Builds run natively on
Windows Server 2022 and Ubuntu 24.04 (x86-64). Download the resulting executables
from the run's **Artifacts** section; these are development scaffolds, not installers.
Linux executables are built against Ubuntu 24.04 and are not promised to run on
older distributions. Artifact downloads may require restoring executable permission
on Linux (`chmod +x cognitive-exoskeleton`).

CI uses no LLM keys, personal data or repository secrets. Hosted runners do not
validate desktop interactions, global hotkeys, selection capture, Wayland/X11
compatibility or Windows permissions. Those rows close only through the protocol in
`docs/spikes/platform-matrix.md`, not through a green CI run.

Repository administrators must require these checks in the `main` branch rule:

- `Rust formatting`
- `Build and test (Linux)`
- `Build and test (Windows)`
- `Dependency audit`

Installer packaging, code signing and GitHub Releases are not configured yet.
