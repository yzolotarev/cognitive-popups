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

## Text capture

```sh
cargo run -- capture --db journal.db            # the selection, or Ctrl+C with the clipboard put back
cargo run -- capture --from-clipboard
```

The selection comes first; if it is empty, an emulated Ctrl+C reads the window
and the old clipboard (text or image) is put back. Password managers' clipboards
are never read. Text from terminals, code editors and AI chats is marked and kept
out of study history. Needs `wl-clipboard` and Hyprland (`hyprctl`) or `wtype`.

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
