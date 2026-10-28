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
